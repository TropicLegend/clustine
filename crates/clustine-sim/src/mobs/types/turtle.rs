//! The turtle (`Turtle` with its nine goals, its move control and its navigation;
//! `docs/groundwork/n7-brief.md`, section 6.10): an amphibious animal of seagrass that
//! remembers the beach it was made on as its home, wanders the sea a leg of up to 16
//! blocks at a time towards places up to 512 away, and, bred, carries an egg home to dig
//! into the sand there and lay a clutch of one to four eggs. Its eggs are blocks
//! (`region/turtle_eggs.rs`), which hatch babies whose home is their nest; a baby that
//! grows up drops a scute.
//!
//! Its home may be anywhere; a leg's end beyond what its region holds is never taken
//! (`hasChunksAt` answers as for chunks not loaded), so a travelling turtle is stuck at
//! its region's edge as the game's is at the edge of what is loaded (the brief's W10 and
//! 11.6.3). What the game does not save (where it travels, whether it goes home, its
//! digging's count) it keeps as every tick's state is.
//!
//! Named differences: its steps and swimming make no sounds (the living body plays none
//! yet); no player is the cause of its love, so no statistic is given.

use clustine_blocks::data::{fluid_in_tag, in_tag, is_block};
use clustine_data::{PathType, blocks, entity_types, items, sounds};
use clustine_world::{BlockPos, ItemStack, TypeState};
use serde::{Deserialize, Serialize};

use super::animal::{self, Own};
use super::{Breeding, Did, Dimensions, Interaction, MobType, Spawning, Used, cow, water};
use crate::living::{Sinks, Travel};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    Items, LookAtPlayerGoal, MoveToBlockGoal, MovingToBlock, Panic, PanicGoal, RandomStrollGoal,
    Stroll, Strolling, TemptGoal, panics_from_damage,
};
use crate::mobs::ai::maths::{move_rotlerp, yaw_towards};
use crate::mobs::ai::navigation::{self, Kind, Settings};
use crate::mobs::ai::random_pos;
use crate::mobs::ai::selector::Goal;
use crate::mobs::ai::sensing::Class;
use crate::mobs::ai::{Ai, Operation, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::{Local, MobTick};
use crate::physics::{self, Vec3};
use crate::region::DEFAULT_PICK_UP_DELAY;
use crate::shown::DataShown;

/// `#turtle_food`.
const FOOD: Items = Items::Tag("minecraft:turtle_food");

/// The synced indices of `HAS_EGG` and `LAYING_EGG`.
const HAS_EGG_INDEX: u8 = 18;
const LAYING_EGG_INDEX: u8 = 19;

/// The overworld's sea level, below which a travel's target is kept.
const SEA_LEVEL: i32 = 63;

/// `(double)(float)(π / 10)` and `(double)(float)(π / 2)`.
const TENTH_PI: f64 = 0.314_159_274_101_257_3;
const HALF_PI: f64 = 1.570_796_370_506_286_6;

/// What a turtle keeps beyond an animal: its home (`homePos`) and whether it carries an
/// egg (`HAS_EGG`), both saved by the game; whether it digs to lay it (`LAYING_EGG`)
/// with its count (`layEggCounter`); where it travels (`travelPos`); and whether it goes
/// home (`goingHome`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct TurtleState {
    pub(crate) home: BlockPos,
    pub(crate) has_egg: bool,
    pub(crate) laying: bool,
    pub(crate) lay_counter: i32,
    pub(crate) travel: Option<BlockPos>,
    pub(crate) going_home: bool,
}

impl Default for TurtleState {
    /// The constructor's: home at the origin (`BlockPos.ZERO`), as the game's until its
    /// `finalizeSpawn` or its load sets it.
    fn default() -> Self {
        Self {
            home: BlockPos::new(0, 0, 0),
            has_egg: false,
            laying: false,
            lay_counter: 0,
            travel: None,
            going_home: false,
        }
    }
}

