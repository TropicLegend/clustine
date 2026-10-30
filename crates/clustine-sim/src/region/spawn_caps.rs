//! Natural spawning's frame (`docs/groundwork/n9-plan.md`, sections 9.1, 9.2 and 9.5):
//! the region's counts of its mobs by spawning category, reported to the store with its
//! commits; the caps, from the region's own counts now and what the store last published
//! of the other regions'; and the nearest player for despawning and spawning, from the
//! region's players, the faces it keeps and the world's players' table.
//!
//! **The counts.** For each chunk the region has loaded (not a copy), the mobs of each of
//! the seven categories (`MobCategory` but `MISC`), counting what the game's
//! `NaturalSpawner.createState` counts: not a mob that is persistent (`isPersistenceRequired`
//! or `requiresCustomPersistence`: one that rides, is leashed, or whose type keeps it).
//! A tick reports each chunk whose counts changed, whole, and a chunk let go as nothing;
//! the region's first tick after it is made or restored (a merge, a split, a take-over)
//! reports every loaded chunk's.

use std::collections::BTreeMap;

use clustine_data::spawns::SpawnCost;
use clustine_data::{EntityTypeInfo, MobCategory};
use clustine_world::{BlockPos, ChunkPos, Entity, EntityData, PlayerId};

use super::{Random, Region};
use crate::api::{CountsReport, PublishedCounts, SPAWN_CATEGORIES, SpawnCounts, TickOutput};
use crate::mobs::Mob;

/// The index of `category` among the counted ones, in the game's order; none for
/// `MISC`.
pub(crate) fn category_index(category: MobCategory) -> Option<usize> {
    let index = match category {
        MobCategory::Monster => 0,
        MobCategory::Creature => 1,
        MobCategory::Ambient => 2,
        MobCategory::Axolotls => 3,
        MobCategory::UndergroundWaterCreature => 4,
        MobCategory::WaterCreature => 5,
        MobCategory::WaterAmbient => 6,
        MobCategory::Misc => return None,
    };
    debug_assert!(index < SPAWN_CATEGORIES);
    Some(index)
}

/// The category `entity` counts in, if it counts: a mob not kept by anything.
fn counted_in(entity: &Entity) -> Option<usize> {
    let EntityData::Mob { .. } = entity.data else {
        return None;
    };
    let index = category_index(EntityTypeInfo::of(i32::from(entity.kind)).category)?;
    let mob = Mob::decode(&entity.data).ok()?;
    let kept = mob.body.flags.persistent
        || entity.riding.vehicle.is_some()
        || entity.riding.leash.is_some()
        || crate::mobs::types::of(entity.kind).requires_custom_persistence(&mob);
    (!kept).then_some(index)
}

/// The counts of `entities`, a chunk's.
fn counts_of(entities: &[Entity]) -> SpawnCounts {
    let mut counts = [0; SPAWN_CATEGORIES];
    for index in entities.iter().filter_map(counted_in) {
        counts[index] += 1;
    }
    counts
}

/// The spectator's game mode.
const SPECTATOR: u8 = 3;

/// What the region keeps of its counts between ticks: each loaded chunk's as last
/// reported. In memory only: a restored region reports every chunk again.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Counts {
    pub(crate) by_chunk: BTreeMap<ChunkPos, SpawnCounts>,
}

impl Region {
    /// The players spawning reads (section 9.3): the region's own where they are now,
    /// the faces it keeps, and the world's players beyond them where the store last
    /// published them; in that order, each once.
    pub(super) fn spawning_players(&self) -> Vec<NearPlayer> {
        let mut players: Vec<NearPlayer> = self
            .players
            .iter()
            .map(|(id, player)| {
                let face = player.face();
                NearPlayer {
                    player: *id,
                    position: face.position,
                    spectator: face.game_mode == SPECTATOR,
                }
            })
            .collect();
        let holds = |chunk| self.land.holds(chunk) || self.copies.contains_key(&chunk);
        for (id, kept) in self.faces.seen(holds) {
            if kept.departed.is_none() && !players.iter().any(|near| near.player == *id) {
                players.push(NearPlayer {
                    player: *id,
                    position: kept.face.position,
                    spectator: kept.face.game_mode == SPECTATOR,
                });
            }
        }
        // The world's other players count where they stand in the region's own level:
        // each level spawns for its own players (`ServerLevel.players()`).
        let dimension = self.config.dimension;
        let here = self.faces.world.iter().flatten();
        for other in here.filter(|other| other.dimension == dimension) {
            if !players.iter().any(|near| near.player == other.player) {
                players.push(NearPlayer {
                    player: other.player,
                    position: other.position,
                    spectator: other.game_mode == SPECTATOR,
                });
            }
        }
        players
    }

