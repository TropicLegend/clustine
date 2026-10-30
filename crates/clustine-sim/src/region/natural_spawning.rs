//! The per-chunk natural spawner (`docs/groundwork/n9-plan.md`, section 9.3; part
//! S10's) on N9h's frame (`spawn_caps.rs`): `NaturalSpawner.spawnForChunk` of a spawning
//! chunk, each category the global cap left open and the local cap lets through, in the
//! categories' order, every draw from the chunk's one stream for spawning in the tick.
//!
//! Each category's position, groups and attempts are the game's
//! (`spawnCategoryForPosition`, n7 NaturalSpawner 215–491), in its order of draws: the
//! position (three draws), a redstone conductor there ending it; three groups, each with
//! a pack size `ceil(nextFloat() · 4)` and each attempt stepping `nextInt(6) −
//! nextInt(6)` in x then in z; the nearest non-spectator at any distance; more than 24
//! blocks from them and the world's spawn, on land the region's entities tick in; the
//! type once a group (`getRandomSpawnMobAt`, the structure's override or the biome's
//! list), the pack size then drawn again from its count; `isValidSpawnPostitionForType`
//! with the type's rule and its draws (`spawning/rules.rs`); the type made, a type
//! Clustine does not build ending the whole position as the game's null does; the yaw;
//! `isValidPositionForMob`; and only then the mob's key, its own stream's draws and its
//! `finalizeSpawn` (finding 32), the chunk's stream as the level's random.
//!
//! Named differences: the potential of the spawn costs is not reckoned (no overworld
//! biome has costs); the Nether fortress's own list on nether bricks is not asked; a
//! box is obstructed by the region's own players and by the entities of its loaded and
//! copied chunks, not by players whose faces it keeps; the world border's collision is
//! not asked of the spawn's box (the placement asks the border of the block); a school
//! of fish spawned together does not start following its first at once (no draw hangs
//! on it: `FollowFlockLeaderGoal` gathers them within ticks).

use clustine_data::spawns::Spawner;
use clustine_data::{BIOME_SPAWNS, EntityTypeInfo, MobCategory, entity_types};
use clustine_world::{Biome, BlockPos, ChunkPos, Entity, EntityId, KeptKind};

use super::spawn_caps::{SPAWNING, SpawnState, category_index};
use super::{Random, Region};
use crate::api::{SPAWN_CATEGORIES, TickOutput};
use crate::mobs::types::{self, SpawnGroup, SpawnReason};
use crate::physics::aabb::Aabb;
use crate::physics::collide;
use crate::physics::entities::Mover;
use crate::spawning::rules::{self, Light, SpawnWorld};

/// The names of the counted categories in their order, as the biomes' lists name them.
const CATEGORY_NAMES: [&str; SPAWN_CATEGORIES] = [
    "monster",
    "creature",
    "ambient",
    "axolotls",
    "underground_water_creature",
    "water_creature",
    "water_ambient",
];

/// The water's ambient category, which a river halves the chance of.
const WATER_AMBIENT: usize = 6;

/// How many groups a position tries.
const GROUPS: u32 = 3;

/// How close to the nearest player and to the world's spawn a mob may not spawn,
/// squared (`isRightDistanceToPlayerAndSpawnPoint`: 24 blocks).
const TOO_NEAR_SQR: f64 = 576.0;

/// The purpose mark of the scratch stream a fresh mob's `removeWhenFarAway` is asked
/// of before it has a key: nothing it draws is kept.
const SCRATCH: u64 = 0x7370_6177_6e5f_7363;

/// `CollisionContext.empty()`: what a block's shape is to no entity at all.
const NO_ENTITY: collide::Context = collide::Context {
    bottom: -f64::MAX,
    fall_distance: 0.0,
    falling_block: false,
    noclip: None,
};

/// The spectator's game mode.
const SPECTATOR: u8 = 3;

