//! The glow squid (`GlowSquid`; `docs/groundwork/n7-brief.md`, section 6.8.6): a squid
//! that goes dark for 100 ticks when hurt, its dark ticks shown to clients as they count
//! down, and that draws three numbers every tick for a glow the server never shows (the
//! brief's W13).

use clustine_data::sounds;

use super::squid::{self, SquidState};
use super::{Breeding, Dimensions, Interaction, MobType, Used, animal, water};
use crate::living::Travel;
use crate::mobs::Mob;
use crate::mobs::ai::Setup;
use crate::mobs::body::hurt::{DamageSource, Struck};
use crate::mobs::tick::MobTick;
use crate::region::Random;
use crate::shown::DataShown;

/// How long a hurt keeps it dark.
const DARK_TICKS: i32 = 100;

/// The glow squid.
pub(crate) struct GlowSquid;

/// `GlowSquid.aiStep()` after the squid's: its dark ticks count down, and the glow's
/// place is drawn (`getRandomX(0.6)`, `getRandomY()`, `getRandomZ(0.6)`) and not sent.
fn glow(tick: &mut MobTick<'_>) {
    squid::update(&mut tick.mob, |own: &mut SquidState| {
        if own.dark_ticks > 0 {
            own.dark_ticks -= 1;
        }
    });
    for _ in 0..3 {
        tick.draws.next_double();
    }
}

impl MobType for GlowSquid {
    fn register_goals(&self, setup: &mut Setup) {
        squid::register_goals(setup);
    }

    fn construct(&self, mob: &mut Mob, own: &mut Random) {
        squid::construct(mob, own);
    }

    fn after_base_tick(&self, tick: &mut MobTick<'_>, air: i32) {
        water::handle_air_supply(tick, air);
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step(tick);
        squid::stroke(tick);
        glow(tick);
    }

    fn travel(&self, _: &MobTick<'_>) -> Travel {
        Travel::Bare
    }

    /// The squid's, its ink of its own kind, then dark for 100 ticks where it took.
    fn after_hurt(&self, struck: &mut Struck<'_>, _: &DamageSource, took: bool) {
        squid::squirt(struck, took, sounds::ENTITY_GLOW_SQUID_SQUIRT);
        if took {
            squid::update(struck.mob, |own| own.dark_ticks = DARK_TICKS);
        }
    }

    fn pushed_by_fluid(&self) -> bool {
        false
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        squid::mob_interact(used)
    }

    fn breed_offspring(&self, _: &mut Breeding<'_>) -> Option<Mob> {
        Some(squid::offspring())
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        animal::dimensions(mob, squid::BABY)
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_GLOW_SQUID_AMBIENT)
    }

    fn ambient_interval(&self) -> u32 {
        120
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_GLOW_SQUID_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_GLOW_SQUID_DEATH)
    }

    fn sound_volume(&self) -> f32 {
        0.4
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        squid::shown(mob, true)
    }
}
