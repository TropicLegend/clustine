//! The generic goals of farm animals and golems (the brief's 2.7 to 2.12), each as the
//! game's class with its arguments, its state the slot's part. The animals' own goals
//! (`BreedGoal`, `FollowParentGoal`, `EatBlockGoal`) read an animal's love, age and wool
//! and are their part's (N7e); a goal reads and writes its mob's type state through
//! `ai.tick.mob.state`.

use clustine_blocks::data::{entity_in_tag, fluid_in_tag, has_water, is_class, item_in_tag};
use clustine_data::{BlockClass, PathType, attributes};
use clustine_world::{BlockPos, EntityId, ItemStack};
use serde::{Deserialize, Serialize};

use super::Ai;
use super::controls;
use super::maths::{self, hotspot_cos, hotspot_sin, reduced_tick_delay};
use super::navigation::{self, Made};
use super::pathfinder::FoundPath;
use super::random_pos;
use super::selector::{Goal, flags};
use super::sensing::{self, Class, Conditions, Looked};
use crate::physics::{self, Vec3};

/// What a goal holds of a path it made in its `canUse` for its `start`: the path and its
/// index, as the game holds the object.
pub(crate) type HeldPath = Option<(FoundPath, u32)>;

/// The path `made` gives, as a goal holds it.
fn hold(ai: &Ai<'_, '_>, made: Option<Made>) -> HeldPath {
    match made? {
        Made::Current => ai
            .tick
            .mob
            .body
            .path
            .followed
            .clone()
            .map(|path| (path, ai.tick.mob.body.progress.next)),
        Made::New(found) => Some((found, 0)),
    }
}

/// `moveTo(path, speed)` of a path a goal held.
fn move_to_held(ai: &mut Ai<'_, '_>, held: HeldPath, speed: f64) -> bool {
    let Some((found, next)) = held else {
        return navigation::move_to_path(ai, None, speed);
    };
    let same = ai
        .tick
        .mob
        .body
        .path
        .followed
        .as_ref()
        .is_some_and(|followed| {
            followed.nodes.len() == found.nodes.len()
                && followed
                    .nodes
                    .iter()
                    .zip(&found.nodes)
                    .all(|(a, b)| (a.x, a.y, a.z) == (b.x, b.y, b.z))
        });
    if same {
        return navigation::move_to_path(ai, Some(Made::Current), speed);
    }
    ai.tick.mob.body.path.followed = Some(found);
    ai.tick.mob.body.progress.next = next;
    navigation::move_to_path(ai, Some(Made::Current), speed)
}

/// `Mob.getNoActionTime()` as the body's stamp gives it: the AI steps since its last
/// reset, counting this one.
pub(crate) fn no_action_time(ai: &Ai<'_, '_>) -> i32 {
    ai.tick
        .lived
        .saturating_add(1)
        .saturating_sub(ai.tick.mob.body.idle.no_action_since) as i32
}

/// Whether the mob's last damage, within the 40 ticks `getLastDamageSource` keeps it,
/// is one of `#panic_causes`. **Named**: the body keeps no damage source yet, only who
/// last hurt it; a hit by a player or a mob is a player's or a mob's attack, both of
/// the tag, and the fire, lava and cactus of the tag's environmental causes are not
/// seen.
fn panics_from_damage(ai: &Ai<'_, '_>) -> bool {
    let lived = ai.tick.lived;
    let recent = |at: u32| lived.saturating_sub(at) <= 40;
    let combat = &ai.tick.mob.body.combat;
    combat.last_hurt_by_player.is_some_and(|(_, at)| recent(at))
        || combat.last_hurt_by.is_some_and(|(_, at)| recent(at))
}

/// `BlockPos.withinManhattan(pos, x, y, z)`'s order (**Inferred**: the iterator,
/// `BlockPos$3`, is not in the dump; this is its known form): by Manhattan distance,
/// then x from low to high, then y, each z after its mirror.
pub(crate) fn manhattan_ordered(
    pos: BlockPos,
    x_size: i32,
    y_size: i32,
    z_size: i32,
) -> Vec<BlockPos> {
    let max_depth = x_size + y_size + z_size;
    let mut out = Vec::new();
    for depth in 0..=max_depth {
        let max_x = x_size.min(depth);
        for x in -max_x..=max_x {
            let max_y = y_size.min(depth - x.abs());
            for y in -max_y..=max_y {
                let z = depth - x.abs() - y.abs();
                if z <= z_size {
                    out.push(BlockPos::new(pos.x + x, pos.y + y, pos.z + z));
                    if z != 0 {
                        out.push(BlockPos::new(pos.x + x, pos.y + y, pos.z - z));
                    }
                }
            }
        }
    }
    out
}

/// `setLookAt(entity, yawSpeed, maxPitch)`: at its feet's x and z and its eyes.
fn look_at_thing(ai: &mut Ai<'_, '_>, thing: &Looked, yaw_speed: f32, max_pitch: f32) {
    controls::set_look_at(
        ai,
        [thing.position[0], thing.eye_y, thing.position[2]],
        yaw_speed,
        max_pitch,
    );
}

/// `FloatGoal(mob)`: jump, four times in five, while deep enough in water or in lava.
/// Building it lets the navigation float.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FloatGoal;

impl Goal for FloatGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::JUMP
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn on_register(&self, config: &mut super::Config) {
        config.navigation.set_can_float(true);
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        // `getFluidJumpThreshold`: nothing for eyes under 0.4 high, else 0.4.
        let threshold = if ai.tick.living.eye_height < 0.4 {
            0.0
        } else {
            0.4
        };
        ai.tick.living.fluids.water_height > threshold || ai.in_lava()
    }

    fn tick(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        if ai.tick.draws.next_float() < 0.8 {
            controls::jump(ai);
        }
    }
}

/// `PanicGoal`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Panic {
    pub(crate) wanted: Vec3,
    pub(crate) running: bool,
}

/// `PanicGoal(mob, speed)`: away from a hurt, towards water when burning.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PanicGoal {
    pub(crate) speed: f64,
}

impl PanicGoal {
    /// `lookForWater(level, mob, 5)`: none where the mob stands in something solid,
    /// else the first water by Manhattan distance in the box of 5, 1, 5.
    fn look_for_water(ai: &Ai<'_, '_>, range: i32) -> Option<BlockPos> {
        let at = ai.block_position();
        let world = ai.tick.world;
        if !world.state(at).voxel_collision_shape().is_empty() {
            return None;
        }
        manhattan_ordered(at, range, 1, range)
            .into_iter()
            .find(|pos| has_water(world.state(*pos)))
    }
}

