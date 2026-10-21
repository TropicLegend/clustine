//! Anger (`NeutralMob`; the brief's section 5.3): the end of a mob's anger in the world's
//! game time, which clients are shown and compare with their own clock, and whom it is
//! angry at, which `updatePersistentAnger` keeps in step with its target every tick.
//! The wolf's here; any neutral mob's type state may keep an [`Anger`] and call
//! [`update_persistent_anger`] from its `aiStep`.
//!
//! The anger's target is held by key, as a goal holds its target: a player by their
//! stay's entity, which their hand-overs keep.
//!
//! Named differences: the game rules `universal_anger` and `forgive_dead_players` are
//! their defaults (off and on), and nothing tells a mob of a player's death yet; the
//! difficulty is the default, normal.

use clustine_world::EntityId;
use serde::{Deserialize, Serialize};

use super::tamable::find;
use crate::mobs::Mob;
use crate::mobs::ai::sensing::{self, Looked};
use crate::mobs::view::MobWorld;
use crate::region::Random;

/// What `NeutralMob` keeps: `persistentAngerEndTime` (−1 for none) and
/// `persistentAngerTarget`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Anger {
    pub(crate) end: i64,
    pub(crate) target: Option<EntityId>,
}

impl Default for Anger {
    fn default() -> Self {
        Self {
            end: -1,
            target: None,
        }
    }
}

impl Anger {
    /// `isAngry()` at the game time `now`.
    pub(crate) fn is_angry(&self, now: i64) -> bool {
        self.end > 0 && self.end - now > 0
    }

    /// `isAngryAt(entity)`: one it may attack that it is angry at (`universal_anger`
    /// being off).
    pub(crate) fn is_angry_at(&self, target: &Looked) -> bool {
        sensing::can_attack(target) && self.target == Some(target.key)
    }
}

/// `isValidPlayerTarget(entity)`: a player neither creative nor a spectator (the
/// difficulty not peaceful).
pub(crate) fn valid_player_target(target: &Looked) -> bool {
    target.is_player() && !target.creative && !target.spectator
}

/// `stopBeingAngry()`: who hurt it forgotten, no anger target, no target, no end.
pub(crate) fn stop_being_angry(mob: &mut Mob, anger: &mut Anger) {
    mob.body.combat.last_hurt_by = None;
    anger.target = None;
    mob.body.combat.target = None;
    anger.end = -1;
}

/// `updatePersistentAnger(level, ticking)` at the game time `now`: a dead mob it was
/// angry at ends its anger; a target it holds becomes its anger's target and restarts
/// the timer (`timer`, which draws from its own random) where it changed or `ticking`;
/// its anger at nothing valid ends once its time is up; and a player it was angry at
/// who is creative or a spectator is forgiven.
pub(crate) fn update_persistent_anger(
    mob: &mut Mob,
    anger: &mut Anger,
    world: &dyn MobWorld,
    now: i64,
    ticking: bool,
    draws: &mut Random,
    timer: fn(&mut Random) -> i64,
) {
    let unchecked = mob.body.combat.target.and_then(|key| find(world, key));
    let reference = anger.target;
    if let Some(held) = &unchecked
        && !held.alive
        && reference == Some(held.key)
        && held.mob
    {
        stop_being_angry(mob, anger);
        return;
    }
    // `getTarget()`: the target held, while it is a valid one.
    let target = unchecked.filter(sensing::valid_target);
    if let Some(target) = &target {
        let changed = anger.target != Some(target.key);
        if changed {
            anger.target = Some(target.key);
        }
        if changed || ticking {
            anger.end = now + timer(draws);
        }
    }
    if reference.is_some()
        && !anger.is_angry(now)
        && target
            .as_ref()
            .is_none_or(|target| !valid_player_target(target) || !ticking)
    {
        stop_being_angry(mob, anger);
    }
    let was = reference.and_then(|key| find(world, key));
    if was.is_some_and(|was| was.is_player() && (was.creative || was.spectator)) {
        stop_being_angry(mob, anger);
    }
}
