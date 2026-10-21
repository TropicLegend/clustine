//! The long jump (the brief's 7.3.16 and 7.3.17): `LongJumpToRandomPos` with the frog's
//! `LongJumpToPreferredBlock`, `LongJumpUtil`'s arc, and `LongJumpMidJump`.
//!
//! What a jump keeps (the plan, section 8 item 9): the jump chosen, the moment its
//! preparation began, the position it started from (compared exactly), its tries, and the
//! frog's wish for its preferred blocks. The candidates are not kept: they are a function
//! of the block the mob stood on at the start, all drawn in the tick that starts the jump
//! (its first tick, which picks until it chooses one or has none left), so that after it
//! only whether one was chosen is read.
//!
//! HotSpot's `Math.atan2`, `sin`, `cos` and the JDK's `Collections.shuffle` are taken as
//! `libm`'s and the mob's stream's (the brief's 7.1.1 findings 4 and 5).

use clustine_blocks::data::in_tag;
use clustine_data::{attributes, blocks};
use clustine_world::BlockPos;
use serde::{Deserialize, Serialize};

use super::super::{Behaviour, Cx, HeldTimed, MemoryKind, Status, Timed, Tracker, Value};
use super::{centre, sample};
use crate::mobs::Mob;
use crate::mobs::ai::{maths, navigation, pathfinder};
use crate::mobs::body::Event;
use crate::physics::aabb::Aabb;
use crate::physics::collide::{Context, no_collision};
use crate::physics::{self, Vec3, World};
use crate::region::Random;

/// What a long jump's type gives it: its sounds, the landing it accepts, its size in
/// the jump's pose, and the pose and friction a mid-air mob keeps (which only a type's
/// body has).
pub(crate) trait Leaper: Sync {
    /// The jump's sound (`getJumpSound`).
    fn jump_sound(&self, mob: &Mob) -> i32;

    /// `acceptableLandingSpot(mob, pos)`: by default the block below is solid to render
    /// and the cell costs nothing.
    fn landing(&self, cx: &Cx<'_, '_, '_>, pos: BlockPos) -> bool {
        default_landing(cx, pos)
    }

    /// `getDimensions(Pose.LONG_JUMPING)`: its width and height.
    fn long_jumping_size(&self, cx: &Cx<'_, '_, '_>) -> (f32, f32) {
        (cx.ai.width(), cx.ai.height())
    }

    /// `setDiscardFriction(discard)`.
    fn set_discard_friction(&self, cx: &mut Cx<'_, '_, '_>, discard: bool) {
        let _ = (cx, discard);
    }

    /// `setPose(LONG_JUMPING)` where `jumping`, else `setPose(STANDING)`.
    fn set_long_jumping(&self, cx: &mut Cx<'_, '_, '_>, jumping: bool) {
        let _ = (cx, jumping);
    }
}

/// `LongJumpToRandomPos.defaultAcceptableLandingSpot(mob, pos)`.
pub(crate) fn default_landing(cx: &Cx<'_, '_, '_>, pos: BlockPos) -> bool {
    let world = cx.ai.tick.world;
    let below = BlockPos::new(pos.x, pos.y - 1, pos.z);
    world.state(below).props().is_solid_render()
        && cx
            .ai
            .malus(pathfinder::static_path_type(world, world.min_y(), pos))
            == 0.0
}

/// `LongJumpToPreferredBlock`'s: the block tag it prefers to land on, and the chance it
/// wants to at each start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Preferred {
    pub(crate) tag: &'static str,
    pub(crate) chance: f32,
}

/// `LongJumpToRandomPos(timeBetweenJumps, maxHeight, maxWidth, maxVelocityMultiplier,
/// sound[, landing])` (7.3.16), 200 ticks; with `preferred`, the frog's
/// `LongJumpToPreferredBlock`.
pub(crate) struct LongJumpToRandomPos {
    pub(crate) interval: (i32, i32),
    pub(crate) max_height: i32,
    pub(crate) max_width: i32,
    pub(crate) max_velocity_multiplier: f32,
    pub(crate) leaper: &'static dyn Leaper,
    pub(crate) preferred: Option<Preferred>,
}

