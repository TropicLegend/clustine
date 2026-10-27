//! `PathNavigation` and its kinds (the brief's 3.2 to 3.8): making paths, following
//! them, the stuck check and the timeout, and recomputing a path when a block near it
//! changes. The path is the body's [`super::Path`], whole as it was made, and the
//! progress along it with the navigation's timers the body's [`super::Progress`].
//!
//! The navigation's own count of its ticks is the mob's AI steps, which are its ticks
//! while it has AI (its `lived`); the game time it measures by is the mob's `lived` too
//! (the module's comment of [`super`]).

use clustine_data::{PathType, attributes};
use clustine_world::BlockPos;

use super::Ai;
use super::controls;
use super::pathfinder::{
    self, Evaluator, EvaluatorFlags, FoundPath, PathNode, Searcher, find_path,
};
use crate::physics::{self, Vec3, World};

/// Which navigation a type makes (`createNavigation`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Kind {
    /// `GroundPathNavigation`: `Mob`'s own.
    #[default]
    Ground,
    /// `AmphibiousPathNavigation`.
    Amphibious,
    /// `WaterBoundPathNavigation`, whose evaluator breaches for a dolphin.
    WaterBound { breach: bool },
    /// `FlyingPathNavigation`.
    Flying,
}

/// The navigation as a type and its goals set it up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Settings {
    pub(crate) kind: Kind,
    pub(crate) flags: EvaluatorFlags,
    /// `requiredPathLength`, and whether anything set it (which makes the node limit
    /// follow the modified range, as `updatePathfinderMaxVisitedNodes` does).
    pub(crate) required_path_length: f32,
    pub(crate) required_set: bool,
    /// `GroundPathNavigation.avoidSun` as the constructor sets it; a goal sets it in the
    /// type's state instead ([`crate::mobs::types::MobType::avoids_sun`], a skeleton's
    /// `RestrictSunGoal`).
    pub(crate) avoid_sun: bool,
    /// `canPathToTargetsBelowSurface`.
    pub(crate) below_surface: bool,
    /// `WallClimberNavigation` (a spider's): a ground navigation that walks straight at
    /// where it was last sent (`pathToPosition`, kept in the spiders' type state) once
    /// its path is done.
    pub(crate) wall_climber: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            kind: Kind::Ground,
            flags: EvaluatorFlags::default(),
            required_path_length: 16.0,
            required_set: false,
            avoid_sun: false,
            below_surface: false,
            wall_climber: false,
        }
    }
}

impl Settings {
    /// A navigation of `kind`, with what its constructor sets (`WaterBoundPathNavigation`
    /// passes no doors).
    pub(crate) fn of(kind: Kind) -> Self {
        let mut settings = Self {
            kind,
            ..Self::default()
        };
        if let Kind::WaterBound { .. } = kind {
            settings.flags.can_pass_doors = false;
        }
        settings
    }

    /// `setCanFloat`, which the amphibious and water navigations ignore.
    pub(crate) fn set_can_float(&mut self, can_float: bool) {
        if matches!(self.kind, Kind::Ground | Kind::Flying) {
            self.flags.can_float = can_float;
        }
    }

    /// `setRequiredPathLength`.
    pub(crate) fn set_required_path_length(&mut self, length: f32) {
        self.required_path_length = length;
        self.required_set = true;
    }

    fn evaluator(&self) -> Evaluator {
        match self.kind {
            Kind::Ground => Evaluator::Walk,
            Kind::Amphibious => Evaluator::Amphibious { shallow: false },
            Kind::WaterBound { breach } => Evaluator::Swim { breach },
            Kind::Flying => Evaluator::Fly,
        }
    }
}

/// `Mob.getMaxFallDistance()`: three blocks without a target; with one, as far as its
/// health allows at the world's difficulty.
fn max_fall_distance(ai: &Ai<'_, '_>) -> i32 {
    if super::sensing::target(ai).is_none() {
        return 3;
    }
    const DIFFICULTY: i32 = clustine_world::Difficulty::WORLD as i32;
    let health = ai.tick.mob.body.vitals.health;
    let max = ai.attribute(attributes::MAX_HEALTH) as f32;
    let spare = ((health - max * 0.33) as i32 - (3 - DIFFICULTY) * 4).max(0);
    super::maths::floor_f32(spare as f32 + 3.0)
}

/// The mob as its navigation's search reads it.
pub(crate) fn searcher(ai: &Ai<'_, '_>) -> Searcher {
    let settings = &ai.config.navigation;
    Searcher {
        position: ai.position(),
        width: ai.width(),
        height: ai.height(),
        on_ground: ai.on_ground(),
        in_floatable: ai.in_water(),
        in_water: ai.in_water(),
        fall_distance: ai.tick.living.body.fall_distance,
        step_height: controls::max_up_step(ai),
        max_fall: max_fall_distance(ai),
        malus: ai.maluses(),
        flags: settings.flags,
        evaluator: settings.evaluator(),
        min_y: ai.tick.world.min_y(),
        sea_level: SEA_LEVEL,
    }
}

