//! Foxes (`Fox` and its nested classes; `docs/groundwork/n7-brief.md`, section 6.1): an
//! animal of sweet and glow berries, red or snowy by the biome it is made in, that sleeps
//! by day under shelter, stalks and pounces on chickens and rabbits, picks berries from
//! bushes and vines, and carries in its mouth what it picks up, eating it when it is
//! food.
//!
//! Its flags (sitting, crouching, interested, pouncing, sleeping, faceplanted,
//! defending) are the synced byte the client draws it by; what its goals keep beside
//! them is in their slots, but the sleep goal's countdown, which the fox's constructor
//! draws and so is kept with the fox.
//!
//! Named differences: a fox bred hunts nothing, as the game's does until its chunk is
//! loaded again, which in Clustine is never (finding F1); no love cause is kept, so a
//! bred cub trusts no one (`animal.rs`), and one an egg made trusts the egg's player; a
//! trusted player's attacker is never known (no mob hurts a creative player), so the fox
//! defends no one, only drawing its chance; the weather is clear, so no thunder
//! wakes it or sends it to shelter; `canSeeSky` is the sky's light at full; its eating
//! leaves a stew's bowl or a honey bottle's glass as the item's remainder and applies no
//! effect (`Consumable` is not in the dump; **Inferred**); berries popped from a bush
//! draw from the block's stream; its pick-ups take only its own region's items, and a
//! bite across a line is not delivered; a turtle is taken as in water where the block at
//! its feet holds water; the head's tilt (`interestedAngle`) is the client's alone.

use clustine_blocks::data::{flag, has_water, is_block, number, set, set_flag};
use clustine_data::{blocks, entity_types, items, sounds};
use clustine_world::{BlockPos, EntityData, EntityId, ItemStack};
use serde::{Deserialize, Serialize};

use super::animal::{self, Own, biome_in_tag};
use super::{
    AgeableGroup, Breeding, Dimensions, Interaction, MobType, SpawnGroup, Spawning, Used, cow,
};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    Attacking, Avoiding, ClimbOnTopOfPowderSnowGoal, Items, LeapAtTargetGoal, LookAtPlayerGoal,
    LookingAt, MeleeAttackGoal, MoveToBlockGoal, MovingToBlock, NearestAttackableTargetGoal, Panic,
    PanicGoal, RandomStrollGoal, Targeting, ai_bounds, hold, look_at_thing, melee_hit,
    move_to_held, view_vector,
};
use crate::mobs::ai::maths::{hotspot_cos, hotspot_sin, reduced_tick_delay, wrap_degrees};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::{self, Class, Conditions, Looked};
use crate::mobs::ai::village_goals::{StrollThroughVillageGoal, StrollingThroughVillage};
use crate::mobs::ai::{self as mind, Ai, Setup, controls, navigation, random_pos};
use crate::mobs::body::hurt::{DamageSource, GameEvent};
use crate::mobs::body::{Event, make_sound};
use crate::mobs::tick::{Local, MobTick};
use crate::physics::{self, Vec3};
use crate::region::Random;
use crate::shown::DataShown;

/// `#fox_food`: sweet and glow berries.
const FOOD: Items = Items::Tag("minecraft:fox_food");

/// A cub's size: the type's (0.6 by 0.7) scaled by 0.6, its eyes at 0.34375.
const BABY: Dimensions = Dimensions {
    width: 0.6 * 0.6,
    height: 0.7 * 0.6,
    eye_height: 0.343_75,
};

/// The synced data's indices: the variant, the flags, and the two trusted.
const TYPE_INDEX: u8 = 18;
const FLAGS_INDEX: u8 = 19;
const TRUSTED_INDEX: [u8; 2] = [20, 21];

/// `Fox$Variant`'s ids.
pub(crate) mod variant {
    pub(crate) const RED: i32 = 0;
    pub(crate) const SNOW: i32 = 1;
}

/// The bits of `DATA_FLAGS_ID`.
pub(crate) mod flag_bits {
    pub(crate) const SITTING: u8 = 1;
    pub(crate) const CROUCHING: u8 = 4;
    pub(crate) const INTERESTED: u8 = 8;
    pub(crate) const POUNCING: u8 = 16;
    pub(crate) const SLEEPING: u8 = 32;
    pub(crate) const FACEPLANTED: u8 = 64;
    pub(crate) const DEFENDING: u8 = 128;
}

/// The items whose default components hold both `minecraft:food` and
/// `minecraft:consumable` (`isConsumableFood`; read from the extracted items'
/// components, which the generated data does not carry yet).
const CONSUMABLE_FOOD: [i32; 40] = [
    items::APPLE,
    items::BAKED_POTATO,
    items::BEEF,
    items::BEETROOT,
    items::BEETROOT_SOUP,
    items::BREAD,
    items::CARROT,
    items::CHICKEN,
    items::CHORUS_FRUIT,
    items::COD,
    items::COOKED_BEEF,
    items::COOKED_CHICKEN,
    items::COOKED_COD,
    items::COOKED_MUTTON,
    items::COOKED_PORKCHOP,
    items::COOKED_RABBIT,
    items::COOKED_SALMON,
    items::COOKIE,
    items::DRIED_KELP,
    items::ENCHANTED_GOLDEN_APPLE,
    items::GLOW_BERRIES,
    items::GOLDEN_APPLE,
    items::GOLDEN_CARROT,
    items::HONEY_BOTTLE,
    items::MELON_SLICE,
    items::MUSHROOM_STEW,
    items::MUTTON,
    items::POISONOUS_POTATO,
    items::PORKCHOP,
    items::POTATO,
    items::PUFFERFISH,
    items::PUMPKIN_PIE,
    items::RABBIT,
    items::RABBIT_STEW,
    items::ROTTEN_FLESH,
    items::SALMON,
    items::SPIDER_EYE,
    items::SUSPICIOUS_STEW,
    items::SWEET_BERRIES,
    items::TROPICAL_FISH,
];

/// What a fox keeps beyond an animal: its variant, its flags, the two it trusts (by
/// UUID), whether it hunts (`setTargetGoals` was called), the moment it last ate or
/// took something (`ticksSinceEaten` counts from it, so that a fox at rest commits
/// nothing for it), `crouchAmount`, and its sleep goal's countdown.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct FoxState {
    pub(crate) variant: i32,
    pub(crate) flags: u8,
    pub(crate) trusted: [Option<u128>; 2],
    pub(crate) hunts: bool,
    pub(crate) eaten_at: Option<u32>,
    pub(crate) crouch_amount: f32,
    pub(crate) sleep_countdown: i32,
}

impl Own for FoxState {}

impl FoxState {
    /// `ticksSinceEaten` in the tick that began at `lived`, after `aiStep` counted it: one
    /// a tick from its last reset, or from its first tick.
    pub(crate) fn ticks_since_eaten(&self, lived: u32) -> i64 {
        match self.eaten_at {
            None => i64::from(lived) + 1,
            Some(at) => (i64::from(lived) - i64::from(at)).max(0),
        }
    }

    pub(crate) fn has(&self, bit: u8) -> bool {
        self.flags & bit != 0
    }

    pub(crate) fn set(&mut self, bit: u8, on: bool) {
        if on {
            self.flags |= bit;
        } else {
            self.flags &= !bit;
        }
    }

    /// `clearStates()`: neither interested, crouching, sitting, sleeping, defending nor
    /// faceplanted.
    fn clear_states(&mut self) {
        use flag_bits::*;
        for bit in [
            INTERESTED,
            CROUCHING,
            SITTING,
            SLEEPING,
            DEFENDING,
            FACEPLANTED,
        ] {
            self.set(bit, false);
        }
    }

    /// `canMove()`: not sleeping, sitting or faceplanted.
    fn can_move(&self) -> bool {
        use flag_bits::*;
        !self.has(SLEEPING) && !self.has(SITTING) && !self.has(FACEPLANTED)
    }

    /// `addTrustedEntity(ref)`: the first slot if it is empty, else the second, which a
    /// third trust replaces.
    pub(crate) fn add_trusted(&mut self, uuid: u128) {
        if self.trusted[0].is_some() {
            self.trusted[1] = Some(uuid);
        } else {
            self.trusted[0] = Some(uuid);
        }
    }

    /// `trusts(entity)`.
    fn trusts(&self, uuid: Option<u128>) -> bool {
        uuid.is_some_and(|uuid| self.trusted.contains(&Some(uuid)))
    }
}

/// The fox.
pub(crate) struct Fox;

pub(crate) fn state_of(mob: &Mob) -> FoxState {
    animal::read::<FoxState>(&mob.state).1
}