impl Goal for PanicGoal {
    type State = Panic;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn panics(&self) -> bool {
        true
    }

    fn can_use(&self, state: &mut Panic, ai: &mut Ai<'_, '_>) -> bool {
        if !panics_from_damage(ai) {
            return false;
        }
        if ai.tick.living.is_on_fire()
            && let Some(water) = Self::look_for_water(ai, 5)
        {
            state.wanted = [f64::from(water.x), f64::from(water.y), f64::from(water.z)];
            return true;
        }
        // `findRandomPosition`.
        match random_pos::default_pos(ai, 5, 4) {
            Some(wanted) => {
                state.wanted = wanted;
                true
            }
            None => false,
        }
    }

    fn can_continue(&self, _: &mut Panic, ai: &mut Ai<'_, '_>) -> bool {
        !navigation::is_done(ai)
    }

    fn start(&self, state: &mut Panic, ai: &mut Ai<'_, '_>) {
        navigation::move_to(ai, state.wanted, self.speed);
        state.running = true;
    }

    fn stop(&self, state: &mut Panic, _: &mut Ai<'_, '_>) {
        state.running = false;
    }
}

/// Where a stroll goes (`RandomStrollGoal.getPosition` of each kind).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Stroll {
    /// `RandomStrollGoal`'s: `DefaultRandomPos.getPos(mob, 10, 7)`.
    Default,
    /// `WaterAvoidingRandomStrollGoal`'s: land, but for one time in `1 / probability`.
    AvoidingWater { probability: f32 },
    /// `RandomSwimmingGoal`'s: somewhere to swim.
    Swimming,
    /// `WaterAvoidingRandomFlyingGoal`'s: hovering ahead, else in the air or water.
    Flying,
}

/// `RandomStrollGoal`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Strolling {
    pub(crate) wanted: Vec3,
    /// `forceTrigger`.
    pub(crate) forced: bool,
}

/// `RandomStrollGoal(mob, speed, interval, checkNoActionTime)` and its kinds.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RandomStrollGoal {
    pub(crate) speed: f64,
    pub(crate) interval: i32,
    pub(crate) check_no_action_time: bool,
    pub(crate) kind: Stroll,
}

impl RandomStrollGoal {
    /// `RandomStrollGoal(mob, speed)`: every 120 ticks or so.
    pub(crate) fn new(speed: f64) -> Self {
        Self {
            speed,
            interval: 120,
            check_no_action_time: true,
            kind: Stroll::Default,
        }
    }

    /// `WaterAvoidingRandomStrollGoal(mob, speed)`.
    pub(crate) fn avoiding_water(speed: f64) -> Self {
        Self {
            kind: Stroll::AvoidingWater { probability: 0.001 },
            ..Self::new(speed)
        }
    }

    /// `RandomSwimmingGoal(mob, speed, interval)`.
    pub(crate) fn swimming(speed: f64, interval: i32) -> Self {
        Self {
            interval,
            kind: Stroll::Swimming,
            ..Self::new(speed)
        }
    }

    /// `getPosition()`.
    fn position(&self, ai: &mut Ai<'_, '_>) -> Option<Vec3> {
        match self.kind {
            Stroll::Default => random_pos::default_pos(ai, 10, 7),
            Stroll::AvoidingWater { probability } => {
                if ai.in_water() {
                    return random_pos::land_pos(ai, 15, 7)
                        .or_else(|| random_pos::default_pos(ai, 10, 7));
                }
                if ai.tick.draws.next_float() >= probability {
                    random_pos::land_pos(ai, 10, 7)
                } else {
                    random_pos::default_pos(ai, 10, 7)
                }
            }
            Stroll::Swimming => random_pos::swimmable_pos(ai, 10, 7),
            Stroll::Flying => {
                // `getViewVector(0)`: the look of its yaw and pitch.
                let view = view_vector(ai.tick.pitch, ai.tick.yaw);
                random_pos::hover_pos(ai, 8, 7, view[0], view[2], 1.570_796_4, 3, 1).or_else(|| {
                    random_pos::air_and_water_pos(
                        ai,
                        8,
                        4,
                        -2,
                        view[0],
                        view[2],
                        1.570_796_370_506_286_6,
                    )
                })
            }
        }
    }
}

/// `Entity.calculateViewVector(pitch, yaw)`: the game's table's sine and cosine.
fn view_vector(pitch: f32, yaw: f32) -> Vec3 {
    let pitch = pitch * maths::RADIANS;
    let yaw = -yaw * maths::RADIANS;
    let (yaw_cos, yaw_sin) = (maths::table_cos(yaw), maths::table_sin(yaw));
    let (pitch_cos, pitch_sin) = (maths::table_cos(pitch), maths::table_sin(pitch));
    [
        f64::from(yaw_sin * pitch_cos),
        f64::from(-pitch_sin),
        f64::from(yaw_cos * pitch_cos),
    ]
}

impl Goal for RandomStrollGoal {
    type State = Strolling;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, state: &mut Strolling, ai: &mut Ai<'_, '_>) -> bool {
        // No mob of N7's goals carries a controlling passenger the region tells of.
        if !state.forced {
            if self.check_no_action_time && no_action_time(ai) >= 100 {
                return false;
            }
            let bound = reduced_tick_delay(self.interval).max(1) as u32;
            if ai.tick.draws.next_int(bound) != 0 {
                return false;
            }
        }
        let Some(wanted) = self.position(ai) else {
            return false;
        };
        state.wanted = wanted;
        state.forced = false;
        true
    }

    fn can_continue(&self, _: &mut Strolling, ai: &mut Ai<'_, '_>) -> bool {
        !navigation::is_done(ai)
    }

    fn start(&self, state: &mut Strolling, ai: &mut Ai<'_, '_>) {
        navigation::move_to(ai, state.wanted, self.speed);
    }

    fn stop(&self, _: &mut Strolling, ai: &mut Ai<'_, '_>) {
        navigation::stop(ai);
    }
}

/// `RandomLookAroundGoal`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct LookingAround {
    pub(crate) rel_x: f64,
    pub(crate) rel_z: f64,
    pub(crate) look_time: i32,
}

/// `RandomLookAroundGoal(mob)`: stand and look about for a second or two.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct RandomLookAroundGoal;