/// How a category's position ended (for the tests and for whoever reads why nothing
/// came of a chunk).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Ended {
    /// No chunk, or a position below the world's lowest block but one.
    NoPosition,
    /// A redstone conductor at the position.
    Conductor,
    /// Its three groups were tried.
    Groups,
    /// Its mobs reached the type's `getMaxSpawnClusterSize()`.
    Cluster,
    /// A type Clustine does not build was picked and passed its checks (the game's
    /// `getMobForSpawn` null).
    Unbuilt(u16),
}

/// What a category's position made, and how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Spawned {
    /// Each mob made, its type and key, in order.
    pub(super) made: Vec<(u16, EntityId)>,
    /// The groups begun.
    pub(super) groups: u32,
    pub(super) ended: Ended,
}

impl Region {
    /// `spawnForChunk` of `chunk`: for each category open at the start of the phase and
    /// let through by the local cap at this chunk, `spawnCategoryForChunk`, all of them
    /// drawing from the chunk's one stream for spawning in this tick.
    pub(super) fn spawn_for_chunk(
        &mut self,
        chunk: ChunkPos,
        state: &mut SpawnState,
        output: &mut TickOutput,
    ) {
        let mut draws = Random::of_chunk(self.seed(), chunk, self.tick, SPAWNING);
        for category in 0..SPAWN_CATEGORIES {
            if state.open[category] && state.can_spawn_local(category, chunk) {
                self.spawn_category_for_chunk(category, chunk, state, &mut draws, output);
            }
        }
    }

    /// `spawnCategoryForChunk` of the counted category `category` (`spawn_caps.rs`'
    /// order) in `chunk`, drawing from `draws`, the chunk's stream for spawning
    /// ([`super::random_ticks::Random::of_chunk`] with [`SPAWNING`]): the position, the
    /// groups, the nearest player ([`SpawnState::nearest`]), the type's rules and the
    /// making, each mob made counted by [`SpawnState::after_spawn`].
    pub(super) fn spawn_category_for_chunk(
        &mut self,
        category: usize,
        chunk: ChunkPos,
        state: &mut SpawnState,
        draws: &mut Random,
        output: &mut TickOutput,
    ) -> Spawned {
        let mut spawned = Spawned {
            made: Vec::new(),
            groups: 0,
            ended: Ended::NoPosition,
        };
        // `getRandomPosWithin`: a column, then a height up to the surface's first free
        // block (`getHeight(WORLD_SURFACE) + 1`), all three drawn whatever comes of them.
        let Some(loaded) = self.chunks.get(&chunk) else {
            return spawned;
        };
        let min_y = loaded.min_y();
        let x = chunk.x * 16 + draws.next_int(16) as i32;
        let z = chunk.z * 16 + draws.next_int(16) as i32;
        let column = ((z & 15) << 4 | (x & 15)) as usize;
        let top = min_y + i32::from(loaded.heights().world_surface[column]);
        let span = u32::try_from(top - min_y + 1).unwrap_or(1);
        let y = draws.next_int(span) as i32 + min_y;
        if y < min_y + 1 {
            return spawned;
        }
        let start = BlockPos::new(x, y, z);
        if self
            .block(start)
            .is_some_and(|here| here.props().is_redstone_conductor())
        {
            spawned.ended = Ended::Conductor;
            return spawned;
        }
        let mut total = 0;
        for _ in 0..GROUPS {
            spawned.groups += 1;
            let (mut x, mut z) = (start.x, start.z);
            let mut entry: Option<Spawner> = None;
            let mut group: Option<SpawnGroup> = None;
            let mut pack = ceil(draws.next_float() * 4.0);
            let mut attempt = 0;
            while attempt < pack {
                attempt += 1;
                x += draws.next_int(6) as i32 - draws.next_int(6) as i32;
                z += draws.next_int(6) as i32 - draws.next_int(6) as i32;
                let at = BlockPos::new(x, y, z);
                let position = [f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5];
                let Some((_, distance)) = state.nearest(position) else {
                    continue;
                };
                if !self.right_distance(chunk, at, position, distance) {
                    continue;
                }
                let picked = match entry {
                    Some(picked) => picked,
                    None => {
                        let Some(picked) = self.random_spawn_mob_at(category, at, draws) else {
                            break;
                        };
                        // `spawnerData.count().sample(random)`: the pack size again.
                        pack = sample_count(&picked, draws);
                        entry = Some(picked);
                        picked
                    }
                };
                let Ok(kind) = u16::try_from(picked.kind) else {
                    continue;
                };
                if !self.valid_spawn_position_for_type(category, &picked, at, distance, draws) {
                    continue;
                }
                // The potential's charge passes: no overworld biome has spawn costs.
                if !types::is_built(kind) {
                    spawned.ended = Ended::Unbuilt(kind);
                    return spawned;
                }
                let yaw = draws.next_float() * 360.0;
                if !self.valid_position_for_mob(kind, position, yaw, distance) {
                    continue;
                }
                let Some(key) =
                    self.make_natural_mob(kind, position, yaw, &mut group, draws, output)
                else {
                    continue;
                };
                spawned.made.push((kind, key));
                total += 1;
                // `afterSpawn`: the type's category, at the mob's chunk.
                if let Some(counted) = category_of(kind) {
                    state.after_spawn(counted, at.chunk());
                }
                if total >= rules::max_spawn_cluster_size(kind) {
                    spawned.ended = Ended::Cluster;
                    return spawned;
                }
                if is_max_group_size_reached(kind, group.as_ref()) {
                    break;
                }
            }
        }
        spawned.ended = Ended::Groups;
        spawned
    }