/// The overworld's sea level, which the amphibious evaluator's shallows are below.
const SEA_LEVEL: i32 = 63;

/// `getMaxPathLength()`: the follow range, at least the required length.
pub(crate) fn max_path_length(ai: &Ai<'_, '_>) -> f32 {
    (ai.attribute(attributes::FOLLOW_RANGE) as f32).max(ai.config.navigation.required_path_length)
}

/// The search's node limit (`maxVisitedNodes`): from the follow range's base at
/// construction, and from the range as modified once it or the tempt range changes or
/// a required length is set (`updatePathfinderMaxVisitedNodes`).
fn max_visited_nodes(ai: &Ai<'_, '_>) -> i32 {
    let changed = [attributes::FOLLOW_RANGE, attributes::TEMPT_RANGE]
        .into_iter()
        .any(|attribute| {
            ai.tick
                .mob
                .body
                .attributes
                .instances
                .iter()
                .any(|instance| {
                    i32::from(instance.attribute) == attribute && !instance.modifiers.is_empty()
                })
        });
    if ai.config.navigation.required_set || changed {
        super::maths::floor_f32(max_path_length(ai) * 16.0)
    } else {
        physics::floor(super::attribute_base(ai.tick, attributes::FOLLOW_RANGE) * 16.0)
    }
}

/// The path followed, and the index of its next node.
fn path<'a>(ai: &'a Ai<'_, '_>) -> Option<&'a FoundPath> {
    ai.tick.mob.body.path.followed.as_ref()
}

fn next_index(ai: &Ai<'_, '_>) -> usize {
    ai.tick.mob.body.progress.next as usize
}

/// `isDone()`: no path, or its index at its end.
fn is_done_own(ai: &Ai<'_, '_>) -> bool {
    path(ai).is_none_or(|path| next_index(ai) >= path.nodes.len())
}

/// `isInProgress()`.
fn in_progress_own(ai: &Ai<'_, '_>) -> bool {
    !is_done_own(ai)
}

/// `stop_own()`: the path dropped, nothing else.
fn stop_own(ai: &mut Ai<'_, '_>) {
    ai.tick.mob.body.path.followed = None;
}

/// `setSpeedModifier(speed)`.
fn set_speed_modifier_own(ai: &mut Ai<'_, '_>, speed: f64) {
    ai.tick.mob.body.progress.speed = speed;
}

/// `getTargetPos()`.
fn target_pos_own(ai: &Ai<'_, '_>) -> Option<BlockPos> {
    ai.tick.mob.body.progress.target
}

/// `canFloat()`.
fn can_float_own(ai: &Ai<'_, '_>) -> bool {
    ai.config.navigation.flags.can_float
}

/// `canNavigateGround()`.
fn can_navigate_ground_own(ai: &Ai<'_, '_>) -> bool {
    matches!(ai.config.navigation.kind, Kind::Ground | Kind::Amphibious)
}

/// The navigation's tick count now: its ticks so far, this step's once it has ticked.
fn navigation_tick(ai: &Ai<'_, '_>) -> u32 {
    if ai.navigation_ticked {
        ai.tick.lived.saturating_add(1)
    } else {
        ai.tick.lived
    }
}

/// `canUpdatePath()`. **Named**: no mob of N7's rides anything the region tells its
/// tick of, so none counts as a passenger.
fn can_update_path(ai: &Ai<'_, '_>) -> bool {
    match ai.config.navigation.kind {
        Kind::Ground => ai.on_ground() || ai.in_liquid(),
        Kind::Amphibious => true,
        Kind::WaterBound { breach } => breach || ai.in_liquid(),
        Kind::Flying => true,
    }
}

/// `getTempMobPos()`.
fn temp_mob_pos(ai: &Ai<'_, '_>) -> Vec3 {
    let [x, y, z] = ai.position();
    match ai.config.navigation.kind {
        Kind::Ground => [x, f64::from(surface_y(ai)), z],
        Kind::Amphibious | Kind::WaterBound { .. } => [x, y + f64::from(ai.height()) * 0.5, z],
        Kind::Flying => [x, y, z],
    }
}

/// `GroundPathNavigation.getSurfaceY()`: a floating mob's first cell above the water it
/// floats in (giving up after sixteen), else the middle of its feet rounded.
fn surface_y(ai: &Ai<'_, '_>) -> i32 {
    let [x, y, z] = ai.position();
    if !(ai.in_water() && can_float_own(ai)) {
        return physics::floor(y + 0.5);
    }
    let block_y = ai.block_position().y;
    let mut at = block_y;
    let floatable = |at: i32| {
        clustine_blocks::data::fluid_in_tag(
            ai.tick
                .world
                .state(physics::containing([x, f64::from(at), z])),
            "minecraft:entity_floatable",
        )
    };
    let mut count = 0;
    while floatable(at) {
        at += 1;
        count += 1;
        if count > 16 {
            return block_y;
        }
    }
    at
}