impl Goal for RandomLookAroundGoal {
    type State = LookingAround;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, _: &mut LookingAround, ai: &mut Ai<'_, '_>) -> bool {
        ai.tick.draws.next_float() < 0.02
    }

    fn can_continue(&self, state: &mut LookingAround, _: &mut Ai<'_, '_>) -> bool {
        state.look_time >= 0
    }

    fn start(&self, state: &mut LookingAround, ai: &mut Ai<'_, '_>) {
        let angle = std::f64::consts::TAU * ai.tick.draws.next_double();
        state.rel_x = hotspot_cos(angle);
        state.rel_z = hotspot_sin(angle);
        state.look_time = 20 + ai.tick.draws.next_int(20) as i32;
    }

    fn tick(&self, state: &mut LookingAround, ai: &mut Ai<'_, '_>) {
        state.look_time -= 1;
        let [x, _, z] = ai.position();
        let eye = ai.eye_y();
        controls::set_look_at_default(ai, [x + state.rel_x, eye, z + state.rel_z]);
    }
}

/// `LookAtPlayerGoal`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct LookingAt {
    pub(crate) look_at: Option<EntityId>,
    pub(crate) look_time: i32,
}

/// `LookAtPlayerGoal(mob, class, lookDistance, probability, onlyHorizontal)`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LookAtPlayerGoal {
    pub(crate) class: Class,
    pub(crate) look_distance: f32,
    pub(crate) probability: f32,
    pub(crate) only_horizontal: bool,
    /// `InteractGoal`'s: it holds the mob still too.
    pub(crate) holds_still: bool,
}

impl LookAtPlayerGoal {
    pub(crate) fn new(class: Class, look_distance: f32) -> Self {
        Self {
            class,
            look_distance,
            probability: 0.02,
            only_horizontal: false,
            holds_still: false,
        }
    }

    fn conditions(&self) -> Conditions {
        Conditions::non_combat().range(f64::from(self.look_distance))
    }
}

impl Goal for LookAtPlayerGoal {
    type State = LookingAt;

    fn flags(&self) -> u8 {
        if self.holds_still {
            flags::LOOK | flags::MOVE
        } else {
            flags::LOOK
        }
    }

    fn can_use(&self, state: &mut LookingAt, ai: &mut Ai<'_, '_>) -> bool {
        if ai.tick.draws.next_float() >= self.probability {
            return false;
        }
        // (The game first takes the mob's target, and at once overwrites it.)
        let [x, _, z] = ai.position();
        let eyes = [x, ai.eye_y(), z];
        let conditions = self.conditions();
        let found = if self.class == Class::Player {
            let me = ai.tick.key;
            // `EntitySelector.notRiding(mob)`: not a player riding the mob.
            let not_riding = move |looked: &Looked| looked.vehicle != Some(me);
            sensing::nearest_player(ai, &conditions, &not_riding, eyes)
        } else {
            let distance = f64::from(self.look_distance);
            let bounds = ai_bounds(ai).inflate([distance, 3.0, distance]);
            let candidates = sensing::things_in(ai, bounds, self.class);
            sensing::nearest(ai, candidates, &conditions, &|_| true, eyes)
        };
        state.look_at = found.map(|found| found.key);
        state.look_at.is_some()
    }

    fn can_continue(&self, state: &mut LookingAt, ai: &mut Ai<'_, '_>) -> bool {
        let Some(looked) = state.look_at.and_then(|key| sensing::find(ai, key)) else {
            return false;
        };
        let reach = f64::from(self.look_distance * self.look_distance);
        looked.alive && looked.distance_sqr(ai.position()) <= reach && state.look_time > 0
    }

    fn start(&self, state: &mut LookingAt, ai: &mut Ai<'_, '_>) {
        let draw = ai.tick.draws.next_int(40) as i32;
        state.look_time = self.adjusted_tick_delay(40 + draw);
    }

    fn stop(&self, state: &mut LookingAt, _: &mut Ai<'_, '_>) {
        state.look_at = None;
    }

    fn tick(&self, state: &mut LookingAt, ai: &mut Ai<'_, '_>) {
        let Some(looked) = state.look_at.and_then(|key| sensing::find(ai, key)) else {
            return;
        };
        if looked.alive {
            let y = if self.only_horizontal {
                ai.eye_y()
            } else {
                looked.eye_y
            };
            controls::set_look_at_default(ai, [looked.position[0], y, looked.position[2]]);
            state.look_time -= 1;
        }
    }
}

/// The mob's box.
fn ai_bounds(ai: &Ai<'_, '_>) -> crate::physics::aabb::Aabb {
    crate::physics::aabb::Aabb::of_entity(ai.position(), ai.width(), ai.height())
}

/// What tempts a mob (an `Ingredient` of items).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Items {
    /// The items of an item tag.
    Tag(&'static str),
    /// These items, by id.
    Of(&'static [i32]),
}

impl Items {
    pub(crate) fn test(self, stack: Option<ItemStack>) -> bool {
        let Some(stack) = stack.filter(|stack| stack.count > 0) else {
            return false;
        };
        match self {
            Items::Tag(tag) => item_in_tag(stack.item, tag),
            Items::Of(items) => items.contains(&stack.item),
        }
    }
}

/// `TemptGoal`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Tempted {
    pub(crate) calm_down: i32,
    pub(crate) player: Option<EntityId>,
    pub(crate) noted: Vec3,
    pub(crate) noted_pitch: f64,
    pub(crate) noted_yaw: f64,
    pub(crate) running: bool,
}

/// `TemptGoal(mob, speed, items, canScare, stopDistance)`: follow a player holding
/// what it likes, within the tempt range, line of sight or not.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TemptGoal {
    pub(crate) speed: f64,
    pub(crate) items: Items,
    pub(crate) can_scare: bool,
    pub(crate) stop_distance: f64,
}

impl TemptGoal {
    pub(crate) fn new(speed: f64, items: Items, can_scare: bool) -> Self {
        Self {
            speed,
            items,
            can_scare,
            stop_distance: 2.5,
        }
    }
}

impl Goal for TemptGoal {
    type State = Tempted;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    fn can_use(&self, state: &mut Tempted, ai: &mut Ai<'_, '_>) -> bool {
        if state.calm_down > 0 {
            state.calm_down -= 1;
            return false;
        }
        let range = ai.attribute(attributes::TEMPT_RANGE);
        let conditions = Conditions::non_combat().ignore_line_of_sight().range(range);
        let items = self.items;
        let follows =
            move |looked: &Looked| items.test(looked.main_hand) || items.test(looked.off_hand);
        let from = ai.position();
        let found = sensing::nearest_player(ai, &conditions, &follows, from);
        state.player = found.map(|found| found.key);
        state.player.is_some()
    }