    /// The spawning phase of the tick (`NaturalSpawner.spawnForChunk` of each spawning
    /// chunk; sections 9.1 to 9.3): nothing before the store's first publication of the
    /// others' counts; the categories filtered by the global cap once, the creatures only
    /// once in 400 ticks of game time; then each chunk the region's entities tick in,
    /// near enough a player, in an order drawn from each chunk's stream.
    pub(super) fn spawn_naturally(&mut self, output: &mut TickOutput) {
        if !self.config.spawn_mobs {
            return;
        }
        let Some(published) = self.published.as_ref() else {
            return;
        };
        let spawn_persistent = self.clock.game_time.is_multiple_of(400);
        let charges = self.spawn_charges();
        let mut state = SpawnState::new(
            self.spawning_players(),
            &self.counts.by_chunk,
            published,
            spawn_persistent,
        )
        .with_charges(charges);
        if !state.any_open() {
            return;
        }
        let seed = self.seed();
        let tick = self.tick;
        let mut chunks: Vec<(u64, ChunkPos)> = self
            .chunks
            .keys()
            .copied()
            .filter(|chunk| self.ticks_chunk(*chunk) && state.may_spawn_in(*chunk))
            .map(|chunk| {
                let key = Random::of_chunk(seed, chunk, tick, SPAWN_ORDER).next_u64();
                (key, chunk)
            })
            .collect();
        chunks.sort_unstable();
        for (_, chunk) in chunks {
            self.spawn_for_chunk(chunk, &mut state, output);
        }
    }

    /// The spawn potential's point charges of the mobs already there (`createState`):
    /// each counted mob of the region's loaded chunks whose type has a spawn cost in the
    /// biome at its block (`getRoughBiome`), its charge at its block. The other regions'
    /// mobs are not charged (named: no region reads another's mobs).
    fn spawn_charges(&self) -> Vec<(BlockPos, f64)> {
        let mut charges = Vec::new();
        for chunk in self.chunks.values() {
            for entity in chunk.entities() {
                if counted_in(entity).is_none() {
                    continue;
                }
                let pos = crate::physics::containing(entity.position);
                if let Some(cost) = self.spawn_cost(entity.kind, pos) {
                    charges.push((pos, cost.charge));
                }
            }
        }
        charges
    }

    /// `MobSpawnSettings.getMobSpawnCost(type)` of the biome at `pos`
    /// (`getRoughBiome`, the cell's), where it gives the type one: the soul sand
    /// valley's and the warped forest's.
    pub(crate) fn spawn_cost(&self, kind: u16, pos: BlockPos) -> Option<SpawnCost> {
        let name = self.biome_at(pos)?;
        let biome = clustine_data::BIOME_SPAWNS
            .iter()
            .find(|biome| biome.name == name)?;
        biome
            .costs
            .iter()
            .find(|cost| cost.kind == usize::from(kind))
            .copied()
    }

    /// The tick's report of its counts (section 9.2), at the end of the tick: every
    /// loaded chunk's where `full`, else each chunk whose counts changed, and nothing for
    /// a chunk let go.
    pub(super) fn report_counts(&mut self, full: bool, output: &mut TickOutput) {
        let mut chunks: Vec<(ChunkPos, SpawnCounts)> = Vec::new();
        let gone: Vec<ChunkPos> = self
            .counts
            .by_chunk
            .keys()
            .copied()
            .filter(|position| !self.chunks.contains_key(position))
            .collect();
        for position in gone {
            self.counts.by_chunk.remove(&position);
            chunks.push((position, [0; SPAWN_CATEGORIES]));
        }
        // Counted again only where the tick changed entities, or the chunk is newly
        // loaded: a mob's record is decoded to tell whether it is kept.
        let touched: std::collections::BTreeSet<ChunkPos> = output
            .entity_changes
            .iter()
            .map(|(position, _)| *position)
            .collect();
        for (position, chunk) in &self.chunks {
            let known = self.counts.by_chunk.get(position).copied();
            let now = match known {
                Some(counts) if !touched.contains(position) => counts,
                _ => counts_of(chunk.entities()),
            };
            self.counts.by_chunk.insert(*position, now);
            if full || known != Some(now) {
                chunks.push((*position, now));
            }
        }
        chunks.sort_by_key(|(position, _)| *position);
        if full || !chunks.is_empty() {
            output.counts = Some(CountsReport { full, chunks });
        }
    }
}

