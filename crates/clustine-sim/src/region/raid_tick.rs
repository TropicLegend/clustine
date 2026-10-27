//! A raid's tick by its rules (`Raid.tick`, n7k Raid 452–945; V 7.2 with D10's part C
//! corrections; `docs/groundwork/n10-plan.md`, section 9.7): the countdown with its
//! progress, the spawn search, the waves (`spawnGroup` with `Raid$RaiderType`'s counts
//! and the bonus draws, the leader's banner, the ravager's riders by wave number), the
//! members judged (`updateRaiders`), the bar's progress and name, victory and loss, Hero
//! of the Village to every hero, the celebrations, the horn, the 48 000-tick limit, and
//! peaceful's stop. The frame (`raids.rs`) calls it for each raid of the land in the
//! tick's order; it also takes a raider's word of its raid to the raid's record
//! ([`Region::raider_news`]) and answers a raider's view of a raid
//! ([`Region::raid_known`]).
//!
//! Every draw of the raid's random is the raid's stream (`raids.rs`), in the game's
//! order: per spawn search one `nextFloat` and two `nextInt(3)` per attempt; per group
//! the types' bonus draws, each type's raiders made before the next type's draws; the
//! horn's `nextLong`. The raiders' `finalizeSpawn` draws the level's random, which is a
//! stream of the spawn position and the game time (`Random::of_block`), and their own
//! streams; a raider's buffs draw its own.
//!
//! **Members**: the raid judges those it holds in its land (`updateRaiders`): one 112 or
//! more from the centre leaves it, and one idle outside a village (its idle time above
//! 2400 once it has lived 600 ticks) for 30 passes. A member it cannot see is alive until
//! its holder says it died or left (section 9.3); its holder judges it the same
//! (`crate::mobs::ai::raider_goals`).
//!
//! Named differences: the game rule `raids` is always on and the world never peaceful,
//! so neither stops a raid; a spawn position is taken only in the region's land
//! (`isPositionEntityTicking`), the square of ten about it in the land or the copies; a
//! wave's raiders face south, where the game leaves their constructors' yaw, and are not
//! put on the ground (`setOnGround(true)`) but fall the block they are placed above it;
//! the horn is a sound at a place, 13 blocks from each player towards the spawn point,
//! which every player near that place hears (the game sends each player their own); the
//! bar's progress is worked out again at each tick of the raid from the health its
//! members last told it of, where the game reads them at each hurt, join and removal; a
//! bar of a raid with no health counted keeps its progress (the game's would be not a
//! number); the raiders' sets and the heroes are walked by key.

use clustine_data::{entity_types, sounds};
use clustine_noise::trig;
use clustine_world::components::EffectInstance;
use clustine_world::raid::{Raid, RaidBarName, RaidMember, RaidStatus};
use clustine_world::{
    BlockPos, ChunkPos, Difficulty, EntityId, EntityUse, Float, GivenEffects, RaidNews,
};

use super::Region;
use super::raids::RaidAt;
use crate::api::{RegionEvent, TickOutput};
use crate::mobs::Mob;
use crate::mobs::ai::maths::floor_f32;
use crate::mobs::ai::raider_goals::{IDLE_LIMIT, MEMBER_REACH_SQR, RaidKnown, RaiderNews};
use crate::mobs::body::effects::{Effect, Layer};
use crate::mobs::making::{finalize, moon_brightness};
use crate::mobs::types::{SpawnReason, Spawning, illagers};
use crate::physics::Vec3;
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::region::Random;
use crate::spawning::rules::{Light, SpawnWorld, is_spawn_position_ok};

/// `Raid`'s limit: stopped at 48 000 active ticks.
const MOST_TICKS: u64 = 48_000;
/// The countdown before a wave (`raidCooldownTicks` from 300).
const COUNTDOWN: i32 = 300;
/// The ticks after the last wave's end before victory.
const POST_RAID_TICKS: i32 = 40;
/// How long a won or lost raid celebrates before it stops.
const CELEBRATION_TICKS: i32 = 600;
/// Hero of the Village's length.
const HERO_TICKS: i32 = 48_000;
/// How far the horn is heard about the spawn point, horizontally.
const HORN_REACH: f64 = 64.0;
/// The purpose of the level's stream a wave's raiders draw (`Random::of_block`).
const RAID_LEVEL: u64 = 0x7261_6964_6c76;

/// `Raid$RaiderType`, in its order (n10 Raid$RaiderType): the type and its
/// `spawnsPerWaveBeforeBonus`, indexed by wave (0 never used).
const RAIDER_TYPES: [(i32, [i32; 8]); 5] = [
    (entity_types::VINDICATOR, [0, 0, 2, 0, 1, 4, 2, 5]),
    (entity_types::EVOKER, [0, 0, 0, 0, 0, 1, 1, 2]),
    (entity_types::PILLAGER, [0, 4, 3, 3, 4, 4, 4, 2]),
    (entity_types::WITCH, [0, 0, 0, 0, 3, 0, 0, 1]),
    (entity_types::RAVAGER, [0, 0, 0, 1, 0, 1, 0, 2]),
];

