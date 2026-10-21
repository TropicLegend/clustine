//! The happy ghast (`HappyGhast`, its controls and goals, `Ghast$GhastMoveControl`,
//! `Ghast$RandomFloatAroundGoal`, `TemptGoal$ForNonPathfinders`;
//! `docs/groundwork/n7-brief.md`, section 7.11): two mobs in one. A ghastling (a baby
//! of a quarter less than a fifth of the size) flies by a flying navigation and its
//! brain, is tempted and grown by snowballs, and breathes under water; grown up it
//! floats about its home by impulses (the ghast's move control), tempted by snowballs and
//! harnesses, and wears a harness that four players may ride, the first steering. A
//! player standing on top holds it still for ten ticks after, solid to stand on, its yaw
//! snapped to a quarter turn. It keeps a home of 32 or 64 blocks, heals a point every 600
//! ticks (every 20 in the clouds), and holds quad leashes, showing `IS_LEASH_HOLDER`
//! while a boat, a horse or a sniffer hangs on one.
//!
//! Built here (`docs/groundwork/n7j-plan.md`, sections 4.2 and 8 item 10): both ages'
//! controls and navigations, the grown one's three goals, its move control's careful
//! sweep and its look control, the still timeout and the scan for players on top, the
//! home, the healing, the harness put on (and taken off by shears, `region/animals.rs`),
//! the snowball's growing, the leash's distances and the leash holder's flag, sounds,
//! and its synced data. The ghastling's brain is part A's: until its provider comes a
//! ghastling has no goals and hovers where it is. Riding is the lead's (part R): the
//! functions of [`riding`] are what it needs, and nobody gets on yet.
//!
//! Named differences: the exact position packet as it begins to stay still and the
//! absolute moves while it stays (`setRequiresPrecisePosition`) are the edge's, which
//! the flag [`HappyGhastState::stays_still`] tells; the time between its impulses goes on
//! while its move control waits (the game's pauses), as a moment kept rather than a
//! counter counted; it is solid to stand on only once part R wires its collision
//! ([`riding::can_be_collided_with`]); its leash's swing is not stopped as it stays
//! still (`resetAngularLeashMomentum`); a baby's navigation moves directly where the
//! flying one's does, not by its own sweep (`BabyFlyingPathNavigation.canMoveDirectly`);
//! its ambient sound's interval is not six times longer while ridden (nobody rides yet);
//! weather is always clear, so only the clouds heal it fast.

use clustine_blocks::data::{in_tag, item_in_tag};
use clustine_blocks::fluid::{FluidState, Kind as FluidKind};
use clustine_data::{EntityTypeInfo, entity_types, sounds};
use clustine_world::{BlockPos, ItemStack};
use serde::{Deserialize, Serialize};

use super::animal::{self, Own};
use super::{Breeding, Did, Dimensions, Interaction, MobType, Used};
use crate::living::{Travel, slots};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{FloatGoal, Items, Tempted, look_at_thing};
use crate::mobs::ai::maths::{self, reduced_tick_delay, wrap_degrees};
use crate::mobs::ai::navigation::{self, Kind, Settings};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::{self, Conditions, Looked};
use crate::mobs::ai::{Ai, MoveKind, Operation, Setup, controls};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::MobTick;
use crate::physics::aabb::Aabb;
use crate::physics::collide::{self, Context};
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::physics::{self, Vec3, walk};
use crate::shown::DataShown;

/// The synced data's indices: whether it holds a quad leash, and whether it stays still.
pub(crate) const IS_LEASH_HOLDER_INDEX: u8 = 18;
pub(crate) const STAYS_STILL_INDEX: u8 = 19;

/// `BABY_SCALE` and the baby's eye height.
pub(crate) const BABY_SCALE: f32 = 0.2375;
const BABY_EYE_HEIGHT: f32 = 0.468_75;

/// `SMALL_RESTRICTION_RADIUS`, `LARGE_RESTRICTION_RADIUS`, `RESTRICTION_RADIUS_BUFFER`.
const SMALL_HOME: i32 = 32;
const LARGE_HOME: i32 = 64;
const HOME_BUFFER: i32 = 16;

/// `FAST_HEALING_TICKS` and `SLOW_HEALING_TICKS`.
const FAST_HEALING: u32 = 20;
const SLOW_HEALING: u32 = 600;

/// `MAX_PASSANGERS`.
pub(crate) const MAX_PASSENGERS: usize = 4;

/// `STILL_TIMEOUT_ON_LOAD_GRACE_PERIOD` and `MAX_STILL_TIMEOUT`.
const STILL_GRACE: u32 = 60;
pub(crate) const MAX_STILL_TIMEOUT: i32 = 10;

/// `notifyLeashHolder`'s time a quad leashee keeps it a holder.
pub(crate) const LEASH_HOLDER_TIME: i32 = 5;