fn put_state(mob: &mut Mob, own: &FoxState) {
    let animal = animal::animal_of(&mob.state);
    mob.state = animal::write(&animal, own);
}

/// Changes the fox's own state.
fn update(mob: &mut Mob, change: impl FnOnce(&mut FoxState)) {
    let mut own = state_of(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        put_state(mob, &own);
    }
}

fn has_bit(ai: &Ai<'_, '_>, bit: u8) -> bool {
    state_of(&ai.tick.mob).has(bit)
}

fn set_bit(ai: &mut Ai<'_, '_>, bit: u8, on: bool) {
    update(&mut ai.tick.mob, |own| own.set(bit, on));
}

fn clear_states(ai: &mut Ai<'_, '_>) {
    update(&mut ai.tick.mob, FoxState::clear_states);
}

/// `Fox.setTarget(target)`: a defending fox given none stops defending.
fn set_target(ai: &mut Ai<'_, '_>, target: Option<&Looked>) {
    if target.is_none() {
        set_bit(ai, flag_bits::DEFENDING, false);
    }
    sensing::set_target(ai, target);
}

/// Whether `stack` is food that is eaten (`isConsumableFood`).
pub(crate) fn is_consumable_food(stack: Option<&ItemStack>) -> bool {
    stack.is_some_and(|stack| stack.count > 0 && CONSUMABLE_FOOD.contains(&stack.item))
}

/// `canHoldItem(stack)`: its mouth is empty, or what it holds is no food and it has
/// eaten since it took it, and `stack` is.
pub(crate) fn can_hold_item(
    own: &FoxState,
    lived: u32,
    held: Option<ItemStack>,
    stack: ItemStack,
) -> bool {
    let empty = held.as_ref().is_none_or(|held| held.count <= 0);
    empty
        || (own.ticks_since_eaten(lived) > 0
            && is_consumable_food(Some(&stack))
            && !is_consumable_food(held.as_ref()))
}

/// The UUID of the thing `key` names (a player's, a mob's), where the view has it.
fn uuid_of(ai: &Ai<'_, '_>, key: EntityId) -> Option<u128> {
    if let Some(seen) = ai.tick.world.entity(key) {
        return Some(seen.entity.uuid.as_u128());
    }
    ai.tick
        .world
        .players()
        .iter()
        .find(|player| player.key == key)
        .map(|player| player.uuid)
}

/// `isBrightOutside()`: the sky darkened by less than four.
fn bright_outside(ai: &Ai<'_, '_>) -> bool {
    ai.tick.world.sky_darken() < 4
}

/// `canSeeSky(pos)`: the sky's light is full there (**Inferred**: `BlockAndTintGetter`'s
/// rule is not in the dump; a block the region does not read sees it).
fn can_see_sky(ai: &Ai<'_, '_>, pos: BlockPos) -> bool {
    ai.tick.world.light(pos).is_none_or(|light| light.sky >= 15)
}

/// `Fox.isPathClear(fox, target)`: from one to three blocks above the fox's feet, at
/// seven points from it to the target, nothing that is not replaceable.
pub(crate) fn is_path_clear(ai: &Ai<'_, '_>, target: &Looked) -> bool {
    let [x, y, z] = ai.position();
    let dz = target.position[2] - z;
    let dx = target.position[0] - x;
    let r = dz / dx;
    for i in 0..6 {
        let part = f64::from(i as f32 / 6.0);
        // `r == 0` by `dcmpl`: a NaN is not nothing.
        let flat = r == 0.0;
        let sz = if flat { 0.0 } else { dz * part };
        let sx = if flat { dx * part } else { sz / r };
        for k in 1..4 {
            let pos = physics::containing([x + sx, y + f64::from(k), z + sz]);
            if !ai.tick.world.state(pos).props().can_be_replaced() {
                return false;
            }
        }
    }
    true
}

/// `FoxAlertableEntitiesSelector.test(entity)`: what keeps a fox awake and from perching
/// (not a fox; a chicken, a rabbit or a monster; an untamed tamable; no creative player;
/// anything else not trusted, asleep or sneaking).
fn alertable_by(own: &FoxState, ai: &Ai<'_, '_>, entity: &Looked) -> bool {
    let kind = entity.kind.map(i32::from);
    if kind == Some(entity_types::FOX) {
        return false;
    }
    if matches!(kind, Some(entity_types::CHICKEN | entity_types::RABBIT)) {
        return true;
    }
    if entity.mob && entity.kind.is_some_and(sensing::is_monster) {
        return true;
    }
    if entity.is_player() && (entity.spectator || entity.creative) {
        return false;
    }
    if own.trusts(uuid_of(ai, entity.key)) {
        return false;
    }
    // No mob of Clustine sleeps but a fox; no tamable is tamed before their part.
    !entity.sneaking
}

/// `FoxBehaviorGoal.alertable()`: anything alertable within 12 (6 up and down), line of
/// sight or not.
fn alertable(ai: &mut Ai<'_, '_>) -> bool {
    let own = state_of(&ai.tick.mob);
    let bounds = ai_bounds(ai).inflate([12.0, 6.0, 12.0]);
    let mut candidates = sensing::things_in(ai, bounds, Class::Player);
    candidates.extend(sensing::things_in(ai, bounds, Class::Mob));
    let allowed: Vec<EntityId> = candidates
        .iter()
        .filter(|candidate| alertable_by(&own, ai, candidate))
        .map(|candidate| candidate.key)
        .collect();
    let conditions = Conditions::combat().range(12.0).ignore_line_of_sight();
    candidates.iter().any(|candidate| {
        let keys = &allowed;
        conditions.test(ai, candidate, &|looked| keys.contains(&looked.key))
    })
}

/// `FoxBehaviorGoal.hasShelter()`: the sky not seen from the top of its box, and a place
/// it does not mind.
fn has_shelter(ai: &Ai<'_, '_>) -> bool {
    let [x, y, z] = ai.position();
    let top = physics::containing([x, y + f64::from(ai.height()), z]);
    !can_see_sky(ai, top) && animal::walk_target_value(ai, top, blocks::GRASS_BLOCK) >= 0.0
}

/// `FoxFloatGoal`: a float that clears the fox's states as it starts, deeper than a
/// quarter of a block in water, or in lava.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FoxFloatGoal;

impl Goal for FoxFloatGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::JUMP
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn on_register(&self, config: &mut mind::Config) {
        config.navigation.set_can_float(true);
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        ai.tick.living.fluids.water_height > 0.25 || ai.in_lava()
    }

    fn start(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        clear_states(ai);
    }

    fn tick(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        if ai.tick.draws.next_float() < 0.8 {
            controls::jump(ai);
        }
    }
}

/// `FaceplantGoal`'s countdown.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Faceplanted {
    pub(crate) countdown: i32,
}

/// `FaceplantGoal`: a fox whose nose went into snow holds still a second.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FaceplantGoal;

impl Goal for FaceplantGoal {
    type State = Faceplanted;

    fn flags(&self) -> u8 {
        flags::LOOK | flags::JUMP | flags::MOVE
    }

    fn can_use(&self, _: &mut Faceplanted, ai: &mut Ai<'_, '_>) -> bool {
        has_bit(ai, flag_bits::FACEPLANTED)
    }

    fn can_continue(&self, state: &mut Faceplanted, ai: &mut Ai<'_, '_>) -> bool {
        has_bit(ai, flag_bits::FACEPLANTED) && state.countdown > 0
    }

    fn start(&self, state: &mut Faceplanted, _: &mut Ai<'_, '_>) {
        state.countdown = self.adjusted_tick_delay(40);
    }

    fn stop(&self, _: &mut Faceplanted, ai: &mut Ai<'_, '_>) {
        set_bit(ai, flag_bits::FACEPLANTED, false);
    }

    fn tick(&self, state: &mut Faceplanted, _: &mut Ai<'_, '_>) {
        state.countdown -= 1;
    }
}

/// `FoxPanicGoal(fox, 2.2)`: a panic, but never while it defends.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FoxPanicGoal;

impl FoxPanicGoal {
    const PANIC: PanicGoal = PanicGoal { speed: 2.2 };
}

impl Goal for FoxPanicGoal {
    type State = Panic;

    fn flags(&self) -> u8 {
        Self::PANIC.flags()
    }

    fn panics(&self) -> bool {
        true
    }

    fn can_use(&self, state: &mut Panic, ai: &mut Ai<'_, '_>) -> bool {
        !has_bit(ai, flag_bits::DEFENDING) && Self::PANIC.can_use(state, ai)
    }

    fn can_continue(&self, state: &mut Panic, ai: &mut Ai<'_, '_>) -> bool {
        Self::PANIC.can_continue(state, ai)
    }

    fn start(&self, state: &mut Panic, ai: &mut Ai<'_, '_>) {
        Self::PANIC.start(state, ai);
    }

