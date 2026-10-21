//! Camels (`Camel`; `docs/groundwork/n7-brief.md`, section 7.7), of the horse family
//! (`AbstractHorse`, which [`super::equine`] keeps) and of their own: always tamed, so
//! never tamed, angered or rearing; of cactus, which heals one, puts a grown one in love
//! and grows a calf; grazing, healing and swishing its tail as a horse does; saddled by
//! a click; sitting down and standing up by a pose stamped with the world's game time
//! (`LAST_POSE_CHANGE_TICK`), which clients compare with their own; dashing where a
//! rider jumps. The camel husk ([`super::camel_husk`]) is the same body of its own
//! [`Breed`].
//!
//! Its record is the horse family's ([`HorseState`] after the animal's) with the camel's
//! own after it ([`CamelState`], with a version byte of its own), as the pig's boost is
//! kept after the pig's state: what the family's code writes of a horse it is called for
//! would end the record before the camel's, so every write here keeps it.
//!
//! Its mind is a brain (`CamelAi`), which N7j's machinery brings: until its provider is
//! wired a camel has no goals (`registerGoals` adds none) and stands, grazes and falls by
//! its body alone. What the brain will call is in [`brain`]: sitting down at random,
//! standing up when it panics, the move control's standing up for a walk. Riding is the
//! lead's (part R): what it needs of a camel is in [`riding`] as plain functions, and no
//! click mounts a player yet.
//!
//! Named differences: a sneaking click opens the camel's menu by the family's
//! [`Did::OpenInventory`], which the region opens only for the horses of
//! [`super::equine::of`] until it takes the camel too (asked of the lead), and a saddle
//! put on in that menu would sound a horse's; the golden dandelion is looked for in the
//! hand used, not in either hand; `isSilent` is never true (no mob of Clustine is
//! silent); a sitting camel's `BodyRotationControl` holds still but its stillness counts
//! on, as a stamp does.

use clustine_blocks::data::in_tag;
use clustine_data::{ItemComponents, attributes as ids, entity_types, items, sounds};
use serde::{Deserialize, Serialize};

use super::animal::{self, Animal};
use super::equine::{self, Behaviour, Equine, HorseState};
use super::{Breeding, Did, Dimensions, Interaction, Mate, MobType, Spawning, Used};
use crate::living::{Landing, slots};
use crate::mobs::Mob;
use crate::mobs::ai::goals::Items;
use crate::mobs::ai::{Ai, HeadLimits, Operation, Setup, controls};
use crate::mobs::body::hurt::{self, DamageSource, GameEvent, Struck, voice_pitch};
use crate::mobs::body::{Event, attribute};
use crate::mobs::tick::{MobTick, Ridden};
use crate::physics::Vec3;
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::region::Random;
use crate::shown::DataShown;
use clustine_world::{BlockPos, ItemStack};

/// The synced data's indices: the pose (`Entity.DATA_POSE`), the dash and the last pose
/// change's tick, after the family's flags (18).
const POSE_INDEX: u8 = 6;
const DASH_INDEX: u8 = 19;
const POSE_TICK_INDEX: u8 = 20;

/// The ordinals of the poses a camel takes (`Pose.STANDING`, `Pose.SITTING`).
const STANDING: i32 = 0;
const SITTING: i32 = 10;

/// How long sitting down and standing up take (`isInPoseTransition`), and the ticks a
/// pose change is put back by to have stood up already (`resetLastPoseChangeTickToFull
/// Stand`).
const SIT_TICKS: i64 = 40;
const STAND_TICKS: i64 = 52;

/// `dashCooldown` after a dash, and below what a landed dash ends.
const DASH_COOLDOWN: u32 = 55;
const DASH_ENDS_BELOW: u32 = 50;

/// The version of the camel's own part of its record.
const TAIL_VERSION: u8 = 1;

/// A calf's size standing and sitting, and a grown camel's sitting (`BABY_STANDING_
/// DIMENSIONS`, `BABY_SITTING_DIMENSIONS`, `ADULT_SITTING_DIMENSIONS`).
pub(crate) const BABY_STANDING: Dimensions = Dimensions {
    width: 0.95,
    height: 1.4,
    eye_height: 1.38,
};
pub(crate) const BABY_SITTING: Dimensions = Dimensions {
    width: 0.95,
    height: 0.425,
    eye_height: 0.41,
};
pub(crate) const ADULT_SITTING: Dimensions = Dimensions {
    width: 1.7,
    height: 2.375 - 1.43,
    eye_height: 0.845,
};

/// The sounds of a breed of camel.
pub(crate) struct Sounds {
    pub(crate) ambient: i32,
    pub(crate) hurt: i32,
    pub(crate) death: i32,
    #[allow(dead_code, reason = "what the riding, not built yet, sounds")]
    pub(crate) dash: i32,
    pub(crate) dash_ready: i32,
    pub(crate) eat: i32,
    pub(crate) stand: i32,
    pub(crate) sit: i32,
    pub(crate) saddle: i32,
    pub(crate) step: i32,
    pub(crate) step_sand: i32,
}

