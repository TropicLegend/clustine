//! The drowned (`Drowned`; `docs/groundwork/n7k-brief.md` 11.1): a zombie at home in
//! water. By day on land it walks to water within ten blocks; in water by day it sinks
//! and walks the bottom; at night it swims up (`DrownedSwimUpGoal`, which sets it
//! searching for land, the only thing that makes it want to swim without a target) and
//! walks out onto a beach. It swims by its own move control and water travel, drawn as
//! a swimmer while it does, and paths as the amphibious do. It may hold a trident or a
//! fishing rod and carry a nautilus shell, whatever the difficulty; one made by ocean
//! ruins or a natural spawn with a trident may ride a zombie nautilus made with it.
//!
//! Named differences: its trident's throw and its attack, and its wish to swim after a
//! target in water, are N9's (it never has a target); its body is not turned with a
//! zombie nautilus it rides (`rideTick`); `drowned.swim` is not heard (the living body
//! plays no swim sound of a type's own).

use clustine_blocks::data::is_block;
use clustine_data::{PathType, attributes, blocks, entity_types, items, sounds};
use clustine_world::{BlockPos, ItemStack};
use serde::{Deserialize, Serialize};

use super::tamable::Idle;
use super::zombie_family as family;
use super::{Breeding, CompanionSeat, Dimensions, MobType, SpawnReason, Spawning, animal, water};
use crate::living::{Sinks, Travel, slots};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{MoveToBlockGoal, MovingToBlock, RandomStrollGoal};
use crate::mobs::ai::maths::{move_rotlerp, yaw_towards};
use crate::mobs::ai::navigation::{self, Kind, Settings};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::{Ai, Operation, Setup, controls, random_pos};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::making;
use crate::mobs::tick::MobTick;
use crate::physics::Vec3;
use crate::region::Random;
use crate::shown::DataShown;

/// The drowned.
pub(crate) struct Drowned;

const KIND: u16 = entity_types::DROWNED as u16;

/// The overworld's sea level (`getSeaLevel`).
const SEA_LEVEL: i32 = 63;

/// `(double)(float)(π / 2)`, the swim up's angle.
const HALF_PI: f64 = 1.570_796_370_506_286_6;

/// `Level.isBrightOutside()`: the sky darkened less than four levels.
fn bright_outside(ai: &Ai<'_, '_>) -> bool {
    ai.tick.world.sky_darken() < 4
}

/// `wantsToSwim()`: searching for land (a target in water is N9's).
fn wants_to_swim(mob: &Mob) -> bool {
    family::read(mob).searching_for_land
}

/// `setSearchingForLand(searching)`.
fn set_searching_for_land(ai: &mut Ai<'_, '_>, searching: bool) {
    family::update(&mut ai.tick.mob, |own| own.searching_for_land = searching);
}

/// `DrownedGoToWaterGoal`'s state: where it goes (`wantedX`, `wantedY`, `wantedZ`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct GoingToWater {
    pub(crate) wanted: Vec3,
}

/// `DrownedGoToWaterGoal(drowned, 1.0)`: by day, out of water, to a block of water up
/// to ten tries of three draws each find about it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GoToWaterGoal;

impl GoToWaterGoal {
    /// `getWaterPos()`: of ten offsets about its block, each drawn x, then y, then z,
    /// the bottom middle of the first that is water (`Blocks.WATER`: a waterlogged block
    /// is not).
    fn water_pos(ai: &mut Ai<'_, '_>) -> Option<Vec3> {
        let at = ai.block_position();
        for _ in 0..10 {
            let dx = ai.tick.draws.next_int(20) as i32 - 10;
            let dy = 2 - ai.tick.draws.next_int(8) as i32;
            let dz = ai.tick.draws.next_int(20) as i32 - 10;
            let pos = at.offset(dx, dy, dz);
            if is_block(ai.tick.world.state(pos), blocks::WATER) {
                return Some([
                    f64::from(pos.x) + 0.5,
                    f64::from(pos.y),
                    f64::from(pos.z) + 0.5,
                ]);
            }
        }
        None
    }
}

impl Goal for GoToWaterGoal {
    type State = GoingToWater;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, state: &mut GoingToWater, ai: &mut Ai<'_, '_>) -> bool {
        if !bright_outside(ai) || ai.in_water() {
            return false;
        }
        let Some(wanted) = Self::water_pos(ai) else {
            return false;
        };
        state.wanted = wanted;
        true
    }

    fn can_continue(&self, _: &mut GoingToWater, ai: &mut Ai<'_, '_>) -> bool {
        !navigation::is_done(ai)
    }

    fn start(&self, state: &mut GoingToWater, ai: &mut Ai<'_, '_>) {
        navigation::move_to(ai, state.wanted, 1.0);
    }
}