/// `Raid.getNumGroups(NORMAL)` and `(HARD)`, by which a ravager's rider is chosen.
const NORMAL_GROUPS: i32 = 5;
const HARD_GROUPS: i32 = 7;

fn kind(id: i32) -> u16 {
    u16::try_from(id).expect("entity type ids fit 16 bits")
}

/// `hasBonusWave()`: an omen above 1.
fn has_bonus_wave(raid: &Raid) -> bool {
    raid.omen_level > 1
}

/// `isFinalWave()`.
fn is_final_wave(raid: &Raid) -> bool {
    raid.groups_spawned == raid.group_count
}

/// `hasSpawnedBonusWave()`.
fn has_spawned_bonus_wave(raid: &Raid) -> bool {
    raid.groups_spawned > raid.group_count
}

/// `hasMoreWaves()`.
fn has_more_waves(raid: &Raid) -> bool {
    if has_bonus_wave(raid) {
        !has_spawned_bonus_wave(raid)
    } else {
        !is_final_wave(raid)
    }
}

/// `getTotalRaidersAlive()`.
fn alive(raid: &Raid) -> i32 {
    i32::try_from(raid.members.len()).unwrap_or(i32::MAX)
}

/// `shouldSpawnBonusGroup()`.
fn should_spawn_bonus_group(raid: &Raid) -> bool {
    is_final_wave(raid) && alive(raid) == 0 && has_bonus_wave(raid)
}

/// `shouldSpawnGroup()`.
fn should_spawn_group(raid: &Raid) -> bool {
    raid.cooldown_ticks == 0
        && (raid.groups_spawned < raid.group_count || should_spawn_bonus_group(raid))
        && alive(raid) == 0
}

/// `stop()`: inactive, its bar taken from everyone, stopped.
fn stop(raid: &mut Raid) {
    raid.active = false;
    raid.bar_shown = false;
    raid.status = RaidStatus::Stopped;
}

/// `updateBossbar()`: the bar's progress, the health of the members as they last told
/// it against the raid's total, kept within 0 and 1; kept as it is where no health was
/// counted.
fn update_bossbar(raid: &mut Raid) {
    if raid.total_health.0 <= 0.0 {
        return;
    }
    // From a positive nought, as the game's float sum starts: an empty sum is not −0.
    let living = raid
        .members
        .iter()
        .fold(0.0_f32, |sum, member| sum + member.health.0);
    raid.bar_progress = Float((living / raid.total_health.0).clamp(0.0, 1.0));
}

/// `getPotentialBonusSpawns(type, random, wave, difficulty, bonus)`: the vindicators' and
/// pillagers' `nextInt(2)` on easy (then `nextInt(n + 1)` where it is above 0), one or
/// two more on normal and hard; the witches' from the third wave but the fourth, not on
/// easy; the ravagers' of a bonus wave, not on easy; the evokers' none.
fn potential_bonus_spawns(
    raider: i32,
    random: &mut Random,
    wave: i32,
    difficulty: Difficulty,
    bonus: bool,
) -> i32 {
    let easy = difficulty == Difficulty::Easy;
    let normal = difficulty == Difficulty::Normal;
    let n = match raider {
        entity_types::VINDICATOR | entity_types::PILLAGER => {
            if easy {
                random.next_int(2) as i32
            } else if normal {
                1
            } else {
                2
            }
        }
        entity_types::WITCH => {
            if easy || wave <= 2 || wave == 4 {
                return 0;
            }
            1
        }
        entity_types::RAVAGER => i32::from(!easy && bonus),
        _ => return 0,
    };
    if n > 0 {
        random.next_int((n + 1) as u32) as i32
    } else {
        0
    }
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

/// The region as the ravager's placement reads it (`findRandomSpawnPos`).
struct RaidWorld<'r>(&'r Region);

impl SpawnWorld for RaidWorld<'_> {
    fn block(&self, pos: BlockPos) -> Option<clustine_data::BlockState> {
        self.0.block(pos)
    }

    fn light(&self, pos: BlockPos) -> Option<Light> {
        let light = self.0.light_at(pos)?;
        Some(Light {
            sky: light.sky,
            block: light.block,
        })
    }

    fn sky_darken(&self) -> i32 {
        crate::time::sky_darken(self.0.clock.day)
    }

    fn biome(&self, _: BlockPos) -> Option<clustine_world::Biome> {
        None
    }

    fn surface(&self, x: i32, z: i32) -> Option<i32> {
        self.0.world_surface(x, z)
    }

    fn game_seed(&self) -> i64 {
        self.0.config.game_seed
    }

    fn day(&self) -> u64 {
        self.0.clock.day
    }

    fn sea_level(&self) -> i32 {
        self.0.config.sea_level
    }
}