    fn can_continue(&self, state: &mut Tempted, ai: &mut Ai<'_, '_>) -> bool {
        if self.can_scare
            && let Some(player) = state.player.and_then(|key| sensing::find(ai, key))
        {
            if player.distance_sqr(ai.position()) < 36.0 {
                if player.distance_sqr(state.noted) > 0.010_000_000_000_000_002 {
                    return false;
                }
                if (f64::from(player.pitch) - state.noted_pitch).abs() > 5.0
                    || (f64::from(player.yaw) - state.noted_yaw).abs() > 5.0
                {
                    return false;
                }
            } else {
                state.noted = player.position;
            }
            state.noted_pitch = f64::from(player.pitch);
            state.noted_yaw = f64::from(player.yaw);
        }
        self.can_use(state, ai)
    }

    fn start(&self, state: &mut Tempted, ai: &mut Ai<'_, '_>) {
        if let Some(player) = state.player.and_then(|key| sensing::find(ai, key)) {
            state.noted = player.position;
        }
        state.running = true;
    }

    fn stop(&self, state: &mut Tempted, ai: &mut Ai<'_, '_>) {
        state.player = None;
        navigation::stop(ai);
        state.calm_down = reduced_tick_delay(100);
        state.running = false;
    }

    fn tick(&self, state: &mut Tempted, ai: &mut Ai<'_, '_>) {
        let Some(player) = state.player.and_then(|key| sensing::find(ai, key)) else {
            return;
        };
        let head = ai.head();
        look_at_thing(
            ai,
            &player,
            (head.max_y_rot + 20) as f32,
            head.max_x_rot as f32,
        );
        if player.distance_sqr(ai.position()) < self.stop_distance * self.stop_distance {
            navigation::stop(ai);
        } else {
            navigation::move_to_entity(ai, player.block_position(), self.speed);
        }
    }
}

/// `AvoidEntityGoal`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Avoiding {
    pub(crate) to_avoid: Option<EntityId>,
    pub(crate) path: HeldPath,
}

/// `AvoidEntityGoal(mob, class, maxDist, walkSpeed, sprintSpeed)`: run from the nearest
/// of a kind, walking, sprinting within seven blocks. A combat goal: no creative player
/// is ever avoided.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AvoidEntityGoal {
    pub(crate) class: Class,
    pub(crate) max_distance: f32,
    pub(crate) walk_speed: f64,
    pub(crate) sprint_speed: f64,
}

impl Goal for AvoidEntityGoal {
    type State = Avoiding;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, state: &mut Avoiding, ai: &mut Ai<'_, '_>) -> bool {
        let distance = f64::from(self.max_distance);
        let conditions = Conditions::combat().range(distance);
        // `NO_CREATIVE_OR_SPECTATOR`, and any thing.
        let selector = |looked: &Looked| !(looked.creative || looked.spectator);
        let bounds = ai_bounds(ai).inflate([distance, 3.0, distance]);
        let candidates = sensing::things_in(ai, bounds, self.class);
        let from = ai.position();
        let Some(threat) = sensing::nearest(ai, candidates, &conditions, &selector, from) else {
            return false;
        };
        state.to_avoid = Some(threat.key);
        let Some(away) = random_pos::default_pos_away(ai, 16, 7, threat.position) else {
            return false;
        };
        if threat.distance_sqr(away) < threat.distance_sqr(ai.position()) {
            return false;
        }
        let made = navigation::create_path_at(ai, away, 0);
        state.path = hold(ai, made);
        state.path.is_some()
    }

    fn can_continue(&self, _: &mut Avoiding, ai: &mut Ai<'_, '_>) -> bool {
        !navigation::is_done(ai)
    }

    fn start(&self, state: &mut Avoiding, ai: &mut Ai<'_, '_>) {
        let held = state.path.take();
        move_to_held(ai, held, self.walk_speed);
    }

    fn stop(&self, state: &mut Avoiding, _: &mut Ai<'_, '_>) {
        state.to_avoid = None;
    }

    fn tick(&self, state: &mut Avoiding, ai: &mut Ai<'_, '_>) {
        let Some(threat) = state.to_avoid.and_then(|key| sensing::find(ai, key)) else {
            return;
        };
        let speed = if threat.distance_sqr(ai.position()) < 49.0 {
            self.sprint_speed
        } else {
            self.walk_speed
        };
        navigation::set_speed_modifier(ai, speed);
    }
}

/// `MoveToBlockGoal`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct MovingToBlock {
    pub(crate) next_start_tick: i32,
    pub(crate) try_ticks: i32,
    pub(crate) max_stay_ticks: i32,
    pub(crate) block: Option<BlockPos>,
    pub(crate) reached: bool,
}

/// `MoveToBlockGoal(mob, speed, searchRange, verticalSearchRange)`: walk to the nearest
/// block that the subclass's `isValidTarget` takes, and stay there a while.
#[derive(Clone, Copy)]
pub(crate) struct MoveToBlockGoal {
    pub(crate) speed: f64,
    pub(crate) search_range: i32,
    pub(crate) vertical_search_range: i32,
    pub(crate) vertical_search_start: i32,
    /// `isValidTarget(level, pos)`.
    pub(crate) valid: fn(&Ai<'_, '_>, BlockPos) -> bool,
    /// `nextStartTick(mob)`, by default `reducedTickDelay(200 + nextInt(200))`.
    pub(crate) next_start: Option<fn(&mut Ai<'_, '_>) -> i32>,
}

impl MoveToBlockGoal {
    /// `findNearestBlock()`: by height 0, 1, -1, 2, … and outwards in rings.
    fn find_nearest(&self, state: &mut MovingToBlock, ai: &Ai<'_, '_>) -> bool {
        let at = ai.block_position();
        let next = |d: i32| if d > 0 { -d } else { 1 - d };
        let mut dy = self.vertical_search_start;
        while dy <= self.vertical_search_range {
            for i in 0..self.search_range {
                let mut dx = 0;
                while dx <= i {
                    let mut dz = if dx < i && dx > -i { i } else { 0 };
                    while dz <= i {
                        let pos = BlockPos::new(at.x + dx, at.y + dy - 1, at.z + dz);
                        if ai.within_home(pos) && (self.valid)(ai, pos) {
                            state.block = Some(pos);
                            return true;
                        }
                        dz = next(dz);
                    }
                    dx = next(dx);
                }
            }
            dy = next(dy);
        }
        false
    }

    fn move_to_block(&self, state: &MovingToBlock, ai: &mut Ai<'_, '_>) {
        let block = state.block.unwrap_or(BlockPos::new(0, 0, 0));
        navigation::move_to(
            ai,
            [
                f64::from(block.x) + 0.5,
                f64::from(block.y + 1),
                f64::from(block.z) + 0.5,
            ],
            self.speed,
        );
    }
}