/// `DrownedGoToBeachGoal(drowned, 1.0)`: a `MoveToBlockGoal` of 8 across and 2 up and
/// down, which at night takes one in water near the surface to land.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GoToBeachGoal;

impl GoToBeachGoal {
    fn moving() -> MoveToBlockGoal {
        MoveToBlockGoal {
            speed: 1.0,
            search_range: 8,
            vertical_search_range: 2,
            vertical_search_start: 0,
            valid: beach,
            next_start: None,
        }
    }
}

/// `DrownedGoToBeachGoal.isValidTarget`: air in the two blocks above a block whose top
/// it can stand on (`entityCanStandOn`; **Inferred**, as the navigation's, from the
/// face's sturdiness).
fn beach(ai: &Ai<'_, '_>, pos: BlockPos) -> bool {
    let world = ai.tick.world;
    world.state(pos.offset(0, 1, 0)).props().is_air()
        && world.state(pos.offset(0, 2, 0)).props().is_air()
        && world
            .state(pos)
            .props()
            .is_face_sturdy(clustine_data::Face::Up, clustine_data::SupportType::Full)
}

impl Goal for GoToBeachGoal {
    type State = MovingToBlock;

    fn flags(&self) -> u8 {
        Self::moving().flags()
    }

    fn every_tick(&self) -> bool {
        true
    }

    /// `MoveToBlockGoal.canUse()` first (its countdown, its draw and its search, by day
    /// too), then night, water and within three of the sea's surface.
    fn can_use(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) -> bool {
        Self::moving().can_use(state, ai)
            && !bright_outside(ai)
            && ai.in_water()
            && ai.position()[1] >= f64::from(SEA_LEVEL - 3)
    }

    fn can_continue(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) -> bool {
        Self::moving().can_continue(state, ai)
    }

    fn start(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) {
        set_searching_for_land(ai, false);
        Self::moving().start(state, ai);
    }

    fn tick(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) {
        Self::moving().tick(state, ai);
    }
}

/// `DrownedSwimUpGoal`'s `stuck`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct SwimmingUp {
    pub(crate) stuck: bool,
}

/// `DrownedSwimUpGoal(drowned, 1.0, seaLevel)`: at night, in water more than two below
/// the sea's surface, it searches for land and swims up towards the surface. It takes no
/// flags, and runs beside any other goal.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SwimUpGoal;

impl Goal for SwimUpGoal {
    type State = SwimmingUp;

    fn flags(&self) -> u8 {
        0
    }

    fn can_use(&self, _: &mut SwimmingUp, ai: &mut Ai<'_, '_>) -> bool {
        !bright_outside(ai) && ai.in_water() && ai.position()[1] < f64::from(SEA_LEVEL - 2)
    }

    fn can_continue(&self, state: &mut SwimmingUp, ai: &mut Ai<'_, '_>) -> bool {
        self.can_use(state, ai) && !state.stuck
    }

    fn start(&self, state: &mut SwimmingUp, ai: &mut Ai<'_, '_>) {
        set_searching_for_land(ai, true);
        state.stuck = false;
    }

    fn stop(&self, _: &mut SwimmingUp, ai: &mut Ai<'_, '_>) {
        set_searching_for_land(ai, false);
    }

    /// Below the surface, with its path done or near its end, a place up to 4 across and
    /// 8 up or down towards the surface above it (`DefaultRandomPos.getPosTowards`); none
    /// leaves it stuck.
    fn tick(&self, state: &mut SwimmingUp, ai: &mut Ai<'_, '_>) {
        let [x, y, z] = ai.position();
        if y >= f64::from(SEA_LEVEL - 1) || !(navigation::is_done(ai) || close_to_next_pos(ai)) {
            return;
        }
        let surface = [x, f64::from(SEA_LEVEL - 1), z];
        match random_pos::default_pos_towards(ai, 4, 8, surface, HALF_PI) {
            Some(to) => {
                navigation::move_to(ai, to, 1.0);
            }
            None => state.stuck = true,
        }
    }
}

