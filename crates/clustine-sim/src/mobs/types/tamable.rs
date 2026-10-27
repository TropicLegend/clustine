//! What tamed animals share (`TamableAnimal`, `OwnableEntity`, `EntityReference`; the
//! brief's sections 5.1 and 5.2): the tame and sitting flags, the owner by UUID, sitting
//! when ordered, following the owner and teleporting to them, panicking towards them,
//! defending them and attacking what they attack, and taming.
//!
//! Each tamed type keeps a [`Tame`] in its own state and says how it reads and writes
//! it ([`Pet`]); the goals here are built with their type's [`Pet`], so that they read
//! any of them.
//!
//! An owner is found among the players the mob reads (the region's own and the faces of
//! others in its copies), then among the world's players the store publishes once a
//! second (`docs/groundwork/n7-plan.md`, section 7.3, item 3): an owner beyond the
//! copies is followed a second late, and an owner offline makes a tamed animal sit, as
//! the game's does (the brief's finding 5.0.1).
//!
//! Named differences: a teleport to an owner beyond the copies finds no blocks to stand
//! on and does not happen (the effect to the owner's region is the second half's, the
//! plan's 8.3.5), so the pet waits where it is until the owner comes back within a chunk
//! of the region; the owner's last damage source is read as the mob that last hurt them,
//! within 100 ticks (no player is hurt yet, every player being creative); a pet takes no
//! team of its owner, and its death is told to nobody.

use clustine_blocks::data::is_class;
use clustine_data::{BlockClass, PathType, attributes, items};
use clustine_world::{BlockPos, EntityId};
use serde::{Deserialize, Serialize};

use super::{Did, Used};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{Panic, PanicGoal, TargetRules, Targeting};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::{self, Class, Conditions, Looked};
use crate::mobs::ai::{Ai, controls, navigation, pathfinder};
use crate::mobs::body::attribute;
use crate::mobs::view::{MobWorld, SeenPlayer};
use crate::physics::aabb::Aabb;
use crate::physics::collide;
use crate::shown::DataShown;

/// The synced data's indices of `TamableAnimal`: its flags and its owner.
pub(crate) const FLAGS_INDEX: u8 = 18;
pub(crate) const OWNER_INDEX: u8 = 19;

/// `DATA_FLAGS_ID`'s bits: in the sitting pose, and tame.
const SITTING: u8 = 1;
const TAME: u8 = 4;

/// What `TamableAnimal` keeps: its synced flags, its owner by UUID, and whether it was
/// ordered to sit (`Sitting`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Tame {
    pub(crate) flags: u8,
    pub(crate) owner: Option<u128>,
    pub(crate) ordered_to_sit: bool,
}

impl Tame {
    /// `isTame()`.
    pub(crate) fn is_tame(&self) -> bool {
        self.flags & TAME != 0
    }

    /// `isInSittingPose()`.
    pub(crate) fn in_sitting_pose(&self) -> bool {
        self.flags & SITTING != 0
    }

    /// `setInSittingPose(sitting)`.
    pub(crate) fn set_in_sitting_pose(&mut self, sitting: bool) {
        if sitting {
            self.flags |= SITTING;
        } else {
            self.flags &= !SITTING;
        }
    }

    /// `setTame(tame, …)`'s flag; the side effects are the type's.
    pub(crate) fn set_tame_flag(&mut self, tame: bool) {
        if tame {
            self.flags |= TAME;
        } else {
            self.flags &= !TAME;
        }
    }

    /// `isOwnedBy(player)`: the owner is the player of this UUID.
    pub(crate) fn is_owned_by(&self, player: Option<u128>) -> bool {
        self.owner.is_some() && self.owner == player
    }

    /// `TamableAnimal`'s synced values: its flags (18) and its owner (19).
    pub(crate) fn shown(&self) -> [(u8, DataShown, bool); 2] {
        [
            (FLAGS_INDEX, DataShown::Byte(self.flags), self.flags == 0),
            (
                OWNER_INDEX,
                DataShown::OptionalUuid(self.owner),
                self.owner.is_none(),
            ),
        ]
    }
}