impl Own for TurtleState {}

/// A turtle's state.
pub(crate) fn read(mob: &Mob) -> TurtleState {
    animal::read::<TurtleState>(&mob.state).1
}

/// Changes a turtle's own state.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut TurtleState)) {
    let (shared, mut own) = animal::read::<TurtleState>(&mob.state);
    let before = own.clone();
    change(&mut own);
    if own != before {
        mob.state = animal::write(&shared, &own);
    }
}

/// `BlockPos.closerToCenterThan(position, distance)`: the block's middle nearer the
/// point than the distance.
pub(crate) fn closer_to_centre_than(pos: BlockPos, point: Vec3, distance: f64) -> bool {
    let centre = [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5,
        f64::from(pos.z) + 0.5,
    ];
    physics::distance_sqr(centre, point) < distance * distance
}

/// `TurtleEggBlock.isSand(level, pos)`: the block is of `#sand`.
pub(crate) fn is_sand(state: clustine_data::BlockState) -> bool {
    in_tag(state, "minecraft:sand")
}

/// `Vec3.atBottomCenterOf(pos)`.
fn bottom_centre(pos: BlockPos) -> Vec3 {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y),
        f64::from(pos.z) + 0.5,
    ]
}

/// A baby's size: the type's scaled by 0.3.
fn baby_size() -> Dimensions {
    let info = clustine_data::EntityTypeInfo::of(entity_types::TURTLE);
    water::scaled(info.width, info.height, info.eye_height, 0.3)
}

/// `TurtlePanicGoal(turtle, 1.2)`: a panic that looks for water within seven blocks
/// first.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TurtlePanicGoal;

impl TurtlePanicGoal {
    fn inner() -> PanicGoal {
        PanicGoal { speed: 1.2 }
    }
}

impl Goal for TurtlePanicGoal {
    type State = Panic;

    fn flags(&self) -> u8 {
        Self::inner().flags()
    }

    fn panics(&self) -> bool {
        true
    }

    fn can_use(&self, state: &mut Panic, ai: &mut Ai<'_, '_>) -> bool {
        if !panics_from_damage(ai) {
            return false;
        }
        if let Some(water) = PanicGoal::look_for_water(ai, 7) {
            state.wanted = [f64::from(water.x), f64::from(water.y), f64::from(water.z)];
            return true;
        }
        match random_pos::default_pos(ai, 5, 4) {
            Some(wanted) => {
                state.wanted = wanted;
                true
            }
            None => false,
        }
    }

    fn can_continue(&self, state: &mut Panic, ai: &mut Ai<'_, '_>) -> bool {
        Self::inner().can_continue(state, ai)
    }

    fn start(&self, state: &mut Panic, ai: &mut Ai<'_, '_>) {
        Self::inner().start(state, ai);
    }

    fn stop(&self, state: &mut Panic, ai: &mut Ai<'_, '_>) {
        Self::inner().stop(state, ai);
    }
}

/// `TurtleBreedGoal(turtle, 1.0)`: an animal's breeding, but for a turtle that carries
/// an egg; its breeding gives it an egg rather than a child (the region's breeding asks
/// [`MobType::bred_without_child`]).
#[derive(Debug, Clone, Copy)]
pub(crate) struct TurtleBreedGoal;

impl TurtleBreedGoal {
    fn inner() -> animal::BreedGoal {
        animal::BreedGoal { speed: 1.0 }
    }
}

impl Goal for TurtleBreedGoal {
    type State = animal::Courting;

    fn flags(&self) -> u8 {
        Self::inner().flags()
    }

    fn can_use(&self, state: &mut animal::Courting, ai: &mut Ai<'_, '_>) -> bool {
        Self::inner().can_use(state, ai) && !read(&ai.tick.mob).has_egg
    }

    fn can_continue(&self, state: &mut animal::Courting, ai: &mut Ai<'_, '_>) -> bool {
        Self::inner().can_continue(state, ai)
    }