/// `Path.getEntityPosAtNode(entity, index)`: the node's cell, moved in by half the
/// mob's width rounded up to whole blocks.
fn entity_pos_at(ai: &Ai<'_, '_>, node: PathNode) -> Vec3 {
    let offset = f64::from((ai.width() + 1.0) as i32) * 0.5;
    [
        f64::from(node.x) + offset,
        f64::from(node.y),
        f64::from(node.z) + offset,
    ]
}

/// What `createPath` gives: the path under way, unchanged, or a new one.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Made {
    Current,
    New(FoundPath),
}

impl Made {
    /// The path, as `moveTo` or a goal reads it.
    pub(crate) fn path<'p>(&'p self, ai: &'p Ai<'_, '_>) -> Option<&'p FoundPath> {
        match self {
            Made::Current => path(ai),
            Made::New(found) => Some(found),
        }
    }
}

/// `createPath(BlockPos, reach)` as the mob's navigation has it: a walker's target moved
/// to the surface in its chunk (no path where that chunk is neither loaded nor
/// copied), then the common body.
fn create_path_own(ai: &mut Ai<'_, '_>, mut target: BlockPos, reach: i32) -> Option<Made> {
    // `WallClimberNavigation.createPath` keeps where it was sent first.
    if ai.config.navigation.wall_climber {
        crate::mobs::types::spider::set_path_to(&mut ai.tick.mob, Some(target));
    }
    if ai.config.navigation.kind == Kind::Ground {
        if !ai.tick.world.has_chunk(target.chunk()) {
            return None;
        }
        if !ai.config.navigation.below_surface {
            target = surface_position(ai.tick.world, target);
        }
    }
    create_path_to(ai, target, reach, max_path_length(ai))
}

/// `createPath(x, y, z, reach)`.
fn create_path_at_own(ai: &mut Ai<'_, '_>, at: Vec3, reach: i32) -> Option<Made> {
    create_path_own(ai, physics::containing(at), reach)
}

/// `GroundPathNavigation.findSurfacePosition`: the first air above what is solid below
/// an air target, or above a solid one.
fn surface_position<W: World + ?Sized>(world: &W, pos: BlockPos) -> BlockPos {
    let (min_y, max_y) = height_limits(world, pos);
    let air = |y: i32| world.state(BlockPos::new(pos.x, y, pos.z)).props().is_air();
    let solid = |y: i32| {
        world
            .state(BlockPos::new(pos.x, y, pos.z))
            .props()
            .is_solid()
    };
    let mut pos = pos;
    if air(pos.y) {
        let mut y = pos.y - 1;
        while y >= min_y && air(y) {
            y -= 1;
        }
        if y >= min_y {
            return BlockPos::new(pos.x, y + 1, pos.z);
        }
        y = pos.y + 1;
        while y <= max_y && air(y) {
            y += 1;
        }
        pos = BlockPos::new(pos.x, y, pos.z);
    }
    if solid(pos.y) {
        let mut y = pos.y + 1;
        while y <= max_y && solid(y) {
            y += 1;
        }
        return BlockPos::new(pos.x, y, pos.z);
    }
    pos
}

/// The world's lowest and highest block (`getMinY`, `getMaxY`) for the chunk's column:
/// the overworld's.
fn height_limits<W: World + ?Sized>(_world: &W, _pos: BlockPos) -> (i32, i32) {
    (OVERWORLD_MIN_Y, OVERWORLD_MIN_Y + OVERWORLD_HEIGHT - 1)
}

/// The overworld's bottom and height, the one dimension Clustine has.
pub(crate) const OVERWORLD_MIN_Y: i32 = -64;
pub(crate) const OVERWORLD_HEIGHT: i32 = 384;

/// `createPath(targets, regionOffset, offsetUpward, reach, range)`: the path under way
/// where it goes to the same target, else a search (its box is not kept: what it does
/// not read it does not need, the brief's 3.3).
fn create_path_to(ai: &mut Ai<'_, '_>, target: BlockPos, reach: i32, range: f32) -> Option<Made> {
    if ai.position()[1] < f64::from(ai.tick.world.min_y()) {
        return None;
    }
    if !can_update_path(ai) {
        return None;
    }
    if !is_done_own(ai) && ai.tick.mob.body.progress.target == Some(target) {
        return Some(Made::Current);
    }
    let searcher = searcher(ai);
    let limit = max_visited_nodes(ai);
    let world = ai.tick.world;
    let found = find_path(world, &searcher, target, range, reach, limit, ai.tick.draws)?;
    if let Evaluator::Amphibious { .. } = searcher.evaluator {
        // `AmphibiousNodeEvaluator.prepare` leaves the mob's water free for good.
        ai.set_malus(PathType::Water, 0.0);
    }
    let progress = &mut ai.tick.mob.body.progress;
    progress.target = Some(target);
    progress.reach = reach;
    reset_stuck_timeout(ai);
    Some(Made::New(found))
}