    /// `isRightDistanceToPlayerAndSpawnPoint`: more than 24 blocks from the nearest
    /// player, not within 24 of the world's spawn (the centre of its block), and on land
    /// the region's entities tick in: the chunk spawned in, or another it ticks (another
    /// region's land is refused, decision 13).
    fn right_distance(
        &self,
        chunk: ChunkPos,
        at: BlockPos,
        position: [f64; 3],
        distance: f64,
    ) -> bool {
        if distance <= TOO_NEAR_SQR {
            return false;
        }
        // The game looks at the world's spawn only in the spawn's own dimension
        // (`isRightDistanceToPlayerAndSpawnPoint`).
        if let Some(spawn) = self.config.spawn {
            let spawn = crate::physics::containing([spawn.x, spawn.y, spawn.z]);
            let centre = [
                f64::from(spawn.x) + 0.5,
                f64::from(spawn.y) + 0.5,
                f64::from(spawn.z) + 0.5,
            ];
            let d: [f64; 3] = std::array::from_fn(|axis| centre[axis] - position[axis]);
            if d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < TOO_NEAR_SQR {
                return false;
            }
        }
        let there = at.chunk();
        there == chunk || self.ticks_chunk(there)
    }

    /// `mobsAt(category, pos)`: the list of the first structure at `pos` with an
    /// override for the category (`ChunkGenerator.getMobsAt`, in the order the chunk
    /// keeps its starts), else the list of the biome there.
    fn mobs_at(&self, category: usize, pos: BlockPos) -> &'static [Spawner] {
        for kept in self.structures_at(pos) {
            if let Some(list) = structure_list(kept.kind, category) {
                return list;
            }
        }
        self.zoomed_biome_at(pos)
            .and_then(|name| BIOME_SPAWNS.iter().find(|biome| biome.name == name))
            .and_then(|biome| biome.of(CATEGORY_NAMES[category]))
            .unwrap_or(&[])
    }

    /// `getRandomSpawnMobAt`: in a biome of `#reduce_water_ambient_spawns`, the water's
    /// ambient category gives nothing at `nextFloat() < 0.98`; else the list's weighted
    /// pick, one `nextInt(total)` (none, and no draw, for a list of no weight).
    fn random_spawn_mob_at(
        &self,
        category: usize,
        pos: BlockPos,
        draws: &mut Random,
    ) -> Option<Spawner> {
        if category == WATER_AMBIENT
            && self.spawn_world().biome(pos).is_some_and(|biome| {
                rules::biome_in_tag(biome, "minecraft:reduce_water_ambient_spawns")
            })
            && draws.next_float() < 0.98
        {
            return None;
        }
        pick(self.mobs_at(category, pos), draws)
    }

    /// `isValidSpawnPostitionForType`: not of `MISC`; within the category's despawn
    /// distance unless the type may spawn far; the list at the block still holding the
    /// entry; the placement; the type's rule with its draws; and no collision of the
    /// type's spawn box with blocks or hard entities.
    fn valid_spawn_position_for_type(
        &self,
        category: usize,
        entry: &Spawner,
        at: BlockPos,
        distance: f64,
        draws: &mut Random,
    ) -> bool {
        let Ok(kind) = u16::try_from(entry.kind) else {
            return false;
        };
        let Some(own) = category_of(kind) else {
            return false;
        };
        if !rules::can_spawn_far_from_player(kind) && distance > rules::despawn_distance_sqr(own) {
            return false;
        }
        if !self.mobs_at(category, at).contains(entry) {
            return false;
        }
        let world = self.spawn_world();
        if !rules::is_spawn_position_ok(kind, &world, at) {
            return false;
        }
        if !rules::check_spawn_rules(kind, &world, at, draws) {
            return false;
        }
        let position = [
            f64::from(at.x) + 0.5,
            f64::from(at.y),
            f64::from(at.z) + 0.5,
        ];
        self.no_collision(&spawn_box(kind, position))
    }

    /// `noCollision(box)`: no block's shape in it, and no entity that is solid to
    /// anything (a boat).
    fn no_collision(&self, bounds: &Aabb) -> bool {
        let view = super::items::View {
            region: self,
            home: ChunkPos::containing(bounds.min[0], bounds.min[2]),
        };
        let nobody = Mover {
            key: EntityId(u64::MAX),
            root: EntityId(u64::MAX),
            like_a_vehicle: false,
            happy_ghast: false,
        };
        collide::no_collision(&view, NO_ENTITY, &nobody, bounds)
    }

    /// `isValidPositionForMob` of a fresh mob of the type `kind` at `position`: beyond
    /// its category's despawn distance a mob that would be removed there is refused;
    /// then its `checkSpawnRules` (a pathfinder's walk target value) and its
    /// `checkSpawnObstruction`.
    fn valid_position_for_mob(
        &self,
        kind: u16,
        position: [f64; 3],
        yaw: f32,
        distance: f64,
    ) -> bool {
        let Some(category) = category_of(kind) else {
            return false;
        };
        if distance > rules::despawn_distance_sqr(category) {
            // Made of a scratch stream: only its kind of mob is asked.
            let chunk = ChunkPos::containing(position[0], position[2]);
            let mut scratch = Random::of_chunk(self.seed(), chunk, self.tick, SCRATCH);
            let (_, mob) = crate::mobs::making::construct(kind, &mut scratch, yaw);
            if types::of(kind).remove_when_far_away(kind, &mob, 0, distance) {
                return false;
            }
        }
        let world = self.spawn_world();
        let at = crate::physics::containing(position);
        if !rules::mob_check_spawn_rules(kind, &world, at) {
            return false;
        }
        let info = EntityTypeInfo::of(i32::from(kind));
        let bounds = Aabb::of_entity(position, info.width, info.height);
        let blocks = (
            crate::physics::containing(bounds.min),
            crate::physics::containing(bounds.max),
        );
        rules::obstruction_by_blocks(kind, &world, at, blocks) && self.unobstructed(&bounds)
    }

    /// `isUnobstructed(mob)`: no entity that blocks building (a living one, a boat, a
    /// minecart, a falling block, primed TNT, an armour stand that is no marker) and no
    /// player but a spectator overlaps the box.
    fn unobstructed(&self, bounds: &Aabb) -> bool {
        let low = ChunkPos::containing(bounds.min[0] - 2.0, bounds.min[2] - 2.0);
        let high = ChunkPos::containing(bounds.max[0] + 2.0, bounds.max[2] + 2.0);
        for x in low.x..=high.x {
            for z in low.z..=high.z {
                let Some(chunk) = self.readable_chunk(ChunkPos::new(x, z)) else {
                    continue;
                };
                let blocked = chunk.entities().iter().any(|entity| {
                    blocks_building(entity) && self.entity_box(entity).intersects(*bounds)
                });
                if blocked {
                    return false;
                }
            }
        }
        !self.players.values().any(|player| {
            let face = player.face();
            let height = if face.sneaking { 1.5 } else { 1.8 };
            face.game_mode != SPECTATOR
                && Aabb::of_entity(face.position, 0.6, height).intersects(*bounds)
        })
    }

    /// The mob of the type `kind` that passed every test, made at `position` facing
    /// `yaw` (`Region::spawn_mob_with`'s making, freed of its player, at its exact place):
    /// its key now, its own stream's draws (its UUID and its constructor's), then its
    /// `finalizeSpawn` for `NATURAL` with the difficulty at its block and the group's
    /// data, the chunk's stream `level` as the level's random; added with what its
    /// `finalizeSpawn` made beside it (`addFreshEntityWithPassengers`). Its key; none
    /// where no key is left.
    fn make_natural_mob(
        &mut self,
        kind: u16,
        position: [f64; 3],
        yaw: f32,
        group: &mut Option<SpawnGroup>,
        level: &mut Random,
        output: &mut TickOutput,
    ) -> Option<EntityId> {
        let block = crate::physics::containing(position);
        let chunk = block.chunk();
        let key = self.take_entity_key(chunk)?;
        let mut own = self.entity_stream(key).random;
        let own_start = own.clone();
        let (uuid, mut mob) = crate::mobs::making::construct(kind, &mut own, yaw);
        let biome = self.zoomed_biome_at(block);
        // `Cat.finalizeSpawn` asks for a piece of `#cats_spawn_as_black` (the swamp
        // hut's) at its block.
        let structure = self
            .structures_at(block)
            .iter()
            .any(|kept| kept.kind == KeptKind::SwampHut)
            .then_some("minecraft:swamp_hut");
        let effective = self.difficulty_at(block);
        let day = self.clock.day;
        let game_time = i64::try_from(self.clock.game_time).unwrap_or(i64::MAX);
        let mut companions = NaturalCompanions {
            region: self,
            chunk,
            at: position,
            made: Vec::new(),
            rides: None,
        };
        crate::mobs::making::finalize(
            kind,
            &mut types::Spawning {
                mob: &mut mob,
                level,
                own: &mut own,
                biome,
                at: block,
                day,
                game_time,
                reason: SpawnReason::Natural,
                group,
                structure,
                moon_brightness: crate::mobs::making::moon_brightness(day),
                difficulty: (effective, special_multiplier(effective)),
                companions: Some(&mut companions as &mut dyn types::Companions),
            },
        );
        let rides = companions.rides;
        let made = companions.made;
        self.entity_drew(key, own.draws_since(&own_start));
        let entity = Entity {
            portal: Default::default(),
            key,
            kind,
            uuid,
            position,
            motion: [0.0; 3],
            yaw,
            pitch: 0.0,
            on_ground: false,
            hops: 0,
            lived: 0,
            fresh_at: 0,
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
            data: mob.encode(),
        };
        // As the egg's mob is added: a mount made in `finalizeSpawn` before the mob, a
        // rider after it, with it.
        let (vehicles, riders): (Vec<_>, Vec<_>) = made
            .into_iter()
            .partition(|made| made.seat == types::CompanionSeat::Vehicle);
        let vehicle = vehicles.first().map(|made| made.entity.key).or(rides);
        for made in vehicles {
            self.add_natural_companion(made, output);
        }
        self.add_fresh_entity(entity, output);
        if let Some(vehicle) = vehicle {
            self.start_riding(key, vehicle, true, output);
        }
        for made in riders {
            let rider = made.entity.key;
            self.add_natural_companion(made, output);
            self.start_riding(rider, key, true, output);
        }
        Some(key)
    }

    /// Adds a mob made beside a spawned one, its own draws counted.
    fn add_natural_companion(&mut self, made: types::Companion, output: &mut TickOutput) {
        let start = self.entity_stream(made.entity.key).random;
        self.entity_drew(made.entity.key, made.own.draws_since(&start));
        self.add_fresh_entity(made.entity, output);
    }

    /// What the rules read of the region (`spawning/rules.rs`).
    pub(super) fn spawn_world(&self) -> RegionWorld<'_> {
        RegionWorld(self)
    }
}

