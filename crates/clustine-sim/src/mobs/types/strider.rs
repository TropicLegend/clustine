//! The strider (`Strider`, d Strider.txt; the brief's section 5.9): family E4 of P12
//! (`docs/groundwork/p-plan.md`, sections 18.3, 18.5 and 18.6).
//!
//! An animal of the Nether that walks on lava: a lava source under it is a half block
//! (`canStandOnFluid`, the contract's collision, travel and path), and it rises through
//! lava over its head (`floatStrider`). Out of `#strider_warm_blocks` and lava it is
//! cold: shown shaking (`DATA_SUFFOCATING`) and 34% slower (`suffocating`, −0.34 of its
//! base), unless it rides a strider that is warm. It panics, breeds with warped fungus,
//! is tempted by it and by the warped fungus on a stick, walks back to lava within 8
//! blocks, and is hurt by water. A saddled one takes a player on, who steers it holding
//! a warped fungus on a stick (the ridden speed, `speed × (suffocating ? 0.35 : 0.55)` and
//! its boost, is the driver's client's) and boosts it with a use of the stick. It spawns
//! in lava (`IN_LAVA`, climbing to air above), one time in 30 with a zombified piglin
//! rider holding a warped fungus on a stick and a saddle on itself, else one time in 10
//! with a baby strider on it.
//!
//! Named differences: the zombified piglin of a jockey draws its own chance of being a
//! baby within its `finalizeSpawn` (the game's strider draws it before, into a
//! `ZombieGroupData` the family has no form of) and so may draw for a chicken it never
//! makes; its steps sound every whole block, not every 0.6 (`nextStep`, the body's);
//! its fall is not forgotten while in lava on its driver's moves (the vehicle's check),
//! nor its riders let off by its own rule (`getDismountLocationForPassenger`), which have no hook; the
//! generator's spawning makes none yet (`docs/groundwork/p-plan.md`, the lead's: the
//! Nether's ceiling in `getTopNonCollidingPos`).

use clustine_blocks::data::{entity_in_tag, fluid_in_tag, in_tag, is_block};
use clustine_data::{BlockState, PathType, attributes as ids, blocks, entity_types, items, sounds};
use clustine_world::generation::Draws;
use clustine_world::{BlockPos, Difficulty, ItemStack};
use serde::{Deserialize, Serialize};

use super::animal::{self, Own};
use super::pig::Boost;
use super::{
    AgeableGroup, Breeding, Companion, CompanionSeat, Did, Dimensions, Interaction, MobType,
    SpawnGroup, SpawnReason, Spawning, Used, cow,
};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    Items, LookAtPlayerGoal, MoveToBlockGoal, MovingToBlock, PanicGoal, RandomLookAroundGoal,
    RandomStrollGoal, TemptGoal,
};
use crate::mobs::ai::selector::Goal;
use crate::mobs::ai::sensing::Class;
use crate::mobs::ai::{Ai, Setup, navigation};
use crate::mobs::body::attributes::ADD_MULTIPLIED_BASE;
use crate::mobs::body::hurt::{DamageSource, DamageType};
use crate::mobs::body::{Modifier, attribute};
use crate::mobs::making;
use crate::mobs::tick::MobTick;
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::shown::DataShown;
use crate::spawning::rules::SpawnWorld;

/// Whether the type is whole, so that `built()` gives it.
pub(crate) const BUILT: bool = true;

/// The strider's type, by id.
const KIND: u16 = entity_types::STRIDER as u16;

/// `ItemTags.STRIDER_FOOD`: warped fungus.
const FOOD: Items = Items::Tag("minecraft:strider_food");

/// `ItemTags.STRIDER_TEMPT_ITEMS`: warped fungus and the warped fungus on a stick.
const TEMPT: Items = Items::Tag("minecraft:strider_tempt_items");

/// The striders, whom one looks at (`LookAtPlayerGoal(this, Strider.class, 8)`).
const STRIDERS: &[u16] = &[KIND];

/// `BABY_DIMENSIONS` (Strider.txt 1211 on, `<clinit>`): 0.45 by 0.85, its eyes at 0.4375.
const BABY: Dimensions = Dimensions {
    width: 0.45,
    height: 0.85,
    eye_height: 0.4375,
};

