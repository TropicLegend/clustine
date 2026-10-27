//! The monsters' target goals (`docs/groundwork/n9-plan.md`, sections 8.1 and 18; the
//! brief's 1.1 and 1.2): each type's goals by priority with their selectors, as the
//! game's `registerGoals` adds them, and `HurtByTargetGoal.alertOthers`.
//!
//! What each goal draws of the mob's own random, goal by goal (finding 29: not every
//! target goal draws). "Asked" is a tick in which the target selector asks the goal's
//! `canUse`: a full tick of the selector, the goal not running and not blocked by a
//! running target goal of a higher priority.
//!
//! | Goal | Draws, each asked tick |
//! |---|---|
//! | `HurtByTargetGoal` (and its `alertOthers`) | nothing |
//! | `NearestAttackableTargetGoal`, interval 10 | `nextInt(5)` first; it searches only on a 0 |
//! | `NearestAttackableTargetGoal`, interval 0 (the vindicator's Johnny goal) | nothing |
//! | `Spider$SpiderTargetGoal` | nothing in light worth a half or more at its eyes; else `nextInt(5)` |
//! | `NearestHealableRaiderTargetGoal` (the witch's) | `nextBoolean()` while its cooldown is 0 or less, which outside a raid it always is; the raid's rules in `raider_goals.rs` |
//! | `NearestAttackableWitchTargetGoal` (the witch's) | `nextInt(5)`: its `canAttack`, set by the witch's `aiStep` from the healer's cooldown, is always true outside a raid |
//! | `VindicatorJohnnyAttackGoal` | nothing: a vindicator not named Johnny asks nothing more, and the goal's interval is 0 |
//!
//! A search draws nothing either: its line of sight is cast, not drawn, and only a goal
//! that must reach paths to what it found, which `NearestAttackableTargetGoal.canUse`
//! never asks (`TargetGoal.canAttack` is not called on its way).
//!
//! **Left for their parts** (S8 and S9, whose types do not exist yet): the vex's
//! `HurtByTargetGoal(Raider)` with its alert, `VexCopyOwnerTargetGoal` (no draw) and its
//! players; the slime's player goal with its height test; the enderman's
//! `EndermanLookForPlayerGoal` (no draw), its `HurtByTargetGoal`, its endermite goal
//! and its universal anger's reset; the phantom's `PhantomAttackPlayerTargetGoal` (a
//! countdown); the silverfish's `HurtByTargetGoal` with its alert and its players; the
//! endermite's.
//!
//! Named differences: no two mobs are of one team, so none is another's ally; none of
//! the types here is tamable, so no alert passes over another owner's mob; `isInWater`
//! of what a drowned looks at is water in the block of its feet (as the turtles'
//! selector reads it), not the fluid its box touches; a guardian's and a Johnny's
//! search, whose class is `LivingEntity`, takes the entities of the box in the view's
//! order and the players in the box after them, where the game's sections mix them
//! (only the order of two as near could differ).

use clustine_data::entity_types as types;
use clustine_world::EntityId;

use super::goals::{HurtByTargetGoal, TargetRules, Targeting, ai_bounds};
use super::maths::reduced_tick_delay;
use super::selector::{Goal, flags};
use super::sensing::{self, Class, Conditions, Looked};
use super::{Ai, Setup};
use crate::mobs::Mob;
use crate::mobs::tick::Local;
use crate::mobs::types::monster_ai;
use crate::physics::aabb::Aabb;

#[cfg(test)]
#[path = "targets_n9_tests.rs"]
mod tests;

/// A selector that reads the world (`TargetingConditions.Selector`'s `test(target,
/// level)`), given the mob it is asked for. Selectors are pure: they are asked first,
/// before the conditions, as the conditions would ask them before anything that is not.
pub(crate) type Pick = fn(&Ai<'_, '_>, &Looked) -> bool;

const fn kind(id: i32) -> u16 {
    id as u16
}