/// `MobCategory.getMaxInstancesPerChunk` of each counted category, in its order
/// (n7 MobCategory, `<clinit>`).
pub(crate) const MAX_PER_CHUNK: [u32; SPAWN_CATEGORIES] = [70, 10, 15, 5, 5, 5, 20];

/// The counted category of the persistent ones (`isPersistent`): the creatures, which
/// spawn only once in 400 ticks.
const CREATURE: usize = 1;

/// The game's `MAGIC_NUMBER`: the chunks of a player's 17 by 17.
const MAGIC_NUMBER: u32 = 289;

/// How far from a player's chunk the chunks are that count towards the spawnable ones
/// and may spawn (`DistanceManager`'s natural spawning range).
const SPAWN_RANGE: i32 = 8;

/// The purpose mark of the stream a chunk's place in the tick's order is drawn from.
const SPAWN_ORDER: u64 = 0x7370_6177_6e5f_6f72;

/// The purpose mark of the stream a chunk's spawning draws from (section 9.3), apart
/// from its order's.
pub(crate) const SPAWNING: u64 = 0x7370_6177_6e69_6e67;

/// A player as spawning reads them: the region's own where they are now, the faces it
/// keeps, and the world's players beyond them where the store last published them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct NearPlayer {
    pub(crate) player: PlayerId,
    /// Their feet.
    pub(crate) position: [f64; 3],
    pub(crate) spectator: bool,
}

impl NearPlayer {
    fn chunk(&self) -> ChunkPos {
        ChunkPos::containing(self.position[0], self.position[2])
    }

    /// `playerIsCloseEnoughForSpawning`: not a spectator, and within 128 blocks of the
    /// chunk's centre horizontally.
    fn close_for_spawning(&self, chunk: ChunkPos) -> bool {
        !self.spectator && centre_distance_sqr(chunk, self.position) < LOCAL_RANGE_SQR
    }
}

/// How far from a player a chunk's centre counts towards their local counts, squared:
/// 128 blocks horizontally (`(cx·16 + 8 − px)² + (cz·16 + 8 − pz)² < 16384`, D9 5.1).
const LOCAL_RANGE_SQR: f64 = 16_384.0;

fn centre_distance_sqr(chunk: ChunkPos, position: [f64; 3]) -> f64 {
    let dx = f64::from(chunk.x * 16 + 8) - position[0];
    let dz = f64::from(chunk.z * 16 + 8) - position[2];
    dx * dx + dz * dz
}

/// The tick's spawning state (the game's `NaturalSpawner.SpawnState` with its
/// `LocalMobCapCalculator`), made once at the start of the phase.
#[derive(Debug, Clone)]
pub(crate) struct SpawnState {
    /// The categories the global cap leaves open for the whole phase
    /// (`getFilteredSpawningCategories`).
    pub(crate) open: [bool; SPAWN_CATEGORIES],
    players: Vec<NearPlayer>,
    /// The non-spectators, by the chunk they stand in: what the nearest player is
    /// looked for in (finding 33).
    by_chunk: BTreeMap<ChunkPos, Vec<usize>>,
    /// Each player's local counts: the other regions' as published, and the region's
    /// own within 128 blocks of where they are now, raised by the phase's spawns.
    local: Vec<SpawnCounts>,
    /// The players close enough to each chunk asked, fixed at its first asking
    /// (`playersNearChunk`).
    near_chunk: BTreeMap<ChunkPos, Vec<usize>>,
    /// The spawn potential's point charges (`PotentialCalculator`): each counted mob
    /// of the region's whose biome gives its type a spawn cost, at its block, and each
    /// mob the phase makes; and the charge of the last type passed (`lastCharge`).
    charges: Vec<(BlockPos, f64)>,
    last_charge: f64,
}

