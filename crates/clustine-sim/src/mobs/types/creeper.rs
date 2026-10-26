//! The creeper (`Creeper`; `docs/groundwork/n7k-brief.md`, section 11.3): it floats,
//! runs from ocelots and cats within six blocks, strolls and looks about; it has no
//! ambient sound. Flint and steel or a fire charge used on it ignites it (the item's
//! sound, of the creeper's own pitch, for all but the player who used it): from the next
//! tick it swells one a tick, `creeper_primed` at the first, and at 30 it explodes where
//! it stands, of radius 3 (6 if powered), as a mob's explosion (`ExplosionInteraction.MOB`:
//! blocks broken, as `mob_griefing` is on, their drops decaying), and is gone with no death
//! and no loot (`region/explosions.rs`).
//!
//! Its swell and the swell before it are kept in its record with the rest (the game keeps
//! them in fields it does not save), so that a restore or a crossing mid-swell goes on.
//!
//! Named differences: with no target it never swells but ignited (`SwellGoal` takes no
//! target, N9's), and its melee goal and target goals are N9's; a fall's swelling
//! (`causeFallDamage`, which only shortens a later fuse) is not kept; lightning, which
//! would power it, never strikes; the item's wear is not kept, every player being
//! creative; whether it is in water, which a frame's or a painting's ignoring of its
//! explosion reads, is the water at its feet as its tick begins (**Inferred**).

use clustine_blocks::data::{has_water, item_in_tag};
use clustine_data::{entity_types, items, sounds};
use clustine_world::{BlockPos, TypeState};
use serde::{Deserialize, Serialize};

use super::monster_ai::{self, AttackOfN9, TargetOfN9};
use super::{Did, Interaction, MobType, Used, template};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    AvoidEntityGoal, FloatGoal, LookAtPlayerGoal, RandomLookAroundGoal, RandomStrollGoal,
};
use crate::mobs::ai::navigation;
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::{self, Class};
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::Event;
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::body::ladder;
use crate::mobs::tick::{Local, MobTick};
use crate::shown::DataShown;

/// The version of a creeper's type state.
const VERSION: u8 = 1;

/// The synced values' indices: `DATA_SWELL_DIR`, `DATA_IS_POWERED`, `DATA_IS_IGNITED`.
const SWELL_DIR_INDEX: u8 = 16;
const POWERED_INDEX: u8 = 17;
const IGNITED_INDEX: u8 = 18;

/// What a creeper keeps: its synced direction of swelling, power and ignition; its swell
/// and the swell of the tick before (`swell`, `oldSwell`); its fuse (`maxSwell`, `Fuse`)
/// and its explosion's radius (`explosionRadius`, `ExplosionRadius`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct CreeperState {
    pub(crate) swell_dir: i32,
    pub(crate) powered: bool,
    pub(crate) ignited: bool,
    pub(crate) swell: i32,
    pub(crate) old_swell: i32,
    pub(crate) max_swell: i32,
    pub(crate) explosion_radius: i32,
}

impl Default for CreeperState {
    /// The constructor's: no swelling (−1), a fuse of 30 and a radius of 3.
    fn default() -> Self {
        Self {
            swell_dir: -1,
            powered: false,
            ignited: false,
            swell: 0,
            old_swell: 0,
            max_swell: 30,
            explosion_radius: 3,
        }
    }
}

/// A creeper's state from its type state; a record of nothing, or one this build cannot
/// read, as a fresh creeper's.
pub(crate) fn state_of(mob: &Mob) -> CreeperState {
    if mob.state.bytes.is_empty() || mob.state.version != VERSION {
        return CreeperState::default();
    }
    postcard::from_bytes(&mob.state.bytes).unwrap_or_default()
}

/// Its state written back; a fresh one's as nothing.
pub(crate) fn put_state(mob: &mut Mob, own: &CreeperState) {
    mob.state = if *own == CreeperState::default() {
        TypeState::default()
    } else {
        TypeState {
            version: VERSION,
            bytes: postcard::to_allocvec(own).expect("a type state is encoded into memory"),
        }
    };
}

fn change(mob: &mut Mob, change: impl FnOnce(&mut CreeperState)) {
    let mut own = state_of(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        put_state(mob, &own);
    }
}

/// The ocelot and the cat, each of a goal of its own.
const OCELOTS: &[u16] = &[entity_types::OCELOT as u16];
const CATS: &[u16] = &[entity_types::CAT as u16];

/// The creeper.
pub(crate) struct Creeper;

/// `SwellGoal(creeper)`: while it swells or a living target is within 3, it stops and
/// keeps its target; each tick its direction is +1 only for a target within 7 that it
/// sees, else −1. Without a target (N9) it runs only while ignited, and sets −1 each
/// tick, which the next tick's ignition sets back before it counts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SwellGoal;