impl Region {
    /// `Raid.tick` of the raid `id`, kept in the chunk `chunk`: run on a copy of its
    /// record, its stream keyed by the active tick it draws in, and kept back where it
    /// changed, in the chunk of its centre where that is now another chunk of the land.
    pub(super) fn tick_raid(&mut self, chunk: ChunkPos, id: EntityId, output: &mut TickOutput) {
        let Some((_, kept)) = self.find_raid(id) else {
            return;
        };
        let mut raid = kept.clone();
        let before = raid.clone();
        let start = self.raid_stream(id, raid.ticks_active.saturating_add(1));
        let mut stream = start.clone();
        self.run_raid(&mut raid, &mut stream, output);
        self.raid_drew(id, &start, &stream);
        if raid == before {
            return;
        }
        // A stopped raid stays where the frame takes it away from.
        let home = raid.centre.chunk();
        if home != chunk && self.chunks.contains_key(&home) && !raid.is_stopped() {
            self.remove_raid(chunk, id, output);
            self.put_raid(home, raid, output);
        } else {
            self.put_raid(chunk, raid, output);
        }
    }

    /// `Raid.tick(level)`.
    fn run_raid(&mut self, raid: &mut Raid, stream: &mut Random, output: &mut TickOutput) {
        match raid.status {
            RaidStatus::Stopped => {}
            RaidStatus::Ongoing => self.raid_ongoing(raid, stream, output),
            RaidStatus::Victory | RaidStatus::Loss => {
                raid.celebration_ticks += 1;
                if raid.celebration_ticks >= CELEBRATION_TICKS {
                    stop(raid);
                    return;
                }
                if raid.celebration_ticks % 20 == 0 {
                    raid.bar_shown = true;
                    if raid.status == RaidStatus::Victory {
                        raid.bar_progress = Float(0.0);
                        raid.bar_name = RaidBarName::Victory;
                    } else {
                        raid.bar_name = RaidBarName::Defeat;
                    }
                }
            }
        }
    }

    /// `Raid.tick` of an ongoing raid.
    fn raid_ongoing(&mut self, raid: &mut Raid, stream: &mut Random, output: &mut TickOutput) {
        // `active` is the frame's (`hasChunkAt(centre)`); the bar is shown as it is.
        raid.bar_shown = raid.active;
        if Difficulty::WORLD == Difficulty::Peaceful {
            stop(raid);
            return;
        }
        if !raid.active {
            return;
        }
        if !self.close_to_village(raid.centre, 1) {
            self.move_raid_centre(raid);
        }
        if !self.close_to_village(raid.centre, 1) {
            if raid.groups_spawned > 0 {
                raid.status = RaidStatus::Loss;
            } else {
                stop(raid);
            }
        }
        raid.ticks_active += 1;
        if raid.ticks_active >= MOST_TICKS {
            stop(raid);
            return;
        }
        // What other regions told it since its last tick, on the bar (where no countdown
        // shows its own progress).
        if raid.groups_spawned > 0
            && !(raid.members.is_empty() && has_more_waves(raid) && raid.cooldown_ticks > 0)
        {
            update_bossbar(raid);
        }
        let alive = alive(raid);
        if alive == 0 && has_more_waves(raid) {
            if raid.cooldown_ticks > 0 {
                let has = raid.spawn_pos.is_some();
                let mut search = !has && raid.cooldown_ticks % 5 == 0;
                if let Some(pos) = raid.spawn_pos
                    && !self.ticks_chunk(pos.chunk())
                {
                    search = true;
                }
                if search {
                    raid.spawn_pos = self.find_random_spawn_pos(raid, stream, 8);
                }
                raid.cooldown_ticks -= 1;
                let progress = (COUNTDOWN - raid.cooldown_ticks) as f32 / 300.0;
                raid.bar_progress = Float(progress.clamp(0.0, 1.0));
            } else if raid.cooldown_ticks == 0 && raid.groups_spawned > 0 {
                raid.cooldown_ticks = COUNTDOWN;
                raid.bar_name = RaidBarName::Raid;
                return;
            }
        }
        if raid.ticks_active.is_multiple_of(20) {
            self.update_raiders(raid, output);
            raid.bar_name = if alive > 0 && alive <= 2 {
                RaidBarName::RaidersRemaining(alive)
            } else {
                RaidBarName::Raid
            };
        }
        let mut horn = false;
        let mut failures = 0;
        while should_spawn_group(raid) {
            let found = match raid.spawn_pos {
                Some(pos) => Some(pos),
                None => self.find_random_spawn_pos(raid, stream, 20),
            };
            match found {
                Some(pos) => {
                    raid.started = true;
                    self.spawn_group(raid, pos, stream, output);
                    if !horn {
                        self.sound_horn(raid, pos, stream, output);
                        horn = true;
                    }
                }
                None => failures += 1,
            }
            if failures > 5 {
                stop(raid);
                break;
            }
        }
        if raid.started && !has_more_waves(raid) && alive == 0 {
            if raid.post_raid_ticks < POST_RAID_TICKS {
                raid.post_raid_ticks += 1;
            } else {
                raid.status = RaidStatus::Victory;
                self.give_heroes(raid, output);
            }
        }
    }