/// `PotentialCalculator.getPotentialEnergyChange(pos, charge)`: the sum of each point
/// charge's `charge / distance` to `pos` (infinite where they coincide), times `charge`.
fn potential_energy_change(charges: &[(BlockPos, f64)], pos: BlockPos, charge: f64) -> f64 {
    let mut total = 0.0;
    for (at, own) in charges {
        let (dx, dy, dz) = (
            f64::from(at.x - pos.x),
            f64::from(at.y - pos.y),
            f64::from(at.z - pos.z),
        );
        let distance = dx * dx + dy * dy + dz * dz;
        total += if distance == 0.0 {
            f64::INFINITY
        } else {
            own / distance.sqrt()
        };
    }
    total * charge
}

impl SpawnState {
    /// The state of the phase: `players`, the region's own `counts` by chunk now, what
    /// the store `published` of the others', and whether creatures may spawn in this
    /// tick (`spawn_persistent`).
    pub(crate) fn new(
        players: Vec<NearPlayer>,
        counts: &BTreeMap<ChunkPos, SpawnCounts>,
        published: &PublishedCounts,
        spawn_persistent: bool,
    ) -> Self {
        // The spawnable chunks: those within 8 of any player's chunk, each once,
        // spectators' too.
        let mut spawnable = std::collections::BTreeSet::new();
        for player in &players {
            let at = player.chunk();
            for x in -SPAWN_RANGE..=SPAWN_RANGE {
                for z in -SPAWN_RANGE..=SPAWN_RANGE {
                    spawnable.insert(ChunkPos::new(at.x + x, at.z + z));
                }
            }
        }
        let spawnable = u32::try_from(spawnable.len()).unwrap_or(u32::MAX);
        let mut totals = published.totals;
        for chunk_counts in counts.values() {
            for (total, count) in totals.iter_mut().zip(chunk_counts) {
                *total += count;
            }
        }
        let mut open = [false; SPAWN_CATEGORIES];
        for (index, open) in open.iter_mut().enumerate() {
            // `spawnFriendlies` and `spawnEnemies` are the game rules', both on.
            let allowed = index != CREATURE || spawn_persistent;
            let cap = MAX_PER_CHUNK[index].saturating_mul(spawnable) / MAGIC_NUMBER;
            *open = allowed && totals[index] < cap;
        }
        let published: BTreeMap<PlayerId, SpawnCounts> =
            published.players.iter().copied().collect();
        let mut by_chunk: BTreeMap<ChunkPos, Vec<usize>> = BTreeMap::new();
        let mut local = Vec::with_capacity(players.len());
        for (index, player) in players.iter().enumerate() {
            let mut near = published.get(&player.player).copied().unwrap_or_default();
            if !player.spectator {
                by_chunk.entry(player.chunk()).or_default().push(index);
                for (chunk, chunk_counts) in counts {
                    if player.close_for_spawning(*chunk) {
                        for (count, own) in near.iter_mut().zip(chunk_counts) {
                            *count += own;
                        }
                    }
                }
            }
            local.push(near);
        }
        Self {
            open,
            players,
            by_chunk,
            local,
            near_chunk: BTreeMap::new(),
            charges: Vec::new(),
            last_charge: 0.0,
        }
    }

    /// Whether any category is open: none, and the phase does nothing.
    pub(crate) fn any_open(&self) -> bool {
        self.open.iter().any(|open| *open)
    }

    /// The players a chunk asks of, fixed at its first asking.
    fn near(&mut self, chunk: ChunkPos) -> &[usize] {
        let players = &self.players;
        self.near_chunk.entry(chunk).or_insert_with(|| {
            (0..players.len())
                .filter(|index| players[*index].close_for_spawning(chunk))
                .collect()
        })
    }

    /// `canSpawnForCategoryLocal`: whether any player near the chunk has fewer than the
    /// category's cap counted; no player near, none.
    pub(crate) fn can_spawn_local(&mut self, category: usize, chunk: ChunkPos) -> bool {
        let near = self.near(chunk).to_vec();
        near.iter()
            .any(|index| self.local[*index][category] < MAX_PER_CHUNK[category])
    }