    fn stop(&self, state: &mut Panic, ai: &mut Ai<'_, '_>) {
        Self::PANIC.stop(state, ai);
    }
}

/// `FoxBreedGoal(fox, 1.0)`: a breeding that clears the fox's states as it starts (the
/// partner's, a write to another mob, are left: named). The breeding itself is the
/// region's, as every animal's.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FoxBreedGoal;

impl FoxBreedGoal {
    const BREED: animal::BreedGoal = animal::BreedGoal { speed: 1.0 };
}

impl Goal for FoxBreedGoal {
    type State = animal::Courting;

    fn flags(&self) -> u8 {
        Self::BREED.flags()
    }

    fn can_use(&self, state: &mut animal::Courting, ai: &mut Ai<'_, '_>) -> bool {
        Self::BREED.can_use(state, ai)
    }

    fn can_continue(&self, state: &mut animal::Courting, ai: &mut Ai<'_, '_>) -> bool {
        Self::BREED.can_continue(state, ai)
    }

    fn start(&self, state: &mut animal::Courting, ai: &mut Ai<'_, '_>) {
        clear_states(ai);
        Self::BREED.start(state, ai);
    }

    fn stop(&self, state: &mut animal::Courting, ai: &mut Ai<'_, '_>) {
        Self::BREED.stop(state, ai);
    }

    fn tick(&self, state: &mut animal::Courting, ai: &mut Ai<'_, '_>) {
        Self::BREED.tick(state, ai);
    }
}

/// What a fox flees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Threat {
    /// A player not sneaking, not creative, not trusted.
    Player,
    /// An untamed wolf.
    Wolf,
    /// A polar bear.
    PolarBear,
}

/// `AvoidEntityGoal<T>(fox, class, maxDist, 1.6, 1.4, predicate)`: run from the nearest
/// of a kind, but never while defending.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FoxAvoidGoal {
    pub(crate) threat: Threat,
    pub(crate) max_distance: f32,
}

impl FoxAvoidGoal {
    fn class(&self) -> Class {
        match self.threat {
            Threat::Player => Class::Player,
            Threat::Wolf => Class::Kinds(&[entity_types::WOLF as u16]),
            Threat::PolarBear => Class::Kinds(&[entity_types::POLAR_BEAR as u16]),
        }
    }

    /// The goal's predicate, with `AvoidEntityGoal`'s own `NO_CREATIVE_OR_SPECTATOR`.
    fn avoids(&self, own: &FoxState, ai: &Ai<'_, '_>, looked: &Looked) -> bool {
        if looked.creative || looked.spectator || own.has(flag_bits::DEFENDING) {
            return false;
        }
        match self.threat {
            Threat::Player => !looked.sneaking && !own.trusts(uuid_of(ai, looked.key)),
            // No wolf is tamed before their part.
            Threat::Wolf | Threat::PolarBear => true,
        }
    }
}

impl Goal for FoxAvoidGoal {
    type State = Avoiding;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, state: &mut Avoiding, ai: &mut Ai<'_, '_>) -> bool {
        let own = state_of(&ai.tick.mob);
        let distance = f64::from(self.max_distance);
        let conditions = Conditions::combat().range(distance);
        let bounds = ai_bounds(ai).inflate([distance, 3.0, distance]);
        let candidates: Vec<Looked> = sensing::things_in(ai, bounds, self.class())
            .into_iter()
            .filter(|looked| self.avoids(&own, ai, looked))
            .collect();
        let from = ai.position();
        let Some(threat) = sensing::nearest(ai, candidates, &conditions, &|_| true, from) else {
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
        move_to_held(ai, held, 1.6);
    }

    fn stop(&self, state: &mut Avoiding, _: &mut Ai<'_, '_>) {
        state.to_avoid = None;
    }

    fn tick(&self, state: &mut Avoiding, ai: &mut Ai<'_, '_>) {
        let Some(threat) = state.to_avoid.and_then(|key| sensing::find(ai, key)) else {
            return;
        };
        let speed = if threat.distance_sqr(ai.position()) < 49.0 {
            1.4
        } else {
            1.6
        };
        navigation::set_speed_modifier(ai, speed);
    }
}

/// Whether `target` is a chicken or a rabbit (`STALKABLE_PREY`).
fn stalkable(target: &Looked) -> bool {
    matches!(
        target.kind.map(i32::from),
        Some(entity_types::CHICKEN | entity_types::RABBIT)
    )
}

/// `StalkPreyGoal`: towards a chicken or rabbit it hunts, and, within six blocks,
/// crouched and still, ready to pounce.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StalkPreyGoal;

impl Goal for StalkPreyGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        if has_bit(ai, flag_bits::SLEEPING) {
            return false;
        }
        let Some(target) = sensing::target(ai) else {
            return false;
        };
        target.alive
            && stalkable(&target)
            && target.distance_sqr(ai.position()) > 36.0
            && !has_bit(ai, flag_bits::CROUCHING)
            && !has_bit(ai, flag_bits::INTERESTED)
            && !ai.tick.mob.body.inputs.jumping
    }

    fn start(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        set_bit(ai, flag_bits::SITTING, false);
        set_bit(ai, flag_bits::FACEPLANTED, false);
    }

    fn stop(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        let target = sensing::target(ai);
        if let Some(target) = target.filter(|target| is_path_clear(ai, target)) {
            set_bit(ai, flag_bits::INTERESTED, true);
            set_bit(ai, flag_bits::CROUCHING, true);
            navigation::stop(ai);
            let head = ai.head();
            look_at_thing(ai, &target, head.max_y_rot as f32, head.max_x_rot as f32);
        } else {
            set_bit(ai, flag_bits::INTERESTED, false);
            set_bit(ai, flag_bits::CROUCHING, false);
        }
    }

    fn tick(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        let Some(target) = sensing::target(ai) else {
            return;
        };
        let head = ai.head();
        look_at_thing(ai, &target, head.max_y_rot as f32, head.max_x_rot as f32);
        if target.distance_sqr(ai.position()) <= 36.0 {
            set_bit(ai, flag_bits::INTERESTED, true);
            set_bit(ai, flag_bits::CROUCHING, true);
            navigation::stop(ai);
        } else {
            navigation::move_to_entity(ai, target.block_position(), 1.5);
        }
    }
}

/// `FoxPounceGoal`: from a full crouch with a clear way, a leap at the prey that bites
/// within two blocks, or ends nose down in a snow layer.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FoxPounceGoal;

impl Goal for FoxPounceGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::MOVE | flags::JUMP
    }

    fn interruptable(&self, _: &()) -> bool {
        false
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        if state_of(&ai.tick.mob).crouch_amount != 5.0 {
            return false;
        }
        let Some(target) = sensing::target(ai).filter(|target| target.alive) else {
            return false;
        };
        // Its motion's direction is always its facing (finding F11).
        let clear = is_path_clear(ai, &target);
        if !clear {
            // The path is made and thrown away.
            let _ = navigation::create_path(ai, target.block_position(), 0);
            set_bit(ai, flag_bits::CROUCHING, false);
            set_bit(ai, flag_bits::INTERESTED, false);
        }
        clear
    }

    fn can_continue(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        if sensing::target(ai).is_none_or(|target| !target.alive) {
            return false;
        }
        let vy = ai.tick.living.body.motion[1];
        let still =
            vy * vy < 0.050_000_000_745_058_06 && ai.tick.pitch.abs() < 15.0 && ai.on_ground();
        !still && !has_bit(ai, flag_bits::FACEPLANTED)
    }

    fn start(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        ai.tick.mob.body.inputs.jumping = true;
        set_bit(ai, flag_bits::POUNCING, true);
        set_bit(ai, flag_bits::INTERESTED, false);
        if let Some(target) = sensing::target(ai) {
            look_at_thing(ai, &target, 60.0, 30.0);
            let d = physics::normalize(physics::sub(target.position, ai.position()));
            let motion = ai.tick.living.body.motion;
            ai.tick.living.body.motion = physics::add(motion, [d[0] * 0.8, 0.9, d[2] * 0.8]);
        }
        navigation::stop(ai);
    }

    fn stop(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        update(&mut ai.tick.mob, |own| {
            own.set(flag_bits::CROUCHING, false);
            own.crouch_amount = 0.0;
            own.set(flag_bits::INTERESTED, false);
            own.set(flag_bits::POUNCING, false);
        });
    }

    fn tick(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        let target = sensing::target(ai);
        if let Some(target) = &target {
            look_at_thing(ai, target, 60.0, 30.0);
        }
        if !has_bit(ai, flag_bits::FACEPLANTED) {
            let v = ai.tick.living.body.motion;
            if v[1] * v[1] < 0.029_999_999_329_447_746 && ai.tick.pitch != 0.0 {
                // `Mth.rotLerp(0.2f, xRot, 0)`.
                let pitch = ai.tick.pitch;
                ai.tick.pitch = pitch + 0.2 * wrap_degrees(0.0 - pitch);
            } else {
                let h = (v[0] * v[0] + v[2] * v[2]).sqrt();
                let m = if ai.tick.mob.body.inputs.jumping && v[1] > 0.0 {
                    f64::from(6.5_f32)
                } else {
                    1.0
                };
                let vy = v[1] * m;
                let len = (h * h + vy * vy).sqrt();
                if len > 9.999_999_747_378_752e-6 {
                    // `Math.acos`, which the JDK defines as `StrictMath`'s.
                    let angle = (-vy).signum() * libm::acos(h / len) * 57.295_776_367_187_5;
                    ai.tick.pitch = angle as f32;
                }
            }
        }
        if let Some(target) = &target {
            let distance = target.distance_sqr(ai.position()).sqrt() as f32;
            if distance <= 2.0 {
                melee_hit(ai, target);
                return;
            }
        }
        let at = ai.block_position();
        if ai.tick.pitch > 0.0
            && ai.on_ground()
            && ai.tick.living.body.motion[1] as f32 != 0.0
            && is_block(ai.tick.world.state(at), blocks::SNOW)
        {
            ai.tick.pitch = 60.0;
            set_target(ai, None);
            set_bit(ai, flag_bits::FACEPLANTED, true);
        }
    }
}