    /// `moveRaidCenterToNearbyVillageSection`: of the 5 × 5 × 5 sections about the
    /// centre's (x fastest, then y, then z), the middle of the nearest that is a village
    /// (`isVillage(SectionPos)`), the first of equals.
    fn move_raid_centre(&self, raid: &mut Raid) {
        let section = super::pois::section_of(raid.centre);
        let mut nearest: Option<(f64, BlockPos)> = None;
        for dz in -2..=2 {
            for dy in -2..=2 {
                for dx in -2..=2 {
                    let middle = BlockPos::new(
                        (section[0] + dx) * 16 + 8,
                        (section[1] + dy) * 16 + 8,
                        (section[2] + dz) * 16 + 8,
                    );
                    if !self.close_to_village(middle, 1) {
                        continue;
                    }
                    let distance = super::pois::dist_sqr(middle, raid.centre);
                    if nearest.is_none_or(|(least, _)| distance < least) {
                        nearest = Some((distance, middle));
                    }
                }
            }
        }
        if let Some((_, middle)) = nearest {
            raid.centre = middle;
        }
    }

    /// `Level.getHeight(WORLD_SURFACE, x, z)`: the first free height above the highest
    /// block that is not air, where the region reads the column.
    fn world_surface(&self, x: i32, z: i32) -> Option<i32> {
        let pos = BlockPos::new(x, 0, z);
        let chunk = self.readable_chunk(pos.chunk())?;
        let (x, z) = pos.in_chunk();
        Some(chunk.min_y() + i32::from(chunk.heights().world_surface[z << 4 | x]))
    }

    /// `hasChunksAt(x0, z0, x1, z1)`: every chunk the square reaches is read.
    fn reads_chunks_at(&self, low: BlockPos, high: BlockPos) -> bool {
        let (low, high) = (low.chunk(), high.chunk());
        (low.x..=high.x).all(|x| (low.z..=high.z).all(|z| self.readable(ChunkPos::new(x, z))))
    }

    /// `findRandomSpawnPos(level, attempts)` (n7k Raid 1666–1825, D10 C.6): about the
    /// centre at a distance the countdown sets, `attempts` turns of an eighth of a half
    /// turn on from a random angle, each with its two `nextInt(3)` drawn before any
    /// refusal; a place on the surface within 96 of the centre's height, not in a village
    /// while the countdown is at 160 or more, every chunk within ten of it read, ticking,
    /// where a ravager may stand or on snow in air.
    fn find_random_spawn_pos(
        &self,
        raid: &Raid,
        stream: &mut Random,
        attempts: i32,
    ) -> Option<BlockPos> {
        let countdown = raid.cooldown_ticks / 20;
        let reach = 0.22_f32 * countdown as f32 - 0.24_f32;
        let angle = stream.next_float() * 6.283_185_5_f32;
        let centre = raid.centre;
        for attempt in 0..attempts {
            let turned = angle + std::f32::consts::PI * attempt as f32 / 8.0_f32;
            let x = centre.x
                + floor_f32(trig::cos(f64::from(turned)) * 32.0 * reach)
                + stream.next_int(3) as i32 * floor_f32(reach);
            let z = centre.z
                + floor_f32(trig::sin(f64::from(turned)) * 32.0 * reach)
                + stream.next_int(3) as i32 * floor_f32(reach);
            let Some(y) = self.world_surface(x, z) else {
                continue;
            };
            if (y - centre.y).abs() > 96 {
                continue;
            }
            let pos = BlockPos::new(x, y, z);
            if self.close_to_village(pos, 1) && countdown > 7 {
                continue;
            }
            if !self.reads_chunks_at(pos.offset(-10, 0, -10), pos.offset(10, 0, 10)) {
                continue;
            }
            if !self.ticks_chunk(pos.chunk()) {
                continue;
            }
            let placed = is_spawn_position_ok(kind(entity_types::RAVAGER), &RaidWorld(self), pos);
            let on_snow = self
                .block(pos.offset(0, -1, 0))
                .and_then(|below| below.block())
                .is_some_and(|block| block.name == "minecraft:snow")
                && self.block(pos).is_some_and(|here| here.props().is_air());
            if placed || on_snow {
                return Some(pos);
            }
        }
        None
    }

