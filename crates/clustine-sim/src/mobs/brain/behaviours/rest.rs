//! A villager's walks and its sleep (`docs/groundwork/n10-brief.md`, section 1.5, cited
//! as V; `n10-plan.md`, section 5): `SetClosestHomeAsWalkTarget`, `InsideBrownianWalk`,
//! `VillageBoundRandomStroll`, `GoToClosestVillage`, `MoveToSkySeeingSpot`, `SleepInBed`
//! and `WakeUp`, with what they share: the path search to any of several points of
//! interest (`AcquirePoi.findPathToPois`), lying down in a bed and getting up from it
//! (`LivingEntity.startSleeping`, `stopSleeping`, V 3.4) and the place a sleeper stands up
//! at (`AbstractBedBlock.findStandUpPosition`).
//!
//! The village is the region's (`MobWorld::sections_to_village`): its land's, copies' and
//! the published margin's points of interest, or, until villagers claim, the stand-in of
//! the kept villages' pieces (`region/pois.rs`).
//!
//! **Named**: the JDK's global random that `InsideBrownianWalk` shuffles with is drawn
//! from the villager's stream (plan, named difference 7); a bed is lain in at the head
//! the region reads, and its `occupied` is written by the mob's block write, which the
//! holder of a copied chunk applies a tick later (named difference 17).

use clustine_blocks::data::{facing, flag, in_tag, is_class, set_flag};
use clustine_data::{BlockClass, BlockState, Face, SupportType};
use clustine_world::poi::PoiType;
use clustine_world::{BlockPos, Vec3};
use serde::{Deserialize, Serialize};

use super::super::{
    Activity, Behaviour, Cx, HeldShot, HeldTimed, MemoryKind, Shot, Status, Timed, Tracker, Value,
};
use super::jumping::jdk_shuffle;
use super::walking::HALF_PI;
use super::{HeldKept, KeptShot, walk, walk_to_point};
use crate::mobs::ai::{Ai, navigation, random_pos};
use crate::mobs::tick::{Local, MobTick};
use crate::mobs::types::villager;
use crate::physics::aabb::Aabb;
use crate::physics::collide::{Context, block_collisions, no_collision};
use crate::physics::{self, World};
use crate::region::pois::{Occupancy, section_of};

/// `BlockAndTintGetter.canSeeSky(pos)`: the sky's light is full there (**Inferred**, as
/// the fox's: the rule is not in the dumps; a block the region does not read sees it).
pub(crate) fn can_see_sky(cx: &Cx<'_, '_, '_>, pos: BlockPos) -> bool {
    cx.ai
        .tick
        .world
        .light(pos)
        .is_none_or(|light| light.sky >= 15)
}

/// `BlockPos.distSqr` between two blocks.
fn dist_sqr(a: BlockPos, b: BlockPos) -> f64 {
    crate::region::pois::dist_sqr(a, b)
}

/// `Vec3i.closerToCenterThan(position, distance)`: the block's centre nearer the point
/// than `distance`.
pub(crate) fn closer_to_centre_than(pos: BlockPos, point: [f64; 3], distance: f64) -> bool {
    let centre = [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5,
        f64::from(pos.z) + 0.5,
    ];
    physics::distance_sqr(centre, point) < distance * distance
}

/// The beds (`PoiTypes.HOME`) within 48 of `centre`, any of them taken or not, in a
/// query's order (`findAllWithType(HOME, …, 48, ANY)`).
fn beds_near(cx: &Cx<'_, '_, '_>, centre: BlockPos) -> Vec<BlockPos> {
    cx.ai
        .tick
        .world
        .pois_in_range(centre, 48, &|kind| kind == PoiType::Home, Occupancy::Any)
        .into_iter()
        .map(|record| record.pos)
        .collect()
}