/// `resetStuckTimeout()`.
fn reset_stuck_timeout(ai: &mut Ai<'_, '_>) {
    let progress = &mut ai.tick.mob.body.progress;
    progress.timeout_node = [0; 3];
    progress.timeout_limit = 0.0;
    progress.stuck = false;
    ai.tick.mob.body.timeout.timer = 0;
}

/// `moveTo(x, y, z, speed)`: reach 1.
fn move_to_own(ai: &mut Ai<'_, '_>, at: Vec3, speed: f64) -> bool {
    move_to_reach_own(ai, at, 1, speed)
}

/// `moveTo(x, y, z, reach, speed)`.
fn move_to_reach_own(ai: &mut Ai<'_, '_>, at: Vec3, reach: i32, speed: f64) -> bool {
    let made = create_path_at_own(ai, at, reach);
    move_to_path_own(ai, made, speed)
}

/// `moveTo(entity, speed)`: to the block it stands in, reach 1 (every navigation of
/// N7's searches the box of a block for it, or one whose difference it does not read).
/// A wall climber's reach is 0, and without a path it is sent there all the same.
fn move_to_entity_own(ai: &mut Ai<'_, '_>, entity: BlockPos, speed: f64) -> bool {
    if ai.config.navigation.wall_climber {
        let made = create_path_own(ai, entity, 0);
        if made.is_some() {
            return move_to_path_own(ai, made, speed);
        }
        crate::mobs::types::spider::set_path_to(&mut ai.tick.mob, Some(entity));
        ai.tick.mob.body.progress.speed = speed;
        return true;
    }
    let made = create_path_own(ai, entity, 1);
    move_to_path_own(ai, made, speed)
}

/// `moveTo(path, speed)`: a path equal node for node to the one followed keeps it and
/// its progress.
fn move_to_path_own(ai: &mut Ai<'_, '_>, made: Option<Made>, speed: f64) -> bool {
    let Some(made) = made else {
        stop_own(ai);
        return false;
    };
    if let Made::New(found) = made {
        let same = path(ai).is_some_and(|held| same_nodes(held, &found));
        if !same {
            ai.tick.mob.body.path.followed = Some(found);
            ai.tick.mob.body.progress.next = 0;
        }
    }
    if is_done_own(ai) {
        return false;
    }
    trim_path(ai);
    if path(ai).is_none_or(|path| path.nodes.is_empty()) {
        return false;
    }
    let tick = navigation_tick(ai);
    let at = temp_mob_pos(ai);
    let progress = &mut ai.tick.mob.body.progress;
    progress.speed = speed;
    progress.stuck_check_at = tick;
    progress.stuck_position = at;
    true
}

/// `Path.sameAs`: the same cells in the same order.
fn same_nodes(a: &FoundPath, b: &FoundPath) -> bool {
    a.nodes.len() == b.nodes.len()
        && a.nodes
            .iter()
            .zip(&b.nodes)
            .all(|(a, b)| (a.x, a.y, a.z) == (b.x, b.y, b.z))
}

/// `PathNavigation.trimPath()`: a node in a cauldron is raised out of it, and the next
/// with it where it is no higher; then `GroundPathNavigation`'s: a mob that avoids the
/// sun and stands where the sky is not seen keeps its path only up to the first node
/// that sees it.
fn trim_path(ai: &mut Ai<'_, '_>) {
    trim_cauldrons(ai);
    let avoids = ai.config.navigation.avoid_sun || ai.tick.mob_type.avoids_sun(&ai.tick.mob);
    if ai.config.navigation.kind != Kind::Ground || !avoids {
        return;
    }
    let [x, y, z] = ai.position();
    if can_see_sky(ai, physics::containing([x, y + 0.5, z])) {
        return;
    }
    let Some(nodes) = ai
        .tick
        .mob
        .body
        .path
        .followed
        .as_ref()
        .map(|path| path.nodes.clone())
    else {
        return;
    };
    if let Some(cut) = nodes.iter().position(|node| can_see_sky(ai, node.pos()))
        && let Some(path) = ai.tick.mob.body.path.followed.as_mut()
    {
        // `Path.truncateNodes(i)`: the nodes from the first that sees the sky go.
        path.nodes.truncate(cut);
    }
}

/// `Level.canSeeSky(pos)`: the sky's light is full there (`BlockAndTintGetter`'s rule,
/// as `burning.rs` reads it); a block the view does not read sees it.
pub(crate) fn can_see_sky(ai: &Ai<'_, '_>, pos: BlockPos) -> bool {
    ai.tick.world.light(pos).is_none_or(|light| light.sky >= 15)
}