/// `AbstractVillager`.
pub(crate) const VILLAGERS: &[u16] = &[kind(types::VILLAGER), kind(types::WANDERING_TRADER)];
/// `IronGolem`.
pub(crate) const IRON_GOLEMS: &[u16] = &[kind(types::IRON_GOLEM)];
/// `Turtle`.
pub(crate) const TURTLES: &[u16] = &[kind(types::TURTLE)];
/// `Axolotl`.
pub(crate) const AXOLOTLS: &[u16] = &[kind(types::AXOLOTL)];
/// `Raider`: every type of the class, those Clustine lacks among them.
pub(crate) const RAIDERS: &[u16] = &[
    kind(types::PILLAGER),
    kind(types::VINDICATOR),
    kind(types::EVOKER),
    kind(types::ILLUSIONER),
    kind(types::WITCH),
    kind(types::RAVAGER),
];
/// `Drowned`, which a drowned's hurts from ignore.
pub(crate) const DROWNED: &[u16] = &[kind(types::DROWNED)];
/// The living things `Guardian$GuardianAttackSelector` takes but players: a `Squid`
/// (the glow squid is one) or an `Axolotl`.
const GUARDIAN_PREY: &[u16] = &[
    kind(types::SQUID),
    kind(types::GLOW_SQUID),
    kind(types::AXOLOTL),
];
/// `Zombie`'s subclasses, which a zombie's alert reaches (`getEntitiesOfClass` by
/// `instanceof`).
const ZOMBIES: &[u16] = &[
    kind(types::ZOMBIE),
    kind(types::HUSK),
    kind(types::ZOMBIE_VILLAGER),
    kind(types::DROWNED),
    kind(types::ZOMBIFIED_PIGLIN),
];

/// What a `NearestAttackableTargetGoal` searches among.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Among {
    /// `Player`: every player, by `getNearestPlayer`.
    Players,
    /// A class of mobs, in the box.
    Mobs(Class),
    /// `LivingEntity`: the mobs of the class and the players, in the box.
    Living(Class),
}

/// What a goal asks before its parent's `canUse`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gate {
    /// Nothing.
    None,
    /// `Spider$SpiderTargetGoal`: nothing in light worth a half or more at its eyes.
    InTheDark,
    /// `VindicatorJohnnyAttackGoal`: only a vindicator named Johnny.
    Johnny,
}

/// `NearestAttackableTargetGoal(mob, class, interval, mustSee, mustReach, selector)` and
/// its kinds that only ask something first (`gate`) or remember an unseen target longer
/// (`setUnseenMemoryTicks`).
#[derive(Clone, Copy)]
pub(crate) struct NearestTarget {
    pub(crate) among: Among,
    pub(crate) interval: i32,
    pub(crate) must_see: bool,
    pub(crate) must_reach: bool,
    pub(crate) selector: Option<Pick>,
    /// The unseen memory set, 0 for `TargetGoal`'s default.
    pub(crate) unseen_memory: i32,
    pub(crate) gate: Gate,
    /// `VindicatorJohnnyAttackGoal.start`'s `setNoActionTime(0)`.
    pub(crate) resets_no_action: bool,
}

impl NearestTarget {
    /// `NearestAttackableTargetGoal(mob, class, mustSee)`: of the interval 10.
    pub(crate) const fn new(among: Among, must_see: bool) -> Self {
        Self {
            among,
            interval: 10,
            must_see,
            must_reach: false,
            selector: None,
            unseen_memory: 0,
            gate: Gate::None,
            resets_no_action: false,
        }
    }

    pub(crate) const fn players(must_see: bool) -> Self {
        Self::new(Among::Players, must_see)
    }

    pub(crate) const fn mobs(kinds: &'static [u16], must_see: bool) -> Self {
        Self::new(Among::Mobs(Class::Kinds(kinds)), must_see)
    }

    const fn selected(self, selector: Pick) -> Self {
        Self {
            selector: Some(selector),
            ..self
        }
    }

    const fn rules(&self) -> TargetRules {
        TargetRules {
            must_see: self.must_see,
            must_reach: self.must_reach,
        }
    }
}

impl Goal for NearestTarget {
    type State = Targeting;

    fn flags(&self) -> u8 {
        flags::TARGET
    }