/// What a camel and a camel husk differ in (`CamelHusk`'s overrides).
pub(crate) struct Breed {
    pub(crate) kind: i32,
    /// `isFood(stack)`.
    pub(crate) food: Items,
    /// `canMate`, `canFallInLove`, `canBeABaby` and `getBreedOffspring`: a camel's, of
    /// which the husk has none.
    pub(crate) breeds: bool,
    /// Whether a player's use makes it persistent first (the husk's `interact`).
    pub(crate) kept_by_a_use: bool,
    /// Its steps' volume.
    pub(crate) step_volume: f32,
    pub(crate) sounds: Sounds,
}

pub(crate) static BREED: Breed = Breed {
    kind: entity_types::CAMEL,
    food: Items::Tag("minecraft:camel_food"),
    breeds: true,
    kept_by_a_use: false,
    step_volume: 1.0,
    sounds: Sounds {
        ambient: sounds::ENTITY_CAMEL_AMBIENT,
        hurt: sounds::ENTITY_CAMEL_HURT,
        death: sounds::ENTITY_CAMEL_DEATH,
        dash: sounds::ENTITY_CAMEL_DASH,
        dash_ready: sounds::ENTITY_CAMEL_DASH_READY,
        eat: sounds::ENTITY_CAMEL_EAT,
        stand: sounds::ENTITY_CAMEL_STAND,
        sit: sounds::ENTITY_CAMEL_SIT,
        saddle: sounds::ENTITY_CAMEL_SADDLE,
        step: sounds::ENTITY_CAMEL_STEP,
        step_sand: sounds::ENTITY_CAMEL_STEP_SAND,
    },
};

/// A camel of a breed: the camel, or the camel husk.
pub(crate) struct Camel {
    pub(crate) breed: &'static Breed,
}

/// The camel.
pub(crate) static CAMEL: Camel = Camel { breed: &BREED };

/// What the family's shared code reads of a camel (its grazing, `AbstractHorse.aiStep`).
const EQUINE: Equine = Equine {
    kind: entity_types::CAMEL,
    max_temper: 100,
    rears: false,
    grazes: true,
    chested: false,
    grows: true,
    baby_scale: 1.0,
    food: Items::Tag("minecraft:camel_food"),
    behaviour: Behaviour::None,
    angry: |_| None,
    eat: |_| None,
    chest_sound: sounds::ENTITY_DONKEY_CHEST,
};

/// What a camel keeps after the family's state: `LAST_POSE_CHANGE_TICK` (game time,
/// negative while it sits), `DASH`, and the moment (`lived`) its `dashCooldown` comes
/// to nothing. The cooldown is not saved by the game; here it is kept as the rest is,
/// and a fresh load ([`fresh_load`]) puts it to nothing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct CamelState {
    pub(crate) pose_tick: i64,
    pub(crate) dashing: bool,
    pub(crate) dash_until: u32,
}

/// The camel's own state of `mob`.
pub(crate) fn camel_of(mob: &Mob) -> CamelState {
    let rest = animal::own_bytes(&mob.state);
    let Ok((_, tail)) = postcard::take_from_bytes::<HorseState>(rest) else {
        return CamelState::default();
    };
    match tail.split_first() {
        Some((&TAIL_VERSION, bytes)) => postcard::from_bytes(bytes).unwrap_or_default(),
        _ => CamelState::default(),
    }
}

/// `mob`'s record with `shared`, `horse` and `camel`.
fn write(mob: &mut Mob, shared: &Animal, horse: &HorseState, camel: &CamelState) {
    let tail = if *camel == CamelState::default() {
        Vec::new()
    } else {
        let mut tail = vec![TAIL_VERSION];
        tail.extend(postcard::to_allocvec(camel).expect("a camel's state is encoded into memory"));
        tail
    };
    mob.state = animal::write_with_tail(shared, horse, &tail);
}

/// Changes the camel's own state of `mob`.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut CamelState)) {
    let mut camel = camel_of(mob);
    let before = camel;
    change(&mut camel);
    if camel != before {
        let (shared, horse) = equine::read(mob);
        write(mob, &shared, &horse, &camel);
    }
}

/// Does `what` to the camel `mob` with the family's code, which writes a horse's record,
/// and puts the camel's own part back after it.
fn keeping_own<T>(mob: &mut Mob, what: impl FnOnce(&mut Mob) -> T) -> T {
    let camel = camel_of(mob);
    let answer = what(mob);
    let (shared, horse) = equine::read(mob);
    write(mob, &shared, &horse, &camel);
    answer
}

/// The same for a tick of the camel.
fn keeping_own_in(tick: &mut MobTick<'_>, what: impl FnOnce(&mut MobTick<'_>)) {
    let camel = camel_of(&tick.mob);
    what(tick);
    let (shared, horse) = equine::read(&tick.mob);
    write(&mut tick.mob, &shared, &horse, &camel);
}

/// `isCamelSitting()`: its last pose change's tick is below nothing.
pub(crate) fn is_sitting(mob: &Mob) -> bool {
    camel_of(mob).pose_tick < 0
}

/// `getPoseTime()` at the game time `now`.
pub(crate) fn pose_time(mob: &Mob, now: i64) -> i64 {
    now.wrapping_sub(camel_of(mob).pose_tick.wrapping_abs())
}