/// `PathNavigation.trimPath()`'s own part: the cauldrons.
fn trim_cauldrons(ai: &mut Ai<'_, '_>) {
    let world = ai.tick.world;
    let Some(path) = ai.tick.mob.body.path.followed.as_mut() else {
        return;
    };
    for index in 0..path.nodes.len() {
        let node = path.nodes[index];
        if !pathfinder::is_cauldron(world.state(node.pos())) {
            continue;
        }
        path.nodes[index] = PathNode {
            y: node.y + 1,
            ..node
        };
        if let Some(&next) = path.nodes.get(index + 1)
            && node.y >= next.y
        {
            // `node.cloneAndMove(next.x, node.y + 1, next.z)`: the raised node's type.
            path.nodes[index + 1] = PathNode {
                x: next.x,
                y: node.y + 1,
                z: next.z,
                kind: node.kind,
            };
        }
    }
}

/// `PathNavigation.tick()`; a wall climber's own once its path is done.
pub(crate) fn tick(ai: &mut Ai<'_, '_>) {
    ai.navigation_ticked = true;
    if ai.config.navigation.wall_climber && is_done_own(ai) {
        wall_climber_tick(ai);
        return;
    }
    if ai.tick.mob.body.progress.delayed_recompute {
        recompute_path_own(ai);
    }
    if is_done_own(ai) {
        return;
    }
    if can_update_path(ai) {
        follow_the_path(ai);
    } else if let Some(path) = path(ai).cloned() {
        let next = path.nodes[next_index(ai)];
        let at = temp_mob_pos(ai);
        let wanted = entity_pos_at(ai, next);
        let advance = if ai.config.navigation.kind == Kind::Flying {
            let block = ai.block_position();
            block.x == physics::floor(wanted[0])
                && block.y == physics::floor(wanted[1])
                && block.z == physics::floor(wanted[2])
        } else {
            at[1] > wanted[1]
                && !ai.on_ground()
                && physics::floor(at[0]) == physics::floor(wanted[0])
                && physics::floor(at[2]) == physics::floor(wanted[2])
        };
        if advance {
            ai.tick.mob.body.progress.next += 1;
        }
    }
    if is_done_own(ai) {
        return;
    }
    let Some(path) = path(ai) else {
        return;
    };
    let next = entity_pos_at(ai, path.nodes[next_index(ai)]);
    let y = match ai.config.navigation.kind {
        Kind::Ground => ground_y(ai, next),
        _ => next[1],
    };
    let speed = ai.tick.mob.body.progress.speed;
    controls::set_wanted_position(ai, [next[0], y, next[2]], speed);
}

/// `WallClimberNavigation.tick()` with its path done: where it was sent is forgotten once
/// it is near (within its width of that block's centre, or of the same column's block at
/// its own height when it is above it); until then it is steered straight at that block's
/// corner. **Named**: the game's navigation does not count this tick, which Clustine's
/// count of the mob's AI steps does (`navigation_tick`).
fn wall_climber_tick(ai: &mut Ai<'_, '_>) {
    let Some(target) = crate::mobs::types::spider::path_to(&ai.tick.mob) else {
        return;
    };
    let at = ai.position();
    let width = f64::from(ai.width());
    // `BlockPos.closerToCenterThan(position, distance)`.
    let near = |pos: BlockPos| {
        let centre = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 0.5,
            f64::from(pos.z) + 0.5,
        ];
        physics::distance_sqr(centre, at) < width * width
    };
    let above = at[1] > f64::from(target.y)
        && near(physics::containing([
            f64::from(target.x),
            at[1],
            f64::from(target.z),
        ]));
    if near(target) || above {
        crate::mobs::types::spider::set_path_to(&mut ai.tick.mob, None);
    } else {
        let speed = ai.tick.mob.body.progress.speed;
        let wanted = [
            f64::from(target.x),
            f64::from(target.y),
            f64::from(target.z),
        ];
        controls::set_wanted_position(ai, wanted, speed);
    }
}

/// `getGroundY(v)`: over air the point's own height, else the floor of its cell.
fn ground_y(ai: &Ai<'_, '_>, at: Vec3) -> f64 {
    let pos = physics::containing(at);
    let below = BlockPos::new(pos.x, pos.y - 1, pos.z);
    if ai.tick.world.state(below).props().is_air() {
        at[1]
    } else {
        pathfinder::floor_level(ai.tick.world, pos)
    }
}