    fn can_use(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        match self.gate {
            Gate::None => {}
            Gate::InTheDark => {
                if monster_ai::magic_value_at_eyes(ai) >= 0.5 {
                    return false;
                }
            }
            Gate::Johnny => {
                if !crate::mobs::types::illagers::own_of::<
                    crate::mobs::types::vindicator::VindicatorState,
                >(&ai.tick.mob)
                .johnny
                {
                    return false;
                }
            }
        }
        let interval = reduced_tick_delay(self.interval);
        if interval > 0 && ai.tick.draws.next_int(interval as u32) != 0 {
            return false;
        }
        let selector = self.selector;
        let found = find_nearest(ai, self.among, &|ai, looked| {
            selector.is_none_or(|test| test(ai, looked))
        });
        state.found = found.map(|found| found.key);
        state.found.is_some()
    }

    fn can_continue(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        self.rules().can_continue(state, ai)
    }

    fn start(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        let found = state.found.and_then(|key| sensing::find(ai, key));
        sensing::set_target(ai, found.as_ref());
        if self.unseen_memory != 0 {
            state.unseen_memory_ticks = self.unseen_memory;
        }
        TargetRules::start(state);
        if self.resets_no_action
            && let Some(since) = &mut ai.tick.mob.body.idle.no_action_since
        {
            // `noActionTime = 0` as the step leaves it: the next step counts it to 1.
            *since = ai.tick.lived + 1;
            ai.tick.mob.body.idle_gained.gained = 0;
        }
    }

    fn stop(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        TargetRules::stop(state, ai);
    }
}

/// `NearestAttackableTargetGoal.findTarget()` among `among`, the range the follow range
/// now, of what `selector` takes: the player nearest the eyes (`getNearestPlayer`) or
/// the nearest of the box (`getNearestEntity` of `getEntitiesOfClass` of the mob's box
/// inflated by the follow range); the earlier of two as near.
pub(crate) fn find_nearest(
    ai: &mut Ai<'_, '_>,
    among: Among,
    selector: &dyn Fn(&Ai<'_, '_>, &Looked) -> bool,
) -> Option<Looked> {
    let follow = TargetRules::follow_distance(ai);
    let conditions = Conditions::combat().range(follow);
    let [x, _, z] = ai.position();
    let eyes = [x, ai.eye_y(), z];
    let bounds = ai_bounds(ai).inflate([follow, follow, follow]);
    let candidates = match among {
        Among::Players => sensing::players(ai),
        Among::Mobs(class) => sensing::things_in(ai, bounds, class),
        Among::Living(class) => {
            let mut things = sensing::things_in(ai, bounds, class);
            things.extend(sensing::things_in(ai, bounds, Class::Player));
            things
        }
    };
    let candidates = candidates
        .into_iter()
        .filter(|looked| selector(ai, looked))
        .collect();
    sensing::nearest(ai, candidates, &conditions, &|_| true, eyes)
}

/// `HurtByTargetGoal(mob, ignoreDamageFrom...).setAlertOthers(toIgnoreAlert...)`: the
/// game's goal, and in its `start`, `alertOthers` (section 8.1). The class alerted is
/// the mob's own (`getClass()`) and what it ignores is the class's (`alert_ignores`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct HurtByAlertingGoal {
    pub(crate) ignore: &'static [u16],
}

impl HurtByAlertingGoal {
    const fn inner(self) -> HurtByTargetGoal {
        HurtByTargetGoal {
            ignore: self.ignore,
        }
    }
}

impl Goal for HurtByAlertingGoal {
    type State = Targeting;

    fn flags(&self) -> u8 {
        flags::TARGET
    }

    fn can_use(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        self.inner().can_use(state, ai)
    }

    fn can_continue(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        self.inner().can_continue(state, ai)
    }

    fn start(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        self.inner().start(state, ai);
        alert_others(ai);
    }

    fn stop(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        self.inner().stop(state, ai);
    }
}

/// Whether a mob of the type `kind` is of the class `class` as `getEntitiesOfClass`
/// asks (`instanceof`), of the types Clustine's alerts name.
pub(crate) fn of_class(class: u16, kind: u16) -> bool {
    kind == class || (class == self::kind(types::ZOMBIE) && ZOMBIES.contains(&kind))
}