    fn stop(&self, state: &mut animal::Courting, ai: &mut Ai<'_, '_>) {
        Self::inner().stop(state, ai);
    }

    fn tick(&self, state: &mut animal::Courting, ai: &mut Ai<'_, '_>) {
        Self::inner().tick(state, ai);
    }
}

/// `TurtleLayEggGoal`'s target: sand with nothing above it.
fn egg_target(ai: &Ai<'_, '_>, pos: BlockPos) -> bool {
    let world = ai.tick.world;
    world.state(pos.offset(0, 1, 0)).props().is_air() && is_sand(world.state(pos))
}

/// `TurtleLayEggGoal(turtle, 1.0)`: with an egg, within nine of home, to sand within 16
/// blocks; there out of water it digs for ten seconds and lays a clutch of one to four
/// above it, and is in love again.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LayEggGoal;

impl LayEggGoal {
    fn inner() -> MoveToBlockGoal {
        MoveToBlockGoal {
            speed: 1.0,
            search_range: 16,
            vertical_search_range: 1,
            vertical_search_start: 0,
            valid: egg_target,
            next_start: None,
        }
    }

    fn near_home(ai: &Ai<'_, '_>) -> bool {
        let own = read(&ai.tick.mob);
        own.has_egg && closer_to_centre_than(own.home, ai.position(), 9.0)
    }
}

impl Goal for LayEggGoal {
    type State = MovingToBlock;

    fn flags(&self) -> u8 {
        Self::inner().flags()
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) -> bool {
        Self::near_home(ai) && Self::inner().can_use(state, ai)
    }

    fn can_continue(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) -> bool {
        Self::inner().can_continue(state, ai) && Self::near_home(ai)
    }

    fn start(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) {
        Self::inner().start(state, ai);
    }

    fn tick(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) {
        Self::inner().tick(state, ai);
        if ai.in_water() || !state.reached {
            return;
        }
        let own = read(&ai.tick.mob);
        if own.lay_counter < 1 {
            // `setLayingEgg(true)`.
            update(&mut ai.tick.mob, |turtle| {
                turtle.laying = true;
                turtle.lay_counter = 1;
            });
        } else if own.lay_counter > self.adjusted_tick_delay(200) {
            let sand = state.block.unwrap_or(BlockPos::new(0, 0, 0));
            let at = ai.block_position();
            // The level's draw on its behalf for the sound's pitch, then its own for the
            // clutch.
            let pitch = 0.9 + ai.tick.draws.next_float() * 0.2;
            ai.tick.out.local.push(Local::Sound {
                sound: sounds::ENTITY_TURTLE_LAY_EGG,
                source: crate::api::SOUND_SOURCE_BLOCKS,
                at: [
                    f64::from(at.x) + 0.5,
                    f64::from(at.y) + 0.5,
                    f64::from(at.z) + 0.5,
                ],
                volume: 0.3,
                pitch,
            });
            let eggs = ai.tick.draws.next_int(4) + 1;
            let egg = clustine_blocks::data::set(blocks::TURTLE_EGG, "eggs", &eggs.to_string());
            let above = sand.offset(0, 1, 0);
            ai.tick.out.local.push(Local::SetBlock {
                pos: above,
                state: egg,
                flags: 3,
            });
            ai.tick.out.local.push(Local::GameEvent {
                name: "minecraft:block_place",
                pos: above,
            });
            let now = ai.tick.lived;
            update(&mut ai.tick.mob, |turtle| {
                turtle.has_egg = false;
                turtle.laying = false;
                turtle.lay_counter = 0;
            });
            // `setInLoveTime(600)`.
            animal::update(&mut ai.tick.mob, |shared| shared.set_in_love(now));
        }
        if read(&ai.tick.mob).laying {
            update(&mut ai.tick.mob, |turtle| turtle.lay_counter += 1);
        }
    }
}