/// `AcquirePoi.findPathToPois(mob, targets)`: one path search to whichever of the points
/// of interest is nearest to reach, as near as the widest of their types' valid ranges
/// asks (one block for a bed, six for the bell): the block it ends at and whether it
/// reaches it (`Path.getTarget`, `canReach`), none where no path is made.
pub(crate) fn find_path_to_pois(
    ai: &mut Ai<'_, '_>,
    targets: &[(PoiType, BlockPos)],
) -> Option<(BlockPos, bool)> {
    if targets.is_empty() {
        return None;
    }
    let reach = targets
        .iter()
        .map(|(kind, _)| i32::from(kind.valid_range()))
        .fold(1, i32::max);
    let positions: Vec<BlockPos> = targets.iter().map(|(_, pos)| *pos).collect();
    let made = navigation::create_path_to_any(ai, &positions, reach)?;
    let path = made.path(ai)?;
    Some((path.target, path.reached))
}

/// A cache of the beds a search tried (`Long2LongMap batchCache`): each with the game
/// time it may be tried again after, kept in the order first put.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct BedCache {
    pub(crate) tried: Vec<(BlockPos, i64)>,
}

impl BedCache {
    fn contains(&self, pos: BlockPos) -> bool {
        self.tried.iter().any(|(at, _)| *at == pos)
    }

    fn put(&mut self, pos: BlockPos, until: i64) {
        match self.tried.iter_mut().find(|(at, _)| *at == pos) {
            Some(entry) => entry.1 = until,
            None => self.tried.push((pos, until)),
        }
    }

    /// `removeIf(entry -> entry.getLongValue() < lastUpdate)`.
    fn forget_before(&mut self, time: i64) {
        self.tried.retain(|(_, until)| *until >= time);
    }
}

/// The search shared by `SetClosestHomeAsWalkTarget` and the baby's `NearestBedSensor`
/// once it is due: `lastUpdate` drawn (the level's `nextInt(20)` on the villager's
/// stream), then up to four beds within 48 the cache has not seen, each put in it for 40
/// ticks past `lastUpdate`, and one path search to any of them. The bed it reaches, if
/// it is still a point of interest; where nothing is reached and fewer than five were
/// counted, what the cache holds from before `lastUpdate` is forgotten.
pub(crate) fn search_beds(
    cx: &mut Cx<'_, '_, '_>,
    cache: &mut BedCache,
    last_update: &mut i64,
) -> Option<BlockPos> {
    *last_update = cx.time + i64::from(cx.ai.tick.draws.next_int(20));
    let at = cx.ai.block_position();
    let mut counted = 0;
    let mut targets = Vec::new();
    for bed in beds_near(cx, at) {
        // The position predicate of `findAllWithType`, run on every record in order:
        // a bed of the cache is passed over, and from the fifth counted on none is kept.
        if cache.contains(bed) {
            continue;
        }
        counted += 1;
        if counted >= 5 {
            continue;
        }
        cache.put(bed, *last_update + 40);
        targets.push((PoiType::Home, bed));
    }
    match find_path_to_pois(cx.ai, &targets) {
        Some((target, true)) => cx.ai.tick.world.poi_type_at(target).map(|_| target),
        _ => {
            if counted < 5 {
                cache.forget_before(*last_update);
            }
            None
        }
    }
}

// ------------------------------------------------------------------------------------
// The rest package's walks.

/// What `SetClosestHomeAsWalkTarget` keeps in its closure: `lastUpdate` and the beds it
/// tried.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ClosestHome {
    last_update: i64,
    cache: BedCache,
}

/// `SetClosestHomeAsWalkTarget.create(speed)` (rest 5, in the `RunOne` of the homeless;
/// V 1.5): with no walk target and no home, at least 20 game ticks after its last
/// search, where the nearest bed within 48 is farther than two blocks, a search
/// ([`search_beds`]) and a walk to the bed it reaches, close enough at 1.
pub(crate) struct SetClosestHomeAsWalkTarget {
    pub(crate) speed: f32,
}