/// The classes an alert of the class `class` passes over (`toIgnoreAlert`, by exact
/// class, finding 28): the zombie's and the drowned's `ZombifiedPiglin`, inherited by
/// the husk and the zombie villager; none for the raiders.
pub(crate) fn alert_ignores(class: u16) -> &'static [u16] {
    const PIGLINS: &[u16] = &[kind(types::ZOMBIFIED_PIGLIN)];
    if ZOMBIES.contains(&class) {
        PIGLINS
    } else {
        &[]
    }
}

/// Whether a mob of the type `kind` comes when one of the class `class` alerts: of the
/// class and not of a class it ignores.
pub(crate) fn comes_to_alert(class: u16, kind: u16) -> bool {
    of_class(class, kind) && !alert_ignores(class).contains(&kind)
}

/// `HurtByTargetGoal.alertOthers()`: every mob of its class in the unit cube at its feet
/// inflated by the follow range across and 10 up and down, in the view's order, but
/// itself, one already with a target and one of a class it ignores, is given the
/// attacker as its target: `Local::Alert`, which the region applies to its own by the
/// same tests and sends to one in a copy (`EntityUse::Alert`). No draws.
fn alert_others(ai: &mut Ai<'_, '_>) {
    let Some((attacker, _)) = ai.tick.mob.body.combat.last_hurt_by else {
        return;
    };
    let attacker_kind = sensing::find(ai, attacker)
        .and_then(|found| found.kind)
        .unwrap_or(kind(types::PLAYER));
    let class = ai.tick.kind;
    let follow = TargetRules::follow_distance(ai);
    let [x, y, z] = ai.position();
    let bounds = Aabb::new([x, y, z], [x + 1.0, y + 1.0, z + 1.0]).inflate([follow, 10.0, follow]);
    let mut alerted: Vec<EntityId> = Vec::new();
    for seen in ai.tick.world.entities_in(&bounds) {
        let entity = seen.entity;
        if entity.key == ai.tick.key || !comes_to_alert(class, entity.kind) {
            continue;
        }
        if Mob::decode(&entity.data).is_ok_and(|mob| mob.body.combat.target.is_some()) {
            continue;
        }
        alerted.push(entity.key);
    }
    for key in alerted {
        ai.tick.out.local.push(Local::Alert {
            key,
            attacker,
            kind: attacker_kind,
            class,
        });
    }
}

/// One target goal of a type's, as the table of its type lists it.
#[derive(Clone, Copy)]
pub(crate) enum Target {
    /// `HurtByTargetGoal(mob, ignore...)`, with `setAlertOthers` where `alert`.
    HurtBy {
        ignore: &'static [u16],
        alert: bool,
    },
    Nearest(NearestTarget),
    /// The witch's `NearestHealableRaiderTargetGoal`.
    HealableRaider,
}

/// Adds the target goals `goals` of a type's table, by priority.
pub(crate) fn register(setup: &mut Setup, goals: &[(i32, Target)]) {
    for &(priority, goal) in goals {
        match goal {
            Target::HurtBy {
                ignore,
                alert: false,
            } => setup.add_target(priority, HurtByTargetGoal { ignore }),
            Target::HurtBy {
                ignore,
                alert: true,
            } => {
                setup.add_target(priority, HurtByAlertingGoal { ignore });
            }
            Target::Nearest(goal) => setup.add_target(priority, goal),
            Target::HealableRaider => setup.add_target(
                priority,
                crate::mobs::ai::raider_goals::NearestHealableRaiderTargetGoal,
            ),
        }
    }
}

/// `Turtle.BABY_ON_LAND_SELECTOR`: a baby turtle out of water.
fn baby_turtle_on_land(ai: &Ai<'_, '_>, looked: &Looked) -> bool {
    crate::mobs::types::tamable::baby_turtle_on_land(ai.tick.world, looked)
}

/// `Drowned.okTarget(target)`: by night, anything; by day only what is in water.
pub(crate) fn drowned_ok_target(ai: &Ai<'_, '_>, looked: &Looked) -> bool {
    !monster_ai::bright_outside(ai)
        || clustine_blocks::data::has_water(ai.tick.world.state(looked.block_position()))
}

/// `Guardian$GuardianAttackSelector`: a player, a squid or an axolotl farther than 3.
pub(crate) fn guardian_prey(ai: &Ai<'_, '_>, looked: &Looked) -> bool {
    let prey = looked.is_player()
        || looked
            .kind
            .is_some_and(|kind| GUARDIAN_PREY.contains(&kind));
    prey && looked.distance_sqr(ai.position()) > 9.0
}