/// What a long jump keeps.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Jumping {
    /// `chosenJump`.
    pub(crate) chosen: Option<Vec3>,
    /// `prepareJumpStart`, a game time.
    pub(crate) prepare_start: i64,
    /// `initialPosition`.
    pub(crate) initial: Option<Vec3>,
    /// `findJumpTries`.
    pub(crate) tries: i32,
    /// `currentlyWantingPreferredOnes`.
    pub(crate) wants_preferred: bool,
}

/// `FIND_JUMP_TRIES`, `PREPARE_JUMP_DURATION`, `MIN_PATHFIND_DISTANCE_TO_VALID_JUMP`.
const FIND_JUMP_TRIES: i32 = 20;
const PREPARE_JUMP_DURATION: i64 = 40;
const MIN_PATHFIND_DISTANCE: f32 = 8.0;
/// `ALLOWED_ANGLES`.
const ALLOWED_ANGLES: [i32; 4] = [65, 70, 75, 80];

/// A candidate: where to, and its weight (`PossibleJump`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PossibleJump {
    pub(crate) target: BlockPos,
    pub(crate) weight: i32,
}

/// The candidates of a jump from `origin`: every block of the box `width` across and
/// `height` up and down about it but its own, in `BlockPos.betweenClosed`'s order (x, then
/// y, then z), each weighted by the ceiling of its distance squared.
pub(crate) fn candidates(origin: BlockPos, width: i32, height: i32) -> Vec<PossibleJump> {
    let mut out = Vec::new();
    for z in origin.z - width..=origin.z + width {
        for y in origin.y - height..=origin.y + height {
            for x in origin.x - width..=origin.x + width {
                let target = BlockPos::new(x, y, z);
                if target == origin {
                    continue;
                }
                let distance = super::super::sensors::block_distance_sqr(origin, target);
                out.push(PossibleJump {
                    target,
                    weight: distance.ceil() as i32,
                });
            }
        }
    }
    out
}

/// `WeightedRandom.getRandomItem(random, list, weight)` and its removal: one
/// `nextInt(total)`, none where the total is nothing.
pub(crate) fn take_weighted(
    draws: &mut Random,
    list: &mut Vec<PossibleJump>,
) -> Option<PossibleJump> {
    let total: i64 = list.iter().map(|jump| i64::from(jump.weight)).sum();
    if total == 0 {
        return None;
    }
    let mut left =
        i64::from(draws.next_int(u32::try_from(total).expect("a jump's weights fit an int")));
    let at = list.iter().position(|jump| {
        left -= i64::from(jump.weight);
        left < 0
    })?;
    Some(list.remove(at))
}

/// `Collections.shuffle(list)` of the JDK: from the end, each swapped with one drawn at or
/// below it (`nextInt(i + 1)`).
pub(crate) fn jdk_shuffle<T>(draws: &mut Random, list: &mut [T]) {
    for i in (1..list.len()).rev() {
        let j = draws.next_int(i as u32 + 1) as usize;
        list.swap(i, j);
    }
}