/// `isInPoseTransition()`: within the 40 ticks of sitting down or the 52 of standing up.
pub(crate) fn in_pose_transition(mob: &Mob, now: i64) -> bool {
    let total = if is_sitting(mob) {
        SIT_TICKS
    } else {
        STAND_TICKS
    };
    pose_time(mob, now) < total
}

/// `refuseToMove()`: sitting, or between poses.
pub(crate) fn refuse_to_move(mob: &Mob, now: i64) -> bool {
    is_sitting(mob) || in_pose_transition(mob, now)
}

/// `resetLastPoseChangeTickToFullStand(now)`: standing as if it stood up 53 ticks ago.
fn full_stand_tick(now: i64) -> i64 {
    (now - STAND_TICKS - 1).max(0)
}

/// `Mob.makeSound(sound)` in the camel's tick: at its volume, with its voice's two draws.
fn make_sound(tick: &mut MobTick<'_>, sound: i32) {
    let baby = animal::is_baby(&tick.mob);
    let pitch = voice_pitch(tick.draws, baby);
    tick.out.events.push(Event::Sound {
        sound,
        volume: tick.mob_type.sound_volume(),
        pitch,
    });
}

/// The breed of the camel of the type `kind`.
pub(crate) fn breed_of(kind: u16) -> &'static Breed {
    if i32::from(kind) == entity_types::CAMEL_HUSK {
        &super::camel_husk::BREED
    } else {
        &BREED
    }
}

/// `sitDown()` in the camel's tick: its sound, the pose, the game event, the stamp.
pub(crate) fn sit_down(tick: &mut MobTick<'_>) {
    if is_sitting(&tick.mob) {
        return;
    }
    make_sound(tick, breed_of(tick.kind).sounds.sit);
    tick.out
        .events
        .push(Event::GameEvent(GameEvent::EntityAction));
    let now = tick.world.game_time();
    update(&mut tick.mob, |camel| camel.pose_tick = now.wrapping_neg());
}

/// `standUp()` in the camel's tick: its sound, the pose, the game event, the stamp.
pub(crate) fn stand_up(tick: &mut MobTick<'_>) {
    if !is_sitting(&tick.mob) {
        return;
    }
    make_sound(tick, breed_of(tick.kind).sounds.stand);
    tick.out
        .events
        .push(Event::GameEvent(GameEvent::EntityAction));
    let now = tick.world.game_time();
    update(&mut tick.mob, |camel| camel.pose_tick = now);
}

/// `standUpInstantly()` of `mob` at the game time `now`, sitting or not: standing as if
/// long since, without a sound. The game event is the caller's.
pub(crate) fn stand_up_instantly(mob: &mut Mob, now: i64) {
    update(mob, |camel| camel.pose_tick = full_stand_tick(now));
}

/// `canCamelChangePose()`: room for its box in the other pose where it stands
/// (`wouldNotSuffocateAtTargetPose`: blocks alone count).
pub(crate) fn can_change_pose(tick: &MobTick<'_>) -> bool {
    let mut other = tick.mob.clone();
    let sitting = is_sitting(&other);
    update(&mut other, |camel| {
        camel.pose_tick = if sitting { 1 } else { -1 };
    });
    let size = dimensions_of(&other).unwrap_or_else(|| type_size(tick.kind));
    let body = &tick.living.body;
    let bounds = crate::physics::aabb::Aabb::of_entity(body.position, size.width, size.height);
    crate::physics::collide::block_collisions(tick.world, body.context(), &bounds).is_empty()
}

/// The type's own size (a grown camel's standing).
fn type_size(kind: u16) -> Dimensions {
    let info = clustine_data::EntityTypeInfo::of(i32::from(kind));
    Dimensions {
        width: info.width,
        height: info.height,
        eye_height: info.eye_height,
    }
}

/// `getDefaultDimensions(pose)`: a sitting camel's and a calf's own sizes, else the
/// type's.
pub(crate) fn dimensions_of(mob: &Mob) -> Option<Dimensions> {
    let baby = animal::is_baby(mob);
    match (is_sitting(mob), baby) {
        (true, true) => Some(BABY_SITTING),
        (true, false) => Some(ADULT_SITTING),
        (false, true) => Some(BABY_STANDING),
        (false, false) => None,
    }
}

/// `Mob.clampHeadRotationToBody()`: the head turned back within `max` degrees of the
/// body.
pub(crate) fn clamp_head_to_body(mob: &mut Mob, max: f32) {
    let head = &mut mob.body.head;
    let yaw = head.head_yaw;
    let off = crate::mobs::ai::maths::wrap_degrees(head.body_yaw - yaw);
    let clamped = off.clamp(-max, max);
    head.head_yaw = yaw + off - clamped;
}

/// `dashCooldown` at the moment `now`.
pub(crate) fn dash_cooldown(mob: &Mob, now: u32) -> u32 {
    camel_of(mob).dash_until.saturating_sub(now)
}