/// `followThePath()`: the next node reached, or skipped when the one after it lies on
/// the other side; then the stuck check and the timeout.
fn follow_the_path(ai: &mut Ai<'_, '_>) {
    let at = temp_mob_pos(ai);
    let width = ai.width();
    let reach = if width > 0.75 {
        width / 2.0
    } else {
        0.75 - width / 2.0
    };
    let Some(path) = path(ai) else {
        return;
    };
    let node = path.nodes[next_index(ai)];
    let [x, y, z] = ai.position();
    let dx = (x - (f64::from(node.x) + 0.5)).abs();
    let dy = (y - f64::from(node.y)).abs();
    let dz = (z - (f64::from(node.z) + 0.5)).abs();
    let vertical = match ai.config.navigation.kind {
        Kind::WaterBound { .. } => 0.5_f32,
        _ => 1.0,
    };
    let close = dx < f64::from(reach) && dz < f64::from(reach) && dy < f64::from(vertical);
    if close || (can_cut_corner(node.kind()) && should_target_next_node_in_direction(ai, at)) {
        ai.tick.mob.body.progress.next += 1;
    }
    stuck_detection(ai, at);
}

/// `canCutCorner(type)`.
fn can_cut_corner(kind: PathType) -> bool {
    !matches!(
        kind,
        PathType::FireInNeighbor | PathType::DamagingInNeighbor | PathType::WalkableDoor
    )
}

/// `shouldTargetNextNodeInDirection(pos)`.
fn should_target_next_node_in_direction(ai: &Ai<'_, '_>, at: Vec3) -> bool {
    let Some(path) = path(ai) else {
        return false;
    };
    let index = next_index(ai);
    if index + 1 >= path.nodes.len() {
        return false;
    }
    let bottom = |node: PathNode| {
        [
            f64::from(node.x) + 0.5,
            f64::from(node.y),
            f64::from(node.z) + 0.5,
        ]
    };
    let centre = bottom(path.nodes[index]);
    if physics::distance_sqr(at, centre) >= 4.0 {
        return false;
    }
    if can_move_directly(ai, at, entity_pos_at(ai, path.nodes[index])) {
        return true;
    }
    let after = bottom(path.nodes[index + 1]);
    let a = physics::sub(centre, at);
    let b = physics::sub(after, at);
    if physics::length_sqr(b) < physics::length_sqr(a) || physics::length_sqr(a) < 0.5 {
        let a = physics::normalize(a);
        let b = physics::normalize(b);
        return b[0] * a[0] + b[1] * a[1] + b[2] * a[2] < 0.0;
    }
    false
}

/// `canMoveDirectly(from, to)`: never on the ground; a clear ray for the others.
fn can_move_directly(ai: &Ai<'_, '_>, from: Vec3, to: Vec3) -> bool {
    match ai.config.navigation.kind {
        Kind::Ground => false,
        Kind::Amphibious => ai.in_liquid() && clear_between(ai, from, to, false),
        Kind::WaterBound { .. } => clear_between(ai, from, to, false),
        Kind::Flying => clear_between(ai, from, to, true),
    }
}

/// `isClearForMovementBetween(mob, from, to, fluids)`: the ray to half the mob's height
/// above `to` meets no block (and, with `fluids`, no fluid).
fn clear_between(ai: &Ai<'_, '_>, from: Vec3, to: Vec3, fluids: bool) -> bool {
    let end = [to[0], to[1] + f64::from(ai.height()) * 0.5, to[2]];
    let context = crate::physics::collide::Context {
        bottom: ai.position()[1],
        fall_distance: ai.tick.living.body.fall_distance,
        falling_block: false,
        noclip: None,
    };
    let world = ai.tick.world;
    !physics::walk::traverse(from, end, |pos| {
        let state = world.state(pos);
        let block = crate::physics::collide::collision_shape(state, pos, context).is_some_and(
            |(shape, _)| {
                shape
                    .boxes()
                    .iter()
                    .any(|bounds| physics::body::clips_shape(bounds, from, end))
            },
        );
        // **Inferred**: `Fluid.ANY` picks any fluid, as high as it is (taken as the
        // full block where it holds a source or falls).
        let fluid = fluids && state.props().fluid().is_some() && {
            let base = crate::physics::aabb::Aabb::of_block(pos);
            physics::body::clips_shape(&base, from, end)
        };
        block || fluid
    })
}