/// `LongJumpUtil.calculateJumpVectorForAngle(mob, target, maxV, angle, true)`: the motion
/// that lands a jump at `angle` degrees at `target`, none where it needs more than `max_v`
/// or meets a block on its way.
pub(crate) fn jump_vector_for_angle(
    cx: &Cx<'_, '_, '_>,
    leaper: &dyn Leaper,
    target: Vec3,
    max_v: f32,
    angle: i32,
) -> Option<Vec3> {
    let origin = cx.ai.position();
    let half = physics::scale(
        physics::normalize([target[0] - origin[0], 0.0, target[2] - origin[2]]),
        0.5,
    );
    let aim = physics::sub(target, half);
    let d = physics::sub(aim, origin);
    let radians = angle as f32 * std::f32::consts::PI / 180.0;
    let yaw = libm::atan2(d[2], d[0]);
    let horizontal_sqr = d[0] * d[0] + d[2] * d[2];
    let horizontal = horizontal_sqr.sqrt();
    let rise = d[1];
    let gravity = cx.ai.attribute(attributes::GRAVITY);
    let sin2 = libm::sin(f64::from(2.0 * radians));
    let cos = libm::cos(f64::from(radians));
    let cos_sqr = cos * cos;
    let sin = libm::sin(f64::from(radians));
    let (yaw_sin, yaw_cos) = (libm::sin(yaw), libm::cos(yaw));
    let speed_sqr = horizontal_sqr * gravity / (horizontal * sin2 - 2.0 * rise * cos_sqr);
    if speed_sqr < 0.0 {
        return None;
    }
    let speed = speed_sqr.sqrt();
    if speed > f64::from(max_v) {
        return None;
    }
    let along = speed * cos;
    let up = speed * sin;
    let steps = (horizontal / along).ceil() as i32 * 2;
    let (width, height) = leaper.long_jumping_size(cx);
    let mut previous: Option<Vec3> = None;
    let mut x = 0.0;
    for _ in 0..steps - 1 {
        x += horizontal / f64::from(steps);
        let y = sin / cos * x - x * x * gravity / (2.0 * speed_sqr * cos_sqr);
        let at = [
            origin[0] + x * yaw_cos,
            origin[1] + y,
            origin[2] + x * yaw_sin,
        ];
        if let Some(from) = previous
            && !clear_transition(cx, width, height, from, at)
        {
            return None;
        }
        previous = Some(at);
    }
    Some(physics::scale(
        [along * yaw_cos, up, along * yaw_sin],
        0.949_999_988_079_071,
    ))
}

/// `LongJumpUtil.isClearTransition(mob, dimensions, from, to)`: the box of the pose free of
/// collisions at steps of nine tenths of its least side from `from`, and at `to`.
fn clear_transition(cx: &Cx<'_, '_, '_>, width: f32, height: f32, from: Vec3, to: Vec3) -> bool {
    let step = f64::from(width.min(height));
    let way = physics::sub(to, from);
    let count = (physics::length_sqr(way).sqrt() / step).ceil() as i32;
    let unit = physics::normalize(way);
    let context = Context {
        bottom: cx.ai.position()[1],
        fall_distance: cx.ai.tick.living.body.fall_distance,
        falling_block: false,
        noclip: None,
    };
    let world = cx.ai.tick.world;
    (0..count).all(|i| {
        let at = if i == count - 1 {
            to
        } else {
            physics::add(
                from,
                physics::scale(unit, step * 0.899_999_976_158_142_1 * f64::from(i)),
            )
        };
        let bounds = Aabb::of_entity(at, width, height);
        no_collision(world, context, &cx.ai.tick.living.body.mover, &bounds)
    })
}

impl LongJumpToRandomPos {
    pub(crate) fn boxed(self) -> Box<dyn Behaviour> {
        Box::new(HeldTimed::new(self))
    }

    /// The cooldown a jump that does not take off leaves: half the interval drawn.
    fn cool_down_half(&self, cx: &mut Cx<'_, '_, '_>) {
        let cooldown = sample(cx.ai.tick.draws, self.interval) / 2;
        cx.memories
            .set(MemoryKind::LongJumpCoolingDown, Value::Int(cooldown));
    }

    /// `getJumpCandidate(level)`, and the frog's: where it wants its preferred blocks,
    /// candidates drawn until one is above them, those that are not kept aside and taken
    /// in their order once none is left.
    fn next_candidate(
        &self,
        state: &Jumping,
        cx: &mut Cx<'_, '_, '_>,
        list: &mut Vec<PossibleJump>,
        aside: &mut Vec<PossibleJump>,
    ) -> Option<PossibleJump> {
        let preferred = self.preferred.filter(|_| state.wants_preferred);
        let Some(preferred) = preferred else {
            return take_weighted(cx.ai.tick.draws, list);
        };
        while !list.is_empty() {
            if let Some(jump) = take_weighted(cx.ai.tick.draws, list) {
                let below = BlockPos::new(jump.target.x, jump.target.y - 1, jump.target.z);
                if in_tag(cx.ai.tick.world.state(below), preferred.tag) {
                    return Some(jump);
                }
                aside.push(jump);
            }
        }
        (!aside.is_empty()).then(|| aside.remove(0))
    }