impl Goal for MoveToBlockGoal {
    type State = MovingToBlock;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::JUMP
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) -> bool {
        if state.next_start_tick > 0 {
            state.next_start_tick -= 1;
            return false;
        }
        state.next_start_tick = match self.next_start {
            Some(next_start) => next_start(ai),
            None => reduced_tick_delay(200 + ai.tick.draws.next_int(200) as i32),
        };
        self.find_nearest(state, ai)
    }

    fn can_continue(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) -> bool {
        let block = state.block.unwrap_or(BlockPos::new(0, 0, 0));
        state.try_ticks >= -state.max_stay_ticks
            && state.try_ticks <= 1200
            && (self.valid)(ai, block)
    }

    fn start(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) {
        self.move_to_block(state, ai);
        state.try_ticks = 0;
        let inner = ai.tick.draws.next_int(1200) as i32;
        state.max_stay_ticks = ai.tick.draws.next_int((inner + 1200) as u32) as i32 + 1200;
    }

    fn tick(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) {
        let block = state.block.unwrap_or(BlockPos::new(0, 0, 0));
        let target = BlockPos::new(block.x, block.y + 1, block.z);
        let [x, y, z] = ai.position();
        let (dx, dy, dz) = (
            f64::from(target.x) + 0.5 - x,
            f64::from(target.y) + 0.5 - y,
            f64::from(target.z) + 0.5 - z,
        );
        // `closerToCenterThan(position, acceptedDistance())`, 1.0.
        if dx * dx + dy * dy + dz * dz >= 1.0 {
            state.reached = false;
            state.try_ticks += 1;
            if state.try_ticks % 40 == 0 {
                navigation::move_to(
                    ai,
                    [
                        f64::from(target.x) + 0.5,
                        f64::from(target.y),
                        f64::from(target.z) + 0.5,
                    ],
                    self.speed,
                );
            }
        } else {
            state.reached = true;
            state.try_ticks -= 1;
        }
    }
}

/// `MoveTowardsRestrictionGoal`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Returning {
    pub(crate) wanted: Vec3,
}

/// `MoveTowardsRestrictionGoal(mob, speed)`: back towards its home.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MoveTowardsRestrictionGoal {
    pub(crate) speed: f64,
}

impl Goal for MoveTowardsRestrictionGoal {
    type State = Returning;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, state: &mut Returning, ai: &mut Ai<'_, '_>) -> bool {
        let Some((home, _)) = ai.home() else {
            return false;
        };
        if ai.within_home(ai.block_position()) {
            return false;
        }
        let centre = [
            f64::from(home.x) + 0.5,
            f64::from(home.y),
            f64::from(home.z) + 0.5,
        ];
        match random_pos::default_pos_towards(ai, 16, 7, centre, std::f64::consts::FRAC_PI_2) {
            Some(wanted) => {
                state.wanted = wanted;
                true
            }
            None => false,
        }
    }

    fn can_continue(&self, _: &mut Returning, ai: &mut Ai<'_, '_>) -> bool {
        !navigation::is_done(ai)
    }

    fn start(&self, state: &mut Returning, ai: &mut Ai<'_, '_>) {
        navigation::move_to(ai, state.wanted, self.speed);
    }
}

/// `FollowMobGoal`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Following {
    pub(crate) following: Option<EntityId>,
    pub(crate) recalc: i32,
    pub(crate) old_water: Option<f32>,
}

/// `FollowMobGoal(mob, speed, stopDistance, areaSize)`: the parrot's, after mobs of
/// other kinds.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FollowMobGoal {
    pub(crate) speed: f64,
    pub(crate) stop_distance: f32,
    pub(crate) area_size: f32,
}

impl Goal for FollowMobGoal {
    type State = Following;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    fn can_use(&self, state: &mut Following, ai: &mut Ai<'_, '_>) -> bool {
        let area = f64::from(self.area_size);
        let bounds = ai_bounds(ai).inflate([area, area, area]);
        let kind = ai.tick.kind;
        // Of another class than its own; the first that is not invisible (none is).
        let found = sensing::things_in(ai, bounds, Class::Mob)
            .into_iter()
            .find(|looked| looked.kind != Some(kind));
        state.following = found.map(|found| found.key);
        state.following.is_some()
    }

    fn can_continue(&self, state: &mut Following, ai: &mut Ai<'_, '_>) -> bool {
        let Some(following) = state.following.and_then(|key| sensing::find(ai, key)) else {
            return false;
        };
        !navigation::is_done(ai)
            && following.distance_sqr(ai.position())
                > f64::from(self.stop_distance * self.stop_distance)
    }

    fn start(&self, state: &mut Following, ai: &mut Ai<'_, '_>) {
        state.recalc = 0;
        state.old_water = Some(ai.malus(PathType::Water));
        ai.set_malus(PathType::Water, 0.0);
    }

    fn stop(&self, state: &mut Following, ai: &mut Ai<'_, '_>) {
        state.following = None;
        navigation::stop(ai);
        if let Some(old) = state.old_water.take() {
            ai.set_malus(PathType::Water, old);
        }
    }

    fn tick(&self, state: &mut Following, ai: &mut Ai<'_, '_>) {
        // **Named**: a leashed follower stays put in the game; the leash is the body's.
        let Some(following) = state.following.and_then(|key| sensing::find(ai, key)) else {
            return;
        };
        let max_x = ai.head().max_x_rot as f32;
        look_at_thing(ai, &following, 10.0, max_x);
        state.recalc -= 1;
        if state.recalc > 0 {
            return;
        }
        state.recalc = self.adjusted_tick_delay(10);
        let [x, y, z] = ai.position();
        let (dx, dy, dz) = (
            x - following.position[0],
            y - following.position[1],
            z - following.position[2],
        );
        let distance = dx * dx + dy * dy + dz * dz;
        if distance > f64::from(self.stop_distance * self.stop_distance) {
            navigation::move_to_entity(ai, following.block_position(), self.speed);
        } else {
            navigation::stop(ai);
            // **Named**: the followed mob's look control is its mind's, not read here; a
            // follower steps away only when nearer than the stop distance.
            if distance <= f64::from(self.stop_distance) {
                let away = [
                    x - (following.position[0] - x),
                    y,
                    z - (following.position[2] - z),
                ];
                navigation::move_to(ai, away, self.speed);
            }
        }
    }
}

/// `TryFindLiquidGoal(mob, fluidTag)`: on dry ground, to the nearest of the fluid
/// around, straight.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TryFindLiquidGoal {
    pub(crate) fluid_tag: &'static str,
}

impl Goal for TryFindLiquidGoal {
    type State = ();