/// The region as the rules read it.
pub(super) struct RegionWorld<'r>(&'r Region);

impl SpawnWorld for RegionWorld<'_> {
    fn sea_level(&self) -> i32 {
        self.0.config.sea_level
    }

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
        self.0.sky_darken()
    }

    fn dimension(&self) -> clustine_world::Dimension {
        self.0.config.dimension
    }

    fn biome(&self, pos: BlockPos) -> Option<Biome> {
        rules::biome_named(self.0.zoomed_biome_at(pos)?)
    }

    fn surface(&self, x: i32, z: i32) -> Option<i32> {
        let chunk = self
            .0
            .readable_chunk(ChunkPos::containing(f64::from(x), f64::from(z)))?;
        let column = ((z & 15) << 4 | (x & 15)) as usize;
        Some(chunk.min_y() + i32::from(chunk.heights().world_surface[column]))
    }

    fn game_seed(&self) -> i64 {
        self.0.config.game_seed
    }

    fn day(&self) -> u64 {
        self.0.clock.day
    }
}

/// The natural spawner's side of a mob made beside a spawned one (a zombie's chicken, a
/// spider's skeleton): its key from the region's block for the chunk, and its stream.
struct NaturalCompanions<'r> {
    region: &'r mut Region,
    chunk: ChunkPos,
    at: crate::physics::Vec3,
    made: Vec<types::Companion>,
    rides: Option<EntityId>,
}