/// A baby's passenger attachment (`BABY_DIMENSIONS`' 0.65625).
const BABY_SEAT: f64 = 0.656_25;

/// `SUFFOCATING_MODIFIER`: `minecraft:suffocating`, −0.34 (a float widened) of the base.
const SUFFOCATING: &str = "minecraft:suffocating";
const SUFFOCATING_AMOUNT: f64 = -0.340_000_003_576_278_7;

/// The synced data's indices: `DATA_BOOST_TIME` and `DATA_SUFFOCATING`.
const BOOST_INDEX: u8 = 18;
const SUFFOCATING_INDEX: u8 = 19;

/// The places of the panic goal and the tempt goal among its goals, which its sounds
/// read (`isPanicking`, `isBeingTempted`).
const PANIC_GOAL: u16 = 0;
const TEMPT_GOAL: u16 = 2;

/// What a strider keeps beyond an animal: whether it is cold (`DATA_SUFFOCATING`, and
/// its speed's modifier with it), whether it is in lava and its eyes are, as its last
/// `baseTick` found them (what its steps and `canAddPassenger` read between ticks), and
/// its boost (`ItemBasedSteering`, durable as the pig's is, so that a crossing of a
/// line keeps it).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct StriderState {
    pub(crate) suffocating: bool,
    pub(crate) in_lava: bool,
    pub(crate) eye_in_lava: bool,
    pub(crate) boost: Boost,
}

impl Own for StriderState {}

/// The strider's own state.
pub(crate) fn state_of(mob: &Mob) -> StriderState {
    animal::read::<StriderState>(&mob.state).1
}

/// Keeps what `change` does to the strider's own state, writing the record only where it
/// changed.
fn change(mob: &mut Mob, change: impl FnOnce(&mut StriderState)) {
    let (shared, mut own) = animal::read::<StriderState>(&mob.state);
    let before = own;
    change(&mut own);
    if own != before {
        mob.state = animal::write(&shared, &own);
    }
}

/// `Mob.isSaddled()`: a saddle in its saddle slot.
fn saddled(mob: &Mob) -> bool {
    mob.body.equipment.slots[slots::SADDLE as usize]
        .as_ref()
        .is_some_and(|stack| stack.count > 0 && stack.item == items::SADDLE)
}

/// Whether a goal at `slot` of its goal selector runs.
fn running(mob: &Mob, slot: u16) -> bool {
    mob.mind.goals.0.running.binary_search(&slot).is_ok()
}

/// `Strider.setSuffocating(cold)` (Strider.txt 255–280): the flag, and its speed's
/// modifier added where cold and taken off where not.
fn set_suffocating(mob: &mut Mob, cold: bool) {
    change(mob, |own| own.suffocating = cold);
    let speed = attribute(ids::MOVEMENT_SPEED);
    if cold {
        if let Some(instance) = mob.body.attributes.instance(KIND, speed) {
            instance.add_modifier(Modifier {
                id: SUFFOCATING.to_owned(),
                amount: SUFFOCATING_AMOUNT,
                operation: ADD_MULTIPLIED_BASE,
            });
        }
    } else if mob
        .body
        .attributes
        .made(speed)
        .is_some_and(|instance| instance.has_modifier(SUFFOCATING))
        && let Some(instance) = mob.body.attributes.instance(KIND, speed)
    {
        instance.remove_modifier(SUFFOCATING);
    }
}

/// `Strider.boost()`, `ItemBasedSteering.boost(random)`: refused while it boosts; else it
/// boosts from 0 for `nextInt(841) + 140` ticks of its own random. Whether it began.
pub(crate) fn boost(mob: &mut Mob, draws: &mut crate::region::Random) -> bool {
    if state_of(mob).boost.boosting {
        return false;
    }
    let total = draws.next_int(841) as i32 + 140;
    change(mob, |own| {
        own.boost = Boost {
            boosting: true,
            time: 0,
            total,
        };
    });
    true
}

/// Whether `state` holds lava (`getFluidState().is(FluidTags.LAVA)`).
fn lava(state: BlockState) -> bool {
    fluid_in_tag(state, "minecraft:lava")
}

