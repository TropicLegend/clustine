//! The elder guardian (`ElderGuardian`; `n7k-brief.md` 3.8), part K5: a guardian kept
//! wherever it is, strolling every 400 ticks or so, kept within 16 of where its AI first
//! ran, and cursing the survival players within 50 every minute.
//!
//! Named difference: every player is creative, so its curse finds nobody; were one
//! found, mining fatigue III and the game event packet `GUARDIAN_ELDER_EFFECT` would not
//! be given, as a player has no effects yet.

use clustine_data::sounds;
use clustine_world::EntityId;

use super::guardian::{Sound, guardian_body};
use super::{MobType, guardians};
use crate::mobs::Mob;
use crate::mobs::ai::Setup;
use crate::mobs::body::Home;
use crate::mobs::tick::MobTick;
use crate::region::Random;

/// The elder guardian.
pub(crate) struct ElderGuardian;

/// `EFFECT_INTERVAL`.
const CURSE_INTERVAL: i32 = 1200;
/// `EFFECT_RADIUS`.
const CURSE_RADIUS: f64 = 50.0;

impl ElderGuardian {
    /// Its sound of `which`, in water or on land.
    fn sound(&self, in_water: bool, which: Sound) -> i32 {
        match (which, in_water) {
            (Sound::Ambient, true) => sounds::ENTITY_ELDER_GUARDIAN_AMBIENT,
            (Sound::Ambient, false) => sounds::ENTITY_ELDER_GUARDIAN_AMBIENT_LAND,
            (Sound::Hurt, true) => sounds::ENTITY_ELDER_GUARDIAN_HURT,
            (Sound::Hurt, false) => sounds::ENTITY_ELDER_GUARDIAN_HURT_LAND,
            (Sound::Death, true) => sounds::ENTITY_ELDER_GUARDIAN_DEATH,
            (Sound::Death, false) => sounds::ENTITY_ELDER_GUARDIAN_DEATH_LAND,
        }
    }
}

/// `MobEffectUtil.addEffectToPlayersAround(level, elder, position, 50, mining fatigue
/// III for 6000, 1200)`'s choice: the living players in survival within 50 of the
/// elder (no player is on a team, and none has an effect that would spare them).
pub(crate) fn cursed_players(tick: &MobTick<'_>) -> Vec<EntityId> {
    let at = tick.living.body.position;
    tick.world
        .players()
        .into_iter()
        .filter(|player| !player.creative && !player.spectator && player.alive)
        .filter(|player| {
            let d = [
                player.position[0] - at[0],
                player.position[1] - at[1],
                player.position[2] - at[2],
            ];
            d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < CURSE_RADIUS * CURSE_RADIUS
        })
        .map(|player| player.key)
        .collect()
}

/// Whether the elder's curse comes in this tick: `(tickCount + getId()) % 1200 == 0`.
pub(crate) fn curses_now(tick: &MobTick<'_>) -> bool {
    let id = tick.key.0 as u32 as i32;
    (tick.tick_count as i32).wrapping_add(id) % CURSE_INTERVAL == 0
}

impl MobType for ElderGuardian {
    fn register_goals(&self, setup: &mut Setup) {
        guardians::register_goals(setup, 400);
    }

    /// `Guardian`'s constructor's draw, then `setPersistenceRequired()`.
    fn construct(&self, mob: &mut Mob, own: &mut Random) {
        guardians::construct(own);
        mob.body.flags.persistent = true;
    }

    /// `ElderGuardian.customServerAiStep`: every 1200 ticks its curse; then its home
    /// where it is, of radius 16, if it has none.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        if curses_now(tick) {
            // Named: nothing is given to whom it finds (none, while every player is
            // creative).
            let _cursed = cursed_players(tick);
        }
        let home = &tick.mob.body.home;
        if home.centre.is_none() || home.radius == -1 {
            tick.mob.body.home = Home {
                centre: Some(crate::physics::containing(tick.living.body.position)),
                radius: 16,
            };
        }
    }

    guardian_body!();
}
