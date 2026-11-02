//! The End's dragon fight (`EnderDragonFight`; `docs/groundwork/p-plan.md`, sections
//! 19.2 E9 and the fight, 19.5 items 6 and 7, and 19.6; the brief's section 6). The fight
//! is kept by the End's home region in its state (`RegionState::dragon_fight`); the
//! dragon lives in the arena, which only the home holds, so the dragon and its fight are
//! always in one region.
//!
//! - **The seam to the dragon** (`mobs/types/ender_dragon.rs`): what it reads of its
//!   fight each tick ([`FightView`]), and what it tells it
//!   ([`Region::fight_dragon_updated`], [`Region::fight_dragon_killed`]).
//! - **The bar** (`EnderDragonFight.updatePlayers`, every 20 ticks): every End region
//!   shows the fight's bar to its own players within 192 blocks of (0, 128, 0), the
//!   home from its own fight and the others from what the store publishes of it, and
//!   takes it away as they leave the range, leave the region, die or enter the credits.
//!   The bar's progress is told each tick it changes.
//! - **The arena** (the fight's `DRAGON` ticket of radius 9): the home keeps the
//!   arena's chunks loaded while one of its own players is on the bar, or another End
//!   region says it has one there (its sighting, which the store publishes to the home
//!   as `arena_wanted`).
//! - **The crystals** (`updateCrystalCount`, every 100 ticks): those within the top
//!   boxes of the world's ten spikes.
//!
//! Named: the bar's UUID is the world's own, not a random one, so that it is the same
//! after a restart and in every region.

use std::collections::BTreeMap;

use clustine_blocks::data::{is_block, same_block};
use clustine_data::{blocks, entity_types};
use clustine_noise::LegacyRandom;
use clustine_world::Entity;
use clustine_world::dragon::{ARENA_REACH, DragonFight};
use clustine_world::{BlockPos, ChunkPos, Dimension, EntityData, EntityId, PlayerId};
use uuid::Uuid;

use super::Region;
use super::updates::{UPDATE_ALL, UPDATE_LIMIT};
use crate::api::{BarChange, BarName, BarStyle, FightSeen, PlayerEvent, RegionEvent, TickOutput};
use crate::physics::aabb::Aabb;

/// How far from (0, 128, 0) a player is on the fight's bar (`validPlayer`: 192 blocks).
const BAR_REACH: f64 = 192.0;

/// How often the bar's players are looked at, and the crystals counted
/// (`EnderDragonFight.tick`).
const PLAYERS_EVERY: u64 = 20;
const CRYSTALS_EVERY: u64 = 100;

/// How long the fight's dragon goes unseen before it is looked for or made again.
const DRAGON_UNSEEN: u32 = 1200;

/// What a dragon reads of its fight (`EnderDragon.getDragonFight()` and what its phases
/// ask of it), none where the dragon has no fight: outside the End's home.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FightView {
    /// `EnderDragonFight.getCrystalsAlive()`: the crystals on the spikes, as the fight
    /// last counted them.
    pub(crate) crystals_alive: u32,
    /// The top of the exit portal's podium, where the dragon lands and dies
    /// (`level.getHeightmapPos(MOTION_BLOCKING_NO_LEAVES,
    /// EndPodiumFeature.getLocation(fight.origin))`).
    pub(crate) podium: BlockPos,
    /// `hasPreviouslyKilledDragon()`: the death's experience is 500, not 12000.
    pub(crate) previously_killed: bool,
}

/// What a region keeps in memory of the fight: the bar each of its players is shown,
/// with its progress; the dragon's progress as it last told it; the crystals counted;
/// and the sighting last reported.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct FightMemory {
    shown: BTreeMap<PlayerId, f32>,
    progress: Option<f32>,
    crystals_alive: u32,
    reported_in_arena: Option<bool>,
    reported_fight: Option<FightSeen>,
    /// The ticks the fight's dragon was not found (`ticksSinceDragonSeen`).
    unseen: u32,
}

/// The fight's bar's UUID, the world's own.
fn bar_id(game_seed: i64) -> Uuid {
    Uuid::from_u64_pair(game_seed as u64, 0x0e4d_d4a6_0f16_47e1)
}

/// Whether a player standing at `at` is within the bar's reach of (0, 128, 0).
fn within_reach(at: [f64; 3]) -> bool {
    let (dx, dy, dz) = (at[0], at[1] - 128.0, at[2]);
    dx * dx + dy * dy + dz * dz <= BAR_REACH * BAR_REACH
}

