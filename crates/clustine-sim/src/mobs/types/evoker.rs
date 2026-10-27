//! The evoker (`Evoker` and `SpellcasterIllager`; `n7k-brief.md` 3.7, part K5;
//! `docs/groundwork/n9-plan.md`, section 8.4, part S8): an illager that floats, strolls,
//! keeps away from a creaking and from players (none of whom, all creative, it ever
//! avoids) and looks, and casts its spells (`crate::mobs::ai::spells`): at a target, its
//! vexes and its fangs; with none, the wololo that turns a blue sheep red. While it casts
//! it stands still and watches its target, or the sheep, its spell shown (data 17).

use clustine_data::{entity_types, sounds};
use clustine_world::{BlockPos, EntityId};
use serde::{Deserialize, Serialize};

use super::illagers;
use super::pillager::AVOID_CREAKING;
use super::{MobType, Spawning};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{AvoidEntityGoal, FloatGoal, LookAtPlayerGoal, RandomStrollGoal};
use crate::mobs::ai::sensing::Class;
use crate::mobs::ai::spells::{CastingSpellGoal, Spell, UseSpellGoal};
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::{DamageSource, Struck};
use crate::mobs::tick::MobTick;
use crate::region::Random;
use crate::shown::DataShown;

#[cfg(test)]
#[path = "spells_n9_tests.rs"]
mod spells_n9_tests;

/// The synced data's index of `SpellcasterIllager.DATA_SPELL_CASTING_ID`.
pub(crate) const SPELL_INDEX: u8 = 17;

/// The evoker.
pub(crate) struct Evoker;

/// The evoker's own part of its raider state (`crate::mobs::types::illagers`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct EvokerState {
    /// `spellCastingTickCount` (`SpellTicks`).
    pub(crate) spell_ticks: i32,
    /// `currentSpell`, synced as `DATA_SPELL_CASTING_ID` (`IllagerSpell.id`, none 0).
    /// The game does not save it; it is kept with the casting goal that clears it.
    pub(crate) spell: u8,
    /// `wololoTarget`: the sheep its wololo is cast at, from that goal's start to its
    /// stop. The game does not save it either.
    pub(crate) wololo_target: Option<EntityId>,
}

impl MobType for Evoker {
    /// `Evoker.registerGoals`, after `Raider`'s (Evoker.txt 16–297).
    fn register_goals(&self, setup: &mut Setup) {
        illagers::register_raider_goals(setup);
        setup.add_goal(0, FloatGoal);
        setup.add_goal(1, CastingSpellGoal);
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
        setup.add_goal(
            4,
            UseSpellGoal {
                spell: Spell::Summon,
            },
        );
        setup.add_goal(
            5,
            UseSpellGoal {
                spell: Spell::Fangs,
            },
        );
        setup.add_goal(
            6,
            UseSpellGoal {
                spell: Spell::Wololo,
            },
        );
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
        illagers::update_own(&mut tick.mob, |own: &mut EvokerState| {
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

    /// `Raider.aiStep`'s own part (`crate::mobs::ai::raider_goals`).
    fn before_ai_step(&self, tick: &mut MobTick<'_>) {
        illagers::ai_step(tick);
    }

    /// `Raider.hurtServer`'s word to its raid, before the hurt lands.
    fn before_hurt_server(&self, struck: &mut Struck<'_>, _: &DamageSource, _: f32) {
        illagers::hurt(struck);
    }

    /// `Raider.die`'s word to its raid.
    fn on_die(&self, struck: &mut Struck<'_>, source: &DamageSource) {
        illagers::died(struck, source);
    }

    fn requires_custom_persistence(&self, mob: &Mob) -> bool {
        illagers::requires_custom_persistence(mob)
    }

    /// Of the evoker's `xpReward`, 10 (Evoker.txt, its constructor).
    fn experience_reward(&self, _: u16, mob: &Mob, draws: &mut Random) -> i32 {
        illagers::experience(10, mob, draws)
    }

    /// `readAdditionalSaveData`: the raider's, and `SpellTicks`.
    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        illagers::from_template(mob, tag);
        illagers::update_own(mob, |own: &mut EvokerState| {
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

    /// The raider's, and the spell being cast (`IllagerSpell.id`, none 0), by which a
    /// client raises its arms and colours its particles.
    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let spell = illagers::own_of::<EvokerState>(mob).spell;
        let mut values = illagers::shown(mob);
        values.push((SPELL_INDEX, DataShown::Byte(spell), spell == 0));
        values
    }
}
