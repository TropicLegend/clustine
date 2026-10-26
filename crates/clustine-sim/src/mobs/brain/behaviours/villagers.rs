//! The villager's own behaviours as they run with no points of interest
//! (`docs/groundwork/n7k-plan.md`, section 10, "The villager"): its panic's trigger and
//! its calming down, its schedule's update, and the rest package's walks:
//! `InsideBrownianWalk` and `GoToClosestVillage`, with `SetClosestHomeAsWalkTarget`,
//! `SetWalkTargetFromBlockMemory(HOME)`, `ValidateNearbyPoi(HOME)` and `SleepInBed`,
//! which need a home and so never start.
//!
//! What the game reads of the points of interest is read as none there are: no place is
//! a village (`isVillage`), every section is as far from one as any other
//! (`sectionsToVillage`), and no villager has a home.
//!
//! An activity a behaviour changes (`setActiveActivityIfPossible`,
//! `updateActivityFromSchedule`) is asked of the brain through
//! [`super::super::Memories::requested`], which makes it active as the behaviour
//! returns.

use clustine_data::{Face, SupportType};
use clustine_world::BlockPos;

use super::super::sensors::find;
use super::super::{
    Activity, Behaviour, Cx, HeldShot, HeldTimed, MemoryKind, Shot, Status, Timed, Tracker,
};
use super::jumping::jdk_shuffle;
use super::{entity_of, walk, walk_to_point};
use crate::mobs::ai::random_pos;
use crate::physics::World;
use crate::physics::aabb::Aabb;
use crate::physics::collide::{Context, no_collision};

/// `BlockAndTintGetter.canSeeSky(pos)`: the sky's light is full there (**Inferred**, as
/// the fox's: the rule is not in the dumps; a block the region does not read sees it).
pub(crate) fn can_see_sky(cx: &Cx<'_, '_, '_>, pos: BlockPos) -> bool {
    cx.ai
        .tick
        .world
        .light(pos)
        .is_none_or(|light| light.sky >= 15)
}

/// `isHurt(villager)`: `hurt_by` holds something.
fn is_hurt(cx: &Cx<'_, '_, '_>) -> bool {
    cx.memories.check(MemoryKind::HurtBy, Status::Present)
}

/// `hasHostile(villager)`: `nearest_hostile` holds something.
fn has_hostile(cx: &Cx<'_, '_, '_>) -> bool {
    cx.memories
        .check(MemoryKind::NearestHostile, Status::Present)
}

/// `VillagerPanicTrigger` (a `Behavior` of no entry condition, its duration 60): while
/// hurt or near a hostile, the panic made active, and as it starts, where the panic is
/// not active already, its walk, look, path and the targets of breeding and interaction
/// forgotten. Its tick asks for a golem every hundredth game tick
/// (`spawnGolemIfNeeded`), which wants a villager that slept: never here.
pub(crate) struct VillagerPanicTrigger;

/// [`VillagerPanicTrigger`], boxed.
pub(crate) fn panic_trigger() -> Box<dyn Behaviour> {
    Box::new(HeldTimed::new(VillagerPanicTrigger))
}

impl Timed for VillagerPanicTrigger {
    type State = ();

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[]
    }

    fn can_still_use(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) -> bool {
        is_hurt(cx) || has_hostile(cx)
    }

    fn start(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        if !(is_hurt(cx) || has_hostile(cx)) {
            return;
        }
        if !cx.memories.active.contains(&Activity::Panic) {
            for kind in [
                MemoryKind::Path,
                MemoryKind::WalkTarget,
                MemoryKind::LookTarget,
                MemoryKind::BreedTarget,
                MemoryKind::InteractionTarget,
            ] {
                cx.memories.erase(kind);
            }
        }
        cx.memories.requested.push(Activity::Panic);
    }
}

/// `VillagerCalmDown.SAFE_DISTANCE_FROM_DANGER`, squared.
const SAFE_DISTANCE_SQR: f64 = 36.0;

