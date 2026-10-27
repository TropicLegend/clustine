//! What the zombie, the husk, the drowned and the zombie villager share (`Zombie`;
//! `docs/groundwork/n7k-brief.md`, sections 3.4, 11.1, 11.5, 11.8 and 11.12, and the
//! plan's section 10): their state, their synched and saved data, a baby's size, speed
//! and experience, their goals but those of a target and an attack, `finalizeSpawn` with
//! every draw the game makes at easy, `handleAttributes`, the drowning conversion
//! (`ConversionTracker`) and the turtle egg goal (`ZombieAttackTurtleEggGoal`).
//!
//! Named differences: the targets are real (`crate::mobs::ai::targets`, N9f) and the
//! attacks are N9's (each holds its slot with a goal that never starts);
//! `MoveThroughVillageGoal` finds no village, which needs the
//! points of interest of N10, and holds its slot so; `BreakDoorGoal` is never added, as
//! no door is broken below hard and no zombie can break doors at easy; Halloween's
//! pumpkin is never drawn (the plan's section 10); a group of zombies made together
//! (N9's natural spawns) shares no `ZombieGroupData`, as an egg's has none; a chicken
//! already there that a baby rides is not made a jockey (a maker of a mob can seat it
//! on one, not write it); loot is not picked up (no zombie may at easy).

use clustine_blocks::data::is_block;
use clustine_data::{attributes as ids, blocks, entity_types, items};
use clustine_world::{BlockPos, ItemStack, TypeState};
use serde::{Deserialize, Serialize};

use super::animal;
use super::chicken::ChickenState;
use super::tamable::Idle;
use super::{Breeding, CompanionSeat, Dimensions, SpawnReason, Spawning, template};
use crate::api::SOUND_SOURCE_BLOCKS;
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    LookAtPlayerGoal, MoveToBlockGoal, MovingToBlock, RandomLookAroundGoal, RandomStrollGoal,
};
use crate::mobs::ai::maths::reduced_tick_delay;
use crate::mobs::ai::navigation;
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::Class;
use crate::mobs::ai::targets;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::attributes::{ADD_MULTIPLIED_BASE, ADD_MULTIPLIED_TOTAL, ADD_VALUE};
use crate::mobs::body::{Modifier, attribute, ladder};
use crate::mobs::making;
use crate::mobs::tick::{Local, MobTick};
use crate::physics::Vec3;
use crate::physics::aabb::Aabb;
use crate::region::Random;
use crate::shown::DataShown;

/// The synched indices of `DATA_BABY_ID`, `DATA_SPECIAL_TYPE_ID` and
/// `DATA_DROWNED_CONVERSION_ID`.
pub(crate) const BABY_INDEX: u8 = 16;
pub(crate) const SPECIAL_TYPE_INDEX: u8 = 17;
pub(crate) const DROWNED_CONVERSION_INDEX: u8 = 18;

/// The version of [`ZombieState`]'s encoding.
const VERSION: u8 = 1;

/// The ticks a zombie's eyes are under water before it starts to turn
/// (`InWaterTime`'s total) and the ticks it then takes (`DrownedConversionTime`'s).
pub(crate) const IN_WATER_TIME: i32 = 600;
pub(crate) const CONVERSION_TIME: i32 = 300;

/// `SPEED_MODIFIER_BABY`: a baby a half faster.
pub(crate) const BABY_SPEED: &str = "minecraft:baby";
/// The modifiers of `handleAttributes`.
const RANDOM_SPAWN_BONUS: &str = "minecraft:random_spawn_bonus";
const ZOMBIE_RANDOM_SPAWN_BONUS: &str = "minecraft:zombie_random_spawn_bonus";
const LEADER_ZOMBIE_BONUS: &str = "minecraft:leader_zombie_bonus";

/// `ConversionTracker`'s state (`n7k-brief.md` 11.5): `afflictionTime`,
/// `conversionTime` and the synched converting flag it is handed (the zombie's 18).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Tracker {
    pub(crate) affliction: i32,
    pub(crate) conversion: i32,
    pub(crate) converting: bool,
}