/// A type of tamed animal, as the shared goals read and write it.
pub(crate) trait Pet: Sync + 'static {
    /// Its [`Tame`], from its type state.
    fn tame(&self, mob: &Mob) -> Tame;

    /// Its type state with its [`Tame`] made `tame`.
    fn set_tame(&self, mob: &mut Mob, tame: Tame);

    /// `canFlyToOwner()`: a parrot lands on leaves.
    fn can_fly_to_owner(&self) -> bool {
        false
    }

    /// `wantsToAttack(target, owner)`: `TamableAnimal`'s takes anything.
    fn wants_to_attack(&self, world: &dyn MobWorld, target: &Looked, owner: &SeenPlayer) -> bool {
        let _ = (world, target, owner);
        true
    }
}

/// Changes the [`Tame`] of `mob`.
pub(crate) fn update(pet: &dyn Pet, mob: &mut Mob, change: impl FnOnce(&mut Tame)) {
    let mut tame = pet.tame(mob);
    let before = tame.clone();
    change(&mut tame);
    if tame != before {
        pet.set_tame(mob, tame);
    }
}

/// `getOwner()`: the owner by UUID wherever the view finds them (see the module's
/// comment); `None` for none or one offline.
pub(crate) fn owner(world: &dyn MobWorld, tame: &Tame) -> Option<SeenPlayer> {
    world.player_anywhere(tame.owner?)
}

/// The id of the sound event `name` (a variant's sounds are named by their sets).
pub(crate) fn sound(name: &str) -> Option<i32> {
    clustine_data::SOUND_EVENTS
        .iter()
        .position(|known| *known == name)
        .and_then(|at| i32::try_from(at).ok())
}

/// `distanceToSqr(entity)`, feet to feet.
pub(crate) fn distance_sqr(from: [f64; 3], to: [f64; 3]) -> f64 {
    crate::physics::distance_sqr(from, to)
}

/// `TamableAnimal.unableToMoveToOwner()`: ordered to sit, riding, on a lead, or the
/// owner a spectator.
pub(crate) fn unable_to_move_to_owner(
    ai: &Ai<'_, '_>,
    tame: &Tame,
    owner: Option<&SeenPlayer>,
) -> bool {
    tame.ordered_to_sit
        || ai.tick.vehicle.is_some()
        || ai.tick.leashed
        || owner.is_some_and(|owner| owner.spectator)
}

/// `shouldTryTeleportToOwner()`: twelve blocks or more from its owner.
pub(crate) fn should_try_teleport_to_owner(ai: &Ai<'_, '_>, owner: &SeenPlayer) -> bool {
    distance_sqr(ai.position(), owner.position) >= 144.0
}

/// `RandomSource.nextIntBetweenInclusive(low, high)`.
fn between(ai: &mut Ai<'_, '_>, low: i32, high: i32) -> i32 {
    ai.tick.draws.next_int((high - low + 1) as u32) as i32 + low
}

/// `tryToTeleportToOwner()` (`teleportToAroundBlockPos`): ten tries of a spot two or
/// three blocks from the owner's block, a block up or down, the first it may stand at.
pub(crate) fn try_to_teleport_to_owner(ai: &mut Ai<'_, '_>, pet: &dyn Pet, owner: &SeenPlayer) {
    let at = crate::physics::containing(owner.position);
    for _ in 0..10 {
        let dx = between(ai, -3, 3);
        let dz = between(ai, -3, 3);
        if dx.abs() < 2 && dz.abs() < 2 {
            continue;
        }
        let dy = between(ai, -1, 1);
        if maybe_teleport_to(ai, pet, BlockPos::new(at.x + dx, at.y + dy, at.z + dz)) {
            return;
        }
    }
}

/// `maybeTeleportTo(x, y, z)`: where it may stand there, `snapTo` the block's bottom
/// middle facing as it does, and its path dropped.
fn maybe_teleport_to(ai: &mut Ai<'_, '_>, pet: &dyn Pet, pos: BlockPos) -> bool {
    if !can_teleport_to(ai, pet, pos) {
        return false;
    }
    ai.tick.living.body.position = [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y),
        f64::from(pos.z) + 0.5,
    ];
    // `snapTo` is a jump the client is told of at once.
    ai.tick.living.body.sync_position = true;
    navigation::stop(ai);
    true
}