/// [`SetClosestHomeAsWalkTarget`], boxed.
pub(crate) fn set_closest_home_as_walk_target(speed: f32) -> Box<dyn Behaviour> {
    Box::new(HeldKept::new(SetClosestHomeAsWalkTarget { speed }))
}

impl KeptShot for SetClosestHomeAsWalkTarget {
    type State = ClosestHome;

    fn required(&self) -> &[MemoryKind] {
        &[MemoryKind::WalkTarget, MemoryKind::Home]
    }

    fn trigger(&self, state: &mut ClosestHome, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx.memories.check(MemoryKind::WalkTarget, Status::Absent)
            || !cx.memories.check(MemoryKind::Home, Status::Absent)
        {
            return false;
        }
        if cx.time - state.last_update < 20 {
            return false;
        }
        let at = cx.ai.block_position();
        // `findClosest(HOME, pos, 48, ANY)`: the least distance, the first of equals.
        let closest = beds_near(cx, at)
            .into_iter()
            .map(|bed| (dist_sqr(bed, at), bed))
            .reduce(|best, next| if next.0 < best.0 { next } else { best });
        match closest {
            Some((distance, _)) if distance > 4.0 => {}
            _ => return false,
        }
        if let Some(bed) = search_beds(cx, &mut state.cache, &mut state.last_update) {
            cx.memories.set(
                MemoryKind::WalkTarget,
                walk(Tracker::Block(bed), self.speed, 1),
            );
        }
        true
    }
}

/// `InsideBrownianWalk.create(speed)` (D10 C.9): with no walk target and under a roof
/// (the sky not seen from its block), a walk to one of the 27 blocks about its own,
/// shuffled (`Collections.shuffle`, of the mob's stream: 26 draws), the first under a
/// roof that it can stand on while its own box is free (the game's `noCollision(mob)`
/// asks where the mob is, not the block); it fires under a roof whether one is found
/// or not, and in the open draws nothing.
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
            stands_on_lava: false,
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

/// `sectionsToVillage` of the section a block is in.
fn sections_to_village(cx: &Cx<'_, '_, '_>, pos: BlockPos) -> i32 {
    cx.ai.tick.world.sections_to_village(section_of(pos))
}

/// `BehaviorUtils.findSectionClosestToVillage(level, section, radius)`: of the cube of
/// sections about `section` (x the fastest, then y, then z: `SectionPos.cube`, D10 A.5),
/// those nearer a village than it, the nearest, the first of equals; itself where none.
pub(crate) fn section_closest_to_village(
    cx: &Cx<'_, '_, '_>,
    section: [i32; 3],
    radius: i32,
) -> [i32; 3] {
    let world = cx.ai.tick.world;
    let own = world.sections_to_village(section);
    let mut best: Option<([i32; 3], i32)> = None;
    for z in section[2] - radius..=section[2] + radius {
        for y in section[1] - radius..=section[1] + radius {
            for x in section[0] - radius..=section[0] + radius {
                let level = world.sections_to_village([x, y, z]);
                if level < own && best.is_none_or(|(_, held)| level < held) {
                    best = Some(([x, y, z], level));
                }
            }
        }
    }
    best.map_or(section, |(found, _)| found)
}

/// `VillageBoundRandomStroll.create(speed, h, v)` (V 1.5): with no walk target, in a
/// village a stroll to land within `h` and `v` (`LandRandomPos.getPos`); outside one,
/// towards the middle of the nearest section nearer a village within two sections
/// (`DefaultRandomPos.getPosTowards`, within a right angle), or to land where none is;
/// the walk target set, or forgotten where no place is found; it fires every time.
pub(crate) struct VillageBoundRandomStroll {
    pub(crate) speed: f32,
    pub(crate) horizontal: i32,
    pub(crate) vertical: i32,
}