impl Tracker {
    /// `ConversionTracker.tick()` of a living mob with AI, `afflicted` as its condition
    /// holds now (a zombie's eyes under water): whether it converts in this tick.
    pub(crate) fn tick(&mut self, afflicted: bool) -> bool {
        if !afflicted {
            self.affliction = -1;
            self.converting = false;
            return false;
        }
        if self.converting {
            self.conversion -= 1;
            return self.conversion < 0;
        }
        self.affliction += 1;
        if self.affliction >= IN_WATER_TIME {
            self.conversion = CONVERSION_TIME;
            self.converting = true;
        }
        false
    }
}

/// What a zombie villager keeps beyond a zombie (`ZombieVillager`): its `VillagerData`
/// (type and profession by their registry ids, and its level), whether it is finalised,
/// the profession its construction drew (the synched value's default, which a client
/// being paired is not sent), its cure's countdown (`villagerConversionTime`, -1 when it
/// is not curing) and its `Xp`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct VillagerState {
    pub(crate) kind: i32,
    pub(crate) profession: i32,
    pub(crate) level: i32,
    pub(crate) finalised: bool,
    pub(crate) drawn_profession: i32,
    pub(crate) conversion: i32,
    pub(crate) xp: i32,
}

/// What the zombie family keeps: the baby flag, `canBreakDoors`, the drowning tracker
/// (saved by the drowned too, which never ticks it), the drowned's `searchingForLand`
/// and whether it was in water at its last tick (which its sounds read between ticks),
/// and a zombie villager's own.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ZombieState {
    pub(crate) baby: bool,
    pub(crate) can_break_doors: bool,
    pub(crate) drowning: Tracker,
    pub(crate) searching_for_land: bool,
    pub(crate) in_water: bool,
    pub(crate) villager: Option<VillagerState>,
}

/// The family's state of `mob`; a record of nothing, or one this build cannot read, as a
/// fresh zombie's.
pub(crate) fn read(mob: &Mob) -> ZombieState {
    read_state(&mob.state)
}

fn read_state(state: &TypeState) -> ZombieState {
    if state.bytes.is_empty() || state.version != VERSION {
        return ZombieState::default();
    }
    postcard::from_bytes(&state.bytes).unwrap_or_default()
}

/// `mob`'s state made `own`: nothing for a fresh zombie's.
pub(crate) fn write(mob: &mut Mob, own: &ZombieState) {
    mob.state = if *own == ZombieState::default() {
        TypeState::default()
    } else {
        TypeState {
            version: VERSION,
            bytes: postcard::to_allocvec(own).expect("a type state is encoded into memory"),
        }
    };
}

/// Changes the family's state of `mob`.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut ZombieState)) {
    let mut own = read(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        write(mob, &own);
    }
}

/// `isBaby()`.
pub(crate) fn is_baby(mob: &Mob) -> bool {
    read(mob).baby
}

/// `Zombie.setBaby(baby)` of a mob of the type `kind`: the flag, and the baby's speed
/// taken off and, for a baby, put on (`getAttribute` makes the speed's instance).
pub(crate) fn set_baby(kind: u16, mob: &mut Mob, baby: bool) {
    update(mob, |own| own.baby = baby);
    if let Some(speed) = mob
        .body
        .attributes
        .instance(kind, attribute(ids::MOVEMENT_SPEED))
    {
        speed.remove_modifier(BABY_SPEED);
        if baby {
            speed.add_modifier(Modifier {
                id: BABY_SPEED.to_owned(),
                amount: 0.5,
                operation: ADD_MULTIPLIED_BASE,
            });
        }
    }
}

/// A baby's size (`BABY_DIMENSIONS`, `scalable(0.49, 0.98)`), its eyes at `eye_height`:
/// the zombie's and the drowned's 0.775, the husk's 0.825, a zombie villager's 0.67.
pub(crate) fn dimensions(mob: &Mob, eye_height: f32) -> Option<Dimensions> {
    is_baby(mob).then_some(Dimensions {
        width: 0.49,
        height: 0.98,
        eye_height,
    })
}