/// How far `Ghast$RandomFloatAroundGoal` looks for a block about its target, and how
/// many targets it tries.
const WANDER_DISTANCE: i32 = 16;
const WANDER_ATTEMPTS: usize = 64;

/// How near a tempting player it stops (`TemptGoal`'s stop distance).
const TEMPT_STOP: f64 = 7.0;

/// The overworld's cloud height (`minecraft:visual/cloud_height`), whose clouds are never
/// clear.
const CLOUD_HEIGHT: f32 = 192.33;

/// `#happy_ghast_food` and `#happy_ghast_tempt_items`.
pub(crate) const FOOD: Items = Items::Tag("minecraft:happy_ghast_food");
const TEMPT_ITEMS: Items = Items::Tag("minecraft:happy_ghast_tempt_items");

/// What a happy ghast keeps beyond an animal: its `serverStillTimeout` (saved by the
/// game), its synced `STAYS_STILL` and `IS_LEASH_HOLDER`, and the moment of its move
/// control's next impulse (`floatDuration`'s).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct HappyGhastState {
    pub(crate) still_timeout: i32,
    pub(crate) stays_still: bool,
    pub(crate) leash_holder: bool,
    pub(crate) impulse_at: u32,
}

impl Own for HappyGhastState {}

impl HappyGhastState {
    /// `isOnStillTimeout()`.
    pub(crate) fn on_still_timeout(&self) -> bool {
        self.stays_still || self.still_timeout > 0
    }

    /// `setServerStillTimeout(n)`: the timeout and its flag; whether it began to stay
    /// still, as the exact position packet is sent then.
    pub(crate) fn set_server_still_timeout(&mut self, timeout: i32) -> bool {
        let began = self.still_timeout <= 0 && timeout > 0;
        self.still_timeout = timeout;
        self.stays_still = timeout > 0;
        began
    }
}

/// A happy ghast's own state.
pub(crate) fn read(mob: &Mob) -> HappyGhastState {
    animal::read::<HappyGhastState>(&mob.state).1
}

/// Changes a happy ghast's own state.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut HappyGhastState)) {
    let (shared, mut own) = animal::read::<HappyGhastState>(&mob.state);
    let before = own.clone();
    change(&mut own);
    if own != before {
        mob.state = animal::write(&shared, &own);
    }
}

/// Whether it wears a harness (`isWearingBodyArmor`: its body's slot holds one).
pub(crate) fn harnessed(mob: &Mob) -> bool {
    mob.body.equipment.slots[usize::from(slots::BODY)]
        .is_some_and(|stack| stack.count > 0 && item_in_tag(stack.item, "minecraft:harnesses"))
}

/// `getHappyGhastRestrictionRadius()`: 64 for a grown one with nothing on, else 32.
fn restriction_radius(mob: &Mob) -> i32 {
    let bare = mob.body.equipment.slots[usize::from(slots::BODY)].is_none();
    if !animal::is_baby(mob) && bare {
        LARGE_HOME
    } else {
        SMALL_HOME
    }
}

/// `checkRestriction()`: not while leashed or ridden, its home kept where it is within
/// its radius and 16 of its block and of its radius, else made again at its block.
fn check_restriction(tick: &mut MobTick<'_>) {
    if tick.leashed || tick.carries {
        return;
    }
    let radius = restriction_radius(&tick.mob);
    let at = physics::containing(tick.living.body.position);
    let home = &tick.mob.body.home;
    if let Some(centre) = home.centre
        && home.radius == radius
    {
        let reach = f64::from(radius + HOME_BUFFER);
        let (dx, dy, dz) = (
            f64::from(centre.x - at.x),
            f64::from(centre.y - at.y),
            f64::from(centre.z - at.z),
        );
        if dx * dx + dy * dy + dz * dz < reach * reach {
            return;
        }
    }
    tick.mob.body.home = crate::mobs::body::Home {
        centre: Some(at),
        radius,
    };
}

/// `isInClouds()` in the overworld: its top at the clouds' height or above and its feet
/// at most four above it.
pub(crate) fn in_clouds(y: f64, height: f32) -> bool {
    y + f64::from(height) >= f64::from(CLOUD_HEIGHT) && f64::from(CLOUD_HEIGHT + 4.0) >= y
}

/// `continuousHeal()`: a living one short of its health heals a point every 20 ticks in
/// the clouds (or where it rains, which it never does), else every 600.
fn continuous_heal(tick: &mut MobTick<'_>) {
    if tick.mob.body.vitals.dying() || !super::tamable::hurt(&tick.mob, tick.kind) {
        return;
    }
    let body = &tick.living.body;
    let every = if in_clouds(body.position[1], body.height) {
        FAST_HEALING
    } else {
        SLOW_HEALING
    };
    if tick.tick_count.wrapping_add(1).is_multiple_of(every) {
        super::tamable::heal(&mut tick.mob, tick.kind, 1.0);
    }
}