/// `checkStriderSpawnRules` (Strider.txt 56–76) of what `block` reads: up from `pos`
/// through every lava block to the first that is not lava, which must be air. A block
/// the world cannot read refuses it. No draw.
pub(crate) fn climbs_to_air(block: impl Fn(BlockPos) -> Option<BlockState>, pos: BlockPos) -> bool {
    let mut at = pos;
    loop {
        at = at.offset(0, 1, 0);
        match block(at) {
            Some(state) if lava(state) => continue,
            Some(state) => return state.props().is_air(),
            None => return false,
        }
    }
}

/// The type's spawn rule at a natural reason (`SpawnPlacements`' predicate,
/// `checkStriderSpawnRules`), which `spawning::rules` asks.
pub(crate) fn check_spawn_rules(
    world: &dyn SpawnWorld,
    pos: BlockPos,
    draws: &mut dyn Draws,
) -> bool {
    let _ = draws;
    climbs_to_air(|at| world.block(at), pos)
}

/// `Entity.isEyeInFluid(LAVA)` as `baseTick`'s `updateFluidOnEyes` finds it: lava at the
/// block of its eyes less 0.11111111, whose surface is above that height.
fn eye_in_lava(tick: &MobTick<'_>) -> bool {
    let [x, y, z] = tick.living.body.position;
    let eye = y + f64::from(tick.living.eye_height) - 0.111_111_11;
    let at = crate::physics::containing([x, eye, z]);
    let state = tick.world.state(at);
    if !lava(state) {
        return false;
    }
    let fluid = clustine_blocks::fluid::FluidState::of(state);
    let height = clustine_blocks::fluid::height(&mut crate::physics::Rules(tick.world), fluid, at);
    f64::from(at.y) + f64::from(height) > eye
}

/// Whether the vehicle it rides is a strider that is not cold (`tick`, Strider.txt
/// 707–792: its bytecode 122–156).
fn rides_a_warm_strider(tick: &MobTick<'_>) -> bool {
    if tick.vehicle != Some(KIND) {
        return false;
    }
    let Some(vehicle) = tick
        .world
        .entity(tick.key)
        .and_then(|seen| seen.entity.riding.vehicle)
    else {
        return false;
    };
    tick.world
        .entity(vehicle)
        .and_then(|seen| Mob::decode(&seen.entity.data).ok())
        .is_some_and(|mob| !state_of(&mob).suffocating)
}

/// `Strider.tick`'s head (Strider.txt 707–792): one time in 140 while tempted its happy
/// sound, else one time in 60 while it panics its retreat; then, with its AI, cold where
/// neither its block, the block under it (`getBlockStateOnLegacy`) nor anything of its
/// box is warm, and it rides no warm strider.
fn before_tick(tick: &mut MobTick<'_>) {
    if running(&tick.mob, TEMPT_GOAL) && tick.draws.next_int(140) == 0 {
        crate::mobs::body::make_sound(tick, Some(sounds::ENTITY_STRIDER_HAPPY), 1.0);
    } else if running(&tick.mob, PANIC_GOAL) && tick.draws.next_int(60) == 0 {
        crate::mobs::body::make_sound(tick, Some(sounds::ENTITY_STRIDER_RETREAT), 1.0);
    }
    if tick.mob.body.flags.no_ai {
        return;
    }
    let world = tick.world;
    let at = world.state(tick.living.body.block_position());
    let under = world.state(tick.living.body.on_pos_legacy(world));
    let warm = in_tag(at, "minecraft:strider_warm_blocks")
        || in_tag(under, "minecraft:strider_warm_blocks")
        || tick.living.fluids.lava_height > 0.0;
    let carried_warm = rides_a_warm_strider(tick);
    set_suffocating(&mut tick.mob, !warm && !carried_warm);
}

/// `Strider.floatStrider` (Strider.txt 812–852), after the living tick: in lava, on the
/// ground where it stands above the half block of its liquid shape (`Block.column(16, 0,
/// 8)`, `CollisionContext.isAbove`) with no lava over its block; else rising, its motion
/// halved and 0.05 up.
fn float_strider(tick: &mut MobTick<'_>) {
    if !tick.living.in_lava() {
        return;
    }
    let body = &tick.living.body;
    let block = body.block_position();
    let above = f64::from(block.y) + 0.5 - f64::from(1.0e-5_f32) < body.position[1];
    let lava_over = lava(tick.world.state(block.offset(0, 1, 0)));
    if above && !lava_over {
        tick.living.body.on_ground = true;
    } else {
        let [x, y, z] = tick.living.body.motion;
        tick.living.body.motion = [x * 0.5, y * 0.5 + 0.05, z * 0.5];
    }
}

