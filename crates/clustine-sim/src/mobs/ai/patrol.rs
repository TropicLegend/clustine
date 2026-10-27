//! Patrols (`docs/groundwork/n9-plan.md`, section 8.6): `PatrollingMonster`'s
//! `LongDistancePatrolGoal`, which every raider registers at 4 (n9
//! PatrollingMonster$LongDistancePatrolGoal), its leader's `findPatrolTarget` and the
//! ominous banner it wears (`Raid.getOminousBannerInstance`, n7k Raid 1649–1660).
//!
//! The game of 26.3 has no other patrol goal: the leader walks towards its patrol target
//! ten blocks at a time, setting each step's end as the target of its companions, who
//! walk to it in the same goal, and finds a new target once within ten blocks of its own.
//!
//! **Named**: the goal's cooldown after a failed path counts in the mob's own ticks
//! (`lived`), the time the AI measures by (`crate::mobs::ai`'s module comment), where the
//! game counts the level's game time; a mob ridden by another mob is not seen as
//! controlled (no raider carries one in N9).

use clustine_data::{entity_types, items};
use clustine_world::components::{BannerLayer, Rarity, TooltipDisplay};
use clustine_world::{BlockPos, Component, EntityId, ItemStack};
use serde::{Deserialize, Serialize};

use super::selector::{Goal, flags};
use super::{Ai, navigation};
use crate::mobs::tick::{Local, Ridden};
use crate::mobs::types::illagers;
use crate::physics::{self, Vec3, aabb::Aabb};
use crate::region::Random;

/// The types that are `PatrollingMonster`s and so can join a patrol (`canJoinPatrol`,
/// a raider's while it has no raid). The illusioner is not built. **Named**: the
/// ravager, a `PatrollingMonster` too, has no `LongDistancePatrolGoal` here and so never
/// joins a patrol (only one made by an egg could; patrols spawn only these four).
pub(crate) const PATROLLING: [u16; 4] = [
    entity_types::PILLAGER as u16,
    entity_types::VINDICATOR as u16,
    entity_types::EVOKER as u16,
    entity_types::WITCH as u16,
];

/// Whether a mob of the type `kind` is a `PatrollingMonster` that can join a patrol.
pub(crate) fn can_join_patrol(kind: u16) -> bool {
    PATROLLING.contains(&kind)
}

/// `NAVIGATION_FAILED_COOLDOWN`.
const NAVIGATION_FAILED_COOLDOWN: i64 = 200;

/// `Raid.getOminousBannerInstance(patterns)`: a white banner with the eight layers of
/// `getBannerComponentPatch` (each pattern registered, so each added), its patterns'
/// lines hidden, named "Ominous Banner" and uncommon.
pub(crate) fn ominous_banner() -> ItemStack {
    const LAYERS: [(&str, &str); 8] = [
        ("minecraft:rhombus", "cyan"),
        ("minecraft:stripe_bottom", "light_gray"),
        ("minecraft:stripe_center", "gray"),
        ("minecraft:border", "light_gray"),
        ("minecraft:stripe_middle", "black"),
        ("minecraft:half_horizontal", "light_gray"),
        ("minecraft:circle", "light_gray"),
        ("minecraft:border", "black"),
    ];
    let mut stack = ItemStack::new(items::WHITE_BANNER, 1);
    stack.components.set(Component::BannerPatterns(
        LAYERS
            .iter()
            .map(|(pattern, colour)| BannerLayer {
                pattern: (*pattern).to_owned(),
                colour: (*colour).to_owned(),
            })
            .collect(),
    ));
    stack
        .components
        .set(Component::TooltipDisplay(TooltipDisplay {
            hide_tooltip: false,
            hidden_components: vec!["minecraft:banner_patterns".to_owned()],
        }));
    stack.components.set(Component::ItemName(
        r#"{"translate":"block.minecraft.ominous_banner"}"#.to_owned(),
    ));
    stack.components.set(Component::Rarity(Rarity::Uncommon));
    stack
}

/// `PatrollingMonster.findPatrolTarget()` of a mob whose block is `at`: up to 500 blocks
/// off along x and z, two `nextInt(1000)` of the mob's own random, at its height.
pub(crate) fn find_patrol_target(at: BlockPos, own: &mut Random) -> BlockPos {
    let x = -500 + own.next_int(1000) as i32;
    let z = -500 + own.next_int(1000) as i32;
    BlockPos::new(at.x + x, at.y, at.z + z)
}

/// `PatrollingMonster$LongDistancePatrolGoal(mob, 0.7, 0.595)`: a companion's speed and
/// the leader's.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LongDistancePatrolGoal {
    pub(crate) speed: f64,
    pub(crate) leader_speed: f64,
}

/// `PatrollingMonster.registerGoals`' goal.
pub(crate) const PATROL_GOAL: LongDistancePatrolGoal = LongDistancePatrolGoal {
    speed: 0.7,
    leader_speed: 0.595,
};

/// What the goal keeps: `cooldownUntil`, in the mob's own ticks (none before a failed
/// path: the game's −1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Cooldown {
    pub(crate) until: Option<i64>,
}