/// `scanPlayerAboveGhast()`: a player of the world but a spectator, not riding a happy
/// ghast, whose root vehicle stands in the slab above its top (a block wider each way,
/// half its height high).
fn scan_player_above(tick: &MobTick<'_>) -> bool {
    let body = &tick.living.body;
    let bounds = Aabb::of_entity(body.position, body.width, body.height);
    let size = f64::from(body.height);
    let above = Aabb::new(
        [
            bounds.min[0] - 1.0,
            bounds.max[1] - 1.0e-5,
            bounds.min[2] - 1.0,
        ],
        [
            bounds.max[0] + 1.0,
            bounds.max[1] + size / 2.0,
            bounds.max[2] + 1.0,
        ],
    );
    tick.world.players().into_iter().any(|player| {
        if player.spectator {
            return false;
        }
        let mut root = player.position;
        let mut vehicle = player.vehicle;
        for _ in 0..8 {
            let Some(key) = vehicle else {
                break;
            };
            let Some(seen) = tick.world.entity(key) else {
                break;
            };
            if i32::from(seen.entity.kind) == entity_types::HAPPY_GHAST {
                return false;
            }
            root = seen.entity.position;
            vehicle = seen.entity.riding.vehicle;
        }
        above.contains(root)
    })
}

/// `ageBoundaryReached()` of a ghastling grown up (`adultGhastSetup`): the ghast's move
/// and look controls and a new navigation, so nothing it steered towards is kept.
fn grown_up(mob: &mut Mob) {
    mob.body.controls.movement = Default::default();
    mob.body.controls.look = Default::default();
    mob.body.path = Default::default();
}

/// `HappyGhastFloatGoal`: the float goal, but not while it stays still.
#[derive(Debug, Clone, Copy)]
struct HappyGhastFloatGoal;

impl Goal for HappyGhastFloatGoal {
    type State = ();

    fn flags(&self) -> u8 {
        FloatGoal.flags()
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn on_register(&self, config: &mut crate::mobs::ai::Config) {
        FloatGoal.on_register(config);
    }

    fn can_use(&self, state: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        !read(&ai.tick.mob).on_still_timeout() && FloatGoal.can_use(state, ai)
    }

    fn tick(&self, state: &mut (), ai: &mut Ai<'_, '_>) {
        FloatGoal.tick(state, ai);
    }
}

/// `TemptGoal$ForNonPathfinders(ghast, 1.0, items, false, 7.0)`: a tempt that steers its
/// move control towards a random point between it and the player's eyes (one draw of its
/// own each time), and stops it by making it wait. A harnessed one is tempted by food
/// alone, a bare one by food and harnesses.
#[derive(Debug, Clone, Copy)]
struct GhastTemptGoal;

impl GhastTemptGoal {
    fn items(mob: &Mob) -> Items {
        if harnessed(mob) || animal::is_baby(mob) {
            FOOD
        } else {
            TEMPT_ITEMS
        }
    }

    /// `navigateTowards(player)`.
    fn navigate_towards(ai: &mut Ai<'_, '_>, player: &Looked) {
        let at = ai.position();
        let eyes = [player.position[0], player.eye_y, player.position[2]];
        let part = ai.tick.draws.next_double();
        let wanted = physics::add(physics::scale(physics::sub(eyes, at), part), at);
        controls::set_wanted_position(ai, wanted, 1.0);
    }
}

impl Goal for GhastTemptGoal {
    type State = Tempted;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    fn can_use(&self, state: &mut Tempted, ai: &mut Ai<'_, '_>) -> bool {
        if state.calm_down > 0 {
            state.calm_down -= 1;
            return false;
        }
        let range = ai.attribute(clustine_data::attributes::TEMPT_RANGE);
        let conditions = Conditions::non_combat().ignore_line_of_sight().range(range);
        let items = Self::items(&ai.tick.mob);
        let follows =
            move |looked: &Looked| items.test(looked.main_hand) || items.test(looked.off_hand);
        let from = ai.position();
        let found = sensing::nearest_player(ai, &conditions, &follows, from);
        state.player = found.map(|found| found.key);
        state.player.is_some()
    }

    fn start(&self, state: &mut Tempted, ai: &mut Ai<'_, '_>) {
        if let Some(player) = state.player.and_then(|key| sensing::find(ai, key)) {
            state.noted = player.position;
        }
        state.running = true;
    }