    /// `spawnGroup(level, pos)` (n7k Raid 1212–1375, D10 C §1.2): the wave's raiders of
    /// each type in `Raid$RaiderType`'s order, each type's count its default for the wave
    /// (the final wave's for a bonus wave) and the bonus drawn of the raid's stream just
    /// before its raiders are made; the first that can lead wears the banner and leads
    /// the wave; a ravager of wave 5 carries a pillager, of wave 7 on an evoker (the
    /// wave's first ravager) or a vindicator. A raider that cannot be made ends its
    /// type's. Then the wave is counted, its place forgotten and the bar its health.
    fn spawn_group(
        &mut self,
        raid: &mut Raid,
        pos: BlockPos,
        stream: &mut Random,
        output: &mut TickOutput,
    ) {
        let wave = raid.groups_spawned + 1;
        raid.total_health = Float(0.0);
        let bonus = should_spawn_bonus_group(raid);
        let mut level = Random::of_block(self.config.seed, pos, self.clock.game_time, RAID_LEVEL);
        let mut led = false;
        for (raider, defaults) in RAIDER_TYPES {
            let index = if bonus { raid.group_count } else { wave };
            let default = usize::try_from(index)
                .ok()
                .and_then(|index| defaults.get(index))
                .copied()
                .unwrap_or(0);
            let count =
                default + potential_bonus_spawns(raider, stream, wave, Difficulty::WORLD, bonus);
            let mut ravagers = 0;
            for _ in 0..count {
                let leads = !led && illagers::can_be_leader(kind(raider));
                let at = [
                    f64::from(pos.x) + 0.5,
                    f64::from(pos.y) + 1.0,
                    f64::from(pos.z) + 0.5,
                ];
                let Some(made) =
                    self.join_raid_made(raid, wave, kind(raider), at, leads, &mut level, output)
                else {
                    break;
                };
                if leads {
                    led = true;
                    raid.leaders.retain(|(of, _)| *of != wave);
                    raid.leaders.push((wave, made));
                    raid.leaders.sort_unstable();
                }
                if raider != entity_types::RAVAGER {
                    continue;
                }
                let rider = if wave == NORMAL_GROUPS {
                    Some(entity_types::PILLAGER)
                } else if wave >= HARD_GROUPS {
                    Some(if ravagers == 0 {
                        entity_types::EVOKER
                    } else {
                        entity_types::VINDICATOR
                    })
                } else {
                    None
                };
                ravagers += 1;
                if let Some(rider) = rider {
                    // `joinRaid` places it as every raider, then `snapTo(pos)` at the
                    // block's bottom middle, and it rides the ravager.
                    let at = [
                        f64::from(pos.x) + 0.5,
                        f64::from(pos.y),
                        f64::from(pos.z) + 0.5,
                    ];
                    if let Some(rider) =
                        self.join_raid_made(raid, wave, kind(rider), at, false, &mut level, output)
                    {
                        self.start_riding(rider, made, false, output);
                    }
                }
            }
        }
        raid.spawn_pos = None;
        raid.groups_spawned += 1;
        update_bossbar(raid);
    }