/// `SeekShelterGoal`'s state: its interval and where it hides.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Sheltering {
    pub(crate) interval: i32,
    pub(crate) wanted: Vec3,
}

impl Default for Sheltering {
    /// `reducedTickDelay(100)` at construction.
    fn default() -> Self {
        Self {
            interval: reduced_tick_delay(100),
            wanted: [0.0; 3],
        }
    }
}

/// `SeekShelterGoal(fox, 1.25)` over `FleeSunGoal`: every hundred tries, by day under the
/// open sky and outside a village (`isVillage`), off to somewhere covered it does not
/// mind.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SeekShelterGoal;

impl SeekShelterGoal {
    /// `FleeSunGoal.getHidePos()`: of ten tries, the first covered and disliked.
    fn hide_pos(ai: &mut Ai<'_, '_>) -> Option<Vec3> {
        let at = ai.block_position();
        for _ in 0..10 {
            let dx = ai.tick.draws.next_int(20) as i32 - 10;
            let dy = ai.tick.draws.next_int(6) as i32 - 3;
            let dz = ai.tick.draws.next_int(20) as i32 - 10;
            let pos = BlockPos::new(at.x + dx, at.y + dy, at.z + dz);
            if !can_see_sky(ai, pos)
                && animal::walk_target_value(ai, pos, blocks::GRASS_BLOCK) < 0.0
            {
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

impl Goal for SeekShelterGoal {
    type State = Sheltering;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, state: &mut Sheltering, ai: &mut Ai<'_, '_>) -> bool {
        if has_bit(ai, flag_bits::SLEEPING) || sensing::target(ai).is_some() {
            return false;
        }
        // No thunder: the weather is clear.
        if state.interval > 0 {
            state.interval -= 1;
            return false;
        }
        state.interval = 100;
        let at = ai.block_position();
        if !(bright_outside(ai) && can_see_sky(ai, at) && !ai.tick.world.is_village(at)) {
            return false;
        }
        match Self::hide_pos(ai) {
            Some(wanted) => {
                state.wanted = wanted;
                true
            }
            None => false,
        }
    }

    fn can_continue(&self, _: &mut Sheltering, ai: &mut Ai<'_, '_>) -> bool {
        !navigation::is_done(ai)
    }

    fn start(&self, state: &mut Sheltering, ai: &mut Ai<'_, '_>) {
        clear_states(ai);
        navigation::move_to(ai, state.wanted, 1.25);
    }
}

/// `FoxMeleeAttackGoal(fox, 1.2f, true)`: a melee attack that only an awake, standing
/// fox makes, with its bite's sound.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FoxMeleeAttackGoal;

impl FoxMeleeAttackGoal {
    const MELEE: MeleeAttackGoal = MeleeAttackGoal {
        speed: 1.200_000_047_683_715_8,
        following_even_if_not_seen: true,
    };
}

impl Goal for FoxMeleeAttackGoal {
    type State = Attacking;

    fn flags(&self) -> u8 {
        Self::MELEE.flags()
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) -> bool {
        use flag_bits::*;
        let own = state_of(&ai.tick.mob);
        !own.has(SITTING)
            && !own.has(SLEEPING)
            && !own.has(CROUCHING)
            && !own.has(FACEPLANTED)
            && Self::MELEE.can_use(state, ai)
    }

    fn can_continue(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) -> bool {
        Self::MELEE.can_continue(state, ai)
    }

    fn start(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) {
        set_bit(ai, flag_bits::INTERESTED, false);
        Self::MELEE.start(state, ai);
    }

    fn stop(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) {
        Self::MELEE.stop(state, ai);
    }

    fn tick(&self, state: &mut Attacking, ai: &mut Ai<'_, '_>) {
        let Some(target) = Self::MELEE.chase(state, ai) else {
            return;
        };
        if MeleeAttackGoal::can_perform_attack(state, ai, &target) {
            Self::MELEE.reset_attack_cooldown(state);
            melee_hit(ai, &target);
            ai.tick.out.events.push(Event::Sound {
                sound: sounds::ENTITY_FOX_BITE,
                volume: 1.0,
                pitch: 1.0,
            });
        }
    }
}

/// `SleepGoal`: by day, sheltered, with nothing about, a still fox lies down to sleep,
/// once its countdown (kept with the fox) is over.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SleepGoal;

impl SleepGoal {
    /// `canSleep()`: the countdown down by one first, while it lasts.
    fn can_sleep(ai: &mut Ai<'_, '_>) -> bool {
        let mut own = state_of(&ai.tick.mob);
        if own.sleep_countdown > 0 {
            own.sleep_countdown -= 1;
            put_state(&mut ai.tick.mob, &own);
            return false;
        }
        bright_outside(ai) && has_shelter(ai) && !alertable(ai) && !ai.tick.living.in_powder_snow
    }
}

impl Goal for SleepGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK | flags::JUMP
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        let inputs = &ai.tick.mob.body.inputs;
        if inputs.xxa != 0.0 || inputs.yya != 0.0 || inputs.zza != 0.0 {
            return false;
        }
        Self::can_sleep(ai) || has_bit(ai, flag_bits::SLEEPING)
    }

    fn can_continue(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        Self::can_sleep(ai)
    }

    fn start(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        update(&mut ai.tick.mob, |own| {
            own.set(flag_bits::SITTING, false);
            own.set(flag_bits::CROUCHING, false);
            own.set(flag_bits::INTERESTED, false);
            own.set(flag_bits::SLEEPING, true);
        });
        ai.tick.mob.body.inputs.jumping = false;
        navigation::stop(ai);
        let at = ai.position();
        controls::set_wanted_position(ai, at, 0.0);
    }

    fn stop(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        let countdown = ai.tick.draws.next_int(70) as i32;
        update(&mut ai.tick.mob, |own| {
            own.sleep_countdown = countdown;
            own.clear_states();
        });
    }
}

/// `FoxFollowParentGoal(fox, 1.25)`: a cub's following, but not while it defends.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FoxFollowParentGoal;

impl FoxFollowParentGoal {
    const FOLLOW: animal::FollowParentGoal = animal::FollowParentGoal { speed: 1.25 };
}

impl Goal for FoxFollowParentGoal {
    type State = animal::FollowingParent;

    fn flags(&self) -> u8 {
        Self::FOLLOW.flags()
    }

    fn can_use(&self, state: &mut animal::FollowingParent, ai: &mut Ai<'_, '_>) -> bool {
        !has_bit(ai, flag_bits::DEFENDING) && Self::FOLLOW.can_use(state, ai)
    }

    fn can_continue(&self, state: &mut animal::FollowingParent, ai: &mut Ai<'_, '_>) -> bool {
        !has_bit(ai, flag_bits::DEFENDING) && Self::FOLLOW.can_continue(state, ai)
    }

    fn start(&self, state: &mut animal::FollowingParent, ai: &mut Ai<'_, '_>) {
        clear_states(ai);
        Self::FOLLOW.start(state, ai);
    }

    fn stop(&self, state: &mut animal::FollowingParent, ai: &mut Ai<'_, '_>) {
        Self::FOLLOW.stop(state, ai);
    }