/// `StriderGoToLavaGoal`'s target (Strider$StriderGoToLavaGoal.txt 69–87): a lava block
/// with room to walk above it.
fn lava_target(ai: &Ai<'_, '_>, pos: BlockPos) -> bool {
    let world = ai.tick.world;
    is_block(world.state(pos), blocks::LAVA)
        && world
            .state(pos.offset(0, 1, 0))
            .is_pathfindable(clustine_data::PathComputation::Land)
}

/// `StriderGoToLavaGoal(strider, 1.0)`: out of lava, to lava within 8 blocks and 2 up or
/// down; its path made again every 20 ticks toward the lava block itself
/// (`getMoveToTarget`), and held only while it is out of lava and the lava is there.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GoToLavaGoal;

impl GoToLavaGoal {
    fn inner() -> MoveToBlockGoal {
        MoveToBlockGoal {
            speed: 1.0,
            search_range: 8,
            vertical_search_range: 2,
            vertical_search_start: 0,
            valid: lava_target,
            next_start: None,
        }
    }
}

impl Goal for GoToLavaGoal {
    type State = MovingToBlock;

    fn flags(&self) -> u8 {
        Self::inner().flags()
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) -> bool {
        !ai.in_lava() && Self::inner().can_use(state, ai)
    }

    fn can_continue(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) -> bool {
        let block = state.block.unwrap_or(BlockPos::new(0, 0, 0));
        !ai.in_lava() && lava_target(ai, block)
    }

    fn start(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) {
        Self::inner().start(state, ai);
    }