/// `TurtleGoToWaterGoal`'s target: water itself.
fn water_target(ai: &Ai<'_, '_>, pos: BlockPos) -> bool {
    is_block(ai.tick.world.state(pos), blocks::WATER)
}

/// `TurtleGoToWaterGoal(turtle, 1.0)`: a baby on land, or a grown one that goes nowhere
/// and carries no egg, to water within 24 blocks, its path made again every 160 ticks.
/// Its speed is the one given (the game's 2 for a baby is chosen as its goals are made,
/// when no turtle is a baby yet).
#[derive(Debug, Clone, Copy)]
pub(crate) struct GoToWaterGoal;

impl GoToWaterGoal {
    fn inner() -> MoveToBlockGoal {
        MoveToBlockGoal {
            speed: 1.0,
            search_range: 24,
            vertical_search_range: 1,
            vertical_search_start: -1,
            valid: water_target,
            next_start: None,
        }
    }
}

impl Goal for GoToWaterGoal {
    type State = MovingToBlock;

    fn flags(&self) -> u8 {
        Self::inner().flags()
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) -> bool {
        let own = read(&ai.tick.mob);
        if animal::is_baby(&ai.tick.mob) && !ai.in_water() {
            return Self::inner().can_use(state, ai);
        }
        !own.going_home && !ai.in_water() && !own.has_egg && Self::inner().can_use(state, ai)
    }

    fn can_continue(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) -> bool {
        let block = state.block.unwrap_or(BlockPos::new(0, 0, 0));
        !ai.in_water() && state.try_ticks <= 1200 && water_target(ai, block)
    }

    fn start(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) {
        Self::inner().start(state, ai);
    }

    /// `MoveToBlockGoal.tick()` with this goal's `shouldRecalculatePath`.
    fn tick(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) {
        let block = state.block.unwrap_or(BlockPos::new(0, 0, 0));
        let target = block.offset(0, 1, 0);
        if !closer_to_centre_than(target, ai.position(), 1.0) {
            state.reached = false;
            state.try_ticks += 1;
            if state.try_ticks % 160 == 0 {
                navigation::move_to(ai, bottom_centre(target), 1.0);
            }
        } else {
            state.reached = true;
            state.try_ticks -= 1;
        }
    }
}

/// `TurtleGoHomeGoal`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct GoingHome {
    pub(crate) stuck: bool,
    pub(crate) close_to_home_try_ticks: i32,
}

/// `TurtleGoHomeGoal(turtle, 1.0)`: with an egg, or now and then far from home, home by
/// legs of up to 16 blocks. No flags.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GoHomeGoal;

impl Goal for GoHomeGoal {
    type State = GoingHome;

    fn flags(&self) -> u8 {
        0
    }

    fn can_use(&self, _: &mut GoingHome, ai: &mut Ai<'_, '_>) -> bool {
        if animal::is_baby(&ai.tick.mob) {
            return false;
        }
        let own = read(&ai.tick.mob);
        if own.has_egg {
            return true;
        }
        if ai
            .tick
            .draws
            .next_int(crate::mobs::ai::maths::reduced_tick_delay(700) as u32)
            != 0
        {
            return false;
        }
        !closer_to_centre_than(own.home, ai.position(), 64.0)
    }

    fn can_continue(&self, state: &mut GoingHome, ai: &mut Ai<'_, '_>) -> bool {
        let own = read(&ai.tick.mob);
        !closer_to_centre_than(own.home, ai.position(), 7.0)
            && !state.stuck
            && state.close_to_home_try_ticks <= self.adjusted_tick_delay(600)
    }

    fn start(&self, state: &mut GoingHome, ai: &mut Ai<'_, '_>) {
        update(&mut ai.tick.mob, |turtle| turtle.going_home = true);
        state.stuck = false;
        state.close_to_home_try_ticks = 0;
    }