impl types::Companions for NaturalCompanions<'_> {
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

/// `Mth.ceil(float)`.
fn ceil(value: f32) -> u32 {
    let truncated = value as u32;
    if value > truncated as f32 {
        truncated + 1
    } else {
        truncated
    }
}

/// `IntProvider.sample` of an entry's count: a uniform count draws
/// (`nextInt(max − min + 1) + min`), a constant does not.
fn sample_count(entry: &Spawner, draws: &mut Random) -> u32 {
    let count = if entry.min_count == entry.max_count {
        entry.min_count
    } else {
        draws.next_int_between(entry.min_count, entry.max_count)
    };
    u32::try_from(count).unwrap_or(0)
}

/// `WeightedList.getRandom`: one `nextInt(total)`, the first entry whose weights so far
/// pass it; nothing, and no draw, for a list of no weight.
fn pick(list: &[Spawner], draws: &mut Random) -> Option<Spawner> {
    let total: i32 = list.iter().map(|entry| entry.weight).sum();
    let total = u32::try_from(total).ok().filter(|total| *total > 0)?;
    let mut left = draws.next_int(total) as i32;
    list.iter().copied().find(|entry| {
        left -= entry.weight;
        left < 0
    })
}

/// The counted category of the type `kind` (`EntityType.getCategory()`), none for
/// `MISC`.
fn category_of(kind: u16) -> Option<usize> {
    let category: MobCategory = EntityTypeInfo::of(i32::from(kind)).category;
    category_index(category)
}