/// `canParent()` of a camel seen as `entity` and `mob`: as the family's, a camel always
/// tamed.
fn can_parent(entity: &clustine_world::Entity, mob: &Mob, partner: bool) -> bool {
    let now = entity.lived;
    let shared = animal::animal_of(&mob.state);
    let in_love = if partner {
        shared.in_love_for_others(now)
    } else {
        shared.in_love(now)
    };
    let max = mob
        .body
        .attributes
        .value(entity.kind, attribute(ids::MAX_HEALTH)) as f32;
    entity.riding.passengers.is_empty()
        && entity.riding.vehicle.is_none()
        && !shared.is_baby()
        && mob.body.vitals.health >= max
        && in_love
}

/// What a fresh load of a camel resets (`dashCooldown` is not saved; the plan's 2.10):
/// for the brain's `fresh_load` hook when it comes.
#[allow(
    dead_code,
    reason = "the fresh load of N7j1's contracts, not built yet"
)]
pub(crate) fn fresh_load(mob: &mut Mob) {
    update(mob, |camel| camel.dash_until = 0);
}

/// `handleEating(player, stack)` of a camel, its food known: healed by 2 where hurt, in
/// love where grown with no cooldown and able to love, a calf (never locked) ten seconds
/// older with the happy villager's place drawn; any of these sounds its eating (two of
/// its own draws) and is heard as eating. Whether it ate.
fn handle_eating(used: &mut Used<'_>, breed: &Breed) -> bool {
    let now = used.moment;
    let kind = used.kind;
    let max = used
        .mob
        .body
        .attributes
        .value(kind, attribute(ids::MAX_HEALTH)) as f32;
    let hurt = used.mob.body.vitals.health < max;
    if hurt {
        super::tamable::heal(used.mob, kind, 2.0);
    }
    let mut shared = animal::animal_of(&used.mob.state);
    let love = breed.breeds && shared.age(now) == 0 && shared.love(now) <= 0;
    if love {
        shared.set_in_love(now);
        used.did.push(Did::Event(18));
    }
    let grow = shared.can_age_up();
    if grow {
        for _ in 0..3 {
            used.draws.next_double();
        }
        shared.age_up(10, false, now);
    }
    used.mob.state = animal::with_animal(&used.mob.state, &shared);
    if !(hurt || love || grow) {
        return false;
    }
    let pitch = 1.0 + (used.draws.next_float() - used.draws.next_float()) * 0.2;
    used.did.push(Did::Sound {
        sound: breed.sounds.eat,
        source: SOUND_SOURCE_NEUTRAL,
        at: used.position,
        volume: 1.0,
        pitch,
        but_the_player: false,
    });
    used.did.push(Did::GameEvent(GameEvent::Eat.name()));
    true
}

/// A saddle's own use on a camel (`Equippable.equipOnTarget`, its `interactLivingEntity`):
/// one of it on a living grown camel with none, dropped whatever kills it, with the
/// breed's saddle sound (its seed drawn) and the equip game event.
fn equip_saddle(used: &mut Used<'_>, breed: &Breed) -> Interaction {
    let Some(stack) = used.held.filter(|stack| stack.count > 0) else {
        return Interaction::Pass;
    };
    let slot = slots::SADDLE;
    let fits = ItemComponents::of(stack.item)
        .equippable
        .is_some_and(|equippable| equippable.slot == slot)
        && equine::equippable_by(stack.item, slot, used.kind);
    if stack.item != items::SADDLE
        || !fits
        || animal::is_baby(used.mob)
        || used.mob.body.vitals.dying()
        || used.mob.body.equipment.slots[usize::from(slot)].is_some()
    {
        return Interaction::Pass;
    }
    used.mob.body.equipment.slots[usize::from(slot)] = Some(ItemStack {
        item: stack.item,
        count: 1,
    });
    let _seed = used.draws.next_u64();
    used.did.push(Did::Sound {
        sound: breed.sounds.saddle,
        source: SOUND_SOURCE_NEUTRAL,
        at: used.position,
        volume: 1.0,
        pitch: 1.0,
        but_the_player: false,
    });
    used.did.push(Did::GameEvent("minecraft:equip"));
    equine::guarantee_drop(used.mob, slot);
    Interaction::Success
}

/// `Camel.mobInteract`: a sneaking player opens a grown one's menu; a saddle goes on;
/// its food is fed; a calf may be given a golden dandelion. Getting on (`doPlayerRide`)
/// is the riding's, not wired yet: such a click does nothing until it is.
fn mob_interact(used: &mut Used<'_>, breed: &Breed) -> Interaction {
    if breed.kept_by_a_use {
        // `CamelHusk.interact`: any use makes it persistent first.
        used.mob.body.flags.persistent = true;
    }
    let baby = animal::is_baby(used.mob);
    if used.sneaking && !baby {
        used.did.push(Did::OpenInventory);
        return Interaction::Success;
    }
    let saddled = equip_saddle(used, breed);
    if saddled != Interaction::Pass {
        return saddled;
    }
    if breed.food.test(used.held) {
        return if handle_eating(used, breed) {
            Interaction::SuccessServer
        } else {
            Interaction::Pass
        };
    }
    if !baby {
        // The seam of part R: `doPlayerRide(player)`, where it has room for one more.
        return Interaction::Pass;
    }
    if animal::is_item(used.held, items::GOLDEN_DANDELION) {
        return animal::mob_interact(used, breed.food, None);
    }
    Interaction::Pass
}