    fn flags(&self) -> u8 {
        0
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        ai.on_ground() && !fluid_in_tag(ai.tick.world.state(ai.block_position()), self.fluid_tag)
    }

    fn start(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        let [x, y, z] = ai.position();
        let block_y = ai.block_position().y;
        let world = ai.tick.world;
        let low = [
            physics::floor(x - 2.0),
            physics::floor(y - 2.0),
            physics::floor(z - 2.0),
        ];
        let high = [physics::floor(x + 2.0), block_y, physics::floor(z + 2.0)];
        for bz in low[2]..=high[2] {
            for by in low[1]..=high[1] {
                for bx in low[0]..=high[0] {
                    let pos = BlockPos::new(bx, by, bz);
                    if fluid_in_tag(world.state(pos), self.fluid_tag) {
                        controls::set_wanted_position(
                            ai,
                            [f64::from(bx), f64::from(by), f64::from(bz)],
                            1.0,
                        );
                        return;
                    }
                }
            }
        }
    }
}

/// `BreathAirGoal(mob)`: the dolphin's, to the air when its air runs low. **Named**:
/// its second move of the body in the tick (`moveRelative` and `move`) is the body's
/// physics, left to the dolphin's part.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct BreathAirGoal;

impl BreathAirGoal {
    fn find_air(ai: &mut Ai<'_, '_>) {
        let at = ai.block_position();
        let world = ai.tick.world;
        let gives_air = |pos: BlockPos| {
            let state = world.state(pos);
            (state.props().fluid().is_none() || is_class(state, BlockClass::BubbleColumnBlock))
                && state.is_pathfindable(clustine_data::PathComputation::Land)
        };
        // `BlockPos.neighborColumn`: at each height, the column and its four sides
        // (**Inferred** height first: the iterator is not in the dump).
        let mut found = None;
        'heights: for y in at.y..=at.y + 8 {
            for (dx, dz) in [(0, 0), (0, -1), (1, 0), (0, 1), (-1, 0)] {
                let pos = BlockPos::new(at.x + dx, y, at.z + dz);
                if gives_air(pos) {
                    found = Some(pos);
                    break 'heights;
                }
            }
        }
        let [x, y, z] = ai.position();
        let target = found.unwrap_or_else(|| physics::containing([x, y + 8.0, z]));
        navigation::move_to(
            ai,
            [
                f64::from(target.x),
                f64::from(target.y + 1),
                f64::from(target.z),
            ],
            1.0,
        );
    }
}

impl Goal for BreathAirGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    fn interruptable(&self, _: &()) -> bool {
        false
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        ai.tick.living.air < 140
    }

    fn start(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        navigation::stop(ai);
        Self::find_air(ai);
    }

    fn tick(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        Self::find_air(ai);
    }
}

/// The flags of a `JumpGoal`.
pub(crate) const JUMP_GOAL_FLAGS: u8 = flags::MOVE | flags::JUMP;

/// `RandomStandGoal`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Standing {
    /// `nextStand`, from `-getAmbientStandInterval()` up; none before the goal first
    /// asks (its constructor's value).
    pub(crate) next_stand: Option<i32>,
}

/// `RandomStandGoal(horse)`: a horse rears now and then. What standing does is the
/// horse's (`standIfPossible` and its sound), given as `stand`.
#[derive(Clone, Copy)]
pub(crate) struct RandomStandGoal {
    /// `getAmbientStandInterval()`.
    pub(crate) interval: i32,
    pub(crate) immobile: fn(&Ai<'_, '_>) -> bool,
    pub(crate) stand: fn(&mut Ai<'_, '_>),
}

impl Goal for RandomStandGoal {
    type State = Standing;

    fn flags(&self) -> u8 {
        0
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, state: &mut Standing, ai: &mut Ai<'_, '_>) -> bool {
        let next = state.next_stand.unwrap_or(-self.interval) + 1;
        state.next_stand = Some(next);
        if next > 0 && (ai.tick.draws.next_int(1000) as i32) < next {
            state.next_stand = Some(-self.interval);
            return !(self.immobile)(ai) && ai.tick.draws.next_int(10) == 0;
        }
        false
    }

    fn can_continue(&self, _: &mut Standing, _: &mut Ai<'_, '_>) -> bool {
        false
    }

    fn start(&self, _: &mut Standing, ai: &mut Ai<'_, '_>) {
        (self.stand)(ai);
    }
}

/// `ClimbOnTopOfPowderSnowGoal(mob, level)`: a mob that walks on powder snow jumps up out
/// of it.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ClimbOnTopOfPowderSnowGoal;

impl Goal for ClimbOnTopOfPowderSnowGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::JUMP
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        let living = &ai.tick.living;
        if !(living.was_in_powder_snow || living.in_powder_snow) {
            return false;
        }
        if !entity_in_tag(
            i32::from(ai.tick.kind),
            "minecraft:powder_snow_walkable_mobs",
        ) {
            return false;
        }
        let at = ai.block_position();
        let above = ai.tick.world.state(BlockPos::new(at.x, at.y + 1, at.z));
        is_class(above, BlockClass::PowderSnowBlock) || above.voxel_collision_shape().is_empty()
    }

    fn tick(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        controls::jump(ai);
    }
}

/// `TargetGoal`'s state, which its kinds extend.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Targeting {
    pub(crate) target_mob: Option<EntityId>,
    pub(crate) reach_cache: i32,
    pub(crate) reach_cache_time: i32,
    pub(crate) unseen_ticks: i32,
    pub(crate) unseen_memory_ticks: i32,
    /// `HurtByTargetGoal.timestamp`.
    pub(crate) timestamp: Option<u32>,
    /// `NearestAttackableTargetGoal.target`.
    pub(crate) found: Option<EntityId>,
}

/// `TargetGoal(mob, mustSee, mustReach)`: what its kinds share.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TargetRules {
    pub(crate) must_see: bool,
    pub(crate) must_reach: bool,
}

impl TargetRules {
    fn follow_distance(ai: &Ai<'_, '_>) -> f64 {
        ai.attribute(attributes::FOLLOW_RANGE)
    }

    /// `TargetGoal.canContinueToUse`.
    fn can_continue(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        let held = ai.tick.mob.body.combat.target.or(state.target_mob);
        let Some(target) = held.and_then(|key| sensing::find(ai, key)) else {
            return false;
        };
        if !sensing::can_attack(&target) {
            return false;
        }
        let follow = Self::follow_distance(ai);
        if target.distance_sqr(ai.position()) > follow * follow {
            return false;
        }
        if self.must_see {
            if sensing::has_line_of_sight(ai, &target) {
                state.unseen_ticks = 0;
            } else {
                state.unseen_ticks += 1;
                let memory = if state.unseen_memory_ticks == 0 {
                    60
                } else {
                    state.unseen_memory_ticks
                };
                if state.unseen_ticks > reduced_tick_delay(memory) {
                    return false;
                }
            }
        }
        sensing::set_target(ai, Some(&target));
        true
    }