impl Region {
    /// The fight's step of the tick, in every region of the End: the home's fight made
    /// where it has none and its crystals counted, the arena kept loaded while it is
    /// wanted, the bar shown to the region's players, and the sighting reported.
    pub(super) fn fight_tick(&mut self, output: &mut TickOutput) {
        if self.config.dimension != Dimension::End {
            return;
        }
        let home = self.is_home_of(Dimension::End);
        if home && self.dragon_fight.is_none() {
            // `init`: the twenty gateways' order, shuffled with the world's seed.
            let mut fight = DragonFight::default();
            let mut gateways: Vec<i32> = (0..20).collect();
            let mut random = LegacyRandom::from_seed(self.config.game_seed as u64);
            clustine_features::features::trees::common::shuffle(&mut gateways, &mut random);
            fight.gateways = gateways;
            self.dragon_fight = Some(Box::new(fight));
            self.journal.dragon_fight = true;
        }
        let on_bar: Vec<PlayerId> = (self.players.iter())
            .filter(|(_, player)| {
                let at = player.pose.position;
                !player.dead && within_reach([at.x, at.y, at.z])
            })
            .map(|(id, _)| *id)
            .collect();
        if home {
            let wanted = !on_bar.is_empty() || self.published_end.arena_wanted;
            if wanted {
                self.keep_arena();
            }
            // `tick`: nothing more while nobody is on the bar or the arena is loading.
            if wanted && self.arena_loaded() {
                self.fight_step(output);
            }
        }
        self.show_fight_bar(&on_bar, output);
        if !home {
            let wanted = !on_bar.is_empty();
            if self.fight_memory.reported_in_arena != Some(wanted) {
                self.fight_memory.reported_in_arena = Some(wanted);
                output.village.get_or_insert_with(Default::default).in_arena = Some(wanted);
            }
        }
    }

    /// The fight as this region sees it: its own where it is the home, else what the
    /// store published of the home's.
    fn fight_seen(&self) -> Option<FightSeen> {
        if !self.is_home_of(Dimension::End) {
            return self.published_end.fight;
        }
        let fight = self.dragon_fight.as_ref()?;
        Some(FightSeen {
            bar: bar_id(self.config.game_seed),
            shown: !fight.dragon_killed,
            progress: self.fight_memory.progress.unwrap_or(1.0),
        })
    }

    /// The home's word on its fight to the store, where it changed, for the End's other
    /// regions (19.6).
    pub(super) fn report_fight(&mut self, output: &mut TickOutput) {
        if !self.is_home_of(Dimension::End) {
            return;
        }
        let seen = self.fight_seen();
        if self.fight_memory.reported_fight != seen {
            self.fight_memory.reported_fight = seen;
            output.village.get_or_insert_with(Default::default).fight = seen;
        }
    }

    /// `updatePlayers` every 20 ticks, and the progress each tick: the bar added to the
    /// players `on_bar` and taken from the others it was shown, and from every player
    /// who is no longer the region's; its progress told where it changed.
    fn show_fight_bar(&mut self, on_bar: &[PlayerId], output: &mut TickOutput) {
        let seen = self.fight_seen().filter(|seen| seen.shown);
        let regather = self.tick.is_multiple_of(PLAYERS_EVERY);
        let shown = std::mem::take(&mut self.fight_memory.shown);
        let mut now = BTreeMap::new();
        for (id, progress) in &shown {
            let stays = self.players.contains_key(id)
                && seen.is_some()
                && (!regather || on_bar.contains(id));
            match seen {
                Some(seen) if stays => {
                    if *progress != seen.progress {
                        output.player_events.push((
                            *id,
                            PlayerEvent::BossBar {
                                id: seen.bar,
                                change: BarChange::Progress(seen.progress),
                            },
                        ));
                    }
                    now.insert(*id, seen.progress);
                }
                _ => {
                    let bar = seen.map_or(bar_id(self.config.game_seed), |seen| seen.bar);
                    output.player_events.push((
                        *id,
                        PlayerEvent::BossBar {
                            id: bar,
                            change: BarChange::Remove,
                        },
                    ));
                }
            }
        }
        if regather && let Some(seen) = seen {
            for id in on_bar {
                if now.contains_key(id) {
                    continue;
                }
                output.player_events.push((
                    *id,
                    PlayerEvent::BossBar {
                        id: seen.bar,
                        change: BarChange::Add {
                            name: BarName::Dragon,
                            progress: seen.progress,
                            style: BarStyle::DRAGON,
                        },
                    },
                ));
                now.insert(*id, seen.progress);
            }
        }
        self.fight_memory.shown = now;
    }