/// The zombie's synched values: the baby, the special type (0, set by nothing) and the
/// drowning's converting flag.
pub(crate) fn shown(mob: &Mob) -> Vec<(u8, DataShown, bool)> {
    let own = read(mob);
    vec![
        (BABY_INDEX, DataShown::Boolean(own.baby), !own.baby),
        (SPECIAL_TYPE_INDEX, DataShown::Int(0), true),
        (
            DROWNED_CONVERSION_INDEX,
            DataShown::Boolean(own.drowning.converting),
            !own.drowning.converting,
        ),
    ]
}

/// `Monster.getWalkTargetValue(pos)`: darker is better.
pub(crate) fn walk_target_value(ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
    -animal::light_cost(ai, pos)
}

/// `Zombie.getBaseExperienceReward`: `Monster`'s 5, two and a half times as much for a
/// baby (12), and `Mob`'s one to three more of its own random for each slot but the
/// saddle that holds what does not surely drop.
pub(crate) fn experience_reward(mob: &Mob, draws: &mut Random) -> i32 {
    let mut reward = if is_baby(mob) { 12 } else { 5 };
    let equipment = &mob.body.equipment;
    for slot in 0..equipment.slots.len() {
        if slot == usize::from(slots::SADDLE) || equipment.slots[slot].is_none() {
            continue;
        }
        if equipment.drop_chance(slot) <= 1.0 {
            reward += 1 + draws.next_int(3) as i32;
        }
    }
    reward
}

/// `Zombie.registerGoals` up to `addBehaviourGoals`: the turtle egg (4), the looks (8).
pub(crate) fn register_base_goals(setup: &mut Setup) {
    setup.add_goal(4, ZombieAttackTurtleEggGoal);
    setup.add_goal(8, LookAtPlayerGoal::new(Class::Player, 8.0));
    setup.add_goal(8, RandomLookAroundGoal);
}

/// `Zombie.registerGoals` with its `addBehaviourGoals` (the zombie's, the husk's and the
/// zombie villager's): the spear (2) and the attack (3), which need a target; through a
/// village (6), which finds none; the stroll avoiding water (7); and the targets.
pub(crate) fn register_goals(setup: &mut Setup) {
    register_base_goals(setup);
    setup.add_goal(2, Idle { flags: 0 });
    setup.add_goal(3, Idle { flags: 0 });
    setup.add_goal(6, Idle { flags: 0 });
    setup.add_goal(7, RandomStrollGoal::avoiding_water(1.0));
    targets::register(setup, targets::ZOMBIE);
}

/// The weapon of `Zombie.populateDefaultEquipmentSlots` after `Mob`'s armour: one time in
/// a hundred (twenty on hard) an iron sword, spear or shovel, by `nextInt(6)`.
pub(crate) fn zombie_equipment(spawn: &mut Spawning<'_>, special: f32) {
    let hard = clustine_world::Difficulty::WORLD == clustine_world::Difficulty::Hard;
    making::populate_default_equipment_slots(spawn.mob, spawn.level, special, hard);
    let chance = if hard { 0.05 } else { 0.01 };
    if spawn.level.next_float() < chance {
        let item = match spawn.level.next_int(6) {
            0 => items::IRON_SWORD,
            1 => items::IRON_SPEAR,
            _ => items::IRON_SHOVEL,
        };
        spawn.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] =
            Some(ItemStack::new(item, 1));
    }
}

/// Where the mob being made stands: what its maker says, else the bottom middle of its
/// block.
fn position_of(spawn: &Spawning<'_>) -> Vec3 {
    spawn
        .companions
        .as_deref()
        .and_then(|companions| companions.position())
        .unwrap_or([
            f64::from(spawn.at.x) + 0.5,
            f64::from(spawn.at.y),
            f64::from(spawn.at.z) + 0.5,
        ])
}