/// `VillageBoundRandomStroll.create(speed)`: within 10 and 7.
pub(crate) fn village_bound_random_stroll(speed: f32) -> Box<dyn Behaviour> {
    village_bound_random_stroll_within(speed, 10, 7)
}

/// `VillageBoundRandomStroll.create(speed, h, v)`.
pub(crate) fn village_bound_random_stroll_within(
    speed: f32,
    horizontal: i32,
    vertical: i32,
) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(VillageBoundRandomStroll {
        speed,
        horizontal,
        vertical,
    }))
}

impl Shot for VillageBoundRandomStroll {
    fn required(&self) -> &[MemoryKind] {
        &[MemoryKind::WalkTarget]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx.memories.check(MemoryKind::WalkTarget, Status::Absent) {
            return false;
        }
        let at = cx.ai.block_position();
        let (h, v) = (self.horizontal, self.vertical);
        let place = if cx.ai.tick.world.is_village(at) {
            random_pos::land_pos(cx.ai, h, v)
        } else {
            let own = section_of(at);
            let closest = section_closest_to_village(cx, own, 2);
            if closest == own {
                random_pos::land_pos(cx.ai, h, v)
            } else {
                // `SectionPos.center()` at its bottom middle (`Vec3.atBottomCenterOf`).
                let middle = [
                    f64::from(closest[0] * 16 + 8) + 0.5,
                    f64::from(closest[1] * 16 + 8),
                    f64::from(closest[2] * 16 + 8) + 0.5,
                ];
                random_pos::default_pos_towards(cx.ai, h, v, middle, HALF_PI)
            }
        };
        match place {
            Some(point) => cx
                .memories
                .set(MemoryKind::WalkTarget, walk_to_point(point, self.speed, 0)),
            None => cx.memories.erase(MemoryKind::WalkTarget),
        }
        true
    }
}

/// `GoToClosestVillage.create(speed, closeEnough)` (V 1.5): outside a village, with no
/// walk target, up to five places of `LandRandomPos.getPos(mob, 15, 7)` each liked the
/// nearer a village it is, taking at once one nearer a village than where it stands, else
/// the last as near; walked to where one is found; it fires outside a village whether
/// one is found or not, and never in one.
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
        let at = cx.ai.block_position();
        if cx.ai.tick.world.is_village(at) {
            return false;
        }
        let own = sections_to_village(cx, at);
        let mut chosen = None;
        for _ in 0..5 {
            let found = random_pos::land_pos_scored(cx.ai, 15, 7, |ai, pos| {
                -f64::from(ai.tick.world.sections_to_village(section_of(pos)))
            });
            let Some(point) = found else {
                continue;
            };
            let level = sections_to_village(cx, physics::containing(point));
            if level < own {
                chosen = Some(point);
                break;
            }
            if level == own {
                chosen = Some(point);
            }
        }
        if let Some(point) = chosen {
            cx.memories.set(
                MemoryKind::WalkTarget,
                walk_to_point(point, self.speed, self.close_enough),
            );
        }
        true
    }
}

/// `MoveToSkySeeingSpot.create(speed)` (raid 0, after a victory; V 1.5): with no walk
/// target and the sky not seen from its block, up to ten blocks about it (its own
/// `nextInt(20) − 10`, `nextInt(6) − 3`, `nextInt(20) − 10` each), the first that sees
/// the sky with the top of the motion-blocking heightmap at or below its feet, walked
/// to; it fires under a roof whether one is found or not.
pub(crate) struct MoveToSkySeeingSpot {
    pub(crate) speed: f32,
}

/// [`MoveToSkySeeingSpot`], boxed.
pub(crate) fn move_to_sky_seeing_spot(speed: f32) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(MoveToSkySeeingSpot { speed }))
}