    fn tick(&self, state: &mut animal::FollowingParent, ai: &mut Ai<'_, '_>) {
        Self::FOLLOW.tick(state, ai);
    }
}

/// `FoxStrollThroughVillageGoal(fox, 32, 200)`: `StrollThroughVillageGoal(fox, 200)`
/// (`crate::mobs::ai::village_goals`) while the fox can move (`canFoxMove`: neither
/// sleeping, sitting nor defending, and without a target), its states cleared as it
/// starts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FoxStrollThroughVillageGoal;

impl FoxStrollThroughVillageGoal {
    const STROLL: StrollThroughVillageGoal = StrollThroughVillageGoal { interval: 200 };

    /// `canFoxMove()`.
    fn can_fox_move(ai: &Ai<'_, '_>) -> bool {
        use flag_bits::*;
        !has_bit(ai, SLEEPING)
            && !has_bit(ai, SITTING)
            && !has_bit(ai, DEFENDING)
            && sensing::target(ai).is_none()
    }
}

impl Goal for FoxStrollThroughVillageGoal {
    type State = StrollingThroughVillage;

    fn flags(&self) -> u8 {
        Self::STROLL.flags()
    }

    fn can_use(&self, state: &mut StrollingThroughVillage, ai: &mut Ai<'_, '_>) -> bool {
        Self::STROLL.can_use(state, ai) && Self::can_fox_move(ai)
    }

    fn can_continue(&self, state: &mut StrollingThroughVillage, ai: &mut Ai<'_, '_>) -> bool {
        Self::STROLL.can_continue(state, ai) && Self::can_fox_move(ai)
    }

    fn start(&self, state: &mut StrollingThroughVillage, ai: &mut Ai<'_, '_>) {
        clear_states(ai);
        Self::STROLL.start(state, ai);
    }

    fn tick(&self, state: &mut StrollingThroughVillage, ai: &mut Ai<'_, '_>) {
        Self::STROLL.tick(state, ai);
    }
}

/// `FoxEatBerriesGoal`'s state: its `MoveToBlockGoal`'s and the ticks it has waited at
/// the bush.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct EatingBerries {
    pub(crate) moving: MovingToBlock,
    pub(crate) ticks_waited: i32,
}

/// `FoxEatBerriesGoal(fox, 1.2f, 12, 1)`: to a ripe sweet berry bush or glow berries
/// within 12 blocks, sniffing on the way, and after two seconds there, picks them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FoxEatBerriesGoal;

/// `FoxEatBerriesGoal.isValidTarget(level, pos)`.
fn berries_at(ai: &Ai<'_, '_>, pos: BlockPos) -> bool {
    let state = ai.tick.world.state(pos);
    (is_block(state, blocks::SWEET_BERRY_BUSH) && number(state, "age").is_some_and(|age| age >= 2))
        || has_glow_berries(state)
}

/// `CaveVines.hasGlowBerries(state)`: a state with `berries` set.
fn has_glow_berries(state: clustine_data::BlockState) -> bool {
    state.property("berries").is_some() && flag(state, "berries")
}

impl FoxEatBerriesGoal {
    const MOVING: MoveToBlockGoal = MoveToBlockGoal {
        speed: 1.200_000_047_683_715_8,
        search_range: 12,
        vertical_search_range: 1,
        vertical_search_start: 0,
        valid: berries_at,
        next_start: None,
    };

    /// `onReachedTarget()`: the berries picked (the game rule `mobGriefing` is on).
    fn on_reached(state: &EatingBerries, ai: &mut Ai<'_, '_>) {
        let pos = state.moving.block.unwrap_or(BlockPos::new(0, 0, 0));
        let block = ai.tick.world.state(pos);
        if is_block(block, blocks::SWEET_BERRY_BUSH) {
            Self::pick_sweet_berries(ai, pos, block);
        } else if has_glow_berries(block) {
            Self::pick_glow_berry(ai, pos, block);
        }
    }

    /// `pickSweetBerries(state)`: one to two berries, a third from a ripe bush, the
    /// first into an empty mouth and the rest popped out; the bush back to age 1.
    fn pick_sweet_berries(ai: &mut Ai<'_, '_>, pos: BlockPos, block: clustine_data::BlockState) {
        let age = number(block, "age").unwrap_or(0);
        let mut count = 1 + ai.tick.draws.next_int(2) as i32 + i32::from(age == 3);
        let hand = usize::from(slots::MAIN_HAND);
        if ai.tick.mob.body.equipment.slots[hand]
            .as_ref()
            .is_none_or(|held| held.count <= 0)
        {
            ai.tick.mob.body.equipment.slots[hand] = Some(ItemStack::new(items::SWEET_BERRIES, 1));
            count -= 1;
        }
        if count > 0 {
            ai.tick.out.local.push(Local::Pop {
                pos,
                stack: ItemStack::new(items::SWEET_BERRIES, count),
            });
        }
        ai.tick.out.events.push(Event::Sound {
            sound: sounds::BLOCK_SWEET_BERRY_BUSH_PICK_BERRIES,
            volume: 1.0,
            pitch: 1.0,
        });
        ai.tick.out.local.push(Local::SetBlock {
            pos,
            state: set(block, "age", "1"),
            flags: 2,
        });
        ai.tick.out.local.push(Local::GameEvent {
            name: "minecraft:block_change",
            pos,
        });
    }

    /// `CaveVines.use(fox, state, level, pos)`: the vine's one glow berry popped out
    /// (`harvest/cave_vine` draws nothing), its sound at the level's pitch, the berries
    /// gone.
    fn pick_glow_berry(ai: &mut Ai<'_, '_>, pos: BlockPos, block: clustine_data::BlockState) {
        ai.tick.out.local.push(Local::Pop {
            pos,
            stack: ItemStack::new(items::GLOW_BERRIES, 1),
        });
        // `Mth.randomBetween(level.getRandom(), 0.8f, 1.2f)`.
        let pitch = ai.tick.draws.next_float() * 0.4 + 0.8;
        ai.tick.out.local.push(Local::Sound {
            sound: sounds::BLOCK_CAVE_VINES_PICK_BERRIES,
            source: crate::api::SOUND_SOURCE_BLOCKS,
            at: [
                f64::from(pos.x) + 0.5,
                f64::from(pos.y) + 0.5,
                f64::from(pos.z) + 0.5,
            ],
            volume: 1.0,
            pitch,
        });
        ai.tick.out.local.push(Local::SetBlock {
            pos,
            state: set_flag(block, "berries", false),
            flags: 2,
        });
        ai.tick.out.local.push(Local::GameEvent {
            name: "minecraft:block_change",
            pos,
        });
    }
}

impl Goal for FoxEatBerriesGoal {
    type State = EatingBerries;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::JUMP
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, state: &mut EatingBerries, ai: &mut Ai<'_, '_>) -> bool {
        !has_bit(ai, flag_bits::SLEEPING) && Self::MOVING.can_use(&mut state.moving, ai)
    }

    fn can_continue(&self, state: &mut EatingBerries, ai: &mut Ai<'_, '_>) -> bool {
        Self::MOVING.can_continue(&mut state.moving, ai)
    }

    fn start(&self, state: &mut EatingBerries, ai: &mut Ai<'_, '_>) {
        state.ticks_waited = 0;
        set_bit(ai, flag_bits::SITTING, false);
        Self::MOVING.start(&mut state.moving, ai);
    }

    fn tick(&self, state: &mut EatingBerries, ai: &mut Ai<'_, '_>) {
        if state.moving.reached {
            if state.ticks_waited >= 40 {
                Self::on_reached(state, ai);
            } else {
                state.ticks_waited += 1;
            }
        } else if ai.tick.draws.next_float() < 0.05 {
            ai.tick.out.events.push(Event::Sound {
                sound: sounds::ENTITY_FOX_SNIFF,
                volume: 1.0,
                pitch: 1.0,
            });
        }
        // `MoveToBlockGoal.tick()`, with the fox's accepted distance of two and its
        // path made again every hundred tries.
        let block = state.moving.block.unwrap_or(BlockPos::new(0, 0, 0));
        let target = BlockPos::new(block.x, block.y + 1, block.z);
        let [x, y, z] = ai.position();
        let (dx, dy, dz) = (
            f64::from(target.x) + 0.5 - x,
            f64::from(target.y) + 0.5 - y,
            f64::from(target.z) + 0.5 - z,
        );
        if dx * dx + dy * dy + dz * dz >= 4.0 {
            state.moving.reached = false;
            state.moving.try_ticks += 1;
            if state.moving.try_ticks % 100 == 0 {
                navigation::move_to(
                    ai,
                    [
                        f64::from(target.x) + 0.5,
                        f64::from(target.y),
                        f64::from(target.z) + 0.5,
                    ],
                    Self::MOVING.speed,
                );
            }
        } else {
            state.moving.reached = true;
            state.moving.try_ticks -= 1;
        }
    }
}