    /// `isArenaLoaded`: the row of the arena's chunks at z = 8 (the game's own quirk)
    /// and the 3 × 3 about the origin, loaded.
    fn arena_loaded(&self) -> bool {
        let row = (-ARENA_REACH..=ARENA_REACH).map(|x| ChunkPos::new(x, ARENA_REACH));
        let middle = (-1..=1).flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)));
        row.chain(middle)
            .all(|chunk| self.chunks.contains_key(&chunk))
    }

    /// The fight's own step, once its arena is loaded (`EnderDragonFight.tick` 5 to 7):
    /// the first scan, the crystals counted, and the dragon found or made.
    fn fight_step(&mut self, output: &mut TickOutput) {
        let Some(fight) = self.dragon_fight.as_ref() else {
            return;
        };
        if fight.needs_state_scanning {
            self.scan_state(output);
        }
        if self.tick.is_multiple_of(CRYSTALS_EVERY) {
            self.fight_memory.crystals_alive = self.count_crystals();
        }
        let Some(fight) = self.dragon_fight.as_ref() else {
            return;
        };
        if fight.dragon_killed {
            return;
        }
        let alive = fight
            .dragon
            .is_some_and(|key| self.find_entity(key).is_some());
        if alive {
            self.fight_memory.unseen = 0;
            return;
        }
        self.fight_memory.unseen += 1;
        if fight.dragon.is_none() || self.fight_memory.unseen >= DRAGON_UNSEEN {
            self.find_or_create_dragon(output);
        }
    }

    /// `scanState`: a fight never looked at finds whether a dragon was killed before (an
    /// exit portal is open) and its exit portal, made where there is none, inactive.
    fn scan_state(&mut self, output: &mut TickOutput) {
        let open = self.open_exit_portal();
        let Some(fight) = self.dragon_fight.as_mut() else {
            return;
        };
        fight.needs_state_scanning = false;
        fight.previously_killed = open.is_some();
        if let Some(open) = open {
            fight.exit_portal = Some(open);
        }
        self.journal.dragon_fight = true;
        if open.is_none() {
            self.spawn_exit_portal(false, output);
        }
        if let Some(fight) = self.dragon_fight.as_mut() {
            // With no dragon alive the game would take it killed; a fight that never
            // killed one makes it.
            fight.dragon_killed = fight.previously_killed && fight.dragon.is_none();
        }
    }

    /// An open exit portal about the podium's column: the end portal block at its
    /// middle's layer (the game looks for any end portal's block entity in the arena).
    fn open_exit_portal(&self) -> Option<BlockPos> {
        let rules = self.config.dimension.rules();
        let top = rules.min_y + i32::try_from(rules.height).unwrap_or(i32::MAX);
        for y in rules.min_y..top {
            for (x, z) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let at = BlockPos::new(x, y, z);
                if self
                    .block(at)
                    .is_some_and(|state| is_block(state, blocks::END_PORTAL))
                {
                    return Some(BlockPos::new(0, y, 0));
                }
            }
        }
        None
    }

    /// `spawnExitPortal(active)`: the podium at the fight's exit portal, found where the
    /// fight has none (the top of the origin's column, below it, down through bedrock
    /// to y 63 at the least, one above the floor at the least).
    fn spawn_exit_portal(&mut self, active: bool, output: &mut TickOutput) {
        let Some(fight) = self.dragon_fight.as_ref() else {
            return;
        };
        let origin = match fight.exit_portal {
            Some(origin) => origin,
            None => {
                let mut at = self.podium_top();
                at.y -= 1;
                while at.y > 63
                    && self
                        .block(at)
                        .is_some_and(|state| is_block(state, blocks::BEDROCK))
                {
                    at.y -= 1;
                }
                at.y = at.y.max(self.config.lowest_y + 1);
                at
            }
        };
        for write in clustine_features::features::end::end_podium::podium_writes(origin, active) {
            if write.drop_previous {
                let here = self.block(write.pos);
                if here.is_some_and(|state| same_block(state, write.state)) {
                    continue;
                }
                self.destroy_block(write.pos, true, UPDATE_LIMIT, output);
            }
            self.set_for_travel(write.pos, write.state, UPDATE_ALL, output);
        }
        if let Some(fight) = self.dragon_fight.as_mut() {
            fight.exit_portal = Some(origin);
        }
        self.journal.dragon_fight = true;
    }

    /// `findOrCreateDragon`: a dragon of the arena taken as the fight's, else a new one.
    fn find_or_create_dragon(&mut self, output: &mut TickOutput) {
        let found = (self.chunks.values())
            .flat_map(|chunk| chunk.entities())
            .find(|entity| i32::from(entity.kind) == entity_types::ENDER_DRAGON)
            .map(|entity| entity.key);
        let key = match found {
            Some(key) => Some(key),
            None => self.create_new_dragon(output),
        };
        if let Some(fight) = self.dragon_fight.as_mut() {
            fight.dragon = key;
        }
        self.fight_memory.unseen = 0;
        self.journal.dragon_fight = true;
    }

    /// `createNewDragon`: a dragon at (0, 128, 0) in its holding pattern, turned at
    /// random, made as the game's `EntityType.create` makes one (no `finalizeSpawn`).
    fn create_new_dragon(&mut self, output: &mut TickOutput) -> Option<EntityId> {
        let kind = entity_types::ENDER_DRAGON as u16;
        if !crate::mobs::types::is_built(kind) {
            return None;
        }
        let chunk = ChunkPos::new(0, 0);
        let key = self.take_entity_key(chunk)?;
        let mut own = self.entity_stream(key).random;
        let own_start = own.clone();
        let (uuid, mut mob) = crate::mobs::making::construct(kind, &mut own, 0.0);
        let _ = crate::mobs::types::ender_dragon::set_phase(
            &mut mob,
            crate::mobs::types::ender_dragon::phases::HOLDING_PATTERN,
        );
        // The yaw is the level's draw; the dragon's own stream stands for it, named.
        let yaw = own.next_float() * 360.0;
        mob.body.head.head_yaw = yaw;
        mob.body.head.body_yaw = yaw;
        self.entity_drew(key, own.draws_since(&own_start));
        let entity = Entity {
            portal: Default::default(),
            key,
            kind,
            uuid,
            position: [0.0, 128.0, 0.0],
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
        self.add_fresh_entity(entity, output);
        Some(key)
    }

    /// `spawnNewGateway`: the next of the twenty gateways about the island, with its
    /// level event 3000, as `end_gateway_delayed` places it (no exit yet).
    fn spawn_new_gateway(&mut self, output: &mut TickOutput) {
        let Some(index) = self
            .dragon_fight
            .as_mut()
            .and_then(|fight| fight.gateways.pop())
        else {
            return;
        };
        self.journal.dragon_fight = true;
        let angle = 2.0 * (-std::f64::consts::PI + 0.157_079_632_679_489_66 * f64::from(index));
        let at = BlockPos::new(
            (96.0 * libm::cos(angle)).floor() as i32,
            75,
            (96.0 * libm::sin(angle)).floor() as i32,
        );
        output.events.push(RegionEvent::LevelEvent {
            kind: 3000,
            position: at,
            data: 0,
        });
        for dy in -2..=2 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let state =
                        clustine_features::features::end::end_gateway::gateway_block(dx, dy, dz);
                    self.set_for_travel(at.offset(dx, dy, dz), state, UPDATE_ALL, output);
                }
            }
        }
    }

    /// The fight's `DRAGON` ticket: the arena's chunks kept loaded for a while longer.
    fn keep_arena(&mut self) {
        let until = self.tick + 2 * PLAYERS_EVERY;
        for x in -ARENA_REACH..=ARENA_REACH {
            for z in -ARENA_REACH..=ARENA_REACH {
                let chunk = ChunkPos::new(x, z);
                if self.is_lending(chunk) {
                    continue;
                }
                let ticket = self.arrival_tickets.entry(chunk).or_insert(until);
                *ticket = (*ticket).max(until);
            }
        }
    }

    /// `updateCrystalCount`: the crystals within the top boxes of the world's spikes.
    fn count_crystals(&self) -> u32 {
        let spikes =
            clustine_features::features::end::end_spike::spikes_of_seed(self.config.game_seed);
        let rules = self.config.dimension.rules();
        let mut count = 0;
        for spike in spikes {
            let (x, z, r) = (
                f64::from(spike.centre_x),
                f64::from(spike.centre_z),
                f64::from(spike.radius),
            );
            let top = Aabb::new(
                [x - r, f64::from(rules.min_y), z - r],
                [
                    x + r,
                    f64::from(rules.min_y) + f64::from(rules.height),
                    z + r,
                ],
            );
            for (key, _) in self.entities_in(&top) {
                let crystal = self
                    .find_entity(key)
                    .is_some_and(|(_, entity)| matches!(entity.data, EntityData::EndCrystal(_)));
                count += u32::from(crystal);
            }
        }
        count
    }

    /// `EnderDragonFight.onCrystalDestroyed` (1365–1402) of a crystal that was not a
    /// respawn's (the respawn by crystals is not built): the crystals counted again at
    /// once, and the fight's dragon told (`EnderDragon.onCrystalDestroyed`: 10 to its
    /// head where it was its nearest crystal), with the player who destroyed it.
    pub(super) fn fight_crystal_destroyed(
        &mut self,
        crystal: &Entity,
        player: Option<(EntityId, Uuid)>,
        output: &mut TickOutput,
    ) {
        if !self.is_home_of(Dimension::End) {
            return;
        }
        self.fight_memory.crystals_alive = self.count_crystals();
        let Some(dragon) = self.dragon_fight.as_ref().and_then(|fight| fight.dragon) else {
            return;
        };
        let pos = crate::physics::containing(crystal.position);
        self.dragon_crystal_destroyed(dragon, crystal.key, pos, player, output);
    }

    /// What the dragon `key` reads of its fight: none outside the End's home, or for a
    /// dragon that is not the fight's own.
    pub(crate) fn fight_view(&self, key: EntityId) -> Option<FightView> {
        if !self.is_home_of(Dimension::End) {
            return None;
        }
        let fight = self.dragon_fight.as_ref()?;
        if fight.dragon != Some(key) {
            return None;
        }
        Some(FightView {
            crystals_alive: self.fight_memory.crystals_alive,
            podium: self.podium_top(),
            previously_killed: fight.previously_killed,
        })
    }

    /// `EnderDragonFight.updateDragon`: the dragon `key`, of `health` out of `max`, as
    /// its `aiStep` tells it each tick; the bar shows it.
    pub(crate) fn fight_dragon_updated(&mut self, key: EntityId, health: f32, max: f32) {
        let ours = (self.dragon_fight.as_ref()).is_some_and(|fight| fight.dragon == Some(key));
        if ours && max > 0.0 {
            self.fight_memory.progress = Some(health / max);
        }
    }

    /// `EnderDragonFight.setDragonKilled`: the dragon `key` has finished dying (its
    /// `tickDeath` at 200) and is removed. The exit portal, the egg and the gateway come
    /// with the fight's next step.
    pub(crate) fn fight_dragon_killed(&mut self, key: EntityId, output: &mut TickOutput) {
        let Some(fight) = self.dragon_fight.as_ref() else {
            return;
        };
        if fight.dragon != Some(key) {
            return;
        }
        let first = !fight.previously_killed;
        self.fight_memory.progress = Some(0.0);
        // `setVisible(false)` at once, before the podium and before the dragon's
        // removal: the bar is taken from everyone this region shows it to.
        let bar = bar_id(self.config.game_seed);
        for id in std::mem::take(&mut self.fight_memory.shown).into_keys() {
            output.player_events.push((
                id,
                PlayerEvent::BossBar {
                    id: bar,
                    change: BarChange::Remove,
                },
            ));
        }
        self.spawn_exit_portal(true, output);
        self.spawn_new_gateway(output);
        if first {
            // The egg on the podium's top (`MOTION_BLOCKING` at its location).
            let y = (self.readable_chunk(ChunkPos::new(0, 0)))
                .map(|chunk| chunk.min_y() + i32::from(chunk.heights().motion_blocking[0]));
            if let Some(y) = y {
                self.set_for_travel(
                    BlockPos::new(0, y, 0),
                    blocks::DRAGON_EGG,
                    UPDATE_ALL,
                    output,
                );
            }
        }
        if let Some(fight) = self.dragon_fight.as_mut() {
            fight.dragon_killed = true;
            fight.previously_killed = true;
        }
        self.journal.dragon_fight = true;
    }

    /// The top of the podium's column at the fight's origin (0, 0): the highest block
    /// that stops movement, not counting leaves, and one above it; the floor of the
    /// level where the region cannot read it.
    fn podium_top(&self) -> BlockPos {
        let y = (self.readable_chunk(ChunkPos::new(0, 0)))
            .map(|chunk| chunk.min_y() + i32::from(chunk.heights().motion_blocking_no_leaves[0]))
            .unwrap_or(self.config.lowest_y);
        BlockPos::new(0, y, 0)
    }
}

#[cfg(test)]
#[path = "end_fight_tests.rs"]
mod tests;