/// `canTeleportTo(pos)`: a cell its path finder walks, not above leaves for a pet that
/// cannot fly, and room for its box moved by the blocks between.
fn can_teleport_to(ai: &Ai<'_, '_>, pet: &dyn Pet, pos: BlockPos) -> bool {
    let world = ai.tick.world;
    if pathfinder::static_path_type(world, world.min_y(), pos) != PathType::Walkable {
        return false;
    }
    let below = world.state(BlockPos::new(pos.x, pos.y - 1, pos.z));
    if !pet.can_fly_to_owner() && is_class(below, BlockClass::LeavesBlock) {
        return false;
    }
    let here = ai.block_position();
    let by = [
        f64::from(pos.x - here.x),
        f64::from(pos.y - here.y),
        f64::from(pos.z - here.z),
    ];
    let bounds = Aabb::of_entity(ai.position(), ai.width(), ai.height()).moved(by);
    let body = &ai.tick.living.body;
    collide::no_collision(world, body.context(), &body.mover, &bounds)
}

/// `heal(amount)`: health back, never past its most, for a living mob.
pub(crate) fn heal(mob: &mut Mob, kind: u16, amount: f32) {
    let health = mob.body.vitals.health;
    if health <= 0.0 {
        return;
    }
    let max = mob
        .body
        .attributes
        .value(kind, attribute(attributes::MAX_HEALTH)) as f32;
    mob.body.vitals.health = (health + amount).clamp(0.0, max);
}

/// `getHealth() < getMaxHealth()`.
pub(crate) fn hurt(mob: &Mob, kind: u16) -> bool {
    let max = mob
        .body
        .attributes
        .value(kind, attribute(attributes::MAX_HEALTH)) as f32;
    mob.body.vitals.health < max
}

/// The nutrition of a food's `minecraft:food` component, of the foods a pet eats (the
/// vanilla items'; the extraction does not carry food yet, the brief's 5.19).
pub(crate) fn nutrition(item: i32) -> Option<i32> {
    let value = match item {
        items::BEEF | items::PORKCHOP | items::RABBIT => 3,
        items::COOKED_BEEF | items::COOKED_PORKCHOP => 8,
        items::CHICKEN | items::MUTTON | items::COD | items::SALMON => 2,
        items::COOKED_CHICKEN | items::COOKED_MUTTON | items::COOKED_SALMON => 6,
        items::COOKED_RABBIT | items::COOKED_COD => 5,
        items::ROTTEN_FLESH => 4,
        items::TROPICAL_FISH | items::PUFFERFISH => 1,
        items::RABBIT_STEW => 10,
        _ => return None,
    };
    Some(value)
}

/// `TamableAnimal.feed(player, hand, stack, perNutrition, otherwise)`: the item eaten
/// (a creative player keeps it), health back by its nutrition times `per_nutrition`, or
/// `otherwise` for an item of no food, then the type's eating sound, if any.
pub(crate) fn feed(used: &mut Used<'_>, per_nutrition: f32, otherwise: f32, eat: Option<Did>) {
    let amount = used
        .held
        .as_ref()
        .and_then(|stack| nutrition(stack.item))
        .map_or(otherwise, |value| per_nutrition * value as f32);
    heal(used.mob, used.kind, amount);
    used.did.extend(eat);
}

/// `tame(player)`: tame, without the type's side effects, which the caller applies,
/// owned by the player.
pub(crate) fn tame(tame: &mut Tame, player: Option<u128>) {
    tame.set_tame_flag(true);
    tame.owner = player;
}

/// The thing the key names wherever the view has it, a mob of a chunk or a player.
pub(crate) fn find(world: &dyn MobWorld, key: EntityId) -> Option<Looked> {
    if let Some(seen) = world.entity(key) {
        return Some(Looked::of_seen(&seen));
    }
    world
        .players()
        .iter()
        .find(|player| player.key == key)
        .map(Looked::of_player)
}