    /// `pickCandidate(level, mob, time)`: candidates drawn until one is acceptable, has an
    /// arc, and is not reached on foot within 8; it is chosen and its preparation begins.
    fn pick(&self, state: &mut Jumping, cx: &mut Cx<'_, '_, '_>) {
        let origin = cx.ai.block_position();
        let mut list = candidates(origin, self.max_width, self.max_height);
        let mut aside = Vec::new();
        while !list.is_empty() {
            let Some(jump) = self.next_candidate(state, cx, &mut list, &mut aside) else {
                continue;
            };
            let target = jump.target;
            let own = cx.ai.block_position();
            if (target.x == own.x && target.z == own.z) || !self.leaper.landing(cx, target) {
                continue;
            }
            let Some(vector) = self.optimal_vector(cx, centre(target)) else {
                continue;
            };
            cx.memories.set(
                MemoryKind::LookTarget,
                Value::Tracker(Tracker::Block(target)),
            );
            let reaches = create_path_within(cx, target).is_some_and(|reached| reached);
            if !reaches {
                state.chosen = Some(vector);
                state.prepare_start = cx.time;
                return;
            }
        }
    }

    /// `calculateOptimalJumpVector(mob, target)`: the four angles shuffled, the first
    /// that has an arc.
    fn optimal_vector(&self, cx: &mut Cx<'_, '_, '_>, target: Vec3) -> Option<Vec3> {
        let mut angles = ALLOWED_ANGLES;
        jdk_shuffle(cx.ai.tick.draws, &mut angles);
        let max_v = (cx.ai.attribute(attributes::JUMP_STRENGTH)
            * f64::from(self.max_velocity_multiplier)) as f32;
        angles
            .into_iter()
            .find_map(|angle| jump_vector_for_angle(cx, self.leaper, target, max_v, angle))
    }
}

/// `navigation.createPath(pos, 0, 8)`, the common navigation's (not a walker's, which
/// moves its target to the surface only in its own `createPath(pos, reach)`): a search
/// within 8, and whether its path reaches. **Named**: the contract's navigation makes no
/// path of a range of its caller's, so the search is made here as its `createPath` makes
/// it, without its keeping a path under way to the same target or its noting the
/// target as its own (`targetPos`, which only the stuck check reads, and a jump never
/// follows this path); the node limit is the one of the mob's path length.
fn create_path_within(cx: &mut Cx<'_, '_, '_>, target: BlockPos) -> Option<bool> {
    let ai = &mut *cx.ai;
    if ai.position()[1] < f64::from(ai.tick.world.min_y()) {
        return None;
    }
    let searcher = navigation::searcher(ai);
    let limit = maths::floor_f32(navigation::max_path_length(ai) * 16.0);
    let world = ai.tick.world;
    let found = pathfinder::find_path(
        world,
        &searcher,
        target,
        MIN_PATHFIND_DISTANCE,
        0,
        limit,
        ai.tick.draws,
    )?;
    Some(found.reached)
}