/// `MoveToSkySeeingSpot.hasNoBlocksAbove(level, mob, pos)`: the sky seen there, and the
/// heightmap's top there at or below the mob's feet (none where the column is not read).
pub(crate) fn has_no_blocks_above(cx: &Cx<'_, '_, '_>, pos: BlockPos) -> bool {
    can_see_sky(cx, pos)
        && cx
            .ai
            .tick
            .world
            .motion_blocking_height(pos.x, pos.z)
            .is_some_and(|top| f64::from(top) <= cx.ai.position()[1])
}

impl Shot for MoveToSkySeeingSpot {
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
        for _ in 0..10 {
            let draws = &mut *cx.ai.tick.draws;
            let dx = draws.next_int(20) as i32 - 10;
            let dy = draws.next_int(6) as i32 - 3;
            let dz = draws.next_int(20) as i32 - 10;
            let pos = at.offset(dx, dy, dz);
            if has_no_blocks_above(cx, pos) {
                let point = [
                    f64::from(pos.x) + 0.5,
                    f64::from(pos.y),
                    f64::from(pos.z) + 0.5,
                ];
                cx.memories
                    .set(MemoryKind::WalkTarget, walk_to_point(point, self.speed, 0));
                break;
            }
        }
        true
    }
}

// ------------------------------------------------------------------------------------
// Sleep.

/// The home a villager remembers (`HOME`), as a block.
fn home(cx: &Cx<'_, '_, '_>) -> Option<BlockPos> {
    match cx.memories.get(MemoryKind::Home)? {
        Value::Block(pos) => Some(pos),
        _ => None,
    }
}

/// `AbstractBedBlock.getSleepHeight`: half a block and a sixteenth above a bed's block,
/// a quarter above a straw bed's.
fn sleep_height(state: BlockState) -> f64 {
    if is_class(state, BlockClass::StrawBedBlock) {
        0.25
    } else {
        0.5625
    }
}

/// `LivingEntity.startSleeping(pos)` of a villager in its tick (V 3.4): where the block
/// at `pos` is a bed, it lies on it (its feet at the bed's middle, the bed's height and
/// an eighth up), still, the bed's `occupied` set by its block write, asleep there
/// (its pose and its sleeping position). False where the block is no bed.
pub(crate) fn start_sleeping(tick: &mut MobTick<'_>, pos: BlockPos) -> bool {
    let state = tick.world.state(pos);
    if !is_class(state, BlockClass::AbstractBedBlock) {
        return false;
    }
    let body = &mut tick.living.body;
    body.position = [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + sleep_height(state) + 0.125,
        f64::from(pos.z) + 0.5,
    ];
    body.motion = [0.0; 3];
    tick.out.local.push(Local::SetBlock {
        pos,
        state: set_flag(state, "occupied", true),
        flags: 3,
    });
    villager::update(&mut tick.mob, |own| own.sleeping = Some(pos));
    tick.out.needs_sync = true;
    true
}

/// The way one step towards a horizontal face goes.
fn step_of(direction: Face) -> (i32, i32) {
    match direction {
        Face::North => (0, -1),
        Face::South => (0, 1),
        Face::West => (-1, 0),
        _ => (1, 0),
    }
}

/// `Direction.getClockWise` of a horizontal step.
fn clockwise((dx, dz): (i32, i32)) -> (i32, i32) {
    (-dz, dx)
}

/// `Direction.isFacingAngle(yRot)`, with the game's sine table.
fn is_facing_angle((dx, dz): (i32, i32), yaw: f32) -> bool {
    let radians = yaw * 0.017_453_292;
    let x = -crate::mobs::ai::maths::table_sin(radians);
    let z = crate::mobs::ai::maths::table_cos(radians);
    dx as f32 * x + dz as f32 * z > 0.0
}