/// `doStuckDetection(pos)`: every 101 of its ticks, a mob that moved too little is
/// stuck and stops; and in every tick with a path, the timeout of its next node.
fn stuck_detection(ai: &mut Ai<'_, '_>, at: Vec3) {
    let tick = navigation_tick(ai);
    let speed = ai.tick.mob.body.inputs.speed;
    let progress = &ai.tick.mob.body.progress;
    if tick.wrapping_sub(progress.stuck_check_at) as i32 > 100 {
        let base = if speed >= 1.0 { speed } else { speed * speed };
        let distance = base * 100.0 * 0.25;
        let stuck =
            physics::distance_sqr(at, progress.stuck_position) < f64::from(distance * distance);
        if stuck {
            ai.tick.mob.body.progress.stuck = true;
            stop_own(ai);
        } else {
            ai.tick.mob.body.progress.stuck = false;
        }
        let progress = &mut ai.tick.mob.body.progress;
        progress.stuck_check_at = tick;
        progress.stuck_position = at;
    }
    let Some(path) = path(ai) else {
        return;
    };
    let index = next_index(ai);
    if index >= path.nodes.len() {
        return;
    }
    let node = path.nodes[index];
    let now = ai.now();
    let cell = [node.x, node.y, node.z];
    let body = &mut ai.tick.mob.body;
    let (progress, timeout) = (&mut body.progress, &mut body.timeout);
    if cell == progress.timeout_node {
        timeout.timer += now - timeout.last_check;
    } else {
        progress.timeout_node = cell;
        let centre = [
            f64::from(node.x) + 0.5,
            f64::from(node.y),
            f64::from(node.z) + 0.5,
        ];
        let distance = physics::distance_sqr(at, centre).sqrt();
        progress.timeout_limit = if speed > 0.0 {
            distance / f64::from(speed) * 20.0
        } else {
            0.0
        };
    }
    if progress.timeout_limit > 0.0 && timeout.timer as f64 > progress.timeout_limit * 3.0 {
        // `timeoutPath`.
        reset_stuck_timeout(ai);
        stop_own(ai);
    }
    ai.tick.mob.body.timeout.last_check = now;
}

/// `recomputePath()`: at most once in 20 ticks; a refusal is retried at the
/// navigation's next ticks. No `moveTo`: no trim, the stuck check and speed kept.
fn recompute_path_own(ai: &mut Ai<'_, '_>) {
    let now = ai.tick.lived;
    let due = ai
        .tick
        .mob
        .body
        .progress
        .last_recompute
        .is_none_or(|last| i64::from(now) - i64::from(last) > 20);
    if due && can_update_path(ai) {
        if let Some(target) = ai.tick.mob.body.progress.target {
            stop_own(ai);
            let reach = ai.tick.mob.body.progress.reach;
            let made = create_path_own(ai, target, reach);
            ai.tick.mob.body.path.followed = match made {
                Some(Made::New(found)) => Some(found),
                _ => None,
            };
            ai.tick.mob.body.progress.next = 0;
            let progress = &mut ai.tick.mob.body.progress;
            progress.last_recompute = Some(now);
            progress.delayed_recompute = false;
        }
    } else {
        ai.tick.mob.body.progress.delayed_recompute = true;
    }
}

/// `shouldRecomputePath(changed)`: the change lies in the sphere about the midpoint of
/// the mob and its path's end whose radius is the nodes still to go.
pub(crate) fn should_recompute_path(ai: &Ai<'_, '_>, changed: BlockPos) -> bool {
    if ai.tick.mob.body.progress.delayed_recompute {
        return false;
    }
    let Some(path) = path(ai) else {
        return false;
    };
    let index = next_index(ai);
    if index >= path.nodes.len() || path.nodes.is_empty() {
        return false;
    }
    let Some(end) = path.nodes.last() else {
        return false;
    };
    let [x, y, z] = ai.position();
    let middle = [
        (f64::from(end.x) + x) / 2.0,
        (f64::from(end.y) + y) / 2.0,
        (f64::from(end.z) + z) / 2.0,
    ];
    let left = (path.nodes.len() - index) as f64;
    let dx = f64::from(changed.x) + 0.5 - middle[0];
    let dy = f64::from(changed.y) + 0.5 - middle[1];
    let dz = f64::from(changed.z) + 0.5 - middle[2];
    dx * dx + dy * dy + dz * dz < left * left
}

/// What `sendBlockUpdated` asks of a navigating mob when the collision shape at
/// `changed` changed: a path that passes near is made again.
pub(crate) fn block_changed(ai: &mut Ai<'_, '_>, changed: BlockPos) {
    if should_recompute_path(ai, changed) {
        recompute_path_own(ai);
    }
}

/// `isStableDestination(pos)`: somewhere to stand, as the navigation's kind has it.
pub(crate) fn is_stable_destination(ai: &Ai<'_, '_>, pos: BlockPos) -> bool {
    // A type's own navigation's (a turtle's).
    if let Some(stable) = ai.tick.mob_type.stable_destination(ai, pos) {
        return stable;
    }
    let world = ai.tick.world;
    let below = BlockPos::new(pos.x, pos.y - 1, pos.z);
    match ai.config.navigation.kind {
        Kind::Ground => world.state(below).props().is_solid_render(),
        Kind::Amphibious => !world.state(below).props().is_air(),
        Kind::WaterBound { .. } => !world.state(pos).props().is_solid_render(),
        // `entityCanStandOn`: the top of its collision shape is full (**Inferred** from
        // the face's sturdiness, which is the support shape's).
        Kind::Flying => world
            .state(pos)
            .props()
            .is_face_sturdy(clustine_data::Face::Up, clustine_data::SupportType::Full),
    }
}

// What a mob's goals ask of its navigation (`Mob.getNavigation()`): the vehicle's where it
// steers one (`docs/groundwork/n9-plan.md`, section 6), its own otherwise. Its own
// navigation's tick reads its own (`this.navigation.tick()`), as the functions above do.