/// The items a fox may go for in `bounds`: alive, without a pick-up delay
/// (`ALLOWED_ITEMS`), its own region's, in the view's order.
fn items_in(ai: &Ai<'_, '_>, bounds: crate::physics::aabb::Aabb) -> Vec<(EntityId, Vec3)> {
    ai.tick
        .world
        .entities_in(&bounds)
        .iter()
        .filter(|seen| !seen.copied)
        .filter_map(|seen| match seen.entity.data {
            EntityData::Item {
                ref stack,
                pick_up_delay: 0,
                ..
            } if stack.count > 0 => Some((seen.entity.key, seen.entity.position)),
            _ => None,
        })
        .collect()
}

/// `FoxSearchForItemsGoal`: an empty-mouthed fox at ease goes, one time in five, for the
/// first item within eight blocks.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FoxSearchForItemsGoal;

fn mouth_empty(ai: &Ai<'_, '_>) -> bool {
    ai.tick.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)]
        .as_ref()
        .is_none_or(|held| held.count <= 0)
}

impl Goal for FoxSearchForItemsGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        if !mouth_empty(ai) {
            return false;
        }
        if ai.tick.mob.body.combat.target.is_some()
            || ai.tick.mob.body.combat.last_hurt_by.is_some()
        {
            return false;
        }
        if !state_of(&ai.tick.mob).can_move() {
            return false;
        }
        if ai.tick.draws.next_int(reduced_tick_delay(10) as u32) != 0 {
            return false;
        }
        let bounds = ai_bounds(ai).inflate([8.0, 8.0, 8.0]);
        !items_in(ai, bounds).is_empty() && mouth_empty(ai)
    }

    fn start(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        let bounds = ai_bounds(ai).inflate([8.0, 8.0, 8.0]);
        if let Some((_, at)) = items_in(ai, bounds).first() {
            navigation::move_to_entity(ai, physics::containing(*at), 1.200_000_047_683_715_8);
        }
    }

    fn tick(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        let bounds = ai_bounds(ai).inflate([8.0, 8.0, 8.0]);
        let found = items_in(ai, bounds);
        if mouth_empty(ai)
            && let Some((_, at)) = found.first()
        {
            navigation::move_to_entity(ai, physics::containing(*at), 1.200_000_047_683_715_8);
        }
    }
}

/// `FoxLookAtPlayerGoal(fox, Player, 24)`: a look at a player, but not faceplanted or
/// interested.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FoxLookAtPlayerGoal;

impl FoxLookAtPlayerGoal {
    fn look() -> LookAtPlayerGoal {
        LookAtPlayerGoal::new(Class::Player, 24.0)
    }
}

impl Goal for FoxLookAtPlayerGoal {
    type State = LookingAt;

    fn flags(&self) -> u8 {
        Self::look().flags()
    }

    fn can_use(&self, state: &mut LookingAt, ai: &mut Ai<'_, '_>) -> bool {
        Self::look().can_use(state, ai)
            && !has_bit(ai, flag_bits::FACEPLANTED)
            && !has_bit(ai, flag_bits::INTERESTED)
    }

    fn can_continue(&self, state: &mut LookingAt, ai: &mut Ai<'_, '_>) -> bool {
        Self::look().can_continue(state, ai)
            && !has_bit(ai, flag_bits::FACEPLANTED)
            && !has_bit(ai, flag_bits::INTERESTED)
    }

    fn start(&self, state: &mut LookingAt, ai: &mut Ai<'_, '_>) {
        Self::look().start(state, ai);
    }

    fn stop(&self, state: &mut LookingAt, ai: &mut Ai<'_, '_>) {
        Self::look().stop(state, ai);
    }

    fn tick(&self, state: &mut LookingAt, ai: &mut Ai<'_, '_>) {
        Self::look().tick(state, ai);
    }
}

/// `PerchAndSearchGoal`'s state: where it looks, for how long, and how many looks are
/// left.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Perching {
    pub(crate) rel_x: f64,
    pub(crate) rel_z: f64,
    pub(crate) look_time: i32,
    pub(crate) looks_remaining: i32,
}

/// `PerchAndSearchGoal`: a fox at rest sits now and then and looks about two to four
/// times.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PerchAndSearchGoal;

impl PerchAndSearchGoal {
    /// `resetLook()`: a direction drawn (HotSpot's `Math.cos` and `Math.sin`, finding
    /// F2) and its time.
    fn reset_look(&self, state: &mut Perching, ai: &mut Ai<'_, '_>) {
        let angle = std::f64::consts::TAU * ai.tick.draws.next_double();
        state.rel_x = hotspot_cos(angle);
        state.rel_z = hotspot_sin(angle);
        let time = 80 + ai.tick.draws.next_int(20) as i32;
        state.look_time = self.adjusted_tick_delay(time);
    }
}

impl Goal for PerchAndSearchGoal {
    type State = Perching;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    fn can_use(&self, _: &mut Perching, ai: &mut Ai<'_, '_>) -> bool {
        use flag_bits::*;
        ai.tick.mob.body.combat.last_hurt_by.is_none()
            && ai.tick.draws.next_float() < 0.02
            && !has_bit(ai, SLEEPING)
            && sensing::target(ai).is_none()
            && navigation::is_done(ai)
            && !alertable(ai)
            && !has_bit(ai, POUNCING)
            && !has_bit(ai, CROUCHING)
    }

    fn can_continue(&self, state: &mut Perching, _: &mut Ai<'_, '_>) -> bool {
        state.looks_remaining > 0
    }

    fn start(&self, state: &mut Perching, ai: &mut Ai<'_, '_>) {
        self.reset_look(state, ai);
        state.looks_remaining = 2 + ai.tick.draws.next_int(3) as i32;
        set_bit(ai, flag_bits::SITTING, true);
        navigation::stop(ai);
    }

    fn stop(&self, _: &mut Perching, ai: &mut Ai<'_, '_>) {
        set_bit(ai, flag_bits::SITTING, false);
    }

    fn tick(&self, state: &mut Perching, ai: &mut Ai<'_, '_>) {
        state.look_time -= 1;
        if state.look_time <= 0 {
            state.looks_remaining -= 1;
            self.reset_look(state, ai);
        }
        let [x, _, z] = ai.position();
        let eye = ai.eye_y();
        let head = ai.head();
        controls::set_look_at(
            ai,
            [x + state.rel_x, eye, z + state.rel_z],
            head.max_y_rot as f32,
            head.max_x_rot as f32,
        );
    }
}

/// `DefendTrustedTargetGoal`: it draws its chance; a trusted one's attacker is never
/// known (no mob hurts a creative player), so it never starts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DefendTrustedTargetGoal;

impl Goal for DefendTrustedTargetGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::TARGET
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        let interval = reduced_tick_delay(10);
        let _ = ai.tick.draws.next_int(interval as u32);
        false
    }
}

/// `turtleEggTargetGoal`: `NearestAttackableTargetGoal<Turtle>(fox, 10, false, false,
/// Turtle.BABY_ON_LAND_SELECTOR)`: a baby turtle out of the water.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HuntTurtlesGoal;

impl HuntTurtlesGoal {
    const NEAREST: NearestAttackableTargetGoal = NearestAttackableTargetGoal {
        class: Class::Kinds(&[entity_types::TURTLE as u16]),
        interval: 10,
        must_see: false,
        must_reach: false,
        selector: None,
    };
}

impl Goal for HuntTurtlesGoal {
    type State = Targeting;

    fn flags(&self) -> u8 {
        flags::TARGET
    }

    fn can_use(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        let interval = reduced_tick_delay(Self::NEAREST.interval);
        if interval > 0 && ai.tick.draws.next_int(interval as u32) != 0 {
            return false;
        }
        let follow = ai.attribute(clustine_data::attributes::FOLLOW_RANGE);
        let bounds = ai_bounds(ai).inflate([follow, follow, follow]);
        let world = ai.tick.world;
        let candidates: Vec<Looked> = sensing::things_in(ai, bounds, Self::NEAREST.class)
            .into_iter()
            .filter(|turtle| {
                let baby = world
                    .entity(turtle.key)
                    .is_some_and(|seen| match &seen.entity.data {
                        EntityData::Mob { state, .. } => animal::animal_of(state).is_baby(),
                        _ => false,
                    });
                baby && !has_water(world.state(turtle.block_position()))
            })
            .collect();
        let conditions = Conditions::combat().range(follow).ignore_line_of_sight();
        let [x, _, z] = ai.position();
        let eyes = [x, ai.eye_y(), z];
        let found = sensing::nearest(ai, candidates, &conditions, &|_| true, eyes);
        state.found = found.map(|found| found.key);
        state.found.is_some()
    }