impl Goal for SwellGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        if state_of(&ai.tick.mob).swell_dir > 0 {
            return true;
        }
        sensing::target(ai)
            .is_some_and(|target| target.alive && target.distance_sqr(ai.position()) < 9.0)
    }

    fn start(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        navigation::stop(ai);
    }

    fn tick(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        let near = sensing::target(ai)
            .filter(|target| target.alive && target.distance_sqr(ai.position()) <= 49.0);
        let seen = near.is_some_and(|target| sensing::has_line_of_sight(ai, &target));
        let dir = if seen { 1 } else { -1 };
        change(&mut ai.tick.mob, |own| own.swell_dir = dir);
    }
}

impl MobType for Creeper {
    fn register_goals(&self, setup: &mut Setup) {
        setup.add_goal(1, FloatGoal);
        setup.add_goal(2, SwellGoal);
        setup.add_goal(
            3,
            AvoidEntityGoal {
                class: Class::Kinds(OCELOTS),
                max_distance: 6.0,
                walk_speed: 1.0,
                sprint_speed: 1.2,
            },
        );
        setup.add_goal(
            3,
            AvoidEntityGoal {
                class: Class::Kinds(CATS),
                max_distance: 6.0,
                walk_speed: 1.0,
                sprint_speed: 1.2,
            },
        );
        // `MeleeAttackGoal(1.0, false)`: N9's.
        setup.add_goal(
            4,
            AttackOfN9 {
                flags: flags::MOVE | flags::LOOK,
            },
        );
        setup.add_goal(5, RandomStrollGoal::avoiding_water(0.8));
        setup.add_goal(6, LookAtPlayerGoal::new(Class::Player, 8.0));
        setup.add_goal(6, RandomLookAroundGoal);
        setup.add_target(1, TargetOfN9::NEAREST);
        setup.add_target(2, TargetOfN9::HURT_BY);
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        monster_ai::walk_target_value(ai, pos)
    }

    /// `Creeper.tick` before `Monster`'s, while it lives: the swell before kept; ignited,
    /// it swells; at its first swell its sound and `prime_fuse`; the swell counted (never
    /// below nothing), and at its fuse the explosion.
    fn before_tick(&self, tick: &mut MobTick<'_>) {
        if tick.mob.body.vitals.dying() {
            return;
        }
        let mut own = state_of(&tick.mob);
        own.old_swell = own.swell;
        if own.ignited {
            own.swell_dir = 1;
        }
        let dir = own.swell_dir;
        let at = tick.living.body.position;
        if dir > 0 && own.swell == 0 {
            tick.out.events.push(Event::Sound {
                sound: sounds::ENTITY_CREEPER_PRIMED,
                volume: 1.0,
                pitch: 0.5,
            });
            tick.out.local.push(Local::GameEventAt {
                name: "minecraft:prime_fuse",
                at,
                affected: None,
            });
        }
        own.swell = (own.swell + dir).max(0);
        if own.swell >= own.max_swell {
            own.swell = own.max_swell;
            // `explodeCreeper`.
            let scale = if own.powered { 2.0 } else { 1.0 };
            let in_water = has_water(tick.world.state(crate::physics::containing(at)));
            tick.out.local.push(Local::Explode {
                at,
                radius: own.explosion_radius as f32 * scale,
                in_water,
            });
        }
        put_state(&mut tick.mob, &own);
    }

    /// `Creeper.mobInteract`: an igniter (`#creeper_igniters`) plays its use's sound at
    /// the creeper, of the creeper's sound source and its own pitch, for all but the
    /// player, and ignites it.
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        let Some(held) = used.held.filter(|stack| stack.count > 0) else {
            return Interaction::Pass;
        };
        if !item_in_tag(held.item, "minecraft:creeper_igniters") {
            return Interaction::Pass;
        }
        let sound = if held.item == items::FIRE_CHARGE {
            sounds::ITEM_FIRECHARGE_USE
        } else {
            sounds::ITEM_FLINTANDSTEEL_USE
        };
        let pitch = used.draws.next_float() * 0.4 + 0.8;
        used.did.push(Did::Sound {
            sound,
            source: ladder::sound_source(used.kind),
            at: used.position,
            volume: 1.0,
            pitch,
            but_the_player: true,
        });
        change(used.mob, |own| own.ignited = true);
        Interaction::Success
    }

    /// `readAdditionalSaveData`: `powered`, and `ignite()` where `ignited`. A template's
    /// `Fuse` and `ExplosionRadius` (a short and a byte) are not read: no template holds
    /// a creeper.
    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        let powered = template::boolean(tag, "powered").unwrap_or(false);
        let ignited = template::boolean(tag, "ignited").unwrap_or(false);
        change(mob, |own| {
            own.powered = powered;
            own.ignited |= ignited;
        });
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_CREEPER_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_CREEPER_DEATH)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let own = state_of(mob);
        vec![
            (
                SWELL_DIR_INDEX,
                DataShown::Int(own.swell_dir),
                own.swell_dir == -1,
            ),
            (POWERED_INDEX, DataShown::Boolean(own.powered), !own.powered),
            (IGNITED_INDEX, DataShown::Boolean(own.ignited), !own.ignited),
        ]
    }
}