    /// `MoveToBlockGoal.tick()` with the goal's `getMoveToTarget` (the block, not the
    /// one above) and `shouldRecalculatePath` (`tryTicks % 20 == 0`).
    fn tick(&self, state: &mut MovingToBlock, ai: &mut Ai<'_, '_>) {
        let target = state.block.unwrap_or(BlockPos::new(0, 0, 0));
        let [x, y, z] = ai.position();
        let (dx, dy, dz) = (
            f64::from(target.x) + 0.5 - x,
            f64::from(target.y) + 0.5 - y,
            f64::from(target.z) + 0.5 - z,
        );
        if dx * dx + dy * dy + dz * dz >= 1.0 {
            state.reached = false;
            state.try_ticks += 1;
            if state.try_ticks % 20 == 0 {
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
            state.reached = true;
            state.try_ticks -= 1;
        }
    }
}

/// `isEquippableInSlot(stack, SADDLE)` and `Equippable.equipOnTarget`: a saddle on a
/// living grown strider without one is put on (`canUseSlot(SADDLE)`), kept to be dropped
/// whatever kills it, with its own saddling sound (`getEquipSound`, its seed one draw of
/// its random) and `GameEvent.EQUIP`; else nothing (`PASS`).
fn equip_saddle(used: &mut Used<'_>) -> Interaction {
    let saddle = slots::SADDLE as usize;
    let Some(stack) = used.held.as_ref().filter(|stack| stack.count > 0) else {
        return Interaction::Pass;
    };
    let fits = clustine_data::ItemComponents::of(stack.item)
        .equippable
        .is_some_and(|equippable| usize::from(equippable.slot) == saddle)
        && entity_in_tag(i32::from(used.kind), "minecraft:can_equip_saddle")
        && !used.mob.body.vitals.dying()
        && !animal::is_baby(used.mob);
    if !fits || used.mob.body.equipment.slots[saddle].is_some() {
        return Interaction::Pass;
    }
    used.mob.body.equipment.slots[saddle] = Some(stack.with_count(1));
    guarantee_saddle_drop(used.mob);
    let _seed = used.draws.next_u64();
    used.did.push(Did::Sound {
        sound: sounds::ENTITY_STRIDER_SADDLE,
        source: SOUND_SOURCE_NEUTRAL,
        at: used.position,
        volume: 1.0,
        pitch: 1.0,
        but_the_player: false,
    });
    used.did.push(Did::GameEvent("minecraft:equip"));
    Interaction::Success
}

/// `setGuaranteedDrop(SADDLE)`.
fn guarantee_saddle_drop(mob: &mut Mob) {
    let saddle = slots::SADDLE as usize;
    let mut chances = mob.body.equipment.drop_chances.unwrap_or([0.085; 8]);
    chances[saddle] = 2.0;
    mob.body.equipment.drop_chances = Some(chances);
}

/// A mob of the type `kind` made to ride the strider being finalised (`spawnJockey`,
/// Strider.txt 1164–1195): constructed of its own stream, `before` done to it (a baby
/// strider's age), snapped to the strider facing its way, finalised as `JOCKEY` of the
/// same level's random with a group of its own, `after` done to it (a zombified piglin's
/// stick), and handed to the maker to seat on it once both are added. False, and
/// nothing drawn for it, where the maker makes none (the game's `create` null).
fn make_rider(
    spawn: &mut Spawning<'_>,
    kind: u16,
    before: impl FnOnce(&mut Mob),
    after: impl FnOnce(&mut Mob),
) -> bool {
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
    let Some((key, mut own)) = spawn
        .companions
        .as_deref_mut()
        .and_then(|companions| companions.next())
    else {
        return false;
    };
    let (uuid, mut mob) = making::construct(kind, &mut own, yaw);
    before(&mut mob);
    let mut group = None;
    making::finalize(
        kind,
        &mut Spawning {
            mob: &mut mob,
            level: &mut *spawn.level,
            own: &mut own,
            biome: spawn.biome,
            at: crate::physics::containing(position),
            day: spawn.day,
            game_time: spawn.game_time,
            reason: SpawnReason::Jockey,
            group: &mut group,
            structure: spawn.structure,
            moon_brightness: spawn.moon_brightness,
            difficulty: spawn.difficulty,
            companions: None,
        },
    );
    after(&mut mob);
    let entity = making::fresh_record(key, kind, uuid, position, yaw, &mob);
    let beside = spawn.mob.clone();
    if let Some(companions) = spawn.companions.as_deref_mut() {
        companions.add(Companion {
            entity,
            seat: CompanionSeat::Passenger,
            own,
            beside,
        });
    }
    true
}

/// `Strider.finalizeSpawn` (Strider.txt 1065–1163), before `AgeableMob`'s: a baby's is the
/// animal's alone. Else one time in 30, where monsters spawn, a zombified piglin rides
/// it holding a warped fungus on a stick and it is saddled for good; else one time in
/// 10 a baby strider rides it; else its group is `AgeableMobGroupData(0.5)`. A rider
/// made gives it the group `spawnJockey` returns, `AgeableMobGroupData(0)`. Every group
/// it makes is fresh, so that `AgeableMob`'s draw for a baby never comes.
fn finalize_spawn(spawn: &mut Spawning<'_>) {
    if animal::is_baby(spawn.mob) {
        animal::finalize_ageable_default(spawn);
        return;
    }
    let spawning_monsters = Difficulty::WORLD != Difficulty::Peaceful;
    let jockey = AgeableGroup::with_chance(0.0);
    let group = if spawn.level.next_int(30) == 0 && spawning_monsters {
        let made = make_rider(
            spawn,
            entity_types::ZOMBIFIED_PIGLIN as u16,
            |_| {},
            |rider| {
                rider.body.equipment.slots[slots::MAIN_HAND as usize] =
                    Some(ItemStack::new(items::WARPED_FUNGUS_ON_A_STICK, 1));
            },
        );
        if made {
            spawn.mob.body.equipment.slots[slots::SADDLE as usize] =
                Some(ItemStack::new(items::SADDLE, 1));
            guarantee_saddle_drop(spawn.mob);
        }
        made.then_some(jockey)
    } else if spawn.level.next_int(10) == 0 {
        let made = make_rider(
            spawn,
            KIND,
            |baby| animal::update(baby, |shared| shared.set_age(animal::BABY_START_AGE, 0)),
            |_| {},
        );
        made.then_some(jockey)
    } else {
        Some(AgeableGroup::with_chance(0.5))
    };
    if let Some(group) = group {
        *spawn.group = Some(SpawnGroup::Ageable(group));
    }
    animal::finalize_ageable_default(spawn);
}

/// `Strider.mobInteract` (Strider.txt 960–1046): a saddled strider that carries nobody
/// takes on a player who does not sneak and holds none of its food; else an animal's
/// use, and where that did nothing a saddle put on; where its food was eaten, its eating
/// sound at volume 1 and a pitch about 1 of its own two draws.
fn mob_interact(used: &mut Used<'_>) -> Interaction {
    let food = FOOD.test(used.held.as_ref());
    if !food && saddled(used.mob) && !used.carries && !used.sneaking {
        used.did.push(Did::Mount);
        return Interaction::Success;
    }
    let result = animal::mob_interact(used, FOOD, None);
    if result == Interaction::Pass {
        return equip_saddle(used);
    }
    // No mob is silent.
    if food {
        let pitch = 1.0 + (used.draws.next_float() - used.draws.next_float()) * 0.2;
        used.did.push(Did::Sound {
            sound: sounds::ENTITY_STRIDER_EAT,
            source: SOUND_SOURCE_NEUTRAL,
            at: used.position,
            volume: 1.0,
            pitch,
            but_the_player: false,
        });
    }
    result
}

/// The strider.
pub(crate) struct Strider;

impl MobType for Strider {
    fn stands_on_lava(&self) -> bool {
        true
    }