    fn stop(&self, _: &mut GoingHome, ai: &mut Ai<'_, '_>) {
        update(&mut ai.tick.mob, |turtle| turtle.going_home = false);
    }

    fn tick(&self, state: &mut GoingHome, ai: &mut Ai<'_, '_>) {
        let home = read(&ai.tick.mob).home;
        let near = closer_to_centre_than(home, ai.position(), 16.0);
        if near {
            state.close_to_home_try_ticks += 1;
        }
        if !navigation::is_done(ai) {
            return;
        }
        let target = bottom_centre(home);
        let mut leg = random_pos::default_pos_towards(ai, 16, 3, target, TENTH_PI)
            .or_else(|| random_pos::default_pos_towards(ai, 8, 7, target, HALF_PI));
        if let Some(at) = leg
            && !near
            && !is_block(ai.tick.world.state(physics::containing(at)), blocks::WATER)
        {
            leg = random_pos::default_pos_towards(ai, 16, 5, target, HALF_PI);
        }
        let Some(at) = leg else {
            state.stuck = true;
            return;
        };
        navigation::move_to(ai, at, 1.0);
    }
}

/// `TurtleTravelGoal`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Travelling {
    pub(crate) stuck: bool,
}

/// `TurtleTravelGoal(turtle, 1.0)`: in water, with nowhere else to go, one leg towards a
/// place up to 512 blocks away, drawn anew each time. No flags.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TravelGoal;

impl TravelGoal {
    /// `level.hasChunksAt(x - 34, z - 34, x + 34, z + 34)`: every chunk of the square is
    /// one the region holds or copies.
    fn has_chunks_around(ai: &Ai<'_, '_>, x: i32, z: i32) -> bool {
        let (low_x, high_x) = ((x - 34) >> 4, (x + 34) >> 4);
        let (low_z, high_z) = ((z - 34) >> 4, (z + 34) >> 4);
        (low_x..=high_x).all(|cx| {
            (low_z..=high_z).all(|cz| ai.tick.world.is_loaded(BlockPos::new(cx << 4, 0, cz << 4)))
        })
    }
}

impl Goal for TravelGoal {
    type State = Travelling;

    fn flags(&self) -> u8 {
        0
    }

    fn can_use(&self, _: &mut Travelling, ai: &mut Ai<'_, '_>) -> bool {
        let own = read(&ai.tick.mob);
        !own.going_home && !own.has_egg && ai.in_water()
    }

    fn can_continue(&self, state: &mut Travelling, ai: &mut Ai<'_, '_>) -> bool {
        let own = read(&ai.tick.mob);
        let now = ai.tick.lived;
        !navigation::is_done(ai)
            && !state.stuck
            && !own.going_home
            && !animal::animal_of(&ai.tick.mob.state).in_love(now)
            && !own.has_egg
    }

    fn start(&self, state: &mut Travelling, ai: &mut Ai<'_, '_>) {
        let draws = &mut *ai.tick.draws;
        let dx = draws.next_int(1025) as i32 - 512;
        let mut dy = draws.next_int(9) as i32 - 4;
        let dz = draws.next_int(1025) as i32 - 512;
        let [x, y, z] = ai.position();
        if f64::from(dy) + y > f64::from(SEA_LEVEL - 1) {
            dy = 0;
        }
        let travel = physics::containing([f64::from(dx) + x, f64::from(dy) + y, f64::from(dz) + z]);
        update(&mut ai.tick.mob, |turtle| turtle.travel = Some(travel));
        state.stuck = false;
    }

    fn stop(&self, _: &mut Travelling, ai: &mut Ai<'_, '_>) {
        update(&mut ai.tick.mob, |turtle| turtle.travel = None);
    }