/// What the brain will call of a camel, until its provider is wired (part A and the
/// lead's N7j1): its own behaviours' bodies and the conditions it reads.
#[allow(dead_code, reason = "what the camel's brain, not wired yet, calls")]
pub(crate) mod brain {
    use super::*;

    /// `RandomSitting`'s least time in a pose, `20 × 20` ticks.
    pub(crate) const MINIMAL_POSE_TICKS: i64 = 400;

    /// `RandomSitting.checkExtraStartConditions`: out of water, at least 400 ticks in its
    /// pose, not on a lead, on the ground, steered by no one, and with room for the
    /// other pose.
    pub(crate) fn random_sitting_can_start(tick: &MobTick<'_>) -> bool {
        let now = tick.world.game_time();
        !tick.living.body.in_water
            && pose_time(&tick.mob, now) >= MINIMAL_POSE_TICKS
            && !tick.leashed
            && tick.living.body.on_ground
            && !matches!(tick.ridden, Ridden::Driven { .. })
            && can_change_pose(tick)
    }

    /// `RandomSitting.start`: a sitting camel stands up; a standing one sits down, unless
    /// it panics.
    pub(crate) fn random_sitting_start(tick: &mut MobTick<'_>, panicking: bool) {
        if is_sitting(&tick.mob) {
            stand_up(tick);
        } else if !panicking {
            sit_down(tick);
        }
    }

    /// `CamelPanic.checkExtraStartConditions` beyond `AnimalPanic`'s: no mob steers it
    /// (`isMobControlled`, which no mob rides yet: N9's husks).
    pub(crate) fn panic_may_start(_: &Mob) -> bool {
        true
    }

    /// `CamelPanic.start` before `AnimalPanic`'s: it stands up at once.
    pub(crate) fn panic_start(tick: &mut MobTick<'_>) {
        let now = tick.world.game_time();
        stand_up_instantly(&mut tick.mob, now);
        tick.out
            .events
            .push(Event::GameEvent(GameEvent::EntityAction));
    }

    /// The predicate of the three `triggerIf`s (**Inferred** `Camel::refuseToMove`):
    /// whether the wrapped stroll may run.
    pub(crate) fn may_stroll(tick: &MobTick<'_>) -> bool {
        !refuse_to_move(&tick.mob, tick.world.game_time())
    }

    /// `FollowTemptation`'s speed and distance: 2.5, and 2.5 for a calf or 3.5.
    pub(crate) const TEMPTED_SPEED: f32 = 2.5;
    pub(crate) fn tempted_distance(mob: &Mob) -> f64 {
        if animal::is_baby(mob) { 2.5 } else { 3.5 }
    }
}

/// What the riding (the lead's part R) reads and calls of a camel, as plain functions,
/// as [`equine::riding`] has them of a horse.
#[allow(dead_code, reason = "what the riding, not built yet, reads of a camel")]
pub(crate) mod riding {
    use super::*;

    /// `AbstractHorse.getControllingPassenger()`'s rule for a player: a player first on
    /// a saddled camel steers it (a camel is always tamed).
    pub(crate) fn steered_by_rider(mob: &Mob) -> bool {
        equine::is_saddled(mob)
    }

    /// `canAddPassenger`: room for one more while it carries two or fewer (the game's
    /// `<= 2`; a click mounts only below two).
    pub(crate) fn can_add_passenger(passengers: usize) -> bool {
        passengers <= 2
    }

    /// Whether a click with `passengers` on it gets the player on (`mobInteract`'s
    /// fourth rule): a grown camel with fewer than two.
    pub(crate) fn click_mounts(mob: &Mob, passengers: usize) -> bool {
        passengers < 2 && !animal::is_baby(mob)
    }

    /// `getRiddenRotation(rider)`: a refusing camel keeps its own; else the rider's yaw
    /// and half their pitch (`AbstractHorse`'s). As `(yaw, pitch)`.
    pub(crate) fn ridden_rotation(
        mob: &Mob,
        now: i64,
        own: (f32, f32),
        rider: (f32, f32),
    ) -> (f32, f32) {
        if refuse_to_move(mob, now) {
            own
        } else {
            (rider.0, rider.1 * 0.5)
        }
    }

    /// `Camel.tickRidden`: the family's (its rotation as [`ridden_rotation`] gives it),
    /// then a sitting camel not between poses stands up where its rider presses forward.
    pub(crate) fn tick_ridden(tick: &mut MobTick<'_>, yaw: f32, pitch: f32, forward: f32) {
        let now = tick.world.game_time();
        let (yaw, pitch) = ridden_rotation(&tick.mob, now, (tick.yaw, tick.pitch), (yaw, pitch));
        // `setRot(y, x)`, each `% 360`, and the body and head turned to it.
        tick.yaw = yaw % 360.0;
        tick.pitch = pitch % 360.0;
        tick.mob.body.head.body_yaw = tick.yaw;
        tick.mob.body.head.head_yaw = tick.yaw;
        if forward > 0.0 && is_sitting(&tick.mob) && !in_pose_transition(&tick.mob, now) {
            stand_up(tick);
        }
    }