    /// `afterSpawn`'s count (`LocalMobCapCalculator.addMob`): a mob of `category` made in
    /// `chunk` counts for every player near it.
    pub(crate) fn after_spawn(&mut self, category: usize, chunk: ChunkPos) {
        let near = self.near(chunk).to_vec();
        for index in near {
            self.local[index][category] += 1;
        }
    }

    /// The point charges of the mobs already there (`createState`), which the phase's
    /// spawns are weighed against.
    pub(crate) fn with_charges(mut self, charges: Vec<(BlockPos, f64)>) -> Self {
        self.charges = charges;
        self
    }

    /// `SpawnState.canSpawn(type, pos, chunk)`: with no spawn cost for the type in the
    /// biome at `pos`, yes; else whether the potential's change of its charge there is
    /// within its energy budget. The charge is kept for [`SpawnState::charged`].
    pub(crate) fn can_spawn(&mut self, cost: Option<SpawnCost>, pos: BlockPos) -> bool {
        let Some(cost) = cost else {
            self.last_charge = 0.0;
            return true;
        };
        self.last_charge = cost.charge;
        potential_energy_change(&self.charges, pos, cost.charge) <= cost.energy_budget
    }

    /// `afterSpawn`'s charge: the mob made at `pos` adds the charge its type was passed
    /// with (`spawnPotential.addCharge(pos, lastCharge)`).
    pub(crate) fn charged(&mut self, pos: BlockPos) {
        self.charges.push((pos, self.last_charge));
    }

    /// Whether some non-spectator's chunk centre is within 128 blocks of `chunk`'s
    /// (`anyPlayerCloseEnoughForSpawning`), and some player's chunk, spectators'
    /// too, within 8 of it.
    pub(super) fn may_spawn_in(&self, chunk: ChunkPos) -> bool {
        let close = self
            .players
            .iter()
            .any(|player| player.close_for_spawning(chunk));
        let tracked = self.players.iter().any(|player| {
            let at = player.chunk();
            (at.x - chunk.x).abs() <= SPAWN_RANGE && (at.z - chunk.z).abs() <= SPAWN_RANGE
        });
        close && tracked
    }

    /// The nearest non-spectator to `at` at any distance and the square of their
    /// distance (`getNearestPlayer(x, y, z, -1, false)`), looked for in the chunks
    /// outward from `at`'s: a ring further out cannot hold anyone nearer once its inner
    /// edge is farther than the nearest found.
    pub(crate) fn nearest(&self, at: [f64; 3]) -> Option<(NearPlayer, f64)> {
        let centre = ChunkPos::containing(at[0], at[2]);
        let mut best: Option<(usize, f64)> = None;
        // The ring of the farthest player's chunk: no one stands beyond it.
        let reach = self
            .by_chunk
            .keys()
            .map(|chunk| (chunk.x - centre.x).abs().max((chunk.z - centre.z).abs()))
            .max()
            .unwrap_or(-1);
        for ring in 0..=reach {
            if let Some((_, distance)) = best {
                // A position in this ring's chunks is at least `(ring − 1) · 16`
                // blocks from `at` horizontally.
                let inner = f64::from((ring - 1).max(0) * 16);
                if inner * inner > distance {
                    break;
                }
            }
            for chunk in ring_of(centre, ring) {
                for index in self.by_chunk.get(&chunk).into_iter().flatten() {
                    let p = self.players[*index].position;
                    let d = [p[0] - at[0], p[1] - at[1], p[2] - at[2]];
                    let distance = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
                    // The first of the nearest in the players' order, as the game's
                    // strict `<` keeps the first.
                    let nearer = match best {
                        None => true,
                        Some((held, least)) => {
                            distance < least || (distance == least && *index < held)
                        }
                    };
                    if nearer {
                        best = Some((*index, distance));
                    }
                }
            }
        }
        best.map(|(index, distance)| (self.players[index], distance))
    }
}