    /// `joinRaid(level, wave, raider, pos, false)` of a raider of the type `kind` made
    /// for the wave (`EntityType.create(EVENT)`): of the raid and the wave, able to join,
    /// leading where `leads` (so its `finalizeSpawn` puts the banner on it), placed at
    /// `at`, finalised for `EVENT` with the level's draws from `level` after its raid is
    /// set (so a vindicator has no axe of its own and no leader is drawn), its buffs of
    /// its own stream, then counted in the wave with its health. `None` where it cannot
    /// be made.
    #[expect(
        clippy::too_many_arguments,
        reason = "`joinRaid`'s arguments and what the region reads for them"
    )]
    fn join_raid_made(
        &mut self,
        raid: &mut Raid,
        wave: i32,
        kind: u16,
        at: Vec3,
        leads: bool,
        level: &mut Random,
        output: &mut TickOutput,
    ) -> Option<EntityId> {
        let id = raid.id;
        let omen = raid.omen_level;
        let block = crate::physics::containing(at);
        let effective = self.difficulty_at(block);
        let difficulty = (effective, special_multiplier(effective));
        let mut health = 0.0_f32;
        let mut prepare = |region: &mut Region, mob: &mut Mob, own: &mut Random| {
            health = mob.body.vitals.health;
            illagers::update(mob, |raider| {
                if leads {
                    raider.patrol_leader = true;
                }
                raider.raid = Some(id);
                raider.wave = wave;
                raider.can_join_raid = true;
                raider.ticks_outside_raid = 0;
            });
            let day = region.clock.day;
            let mut group = None;
            finalize(
                kind,
                &mut Spawning {
                    mob: &mut *mob,
                    level: &mut *level,
                    own: &mut *own,
                    biome: region.biome_at(block),
                    at: block,
                    day,
                    game_time: i64::try_from(region.clock.game_time).unwrap_or(i64::MAX),
                    reason: SpawnReason::Event,
                    group: &mut group,
                    structure: None,
                    moon_brightness: moon_brightness(day),
                    difficulty,
                    companions: None,
                },
            );
            illagers::apply_raid_buffs(kind, mob, own, wave, omen);
        };
        let made = self.make_mob(kind, at, (0.0, 0.0), &mut prepare, output)?;
        raid.members.retain(|member| member.key != made);
        raid.members.push(RaidMember {
            wave,
            key: made,
            health: Float(health),
        });
        raid.members.sort_by_key(|member| (member.wave, member.key));
        raid.total_health = Float(raid.total_health.0 + health);
        update_bossbar(raid);
        Some(made)
    }

    /// `updateRaiders(level)` of the members the region holds in its land (section 9.3):
    /// one 112 or more from the centre is removed; one that has lived more than 600
    /// ticks counts a pass outside the raid while idle beyond 2400 outside a village, and
    /// is removed at its thirtieth. A removed member is of no raid, its health off the
    /// raid's total and its wave's leader gone where it led.
    fn update_raiders(&mut self, raid: &mut Raid, output: &mut TickOutput) {
        let mut gone = Vec::new();
        for member in raid.members.clone() {
            let Some((_, record)) = self.find_entity(member.key) else {
                continue;
            };
            let record = record.clone();
            let Ok(mob) = Mob::decode(&record.data) else {
                continue;
            };
            let block = crate::physics::containing(record.position);
            if super::pois::dist_sqr(raid.centre, block) >= MEMBER_REACH_SQR {
                gone.push((member, mob));
                continue;
            }
            if record.lived <= 600 {
                continue;
            }
            let mut outside = illagers::state_of(&mob).ticks_outside_raid;
            if !self.close_to_village(block, 1)
                && mob.body.no_action_time(record.lived) > IDLE_LIMIT
            {
                outside += 1;
                self.struck_between_ticks(member.key, output, |struck| {
                    illagers::update(&mut *struck.mob, |raider| {
                        raider.ticks_outside_raid = outside;
                    });
                });
            }
            if outside >= 30 {
                gone.push((member, mob));
            }
        }
        for (member, mob) in gone {
            let led = illagers::state_of(&mob).patrol_leader;
            self.remove_from_raid(raid, member, Some(mob.body.vitals.health), output);
            if led {
                raid.leaders.retain(|(wave, _)| *wave != member.wave);
            }
        }
    }

    /// `removeFromRaid(level, raider, updateHealth)` of a member the region holds: out of
    /// its wave, its health `health` off the raid's total where given, of no raid, and the
    /// bar its members' health.
    fn remove_from_raid(
        &mut self,
        raid: &mut Raid,
        member: RaidMember,
        health: Option<f32>,
        output: &mut TickOutput,
    ) {
        let before = raid.members.len();
        raid.members.retain(|kept| kept.key != member.key);
        if raid.members.len() == before {
            return;
        }
        if let Some(health) = health {
            raid.total_health = Float(raid.total_health.0 - health);
        }
        let id = raid.id;
        self.struck_between_ticks(member.key, output, |struck| {
            illagers::update(&mut *struck.mob, |raider| {
                if raider.raid == Some(id) {
                    raider.raid = None;
                }
            });
        });
        update_bossbar(raid);
    }

    /// `playSound(level, pos)`, the horn (D10 C §3.5): the sound's seed drawn of the
    /// raid's stream; to each of the region's players within 64 of the spawn point's
    /// middle horizontally or shown the raid's bar, 13 blocks from them towards it at
    /// their height, at volume 64.
    fn sound_horn(
        &mut self,
        raid: &Raid,
        pos: BlockPos,
        stream: &mut Random,
        output: &mut TickOutput,
    ) {
        let _seed = stream.next_u64();
        let centre = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 0.5,
            f64::from(pos.z) + 0.5,
        ];
        let players: Vec<Vec3> = self
            .players
            .values()
            .map(|player| {
                let at = player.pose.position;
                [at.x, at.y, at.z]
            })
            .collect();
        for at in players {
            let (dx, dz) = (centre[0] - at[0], centre[2] - at[2]);
            let distance = (dx * dx + dz * dz).sqrt();
            let in_bar = matches!(
                self.get_raid_at(crate::physics::containing(at)),
                Some(RaidAt::Own { id, .. }) if id == raid.id
            );
            if distance > HORN_REACH && !in_bar {
                continue;
            }
            let (x, z) = if distance > 0.0 {
                (at[0] + 13.0 / distance * dx, at[2] + 13.0 / distance * dz)
            } else {
                (centre[0], centre[2])
            };
            output.events.push(RegionEvent::Sound {
                sound: sounds::EVENT_RAID_HORN,
                source: SOUND_SOURCE_NEUTRAL,
                position: clustine_world::Vec3::new(x, at[1], z),
                volume: 64.0,
                pitch: 1.0,
                except: None,
            });
        }
    }

    /// Victory's Hero of the Village (48 000 ticks, the omen less one, not ambient, not
    /// visible, with its icon) to each hero: a player of the region's at once, one of the
    /// world's players elsewhere by an effect to them (N9's effects to players).
    fn give_heroes(&mut self, raid: &Raid, output: &mut TickOutput) {
        let amplifier = (raid.omen_level - 1).clamp(0, 255);
        for hero in raid.heroes.clone() {
            let here = self
                .players
                .iter()
                .find(|(id, _)| id.0 == hero)
                .map(|(id, player)| (*id, player.face().game_mode));
            if let Some((id, game_mode)) = here {
                if game_mode == SPECTATOR {
                    continue;
                }
                let layer = Layer {
                    ambient: false,
                    visible: false,
                    show_icon: true,
                    ..Layer::new(u8::try_from(amplifier).unwrap_or(0), HERO_TICKS, self.tick)
                };
                self.add_player_effect(id, Effect::HERO_OF_THE_VILLAGE, layer, output);
                continue;
            }
            let elsewhere = self.faces.world.as_ref().and_then(|world| {
                world
                    .iter()
                    .find(|seen| seen.player.0 == hero && seen.game_mode != SPECTATOR)
                    .map(|seen| (seen.player, seen.entity, seen.position))
            });
            let Some((player, entity, at)) = elsewhere else {
                continue;
            };
            let given = GivenEffects {
                instances: vec![EffectInstance {
                    effect: "minecraft:hero_of_the_village".to_owned(),
                    amplifier,
                    duration: HERO_TICKS,
                    ambient: false,
                    visible: false,
                    show_icon: true,
                }],
                instant_factor: Float(1.0),
                source: None,
                owner: None,
                water: false,
            };
            self.send_on_player(
                player,
                entity,
                crate::physics::containing(at),
                EntityUse::Effects(Box::new(given)),
            );
        }
    }

    /// The raid `id` as a raider of the region reads it: of its own raids every field
    /// it asks (its waves, the leader of `wave`), of the table's what the table says.
    pub(crate) fn raid_known(&self, id: EntityId, wave: i32) -> Option<RaidKnown> {
        if let Some((chunk, raid)) = self.find_raid(id) {
            return Some(RaidKnown {
                seen: super::raids::seen(chunk, raid),
                own: true,
                groups_spawned: Some(raid.groups_spawned),
                leader: Some(
                    raid.leaders
                        .iter()
                        .find(|(of, _)| *of == wave)
                        .map(|(_, leader)| *leader),
                ),
            });
        }
        self.raid_memory
            .table
            .iter()
            .find(|seen| seen.id == id)
            .map(|seen| RaidKnown {
                seen: seen.clone(),
                own: false,
                groups_spawned: None,
                leader: None,
            })
    }

    /// The raider `from`'s word `news` of its raid (`Local::Raider`), of itself or, where
    /// `about` names one, of the raider it recruits: taken to the raid's record, the
    /// region's own at once (a join counted with its health, as `addWaveMob` counts it)
    /// or the table's by its news to the record's chunk. A raid the region does not know
    /// hears nothing.
    pub(crate) fn raider_news(
        &mut self,
        from: EntityId,
        about: Option<EntityId>,
        news: RaiderNews,
        output: &mut TickOutput,
    ) {
        let member = about.unwrap_or(from);
        match news {
            RaiderNews::Joined { raid, wave, health } => {
                self.joined_raid(raid, member, wave, health, output);
            }
            RaiderNews::Recruit { raid, wave } => self.recruited(raid, member, wave, output),
            RaiderNews::Hurt { raid, health } => {
                let news = RaidNews::Hurt {
                    raid,
                    member,
                    health: Float(health),
                };
                self.tell_raid(raid, news, output);
            }
            RaiderNews::Died { raid, killer } => {
                let news = RaidNews::Died {
                    raid,
                    member,
                    killer,
                };
                self.tell_raid(raid, news, output);
            }
            RaiderNews::Left { raid } => {
                self.tell_raid(raid, RaidNews::Left { raid, member }, output);
            }
            RaiderNews::Leads { raid, wave } => {
                self.change_raid(raid, output, |kept| {
                    kept.leaders.retain(|(of, _)| *of != wave);
                    kept.leaders.push((wave, member));
                    kept.leaders.sort_unstable();
                });
            }
        }
    }

    /// `joinRaid(level, wave, raider, null, true)` of the region's raider `member` a
    /// recruiter found: where it still has no active raid and may join, of the raid and
    /// the wave, and counted in it.
    fn recruited(&mut self, raid: EntityId, member: EntityId, wave: i32, output: &mut TickOutput) {
        let Some((_, record)) = self.find_entity(member) else {
            return;
        };
        let lived = record.lived;
        let Ok(mob) = Mob::decode(&record.data) else {
            return;
        };
        let state = illagers::state_of(&mob);
        let active = state
            .raid
            .and_then(|other| self.raid_known(other, 0))
            .is_some_and(|known| known.is_active());
        if active
            || mob.body.vitals.dying()
            || !state.can_join_raid
            || mob.body.no_action_time(lived) > IDLE_LIMIT
        {
            return;
        }
        let health = mob.body.vitals.health;
        self.struck_between_ticks(member, output, |struck| {
            illagers::update(&mut *struck.mob, |raider| {
                raider.raid = Some(raid);
                raider.wave = wave;
                raider.can_join_raid = true;
                raider.ticks_outside_raid = 0;
            });
        });
        self.joined_raid(raid, member, wave, health, output);
    }

    /// `addWaveMob` of the raider `member` into the raid `raid`'s wave `wave`: the
    /// region's own counts it with its health at once; the table's is told, and judges
    /// it (section 9.3).
    fn joined_raid(
        &mut self,
        raid: EntityId,
        member: EntityId,
        wave: i32,
        health: f32,
        output: &mut TickOutput,
    ) {
        if self.find_raid(raid).is_some() {
            self.change_raid(raid, output, |kept| {
                kept.members.retain(|known| known.key != member);
                kept.members.push(RaidMember {
                    wave,
                    key: member,
                    health: Float(health),
                });
                kept.members.sort_by_key(|known| (known.wave, known.key));
                kept.total_health = Float(kept.total_health.0 + health);
                update_bossbar(kept);
            });
            return;
        }
        let Some(from) = self.find_entity(member).map(|(chunk, _)| chunk) else {
            return;
        };
        let news = RaidNews::Joined {
            raid,
            member,
            wave,
            health: Float(health),
            from,
        };
        self.tell_raid(raid, news, output);
    }

    /// `news` to the raid `raid`, at the chunk its record is kept in: the region's own,
    /// or the table's.
    fn tell_raid(&mut self, raid: EntityId, news: RaidNews, output: &mut TickOutput) {
        let chunk = self.find_raid(raid).map(|(chunk, _)| chunk).or_else(|| {
            self.raid_memory
                .table
                .iter()
                .find(|seen| seen.id == raid)
                .map(|seen| seen.chunk)
        });
        if let Some(chunk) = chunk {
            self.send_raid_news(chunk, news, output);
        }
    }
}