    /// `getRiddenInput`: none from a refusing camel's rider; else the family's.
    pub(crate) fn refuses_ridden_input(mob: &Mob, now: i64) -> bool {
        refuse_to_move(mob, now)
    }

    /// `getRiddenSpeed(player)`: its speed, and a tenth more for a sprinting rider while
    /// its dash is ready.
    pub(crate) fn ridden_speed(kind: u16, mob: &Mob, sprinting: bool, lived: u32) -> f32 {
        let bonus = if sprinting && dash_cooldown(mob, lived) == 0 {
            0.1
        } else {
            0.0
        };
        mob.body
            .attributes
            .value(kind, attribute(ids::MOVEMENT_SPEED)) as f32
            + bonus
    }

    /// `canJump()`: a saddled camel that does not refuse to move.
    pub(crate) fn can_jump(mob: &Mob, now: i64) -> bool {
        !refuse_to_move(mob, now) && equine::is_saddled(mob)
    }

    /// `onPlayerJump(power)`'s test before the family's: saddled, its dash ready, on the
    /// ground.
    pub(crate) fn may_dash(mob: &Mob, on_ground: bool, lived: u32) -> bool {
        equine::is_saddled(mob) && dash_cooldown(mob, lived) == 0 && on_ground
    }

    /// `handleStartJump(power)`: the dash begins. Its sound (`makeSound`, of the
    /// camel's voice) and `ENTITY_ACTION` are the caller's.
    pub(crate) fn handle_start_jump(kind: u16, mob: &mut Mob) -> i32 {
        update(mob, |camel| camel.dashing = true);
        breed_of(kind).sounds.dash
    }

    /// `executeRidersJump(scale, input)`: the push of a dash at the moment `lived`, for
    /// a camel looking along `yaw` and `pitch`, of `speed` (`MOVEMENT_SPEED`), on a block
    /// of `block_speed` (`getBlockSpeedFactor`) and of `jump_power` (`getJumpPower`); its
    /// cooldown of 55 begun and its dash shown. The caller adds the push and marks it
    /// for a sync (`needsSync`).
    pub(crate) fn execute_riders_jump(
        mob: &mut Mob,
        lived: u32,
        scale: f32,
        (yaw, pitch): (f32, f32),
        speed: f64,
        block_speed: f32,
        jump_power: f32,
    ) -> Vec3 {
        let pitch = pitch * 0.017_453_292;
        let yaw = -yaw * 0.017_453_292;
        let pitch_cos = clustine_noise::trig::cos(f64::from(pitch));
        let look = [
            f64::from(clustine_noise::trig::sin(f64::from(yaw)) * pitch_cos),
            0.0,
            f64::from(clustine_noise::trig::cos(f64::from(yaw)) * pitch_cos),
        ];
        let flat = hurt::normalize(look);
        let by = f64::from(22.2222_f32 * scale) * speed * f64::from(block_speed);
        let up = f64::from(1.4285_f32 * scale) * f64::from(jump_power);
        update(mob, |camel| {
            camel.dash_until = lived.saturating_add(DASH_COOLDOWN);
            camel.dashing = true;
        });
        [flat[0] * by, up, flat[2] * by]
    }

    /// `getAgeScale()`: a calf's 0.6, which its seats are scaled by.
    pub(crate) fn age_scale(mob: &Mob) -> f32 {
        if animal::is_baby(mob) { 0.6 } else { 1.0 }
    }

    /// `getJumpCooldown()`: `dashCooldown`.
    pub(crate) fn jump_cooldown(mob: &Mob, lived: u32) -> u32 {
        dash_cooldown(mob, lived)
    }

    /// `getBodyAnchorAnimationYOffset(first, partial, dims, scale)` at the game time
    /// `now`, of a box `height` high: the riders' height, which follows the pose's
    /// animation as it sits down and stands up.
    pub(crate) fn body_anchor_y_offset(
        mob: &Mob,
        now: i64,
        first: bool,
        partial: f32,
        height: f32,
        scale: f32,
    ) -> f64 {
        let base = if animal::is_baby(mob) {
            0.093_75
        } else {
            0.375
        };
        let mut y = f64::from(height) - base;
        let sit = scale * 1.43;
        let mid0 = sit - scale * 0.2;
        let low = sit - mid0;
        let transition = in_pose_transition(mob, now);
        let sitting = is_sitting(mob);
        if transition {
            let total = if sitting { SIT_TICKS } else { STAND_TICKS } as i32;
            let (split, k) = if sitting {
                (28, if first { 0.5 } else { 0.1 })
            } else if first {
                (24, 0.6)
            } else {
                (32, 0.35)
            };
            let t = (pose_time(mob, now) as f32 + partial).clamp(0.0, total as f32);
            let early = t < split as f32;
            let p = if early {
                t / split as f32
            } else {
                (t - split as f32) / (total - split) as f32
            };
            let mid = sit - k * mid0;
            let lerp = |p: f32, a: f32, b: f32| a + p * (b - a);
            y += f64::from(if sitting {
                lerp(
                    p,
                    if early { sit } else { mid },
                    if early { mid } else { low },
                )
            } else {
                lerp(
                    p,
                    if early { low - sit } else { low - mid },
                    if early { low - mid } else { 0.0 },
                )
            });
        }
        if sitting && !transition {
            y += f64::from(low);
        }
        y
    }