    fn stop(&self, state: &mut Tempted, ai: &mut Ai<'_, '_>) {
        state.player = None;
        controls::set_wait(ai);
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
        if player.distance_sqr(ai.position()) < TEMPT_STOP * TEMPT_STOP {
            controls::set_wait(ai);
        } else {
            Self::navigate_towards(ai, &player);
        }
    }
}

/// `Ghast$RandomFloatAroundGoal(ghast, 16)`: where its move control steers nowhere, or
/// to a point nearer than a block or further than 60, a new point to float to, for one
/// tick.
#[derive(Debug, Clone, Copy)]
struct RandomFloatAroundGoal;

impl RandomFloatAroundGoal {
    /// `chooseRandomPosition(pos, random)`: up to 16 each way, x, then y, then z.
    fn random_position(ai: &mut Ai<'_, '_>, from: Vec3) -> Vec3 {
        let mut part = || f64::from((ai.tick.draws.next_float() * 2.0 - 1.0) * 16.0);
        let x = from[0] + part();
        let y = from[1] + part();
        let z = from[2] + part();
        [x, y, z]
    }

    /// `isGoodTarget(level, v, d)`: in air, with a block that is not air within `d - 1`
    /// along some axis (down, up, north, south, west, east).
    fn good_target(ai: &Ai<'_, '_>, target: Vec3, distance: i32) -> bool {
        if distance <= 0 {
            return true;
        }
        let at = physics::containing(target);
        let world = ai.tick.world;
        if !world.state(at).props().is_air() {
            return false;
        }
        let directions = [
            (0, -1, 0),
            (0, 1, 0),
            (0, 0, -1),
            (0, 0, 1),
            (-1, 0, 0),
            (1, 0, 0),
        ];
        directions.into_iter().any(|(dx, dy, dz)| {
            (1..distance).any(|i| {
                !world
                    .state(at.offset(dx * i, dy * i, dz * i))
                    .props()
                    .is_air()
            })
        })
    }

    /// `getSuitableFlyToPosition(ghast, 16)`.
    fn suitable_position(ai: &mut Ai<'_, '_>) -> Vec3 {
        let from = ai.position();
        let mut last = None;
        for _ in 0..WANDER_ATTEMPTS {
            let chosen = Self::random_position(ai, from);
            // `chooseRandomPositionWithRestriction`: none outside its home.
            last = (ai.home().is_none() || ai.within_home(physics::containing(chosen)))
                .then_some(chosen);
            if let Some(found) = last
                && Self::good_target(ai, found, WANDER_DISTANCE)
            {
                return found;
            }
        }
        let mut chosen = match last {
            Some(found) => found,
            None => Self::random_position(ai, from),
        };
        let at = physics::containing(chosen);
        if let Some(height) = ai.tick.world.motion_blocking_height(at.x, at.z)
            && height < at.y
            && height > ai.tick.world.min_y()
        {
            let y = ai.position()[1];
            chosen = [chosen[0], y - (y - chosen[1]).abs(), chosen[2]];
        }
        chosen
    }
}

impl Goal for RandomFloatAroundGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        let movement = &ai.tick.mob.body.controls.movement;
        if movement.operation != Operation::MoveTo {
            return true;
        }
        let at = ai.position();
        let distance = physics::distance_sqr(movement.wanted, at);
        !(1.0..=3600.0).contains(&distance)
    }

    fn can_continue(&self, _: &mut (), _: &mut Ai<'_, '_>) -> bool {
        false
    }

    fn start(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        let wanted = Self::suitable_position(ai);
        controls::set_wanted_position(ai, wanted, 1.0);
    }
}

/// `Ghast$GhastMoveControl.blockTraversalPossible(level, from, to, pos, inWater,
/// inLava)` of a careful one: air passes; a block it avoids does not; a fluid it meets
/// passes only where it is in that fluid already; else whether its shape is in the way
/// (moving from `from` to `to`, or at all where there is no move).
fn traversal_possible(
    ai: &Ai<'_, '_>,
    sweep: Option<(Vec3, Vec3)>,
    pos: BlockPos,
    in_water: bool,
    in_lava: bool,
) -> bool {
    let world = ai.tick.world;
    let state = world.state(pos);
    if state.props().is_air() {
        return true;
    }
    let boxes = collide::collision_shape(state, pos, NO_ENTITY)
        .map(|(shape, _)| shape.boxes())
        .unwrap_or_default();
    let passable = match sweep {
        Some((from, to)) => {
            let moving = Aabb::of_entity(from, ai.width(), ai.height());
            !moving.collided_along_vector(physics::sub(to, from), &boxes)
        }
        None => boxes.is_empty(),
    };
    if in_tag(state, "minecraft:happy_ghast_avoids") {
        return false;
    }
    let fluid = FluidState::of(state);
    if let Some(kind) = fluid.kind() {
        let met = match sweep {
            None => true,
            Some((from, to)) => {
                let height = clustine_blocks::fluid::height(&mut physics::Rules(world), fluid, pos);
                let base = Aabb::of_block(pos);
                let fluid_box = Aabb::new(
                    base.min,
                    [base.max[0], f64::from(pos.y as f32 + height), base.max[2]],
                );
                let moving = Aabb::of_entity(from, ai.width(), ai.height());
                moving.collided_along_vector(physics::sub(to, from), &[fluid_box])
            }
        };
        if met {
            return match kind {
                FluidKind::Water => in_water,
                FluidKind::Lava => in_lava,
            };
        }
    }
    passable
}