/// The spectator's game mode.
const SPECTATOR: u8 = 3;

#[cfg(test)]
mod tests {
    use super::*;

    /// How many numbers `potential_bonus_spawns` draws, and what it gives, of a fresh
    /// stream.
    fn bonus(raider: i32, wave: i32, difficulty: Difficulty, bonus: bool) -> (u64, i32) {
        let start = Random::of_raid(1, EntityId(2), 3);
        let mut stream = start.clone();
        let extra = potential_bonus_spawns(raider, &mut stream, wave, difficulty, bonus);
        (stream.draws_since(&start), extra)
    }

    #[test]
    fn easy_draws_once_or_twice_for_vindicators_and_pillagers_and_never_for_the_rest() {
        for raider in [entity_types::VINDICATOR, entity_types::PILLAGER] {
            for wave in 1..=4 {
                let (draws, extra) = bonus(raider, wave, Difficulty::Easy, wave == 4);
                assert!((1..=2).contains(&draws));
                assert!((0..=1).contains(&extra));
                assert_eq!(draws == 1, extra == 0, "a second draw only after a 1");
            }
        }
        for raider in [
            entity_types::EVOKER,
            entity_types::WITCH,
            entity_types::RAVAGER,
        ] {
            for wave in 1..=4 {
                assert_eq!(bonus(raider, wave, Difficulty::Easy, wave == 4), (0, 0));
            }
        }
    }