/// `EntityType.isBlockDangerous` of a type immune to nothing: what burns and what hurts
/// by touch (**Inferred**, as a player's in `region/sleep.rs`).
fn is_dangerous(state: BlockState) -> bool {
    use clustine_blocks::data::is_block;
    use clustine_data::blocks;
    in_tag(state, "minecraft:fire")
        || is_block(state, blocks::LAVA)
        || is_block(state, blocks::MAGMA_BLOCK)
        || is_block(state, blocks::LAVA_CAULDRON)
        || (in_tag(state, "minecraft:campfires") && flag(state, "lit"))
        || is_block(state, blocks::WITHER_ROSE)
        || is_block(state, blocks::SWEET_BERRY_BUSH)
        || is_block(state, blocks::CACTUS)
        || is_block(state, blocks::POWDER_SNOW)
}

/// `AbstractBedBlock.findStandUpPosition(type, level, head, facing, yRot)` for an
/// entity `width` by `height` (a player's, `region/sleep.rs`, and a villager's): beside
/// the bed on the side away from where it looks, then the rest of the ring about it,
/// then on it, the first that is safe, and then again without the danger checks; a bunk
/// bed's below too.
pub(crate) fn stand_up_position<W: World + ?Sized>(
    world: &W,
    head: BlockPos,
    facing: (i32, i32),
    yaw: f32,
    width: f32,
    height: f32,
) -> Option<Vec3> {
    let mut side = clockwise(facing);
    if is_facing_angle(side, yaw) {
        side = (-side.0, -side.1);
    }
    let (d, s) = (facing, side);
    let ring = [
        (s.0, s.1),
        (s.0 - d.0, s.1 - d.1),
        (s.0 - 2 * d.0, s.1 - 2 * d.1),
        (-2 * d.0, -2 * d.1),
        (-s.0 - 2 * d.0, -s.1 - 2 * d.1),
        (-s.0 - d.0, -s.1 - d.1),
        (-s.0, -s.1),
        (-s.0 + d.0, -s.1 + d.1),
        (d.0, d.1),
        (s.0 + d.0, s.1 + d.1),
    ];
    let on = [(0, 0), (-d.0, -d.1)];
    let below = BlockPos::new(head.x, head.y - 1, head.z);
    let bunk = world
        .block(below)
        .is_some_and(|state| is_class(state, BlockClass::AbstractBedBlock));
    let at = |(dx, dz): (i32, i32), dy: i32| BlockPos::new(head.x + dx, head.y + dy, head.z + dz);
    let mut order: Vec<(BlockPos, bool)> = Vec::new();
    if bunk {
        for dangerous in [true, false] {
            order.extend(ring.iter().map(|o| (at(*o, 0), dangerous)));
            order.extend(ring.iter().map(|o| (at(*o, -1), dangerous)));
            if dangerous {
                order.extend(on.iter().map(|o| (at(*o, 0), dangerous)));
            }
        }
        order.extend(on.iter().map(|o| (at(*o, 0), false)));
    } else {
        for dangerous in [true, false] {
            order.extend(ring.iter().chain(&on).map(|o| (at(*o, 0), dangerous)));
        }
    }
    order
        .into_iter()
        .find_map(|(pos, checked)| safe_dismount(world, pos, checked, width, height))
}

/// `DismountHelper.findSafeDismountLocation` of an entity `width` by `height` standing at
/// `pos`: a block the world cannot read is unsafe.
fn safe_dismount<W: World + ?Sized>(
    world: &W,
    pos: BlockPos,
    checked: bool,
    width: f32,
    height: f32,
) -> Option<Vec3> {
    use crate::region::riding::{block_floor_height, is_floor_valid, non_climbable};
    let here = world.block(pos)?;
    let below = world.block(BlockPos::new(pos.x, pos.y - 1, pos.z))?;
    if checked && is_dangerous(here) {
        return None;
    }
    let floor = block_floor_height(non_climbable(here), || non_climbable(below));
    if !is_floor_valid(floor) {
        return None;
    }
    if checked && floor <= 0.0 && is_dangerous(below) {
        return None;
    }
    let at = Vec3::new(
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + floor,
        f64::from(pos.z) + 0.5,
    );
    let bounds = Aabb::of_entity([at.x, at.y, at.z], width, height);
    let context = Context {
        stands_on_lava: false,
        bottom: at.y,
        fall_distance: 0.0,
        falling_block: false,
        noclip: None,
    };
    if !block_collisions(world, context, &bounds).is_empty() {
        return None;
    }
    let above = BlockPos::new(pos.x, pos.y + 1, pos.z);
    let invalid = |p: BlockPos| {
        world
            .block(p)
            .is_none_or(|state| in_tag(state, "minecraft:invalid_spawn_inside"))
    };
    if invalid(pos) || invalid(above) {
        return None;
    }
    Some(at)
}