    /// `TargetGoal.start`.
    fn start(state: &mut Targeting) {
        state.reach_cache = 0;
        state.reach_cache_time = 0;
        state.unseen_ticks = 0;
    }

    /// `TargetGoal.stop`.
    fn stop(state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        sensing::set_target(ai, None);
        state.target_mob = None;
    }

    /// `TargetGoal.canAttack(target, conditions)`.
    fn can_attack(
        &self,
        state: &mut Targeting,
        ai: &mut Ai<'_, '_>,
        target: Option<&Looked>,
        conditions: &Conditions,
        selector: &dyn Fn(&Looked) -> bool,
    ) -> bool {
        let Some(target) = target else {
            return false;
        };
        if !conditions.test(ai, target, selector) {
            return false;
        }
        if !ai.within_home(target.block_position()) {
            return false;
        }
        if self.must_reach {
            state.reach_cache_time -= 1;
            if state.reach_cache_time <= 0 {
                state.reach_cache = 0;
            }
            if state.reach_cache == 0 {
                state.reach_cache = if Self::can_reach(state, ai, target) {
                    1
                } else {
                    2
                };
            }
            if state.reach_cache == 2 {
                return false;
            }
        }
        true
    }

    /// `TargetGoal.canReach(target)`.
    fn can_reach(state: &mut Targeting, ai: &mut Ai<'_, '_>, target: &Looked) -> bool {
        state.reach_cache_time = reduced_tick_delay(10 + ai.tick.draws.next_int(5) as i32);
        let made = navigation::create_path(ai, target.block_position(), 0);
        let Some(made) = made else {
            return false;
        };
        let Some(end) = made.path(ai).and_then(|path| path.nodes.last().copied()) else {
            return false;
        };
        let at = target.block_position();
        let dx = end.x - at.x;
        let dz = end.z - at.z;
        f64::from(dx * dx + dz * dz) <= 2.25
    }
}

/// `HurtByTargetGoal(mob, ignoreDamageFrom...)`: take as target who last hurt it. A
/// creative player who hurt it is never taken (a combat test). **Named**: alerting
/// others writes their targets, an effect not built yet.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HurtByTargetGoal {
    /// The entity types whose hurts it ignores.
    pub(crate) ignore: &'static [u16],
}

impl Goal for HurtByTargetGoal {
    type State = Targeting;

    fn flags(&self) -> u8 {
        flags::TARGET
    }

    fn can_use(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        let Some((attacker, at)) = ai.tick.mob.body.combat.last_hurt_by else {
            return false;
        };
        if state.timestamp == Some(at) {
            return false;
        }
        let Some(attacker) = sensing::find(ai, attacker) else {
            return false;
        };
        // `universalAnger` is off, the game's default.
        if attacker
            .kind
            .is_some_and(|kind| self.ignore.contains(&kind))
        {
            return false;
        }
        let conditions = Conditions::combat()
            .ignore_line_of_sight()
            .ignore_invisibility();
        let rules = TargetRules {
            must_see: true,
            must_reach: false,
        };
        rules.can_attack(state, ai, Some(&attacker), &conditions, &|_| true)
    }

    fn can_continue(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        TargetRules {
            must_see: true,
            must_reach: false,
        }
        .can_continue(state, ai)
    }

    fn start(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        let attacker = ai
            .tick
            .mob
            .body
            .combat
            .last_hurt_by
            .and_then(|(key, at)| sensing::find(ai, key).map(|found| (found, at)));
        if let Some((attacker, at)) = attacker {
            sensing::set_target(ai, Some(&attacker));
            state.timestamp = Some(at);
        }
        state.target_mob = ai.tick.mob.body.combat.target;
        state.unseen_memory_ticks = 300;
        TargetRules::start(state);
    }

    fn stop(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        TargetRules::stop(state, ai);
    }
}

/// `NearestAttackableTargetGoal(mob, class, interval, mustSee, mustReach, selector)`.
#[derive(Clone, Copy)]
pub(crate) struct NearestAttackableTargetGoal {
    pub(crate) class: Class,
    pub(crate) interval: i32,
    pub(crate) must_see: bool,
    pub(crate) must_reach: bool,
    pub(crate) selector: Option<fn(&Looked) -> bool>,
}

impl NearestAttackableTargetGoal {
    pub(crate) fn new(class: Class, must_see: bool) -> Self {
        Self {
            class,
            interval: 10,
            must_see,
            must_reach: false,
            selector: None,
        }
    }

    fn rules(&self) -> TargetRules {
        TargetRules {
            must_see: self.must_see,
            must_reach: self.must_reach,
        }
    }
}

impl Goal for NearestAttackableTargetGoal {
    type State = Targeting;

    fn flags(&self) -> u8 {
        flags::TARGET
    }

    fn can_use(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        let interval = reduced_tick_delay(self.interval);
        if interval > 0 && ai.tick.draws.next_int(interval as u32) != 0 {
            return false;
        }
        // `findTarget()`, the range the follow range now.
        let follow = TargetRules::follow_distance(ai);
        let conditions = Conditions::combat().range(follow);
        let selector = self.selector;
        let selector = move |looked: &Looked| selector.is_none_or(|test| test(looked));
        let [x, _, z] = ai.position();
        let eyes = [x, ai.eye_y(), z];
        let found = if self.class == Class::Player {
            sensing::nearest_player(ai, &conditions, &selector, eyes)
        } else {
            let bounds = ai_bounds(ai).inflate([follow, follow, follow]);
            let candidates = sensing::things_in(ai, bounds, self.class);
            sensing::nearest(ai, candidates, &conditions, &selector, eyes)
        };
        state.found = found.map(|found| found.key);
        state.found.is_some()
    }

    fn can_continue(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        self.rules().can_continue(state, ai)
    }

    fn start(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        let found = state.found.and_then(|key| sensing::find(ai, key));
        sensing::set_target(ai, found.as_ref());
        TargetRules::start(state);
    }

    fn stop(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        TargetRules::stop(state, ai);
    }
}

/// `MeleeAttackGoal`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Attacking {
    pub(crate) path: HeldPath,
    pub(crate) pathed: Vec3,
    pub(crate) recalc: i32,
    pub(crate) attack_cooldown: i32,
    pub(crate) last_can_use_check: Option<i64>,
}