/// The chunks at Chebyshev distance `ring` from `centre`.
fn ring_of(centre: ChunkPos, ring: i32) -> Vec<ChunkPos> {
    if ring == 0 {
        return vec![centre];
    }
    let mut chunks = Vec::with_capacity(usize::try_from(ring * 8).unwrap_or(0));
    for x in -ring..=ring {
        chunks.push(ChunkPos::new(centre.x + x, centre.z - ring));
        chunks.push(ChunkPos::new(centre.x + x, centre.z + ring));
    }
    for z in (1 - ring)..ring {
        chunks.push(ChunkPos::new(centre.x - ring, centre.z + z));
        chunks.push(ChunkPos::new(centre.x + ring, centre.z + z));
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(number: u128, x: f64, z: f64, spectator: bool) -> NearPlayer {
        NearPlayer {
            player: PlayerId(uuid::Uuid::from_u128(number)),
            position: [x, 64.0, z],
            spectator,
        }
    }

    fn monsters(count: u32) -> SpawnCounts {
        [count, 0, 0, 0, 0, 0, 0]
    }

    fn state(
        players: Vec<NearPlayer>,
        own: &[(ChunkPos, SpawnCounts)],
        published: &PublishedCounts,
        persistent: bool,
    ) -> SpawnState {
        let own: BTreeMap<ChunkPos, SpawnCounts> = own.iter().copied().collect();
        SpawnState::new(players, &own, published, persistent)
    }

    /// The soul sand valley's cost of a ghast (budget 0.15, charge 0.7).
    const GHAST_COST: SpawnCost = SpawnCost {
        kind: 0,
        energy_budget: 0.15,
        charge: 0.7,
    };

    /// `PotentialCalculator`: a type with no cost always passes; with a cost, the
    /// first passes in an empty potential, and a second near the first does not,
    /// `0.7 · 0.7 / d` against the budget: at 4 blocks 0.1225 passes, at 3 blocks
    /// 0.163 does not.
    #[test]
    fn the_spawn_potential_keeps_charged_mobs_apart_by_their_budget() {
        let published = PublishedCounts::default();
        let mut state = state(Vec::new(), &[], &published, false);
        let origin = BlockPos::new(0, 40, 0);
        assert!(state.can_spawn(None, origin));
        assert!(state.can_spawn(Some(GHAST_COST), origin));
        state.charged(origin);
        assert!(state.can_spawn(None, origin), "no cost: no charge weighed");
        assert!(state.can_spawn(Some(GHAST_COST), BlockPos::new(4, 40, 0)));
        assert!(!state.can_spawn(Some(GHAST_COST), BlockPos::new(3, 40, 0)));
        assert!(
            !state.can_spawn(Some(GHAST_COST), origin),
            "on a charge the potential is infinite"
        );
    }

    /// A mob made with no cost charges nothing.
    #[test]
    fn a_mob_without_a_cost_adds_no_charge() {
        let published = PublishedCounts::default();
        let mut state = state(Vec::new(), &[], &published, false);
        let origin = BlockPos::new(0, 40, 0);
        assert!(state.can_spawn(None, origin));
        state.charged(origin);
        assert!(state.can_spawn(Some(GHAST_COST), BlockPos::new(1, 40, 0)));
    }

    #[test]
    fn the_global_cap_counts_the_region_s_own_and_the_others_against_the_spawnable_chunks() {
        // One player: 289 spawnable chunks, so the monsters' cap is 70.
        let players = vec![near(1, 8.0, 8.0, false)];
        let others = PublishedCounts {
            totals: monsters(40),
            players: Vec::new(),
        };
        let open = state(
            players.clone(),
            &[(ChunkPos::new(0, 0), monsters(29))],
            &others,
            false,
        );
        assert!(open.open[0]);
        let closed = state(
            players,
            &[(ChunkPos::new(0, 0), monsters(30))],
            &others,
            false,
        );
        assert!(!closed.open[0]);
        // Two players far apart: twice the chunks, twice the cap.
        let two = vec![near(1, 8.0, 8.0, false), near(2, 1000.0, 8.0, false)];
        let wider = state(
            two,
            &[(ChunkPos::new(0, 0), monsters(100))],
            &PublishedCounts::default(),
            false,
        );
        assert!(wider.open[0]);
    }

    #[test]
    fn creatures_are_open_only_in_a_tick_of_persistent_spawning() {
        let players = vec![near(1, 8.0, 8.0, false)];
        let none = PublishedCounts::default();
        assert!(!state(players.clone(), &[], &none, false).open[1]);
        assert!(state(players, &[], &none, true).open[1]);
    }

    #[test]
    fn the_local_cap_passes_if_any_player_near_has_room_and_nobody_near_refuses() {
        let full = near(1, 8.0, 8.0, false);
        let published = PublishedCounts {
            totals: [0; SPAWN_CATEGORIES],
            players: vec![(full.player, monsters(60))],
        };
        // Ten of the region's own within 128 blocks of the first player fill them.
        let own = [(ChunkPos::new(1, 0), monsters(10))];
        let mut alone = state(vec![full], &own, &published, false);
        assert!(!alone.can_spawn_local(0, ChunkPos::new(0, 0)));
        // A second player near with room lets it through.
        let mut two = state(
            vec![full, near(2, 40.0, 8.0, false)],
            &own,
            &published,
            false,
        );
        assert!(two.can_spawn_local(0, ChunkPos::new(0, 0)));
        // A chunk nobody is near refuses.
        assert!(!two.can_spawn_local(0, ChunkPos::new(40, 0)));
        // A spectator is near nobody's cap.
        let mut watched = state(
            vec![near(3, 8.0, 8.0, true)],
            &[],
            &PublishedCounts::default(),
            false,
        );
        assert!(!watched.can_spawn_local(0, ChunkPos::new(0, 0)));
    }

    #[test]
    fn a_mob_spawned_counts_for_every_player_near_its_chunk_at_once() {
        let player = near(1, 8.0, 8.0, false);
        let published = PublishedCounts {
            totals: [0; SPAWN_CATEGORIES],
            players: vec![(player.player, monsters(69))],
        };
        let mut spawning = state(vec![player], &[], &published, false);
        assert!(spawning.can_spawn_local(0, ChunkPos::new(0, 0)));
        spawning.after_spawn(0, ChunkPos::new(2, 0));
        assert!(!spawning.can_spawn_local(0, ChunkPos::new(0, 0)));
    }

    #[test]
    fn the_nearest_non_spectator_is_found_at_any_distance() {
        let players = vec![
            near(1, 300.0, 0.0, false),
            near(2, 5.0, 0.0, true),
            near(3, -200.0, 0.0, false),
        ];
        let spawning = state(players, &[], &PublishedCounts::default(), false);
        let (found, distance) = spawning.nearest([0.0, 64.0, 0.0]).expect("someone");
        assert_eq!(found.player, PlayerId(uuid::Uuid::from_u128(3)));
        assert_eq!(distance, 40_000.0);
        let nobody = state(
            vec![near(1, 0.0, 0.0, true)],
            &[],
            &PublishedCounts::default(),
            false,
        );
        assert_eq!(nobody.nearest([0.0, 64.0, 0.0]), None);
    }

    #[test]
    fn the_nearest_of_two_in_neighbouring_rings_is_the_nearer() {
        // The one in the next chunk's ring is nearer than the one in the same chunk.
        let players = vec![near(1, 1.0, 1.0, false), near(2, 16.5, 15.0, false)];
        let spawning = state(players, &[], &PublishedCounts::default(), false);
        let (found, _) = spawning.nearest([15.5, 64.0, 15.0]).expect("someone");
        assert_eq!(found.player, PlayerId(uuid::Uuid::from_u128(2)));
    }

    #[test]
    fn a_chunk_spawns_only_near_a_non_spectator_and_within_eight_of_a_player_s_chunk() {
        let spawning = state(
            vec![near(1, 8.0, 8.0, false)],
            &[],
            &PublishedCounts::default(),
            false,
        );
        assert!(spawning.may_spawn_in(ChunkPos::new(7, 0)));
        // Centre 136 blocks off: too far.
        assert!(!spawning.may_spawn_in(ChunkPos::new(8, 1)));
        let watching = state(
            vec![near(1, 8.0, 8.0, true)],
            &[],
            &PublishedCounts::default(),
            false,
        );
        assert!(!watching.may_spawn_in(ChunkPos::new(0, 0)));
    }
}