/// The yaw that faces from `from` to the bed whose head is `head`, as a sleeper turns
/// when it gets up (`atan2(z, x)` in degrees, less 90, wrapped).
pub(crate) fn yaw_to_bed(head: BlockPos, from: Vec3) -> f32 {
    let bed = Vec3::new(
        f64::from(head.x) + 0.5,
        f64::from(head.y),
        f64::from(head.z) + 0.5,
    );
    let (dx, dy, dz) = (bed.x - from.x, bed.y - from.y, bed.z - from.z);
    let length = (dx * dx + dy * dy + dz * dz).sqrt();
    let (vx, vz) = if length < 1.0e-4 {
        (0.0, 0.0)
    } else {
        (dx / length, dz / length)
    };
    crate::mobs::ai::maths::wrap_degrees(
        (crate::mobs::ai::maths::atan2(vz, vx) * 57.295_776_367_187_5 - 90.0) as f32,
    )
}

/// The width and height a villager stands up in (`EntityType.VILLAGER`'s dimensions).
const VILLAGER_SIZE: (f32, f32) = (0.6, 1.95);

/// `Villager.stopSleeping()` in its tick (V 3.4): where it sleeps and its bed is read, the
/// bed's `occupied` cleared by its block write, and up beside it facing it (above the
/// bed where no place is safe); standing, its sleeping position cleared; and
/// `last_woken` the game time, written by the caller into its memories. False where it
/// did not sleep.
pub(crate) fn stop_sleeping(tick: &mut MobTick<'_>) -> bool {
    let Some(head) = villager::state_of(&tick.mob).1.sleeping else {
        return false;
    };
    if tick.world.is_loaded(head) {
        let state = tick.world.state(head);
        if is_class(state, BlockClass::AbstractBedBlock) {
            tick.out.local.push(Local::SetBlock {
                pos: head,
                state: set_flag(state, "occupied", false),
                flags: 3,
            });
            let toward = facing(state).map_or((0, 1), step_of);
            let (width, height) = VILLAGER_SIZE;
            let up = stand_up_position(tick.world, head, toward, tick.yaw, width, height)
                .unwrap_or(Vec3::new(
                    f64::from(head.x) + 0.5,
                    f64::from(head.y) + 1.1,
                    f64::from(head.z) + 0.5,
                ));
            tick.yaw = yaw_to_bed(head, up);
            tick.pitch = 0.0;
            tick.living.body.position = [up.x, up.y, up.z];
        }
    }
    villager::update(&mut tick.mob, |own| own.sleeping = None);
    tick.out.needs_sync = true;
    true
}

/// [`stop_sleeping`] from a behaviour, with `Villager.stopSleeping`'s `last_woken`.
fn wake(cx: &mut Cx<'_, '_, '_>) {
    if stop_sleeping(cx.ai.tick) {
        cx.memories.set(MemoryKind::LastWoken, Value::Long(cx.time));
    }
}

/// Whether the villager sleeps (`isSleeping`: it has a sleeping position).
pub(crate) fn is_sleeping(tick: &MobTick<'_>) -> bool {
    villager::state_of(&tick.mob).1.sleeping.is_some()
}