    fn can_continue(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        Self::NEAREST.can_continue(state, ai)
    }

    fn start(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        Self::NEAREST.start(state, ai);
    }

    fn stop(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        Self::NEAREST.stop(state, ai);
    }
}

/// `landTargetGoal`: a chicken or a rabbit.
const HUNT_LAND: NearestAttackableTargetGoal = NearestAttackableTargetGoal {
    class: Class::Kinds(&[entity_types::CHICKEN as u16, entity_types::RABBIT as u16]),
    interval: 10,
    must_see: false,
    must_reach: false,
    selector: None,
};

/// `fishTargetGoal`: a fish of a school (cod, salmon, tropical fish).
const HUNT_FISH: NearestAttackableTargetGoal = NearestAttackableTargetGoal {
    class: Class::Kinds(&[
        entity_types::COD as u16,
        entity_types::SALMON as u16,
        entity_types::TROPICAL_FISH as u16,
    ]),
    interval: 20,
    must_see: false,
    must_reach: false,
    selector: None,
};

/// `Mob.aiStep`'s pick-up for a fox (`canPickUpLoot`): what it may hold of the items it
/// touches (its box grown by one sideways), each in turn (`pickUpItem`): the rest of a
/// stack put down, what it held spat out, one taken into its mouth for sure to drop.
fn pick_up(tick: &mut MobTick<'_>) {
    if !tick.mob.body.flags.picks_up_loot || tick.mob.body.vitals.dying() {
        return;
    }
    let [x, y, z] = tick.living.body.position;
    let bounds = crate::physics::aabb::Aabb::of_entity(
        [x, y, z],
        tick.living.body.width,
        tick.living.body.height,
    )
    .inflate([1.0, 0.0, 1.0]);
    let found: Vec<(EntityId, ItemStack)> = tick
        .world
        .entities_in(&bounds)
        .iter()
        .filter(|seen| !seen.copied)
        .filter_map(|seen| match seen.entity.data {
            EntityData::Item {
                ref stack,
                pick_up_delay: 0,
                ..
            } if stack.count > 0 => Some((seen.entity.key, stack.clone())),
            _ => None,
        })
        .collect();
    let hand = usize::from(slots::MAIN_HAND);
    for (item, stack) in found {
        let own = state_of(&tick.mob);
        let held = &tick.mob.body.equipment.slots[hand];
        if !can_hold_item(&own, tick.lived, held.clone(), stack.clone()) {
            continue;
        }
        let position = tick.living.body.position;
        if stack.count > 1 {
            // `dropItemStack`: a new item at its feet, of no delay.
            tick.out.local.push(Local::Drop {
                stack: stack.with_count(stack.count - 1),
                at: position,
                pick_up_delay: 0,
            });
        }
        spit_out_item(tick, held.clone());
        tick.mob.body.equipment.slots[hand] = Some(stack.with_count(1));
        let mut chances = tick.mob.body.equipment.drop_chances.unwrap_or([0.085; 8]);
        chances[hand] = 2.0;
        tick.mob.body.equipment.drop_chances = Some(chances);
        tick.out.local.push(Local::TakeItem { item, count: 0 });
        let now = tick.lived;
        update(&mut tick.mob, |own| own.eaten_at = Some(now));
    }
}

/// `spitOutItem(stack)`: what it held, an item ahead of it a block up that no one may
/// take for two seconds, with its spit's sound.
fn spit_out_item(tick: &mut MobTick<'_>, held: Option<ItemStack>) {
    let Some(stack) = held.filter(|stack| stack.count > 0) else {
        return;
    };
    let look = view_vector(tick.pitch, tick.yaw);
    let [x, y, z] = tick.living.body.position;
    tick.out.events.push(Event::Sound {
        sound: sounds::ENTITY_FOX_SPIT,
        volume: 1.0,
        pitch: 1.0,
    });
    tick.out.local.push(Local::Drop {
        stack,
        at: [x + look[0], y + 1.0, z + look[2]],
        pick_up_delay: 40,
    });
}

/// `finishUsingItem` of a fox's food: one eaten, and what the item leaves (a bowl, a
/// bottle), else nothing (**Inferred**, `Consumable` is not in the dump).
fn finish_eating(held: ItemStack) -> Option<ItemStack> {
    let remainder = match held.item {
        items::MUSHROOM_STEW
        | items::BEETROOT_SOUP
        | items::RABBIT_STEW
        | items::SUSPICIOUS_STEW => Some(items::BOWL),
        items::HONEY_BOTTLE => Some(items::GLASS_BOTTLE),
        _ => None,
    };
    let left = held.count - 1;
    if left > 0 {
        return Some(held.with_count(left));
    }
    remainder.map(|item| ItemStack::new(item, 1))
}

/// `getTarget()` of the mob in a tick.
fn target_in_tick(tick: &MobTick<'_>) -> Option<Looked> {
    let key = tick.mob.body.combat.target?;
    super::tamable::find(tick.world, key).filter(sensing::valid_target)
}

/// `getAmbientSound()`: asleep, its sleep's; in the dark, one time in ten, a screech
/// where no player is within 16; else its ambient.
fn ambient(tick: &mut MobTick<'_>) -> i32 {
    if state_of(&tick.mob).has(flag_bits::SLEEPING) {
        return sounds::ENTITY_FOX_SLEEP;
    }
    if tick.world.sky_darken() >= 4 && tick.draws.next_float() < 0.1 {
        let [x, y, z] = tick.living.body.position;
        let around = crate::physics::aabb::Aabb::of_entity(
            [x, y, z],
            tick.living.body.width,
            tick.living.body.height,
        )
        .inflate([16.0, 16.0, 16.0]);
        let alone = !tick.world.players().iter().any(|player| {
            !player.spectator
                && crate::physics::aabb::Aabb::of_entity(player.position, 0.6, 1.8)
                    .intersects(around)
        });
        if alone {
            return sounds::ENTITY_FOX_SCREECH;
        }
    }
    sounds::ENTITY_FOX_AMBIENT
}

impl MobType for Fox {
    fn register_goals_of(&self, mob: &Mob, setup: &mut Setup) {
        cow::animal_maluses(setup);
        // `setPathfindingMalus(DAMAGING_IN_NEIGHBOR, 0)` and `(DAMAGING, 0)`: berry bushes
        // spare it.
        setup.set_malus(clustine_data::PathType::DamagingInNeighbor, 0.0);
        setup.set_malus(clustine_data::PathType::Damaging, 0.0);
        setup.config.navigation.set_required_path_length(32.0);
        setup.add_goal(0, FoxFloatGoal);
        setup.add_goal(0, ClimbOnTopOfPowderSnowGoal);
        setup.add_goal(1, FaceplantGoal);
        setup.add_goal(2, FoxPanicGoal);
        setup.add_goal(3, FoxBreedGoal);
        for (threat, distance) in [
            (Threat::Player, 16.0),
            (Threat::Wolf, 8.0),
            (Threat::PolarBear, 8.0),
        ] {
            setup.add_goal(
                4,
                FoxAvoidGoal {
                    threat,
                    max_distance: distance,
                },
            );
        }
        setup.add_goal(5, StalkPreyGoal);
        setup.add_goal(6, FoxPounceGoal);
        setup.add_goal(6, SeekShelterGoal);
        setup.add_goal(7, FoxMeleeAttackGoal);
        setup.add_goal(7, SleepGoal);
        setup.add_goal(8, FoxFollowParentGoal);
        setup.add_goal(9, FoxStrollThroughVillageGoal);
        setup.add_goal(10, FoxEatBerriesGoal);
        setup.add_goal(10, LeapAtTargetGoal { yd: 0.4 });
        setup.add_goal(11, RandomStrollGoal::avoiding_water(1.0));
        setup.add_goal(11, FoxSearchForItemsGoal);
        setup.add_goal(12, FoxLookAtPlayerGoal);
        setup.add_goal(13, PerchAndSearchGoal);
        setup.add_target(3, DefendTrustedTargetGoal);
        let own = state_of(mob);
        if own.hunts {
            if own.variant == variant::RED {
                setup.add_target(4, HUNT_LAND);
                setup.add_target(4, HuntTurtlesGoal);
                setup.add_target(6, HUNT_FISH);
            } else {
                setup.add_target(4, HUNT_FISH);
                setup.add_target(6, HUNT_LAND);
                setup.add_target(6, HuntTurtlesGoal);
            }
        }
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        animal::walk_target_value(ai, pos, blocks::GRASS_BLOCK)
    }