/// `CollisionContext.empty()`, which `getCollisionShape(level, pos)` uses.
const NO_ENTITY: Context = Context {
    bottom: -f64::MAX,
    fall_distance: 0.0,
    falling_block: false,
    noclip: None,
};

/// `Ghast$GhastMoveControl.canReach(delta)` of a careful one: every block about its box
/// moved there (grown by one) passes, and so does every block its box sweeps through on
/// the way that it is not in already.
fn can_reach(ai: &Ai<'_, '_>, delta: Vec3) -> bool {
    let position = ai.position();
    let current = Aabb::of_entity(position, ai.width(), ai.height());
    let moved = current.moved(delta);
    let grown = moved.inflate([1.0; 3]);
    let low = physics::containing(grown.min);
    let high = physics::containing(grown.max);
    for z in low.z..=high.z {
        for y in low.y..=high.y {
            for x in low.x..=high.x {
                if !traversal_possible(ai, None, BlockPos::new(x, y, z), false, false) {
                    return false;
                }
            }
        }
    }
    let (in_water, in_lava) = (ai.in_water(), ai.in_lava());
    let to = physics::add(position, delta);
    walk::for_each_block_between(position, to, &moved, |pos, _| {
        current.intersects(Aabb::of_block(pos))
            || traversal_possible(ai, Some((position, to)), pos, in_water, in_lava)
    })
}

/// `Ghast$GhastMoveControl.tick()` with `careful` and the still timeout: still, it waits
/// and stops in place; moving to a point, every 2 to 6 ticks (one draw of its own) an
/// impulse of its flying speed's five thirds towards it where it can reach it, else it
/// waits.
fn tick_ghast_move(ai: &mut Ai<'_, '_>) {
    let own = read(&ai.tick.mob);
    if own.on_still_timeout() {
        ai.tick.mob.body.controls.movement.operation = Operation::Wait;
        // `Mob.stopInPlace()`.
        navigation::stop(ai);
        let inputs = &mut ai.tick.mob.body.inputs;
        inputs.xxa = 0.0;
        inputs.yya = 0.0;
        inputs.speed = 0.0;
        inputs.zza = 0.0;
        ai.tick.living.body.motion = [0.0; 3];
    }
    let movement = ai.tick.mob.body.controls.movement.clone();
    if movement.operation != Operation::MoveTo {
        return;
    }
    let now = ai.tick.lived;
    if now < own.impulse_at {
        return;
    }
    let wait = ai.tick.draws.next_int(5) + 2;
    update(&mut ai.tick.mob, |own| own.impulse_at = now + wait);
    let delta = physics::sub(movement.wanted, ai.position());
    if can_reach(ai, delta) {
        let speed = ai.attribute(clustine_data::attributes::FLYING_SPEED) * 5.0 / 3.0;
        let push = physics::scale(physics::normalize(delta), speed);
        let body = &mut ai.tick.living.body;
        body.motion = physics::add(body.motion, push);
    } else {
        ai.tick.mob.body.controls.movement.operation = Operation::Wait;
    }
}

/// `HappyGhastLookControl.tick()`: still, its yaw snapped to a quarter turn; looking at
/// something, its yaw, body and head towards it; else facing the way it drifts
/// (`Ghast.faceMovementDirection`, with no target).
fn tick_ghast_look(ai: &mut Ai<'_, '_>) {
    if read(&ai.tick.mob).on_still_timeout() {
        let yaw = ai.tick.yaw;
        ai.tick.yaw = yaw - wrap_degrees_90(yaw);
        ai.tick.mob.body.head.head_yaw = ai.tick.yaw;
        return;
    }
    let look = &mut ai.tick.mob.body.controls.look;
    if look.cooldown > 0 {
        look.cooldown -= 1;
        let wanted = look.wanted;
        let [x, _, z] = ai.position();
        let yaw = -(maths::atan2(wanted[0] - x, wanted[2] - z) as f32) * 57.295_776;
        ai.tick.yaw = yaw;
        ai.tick.mob.body.head.body_yaw = yaw;
        ai.tick.mob.body.head.head_yaw = yaw;
        return;
    }
    let motion = ai.tick.living.body.motion;
    let yaw = -(maths::atan2(motion[0], motion[2]) as f32) * 57.295_776;
    ai.tick.yaw = yaw;
    ai.tick.mob.body.head.body_yaw = yaw;
}

/// `Mth.wrapDegrees90(degrees)`.
pub(crate) fn wrap_degrees_90(degrees: f32) -> f32 {
    let mut wrapped = degrees % 90.0;
    if wrapped >= 45.0 {
        wrapped -= 90.0;
    }
    if wrapped < -45.0 {
        wrapped += 90.0;
    }
    wrapped
}