    /// `getPassengerAttachmentPoint(passenger, dims, scale)` of the passenger at `index`
    /// of `count`, at the game time `now`, the camel facing `yaw`, of a box `height` high:
    /// the first at 0.5 ahead, a second 0.7 behind, an animal 0.2 further ahead, at the
    /// body's anchor; a removed camel's at 0.01.
    #[expect(
        clippy::too_many_arguments,
        reason = "what `getPassengerAttachmentPoint` reads of the camel and the rider"
    )]
    pub(crate) fn seat(
        mob: &Mob,
        now: i64,
        index: usize,
        count: usize,
        animal_rider: bool,
        height: f32,
        scale: f32,
        yaw: f32,
        removed: bool,
    ) -> Vec3 {
        let first = index == 0;
        let mut z = 0.5_f32;
        let y = if removed {
            0.009_999_999_776_482_582_f64 as f32
        } else {
            body_anchor_y_offset(mob, now, first, 0.0, height, scale) as f32
        };
        if count > 1 {
            if !first {
                z = -0.7;
            }
            if animal_rider {
                z += 0.2;
            }
        }
        crate::region::riding::transform_point([0.0, f64::from(y), f64::from(z * scale)], yaw)
    }

    /// `getDismountLocationForPassenger(rider)`: the family's ([`equine::riding::
    /// dismount_directions`]), of the camel's width.
    pub(crate) fn dismount_directions(
        camel_width: f32,
        rider_width: f32,
        yaw: f32,
        right_handed: bool,
    ) -> [Vec3; 2] {
        equine::riding::dismount_directions(camel_width, rider_width, yaw, right_handed)
    }
}

impl MobType for Camel {
    /// The constructor's: `Animal`'s maluses, floating and over fences. `registerGoals`
    /// adds nothing; its brain is not wired yet.
    fn register_goals(&self, setup: &mut Setup) {
        super::cow::animal_maluses(setup);
        setup.config.navigation.flags.can_float = true;
        setup.config.navigation.flags.can_walk_over_fences = true;
    }

    fn head(&self) -> HeadLimits {
        HeadLimits {
            max_y_rot: 30,
            ..HeadLimits::default()
        }
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        animal::walk_target_value(ai, pos, clustine_data::blocks::GRASS_BLOCK)
    }

    /// `CamelLookControl.tick`: only where no player steers it.
    fn tick_look_control(&self, ai: &mut Ai<'_, '_>) {
        if !matches!(ai.tick.ridden, Ridden::Driven { .. }) {
            controls::tick_look(ai);
        }
    }

    /// `CamelMoveControl.tick`: a walk wanted of a sitting camel, not on a lead, not
    /// between poses and with room, stands it up first.
    fn tick_move_control(&self, ai: &mut Ai<'_, '_>) {
        let now = ai.tick.world.game_time();
        let mob = &ai.tick.mob;
        if mob.body.controls.movement.operation == Operation::MoveTo
            && !ai.tick.leashed
            && is_sitting(mob)
            && !in_pose_transition(mob, now)
            && can_change_pose(ai.tick)
        {
            stand_up(ai.tick);
        }
        controls::tick_move(ai);
    }

    /// `CamelBodyRotationControl.clientTick`: none while it refuses to move.
    fn body_rotation_ticks(&self, tick: &MobTick<'_>) -> bool {
        !refuse_to_move(&tick.mob, tick.world.game_time())
    }