    fn tick(&self, state: &mut Travelling, ai: &mut Ai<'_, '_>) {
        let Some(travel) = read(&ai.tick.mob).travel else {
            state.stuck = true;
            return;
        };
        if !navigation::is_done(ai) {
            return;
        }
        let target = bottom_centre(travel);
        let mut leg = random_pos::default_pos_towards(ai, 16, 3, target, TENTH_PI)
            .or_else(|| random_pos::default_pos_towards(ai, 8, 7, target, HALF_PI));
        if let Some(at) = leg {
            let (x, z) = (physics::floor(at[0]), physics::floor(at[2]));
            if !Self::has_chunks_around(ai, x, z) {
                leg = None;
            }
        }
        let Some(at) = leg else {
            state.stuck = true;
            return;
        };
        navigation::move_to(ai, at, 1.0);
    }
}

/// `TurtleRandomStrollGoal(turtle, 1.0, 100)`: on land, going nowhere, with no egg.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TurtleStrollGoal;

impl TurtleStrollGoal {
    fn inner() -> RandomStrollGoal {
        RandomStrollGoal {
            speed: 1.0,
            interval: 100,
            check_no_action_time: true,
            kind: Stroll::Default,
        }
    }
}

impl Goal for TurtleStrollGoal {
    type State = Strolling;

    fn flags(&self) -> u8 {
        Self::inner().flags()
    }

    fn can_use(&self, state: &mut Strolling, ai: &mut Ai<'_, '_>) -> bool {
        let own = read(&ai.tick.mob);
        !ai.in_water() && !own.going_home && !own.has_egg && Self::inner().can_use(state, ai)
    }

    fn can_continue(&self, state: &mut Strolling, ai: &mut Ai<'_, '_>) -> bool {
        Self::inner().can_continue(state, ai)
    }

    fn start(&self, state: &mut Strolling, ai: &mut Ai<'_, '_>) {
        Self::inner().start(state, ai);
    }

    fn stop(&self, state: &mut Strolling, ai: &mut Ai<'_, '_>) {
        Self::inner().stop(state, ai);
    }
}

/// `Mob.setSpeed(speed)`, which sets `zza` too.
fn set_speed(ai: &mut Ai<'_, '_>, speed: f32) {
    let inputs = &mut ai.tick.mob.body.inputs;
    inputs.speed = speed;
    inputs.zza = speed;
}

/// `TurtleMoveControl.tick()`: its speed held back first (`updateSpeed`: buoyed in
/// water, halved there far from home and a third for a baby, halved on land, each with a
/// floor), then towards what it wants as a fish's control steers, its yaw turned up to 90
/// a tick.
fn tick_move_control(ai: &mut Ai<'_, '_>) {
    let speed = ai.tick.mob.body.inputs.speed;
    if ai.in_water() {
        ai.tick.living.body.motion[1] += 0.005;
        let home = read(&ai.tick.mob).home;
        if !closer_to_centre_than(home, ai.position(), 16.0) {
            set_speed(ai, (speed / 2.0).max(0.08));
        }
        if animal::is_baby(&ai.tick.mob) {
            let speed = ai.tick.mob.body.inputs.speed;
            set_speed(ai, (speed / 3.0).max(0.06));
        }
    } else if ai.on_ground() {
        set_speed(ai, (speed / 2.0).max(0.06));
    }
    let movement = ai.tick.mob.body.controls.movement.clone();
    if movement.operation != Operation::MoveTo || navigation::is_done(ai) {
        set_speed(ai, 0.0);
        return;
    }
    let [x, y, z] = ai.position();
    let dx = movement.wanted[0] - x;
    let dy = movement.wanted[1] - y;
    let dz = movement.wanted[2] - z;
    let distance = (dx * dx + dy * dy + dz * dz).sqrt();
    if distance < 9.999_999_747_378_752e-6 {
        set_speed(ai, 0.0);
        return;
    }
    let dy = dy / distance;
    let yaw = yaw_towards(dx, dz);
    ai.tick.yaw = move_rotlerp(ai.tick.yaw, yaw, 90.0);
    ai.tick.mob.body.head.body_yaw = ai.tick.yaw;
    let target = (movement.speed * ai.attribute(clustine_data::attributes::MOVEMENT_SPEED)) as f32;
    let speed = ai.tick.mob.body.inputs.speed;
    set_speed(ai, speed + 0.125 * (target - speed));
    let speed = f64::from(ai.tick.mob.body.inputs.speed);
    ai.tick.living.body.motion[1] += speed * dy * 0.1;
}