/// `SleepInBed` (rest 3; a `Behavior` of 60 to 60 ticks, never timing out; V 1.5): not
/// riding, more than 100 ticks after it was woken (or woken in the future), within two
/// blocks of its home's middle, the home a bed villagers sleep on and free. It lies down
/// as it starts (unless it was woken less than 40 ticks before by this behaviour's own
/// stop), closing the doors it opened, `last_slept` the time, its walk forgotten; it
/// keeps lying while it rests above the bed's half and within 1.14 of its middle, and
/// gets up as it stops.
pub(crate) struct SleepInBed;

/// [`SleepInBed`], boxed.
pub(crate) fn sleep_in_bed() -> Box<dyn Behaviour> {
    Box::new(HeldTimed::new(SleepInBed))
}

/// What `SleepInBed` keeps: `nextOkStartTime`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct NextOk {
    next_ok: i64,
}

impl Timed for SleepInBed {
    type State = NextOk;

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[
            (MemoryKind::Home, Status::Present),
            (MemoryKind::LastWoken, Status::Registered),
            (MemoryKind::LastSlept, Status::Registered),
            (MemoryKind::WalkTarget, Status::Registered),
            (MemoryKind::CantReachWalkTargetSince, Status::Registered),
        ]
    }

    fn extra_start(&self, _: &mut NextOk, cx: &mut Cx<'_, '_, '_>) -> bool {
        if cx.ai.tick.vehicle.is_some() {
            return false;
        }
        let Some(home) = home(cx) else {
            return false;
        };
        if let Some(Value::Long(woken)) = cx.memories.get(MemoryKind::LastWoken) {
            let since = cx.time - woken;
            if since > 0 && since < 100 {
                return false;
            }
        }
        let state = cx.ai.tick.world.state(home);
        closer_to_centre_than(home, cx.ai.position(), 2.0)
            && in_tag(state, "minecraft:villagers_can_sleep_on_bed")
            && !flag(state, "occupied")
    }

    fn can_still_use(&self, _: &mut NextOk, cx: &mut Cx<'_, '_, '_>) -> bool {
        let Some(home) = home(cx) else {
            return false;
        };
        let position = cx.ai.position();
        cx.memories.active.contains(&Activity::Rest)
            && position[1] > f64::from(home.y) + 0.4
            && closer_to_centre_than(home, position, 1.14)
    }

    fn timed_out(&self, _: &NextOk, _: &Cx<'_, '_, '_>, _: i64) -> bool {
        false
    }

    fn start(&self, state: &mut NextOk, cx: &mut Cx<'_, '_, '_>) {
        if cx.time <= state.next_ok {
            return;
        }
        if cx.memories.check(MemoryKind::DoorsToClose, Status::Present) {
            super::doors::close_doors_passed(cx);
        }
        if let Some(home) = home(cx)
            && start_sleeping(cx.ai.tick, home)
        {
            cx.memories.set(MemoryKind::LastSlept, Value::Long(cx.time));
        }
        cx.memories.erase(MemoryKind::WalkTarget);
        cx.memories.erase(MemoryKind::CantReachWalkTargetSince);
    }

    fn stop(&self, state: &mut NextOk, cx: &mut Cx<'_, '_, '_>) {
        if is_sleeping(cx.ai.tick) {
            wake(cx);
            state.next_ok = cx.time + 40;
        }
    }
}

/// `WakeUp.create()` (core 0; V 1.5): asleep while it does not rest, it gets up.
pub(crate) struct WakeUp;

/// [`WakeUp`], boxed.
pub(crate) fn wake_up() -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(WakeUp))
}

impl Shot for WakeUp {
    fn required(&self) -> &[MemoryKind] {
        &[]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if cx.memories.active.contains(&Activity::Rest) || !is_sleeping(cx.ai.tick) {
            return false;
        }
        wake(cx);
        true
    }
}