    /// `brain.tick` and `CamelAi.updateActivity` (the seam its provider is wired at),
    /// then `Animal`'s.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::custom_server_ai_step(tick);
    }

    fn before_ai_step(&self, tick: &mut MobTick<'_>) {
        equine::before_ai_step(tick);
    }

    /// `AbstractHorse.aiStep` after `Animal`'s (grazing, healing) and `AbstractHorse.
    /// tick`'s counters, the camel's own part kept through them.
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step_growing(tick, self.breed.breeds);
        keeping_own_in(tick, |tick| {
            equine::ai_step(tick, &EQUINE);
            equine::tick_end(tick);
        });
    }

    /// `Camel.tick` after the family's: a landed dash ends once its cooldown is under
    /// 50, the cooldown counts down and sounds its readiness at nothing; a refusing
    /// camel's head is held to its body; a sitting one in water stands up at once.
    fn after_tick(&self, tick: &mut MobTick<'_>) {
        let lived = tick.lived;
        let cooldown = dash_cooldown(&tick.mob, lived);
        let camel = camel_of(&tick.mob);
        let body = &tick.living.body;
        let settled =
            body.on_ground || body.in_water || tick.living.in_lava() || tick.vehicle.is_some();
        if camel.dashing && cooldown < DASH_ENDS_BELOW && settled {
            update(&mut tick.mob, |camel| camel.dashing = false);
        }
        if cooldown == 1 {
            update(&mut tick.mob, |camel| camel.dash_until = 0);
            let block = crate::physics::containing(tick.living.body.position);
            tick.out.local.push(crate::mobs::tick::Local::Sound {
                sound: self.breed.sounds.dash_ready,
                source: SOUND_SOURCE_NEUTRAL,
                at: [
                    f64::from(block.x) + 0.5,
                    f64::from(block.y) + 0.5,
                    f64::from(block.z) + 0.5,
                ],
                volume: 1.0,
                pitch: 1.0,
            });
        }
        let now = tick.world.game_time();
        if refuse_to_move(&tick.mob, now) {
            clamp_head_to_body(&mut tick.mob, 30.0);
        }
        if is_sitting(&tick.mob) && tick.living.body.in_water {
            stand_up_instantly(&mut tick.mob, now);
            tick.out
                .events
                .push(Event::GameEvent(GameEvent::EntityAction));
        }
    }

    /// `Camel.travel`: a refusing camel on the ground keeps only its fall.
    fn before_travel(&self, tick: &mut MobTick<'_>) {
        if refuse_to_move(&tick.mob, tick.world.game_time()) && tick.living.body.on_ground {
            let motion = &mut tick.living.body.motion;
            motion[0] = 0.0;
            motion[2] = 0.0;
            tick.living.input[0] = 0.0;
            tick.living.input[2] = 0.0;
        }
    }

    /// `AbstractHorse.isImmobile()`: dying under a saddled rider, or grazing (a camel
    /// never rears).
    fn immobile(&self, tick: &MobTick<'_>) -> bool {
        let horse = equine::horse_of(&tick.mob);
        (tick.mob.body.vitals.dying() && tick.carries && equine::is_saddled(&tick.mob))
            || horse.eating
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        mob_interact(used, self.breed)
    }

    /// `Camel.finalizeSpawn`: standing as if long since, then the family's (nothing of
    /// its own, its stats fixed).
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        stand_up_instantly(spawn.mob, spawn.game_time);
    }

    /// `Camel.getBreedOffspring`: a new camel, nothing of its parents taken; the husk's
    /// none.
    fn breed_offspring(&self, _: &mut Breeding<'_>) -> Option<Mob> {
        self.breed.breeds.then(Mob::fresh)
    }

    /// `Camel.canMate(other)`: another camel (a husk is one), both able to parent.
    fn can_mate(&self, this: Mate<'_>, partner: Mate<'_>) -> bool {
        let camel = |kind: u16| {
            matches!(
                i32::from(kind),
                entity_types::CAMEL | entity_types::CAMEL_HUSK
            )
        };
        self.breed.breeds
            && this.entity.key != partner.entity.key
            && camel(partner.entity.kind)
            && breed_of(partner.entity.kind).breeds
            && can_parent(this.entity, this.mob, false)
            && can_parent(partner.entity, partner.mob, true)
    }

    /// `Camel.actuallyHurt`: it stands up at once, sitting or not, before the family's.
    fn before_actually_hurt(&self, struck: &mut Struck<'_>, _: &DamageSource) {
        let now = struck.game_time;
        stand_up_instantly(struck.mob, now);
        struck
            .out
            .events
            .push(Event::GameEvent(GameEvent::EntityAction));
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    /// `AbstractHorse.hurtServer`'s one in three, drawn though a camel never rears.
    fn hurt_taken(&self, mob: &mut Mob, draws: &mut Random, moment: u32) {
        let kind = self.breed.kind as u16;
        keeping_own(mob, |mob| equine::hurt_taken(mob, kind, draws, moment));
    }

    fn drop_equipment(&self, mob: &mut Mob) -> Vec<ItemStack> {
        keeping_own(mob, equine::drop_equipment)
    }

    fn landing(&self, mob: &Mob) -> Option<Landing> {
        Some(Landing::Horse {
            baby: animal::is_baby(mob),
        })
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        dimensions_of(mob)
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(self.breed.sounds.ambient)
    }

    /// `playStepSound`: on sand and concrete powder its sandy step, else its own, at the
    /// breed's volume.
    fn step_sound_on(&self, _: &Mob, state: clustine_data::BlockState) -> Option<(i32, f32, f32)> {
        let sounds = &self.breed.sounds;
        let sound = if in_tag(state, "minecraft:camel_sand_step_sound_blocks") {
            sounds.step_sand
        } else {
            sounds.step
        };
        Some((sound, self.breed.step_volume, 1.0))
    }

    /// `AbstractHorse.getAmbientSoundInterval()`.
    fn ambient_interval(&self) -> u32 {
        400
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(self.breed.sounds.hurt)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(self.breed.sounds.death)
    }

    /// `AbstractHorse.getSoundVolume()`.
    fn sound_volume(&self) -> f32 {
        0.8
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    /// The family's flags (a camel's tame bit is never set: `isTamed` is its own), then
    /// its pose, its dash and its pose's tick.
    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let camel = camel_of(mob);
        let mut values = equine::shown(mob);
        let sitting = camel.pose_tick < 0;
        values.push((
            POSE_INDEX,
            DataShown::Pose(if sitting { SITTING } else { STANDING }),
            !sitting,
        ));
        values.push((
            DASH_INDEX,
            DataShown::Boolean(camel.dashing),
            !camel.dashing,
        ));
        values.push((
            POSE_TICK_INDEX,
            DataShown::Long(camel.pose_tick),
            camel.pose_tick == 0,
        ));
        values
    }
}