/// `isMaxGroupSizeReached(count)`: only the tropical fish's says so, of one that is no
/// school's (`isSchool` false: one of the tenth whose variant was drawn whole, which
/// left no group's variant behind).
fn is_max_group_size_reached(kind: u16, group: Option<&SpawnGroup>) -> bool {
    i32::from(kind) == entity_types::TROPICAL_FISH
        && !matches!(group, Some(SpawnGroup::TropicalFish { .. }))
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

/// `EntityType.getSpawnAABB(x, y, z)`: the type's box at its feet, scaled by its
/// `spawnDimensionsScale` (the slime's and the magma cube's 4, the sulfur cube's 2,
/// n7 EntityTypes 3764, 5554, 6151; every other's 1), its half width a float.
fn spawn_box(kind: u16, position: [f64; 3]) -> Aabb {
    use entity_types::*;
    let info = EntityTypeInfo::of(i32::from(kind));
    let scale: f32 = match i32::from(kind) {
        SLIME | MAGMA_CUBE => 4.0,
        SULFUR_CUBE => 2.0,
        _ => 1.0,
    };
    let half = f64::from(scale * info.width / 2.0);
    let height = f64::from(scale * info.height);
    Aabb::new(
        [position[0] - half, position[1], position[2] - half],
        [position[0] + half, position[1] + height, position[2] + half],
    )
}

/// `blocksBuilding` of an entity of the region's chunks: every living one, boats,
/// minecarts, falling blocks, primed TNT and an armour stand that is no marker.
fn blocks_building(entity: &Entity) -> bool {
    use clustine_world::EntityData;
    matches!(
        entity.data,
        EntityData::Mob { .. } | EntityData::FallingBlock { .. } | EntityData::PrimedTnt { .. }
    ) || crate::physics::entities::blocks_building(entity.kind, &entity.data)
}

/// The lists of the structures' spawn overrides (`worldgen/structure/*.json`), by the
/// counted category; `None` where the structure has no override for it. The ancient
/// city and the trial chambers override every category with nothing.
fn structure_list(kind: KeptKind, category: usize) -> Option<&'static [Spawner]> {
    const fn one(kind: i32, weight: i32, min_count: i32, max_count: i32) -> Spawner {
        Spawner {
            kind: kind as usize,
            weight,
            min_count,
            max_count,
        }
    }
    const WITCH: [Spawner; 1] = [one(entity_types::WITCH, 1, 1, 1)];
    const CAT: [Spawner; 1] = [one(entity_types::CAT, 1, 1, 1)];
    const GUARDIAN: [Spawner; 1] = [one(entity_types::GUARDIAN, 1, 2, 4)];
    const PILLAGER: [Spawner; 1] = [one(entity_types::PILLAGER, 1, 1, 1)];
    const FORTRESS: [Spawner; 5] = [
        one(entity_types::BLAZE, 10, 2, 3),
        one(entity_types::ZOMBIFIED_PIGLIN, 5, 4, 4),
        one(entity_types::WITHER_SKELETON, 8, 5, 5),
        one(entity_types::SKELETON, 2, 5, 5),
        one(entity_types::MAGMA_CUBE, 3, 4, 4),
    ];
    const MONSTER: usize = 0;
    const CREATURE: usize = 1;
    const AXOLOTLS: usize = 3;
    const UNDERGROUND_WATER_CREATURE: usize = 4;
    match (kind, category) {
        (KeptKind::SwampHut, MONSTER) => Some(&WITCH),
        (KeptKind::SwampHut, CREATURE) => Some(&CAT),
        (KeptKind::Monument, MONSTER) => Some(&GUARDIAN),
        (KeptKind::Monument, AXOLOTLS | UNDERGROUND_WATER_CREATURE) => Some(&[]),
        (KeptKind::PillagerOutpost, MONSTER) => Some(&PILLAGER),
        (KeptKind::AncientCity | KeptKind::TrialChambers, _) => Some(&[]),
        (KeptKind::Fortress, MONSTER) => Some(&FORTRESS),
        _ => None,
    }
}

#[cfg(test)]
#[path = "../spawning/rules_n9_tests.rs"]
mod rules_n9_tests;