/// `Zombie.finalizeSpawn` after `Monster`'s (the Mob tail, which the maker made first):
/// section 5's **Z** of the brief, steps 2 to 8, with `equip` the type's
/// `populateDefaultEquipmentSlots` (the zombie's, or the drowned's own), for the mob of
/// the type `kind`.
pub(crate) fn finalize_spawn(
    kind: u16,
    spawn: &mut Spawning<'_>,
    equip: fn(&mut Spawning<'_>, f32),
) {
    let special = spawn.difficulty.1;
    let conversion = spawn.reason == SpawnReason::Conversion;
    if !conversion {
        spawn.mob.body.flags.picks_up_loot = spawn.level.next_float() < 0.55 * special;
    }
    // `ZombieGroupData(getSpawnAsBabyOdds(random), true)` where no group is handed in; a
    // group of another class skips the baby, the jockey, the doors and the equipment.
    if spawn.group.is_none() {
        let baby = spawn.level.next_float() < 0.05;
        if baby {
            set_baby(kind, spawn.mob, true);
            jockey(spawn);
        }
        let doors = spawn.level.next_float() < special * 0.1;
        update(spawn.mob, |own| own.can_break_doors = doors);
        if !conversion {
            equip(spawn, special);
            making::populate_default_equipment_enchantments(spawn.mob, spawn.level, special);
        }
    }
    // `SpecialDates.isHalloween()`: never (the plan's section 10).
    let reason = spawn.reason;
    handle_attributes(kind, spawn.mob, spawn.own, special, reason);
}

/// A baby's chicken (`Zombie.finalizeSpawn`, Zombie.txt 106–296): one time in twenty it
/// rides the first chicken within 5, 3 and 5 of its box that carries nobody and rides
/// nothing, if there is one; else one time in twenty a new chicken is made a jockey
/// under it, finalised from the level's random as the game's `JOCKEY`.
fn jockey(spawn: &mut Spawning<'_>) {
    let position = position_of(spawn);
    let yaw = spawn.mob.body.head.body_yaw;
    let chicken = entity_types::CHICKEN as u16;
    if f64::from(spawn.level.next_float()) < 0.05 {
        // The baby's box, its size set by `setBaby` already.
        let bounds = Aabb::of_entity(position, 0.49, 0.98).inflate([5.0, 3.0, 5.0]);
        if let Some(companions) = spawn.companions.as_deref_mut() {
            let found = companions
                .nearby(&bounds)
                .into_iter()
                .find(|&(_, kind, carries)| kind == chicken && !carries);
            if let Some((key, _, _)) = found {
                companions.ride(key);
            }
        }
        return;
    }
    if f64::from(spawn.level.next_float()) < 0.05 {
        let reason = SpawnReason::Jockey;
        making::companion(
            spawn,
            chicken,
            position,
            yaw,
            (reason, CompanionSeat::Vehicle),
            |made| {
                let (shared, mut own) = animal::read::<ChickenState>(&made.state);
                own.jockey = true;
                made.state = animal::write(&shared, &own);
            },
        );
    }
}

/// `Zombie.handleAttributes(special, reason)` of the mob of the type `kind`, from its own
/// stream `own`: its reinforcements' chance, a knockback bonus, a follow range's bonus
/// above a draw of one and a half times `special`, and a leader one time in
/// `special × 0.05` (four draws at easy, six for a leader).
pub(crate) fn handle_attributes(
    kind: u16,
    mob: &mut Mob,
    own: &mut Random,
    special: f32,
    reason: SpawnReason,
) {
    let attributes = &mut mob.body.attributes;
    let chance = own.next_double() * 0.100_000_001_490_116_12;
    if let Some(reinforcements) = attributes.instance(kind, attribute(ids::SPAWN_REINFORCEMENTS)) {
        reinforcements.base = chance;
    }
    let bonus = own.next_double() * 0.050_000_000_745_058_06;
    put_modifier(
        mob,
        kind,
        ids::KNOCKBACK_RESISTANCE,
        RANDOM_SPAWN_BONUS,
        bonus,
        ADD_VALUE,
    );
    let follow = own.next_double() * 1.5 * f64::from(special);
    if follow > 1.0 {
        put_modifier(
            mob,
            kind,
            ids::FOLLOW_RANGE,
            ZOMBIE_RANDOM_SPAWN_BONUS,
            follow,
            ADD_MULTIPLIED_TOTAL,
        );
    }
    if own.next_float() < special * 0.05 {
        let more = own.next_double() * 0.25 + 0.5;
        put_modifier(
            mob,
            kind,
            ids::SPAWN_REINFORCEMENTS,
            LEADER_ZOMBIE_BONUS,
            more,
            ADD_VALUE,
        );
        let health = own.next_double() * 3.0 + 1.0;
        put_modifier(
            mob,
            kind,
            ids::MAX_HEALTH,
            LEADER_ZOMBIE_BONUS,
            health,
            ADD_MULTIPLIED_TOTAL,
        );
        if reason != SpawnReason::Conversion {
            mob.body.vitals.health =
                mob.body.attributes.value(kind, attribute(ids::MAX_HEALTH)) as f32;
        }
        update(mob, |own| own.can_break_doors = true);
    }
}

/// `addOrReplacePermanentModifier` of the attribute `id`.
fn put_modifier(mob: &mut Mob, kind: u16, id: i32, name: &str, amount: f64, operation: u8) {
    if let Some(instance) = mob.body.attributes.instance(kind, attribute(id)) {
        instance.remove_modifier(name);
        instance.add_modifier(Modifier {
            id: name.to_owned(),
            amount,
            operation,
        });
    }
}

/// `ConversionTracker.tick()` of the drowning, after the whole of `Zombie.tick`'s
/// superclass: a living zombie or husk with AI whose eyes are under water counts to 600,
/// then turns over 300 more into the type `into`, heard as the level event `sound` at
/// the new mob; out of water it starts again.
pub(crate) fn tick_drowning(tick: &mut MobTick<'_>, into: i32, sound: i32) {
    if tick.mob.body.vitals.dying() || tick.out.removed.is_some() || tick.mob.body.flags.no_ai {
        return;
    }
    let afflicted = super::water::eye_in_water(tick);
    let mut own = read(&tick.mob);
    let converts = own.drowning.tick(afflicted);
    write(&mut tick.mob, &own);
    if converts {
        tick.out.local.push(Local::Convert {
            into: into as u16,
            keep_equipment: true,
            sound: Some(sound),
        });
    }
}

/// `onOffspringSpawnedFromEgg` of a spawn egg used on a mob of the family: a baby of its
/// own type, as `EntityType.create` makes it, that picks up loot one time in
/// `0.55 × special` (at easy never) by the parent's own random. Its `setBaby(true)` is
/// the egg's, after.
pub(crate) fn offspring_from_egg(breeding: &mut Breeding<'_>) -> Option<Mob> {
    let mut child = Mob::fresh();
    // The special multiplier at the child's block: the world's easy gives 0.
    let special = 0.0;
    child.body.flags.picks_up_loot = breeding.draws.next_float() < 0.55 * special;
    Some(child)
}

/// What `Zombie.readAdditionalSaveData` reads of a structure template's tag: `IsBaby`
/// (its speed with it), `CanBreakDoors`, and the drowning's `InWaterTime` (0 by default)
/// and `DrownedConversionTime` (a conversion under way where it is not -1).
pub(crate) fn from_template(kind: u16, mob: &mut Mob, tag: &[u8]) {
    set_baby(kind, mob, template::boolean(tag, "IsBaby").unwrap_or(false));
    let doors = template::boolean(tag, "CanBreakDoors").unwrap_or(false);
    let affliction = template::int(tag, "InWaterTime").unwrap_or(0);
    let conversion = template::int(tag, "DrownedConversionTime").unwrap_or(-1);
    update(mob, |own| {
        own.can_break_doors = doors;
        own.drowning.affliction = affliction;
        if conversion == -1 {
            own.drowning.converting = false;
        } else {
            own.drowning.conversion = conversion;
            own.drowning.converting = true;
        }
    });
}

/// Whether the mob type `kind` is of the family (`instanceof Zombie`).
#[cfg_attr(not(test), expect(dead_code, reason = "the tests ask it"))]
pub(crate) fn is_zombie(kind: u16) -> bool {
    matches!(
        i32::from(kind),
        entity_types::ZOMBIE
            | entity_types::HUSK
            | entity_types::DROWNED
            | entity_types::ZOMBIE_VILLAGER
    )
}

/// `ZombieAttackTurtleEggGoal`'s state: `MoveToBlockGoal`'s and `RemoveBlockGoal`'s
/// `ticksSinceReachedGoal`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Trampling {
    pub(crate) moving: MovingToBlock,
    pub(crate) since_reached: i32,
}

