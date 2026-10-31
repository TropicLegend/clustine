//! The End's dragon fight as the dragon reads it and tells it (`EnderDragonFight`;
//! `docs/groundwork/p-plan.md`, sections 19.2 E9 and the fight, and 19.5; the brief's
//! section 6). The fight is kept by the End's home region in its state
//! (`RegionState::dragon_fight`); the dragon lives in the arena, which only the home
//! holds, so the dragon and its fight are always in one region.
//!
//! This file is the seam between the dragon (`mobs/types/ender_dragon.rs`) and the
//! fight: what the dragon reads of the fight each tick ([`FightView`]), and what it tells
//! it ([`Region::fight_dragon_updated`], [`Region::fight_dragon_killed`]). The fight's
//! own tick (scanning for an old portal, making the dragon, the bar, the crystals
//! counted, the exit portal, the egg and the gateways) fills them.

#![expect(
    dead_code,
    reason = "the dragon (E9) and the fight's step call into this seam; remove this when they do"
)]

use clustine_world::{BlockPos, Dimension, EntityId};

use super::Region;
use crate::api::TickOutput;

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

impl Region {
    /// What the dragon `key` reads of its fight: none outside the End's home, or for a
    /// dragon that is not the fight's own.
    pub(crate) fn fight_view(&self, key: EntityId) -> Option<FightView> {
        if self.config.dimension != Dimension::End {
            return None;
        }
        let fight = self.dragon_fight.as_ref()?;
        if fight.dragon != Some(key) {
            return None;
        }
        Some(FightView {
            crystals_alive: self.fight_crystals_alive(),
            podium: self.podium_top(),
            previously_killed: fight.previously_killed,
        })
    }

    /// `EnderDragonFight.updateDragon`: the dragon `key`, of `health` out of `max`, as
    /// its `aiStep` tells it each tick; the bar shows it. Filled by the fight's step.
    pub(crate) fn fight_dragon_updated(&mut self, key: EntityId, health: f32, max: f32) {
        let _ = (key, health, max);
    }

    /// `EnderDragonFight.setDragonKilled`: the dragon `key` has finished dying (its
    /// `tickDeath` at 200) and is removed. Filled by the fight's step: the exit portal
    /// opened, the egg where it was never killed before, the next gateway.
    pub(crate) fn fight_dragon_killed(&mut self, key: EntityId, output: &mut TickOutput) {
        let _ = (key, output);
    }

    /// The crystals on the spikes as the fight counts them. Filled by the fight's step.
    fn fight_crystals_alive(&self) -> u32 {
        0
    }

    /// The top of the podium's column at the fight's origin (0, 0): the highest block
    /// that stops movement, not counting leaves, and one above it; the floor of the
    /// level where the region cannot read it.
    fn podium_top(&self) -> BlockPos {
        let y = (self.readable_chunk(clustine_world::ChunkPos::new(0, 0)))
            .map(|chunk| chunk.min_y() + i32::from(chunk.heights().motion_blocking_no_leaves[0]))
            .unwrap_or(self.config.lowest_y);
        BlockPos::new(0, y, 0)
    }
}