    fn steered_by_stick(&self) -> Option<i32> {
        Some(items::WARPED_FUNGUS_ON_A_STICK)
    }

    fn stick_boost(&self, mob: &mut Mob, draws: &mut crate::region::Random) -> bool {
        boost(mob, draws)
    }

    /// `registerGoals` (Strider.txt 162–254) and the constructor's maluses (21–55, after
    /// `Animal`'s): water refused, lava and fire free.
    fn register_goals(&self, setup: &mut Setup) {
        cow::animal_maluses(setup);
        setup.set_malus(PathType::Water, -1.0);
        setup.set_malus(PathType::Lava, 0.0);
        setup.set_malus(PathType::FireInNeighbor, 0.0);
        setup.set_malus(PathType::Fire, 0.0);
        setup.add_goal(1, PanicGoal { speed: 1.65 });
        setup.add_goal(2, animal::BreedGoal { speed: 1.0 });
        setup.add_goal(3, TemptGoal::new(1.4, TEMPT, false));
        setup.add_goal(4, GoToLavaGoal);
        setup.add_goal(5, animal::FollowParentGoal { speed: 1.0 });
        setup.add_goal(
            7,
            RandomStrollGoal {
                interval: 60,
                ..RandomStrollGoal::new(1.0)
            },
        );
        setup.add_goal(8, LookAtPlayerGoal::new(Class::Player, 8.0));
        setup.add_goal(8, RandomLookAroundGoal);
        setup.add_goal(9, LookAtPlayerGoal::new(Class::Kinds(STRIDERS), 8.0));
    }

    /// `getWalkTargetValue` (Strider.txt 925–943): 10 where its block holds lava, else
    /// nothing, or no worth at all while it is in lava.
    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        if lava(ai.tick.world.state(pos)) {
            10.0
        } else if ai.in_lava() {
            f32::NEG_INFINITY
        } else {
            0.0
        }
    }

    /// `StriderPathNavigation.isStableDestination` (Strider$StriderPathNavigation.txt
    /// 26–42): a lava block, or ground's.
    fn stable_destination(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> Option<bool> {
        let world = ai.tick.world;
        Some(
            is_block(world.state(pos), blocks::LAVA)
                || world.state(pos.offset(0, -1, 0)).props().is_solid_render(),
        )
    }

    fn before_tick(&self, tick: &mut MobTick<'_>) {
        before_tick(tick);
    }

    /// What its last `baseTick` found of lava about it, kept for its steps and its
    /// passengers (`isInLava`, `isEyeInFluid(LAVA)`).
    fn after_base_tick(&self, tick: &mut MobTick<'_>, _: i32) {
        let in_lava = tick.living.in_lava();
        let eye = eye_in_lava(tick);
        change(&mut tick.mob, |own| {
            own.in_lava = in_lava;
            own.eye_in_lava = eye;
        });
    }

    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::custom_server_ai_step(tick);
    }