/// `Zombie.ZombieAttackTurtleEggGoal(zombie, 1.0, 3)`, a `RemoveBlockGoal` of turtle
/// eggs (`n7k-brief.md` 11.8): with no egg within 24 blocks across and 3 up or down it
/// waits 100 to 200 ticks by a draw of its own; with one it walks to it, hops on it, and
/// a little after its sixtieth tick there tramples it. It needs no target. `mob_griefing`
/// is on, as Clustine's world has it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ZombieAttackTurtleEggGoal;

/// `acceptedDistance()`.
const ACCEPTED_DISTANCE: f64 = 1.14;

impl ZombieAttackTurtleEggGoal {
    fn moving() -> MoveToBlockGoal {
        MoveToBlockGoal {
            speed: 1.0,
            search_range: 24,
            vertical_search_range: 3,
            vertical_search_start: 0,
            valid: egg_target,
            next_start: None,
        }
    }
}

/// `RemoveBlockGoal.isValidTarget`: a turtle egg in a loaded chunk with air in the two
/// blocks above it.
fn egg_target(ai: &Ai<'_, '_>, pos: BlockPos) -> bool {
    let world = ai.tick.world;
    world.is_loaded(pos)
        && is_block(world.state(pos), blocks::TURTLE_EGG)
        && world.state(pos.offset(0, 1, 0)).props().is_air()
        && world.state(pos.offset(0, 2, 0)).props().is_air()
}