/// `Equippable.equipOnTarget` of a harness on a grown, living happy ghast that wears
/// none: put on, kept to drop whatever kills it, with its equipping sound (the seed one
/// draw of its own) and `GameEvent.EQUIP`. The creative player keeps their harness.
fn equip_harness(used: &mut Used<'_>) -> Interaction {
    let body = usize::from(slots::BODY);
    let Some(stack) = used.held.filter(|stack| stack.count > 0) else {
        return Interaction::Pass;
    };
    let fits = clustine_data::ItemComponents::of(stack.item)
        .equippable
        .is_some_and(|equippable| usize::from(equippable.slot) == body)
        && clustine_blocks::data::entity_in_tag(
            i32::from(used.kind),
            "minecraft:can_equip_harness",
        )
        && !used.mob.body.vitals.dying()
        && !animal::is_baby(used.mob);
    if !fits || used.mob.body.equipment.slots[body].is_some() {
        return Interaction::Pass;
    }
    used.mob.body.equipment.slots[body] = Some(ItemStack { count: 1, ..stack });
    let mut chances = used.mob.body.equipment.drop_chances.unwrap_or([0.085; 8]);
    chances[body] = 2.0;
    used.mob.body.equipment.drop_chances = Some(chances);
    let _seed = used.draws.next_u64();
    used.did.push(Did::Sound {
        sound: sounds::ENTITY_HAPPY_GHAST_EQUIP,
        source: SOUND_SOURCE_NEUTRAL,
        at: used.position,
        volume: 1.0,
        pitch: 1.0,
        but_the_player: false,
    });
    used.did.push(Did::GameEvent("minecraft:equip"));
    Interaction::Success
}

/// What riding a happy ghast needs (`HappyGhast`, the brief's 7.11.4), for the lead's
/// part R to wire: nobody gets on yet.
#[allow(dead_code, reason = "what part R's riding of the happy ghast calls")]
pub(crate) mod riding {
    use super::*;

    /// `canAddPassenger`: fewer than four.
    pub(crate) fn can_add_passenger(passengers: usize) -> bool {
        passengers < MAX_PASSENGERS
    }

    /// `getControllingPassenger()`'s test: the first passenger steers where it is a
    /// player and the ghast wears a harness and does not stay still.
    pub(crate) fn steered(mob: &Mob, first_is_player: bool) -> bool {
        first_is_player && harnessed(mob) && !read(mob).on_still_timeout()
    }

    /// Its four seats (`PASSENGER` attachments, the type's), in order.
    pub(crate) fn seats() -> &'static [[f64; 3]] {
        EntityTypeInfo::of(entity_types::HAPPY_GHAST).passenger_points
    }

    /// `getRiddenInput(player, input)`: sideways as the player strafes, ahead or back
    /// (half as fast) along the player's pitch whatever the key's size, half a block a
    /// tick more up while they jump, all times 3.9 of its flying speed.
    pub(crate) fn ridden_input(
        xxa: f32,
        zza: f32,
        pitch: f32,
        jumping: bool,
        flying_speed: f64,
    ) -> Vec3 {
        let x = f64::from(xxa);
        let mut y = 0.0_f64;
        let mut z = 0.0_f64;
        if zza != 0.0 {
            let mut c = maths::table_cos(pitch * maths::RADIANS);
            let mut s = -maths::table_sin(pitch * maths::RADIANS);
            if zza < 0.0 {
                c *= -0.5;
                s *= -0.5;
            }
            y = f64::from(s);
            z = f64::from(c);
        }
        if jumping {
            y += 0.5;
        }
        physics::scale([x, y, z], 3.900_000_095_367_431_6 * flying_speed)
    }

    /// `getRiddenRotation(rider)`: half the rider's pitch, and their yaw.
    pub(crate) fn ridden_rotation(rider_pitch: f32, rider_yaw: f32) -> (f32, f32) {
        (rider_pitch * 0.5, rider_yaw)
    }

    /// `tickRidden`'s turn: its yaw towards the rider's by 0.08 of the way; its body
    /// and head follow (`yRotO = yBodyRot = yHeadRot = yaw`), its pitch half the rider's.
    pub(crate) fn ridden_yaw(yaw: f32, rider_yaw: f32) -> f32 {
        yaw + wrap_degrees(rider_yaw - yaw) * 0.08
    }

    /// `addPassenger`'s still timeout: none where nobody stands on top, else at most 10;
    /// and the goggles' sound where it carried nobody.
    pub(crate) fn passenger_added(
        mob: &mut Mob,
        had_riders: bool,
        player_above: bool,
    ) -> Option<i32> {
        update(mob, |own| {
            if !player_above {
                own.set_server_still_timeout(0);
            } else if own.still_timeout > MAX_STILL_TIMEOUT {
                own.set_server_still_timeout(MAX_STILL_TIMEOUT);
            }
        });
        (!had_riders).then_some(sounds::ENTITY_HAPPY_GHAST_HARNESS_GOGGLES_DOWN)
    }

    /// `removePassenger`: still for 10 ticks, and where nobody is left, no home and the
    /// goggles' sound.
    pub(crate) fn passenger_removed(mob: &mut Mob, riders_left: usize) -> Option<i32> {
        update(mob, |own| {
            own.set_server_still_timeout(MAX_STILL_TIMEOUT);
        });
        if riders_left > 0 {
            return None;
        }
        mob.body.home = Default::default();
        Some(sounds::ENTITY_HAPPY_GHAST_HARNESS_GOGGLES_UP)
    }

    /// `canBeCollidedWith(other)` on the server: never a baby or a dying one; another
    /// happy ghast where it carries riders; else while it stays still.
    pub(crate) fn can_be_collided_with(
        mob: &Mob,
        carries: bool,
        other_is_happy_ghast: bool,
    ) -> bool {
        if animal::is_baby(mob) || mob.body.vitals.dying() {
            return false;
        }
        if carries && other_is_happy_ghast {
            return true;
        }
        read(mob).on_still_timeout()
    }

    /// `isFlyingVehicle()`: a grown one.
    pub(crate) fn flying_vehicle(mob: &Mob) -> bool {
        !animal::is_baby(mob)
    }

    /// `getDismountLocationForPassenger`: on its top, at its middle.
    pub(crate) fn dismount_at(position: Vec3, height: f32) -> Vec3 {
        [position[0], position[1] + f64::from(height), position[2]]
    }
}