impl Timed for LongJumpToRandomPos {
    type State = Jumping;

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[
            (MemoryKind::LookTarget, Status::Registered),
            (MemoryKind::LongJumpCoolingDown, Status::Absent),
            (MemoryKind::LongJumpMidJump, Status::Absent),
        ]
    }

    fn durations(&self) -> (i64, i64) {
        (200, 200)
    }

    fn extra_start(&self, _: &mut Jumping, cx: &mut Cx<'_, '_, '_>) -> bool {
        let standing = cx.ai.on_ground()
            && !cx.ai.in_water()
            && !cx.ai.in_lava()
            && !clustine_blocks::data::is_block(
                cx.ai.tick.world.state(cx.ai.block_position()),
                blocks::HONEY_BLOCK,
            );
        if !standing {
            self.cool_down_half(cx);
        }
        standing
    }

    fn can_still_use(&self, state: &mut Jumping, cx: &mut Cx<'_, '_, '_>) -> bool {
        // The candidates are left only before the first tick has drawn them.
        let candidates_left = state.tries == FIND_JUMP_TRIES;
        let running = state.initial == Some(cx.ai.position())
            && state.tries > 0
            && !cx.ai.in_water()
            && (state.chosen.is_some() || candidates_left);
        if !running
            && cx
                .memories
                .check(MemoryKind::LongJumpMidJump, Status::Absent)
        {
            self.cool_down_half(cx);
            cx.memories.erase(MemoryKind::LookTarget);
        }
        running
    }

    fn start(&self, state: &mut Jumping, cx: &mut Cx<'_, '_, '_>) {
        *state = Jumping {
            chosen: None,
            prepare_start: 0,
            initial: Some(cx.ai.position()),
            tries: FIND_JUMP_TRIES,
            wants_preferred: false,
        };
        if let Some(preferred) = self.preferred {
            state.wants_preferred = cx.ai.tick.draws.next_float() < preferred.chance;
        }
    }

    fn tick(&self, state: &mut Jumping, cx: &mut Cx<'_, '_, '_>) {
        match state.chosen {
            Some(chosen) => {
                if cx.time - state.prepare_start < PREPARE_JUMP_DURATION {
                    return;
                }
                cx.ai.tick.yaw = cx.ai.tick.mob.body.head.body_yaw;
                self.leaper.set_discard_friction(cx, true);
                let length = physics::length_sqr(chosen).sqrt();
                // `getJumpBoostPower()`: nothing, no mob having effects.
                let boosted = length + 0.0;
                cx.ai.tick.living.body.motion = physics::scale(chosen, boosted / length);
                cx.memories
                    .set(MemoryKind::LongJumpMidJump, Value::Bool(true));
                let sound = self.leaper.jump_sound(&cx.ai.tick.mob);
                cx.ai.tick.out.events.push(Event::Sound {
                    sound,
                    volume: 1.0,
                    pitch: 1.0,
                });
            }
            None => {
                state.tries -= 1;
                self.pick(state, cx);
            }
        }
    }

    fn stop(&self, state: &mut Jumping, _: &mut Cx<'_, '_, '_>) {
        // Nothing of the game's: what a run leaves is read only by its next start, which
        // sets it all again; at rest it keeps nothing.
        *state = Jumping::default();
    }
}

/// `LongJumpMidJump(timeBetweenJumps, landingSound)` (7.3.17), 100 ticks: in the air
/// after a launch, the pose of the jump; landing, the motion's sideways part cut to a
/// tenth with the landing's sound, the pose back, and the whole interval drawn as the
/// cooldown.
pub(crate) struct LongJumpMidJump {
    pub(crate) interval: (i32, i32),
    pub(crate) landing_sound: i32,
    pub(crate) leaper: &'static dyn Leaper,
}

impl LongJumpMidJump {
    pub(crate) fn boxed(self) -> Box<dyn Behaviour> {
        Box::new(HeldTimed::new(self))
    }
}

impl Timed for LongJumpMidJump {
    type State = ();

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[
            (MemoryKind::LookTarget, Status::Registered),
            (MemoryKind::LongJumpMidJump, Status::Present),
        ]
    }

    fn durations(&self) -> (i64, i64) {
        (100, 100)
    }

    fn can_still_use(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) -> bool {
        !cx.ai.on_ground()
    }

    fn start(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        self.leaper.set_discard_friction(cx, true);
        self.leaper.set_long_jumping(cx, true);
    }

    fn stop(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        if cx.ai.on_ground() {
            let motion = &mut cx.ai.tick.living.body.motion;
            *motion = [motion[0] * 0.1, motion[1], motion[2] * 0.1];
            cx.ai.tick.out.events.push(Event::Sound {
                sound: self.landing_sound,
                volume: 2.0,
                pitch: 1.0,
            });
        }
        self.leaper.set_discard_friction(cx, false);
        self.leaper.set_long_jumping(cx, false);
        cx.memories.erase(MemoryKind::LongJumpMidJump);
        let cooldown = sample(cx.ai.tick.draws, self.interval);
        cx.memories
            .set(MemoryKind::LongJumpCoolingDown, Value::Int(cooldown));
    }
}
