//! The evoker (`Evoker` and `SpellcasterIllager`; `n7k-brief.md` 3.7), part K5: an
//! illager that floats, strolls, keeps away from a creaking and from players (none of
//! whom, all creative, it ever avoids) and looks.
//!
//! Named differences: its spells are N9's (the summoning and the fangs a target's) and
//! the wololo, which turns a blue sheep within 16 red with no target, is not built (its
//! goal holds its place and never starts): its spell's synced id stays at none and its
//! `SpellTicks` at 0.

use clustine_data::{entity_types, sounds};
use clustine_world::BlockPos;

use super::illagers;
use super::pillager::AVOID_CREAKING;
use super::{MobType, Spawning};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{AvoidEntityGoal, FloatGoal, LookAtPlayerGoal, RandomStrollGoal};
use crate::mobs::ai::selector::flags;
use crate::mobs::ai::sensing::Class;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::MobTick;
use crate::mobs::types::tamable::Idle;
use crate::region::Random;
use crate::shown::DataShown;

/// The synced data's index of `SpellcasterIllager.DATA_SPELL_CASTING_ID`.
pub(crate) const SPELL_INDEX: u8 = 17;

/// The evoker.
pub(crate) struct Evoker;

impl MobType for Evoker {
    /// `Evoker.registerGoals`, after `Raider`'s.
    fn register_goals(&self, setup: &mut Setup) {
        illagers::register_raider_goals(setup);
        setup.add_goal(0, FloatGoal);
        // `EvokerCastingSpellGoal`: only while a spell is cast.
        setup.add_goal(
            1,
            Idle {
                flags: flags::MOVE | flags::LOOK,
            },
        );
        setup.add_goal(
            2,
            AvoidEntityGoal {
                class: Class::Player,
                max_distance: 8.0,
                walk_speed: 0.6,
                sprint_speed: 1.0,
            },
        );
        setup.add_goal(
            3,
            AvoidEntityGoal {
                walk_speed: 0.6,
                sprint_speed: 1.0,
                ..AVOID_CREAKING
            },
        );
        // The summoning and the fangs (a target's) and the wololo (not built).
        setup.add_goal(4, Idle { flags: 0 });
        setup.add_goal(5, Idle { flags: 0 });
        setup.add_goal(6, Idle { flags: 0 });
        setup.add_goal(8, RandomStrollGoal::new(0.6));
        setup.add_goal(
            9,
            LookAtPlayerGoal {
                probability: 1.0,
                ..LookAtPlayerGoal::new(Class::Player, 3.0)
            },
        );
        setup.add_goal(10, LookAtPlayerGoal::new(Class::Mob, 8.0));
        crate::mobs::ai::targets::register(setup, crate::mobs::ai::targets::EVOKER);
    }

    /// `Raider.finalizeSpawn`: the evoker has none of its own.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        illagers::finalize_spawn(entity_types::EVOKER as u16, spawn, true);
    }

    /// `SpellcasterIllager.customServerAiStep`: a spell's ticks counted down.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        illagers::update(&mut tick.mob, |own| {
            if own.spell_ticks > 0 {
                own.spell_ticks -= 1;
            }
        });
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        -super::animal::light_cost(ai, pos)
    }

    fn remove_when_far_away(&self, _: u16, mob: &Mob, _: u32, distance: f64) -> bool {
        illagers::remove_when_far_away(mob, distance)
    }

    fn experience_reward(&self, _: u16, mob: &Mob, draws: &mut Random) -> i32 {
        illagers::experience(5, mob, draws)
    }

    /// `readAdditionalSaveData`: the raider's, and `SpellTicks`.
    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        illagers::from_template(mob, tag, |own| {
            own.spell_ticks = super::template::int(tag, "SpellTicks").unwrap_or(0);
        });
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_EVOKER_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_EVOKER_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_EVOKER_DEATH)
    }

    /// The raider's, and the spell being cast, none (`IllagerSpell.NONE`, 0): only a
    /// spell's goal sets another.
    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let mut values = illagers::shown(mob);
        values.push((SPELL_INDEX, DataShown::Byte(0), true));
        values
    }
}