/// The happy ghast.
pub(crate) struct HappyGhast;

impl MobType for HappyGhast {
    /// The ghastling's `babyGhastSetup` (a flying navigation that opens no doors, floats
    /// and plans 48, a flying move control turning 180 that hovers, no goals: its mind
    /// is its brain's) or the grown one's `registerGoals` and ground navigation, whose
    /// move and look controls are its own ([`MobType::tick_move_control`]).
    fn register_goals_of(&self, mob: &Mob, setup: &mut Setup) {
        super::cow::animal_maluses(setup);
        if animal::is_baby(mob) {
            setup.config.navigation = Settings::of(Kind::Flying);
            setup.config.navigation.flags.can_open_doors = false;
            setup.config.navigation.set_can_float(true);
            setup.config.navigation.set_required_path_length(48.0);
            setup.config.move_kind = MoveKind::Flying {
                max_turn: 180,
                hovers: true,
            };
            return;
        }
        setup.add_goal(3, HappyGhastFloatGoal);
        setup.add_goal(4, GhastTemptGoal);
        setup.add_goal(5, RandomFloatAroundGoal);
    }

    fn tick_move_control(&self, ai: &mut Ai<'_, '_>) {
        if animal::is_baby(&ai.tick.mob) {
            controls::tick_move(ai);
        } else {
            tick_ghast_move(ai);
        }
    }

    fn tick_look_control(&self, ai: &mut Ai<'_, '_>) {
        if animal::is_baby(&ai.tick.mob) {
            controls::tick_look(ai);
        } else {
            tick_ghast_look(ai);
        }
    }