/// `RemoveBlockGoal.getPosWithBlock(pos)`: `pos` if it is an egg, else the first egg of
/// the blocks below, west, east, north and south of it and two below.
fn egg_near(ai: &Ai<'_, '_>, pos: BlockPos) -> Option<BlockPos> {
    let world = ai.tick.world;
    let egg = |at: BlockPos| is_block(world.state(at), blocks::TURTLE_EGG);
    if egg(pos) {
        return Some(pos);
    }
    [
        pos.offset(0, -1, 0),
        pos.offset(-1, 0, 0),
        pos.offset(1, 0, 0),
        pos.offset(0, 0, -1),
        pos.offset(0, 0, 1),
        pos.offset(0, -2, 0),
    ]
    .into_iter()
    .find(|&at| egg(at))
}

/// The middle of the block `pos`, where a sound played at a block is heard.
fn centre(pos: BlockPos) -> Vec3 {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5,
        f64::from(pos.z) + 0.5,
    ]
}

impl Goal for ZombieAttackTurtleEggGoal {
    type State = Trampling;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::JUMP
    }

    fn every_tick(&self) -> bool {
        true
    }

    /// `RemoveBlockGoal.canUse()`: its countdown, then the search; found, the next
    /// search in a second, else in 100 to 200 ticks by a draw.
    fn can_use(&self, state: &mut Trampling, ai: &mut Ai<'_, '_>) -> bool {
        let moving = &mut state.moving;
        if moving.next_start_tick > 0 {
            moving.next_start_tick -= 1;
            return false;
        }
        if Self::moving().find_nearest(moving, ai) {
            moving.next_start_tick = reduced_tick_delay(20);
            return true;
        }
        moving.next_start_tick = reduced_tick_delay(200 + ai.tick.draws.next_int(200) as i32);
        false
    }

    fn can_continue(&self, state: &mut Trampling, ai: &mut Ai<'_, '_>) -> bool {
        Self::moving().can_continue(&mut state.moving, ai)
    }

    fn start(&self, state: &mut Trampling, ai: &mut Ai<'_, '_>) {
        Self::moving().start(&mut state.moving, ai);
        state.since_reached = 0;
    }

    /// `RemoveBlockGoal.stop()`: its fall is taken as one block's.
    fn stop(&self, _: &mut Trampling, ai: &mut Ai<'_, '_>) {
        ai.tick.living.body.fall_distance = 1.0;
    }

    fn tick(&self, state: &mut Trampling, ai: &mut Ai<'_, '_>) {
        // `MoveToBlockGoal.tick()`, within 1.14 of the middle of the block above the egg.
        let block = state.moving.block.unwrap_or(BlockPos::new(0, 0, 0));
        let target = block.offset(0, 1, 0);
        let [x, y, z] = ai.position();
        let (dx, dy, dz) = (
            f64::from(target.x) + 0.5 - x,
            f64::from(target.y) + 0.5 - y,
            f64::from(target.z) + 0.5 - z,
        );
        if dx * dx + dy * dy + dz * dz >= ACCEPTED_DISTANCE * ACCEPTED_DISTANCE {
            state.moving.reached = false;
            state.moving.try_ticks += 1;
            if state.moving.try_ticks % 40 == 0 {
                navigation::move_to(
                    ai,
                    [
                        f64::from(target.x) + 0.5,
                        f64::from(target.y),
                        f64::from(target.z) + 0.5,
                    ],
                    1.0,
                );
            }
        } else {
            state.moving.reached = true;
            state.moving.try_ticks -= 1;
        }
        // `RemoveBlockGoal.tick()`'s own.
        let Some(egg) = egg_near(ai, ai.block_position()) else {
            return;
        };
        if !state.moving.reached {
            return;
        }
        let kind = ai.tick.kind;
        if state.since_reached > 0 {
            let motion = &mut ai.tick.living.body.motion;
            motion[1] = 0.3;
            // The egg's crumbs (`ItemParticleOption`), three draws each a spread.
            for _ in 0..3 {
                let _spread = ai.tick.draws.next_float();
            }
        }
        if state.since_reached % 2 == 0 {
            ai.tick.living.body.motion[1] = -0.3;
            if state.since_reached % 6 == 0 {
                // `playDestroyProgressSound(level, blockPos)`, at the block it went to.
                let pitch = 0.9 + ai.tick.draws.next_float() * 0.2;
                ai.tick.out.local.push(Local::Sound {
                    sound: clustine_data::sounds::ENTITY_ZOMBIE_DESTROY_EGG,
                    source: ladder::sound_source(kind),
                    at: centre(block),
                    volume: 0.5,
                    pitch,
                });
            }
        }
        if state.since_reached > 60 {
            // `level.removeBlock(pos, false)`, then twenty puffs of three draws each.
            ai.tick.out.local.push(Local::SetBlock {
                pos: egg,
                state: blocks::AIR,
                flags: 3,
            });
            for _ in 0..20 {
                for _ in 0..3 {
                    let _speed = ai.tick.draws.next_gaussian();
                }
            }
            // `playBreakSound`: the level's draw, at its place in the mob's stream.
            let pitch = 0.9 + ai.tick.draws.next_float() * 0.2;
            ai.tick.out.local.push(Local::Sound {
                sound: clustine_data::sounds::ENTITY_TURTLE_EGG_BREAK,
                source: SOUND_SOURCE_BLOCKS,
                at: centre(egg),
                volume: 0.7,
                pitch,
            });
        }
        state.since_reached += 1;
    }
}