/// `SitWhenOrderedToGoal(tamable)`: sit while ordered to, or wherever the owner is not
/// to be found, on the ground and out of water; stand to defend an owner hurt near by.
#[derive(Clone, Copy)]
pub(crate) struct SitWhenOrderedToGoal {
    pub(crate) pet: &'static dyn Pet,
}

impl Goal for SitWhenOrderedToGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::JUMP | flags::MOVE
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        let tame = self.pet.tame(&ai.tick.mob);
        let sit = tame.ordered_to_sit;
        if !sit && !tame.is_tame() {
            return false;
        }
        if ai.in_water() || !ai.on_ground() {
            return false;
        }
        let Some(owner) = owner(ai.tick.world, &tame) else {
            return true;
        };
        let now = ai.tick.world.game_time();
        let hurt_lately = owner
            .last_hurt_by
            .is_some_and(|(_, at)| now - at as i64 <= 100);
        if distance_sqr(ai.position(), owner.position) < 144.0 && hurt_lately {
            return false;
        }
        sit
    }

    fn can_continue(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        self.pet.tame(&ai.tick.mob).ordered_to_sit
    }

    fn start(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        navigation::stop(ai);
        update(self.pet, &mut ai.tick.mob, |tame| {
            tame.set_in_sitting_pose(true)
        });
    }

    fn stop(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        update(self.pet, &mut ai.tick.mob, |tame| {
            tame.set_in_sitting_pose(false)
        });
    }
}

/// `FollowOwnerGoal`'s state: the owner it follows, by UUID, the ticks to its next
/// path, and the water's cost it put aside.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct FollowingOwner {
    pub(crate) owner: Option<u128>,
    pub(crate) recalc: i32,
    pub(crate) old_water: Option<f32>,
}

/// `FollowOwnerGoal(tamable, speed, startDistance, stopDistance)`: after its owner from
/// `start` blocks until within `stop`, teleporting to them from twelve.
#[derive(Clone, Copy)]
pub(crate) struct FollowOwnerGoal {
    pub(crate) pet: &'static dyn Pet,
    pub(crate) speed: f64,
    pub(crate) start: f32,
    pub(crate) stop: f32,
}

impl Goal for FollowOwnerGoal {
    type State = FollowingOwner;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    fn can_use(&self, state: &mut FollowingOwner, ai: &mut Ai<'_, '_>) -> bool {
        let tame = self.pet.tame(&ai.tick.mob);
        let Some(owner) = owner(ai.tick.world, &tame) else {
            return false;
        };
        if unable_to_move_to_owner(ai, &tame, Some(&owner)) {
            return false;
        }
        if distance_sqr(ai.position(), owner.position) < f64::from(self.start * self.start) {
            return false;
        }
        state.owner = tame.owner;
        true
    }

    fn can_continue(&self, state: &mut FollowingOwner, ai: &mut Ai<'_, '_>) -> bool {
        if navigation::is_done(ai) {
            return false;
        }
        let tame = self.pet.tame(&ai.tick.mob);
        let followed = state
            .owner
            .and_then(|uuid| ai.tick.world.player_anywhere(uuid));
        if unable_to_move_to_owner(ai, &tame, followed.as_ref()) {
            return false;
        }
        followed.is_some_and(|owner| {
            distance_sqr(ai.position(), owner.position) > f64::from(self.stop * self.stop)
        })
    }

    fn start(&self, state: &mut FollowingOwner, ai: &mut Ai<'_, '_>) {
        state.recalc = 0;
        state.old_water = Some(ai.malus(PathType::Water));
        ai.set_malus(PathType::Water, 0.0);
    }

    fn stop(&self, state: &mut FollowingOwner, ai: &mut Ai<'_, '_>) {
        state.owner = None;
        navigation::stop(ai);
        if let Some(old) = state.old_water.take() {
            ai.set_malus(PathType::Water, old);
        }
    }