/// `VillagerCalmDown.create()`: unless hurt, near a hostile, or within 6 of what hurt it
/// last, `hurt_by` and `hurt_by_entity` forgotten and the activity the schedule's
/// ([`VillagerCalmDown::update`]). It fires every time. What hurt it that the region's
/// view no longer has is taken as far off.
pub(crate) struct VillagerCalmDown {
    /// `brain.updateActivityFromSchedule(…)`, the type's.
    pub(crate) update: fn(&mut Cx<'_, '_, '_>),
}

/// [`VillagerCalmDown`], boxed.
pub(crate) fn calm_down(update: fn(&mut Cx<'_, '_, '_>)) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(VillagerCalmDown { update }))
}

impl Shot for VillagerCalmDown {
    fn required(&self) -> &[MemoryKind] {
        &[
            MemoryKind::HurtBy,
            MemoryKind::HurtByEntity,
            MemoryKind::NearestHostile,
        ]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        let position = cx.ai.position();
        let near_attacker = entity_of(cx, MemoryKind::HurtByEntity)
            .and_then(|key| find(cx.ai, key))
            .is_some_and(|looked| looked.distance_sqr(position) <= SAFE_DISTANCE_SQR);
        let panicking = is_hurt(cx) || has_hostile(cx) || near_attacker;
        if !panicking {
            cx.memories.erase(MemoryKind::HurtBy);
            cx.memories.erase(MemoryKind::HurtByEntity);
            (self.update)(cx);
        }
        true
    }
}

/// `UpdateActivityFromSchedule.create()`: `brain.updateActivityFromSchedule(…)` each
/// tick its activity is active (the type's [`Self::update`], which acts at most every 21
/// game ticks); it fires every time.
pub(crate) struct UpdateActivityFromSchedule {
    pub(crate) update: fn(&mut Cx<'_, '_, '_>),
}

/// [`UpdateActivityFromSchedule`], boxed.
pub(crate) fn from_schedule(update: fn(&mut Cx<'_, '_, '_>)) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(UpdateActivityFromSchedule { update }))
}

impl Shot for UpdateActivityFromSchedule {
    fn required(&self) -> &[MemoryKind] {
        &[]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        (self.update)(cx);
        true
    }
}

/// A behaviour of the rest package that needs a home (`HOME` present), which no villager
/// has without points of interest: `SetWalkTargetFromBlockMemory(HOME, …)`,
/// `ValidateNearbyPoi(HOME)`, `SleepInBed`, `SetClosestHomeAsWalkTarget` (whose search,
/// at most every 20 game ticks, finds no bed and so draws nothing). Each never starts and
/// draws nothing; its memories are registered as the game's.
pub(crate) struct WantsAHome {
    pub(crate) required: &'static [MemoryKind],
}

/// [`WantsAHome`], boxed.
pub(crate) fn wants_a_home(required: &'static [MemoryKind]) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(WantsAHome { required }))
}

impl Shot for WantsAHome {
    fn required(&self) -> &[MemoryKind] {
        self.required
    }

    fn trigger(&self, _: &mut Cx<'_, '_, '_>) -> bool {
        false
    }
}

/// `InsideBrownianWalk.create(speed)`: with no walk target and under a roof (the sky not
/// seen from its block), a walk to one of the 27 blocks about its own, shuffled
/// (`Collections.shuffle`, of the mob's stream: 26 draws), the first under a roof that it
/// can stand on while its own box is free (the game's `noCollision(mob)` asks where the
/// mob is, not the block); it fires under a roof whether one is found or not.
pub(crate) struct InsideBrownianWalk {
    pub(crate) speed: f32,
}

/// [`InsideBrownianWalk`], boxed.
pub(crate) fn inside_brownian_walk(speed: f32) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(InsideBrownianWalk { speed }))
}

/// `BlockPos.betweenClosed(from, to)`'s order: x the fastest, then y, then z.
pub(crate) fn between_closed(from: BlockPos, to: BlockPos) -> Vec<BlockPos> {
    let mut list = Vec::new();
    for z in from.z..=to.z {
        for y in from.y..=to.y {
            for x in from.x..=to.x {
                list.push(BlockPos::new(x, y, z));
            }
        }
    }
    list
}

impl Shot for InsideBrownianWalk {
    fn required(&self) -> &[MemoryKind] {
        &[MemoryKind::WalkTarget]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx.memories.check(MemoryKind::WalkTarget, Status::Absent) {
            return false;
        }
        let at = cx.ai.block_position();
        if can_see_sky(cx, at) {
            return false;
        }
        let mut around = between_closed(at.offset(-1, -1, -1), at.offset(1, 1, 1));
        jdk_shuffle(cx.ai.tick.draws, &mut around);
        let body = &cx.ai.tick.living.body;
        let bounds = Aabb::of_entity(body.position, body.width, body.height);
        let context = Context {
            bottom: body.position[1],
            fall_distance: body.fall_distance,
            falling_block: false,
            noclip: None,
        };
        let world = cx.ai.tick.world;
        let free = no_collision(world, context, &body.mover, &bounds);
        let found = around.into_iter().find(|pos| {
            !can_see_sky(cx, *pos)
                && world.is_loaded(*pos)
                // `entityCanStandOn`: the top of its collision shape is full (**Inferred**
                // from the face's sturdiness, as the navigation reads it).
                && world.state(*pos).props().is_face_sturdy(Face::Up, SupportType::Full)
                && free
        });
        if let Some(pos) = found {
            cx.memories.set(
                MemoryKind::WalkTarget,
                walk(Tracker::Block(pos), self.speed, 0),
            );
        }
        true
    }
}

/// `GoToClosestVillage.create(speed, closeEnoughDistance)` with no village anywhere:
/// with no walk target, five places of `LandRandomPos.getPos(mob, 15, 7)` (each as near
/// a village as where the mob stands, so the last found is taken), walked to; it fires
/// whether one is found or not. So a villager out at night strolls far.
pub(crate) struct GoToClosestVillage {
    pub(crate) speed: f32,
    pub(crate) close_enough: i32,
}

/// [`GoToClosestVillage`], boxed.
pub(crate) fn go_to_closest_village(speed: f32, close_enough: i32) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(GoToClosestVillage {
        speed,
        close_enough,
    }))
}

impl Shot for GoToClosestVillage {
    fn required(&self) -> &[MemoryKind] {
        &[MemoryKind::WalkTarget]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx.memories.check(MemoryKind::WalkTarget, Status::Absent) {
            return false;
        }
        let mut chosen = None;
        for _ in 0..5 {
            // The scorer, `-sectionsToVillage`, is the same everywhere: the mob's own
            // walk target value (0 for a villager) chooses as it does.
            if let Some(at) = random_pos::land_pos(cx.ai, 15, 7) {
                chosen = Some(at);
            }
        }
        if let Some(at) = chosen {
            cx.memories.set(
                MemoryKind::WalkTarget,
                walk_to_point(at, self.speed, self.close_enough),
            );
        }
        true
    }
}