/// `ageBoundaryReached()` of a baby grown up: a scute (`gameplay/turtle_grow`, a gift
/// table of one scute, one roll, no condition and no function: nothing drawn).
fn scute() -> ItemStack {
    ItemStack {
        item: items::TURTLE_SCUTE,
        count: 1,
    }
}

/// `Turtle.aiStep()` after the animal's: while it digs, every fifth of its count on
/// sand, the sand's breaking shown (level event 2001) and `ENTITY_ACTION`.
fn dig(tick: &mut MobTick<'_>) {
    let own = read(&tick.mob);
    if !water::alive(&tick.mob) || !own.laying || own.lay_counter < 1 || own.lay_counter % 5 != 0 {
        return;
    }
    let at = physics::containing(tick.living.body.position);
    let below = tick.world.state(at.offset(0, -1, 0));
    if !is_sand(below) {
        return;
    }
    tick.out.local.push(Local::LevelEvent {
        kind: 2001,
        pos: at,
        data: i32::from(below.0),
    });
    tick.out.local.push(Local::GameEvent {
        name: "minecraft:entity_action",
        pos: at,
    });
}

/// The turtle.
pub(crate) struct Turtle;

impl MobType for Turtle {
    fn register_goals(&self, setup: &mut Setup) {
        setup.config.navigation = Settings::of(Kind::Amphibious);
        cow::animal_maluses(setup);
        setup.set_malus(PathType::Water, 0.0);
        setup.set_malus(PathType::DoorIronClosed, -1.0);
        setup.set_malus(PathType::DoorWoodClosed, -1.0);
        setup.set_malus(PathType::DoorOpen, -1.0);
        setup.add_goal(0, TurtlePanicGoal);
        setup.add_goal(1, TurtleBreedGoal);
        setup.add_goal(1, LayEggGoal);
        setup.add_goal(2, TemptGoal::new(1.1, FOOD, false));
        setup.add_goal(3, GoToWaterGoal);
        setup.add_goal(4, GoHomeGoal);
        setup.add_goal(7, TravelGoal);
        setup.add_goal(8, LookAtPlayerGoal::new(Class::Player, 8.0));
        setup.add_goal(9, TurtleStrollGoal);
    }

    /// `TurtlePathNavigation.isStableDestination(pos)`: water while it travels, else
    /// anything but air below.
    fn stable_destination(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> Option<bool> {
        let world = ai.tick.world;
        Some(if read(&ai.tick.mob).travel.is_some() {
            is_block(world.state(pos), blocks::WATER)
        } else {
            !world.state(pos.offset(0, -1, 0)).props().is_air()
        })
    }

    /// `Turtle.getWalkTargetValue(pos)`: water while it goes nowhere, sand, else the
    /// light's worth.
    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        let world = ai.tick.world;
        if !read(&ai.tick.mob).going_home && fluid_in_tag(world.state(pos), "minecraft:water") {
            return 10.0;
        }
        if is_sand(world.state(pos.offset(0, -1, 0))) {
            return 10.0;
        }
        animal::light_cost(ai, pos)
    }

    fn tick_move_control(&self, ai: &mut Ai<'_, '_>) {
        tick_move_control(ai);
    }