    fn tick(&self, state: &mut FollowingOwner, ai: &mut Ai<'_, '_>) {
        let Some(owner) = state
            .owner
            .and_then(|uuid| ai.tick.world.player_anywhere(uuid))
        else {
            return;
        };
        let teleport = should_try_teleport_to_owner(ai, &owner);
        if !teleport {
            let max_pitch = ai.head().max_x_rot as f32;
            let eyes = [
                owner.position[0],
                owner.position[1] + owner.eye_height,
                owner.position[2],
            ];
            controls::set_look_at(ai, eyes, 10.0, max_pitch);
        }
        state.recalc -= 1;
        if state.recalc > 0 {
            return;
        }
        state.recalc = self.adjusted_tick_delay(10);
        if teleport {
            try_to_teleport_to_owner(ai, self.pet, &owner);
        } else {
            let at = crate::physics::containing(owner.position);
            navigation::move_to_entity(ai, at, self.speed);
        }
    }
}

/// `TamableAnimal.TamableAnimalPanicGoal(speed[, damageTypes])`: a panic that first
/// teleports to an owner left far behind. **Named**: the body keeps no damage source,
/// so a panic of the environmental causes alone (a wolf's) never starts.
#[derive(Clone, Copy)]
pub(crate) struct TamablePanicGoal {
    pub(crate) pet: &'static dyn Pet,
    pub(crate) speed: f64,
    /// Of `#panic_environmental_causes` only, not of a hit.
    pub(crate) environmental: bool,
}

impl Goal for TamablePanicGoal {
    type State = Panic;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn panics(&self) -> bool {
        true
    }

    fn can_use(&self, state: &mut Panic, ai: &mut Ai<'_, '_>) -> bool {
        !self.environmental && PanicGoal { speed: self.speed }.can_use(state, ai)
    }

    fn can_continue(&self, state: &mut Panic, ai: &mut Ai<'_, '_>) -> bool {
        PanicGoal { speed: self.speed }.can_continue(state, ai)
    }

    fn start(&self, state: &mut Panic, ai: &mut Ai<'_, '_>) {
        PanicGoal { speed: self.speed }.start(state, ai);
    }

    fn stop(&self, state: &mut Panic, ai: &mut Ai<'_, '_>) {
        PanicGoal { speed: self.speed }.stop(state, ai);
    }

    fn tick(&self, _: &mut Panic, ai: &mut Ai<'_, '_>) {
        let tame = self.pet.tame(&ai.tick.mob);
        let Some(owner) = owner(ai.tick.world, &tame) else {
            return;
        };
        if !unable_to_move_to_owner(ai, &tame, Some(&owner))
            && should_try_teleport_to_owner(ai, &owner)
        {
            try_to_teleport_to_owner(ai, self.pet, &owner);
        }
    }
}

/// The rules of a target goal that need not see its target (`TargetGoal(mob, false)`).
const UNSEEN: TargetRules = TargetRules {
    must_see: false,
    must_reach: false,
};

/// `OwnerHurtByTargetGoal(tamable)`: the mob that hurt its owner, once for each hurt,
/// unless the hurt was of `#no_wolf_retaliation`.
#[derive(Clone, Copy)]
pub(crate) struct OwnerHurtByTargetGoal {
    pub(crate) pet: &'static dyn Pet,
}

/// What the owner goals keep beyond a target goal's: the stamp of the hurt answered, and
/// what is to be attacked.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Defending {
    pub(crate) targeting: Targeting,
    pub(crate) stamp: Option<u64>,
    pub(crate) attacker: Option<EntityId>,
}

/// What an owner goal reads of the owner: whom (their last attacker, or what they last
/// hurt) and the stamp of it.
type Read = fn(&SeenPlayer) -> Option<(EntityId, u64)>;

/// The owner goals' `canUse`: tame, not ordered to sit, an owner whose `read` is new
/// since the last answered, attackable by the default conditions and wanted.
fn owner_goal_can_use(
    pet: &dyn Pet,
    read: Read,
    state: &mut Defending,
    ai: &mut Ai<'_, '_>,
) -> bool {
    let tame = pet.tame(&ai.tick.mob);
    if !tame.is_tame() || tame.ordered_to_sit {
        return false;
    }
    let Some(owner) = owner(ai.tick.world, &tame) else {
        return false;
    };
    let Some((attacker, stamp)) = read(&owner) else {
        return false;
    };
    state.attacker = Some(attacker);
    if state.stamp == Some(stamp) {
        return false;
    }
    let world = ai.tick.world;
    let Some(target) = find(world, attacker) else {
        return false;
    };
    let conditions = Conditions::combat();
    UNSEEN.can_attack(
        &mut state.targeting,
        ai,
        Some(&target),
        &conditions,
        &|_| true,
    ) && pet.wants_to_attack(world, &target, &owner)
}