/// Where the patrol's walk goes next from `position` towards the patrol target
/// `target`: ten blocks along the line from the mob to the target's bottom middle turned
/// a little aside, the line from the mob to the target turned by `yRot(90)` (90 radians,
/// as the game writes it) and scaled by 0.4 about the target's bottom middle; the block
/// holding that point, before its height is taken.
pub(crate) fn step_towards(target: BlockPos, position: Vec3) -> BlockPos {
    let bottom = [
        f64::from(target.x) + 0.5,
        f64::from(target.y),
        f64::from(target.z) + 0.5,
    ];
    let offset = physics::sub(position, bottom);
    // `Vec3.yRot(90.0f)`, of the table's sine and cosine.
    let cos = f64::from(clustine_noise::trig::cos(90.0));
    let sin = f64::from(clustine_noise::trig::sin(90.0));
    let turned = [
        offset[0] * cos + offset[2] * sin,
        offset[1],
        offset[2] * cos - offset[0] * sin,
    ];
    let aside = physics::add(physics::scale(turned, 0.4), bottom);
    let towards = physics::add(
        physics::scale(physics::normalize(physics::sub(aside, position)), 10.0),
        position,
    );
    physics::containing(towards)
}

/// `Level.getHeightmapPos(MOTION_BLOCKING_NO_LEAVES, pos)`: the column's height, or
/// `pos` as it is where the view does not read the chunk.
fn surface(ai: &Ai<'_, '_>, pos: BlockPos) -> BlockPos {
    let y = ai
        .tick
        .world
        .motion_blocking_no_leaves_height(pos.x, pos.z)
        .unwrap_or(pos.y);
    BlockPos::new(pos.x, y, pos.z)
}

/// `moveTo(x, y, z, speed)` to the block `pos`'s corner.
fn move_to_block(ai: &mut Ai<'_, '_>, pos: BlockPos, speed: f64) -> bool {
    let at = [f64::from(pos.x), f64::from(pos.y), f64::from(pos.z)];
    navigation::move_to(ai, at, speed)
}

/// `findPatrolCompanions()`: the patrolling monsters but the mob in its box inflated by
/// 16, in the view's order, by key.
fn companions(ai: &Ai<'_, '_>) -> Vec<EntityId> {
    let bounds = Aabb::of_entity(ai.position(), ai.width(), ai.height()).inflate([16.0; 3]);
    ai.tick
        .world
        .entities_in(&bounds)
        .into_iter()
        .filter(|seen| seen.entity.key != ai.tick.key && can_join_patrol(seen.entity.kind))
        .map(|seen| seen.entity.key)
        .collect()
}

impl LongDistancePatrolGoal {
    /// `moveRandomly()`: to the surface of a block up to eight off along x and z, two
    /// `nextInt(16)` of the mob's own random, at the companion's speed.
    fn move_randomly(&self, ai: &mut Ai<'_, '_>) -> bool {
        let at = ai.block_position();
        let x = -8 + ai.tick.draws.next_int(16) as i32;
        let z = -8 + ai.tick.draws.next_int(16) as i32;
        let pos = surface(ai, BlockPos::new(at.x + x, at.y, at.z + z));
        move_to_block(ai, pos, self.speed)
    }
}

impl Goal for LongDistancePatrolGoal {
    type State = Cooldown;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    /// Patrolling, with no target, not controlled, with a patrol target and not cooling
    /// down from a failed path.
    fn can_use(&self, state: &mut Cooldown, ai: &mut Ai<'_, '_>) -> bool {
        let cooling = state.until.is_some_and(|until| ai.now() < until);
        let raider = illagers::state_of(&ai.tick.mob);
        raider.patrolling
            && ai.tick.mob.body.combat.target.is_none()
            && !matches!(ai.tick.ridden, Ridden::Driven { .. })
            && raider.patrol_target.is_some()
            && !cooling
    }

    fn tick(&self, state: &mut Cooldown, ai: &mut Ai<'_, '_>) {
        let raider = illagers::state_of(&ai.tick.mob);
        let leader = raider.patrol_leader;
        if !navigation::is_done(ai) {
            return;
        }
        let companions = companions(ai);
        if raider.patrolling && companions.is_empty() {
            illagers::update(&mut ai.tick.mob, |own| own.patrolling = false);
            return;
        }
        let Some(target) = raider.patrol_target else {
            return;
        };
        let position = ai.position();
        // `BlockPos.closerToCenterThan(position, 10)`.
        let centre = [
            f64::from(target.x) + 0.5,
            f64::from(target.y) + 0.5,
            f64::from(target.z) + 0.5,
        ];
        let near = physics::distance_sqr(centre, position) < 100.0;
        if leader && near {
            let at = ai.block_position();
            let found = find_patrol_target(at, ai.tick.draws);
            illagers::update(&mut ai.tick.mob, |own| {
                own.patrol_target = Some(found);
                own.patrolling = true;
            });
            return;
        }
        let step = surface(ai, step_towards(target, position));
        let speed = if leader {
            self.leader_speed
        } else {
            self.speed
        };
        if !move_to_block(ai, step, speed) {
            self.move_randomly(ai);
            state.until = Some(ai.now() + NAVIGATION_FAILED_COOLDOWN);
        } else if leader {
            for key in companions {
                ai.tick
                    .out
                    .local
                    .push(Local::PatrolTarget { key, pos: step });
            }
        }
    }
}