/// `Drowned.closeToNextPos()`: its path's target within two blocks of its feet (the
/// target's corner, as `distanceToSqr` of its whole coordinates).
fn close_to_next_pos(ai: &Ai<'_, '_>) -> bool {
    let Some(path) = ai.tick.mob.body.path.followed.as_ref() else {
        return false;
    };
    let target = path.target;
    let [x, y, z] = ai.position();
    let (dx, dy, dz) = (
        x - f64::from(target.x),
        y - f64::from(target.y),
        z - f64::from(target.z),
    );
    dx * dx + dy * dy + dz * dz < 4.0
}

/// `Mob.setSpeed(speed)`, which sets `zza` too.
fn set_speed(ai: &mut Ai<'_, '_>, speed: f32) {
    let inputs = &mut ai.tick.mob.body.inputs;
    inputs.speed = speed;
    inputs.zza = speed;
}

/// `DrownedMoveControl.tick()`: wanting to swim in water, it rises a little while it
/// searches for land and steers itself towards what it wants, its yaw turned up to 90 a
/// tick and its speed eased an eighth of the way; otherwise it sinks a little faster off
/// the ground and walks as `MoveControl` walks.
fn tick_move_control(ai: &mut Ai<'_, '_>) {
    let own = family::read(&ai.tick.mob);
    if !(wants_to_swim(&ai.tick.mob) && ai.in_water()) {
        if !ai.on_ground() {
            ai.tick.living.body.motion[1] += -0.008;
        }
        controls::tick_move(ai);
        return;
    }
    if own.searching_for_land {
        ai.tick.living.body.motion[1] += 0.002;
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
    let dy = dy / distance;
    let yaw = yaw_towards(dx, dz);
    ai.tick.yaw = move_rotlerp(ai.tick.yaw, yaw, 90.0);
    ai.tick.mob.body.head.body_yaw = ai.tick.yaw;
    let wanted = (movement.speed * ai.attribute(attributes::MOVEMENT_SPEED)) as f32;
    let speed = ai.tick.mob.body.inputs.speed;
    let speed = speed + 0.125 * (wanted - speed);
    set_speed(ai, speed);
    let speed = f64::from(speed);
    let motion = &mut ai.tick.living.body.motion;
    motion[0] += speed * dx * 0.005;
    motion[1] += speed * dy * 0.1;
    motion[2] += speed * dz * 0.005;
}

/// `Drowned.populateDefaultEquipmentSlots`, in place of the zombie's and `Monster`'s:
/// one time in ten a trident (ten in sixteen) or a fishing rod, whatever the difficulty.
fn drowned_equipment(spawn: &mut Spawning<'_>, _special: f32) {
    if f64::from(spawn.level.next_float()) > 0.9 {
        let item = if spawn.level.next_int(16) < 10 {
            items::TRIDENT
        } else {
            items::FISHING_ROD
        };
        spawn.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] =
            Some(ItemStack { item, count: 1 });
    }
}

/// `Drowned.finalizeSpawn` after `Zombie`'s: one time in 33 a nautilus shell in the off
/// hand, sure to drop; then, made by ocean ruins or a natural spawn with a trident, one
/// time in two an adult outside a river rides a zombie nautilus made with it (persistent
/// for a structure's), the half drawn before its age and the biome are asked.
fn finalize_spawn(spawn: &mut Spawning<'_>) {
    family::finalize_spawn(KIND, spawn, drowned_equipment);
    let off_hand = usize::from(slots::OFF_HAND);
    if spawn.mob.body.equipment.slots[off_hand].is_none() && spawn.level.next_float() < 0.03 {
        let equipment = &mut spawn.mob.body.equipment;
        equipment.slots[off_hand] = Some(ItemStack {
            item: items::NAUTILUS_SHELL,
            count: 1,
        });
        equipment.drop_chances.get_or_insert([0.085; 8])[off_hand] = 2.0;
    }
    let reason = spawn.reason;
    let trident = animal::is_item(
        spawn.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)],
        items::TRIDENT,
    );
    if !matches!(reason, SpawnReason::Natural | SpawnReason::Structure) || !trident {
        return;
    }
    if spawn.level.next_float() >= 0.5
        || family::is_baby(spawn.mob)
        || animal::biome_in_tag(spawn.biome, "minecraft:more_frequent_drowned_spawns")
    {
        return;
    }
    let position = spawn
        .companions
        .as_deref()
        .and_then(|companions| companions.position())
        .unwrap_or([
            f64::from(spawn.at.x) + 0.5,
            f64::from(spawn.at.y),
            f64::from(spawn.at.z) + 0.5,
        ]);
    let yaw = spawn.mob.body.head.body_yaw;
    making::companion(
        spawn,
        entity_types::ZOMBIE_NAUTILUS as u16,
        position,
        yaw,
        (reason, CompanionSeat::Vehicle),
        |nautilus| {
            if reason == SpawnReason::Structure {
                nautilus.body.flags.persistent = true;
            }
        },
    );
}

