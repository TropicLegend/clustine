//! The cod (`Cod`; `docs/groundwork/n7-brief.md`, section 6.6.5): a schooling fish of
//! nothing of its own but its bucket and its sounds.

use clustine_data::sounds;

use super::fish::{self, Species};
use super::{Interaction, MobType, Used, water};
use crate::living::Travel;
use crate::mobs::Mob;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::MobTick;
use crate::region::Random;
use crate::shown::DataShown;

/// The cod.
pub(crate) struct Cod;

impl MobType for Cod {
    fn register_goals(&self, setup: &mut Setup) {
        fish::register_goals(setup, Species::Cod);
    }

    fn construct(&self, mob: &mut Mob, own: &mut Random) {
        fish::construct(mob, Species::Cod, own);
    }

    fn after_base_tick(&self, tick: &mut MobTick<'_>, air: i32) {
        water::handle_air_supply(tick, air);
    }

    fn before_ai_step(&self, tick: &mut MobTick<'_>) {
        fish::flop(tick, sounds::ENTITY_COD_FLOP);
    }

    fn after_tick(&self, tick: &mut MobTick<'_>) {
        fish::census(tick);
    }

    fn travel(&self, tick: &MobTick<'_>) -> Travel {
        fish::travel(tick)
    }

    fn tick_move_control(&self, ai: &mut Ai<'_, '_>) {
        fish::tick_move_control(ai);
    }

    fn pushed_by_fluid(&self) -> bool {
        false
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        fish::mob_interact(used)
    }

    fn remove_when_far_away(&self, kind: u16, mob: &Mob, _: u32, _: f64) -> bool {
        fish::remove_when_far_away(kind, mob)
    }

    fn can_be_leashed(&self, _: u16, _: &Mob) -> bool {
        false
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_COD_AMBIENT)
    }

    fn ambient_interval(&self) -> u32 {
        120
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_COD_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_COD_DEATH)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        fish::shown(clustine_data::entity_types::COD as u16, mob, false)
    }
}