/// `AbstractIllager.canAttack(target)` beyond `Mob`'s, as a selector of its goals: never
/// a baby villager.
pub(crate) fn illager_can_attack(ai: &Ai<'_, '_>, looked: &Looked) -> bool {
    let Some(kind) = looked.kind.filter(|kind| VILLAGERS.contains(kind)) else {
        return true;
    };
    let baby = ai.tick.world.entity(looked.key).is_some_and(|seen| {
        Mob::decode(&seen.entity.data).is_ok_and(|mob| crate::mobs::types::of(kind).is_baby(&mob))
    });
    !baby
}

/// The Johnny goal's selector, `LivingEntity.attackable()`, which every mob and player
/// is (only an armour stand is not, and it is no mob), with the illagers' `canAttack`.
fn johnny_can_attack(ai: &Ai<'_, '_>, looked: &Looked) -> bool {
    illager_can_attack(ai, looked)
}

/// The baby turtles on land of the zombies and the skeletons (interval 10, must see,
/// need not reach).
const BABY_TURTLES: NearestTarget =
    NearestTarget::mobs(TURTLES, true).selected(baby_turtle_on_land);

/// `Zombie.addBehaviourGoals`' targets, the husk's and the zombie villager's too
/// (Zombie.txt 84–197): who hurt it, alerting its class but zombified piglins; players
/// it sees; villagers seen or not; iron golems it sees; baby turtles on land.
pub(crate) const ZOMBIE: &[(i32, Target)] = &[
    (
        1,
        Target::HurtBy {
            ignore: &[],
            alert: true,
        },
    ),
    (2, Target::Nearest(NearestTarget::players(true))),
    (3, Target::Nearest(NearestTarget::mobs(VILLAGERS, false))),
    (3, Target::Nearest(NearestTarget::mobs(IRON_GOLEMS, true))),
    (5, Target::Nearest(BABY_TURTLES)),
];

/// `Drowned.addBehaviourGoals`' targets (Drowned.txt 120–261): who hurt it but another
/// drowned, alerting drowned; players by `okTarget` (interval 10, must see); villagers,
/// iron golems, axolotls; baby turtles on land.
pub(crate) const DROWNED_TARGETS: &[(i32, Target)] = &[
    (
        1,
        Target::HurtBy {
            ignore: DROWNED,
            alert: true,
        },
    ),
    (
        2,
        Target::Nearest(NearestTarget::players(true).selected(drowned_ok_target)),
    ),
    (3, Target::Nearest(NearestTarget::mobs(VILLAGERS, false))),
    (3, Target::Nearest(NearestTarget::mobs(IRON_GOLEMS, true))),
    (3, Target::Nearest(NearestTarget::mobs(AXOLOTLS, true))),
    (5, Target::Nearest(BABY_TURTLES)),
];

/// `AbstractSkeleton.registerGoals`' targets: who hurt it (no alert); players and iron
/// golems it sees; baby turtles on land.
pub(crate) const SKELETON: &[(i32, Target)] = &[
    (
        1,
        Target::HurtBy {
            ignore: &[],
            alert: false,
        },
    ),
    (2, Target::Nearest(NearestTarget::players(true))),
    (3, Target::Nearest(NearestTarget::mobs(IRON_GOLEMS, true))),
    (3, Target::Nearest(BABY_TURTLES)),
];

/// `Creeper.registerGoals`' targets: players it sees, then who hurt it.
pub(crate) const CREEPER: &[(i32, Target)] = &[
    (1, Target::Nearest(NearestTarget::players(true))),
    (
        2,
        Target::HurtBy {
            ignore: &[],
            alert: false,
        },
    ),
];

/// `Spider$SpiderTargetGoal(spider, class)`: `NearestAttackableTargetGoal(mob, class,
/// true)` asked only in the dark.
const fn spider_target(among: Among) -> NearestTarget {
    NearestTarget {
        gate: Gate::InTheDark,
        ..NearestTarget::new(among, true)
    }
}