    /// `getWalkTargetValue(pos)`: nothing in a block that is not empty; ten for a hover
    /// a block above the ground; five elsewhere.
    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        let world = ai.tick.world;
        if !world.state(pos).props().is_air() {
            return 0.0;
        }
        let below = world.state(pos.offset(0, -1, 0)).props().is_air();
        let ground = !world.state(pos.offset(0, -2, 0)).props().is_air();
        if below && ground { 10.0 } else { 5.0 }
    }

    /// `customServerAiStep`: a ghastling's brain and activity (part A's, not wired yet),
    /// its home checked at every age, then the animal's.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        check_restriction(tick);
        animal::custom_server_ai_step(tick);
    }

    /// `travel(input)`: `travelFlying` at five thirds of its flying speed (as floats)
    /// in the air, in water and in lava alike.
    fn travel(&self, tick: &MobTick<'_>) -> Travel {
        let flying =
            crate::mobs::ai::attribute_value(tick, clustine_data::attributes::FLYING_SPEED);
        let speed = flying as f32 * 5.0 / 3.0;
        Travel::Flying {
            water: speed,
            lava: speed,
            air: speed,
        }
    }

    /// `aiStep` after the animal's (its growing up, where the ghast's controls come)
    /// and `continuousHeal`.
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        let was_baby = animal::is_baby(&tick.mob);
        animal::after_ai_step(tick);
        if was_baby && !animal::is_baby(&tick.mob) {
            grown_up(&mut tick.mob);
        }
        continuous_heal(tick);
    }

    /// `HappyGhast.tick()` after the animal's: its still timeout counted down (after a
    /// grace of 60 ticks from its load), and ten more ticks of it while a player stands
    /// on top.
    fn after_tick(&self, tick: &mut MobTick<'_>) {
        let tick_count = tick.tick_count.wrapping_add(1);
        let mut own = read(&tick.mob);
        if own.still_timeout > 0 {
            if tick_count > STILL_GRACE {
                own.still_timeout -= 1;
            }
            let timeout = own.still_timeout;
            own.set_server_still_timeout(timeout);
        }
        if scan_player_above(tick) {
            own.set_server_still_timeout(MAX_STILL_TIMEOUT);
        }
        update(&mut tick.mob, |kept| *kept = own);
    }

    /// The first step of `HappyGhast.tick()`: `leashHolderTime` (which the game does not
    /// save, and the region keeps in its memory) counted down, and `IS_LEASH_HOLDER` set
    /// by it.
    fn leash_holder_tick(&self, mob: &mut Mob, time: &mut i32) {
        if *time > 0 {
            *time -= 1;
        }
        let holds = *time > 0;
        update(mob, |own| own.leash_holder = holds);
    }

    fn quad_leash_holder(&self) -> bool {
        true
    }

    /// `leashElasticDistance()` and `leashSnapDistance()`: 10 and 16.
    fn leash_distances(&self) -> (f64, f64) {
        (10.0, 16.0)
    }

    /// `onElasticLeashPull()`: its move control waits.
    fn on_elastic_leash_pull(&self, mob: &mut Mob) {
        mob.body.controls.movement.operation = Operation::Wait;
    }

    /// `HappyGhast.mobInteract`: a ghastling as an animal fed snowballs (it grows, and
    /// never falls in love); a grown one first the item's own use (a harness put on),
    /// then, harnessed and not sneaking, a ride (part R's: nobody gets on yet), then an
    /// animal's use of no food.
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        if animal::is_baby(used.mob) {
            let result = animal::mob_interact(used, FOOD, None);
            if !animal::is_baby(used.mob) {
                grown_up(used.mob);
            }
            return result;
        }
        let equipped = equip_harness(used);
        if equipped != Interaction::Pass {
            return equipped;
        }
        animal::mob_interact(used, Items::Of(&[]), None)
    }

    /// `getBreedOffspring`: a happy ghast (what an egg used on one makes).
    fn breed_offspring(&self, _: &mut Breeding<'_>) -> Option<Mob> {
        Some(Mob::fresh())
    }

    /// `canFallInLove()` is false: no breeding.
    fn can_mate(&self, _: super::Mate<'_>, _: super::Mate<'_>) -> bool {
        false
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    /// `canBreatheUnderwater()`: a ghastling does.
    fn breathes_under_water(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    /// `getDefaultDimensions`: a ghastling's of 0.2375 of the type's, with eyes at
    /// 0.46875.
    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        let info = EntityTypeInfo::of(entity_types::HAPPY_GHAST);
        animal::dimensions(
            mob,
            Dimensions {
                width: info.width * BABY_SCALE,
                height: info.height * BABY_SCALE,
                eye_height: BABY_EYE_HEIGHT,
            },
        )
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn ambient_sound(&self, mob: &Mob) -> Option<i32> {
        Some(if animal::is_baby(mob) {
            sounds::ENTITY_GHASTLING_AMBIENT
        } else {
            sounds::ENTITY_HAPPY_GHAST_AMBIENT
        })
    }

    fn hurt_sound(&self, mob: &Mob, _: &DamageSource) -> Option<i32> {
        Some(if animal::is_baby(mob) {
            sounds::ENTITY_GHASTLING_HURT
        } else {
            sounds::ENTITY_HAPPY_GHAST_HURT
        })
    }

    fn death_sound(&self, mob: &Mob) -> Option<i32> {
        Some(if animal::is_baby(mob) {
            sounds::ENTITY_GHASTLING_DEATH
        } else {
            sounds::ENTITY_HAPPY_GHAST_DEATH
        })
    }

    /// `getSoundVolume()`: 1 for a ghastling, 4 grown.
    fn sound_volume_of(&self, mob: &Mob) -> f32 {
        if animal::is_baby(mob) { 1.0 } else { 4.0 }
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let own = read(mob);
        let mut values = animal::shown(mob);
        values.push((
            IS_LEASH_HOLDER_INDEX,
            DataShown::Boolean(own.leash_holder),
            !own.leash_holder,
        ));
        values.push((
            STAYS_STILL_INDEX,
            DataShown::Boolean(own.stays_still),
            !own.stays_still,
        ));
        values
    }
}

/// A ghastling (a dried ghast's, `DriedGhastBlock.spawnGhastling`): `EntityType.create(
/// level, BREEDING)` with no `finalizeSpawn`, `setBaby(true)`.
pub(crate) fn ghastling() -> Mob {
    let mut mob = Mob::fresh();
    animal::made_baby(&mut mob);
    mob
}
