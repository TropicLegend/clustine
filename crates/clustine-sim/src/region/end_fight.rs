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

use clustine_world::dragon::{ARENA_REACH, DragonFight};
use clustine_world::{BlockPos, ChunkPos, Dimension, EntityData, EntityId, PlayerId};
use uuid::Uuid;

use super::Region;
use crate::api::{BarChange, BarName, BarStyle, FightSeen, PlayerEvent, TickOutput};
use crate::physics::aabb::Aabb;

/// How far from (0, 128, 0) a player is on the fight's bar (`validPlayer`: 192 blocks).
const BAR_REACH: f64 = 192.0;

/// How often the bar's players are looked at, and the crystals counted
/// (`EnderDragonFight.tick`).
const PLAYERS_EVERY: u64 = 20;
const CRYSTALS_EVERY: u64 = 100;

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
            self.dragon_fight = Some(Box::new(DragonFight::default()));
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
            if !on_bar.is_empty() || self.published_end.arena_wanted {
                self.keep_arena();
            }
            if self.tick.is_multiple_of(CRYSTALS_EVERY) {
                self.fight_memory.crystals_alive = self.count_crystals();
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
        let _ = output;
        let Some(fight) = self.dragon_fight.as_mut() else {
            return;
        };
        if fight.dragon != Some(key) {
            return;
        }
        fight.dragon_killed = true;
        fight.previously_killed = true;
        self.fight_memory.progress = Some(0.0);
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