    /// The constructor: its sleep goal's countdown drawn, and it picks up loot.
    fn construct(&self, mob: &mut Mob, own: &mut Random) {
        let countdown = own.next_int(70) as i32;
        update(mob, |state| state.sleep_countdown = countdown);
        mob.body.flags.picks_up_loot = true;
    }

    /// `Fox.aiStep` before `Animal`'s: its time since it ate counts on, food in its
    /// mouth is eaten after thirty seconds (and chewed before), and without a target it
    /// neither crouches nor is interested; asleep it does not jump or walk.
    fn before_ai_step(&self, tick: &mut MobTick<'_>) {
        if !tick.mob.body.vitals.dying() && tick.effective_ai() {
            let hand = usize::from(slots::MAIN_HAND);
            let mut own = state_of(&tick.mob);
            let now = tick.lived;
            let since = own.ticks_since_eaten(now);
            let held = &tick.mob.body.equipment.slots[hand];
            let target = target_in_tick(tick);
            let can_eat = is_consumable_food(held.as_ref())
                && target.is_none()
                && tick.living.body.on_ground
                && !own.has(flag_bits::SLEEPING);
            if can_eat && let Some(stack) = held {
                if since > 600 {
                    tick.mob.body.equipment.slots[hand] = finish_eating(stack.clone());
                    tick.out.events.push(Event::GameEvent(GameEvent::Eat));
                    own.eaten_at = Some(now);
                } else if since > 560 && tick.draws.next_float() < 0.1 {
                    tick.out.events.push(Event::Sound {
                        sound: sounds::ENTITY_FOX_EAT,
                        volume: 1.0,
                        pitch: 1.0,
                    });
                    tick.out.events.push(Event::Entity(45));
                }
            }
            if target.is_none_or(|target| !target.alive) {
                own.set(flag_bits::CROUCHING, false);
                own.set(flag_bits::INTERESTED, false);
            }
            put_state(&mut tick.mob, &own);
        }
        if state_of(&tick.mob).has(flag_bits::SLEEPING) || tick.mob.body.vitals.dying() {
            let inputs = &mut tick.mob.body.inputs;
            inputs.jumping = false;
            inputs.xxa = 0.0;
            inputs.zza = 0.0;
        }
    }

    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::custom_server_ai_step(tick);
    }

    /// `FoxMoveControl`: it moves only when it can.
    fn tick_move_control(&self, ai: &mut Ai<'_, '_>) {
        if state_of(&ai.tick.mob).can_move() {
            controls::tick_move(ai);
        }
    }

    /// `FoxLookControl`: asleep it turns nothing; pouncing, crouching, interested or
    /// faceplanted it keeps its pitch.
    fn tick_look_control(&self, ai: &mut Ai<'_, '_>) {
        use flag_bits::*;
        let own = state_of(&ai.tick.mob);
        if own.has(SLEEPING) {
            return;
        }
        let keep =
            own.has(POUNCING) || own.has(CROUCHING) || own.has(INTERESTED) || own.has(FACEPLANTED);
        controls::tick_plain_look_resetting(ai, !keep);
    }

    /// `Mob.aiStep`'s pick-up, `Animal.aiStep`, then the fox's own: a defender's growl
    /// now and then; and `Fox.tick`'s after the living tick: water, a target or thunder
    /// wake it, water or sleep stand it up, a faceplanted fox digs, and its crouch
    /// deepens by a fifth a tick to five.
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        pick_up(tick);
        animal::after_ai_step(tick);
        if state_of(&tick.mob).has(flag_bits::DEFENDING) && tick.draws.next_float() < 0.05 {
            tick.out.events.push(Event::Sound {
                sound: sounds::ENTITY_FOX_AGGRO,
                volume: 1.0,
                pitch: 1.0,
            });
        }
        let mut own = state_of(&tick.mob);
        if tick.effective_ai() {
            let in_water = tick.living.fluids.in_water();
            if in_water || target_in_tick(tick).is_some() {
                own.set(flag_bits::SLEEPING, false);
            }
            if in_water || own.has(flag_bits::SLEEPING) {
                own.set(flag_bits::SITTING, false);
            }
            if own.has(flag_bits::FACEPLANTED) && tick.draws.next_float() < 0.2 {
                let at = physics::containing(tick.living.body.position);
                let block = tick.world.state(at);
                tick.out.local.push(Local::LevelEvent {
                    kind: 2001,
                    pos: at,
                    data: i32::from(block.0),
                });
            }
        }
        if own.has(flag_bits::CROUCHING) {
            own.crouch_amount += 0.2;
            if own.crouch_amount > 5.0 {
                own.crouch_amount = 5.0;
            }
        } else {
            own.crouch_amount = 0.0;
        }
        put_state(&mut tick.mob, &own);
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        animal::mob_interact(used, FOOD, None)
    }

    /// `finalizeSpawn`: the variant its biome gives (snowy in `#spawns_snow_foxes`),
    /// or its group's, a cub from the group's third on (`FoxGroupData`, which wants no
    /// babies of `AgeableMob`'s); its hunting goals; one time in five something in its
    /// mouth, all of the level's draws; then `Animal`'s.
    ///
    /// The hunting goals are set where the level is a `ServerLevel`, which a chunk's
    /// generation is not; but the load of the chunk sets them again, so a generated fox
    /// hunts as an egg's does.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let mut variant = if biome_in_tag(spawn.biome, "minecraft:spawns_snow_foxes") {
            variant::SNOW
        } else {
            variant::RED
        };
        let mut cub = false;
        match spawn.group {
            Some(SpawnGroup::Fox {
                ageable,
                variant: shared,
            }) => {
                variant = *shared;
                cub = ageable.made >= 2;
            }
            _ => {
                *spawn.group = Some(SpawnGroup::Fox {
                    ageable: AgeableGroup::new(false),
                    variant,
                });
            }
        }
        update(spawn.mob, |own| {
            own.variant = variant;
            own.hunts = true;
        });
        if cub {
            animal::update(spawn.mob, |shared| {
                shared.set_age(animal::BABY_START_AGE, 0);
            });
        }
        // `populateDefaultEquipmentSlots`.
        let level = &mut *spawn.level;
        if level.next_float() < 0.2 {
            let f = level.next_float();
            let item = if f < 0.05 {
                items::EMERALD
            } else if f < 0.2 {
                items::EGG
            } else if f < 0.4 {
                if level.next_bool() {
                    items::RABBIT_FOOT
                } else {
                    items::RABBIT_HIDE
                }
            } else if f < 0.6 {
                items::WHEAT
            } else if f < 0.8 {
                items::LEATHER
            } else {
                items::FEATHER
            };
            spawn.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] =
                Some(ItemStack::new(item, 1));
        }
        animal::finalize_ageable_default(spawn);
    }

    /// `getBreedOffspring`: a cub of one parent's variant or the other's, by the
    /// breeding parent's draw; one an egg made trusts the egg's player.
    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        let this = state_of(breeding.this);
        let partner = state_of(breeding.partner);
        let variant = if breeding.draws.next_bool() {
            this.variant
        } else {
            partner.variant
        };
        let mut child = Mob::fresh();
        let mut own = FoxState {
            variant,
            ..FoxState::default()
        };
        if let Some(player) = breeding.egg_user {
            own.add_trusted(player);
        }
        child.state = animal::write(&animal::Animal::default(), &own);
        Some(child)
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        animal::dimensions(mob, BABY)
    }

    fn ambient_sound(&self, mob: &Mob) -> Option<i32> {
        Some(if state_of(mob).has(flag_bits::SLEEPING) {
            sounds::ENTITY_FOX_SLEEP
        } else {
            sounds::ENTITY_FOX_AMBIENT
        })
    }

    /// `Fox.playAmbientSound`: a screech at twice the volume, else `Mob`'s, which asks
    /// for the sound again (and draws again in the dark).
    fn play_ambient_sound(&self, tick: &mut MobTick<'_>) {
        let first = ambient(tick);
        if first == sounds::ENTITY_FOX_SCREECH {
            make_sound(tick, Some(first), 2.0);
        } else {
            let second = ambient(tick);
            make_sound(tick, Some(second), 1.0);
        }
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_FOX_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_FOX_DEATH)
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let own = state_of(mob);
        let mut values = animal::shown(mob);
        values.push((
            TYPE_INDEX,
            DataShown::Int(own.variant),
            own.variant == variant::RED,
        ));
        values.push((FLAGS_INDEX, DataShown::Byte(own.flags), own.flags == 0));
        for (index, trusted) in TRUSTED_INDEX.into_iter().zip(own.trusted) {
            values.push((index, DataShown::OptionalUuid(trusted), trusted.is_none()));
        }
        values
    }
}
