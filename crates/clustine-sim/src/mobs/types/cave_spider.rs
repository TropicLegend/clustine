//! The cave spider (`CaveSpider`; `n7k-brief.md` 11.4): a spider ([`super::spider`]) of
//! 12 health and its own size, whose `finalizeSpawn` returns the group it is handed and
//! does nothing else: no follow range's bonus, never left-handed, no jockey, no effect,
//! not one draw. Its bite (`CaveSpider.doHurtTarget`) poisons for 7 seconds on normal
//! and 15 on hard, and on easy, Clustine's world, gives nothing and draws nothing: its
//! hit is the spider's.

use clustine_data::sounds;
use clustine_world::BlockPos;

use super::{MobType, monster_ai, spider};
use crate::mobs::Mob;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::MobTick;
use crate::shown::DataShown;

/// The cave spider.
pub(crate) struct CaveSpider;

impl MobType for CaveSpider {
    fn register_goals(&self, setup: &mut Setup) {
        spider::register_goals(setup);
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        monster_ai::walk_target_value(ai, pos)
    }

    /// `CaveSpider.finalizeSpawn` calls none of its parents'.
    fn mob_finalize_spawn(&self) -> bool {
        false
    }

    fn after_tick(&self, tick: &mut MobTick<'_>) {
        spider::after_tick(tick);
    }

    fn on_climbable(&self, mob: &Mob) -> Option<bool> {
        spider::on_climbable(mob)
    }

    fn stuck_in_webs(&self) -> bool {
        false
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SPIDER_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_SPIDER_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SPIDER_DEATH)
    }

    fn step_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SPIDER_STEP)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        spider::shown(mob)
    }
}