/// `Spider.registerGoals`' targets, the cave spider's too: who hurt it; players and iron
/// golems, in the dark.
pub(crate) const SPIDER: &[(i32, Target)] = &[
    (
        1,
        Target::HurtBy {
            ignore: &[],
            alert: false,
        },
    ),
    (2, Target::Nearest(spider_target(Among::Players))),
    (
        3,
        Target::Nearest(spider_target(Among::Mobs(Class::Kinds(IRON_GOLEMS)))),
    ),
];

/// `Witch.registerGoals`' targets: who hurt it but a raider (no alert); the healer of
/// raiders; players (interval 10, must see).
pub(crate) const WITCH: &[(i32, Target)] = &[
    (
        1,
        Target::HurtBy {
            ignore: RAIDERS,
            alert: false,
        },
    ),
    (2, Target::HealableRaider),
    (3, Target::Nearest(NearestTarget::players(true))),
];

/// An illager's `NearestAttackableTargetGoal`: its `canAttack` refuses baby villagers.
const fn illager_target(among: Among, must_see: bool) -> NearestTarget {
    NearestTarget::new(among, must_see).selected(illager_can_attack)
}

/// The raiders' `HurtByTargetGoal(Raider).setAlertOthers()`.
const RAIDER_HURT_BY: Target = Target::HurtBy {
    ignore: RAIDERS,
    alert: true,
};

/// `Vindicator.registerGoals`' targets: who hurt it but a raider, alerting vindicators;
/// players, villagers and iron golems it sees; Johnny's, of anything living
/// (interval 0, must see and reach).
pub(crate) const VINDICATOR: &[(i32, Target)] = &[
    (1, RAIDER_HURT_BY),
    (2, Target::Nearest(illager_target(Among::Players, true))),
    (
        3,
        Target::Nearest(illager_target(Among::Mobs(Class::Kinds(VILLAGERS)), true)),
    ),
    (
        3,
        Target::Nearest(illager_target(Among::Mobs(Class::Kinds(IRON_GOLEMS)), true)),
    ),
    (
        4,
        Target::Nearest(NearestTarget {
            interval: 0,
            must_reach: true,
            gate: Gate::Johnny,
            resets_no_action: true,
            ..NearestTarget::new(Among::Living(Class::Mob), true).selected(johnny_can_attack)
        }),
    ),
];

/// `Pillager.registerGoals`' targets: who hurt it but a raider, alerting pillagers;
/// players it sees; villagers seen or not; iron golems it sees.
pub(crate) const PILLAGER: &[(i32, Target)] = &[
    (1, RAIDER_HURT_BY),
    (2, Target::Nearest(illager_target(Among::Players, true))),
    (
        3,
        Target::Nearest(illager_target(Among::Mobs(Class::Kinds(VILLAGERS)), false)),
    ),
    (
        3,
        Target::Nearest(illager_target(Among::Mobs(Class::Kinds(IRON_GOLEMS)), true)),
    ),
];

/// `Evoker.registerGoals`' targets: who hurt it but a raider, alerting evokers; players
/// it sees and villagers seen or not, each remembered 300 ticks unseen; iron golems seen
/// or not.
pub(crate) const EVOKER: &[(i32, Target)] = &[
    (1, RAIDER_HURT_BY),
    (
        2,
        Target::Nearest(NearestTarget {
            unseen_memory: 300,
            ..illager_target(Among::Players, true)
        }),
    ),
    (
        3,
        Target::Nearest(NearestTarget {
            unseen_memory: 300,
            ..illager_target(Among::Mobs(Class::Kinds(VILLAGERS)), false)
        }),
    ),
    (
        3,
        Target::Nearest(illager_target(
            Among::Mobs(Class::Kinds(IRON_GOLEMS)),
            false,
        )),
    ),
];

/// `Guardian.registerGoals`' target, the elder's too: `NearestAttackableTargetGoal(
/// LivingEntity, 10, true, false, GuardianAttackSelector)`.
pub(crate) const GUARDIAN: &[(i32, Target)] = &[(
    1,
    Target::Nearest(NearestTarget {
        selector: Some(guardian_prey),
        ..NearestTarget::new(Among::Living(Class::Kinds(GUARDIAN_PREY)), true)
    }),
)];