impl MobType for Drowned {
    /// `Zombie.registerGoals` with the drowned's `addBehaviourGoals`, its amphibious
    /// navigation, and water no malus.
    fn register_goals(&self, setup: &mut Setup) {
        setup.config.navigation = Settings::of(Kind::Amphibious);
        setup.set_malus(PathType::Water, 0.0);
        family::register_base_goals(setup);
        setup.add_goal(1, GoToWaterGoal);
        // `DrownedTridentAttackGoal` and `DrownedAttackGoal`, which need a target.
        setup.add_goal(2, Idle { flags: 0 });
        setup.add_goal(2, Idle { flags: 0 });
        setup.add_goal(5, GoToBeachGoal);
        setup.add_goal(6, SwimUpGoal);
        setup.add_goal(7, RandomStrollGoal::new(1.0));
        // Its targets: N9's.
        for priority in [1, 2, 3, 3, 3, 5] {
            setup.add_target(priority, Idle { flags: 0 });
        }
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        family::walk_target_value(ai, pos)
    }

    fn tick_move_control(&self, ai: &mut Ai<'_, '_>) {
        tick_move_control(ai);
    }

    /// `Drowned.travelInWater`: with its eyes under water and wanting to swim, a
    /// hundredth along its input and its motion kept at nine tenths; else the zombie's.
    fn travel(&self, tick: &MobTick<'_>) -> Travel {
        if water::eye_in_water(tick) && wants_to_swim(&tick.mob) {
            Travel::Swims {
                speed: 0.01,
                sinks: Sinks::Never,
            }
        } else {
            Travel::Living
        }
    }

    /// `Drowned.updateSwimming()`, in its `baseTick`: it swims while its AI runs, its
    /// eyes are under water and it wants to swim.
    fn after_base_tick(&self, tick: &mut MobTick<'_>, _: i32) {
        let swimming = tick.effective_ai() && water::eye_in_water(tick) && wants_to_swim(&tick.mob);
        tick.mob.body.swimming.swimming = swimming;
    }

    /// Whether it was in water at its last tick, which its sounds ask between ticks.
    fn after_tick(&self, tick: &mut MobTick<'_>) {
        let in_water = tick.living.body.in_water;
        family::update(&mut tick.mob, |own| own.in_water = in_water);
    }

    /// `isPushedByFluid()`: not while it swims.
    fn pushed_by_fluid_of(&self, mob: &Mob) -> bool {
        !mob.body.swimming.swimming
    }

    fn after_finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        finalize_spawn(spawn);
    }

    fn set_baby(&self, mob: &mut Mob, baby: bool) {
        family::set_baby(KIND, mob, baby);
    }

    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        family::from_template(KIND, mob, tag);
    }

    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        family::offspring_from_egg(breeding)
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        family::dimensions(mob, 0.775)
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        family::is_baby(mob)
    }

    fn experience_reward(&self, _: u16, mob: &Mob, draws: &mut Random) -> i32 {
        family::experience_reward(mob, draws)
    }

    fn ambient_sound(&self, mob: &Mob) -> Option<i32> {
        Some(if family::read(mob).in_water {
            sounds::ENTITY_DROWNED_AMBIENT_WATER
        } else {
            sounds::ENTITY_DROWNED_AMBIENT
        })
    }

    fn ambient_sound_at(&self, _: &Mob, in_water: bool, _: bool) -> Option<i32> {
        Some(if in_water {
            sounds::ENTITY_DROWNED_AMBIENT_WATER
        } else {
            sounds::ENTITY_DROWNED_AMBIENT
        })
    }

    fn hurt_sound(&self, mob: &Mob, _: &DamageSource) -> Option<i32> {
        Some(if family::read(mob).in_water {
            sounds::ENTITY_DROWNED_HURT_WATER
        } else {
            sounds::ENTITY_DROWNED_HURT
        })
    }

    fn death_sound(&self, mob: &Mob) -> Option<i32> {
        Some(if family::read(mob).in_water {
            sounds::ENTITY_DROWNED_DEATH_WATER
        } else {
            sounds::ENTITY_DROWNED_DEATH
        })
    }

    fn step_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_DROWNED_STEP)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        family::shown(mob)
    }
}