    /// `finalizeSpawn`: its home where it is made.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let at = spawn.at;
        update(spawn.mob, |turtle| turtle.home = at);
    }

    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::custom_server_ai_step(tick);
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        let was_baby = animal::is_baby(&tick.mob);
        animal::after_ai_step(tick);
        if was_baby && !animal::is_baby(&tick.mob) {
            tick.out.local.push(Local::Drop {
                stack: scute(),
                at: tick.living.body.position,
                pick_up_delay: DEFAULT_PICK_UP_DELAY,
            });
        }
        dig(tick);
    }

    /// `Turtle.travelInWater`: a tenth along its input, sinking with no target unless it
    /// goes home and is within 20 of it.
    fn travel(&self, tick: &MobTick<'_>) -> Travel {
        let own = read(&tick.mob);
        let sinks = if tick.mob.body.combat.target.is_some() {
            Sinks::Never
        } else if own.going_home {
            Sinks::UnlessNear {
                centre: own.home,
                distance: 20.0,
            }
        } else {
            Sinks::Always
        };
        Travel::Swims { speed: 0.1, sinks }
    }

    fn pushed_by_fluid(&self) -> bool {
        false
    }

    fn can_be_leashed(&self, _: u16, _: &Mob) -> bool {
        false
    }

    /// `Animal.mobInteract` with seagrass, but no love for a turtle that carries an egg
    /// (`canFallInLove`); a baby fed to grow up drops its scute.
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        let was_baby = animal::is_baby(used.mob);
        let result = if read(used.mob).has_egg && !was_baby {
            animal::mob_interact(used, Items::Of(&[]), None)
        } else {
            animal::mob_interact(used, FOOD, None)
        };
        if was_baby && !animal::is_baby(used.mob) {
            used.did.push(Did::Drop {
                stack: scute(),
                at: used.position,
                pick_up_delay: DEFAULT_PICK_UP_DELAY,
                push: None,
            });
        }
        result
    }

    fn breed_offspring(&self, _: &mut Breeding<'_>) -> Option<Mob> {
        Some(Mob::fresh())
    }

    /// `TurtleBreedGoal.breed()`: an egg for the turtle that bred.
    fn bred_without_child(&self, this: &Mob) -> Option<TypeState> {
        let (shared, mut own) = animal::read::<TurtleState>(&this.state);
        own.has_egg = true;
        Some(animal::write(&shared, &own))
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        animal::dimensions(mob, baby_size())
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    /// None, as a fresh turtle is in no water and on no ground.
    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        None
    }

    /// `turtle.ambient_land` for a grown one on land, else the animal's none.
    fn ambient_sound_at(&self, mob: &Mob, in_water: bool, on_ground: bool) -> Option<i32> {
        (!in_water && on_ground && !animal::is_baby(mob))
            .then_some(sounds::ENTITY_TURTLE_AMBIENT_LAND)
    }

    fn ambient_interval(&self) -> u32 {
        200
    }

    fn hurt_sound(&self, mob: &Mob, _: &DamageSource) -> Option<i32> {
        Some(if animal::is_baby(mob) {
            sounds::ENTITY_TURTLE_HURT_BABY
        } else {
            sounds::ENTITY_TURTLE_HURT
        })
    }

    fn death_sound(&self, mob: &Mob) -> Option<i32> {
        Some(if animal::is_baby(mob) {
            sounds::ENTITY_TURTLE_DEATH_BABY
        } else {
            sounds::ENTITY_TURTLE_DEATH
        })
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let own = read(mob);
        let mut values = animal::shown(mob);
        values.push((HAS_EGG_INDEX, DataShown::Boolean(own.has_egg), !own.has_egg));
        values.push((
            LAYING_EGG_INDEX,
            DataShown::Boolean(own.laying),
            !own.laying,
        ));
        values
    }
}

/// A baby hatched from an egg (`TurtleEggBlock`'s hatching): `EntityType.create(level,
/// BREEDING)` with no `finalizeSpawn`, a baby of `-24000` whose home is its nest.
pub(crate) fn hatchling(nest: BlockPos) -> Mob {
    let mut mob = Mob::fresh();
    animal::made_baby(&mut mob);
    update(&mut mob, |turtle| turtle.home = nest);
    mob
}