/// `getNavigation().createPath(target, reach)`.
pub(crate) fn create_path(ai: &mut Ai<'_, '_>, target: BlockPos, reach: i32) -> Option<Made> {
    match ai.steered() {
        Some(mut vehicle) => create_path_own(&mut vehicle, target, reach),
        None => create_path_own(ai, target, reach),
    }
}

/// `getNavigation().createPath(x, y, z, reach)`.
pub(crate) fn create_path_at(ai: &mut Ai<'_, '_>, at: Vec3, reach: i32) -> Option<Made> {
    match ai.steered() {
        Some(mut vehicle) => create_path_at_own(&mut vehicle, at, reach),
        None => create_path_at_own(ai, at, reach),
    }
}

/// `getNavigation().moveTo(x, y, z, speed)`.
pub(crate) fn move_to(ai: &mut Ai<'_, '_>, at: Vec3, speed: f64) -> bool {
    match ai.steered() {
        Some(mut vehicle) => move_to_own(&mut vehicle, at, speed),
        None => move_to_own(ai, at, speed),
    }
}

/// `getNavigation().moveTo(x, y, z, reach, speed)`.
pub(crate) fn move_to_reach(ai: &mut Ai<'_, '_>, at: Vec3, reach: i32, speed: f64) -> bool {
    match ai.steered() {
        Some(mut vehicle) => move_to_reach_own(&mut vehicle, at, reach, speed),
        None => move_to_reach_own(ai, at, reach, speed),
    }
}

/// `getNavigation().moveTo(entity, speed)`.
pub(crate) fn move_to_entity(ai: &mut Ai<'_, '_>, entity: BlockPos, speed: f64) -> bool {
    match ai.steered() {
        Some(mut vehicle) => move_to_entity_own(&mut vehicle, entity, speed),
        None => move_to_entity_own(ai, entity, speed),
    }
}

/// `getNavigation().moveTo(path, speed)`.
pub(crate) fn move_to_path(ai: &mut Ai<'_, '_>, made: Option<Made>, speed: f64) -> bool {
    match ai.steered() {
        Some(mut vehicle) => move_to_path_own(&mut vehicle, made, speed),
        None => move_to_path_own(ai, made, speed),
    }
}

/// `getNavigation().recomputePath()`.
pub(crate) fn recompute_path(ai: &mut Ai<'_, '_>) {
    match ai.steered() {
        Some(mut vehicle) => recompute_path_own(&mut vehicle),
        None => recompute_path_own(ai),
    }
}

/// `getNavigation().stop()`.
pub(crate) fn stop(ai: &mut Ai<'_, '_>) {
    match ai.steered() {
        Some(mut vehicle) => stop_own(&mut vehicle),
        None => stop_own(ai),
    }
}

/// `getNavigation().setSpeedModifier(speed)`.
pub(crate) fn set_speed_modifier(ai: &mut Ai<'_, '_>, speed: f64) {
    match ai.steered() {
        Some(mut vehicle) => set_speed_modifier_own(&mut vehicle, speed),
        None => set_speed_modifier_own(ai, speed),
    }
}

/// The record and configuration of the navigation a mob's goals read: the vehicle's
/// where it steers one.
fn steered_parts<'a>(ai: &'a Ai<'_, '_>) -> (&'a crate::mobs::Mob, &'a super::Config) {
    match ai.tick.controlled.as_deref() {
        Some(vehicle) => (&vehicle.tick.mob, &vehicle.config),
        None => (&ai.tick.mob, ai.config),
    }
}

/// `getNavigation().isDone()`.
pub(crate) fn is_done(ai: &Ai<'_, '_>) -> bool {
    let (mob, _) = steered_parts(ai);
    done_of(mob)
}

/// `getNavigation().isInProgress()`.
pub(crate) fn in_progress(ai: &Ai<'_, '_>) -> bool {
    !is_done(ai)
}

/// `getNavigation().getTargetPos()`.
pub(crate) fn target_pos(ai: &Ai<'_, '_>) -> Option<BlockPos> {
    steered_parts(ai).0.body.progress.target
}

/// `getNavigation().canFloat()`.
pub(crate) fn can_float(ai: &Ai<'_, '_>) -> bool {
    steered_parts(ai).1.navigation.flags.can_float
}

/// `getNavigation() instanceof GroundPathNavigation`.
pub(crate) fn can_navigate_ground(ai: &Ai<'_, '_>) -> bool {
    matches!(
        steered_parts(ai).1.navigation.kind,
        Kind::Ground | Kind::Amphibious
    )
}

/// `isDone()` of the navigation kept in `mob`'s record.
fn done_of(mob: &crate::mobs::Mob) -> bool {
    mob.body
        .path
        .followed
        .as_ref()
        .is_none_or(|path| mob.body.progress.next as usize >= path.nodes.len())
}