/// `MeleeAttackGoal(mob, speed, followingTargetEvenIfNotSeen)`: chase the target and hit
/// it within reach. **Named**: the hit itself (`swingForAttack`, `doHurtTarget`) is
/// the body's, written into another entity, and waits for it; the goal keeps its timing.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MeleeAttackGoal {
    pub(crate) speed: f64,
    pub(crate) following_even_if_not_seen: bool,
}

impl MeleeAttackGoal {
    /// `Mob.isWithinMeleeAttackRange(target)` with no item's reach: the mob's box grown
    /// by the default reach meets the target's.
    fn within_range(ai: &Ai<'_, '_>, target: &Looked) -> bool {
        let reach = f64::from(2.04_f32).sqrt() - f64::from(0.6_f32);
        ai_bounds(ai)
            .inflate([reach, 0.0, reach])
            .intersects(target.bounds)
    }
}

impl Goal for MeleeAttackGoal {
    type State = Attacking;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) -> bool {
        let now = ai.now();
        if state.last_can_use_check.is_some_and(|last| now - last < 20) {
            return false;
        }
        state.last_can_use_check = Some(now);
        let Some(target) = sensing::target(ai) else {
            return false;
        };
        if !target.alive {
            return false;
        }
        let made = navigation::create_path(ai, target.block_position(), 0);
        state.path = hold(ai, made);
        if state.path.is_some() {
            return true;
        }
        Self::within_range(ai, &target)
    }

    fn can_continue(&self, _: &mut Attacking, ai: &mut Ai<'_, '_>) -> bool {
        let Some(target) = sensing::target(ai) else {
            return false;
        };
        if !target.alive {
            return false;
        }
        if !self.following_even_if_not_seen {
            return !navigation::is_done(ai);
        }
        ai.within_home(target.block_position())
            && !(target.is_player() && (target.creative || target.spectator))
    }

    fn start(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) {
        let held = state.path.take();
        move_to_held(ai, held, self.speed);
        ai.tick.mob.body.combat.aggressive = true;
        state.recalc = 0;
        state.attack_cooldown = 0;
    }

    fn stop(&self, _: &mut Attacking, ai: &mut Ai<'_, '_>) {
        if let Some(target) = sensing::target(ai)
            && (target.creative || target.spectator)
        {
            sensing::set_target(ai, None);
        }
        ai.tick.mob.body.combat.aggressive = false;
        navigation::stop(ai);
    }

    fn tick(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) {
        let Some(target) = sensing::target(ai) else {
            return;
        };
        look_at_thing(ai, &target, 30.0, 30.0);
        state.recalc = (state.recalc - 1).max(0);
        let sees = self.following_even_if_not_seen || sensing::has_line_of_sight(ai, &target);
        if sees
            && state.recalc <= 0
            && (state.pathed == [0.0; 3]
                || target.distance_sqr(state.pathed) >= 1.0
                || ai.tick.draws.next_float() < 0.05)
        {
            state.pathed = target.position;
            state.recalc = 4 + ai.tick.draws.next_int(7) as i32;
            let distance = target.distance_sqr(ai.position());
            if distance > 1024.0 {
                state.recalc += 10;
            } else if distance > 256.0 {
                state.recalc += 5;
            }
            if !navigation::move_to_entity(ai, target.block_position(), self.speed) {
                state.recalc += 15;
            }
            state.recalc = self.adjusted_tick_delay(state.recalc);
        }
        state.attack_cooldown = (state.attack_cooldown - 1).max(0);
        // `checkAndPerformAttack`: the cooldown and its reset; the hit is the body's.
        if state.attack_cooldown == 0
            && Self::within_range(ai, &target)
            && sensing::has_line_of_sight(ai, &target)
        {
            state.attack_cooldown = self.adjusted_tick_delay(20);
        }
    }
}

/// `LeapAtTargetGoal(mob, yd)`: leap at a target two to four blocks away.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LeapAtTargetGoal {
    pub(crate) yd: f32,
}

impl Goal for LeapAtTargetGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::JUMP | flags::MOVE
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        let Some(target) = sensing::target(ai) else {
            return false;
        };
        let distance = target.distance_sqr(ai.position());
        if !(4.0..=16.0).contains(&distance) {
            return false;
        }
        if !ai.on_ground() {
            return false;
        }
        ai.tick.draws.next_int(reduced_tick_delay(5) as u32) == 0
    }

    fn can_continue(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        !ai.on_ground()
    }

    fn start(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        let Some(target) = sensing::target(ai) else {
            return;
        };
        let [x, _, z] = ai.position();
        let motion = ai.tick.living.body.motion;
        let mut leap = [target.position[0] - x, 0.0, target.position[2] - z];
        if physics::length_sqr(leap) > 1.0e-7 {
            leap = physics::add(
                physics::scale(physics::normalize(leap), 0.4),
                physics::scale(motion, 0.2),
            );
        }
        ai.tick.living.body.motion = [leap[0], f64::from(self.yd), leap[2]];
    }
}

/// `UseItemGoal`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct UsingItem {
    pub(crate) using: bool,
}

/// `UseItemGoal(mob, item, finishSound, predicate)`: the wandering trader's (N10).
/// **Named**: using an item is the body's, not built yet: the goal holds the item until
/// its next clean-up.
#[derive(Clone, Copy)]
pub(crate) struct UseItemGoal {
    pub(crate) item: ItemStack,
    pub(crate) finish_sound: Option<i32>,
    pub(crate) predicate: fn(&Ai<'_, '_>) -> bool,
}

impl Goal for UseItemGoal {
    type State = UsingItem;

    fn flags(&self) -> u8 {
        0
    }

    fn can_use(&self, _: &mut UsingItem, ai: &mut Ai<'_, '_>) -> bool {
        (self.predicate)(ai)
    }

    fn can_continue(&self, state: &mut UsingItem, _: &mut Ai<'_, '_>) -> bool {
        // `isUsingItem`: for the one selector tick it was started in.
        std::mem::take(&mut state.using)
    }

    fn start(&self, state: &mut UsingItem, ai: &mut Ai<'_, '_>) {
        ai.tick.mob.body.equipment.slots[0] = Some(self.item);
        state.using = true;
    }

    fn stop(&self, _: &mut UsingItem, ai: &mut Ai<'_, '_>) {
        ai.tick.mob.body.equipment.slots[0] = None;
        if let Some(sound) = self.finish_sound {
            let pitch = ai.tick.draws.next_float() * 0.2 + 0.9;
            let position = ai.position();
            ai.tick.living.sounds.push(crate::physics::item::Sound {
                sound,
                position,
                volume: 1.0,
                pitch,
            });
        }
    }
}