    /// `LivingEntity.aiStep`'s water hurt (`isSensitiveToWater`: drowning's 1 in
    /// water), then the animal's.
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        if tick.living.fluids.in_water() && !tick.mob.body.vitals.dying() {
            crate::mobs::body::hurt_by(tick, &DamageSource::of(DamageType::DROWN), 1.0);
        }
        animal::after_ai_step(tick);
    }

    fn after_tick(&self, tick: &mut MobTick<'_>) {
        float_strider(tick);
    }

    /// `Strider.checkFallDamage` (Strider.txt 691–706): in lava its fall is forgotten.
    /// Forgotten before its move, as the move is where the game's check comes: what the
    /// move itself drops in lava (a few hundredths a tick) is all it can count.
    fn before_travel(&self, tick: &mut MobTick<'_>) {
        if tick.living.in_lava() {
            tick.living.body.fall_distance = 0.0;
        }
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        mob_interact(used)
    }

    /// `getControllingPassenger` (Strider.txt 365–388): saddled, and the player holds a
    /// warped fungus on a stick in either hand.
    fn steered_by(&self, mob: &Mob, hands: [Option<ItemStack>; 2]) -> bool {
        saddled(mob)
            && hands.iter().any(|hand| {
                hand.as_ref().is_some_and(|stack| {
                    stack.count > 0 && stack.item == items::WARPED_FUNGUS_ON_A_STICK
                })
            })
    }

    /// `canAddPassenger`: nobody on it, and its eyes out of lava.
    fn can_add_passenger(&self, mob: &Mob, passengers: usize) -> bool {
        passengers == 0 && !state_of(mob).eye_in_lava
    }

    /// A baby's seat is its own (`BABY_DIMENSIONS`' passenger attachment).
    fn seat(
        &self,
        _: &clustine_world::Entity,
        mob: &Mob,
        _: usize,
        _: usize,
        _: u16,
        _: i64,
    ) -> Option<crate::physics::Vec3> {
        animal::is_baby(mob).then_some([0.0, BABY_SEAT, 0.0])
    }

    /// `Strider.tickRidden` (Strider.txt 600–628): the rider's yaw and half their pitch,
    /// the body and the head turned to the yaw; and `ItemBasedSteering.tickBoost`:
    /// `boosting && boostTime++ > total` ends it.
    fn tick_ridden(&self, tick: &mut MobTick<'_>, yaw: f32, pitch: f32) {
        tick.yaw = yaw % 360.0;
        tick.pitch = (pitch * 0.5) % 360.0;
        tick.mob.body.head.body_yaw = tick.yaw;
        tick.mob.body.head.head_yaw = tick.yaw;
        change(&mut tick.mob, |own| {
            if own.boost.boosting {
                let before = own.boost.time;
                own.boost.time += 1;
                if i64::from(before) > i64::from(own.boost.total) {
                    own.boost.boosting = false;
                }
            }
        });
    }

    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        finalize_spawn(spawn);
    }

    /// `getBreedOffspring`: a fresh strider (`EntityType.create(STRIDER, BREEDING)`).
    fn breed_offspring(&self, _: &mut Breeding<'_>) -> Option<Mob> {
        Some(Mob::fresh())
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        animal::dimensions(mob, BABY)
    }

    /// `getAmbientSound`: none while it panics or is tempted.
    fn ambient_sound(&self, mob: &Mob) -> Option<i32> {
        (!running(mob, PANIC_GOAL) && !running(mob, TEMPT_GOAL))
            .then_some(sounds::ENTITY_STRIDER_AMBIENT)
    }

    /// `playStepSound` (Strider.txt 668–681): in lava its lava step, else its step, at
    /// volume 1 and pitch 1.
    fn step_sound_on(&self, mob: &Mob, _: BlockState) -> Option<(i32, f32, f32)> {
        let sound = if state_of(mob).in_lava {
            sounds::ENTITY_STRIDER_STEP_LAVA
        } else {
            sounds::ENTITY_STRIDER_STEP
        };
        Some((sound, 1.0, 1.0))
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_STRIDER_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_STRIDER_DEATH)
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let own = state_of(mob);
        let mut values = animal::shown(mob);
        // `DATA_BOOST_TIME`: the last boost's length, kept when it is over.
        values.push((
            BOOST_INDEX,
            DataShown::Int(own.boost.total),
            own.boost.total == 0,
        ));
        values.push((
            SUFFOCATING_INDEX,
            DataShown::Boolean(own.suffocating),
            !own.suffocating,
        ));
        values
    }
}