/// The owner goals' `start`: the target taken, the stamp answered.
fn owner_goal_start(pet: &dyn Pet, read: Read, state: &mut Defending, ai: &mut Ai<'_, '_>) {
    let target = state.attacker.and_then(|key| find(ai.tick.world, key));
    sensing::set_target(ai, target.as_ref());
    let tame = pet.tame(&ai.tick.mob);
    if let Some(owner) = owner(ai.tick.world, &tame) {
        state.stamp = read(&owner).map(|(_, stamp)| stamp);
    }
    TargetRules::start(&mut state.targeting);
}

/// The owner's last attacker, within the 100 ticks of `getLastDamageSource(100)` (see
/// the module's comment).
fn last_hurt_by(owner: &SeenPlayer) -> Option<(EntityId, u64)> {
    owner.last_hurt_by
}

/// What the owner last hurt (`getLastHurtMob`, which does not lapse).
fn last_hurt_mob(owner: &SeenPlayer) -> Option<(EntityId, u64)> {
    owner.last_hurt_mob
}

impl Goal for OwnerHurtByTargetGoal {
    type State = Defending;

    fn flags(&self) -> u8 {
        flags::TARGET
    }

    fn can_use(&self, state: &mut Defending, ai: &mut Ai<'_, '_>) -> bool {
        // `getLastDamageSource(100)`: a hurt of the last hundred ticks.
        let now = ai.tick.world.game_time();
        let tame = self.pet.tame(&ai.tick.mob);
        let recent = owner(ai.tick.world, &tame)
            .and_then(|owner| owner.last_hurt_by)
            .is_some_and(|(_, at)| now - at as i64 <= 100);
        recent && owner_goal_can_use(self.pet, last_hurt_by, state, ai)
    }

    fn can_continue(&self, state: &mut Defending, ai: &mut Ai<'_, '_>) -> bool {
        UNSEEN.can_continue(&mut state.targeting, ai)
    }

    fn start(&self, state: &mut Defending, ai: &mut Ai<'_, '_>) {
        owner_goal_start(self.pet, last_hurt_by, state, ai);
    }

    fn stop(&self, state: &mut Defending, ai: &mut Ai<'_, '_>) {
        TargetRules::stop(&mut state.targeting, ai);
    }
}

/// `OwnerHurtTargetGoal(tamable)`: what its owner hurt, once for each hurt.
#[derive(Clone, Copy)]
pub(crate) struct OwnerHurtTargetGoal {
    pub(crate) pet: &'static dyn Pet,
}

impl Goal for OwnerHurtTargetGoal {
    type State = Defending;

    fn flags(&self) -> u8 {
        flags::TARGET
    }

    fn can_use(&self, state: &mut Defending, ai: &mut Ai<'_, '_>) -> bool {
        owner_goal_can_use(self.pet, last_hurt_mob, state, ai)
    }

    fn can_continue(&self, state: &mut Defending, ai: &mut Ai<'_, '_>) -> bool {
        UNSEEN.can_continue(&mut state.targeting, ai)
    }

    fn start(&self, state: &mut Defending, ai: &mut Ai<'_, '_>) {
        owner_goal_start(self.pet, last_hurt_mob, state, ai);
    }

    fn stop(&self, state: &mut Defending, ai: &mut Ai<'_, '_>) {
        TargetRules::stop(&mut state.targeting, ai);
    }
}