    #[test]
    fn normal_and_hard_draw_once_by_their_rules() {
        let (draws, extra) = bonus(entity_types::PILLAGER, 1, Difficulty::Normal, false);
        assert_eq!(draws, 1);
        assert!((0..=1).contains(&extra));
        let (draws, extra) = bonus(entity_types::VINDICATOR, 1, Difficulty::Hard, false);
        assert_eq!(draws, 1);
        assert!((0..=2).contains(&extra));
        assert_eq!(
            bonus(entity_types::WITCH, 2, Difficulty::Normal, false).0,
            0
        );
        assert_eq!(
            bonus(entity_types::WITCH, 4, Difficulty::Normal, false).0,
            0
        );
        assert_eq!(
            bonus(entity_types::WITCH, 3, Difficulty::Normal, false).0,
            1
        );
        assert_eq!(
            bonus(entity_types::RAVAGER, 5, Difficulty::Normal, false).0,
            0
        );
        assert_eq!(
            bonus(entity_types::RAVAGER, 6, Difficulty::Normal, true).0,
            1
        );
        assert_eq!(bonus(entity_types::EVOKER, 5, Difficulty::Hard, true).0, 0);
    }

    #[test]
    fn two_ticks_of_a_raid_draw_differently() {
        let mut first = Random::of_raid(1, EntityId(2), 3);
        let mut second = Random::of_raid(1, EntityId(2), 4);
        let a: Vec<u64> = (0..4).map(|_| first.next_u64()).collect();
        let b: Vec<u64> = (0..4).map(|_| second.next_u64()).collect();
        assert_ne!(a, b);
    }

    #[test]
    fn the_bonus_wave_comes_only_from_an_omen_of_2_after_the_final_wave() {
        let mut raid = Raid {
            id: EntityId(2),
            centre: BlockPos::new(0, 0, 0),
            started: true,
            active: true,
            ticks_active: 0,
            omen_level: 1,
            groups_spawned: 3,
            group_count: 3,
            cooldown_ticks: 0,
            post_raid_ticks: 0,
            celebration_ticks: 0,
            total_health: Float(0.0),
            status: RaidStatus::Ongoing,
            spawn_pos: None,
            bar: uuid::Uuid::nil(),
            bar_shown: true,
            bar_progress: Float(0.0),
            bar_name: RaidBarName::Raid,
            heroes: Vec::new(),
            members: Vec::new(),
            leaders: Vec::new(),
        };
        assert!(!has_more_waves(&raid));
        assert!(!should_spawn_group(&raid));
        raid.omen_level = 2;
        assert!(has_more_waves(&raid));
        assert!(should_spawn_group(&raid));
        raid.groups_spawned = 4;
        assert!(!has_more_waves(&raid));
    }
}