/// `NearestAttackableTargetGoal.findTarget()` with a selector that may read the world:
/// of the things of `class` the combat conditions within the follow range take, the
/// nearest to its eyes' height.
pub(crate) fn nearest_target(
    ai: &mut Ai<'_, '_>,
    class: Class,
    selector: &dyn Fn(&Looked) -> bool,
) -> Option<Looked> {
    let follow = ai.attribute(attributes::FOLLOW_RANGE);
    let conditions = Conditions::combat().range(follow);
    let [x, _, z] = ai.position();
    let eyes = [x, ai.eye_y(), z];
    let candidates: Vec<Looked> = if class == Class::Player {
        sensing::players(ai)
    } else {
        let bounds = Aabb::of_entity(ai.position(), ai.width(), ai.height())
            .inflate([follow, follow, follow]);
        sensing::things_in(ai, bounds, class)
    };
    // The selector is pure: it is asked first, as the conditions would ask it.
    let candidates = candidates
        .into_iter()
        .filter(|looked| selector(looked))
        .collect();
    sensing::nearest(ai, candidates, &conditions, &|_| true, eyes)
}

/// A selector of things a mob's target goal takes, given the world it reads.
pub(crate) type Selector = fn(&dyn MobWorld, &Looked) -> bool;

/// `NonTameRandomTargetGoal(tamable, class, mustSee, selector)`: a wild one's nearest
/// prey, every tenth tick or so, kept while the conditions still take it.
#[derive(Clone, Copy)]
pub(crate) struct NonTameRandomTargetGoal {
    pub(crate) pet: &'static dyn Pet,
    pub(crate) class: Class,
    pub(crate) selector: Option<Selector>,
}

impl NonTameRandomTargetGoal {
    /// The conditions it was made with: the combat's, within the follow range its type
    /// had when it was made (before a spawn's bonus), with its selector.
    fn holds(&self, ai: &mut Ai<'_, '_>, target: &Looked) -> bool {
        let follow = crate::mobs::body::attributes::supplied(
            ai.tick.kind,
            attribute(attributes::FOLLOW_RANGE),
        )
        .unwrap_or(16.0);
        let world = ai.tick.world;
        let selector = self.selector;
        let test = move |looked: &Looked| selector.is_none_or(|select| select(world, looked));
        Conditions::combat().range(follow).test(ai, target, &test)
    }
}

impl Goal for NonTameRandomTargetGoal {
    type State = Targeting;

    fn flags(&self) -> u8 {
        flags::TARGET
    }

    fn can_use(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        if self.pet.tame(&ai.tick.mob).is_tame() {
            return false;
        }
        let interval = crate::mobs::ai::maths::reduced_tick_delay(10);
        if ai.tick.draws.next_int(interval as u32) != 0 {
            return false;
        }
        let world = ai.tick.world;
        let selector = self.selector;
        let found = nearest_target(ai, self.class, &move |looked| {
            selector.is_none_or(|select| select(world, looked))
        });
        state.found = found.map(|found| found.key);
        state.found.is_some()
    }

    fn can_continue(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        let Some(target) = state.found.and_then(|key| find(ai.tick.world, key)) else {
            return false;
        };
        self.holds(ai, &target)
    }

    fn start(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        let found = state.found.and_then(|key| find(ai.tick.world, key));
        sensing::set_target(ai, found.as_ref());
        TargetRules::start(state);
    }

    fn stop(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        TargetRules::stop(state, ai);
    }
}

/// `Turtle.BABY_ON_LAND_SELECTOR`: a baby turtle out of water. **Named**: no turtle's
/// age is read until the turtle's part; none is taken.
pub(crate) fn baby_turtle_on_land(world: &dyn MobWorld, looked: &Looked) -> bool {
    let Some(seen) = world.entity(looked.key) else {
        return false;
    };
    let Ok(mob) = Mob::decode(&seen.entity.data) else {
        return false;
    };
    let in_water = clustine_blocks::data::has_water(world.state(looked.block_position()));
    super::of(seen.entity.kind).is_baby(&mob) && !in_water
}

/// A goal that never starts, holding its place among a type's goals for what it does
/// not do yet (a parrot landing on a shoulder, a universal anger's reset).
#[derive(Clone, Copy)]
pub(crate) struct Idle {
    pub(crate) flags: u8,
}

impl Goal for Idle {
    type State = ();

    fn flags(&self) -> u8 {
        self.flags
    }

    fn can_use(&self, _: &mut (), _: &mut Ai<'_, '_>) -> bool {
        false
    }
}
