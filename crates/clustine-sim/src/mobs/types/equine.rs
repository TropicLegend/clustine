//! What every horse, donkey, mule, llama, trader llama, skeleton horse and zombie horse
//! is beyond an animal (`AbstractHorse` and `AbstractChestedHorse`;
//! `docs/groundwork/n7-brief.md`, sections 5.9 to 5.14 and 5.18): its tame flag, temper
//! and owner; eating grass, rearing and the open mouth, each a flag clients see; its goals
//! of taming by riding and of breeding with another of the family; feeding by the
//! brief's table; its random stats at spawn and a foal's inherited ones; its saddle and
//! body armour, put on by a click or in its menu; a donkey's, mule's or llama's chest and
//! the inventory behind it.
//!
//! The counters of the game that count every tick are stamps against `lived` (the plan's
//! 11.2): eating's count goes on from the moment it began, rearing and the open mouth end
//! at moments of their own; the stand animation's two floats, which the rider's seat
//! reads, change only while the horse rears or settles from it.
//!
//! Riding is the lead's part of N7h, from N5's vehicles. What it needs of a horse is
//! here as plain functions: whether it is saddled and tamed ([`is_saddled`],
//! [`is_tamed`]), and in [`riding`] whether a player on it steers, its jump strength and
//! speed, its passenger seat with the rearing's offset, `getDismountLocationForPassenger`'s
//! two directions and the jump's rearing.
//!
//! Named differences: `isMobControlled` is false, as no mob rides a horse before N9's
//! jockeys, so a zombie horse panics and bucks as the others do; a foal's
//! `followMommy` is not run, as it only makes a path it throws away (the brief's finding
//! 13) and `Bred` is never set by the game; the golden dandelion is looked for in the
//! hand used, not in either hand; a saddle put on by a click is not taken from a
//! creative player (the game's `split`), as N7e's pig's is not; the four-point leash is
//! the client's drawing and N5's leash pulls a horse as it pulls anything; the
//! advancement of taming is not counted; its steps, gallop, breath and landing sound and
//! the share of its fall its riders take are the body's and the riding's, its fall
//! damage the body's by its attributes. Until the riding comes a player cannot get on, so
//! a wild horse is tamed only by feeding its temper up and riding it later.

use clustine_blocks::data::{entity_in_tag, item_in_tag};
use clustine_data::{EntityTypeInfo, ItemComponents, attributes as ids, entity_types, items};
use clustine_world::{Entity, EntityData, EntityId, ItemStack, Seat};
use serde::{Deserialize, Serialize};

use super::animal::{self, Animal, Asked, Courting, Own};
use super::{Did, Dimensions, Interaction, Mate, Used};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    FloatGoal, Items, LookAtPlayerGoal, PanicGoal, RandomLookAroundGoal, RandomStandGoal,
    RandomStrollGoal, TemptGoal,
};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::{Class, Conditions, Looked};
use crate::mobs::ai::{self, Ai, Setup, controls, navigation, random_pos};
use crate::mobs::body::hurt::voice_pitch;
use crate::mobs::body::{Event, attribute};
use crate::mobs::tick::{Local, MobTick};
use crate::physics::aabb::Aabb;
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::physics::{self, Vec3};
use crate::region::Random;
use crate::shown::DataShown;

/// `DATA_ID_FLAGS` and its bits.
pub(crate) const FLAGS_INDEX: u8 = 18;
const FLAG_TAME: u8 = 2;
const FLAG_BRED: u8 = 8;
const FLAG_EATING: u8 = 16;
const FLAG_STANDING: u8 = 32;
const FLAG_MOUTH: u8 = 64;

/// `AbstractChestedHorse.DATA_ID_CHEST`.
pub(crate) const CHEST_INDEX: u8 = 19;

/// How long a horse rears (`setStanding(20)`), keeps its mouth open (`mouthCounter`
/// to 30) and grazes (`eatingCounter` past 50).
const STAND_TICKS: u32 = 20;
const MOUTH_TICKS: u32 = 30;
const EATING_TICKS: i32 = 50;

/// `getInventoryColumns()` of a donkey or mule with a chest.
const CHESTED_COLUMNS: u8 = 5;

/// What a horse of the family keeps beyond an animal: `AbstractHorse`'s, then each
/// kind's own (a horse's coat, a chest and its items, a llama's strength and coat and
/// caravan, a trader llama's despawn delay). One record for the whole family, each kind
/// leaving the others' at their defaults, which cost a byte each.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct HorseState {
    pub(crate) tame: bool,
    /// `Bred`, which nothing of 26.3 sets (the brief's finding 13), kept as loaded.
    pub(crate) bred: bool,
    /// The owner's UUID (`EntityReference`), set by taming.
    pub(crate) owner: Option<u128>,
    pub(crate) temper: i32,
    /// The eating flag, and `eatingCounter`: `eaten` as of the moment `eating_at`,
    /// going on by one a tick while eating.
    pub(crate) eating: bool,
    pub(crate) eaten: i32,
    pub(crate) eating_at: u32,
    /// The standing flag, and the moment its `standCounter` comes to nothing.
    pub(crate) standing: bool,
    pub(crate) stand_until: u32,
    /// The open mouth's flag, and the moment its `mouthCounter` passes 30.
    pub(crate) mouth: bool,
    pub(crate) mouth_until: u32,
    /// `standAnim` and `standAnimO`, which the rider's seat reads.
    pub(crate) stand_anim: f32,
    pub(crate) stand_anim_o: f32,
    /// `allowStandSliding`, which a rider's jump sets (the riding's).
    pub(crate) allow_stand_sliding: bool,
    /// A horse's `DATA_ID_TYPE_VARIANT`: its coat in the low byte, its markings in the
    /// next.
    pub(crate) variant: i32,
    /// A donkey's, mule's or llama's chest, and its inventory's stacks by slot.
    pub(crate) chest: bool,
    pub(crate) items: Vec<(u8, ItemStack)>,
    /// A llama's strength (`DATA_STRENGTH_ID`, its chest's columns) and coat
    /// (`DATA_VARIANT_ID`).
    pub(crate) strength: i32,
    pub(crate) llama_variant: i32,
    /// The llama it follows in a caravan (`caravanHead`); who follows it is derived
    /// from the others' heads (`hasCaravanTail`).
    pub(crate) caravan_head: Option<EntityId>,
    /// `didSpit`, which ends a llama's revenge.
    pub(crate) did_spit: bool,
    /// A trader llama's `despawnDelay` spent (47999 less it), as of the moment it last
    /// began counting, and that moment while it counts.
    pub(crate) despawn_spent: i32,
    pub(crate) despawn_since: Option<u32>,
}

impl Own for HorseState {}

impl HorseState {
    /// `eatingCounter` at the moment `now`, its count of every tick since it began
    /// eating.
    fn eating_counter(&self, now: u32) -> i32 {
        if !self.eating {
            return self.eaten;
        }
        let gone = (i64::from(now) - i64::from(self.eating_at)).max(0);
        self.eaten.saturating_add(gone as i32)
    }

    /// `setEating(on)` at the moment `now`: its count is kept as it stands.
    pub(crate) fn set_eating(&mut self, on: bool, now: u32) {
        if on == self.eating {
            return;
        }
        if on {
            self.eating = true;
            self.eating_at = now;
        } else {
            self.eaten = self.eating_counter(now);
            self.eating = false;
            self.eating_at = 0;
        }
    }

    /// `setStanding(20)` at the moment `now`: eating ends, the flag is on.
    pub(crate) fn set_standing(&mut self, now: u32) {
        self.set_eating(false, now);
        self.standing = true;
        self.stand_until = now.saturating_add(STAND_TICKS);
    }

    /// `clearStanding()`.
    pub(crate) fn clear_standing(&mut self) {
        self.standing = false;
        self.stand_until = 0;
    }

    /// `openMouth()` at the moment `now`.
    fn open_mouth(&mut self, now: u32) {
        self.mouth = true;
        self.mouth_until = now.saturating_add(MOUTH_TICKS);
    }

    /// `DATA_ID_FLAGS` as clients see it.
    pub(crate) fn flags(&self) -> u8 {
        let bit = |on: bool, bit: u8| if on { bit } else { 0 };
        bit(self.tame, FLAG_TAME)
            | bit(self.bred, FLAG_BRED)
            | bit(self.eating, FLAG_EATING)
            | bit(self.standing, FLAG_STANDING)
            | bit(self.mouth, FLAG_MOUTH)
    }

    /// `modifyTemper(d)`: within nothing and the kind's most.
    fn modify_temper(&mut self, by: i32, max: i32) -> i32 {
        self.temper = self.temper.saturating_add(by).clamp(0, max.max(0));
        self.temper
    }

    /// `tameWithName(player)`: the owner, the tame flag.
    pub(crate) fn tame_with_name(&mut self, owner: u128) {
        self.owner = Some(owner);
        self.tame = true;
    }
}

/// Which goals a kind adds after `AbstractHorse.registerGoals`'s (`addBehaviourGoals`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Behaviour {
    /// `AbstractHorse`'s: floating, panicking unless a mob controls it, tempted.
    Horse,
    /// A skeleton horse's: none.
    None,
    /// A zombie horse's: floating, and tempted by its own food.
    Zombie,
}

/// What a kind of the family is, as the shared part reads it.
pub(crate) struct Equine {
    /// The kind's entity type, by its id in `clustine_data::ENTITY_TYPES`.
    pub(crate) kind: i32,
    /// `getMaxTemper()`: 100, a llama's 30.
    pub(crate) max_temper: i32,
    /// `canPerformRearing()`: all but llamas.
    pub(crate) rears: bool,
    /// `canEatGrass()`: all but llamas.
    pub(crate) grazes: bool,
    /// Whether it is an `AbstractChestedHorse`.
    pub(crate) chested: bool,
    /// `canAgeUp()`: false for the undead horses.
    pub(crate) grows: bool,
    /// What its baby's size is of the type's (`BABY_DIMENSIONS`' scale).
    pub(crate) baby_scale: f32,
    /// `isFood(stack)`.
    pub(crate) food: Items,
    pub(crate) behaviour: Behaviour,
    /// `getAngrySound()` and `getEatingSound()`, adult or baby.
    pub(crate) angry: fn(bool) -> Option<i32>,
    pub(crate) eat: fn(bool) -> Option<i32>,
    /// `playChestEquipsSound()`'s sound.
    pub(crate) chest_sound: i32,
}

/// The kind of the family of the entity type `kind`, if it is one.
pub(crate) fn of(kind: u16) -> Option<&'static Equine> {
    Some(match i32::from(kind) {
        entity_types::HORSE => &super::horse::EQUINE,
        entity_types::DONKEY => &super::donkey::EQUINE,
        entity_types::MULE => &super::mule::EQUINE,
        entity_types::LLAMA => &super::llama::EQUINE,
        entity_types::TRADER_LLAMA => &super::trader_llama::EQUINE,
        entity_types::SKELETON_HORSE => &super::skeleton_horse::EQUINE,
        entity_types::ZOMBIE_HORSE => &super::zombie_horse::EQUINE,
        _ => return None,
    })
}

/// Whether the entity type `kind` is of the family (`instanceof AbstractHorse`, the
/// camel's apart, which its own part brings).
pub(crate) fn is_equine(kind: u16) -> bool {
    of(kind).is_some()
}

/// What every animal shares and the horse's own, from its type state.
pub(crate) fn read(mob: &Mob) -> (Animal, HorseState) {
    animal::read::<HorseState>(&mob.state)
}

/// The horse's own state of `mob`.
pub(crate) fn horse_of(mob: &Mob) -> HorseState {
    read(mob).1
}

/// Changes the horse's own state of `mob`.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut HorseState)) {
    let (shared, mut own) = read(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        mob.state = animal::write(&shared, &own);
    }
}

/// `isTamed()`.
pub(crate) fn is_tamed(mob: &Mob) -> bool {
    horse_of(mob).tame
}

/// `isSaddled()`: a saddle (an item that goes in the saddle's slot) worn there.
pub(crate) fn is_saddled(mob: &Mob) -> bool {
    mob.body.equipment.slots[usize::from(slots::SADDLE)].is_some_and(|stack| {
        stack.count > 0
            && ItemComponents::of(stack.item)
                .equippable
                .is_some_and(|equippable| equippable.slot == slots::SADDLE)
    })
}

/// What the riding (the lead's part of N7h, from N5's vehicles) reads of a horse of
/// the family, as plain functions.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "what the riding, not built yet, reads of a horse")
)]
pub(crate) mod riding {
    use super::*;

    /// `getControllingPassenger()`'s rule for a player: a player in the first seat steers a
    /// saddled horse; on an unsaddled one (being tamed) the horse goes its own way.
    pub(crate) fn steered_by_rider(mob: &Mob) -> bool {
        is_saddled(mob)
    }

    /// `canJump()`: a saddled horse, whose rider's jump makes it rear
    /// ([`handle_start_jump`]).
    pub(crate) fn can_jump(mob: &Mob) -> bool {
        is_saddled(mob)
    }

    /// `getAttributeValue(JUMP_STRENGTH)` of a horse of the type `kind`: what the riding's
    /// jump multiplies by its power.
    pub(crate) fn jump_strength(kind: u16, mob: &Mob) -> f64 {
        mob.body
            .attributes
            .value(kind, attribute(ids::JUMP_STRENGTH))
    }

    /// `getRiddenSpeed(player)`: the movement speed attribute.
    pub(crate) fn speed(kind: u16, mob: &Mob) -> f64 {
        mob.body
            .attributes
            .value(kind, attribute(ids::MOVEMENT_SPEED))
    }

    /// What `AbstractHorse.getPassengerAttachmentPoint` adds to the type's passenger point
    /// (`EntityTypeInfo::passenger_points`, a baby's scaled by its size): the rearing's
    /// `(0, 0.15·standAnimO·scale, −0.7·standAnimO·scale)` turned by the horse's `-yRot`.
    /// A llama's seat is the default rule's and has none.
    pub(crate) fn passenger_offset(kind: u16, mob: &Mob, scale: f32, yaw: f32) -> Vec3 {
        if !of(kind).is_some_and(|equine| equine.rears) {
            return [0.0; 3];
        }
        let anim = f64::from(horse_of(mob).stand_anim_o);
        let scale = f64::from(scale);
        crate::region::riding::transform_point([0.0, 0.15 * anim * scale, -0.7 * anim * scale], yaw)
    }

    /// `getDismountLocationForPassenger(rider)`'s two directions, in the order tried: the
    /// side of the rider's main arm (`yRot + 90` for a right-handed rider), then the other
    /// (`getCollisionHorizontalEscapeVector(horse width, rider width, yaw)`); failing both,
    /// the horse's own position. The riding tries each with
    /// `getDismountLocationInDirection` over N5's dismount helpers.
    pub(crate) fn dismount_directions(
        horse_width: f32,
        rider_width: f32,
        yaw: f32,
        right_handed: bool,
    ) -> [Vec3; 2] {
        let first = if right_handed { 90.0 } else { -90.0 };
        let second = -first;
        let escape = |turn: f32| {
            crate::region::riding::escape_vector(
                f64::from(horse_width),
                f64::from(rider_width),
                yaw + turn,
            )
        };
        [escape(first), escape(second)]
    }

    /// `handleStartJump(power)` of a saddled horse its rider jumps: `allowStandSliding`, the
    /// rearing where it rears, at the moment `now`. The jump's sound (`playJumpSound`) is
    /// the caller's; the impulse is the rider's client's.
    pub(crate) fn handle_start_jump(kind: u16, mob: &mut Mob, now: u32) {
        let rears = of(kind).is_some_and(|equine| equine.rears);
        update(mob, |horse| {
            horse.allow_stand_sliding = true;
            if rears {
                horse.set_standing(now);
            }
        });
    }
}

/// `isImmobile()`: a dying horse that carries a saddled rider, or one eating or
/// rearing; a llama's, dying or eating.
pub(crate) fn immobile(tick: &MobTick<'_>, equine: &Equine) -> bool {
    let dying = tick.mob.body.vitals.dying();
    let horse = horse_of(&tick.mob);
    if !equine.rears {
        return dying || horse.eating;
    }
    (dying && tick.carries && is_saddled(&tick.mob)) || horse.eating || horse.standing
}

/// A baby's size: the type's, scaled.
pub(crate) fn dimensions(mob: &Mob, equine: &Equine) -> Option<Dimensions> {
    if !animal::is_baby(mob) {
        return None;
    }
    let info = EntityTypeInfo::of(equine.kind);
    Some(Dimensions {
        width: info.width * equine.baby_scale,
        height: info.height * equine.baby_scale,
        eye_height: info.eye_height * equine.baby_scale,
    })
}

/// `AbstractHorse.registerGoals` and its `addBehaviourGoals`, in the game's order of
/// adding: its own goals first, then the behaviour goals of its kind.
pub(crate) fn register_goals(setup: &mut Setup, equine: &'static Equine) {
    super::cow::animal_maluses(setup);
    setup.add_goal(1, RunAroundLikeCrazyGoal { speed: 1.2, equine });
    setup.add_goal(
        2,
        HorseBreedGoal {
            speed: 1.0,
            partners: is_equine,
        },
    );
    setup.add_goal(4, animal::FollowParentGoal { speed: 1.0 });
    setup.add_goal(6, RandomStrollGoal::avoiding_water(0.7));
    setup.add_goal(7, LookAtPlayerGoal::new(Class::Player, 6.0));
    setup.add_goal(8, RandomLookAroundGoal);
    if equine.rears {
        setup.add_goal(
            9,
            RandomStandGoal {
                // `getAmbientStandInterval()`: its ambient sound's interval.
                interval: 400,
                immobile: stand_goal_immobile,
                stand: stand_goal_stand,
            },
        );
    }
    match equine.behaviour {
        Behaviour::Horse => {
            setup.add_goal(0, FloatGoal);
            // `MountPanicGoal`: a panic unless a mob controls it, which none does yet.
            setup.add_goal(1, PanicGoal { speed: 1.2 });
            setup.add_goal(
                3,
                TemptGoal::new(1.25, Items::Tag("minecraft:horse_tempt_items"), false),
            );
        }
        Behaviour::None => {}
        Behaviour::Zombie => {
            setup.add_goal(0, FloatGoal);
            setup.add_goal(3, TemptGoal::new(1.25, equine.food, false));
        }
    }
}

/// `RandomStandGoal`'s `isImmobile()` test of the horse.
fn stand_goal_immobile(ai: &Ai<'_, '_>) -> bool {
    of(ai.tick.kind).is_some_and(|equine| immobile(ai.tick, equine))
}

/// `RandomStandGoal.start`: `standIfPossible()` and the ambient stand sound
/// (`getAmbientStandSound`, its ambient sound) by `Entity.playSound(sound)`: at full
/// volume and pitch, with no draw.
fn stand_goal_stand(ai: &mut Ai<'_, '_>) {
    let now = ai.tick.lived;
    let kind = ai.tick.kind;
    stand_if_possible(&mut ai.tick.mob, kind, now);
    if let Some(sound) = ai.tick.mob_type.ambient_sound(&ai.tick.mob) {
        ai.tick.out.events.push(Event::Sound {
            sound,
            volume: 1.0,
            pitch: 1.0,
        });
    }
}

/// `Mob.makeSound(sound)` in the mob's tick: at its volume, with its voice's two draws.
fn make_sound(tick: &mut MobTick<'_>, sound: Option<i32>) {
    let Some(sound) = sound else {
        return;
    };
    let baby = tick.mob_type.is_baby(&tick.mob);
    let pitch = voice_pitch(tick.draws, baby);
    tick.out.events.push(Event::Sound {
        sound,
        volume: tick.mob_type.sound_volume(),
        pitch,
    });
}

/// `standIfPossible()` at the moment `now`: a kind that rears rears.
pub(crate) fn stand_if_possible(mob: &mut Mob, kind: u16, now: u32) {
    if of(kind).is_some_and(|equine| equine.rears) {
        update(mob, |horse| horse.set_standing(now));
    }
}

/// `makeMad()` in the mob's tick: a horse not rearing rears, with its angry sound.
fn make_mad_in_tick(tick: &mut MobTick<'_>, equine: &Equine) {
    if horse_of(&tick.mob).standing {
        return;
    }
    let now = tick.lived;
    let kind = tick.kind;
    stand_if_possible(&mut tick.mob, kind, now);
    let baby = animal::is_baby(&tick.mob);
    make_sound(tick, (equine.angry)(baby));
}

/// `makeMad()` of a player's use: the same, its sound a thing the use did.
fn make_mad_used(used: &mut Used<'_>, equine: &Equine) {
    if horse_of(used.mob).standing {
        return;
    }
    stand_if_possible(used.mob, used.kind, used.moment);
    let baby = animal::is_baby(used.mob);
    if let Some(sound) = (equine.angry)(baby) {
        let pitch = voice_pitch(used.draws, baby);
        used.did.push(Did::Sound {
            sound,
            source: SOUND_SOURCE_NEUTRAL,
            at: used.position,
            volume: crate::mobs::types::of(used.kind).sound_volume(),
            pitch,
            but_the_player: false,
        });
    }
}

/// `AbstractHorse.aiStep` before `Animal`'s: the tail's draw, both sides' (`moveTail`,
/// which the server keeps nothing of).
pub(crate) fn before_ai_step(tick: &mut MobTick<'_>) {
    let _tail = tick.draws.next_int(200) == 0;
}

/// `AbstractHorse.aiStep` after `Animal`'s, on the server and alive: one point of health
/// one tick in 900, grazing one tick in 300 on a grass block (which the block keeps),
/// for 50 ticks.
pub(crate) fn ai_step(tick: &mut MobTick<'_>, equine: &Equine) {
    if tick.mob.body.vitals.dying() {
        return;
    }
    if tick.draws.next_int(900) == 0 && tick.mob.body.vitals.died_from.is_none() {
        heal(&mut tick.mob, tick.kind, 1.0);
    }
    let now = tick.lived;
    let mut horse = horse_of(&tick.mob);
    let before = horse.clone();
    if equine.grazes {
        if !horse.eating && !tick.carries && tick.draws.next_int(300) == 0 {
            let below = physics::containing(tick.living.body.position);
            let below = clustine_world::BlockPos::new(below.x, below.y - 1, below.z);
            if animal::is_grass_block(tick.world.state(below)) {
                horse.set_eating(true, now);
            }
        }
        // `++eatingCounter > 50`, counted in this tick.
        if horse.eating && horse.eating_counter(now.saturating_add(1)) > EATING_TICKS {
            horse.eaten = 0;
            horse.eating = false;
            horse.eating_at = 0;
        }
    }
    if horse != before {
        let (shared, _) = read(&tick.mob);
        tick.mob.state = animal::write(&shared, &horse);
    }
}

/// `AbstractHorse.tick` after the living tick: the open mouth closes after 30 ticks, a
/// rearing ends after its 20, and the stand animation eases towards the flag.
pub(crate) fn tick_end(tick: &mut MobTick<'_>) {
    let next = tick.lived.saturating_add(1);
    update(&mut tick.mob, |horse| {
        if horse.mouth && next >= horse.mouth_until {
            horse.mouth = false;
            horse.mouth_until = 0;
        }
        if horse.standing && next >= horse.stand_until {
            horse.clear_standing();
        }
        horse.stand_anim_o = horse.stand_anim;
        let s = horse.stand_anim;
        if horse.standing {
            horse.stand_anim = (s + ((1.0 - s) * 0.4 + 0.05)).min(1.0);
        } else {
            horse.allow_stand_sliding = false;
            horse.stand_anim = (s + ((0.8 * s * s * s - s) * 0.6 - 0.05)).max(0.0);
        }
    });
}

/// `heal(amount)` of a living horse of the type `kind`: up to its most.
fn heal(mob: &mut Mob, kind: u16, amount: f32) {
    let health = mob.body.vitals.health;
    if health > 0.0 {
        let max = max_health(mob, kind);
        mob.body.vitals.health = (health + amount).min(max);
    }
}

/// `getMaxHealth()`.
fn max_health(mob: &Mob, kind: u16) -> f32 {
    mob.body.attributes.value(kind, attribute(ids::MAX_HEALTH)) as f32
}

/// `AbstractHorse.hurtServer` after the hurt took: one in three it rears.
pub(crate) fn hurt_taken(mob: &mut Mob, kind: u16, draws: &mut Random, moment: u32) {
    if draws.next_int(3) == 0 {
        stand_if_possible(mob, kind, moment);
    }
}

/// `dropEquipment`: each stack of its inventory, then a chest for one that had it, the
/// chest gone (`AbstractChestedHorse.dropEquipment`).
pub(crate) fn drop_equipment(mob: &mut Mob) -> Vec<ItemStack> {
    let mut horse = horse_of(mob);
    let mut drops: Vec<ItemStack> = horse
        .items
        .iter()
        .map(|(_, stack)| *stack)
        .filter(|stack| stack.count > 0)
        .collect();
    if horse.chest {
        drops.push(ItemStack {
            item: items::CHEST,
            count: 1,
        });
        horse.chest = false;
        horse.items.clear();
    }
    let (shared, _) = read(mob);
    mob.state = animal::write(&shared, &horse);
    drops
}

/// `AbstractHorse`'s synced flags, after `AgeableMob`'s.
pub(crate) fn shown(mob: &Mob) -> Vec<(u8, DataShown, bool)> {
    let flags = horse_of(mob).flags();
    let mut values = animal::shown(mob);
    values.push((FLAGS_INDEX, DataShown::Byte(flags), flags == 0));
    values
}

/// `AbstractChestedHorse`'s chest flag, after the horse's.
pub(crate) fn shown_chested(mob: &Mob) -> Vec<(u8, DataShown, bool)> {
    let chest = horse_of(mob).chest;
    let mut values = shown(mob);
    values.push((CHEST_INDEX, DataShown::Boolean(chest), !chest));
    values
}

/// `getInventoryColumns()` of a horse of the type `kind`: none for a horse or an undead
/// one, five for a donkey or mule with a chest, a llama's strength with one.
pub(crate) fn inventory_columns(kind: u16, mob: &Mob) -> u8 {
    let horse = horse_of(mob);
    if !horse.chest {
        return 0;
    }
    match i32::from(kind) {
        entity_types::DONKEY | entity_types::MULE => CHESTED_COLUMNS,
        entity_types::LLAMA | entity_types::TRADER_LLAMA => {
            u8::try_from(horse.strength.clamp(0, 5)).unwrap_or(0)
        }
        _ => 0,
    }
}

/// `getInventorySize()`: three rows of its columns.
pub(crate) fn inventory_size(kind: u16, mob: &Mob) -> usize {
    usize::from(inventory_columns(kind, mob)) * 3
}

/// `createInventory()`: a new inventory of the size the horse has now, with the old
/// one's stacks that fit.
fn create_inventory(horse: &mut HorseState, size: usize) {
    horse.items.retain(|(slot, _)| usize::from(*slot) < size);
}

/// `canUseSlot(slot)` of a horse of the type `kind`: a saddle only on a living, grown,
/// tamed one, but for the kinds that may use every slot (a horse, a llama, the undead
/// horses); every other slot `Animal`'s.
pub(crate) fn can_use_slot(kind: u16, mob: &Mob, slot: u8) -> bool {
    let every = matches!(
        i32::from(kind),
        entity_types::HORSE
            | entity_types::LLAMA
            | entity_types::TRADER_LLAMA
            | entity_types::SKELETON_HORSE
            | entity_types::ZOMBIE_HORSE
    );
    if every || slot != slots::SADDLE {
        return true;
    }
    !mob.body.vitals.dying() && !animal::is_baby(mob) && is_tamed(mob)
}

/// `Equippable.canBeEquippedBy(type)` and the slot's test of `isEquippableInSlot(stack,
/// slot)` but `canUseSlot`: the item goes in `slot` and its allowed entities have the
/// type `kind`. The allowed entities are not in Clustine's extraction (the brief's 5.13):
/// a saddle is `#can_equip_saddle`'s, horse armour `#can_wear_horse_armor`'s
/// (**Inferred**: `Item$Properties.horseArmor` is not dumped), a carpet a llama's and a
/// trader llama's, wolf armour a wolf's, a harness a happy ghast's, nautilus armour
/// `#can_wear_nautilus_armor`'s; anything else for a body or a saddle no one's.
pub(crate) fn equippable_by(item: i32, slot: u8, kind: u16) -> bool {
    let Some(equippable) = ItemComponents::of(item).equippable else {
        return false;
    };
    if equippable.slot != slot {
        return false;
    }
    let kind = i32::from(kind);
    if slot != slots::SADDLE && slot != slots::BODY {
        return true;
    }
    let name = clustine_data::ITEMS
        .get(usize::try_from(item).unwrap_or(usize::MAX))
        .map_or("", |known| known.name);
    if item == items::SADDLE {
        entity_in_tag(kind, "minecraft:can_equip_saddle")
    } else if name.ends_with("_horse_armor") {
        entity_in_tag(kind, "minecraft:can_wear_horse_armor")
    } else if item_in_tag(item, "minecraft:wool_carpets") {
        kind == entity_types::LLAMA || kind == entity_types::TRADER_LLAMA
    } else if name == "minecraft:wolf_armor" {
        kind == entity_types::WOLF
    } else if name.ends_with("_harness") {
        kind == entity_types::HAPPY_GHAST
    } else if name.ends_with("_nautilus_armor") {
        entity_in_tag(kind, "minecraft:can_wear_nautilus_armor")
    } else {
        false
    }
}

/// `isEquippableInSlot(stack, slot)` of the horse.
pub(crate) fn is_equippable_in_slot(kind: u16, mob: &Mob, stack: ItemStack, slot: u8) -> bool {
    stack.count > 0 && can_use_slot(kind, mob, slot) && equippable_by(stack.item, slot, kind)
}

/// `LivingEntity.onEquipItem(slot, old, new)` of the horse at `at`, what it shows: where
/// the item changed, the new one's equip sound (a saddle's the horse's own), with the
/// seed the horse's random draws, and `GameEvent.EQUIP` (or `UNEQUIP` where it took
/// something off).
pub(crate) fn on_equip_item(
    slot: u8,
    old: Option<ItemStack>,
    new: Option<ItemStack>,
    at: Vec3,
    draws: &mut Random,
) -> Vec<Did> {
    if old.map(|stack| stack.item) == new.map(|stack| stack.item) {
        return Vec::new();
    }
    let mut did = Vec::new();
    let equippable = new.and_then(|stack| ItemComponents::of(stack.item).equippable);
    if let Some(equippable) = equippable
        && equippable.slot == slot
    {
        let _seed = draws.next_u64();
        let sound = if slot == slots::SADDLE {
            clustine_data::sounds::ENTITY_HORSE_SADDLE
        } else {
            equippable.sound
        };
        did.push(Did::Sound {
            sound,
            source: SOUND_SOURCE_NEUTRAL,
            at,
            volume: 1.0,
            pitch: 1.0,
            but_the_player: false,
        });
    }
    did.push(Did::GameEvent(if equippable.is_some() {
        "minecraft:equip"
    } else {
        "minecraft:unequip"
    }));
    did
}

/// `setGuaranteedDrop(slot)`: the slot's stack drops whatever kills the mob.
pub(crate) fn guarantee_drop(mob: &mut Mob, slot: u8) {
    let mut chances = mob.body.equipment.drop_chances.unwrap_or([0.085; 8]);
    chances[usize::from(slot)] = 2.0;
    mob.body.equipment.drop_chances = Some(chances);
}

/// The horse's menu's container slots as it has them now: its saddle, its body, then
/// its inventory's.
pub(crate) fn menu_container(kind: u16, mob: &Mob) -> Vec<Option<ItemStack>> {
    let equipment = &mob.body.equipment.slots;
    let mut slots = vec![
        equipment[usize::from(slots::SADDLE)],
        equipment[usize::from(slots::BODY)],
    ];
    let size = inventory_size(kind, mob);
    let mut inventory = vec![None; size];
    for (slot, stack) in horse_of(mob).items {
        if let Some(place) = inventory.get_mut(usize::from(slot)) {
            *place = Some(stack);
        }
    }
    slots.extend(inventory);
    slots
}

/// Writes the menu's container slots back into the horse: its saddle and body as
/// `Mob$1.setTheItem` sets them (a stack put there drops whatever kills it, and makes the
/// horse persistent), its inventory's stacks. What each equipment slot was and became,
/// where it changed, for the region to show.
pub(crate) fn write_menu_container(
    mob: &mut Mob,
    container: &[Option<ItemStack>],
) -> Vec<(u8, Option<ItemStack>, Option<ItemStack>)> {
    let mut changed = Vec::new();
    for (index, slot) in [(0, slots::SADDLE), (1, slots::BODY)] {
        let new = container.get(index).copied().flatten();
        let place = &mut mob.body.equipment.slots[usize::from(slot)];
        if *place != new {
            changed.push((slot, *place, new));
            *place = new;
            if new.is_some() {
                guarantee_drop(mob, slot);
                mob.body.flags.persistent = true;
            }
        }
    }
    let items: Vec<(u8, ItemStack)> = container
        .iter()
        .skip(2)
        .enumerate()
        .filter_map(|(slot, stack)| Some((u8::try_from(slot).ok()?, (*stack)?)))
        .collect();
    update(mob, |horse| horse.items = items);
    changed
}

/// The kinds a horse of the family's menu's body slot shows a llama's carpet for.
pub(crate) fn is_llama(kind: u16) -> bool {
    matches!(
        i32::from(kind),
        entity_types::LLAMA | entity_types::TRADER_LLAMA
    )
}

/// `HorseInventoryMenu`'s two equipment slots' facts for the menu's click rules.
pub(crate) fn mount_facts(kind: u16, mob: &Mob) -> crate::menus::MountFacts {
    crate::menus::MountFacts {
        kind,
        saddle: can_use_slot(kind, mob, slots::SADDLE),
        body: can_use_slot(kind, mob, slots::BODY),
    }
}

/// The feeding table of a horse (`AbstractHorse.handleEating`, 850–1064): what the item
/// heals, the seconds it ages a foal, the temper it adds, and whether it puts a tamed
/// adult in love.
fn horse_food(item: i32) -> Option<(f32, i32, i32, bool)> {
    Some(match item {
        items::WHEAT => (2.0, 20, 3, false),
        items::SUGAR => (1.0, 30, 3, false),
        items::HAY_BLOCK => (20.0, 180, 0, false),
        items::APPLE => (3.0, 60, 3, false),
        items::RED_MUSHROOM => (3.0, 0, 3, false),
        items::CARROT => (3.0, 60, 3, false),
        items::GOLDEN_CARROT => (4.0, 60, 5, true),
        items::GOLDEN_APPLE | items::ENCHANTED_GOLDEN_APPLE => (10.0, 240, 10, true),
        _ => return None,
    })
}

/// A llama's (`Llama.handleEating`, 339–492): wheat and the hay block.
fn llama_food(item: i32) -> Option<(f32, i32, i32, bool)> {
    Some(match item {
        items::WHEAT => (2.0, 10, 3, false),
        items::HAY_BLOCK => (10.0, 90, 6, true),
        _ => return None,
    })
}

/// `fedFood(player, stack)`: `handleEating` by the kind's table; what ate is used up
/// (but a creative player's), `SUCCESS_SERVER`; what did not, a pass.
pub(crate) fn fed_food(used: &mut Used<'_>, equine: &Equine) -> Interaction {
    let Some(stack) = used.held.filter(|stack| stack.count > 0) else {
        return Interaction::Pass;
    };
    let llama = is_llama(used.kind);
    let found = if llama {
        llama_food(stack.item)
    } else {
        horse_food(stack.item)
    };
    let (heal_by, age_by, temper_by, loves) = found.unwrap_or((0.0, 0, 0, false));
    let now = used.moment;
    let (mut shared, mut horse) = read(used.mob);
    let mut ate = false;
    if loves && horse.tame && shared.age(now) == 0 && !shared.in_love(now) {
        ate = true;
        shared.set_in_love(now);
        used.did.push(Did::Event(18));
    }
    let kind = used.kind;
    if used.mob.body.vitals.health < max_health(used.mob, kind) && heal_by > 0.0 {
        heal(used.mob, kind, heal_by);
        ate = true;
    }
    if shared.is_baby() && age_by > 0 && !shared.age_locked {
        // The happy villager's place, drawn on the server and not sent.
        for _ in 0..3 {
            used.draws.next_double();
        }
        shared.age_up(age_by, false, now);
        ate = true;
    }
    if temper_by > 0 && (ate || !horse.tame) && horse.temper < equine.max_temper {
        horse.modify_temper(temper_by, equine.max_temper);
        ate = true;
    }
    if ate {
        let baby = shared.is_baby();
        if !llama {
            // `eating()`: the mouth opens.
            horse.open_mouth(now);
        }
        if let Some(sound) = (equine.eat)(baby) {
            let pitch = 1.0 + (used.draws.next_float() - used.draws.next_float()) * 0.2;
            used.did.push(Did::Sound {
                sound,
                source: SOUND_SOURCE_NEUTRAL,
                at: used.position,
                volume: 1.0,
                pitch,
                but_the_player: false,
            });
        }
        if !llama {
            used.did.push(Did::GameEvent("minecraft:eat"));
        }
    }
    used.mob.state = animal::write(&shared, &horse);
    if ate {
        Interaction::SuccessServer
    } else {
        Interaction::Pass
    }
}

/// `Horse.mobInteract` (and a zombie horse's, a chested one's): a rider, a sneaking
/// owner, or a foal and a golden dandelion go to `AbstractHorse`'s; else its food is
/// fed, anything else makes a wild one mad, a chest goes on a chested one without one;
/// else `AbstractHorse`'s.
pub(crate) fn mob_interact(used: &mut Used<'_>, equine: &Equine, dandelion: bool) -> Interaction {
    let baby = animal::is_baby(used.mob);
    let tamed = is_tamed(used.mob);
    let secondary = !baby && tamed && used.sneaking;
    let ridden = carried(used);
    let holds_dandelion = dandelion && animal::is_item(used.held, items::GOLDEN_DANDELION);
    if ridden || secondary || (baby && holds_dandelion) {
        return abstract_mob_interact(used, equine);
    }
    if used.held.is_some_and(|stack| stack.count > 0) {
        if equine.food.test(used.held) {
            return fed_food(used, equine);
        }
        if !tamed {
            make_mad_used(used, equine);
            return Interaction::Success;
        }
        if equine.chested && !horse_of(used.mob).chest && animal::is_item(used.held, items::CHEST) {
            equip_chest(used, equine);
            return Interaction::Success;
        }
    }
    abstract_mob_interact(used, equine)
}

/// Whether the mob a player uses carries anyone (`isVehicle`).
fn carried(used: &Used<'_>) -> bool {
    used.carries
}

/// `AbstractChestedHorse.equipChest`: the flag, the chest's sound (two draws), the chest
/// used up (but a creative player's), and an inventory of its new size.
fn equip_chest(used: &mut Used<'_>, equine: &Equine) {
    let kind = used.kind;
    update(used.mob, |horse| horse.chest = true);
    let pitch = (used.draws.next_float() - used.draws.next_float()) * 0.2 + 1.0;
    used.did.push(Did::Sound {
        sound: equine.chest_sound,
        source: SOUND_SOURCE_NEUTRAL,
        at: used.position,
        volume: 1.0,
        pitch,
        but_the_player: false,
    });
    let size = inventory_size(kind, used.mob);
    update(used.mob, |horse| create_inventory(horse, size));
}

/// `AbstractHorse.mobInteract`: a ridden horse or a foal is an animal's (feeding and
/// the golden dandelion); a sneaking player opens a tamed one's inventory; a held item's
/// own use on it (a saddle); body armour or a carpet onto a bare body; else the player
/// gets on.
pub(crate) fn abstract_mob_interact(used: &mut Used<'_>, equine: &Equine) -> Interaction {
    if carried(used) || animal::is_baby(used.mob) {
        return animal::mob_interact(used, equine.food, None);
    }
    if is_tamed(used.mob) && used.sneaking {
        used.did.push(Did::OpenInventory);
        return Interaction::Success;
    }
    if let Some(stack) = used.held.filter(|stack| stack.count > 0) {
        let result = equip_on_interact(used, stack);
        if result != Interaction::Pass {
            return result;
        }
        let kind = used.kind;
        let body = usize::from(slots::BODY);
        if is_equippable_in_slot(kind, used.mob, stack, slots::BODY)
            && used.mob.body.equipment.slots[body].is_none()
        {
            equip_body_armour(used, stack);
            return Interaction::Success;
        }
    }
    do_player_ride(used);
    Interaction::Success
}

/// `ItemStack.interactLivingEntity` of an `equippable` with `equip_on_interact` (the
/// saddle alone of the items a horse may wear: **Inferred**, horse armour's properties
/// are not dumped): `Equippable.equipOnTarget`, one of the stack put on a living horse
/// that may wear it there and wears nothing there, dropped whatever kills it.
fn equip_on_interact(used: &mut Used<'_>, stack: ItemStack) -> Interaction {
    if stack.item != items::SADDLE {
        return Interaction::Pass;
    }
    let slot = slots::SADDLE;
    let kind = used.kind;
    if !is_equippable_in_slot(kind, used.mob, stack, slot)
        || used.mob.body.equipment.slots[usize::from(slot)].is_some()
        || used.mob.body.vitals.dying()
    {
        return Interaction::Pass;
    }
    let one = ItemStack {
        item: stack.item,
        count: 1,
    };
    used.mob.body.equipment.slots[usize::from(slot)] = Some(one);
    let did = on_equip_item(slot, None, Some(one), used.position, used.draws);
    used.did.extend(did);
    guarantee_drop(used.mob, slot);
    Interaction::Success
}

/// `equipBodyArmor(player, stack)`: one of it on the body (but a creative player's is
/// kept), dropped whatever kills it.
fn equip_body_armour(used: &mut Used<'_>, stack: ItemStack) {
    let slot = slots::BODY;
    let one = ItemStack {
        item: stack.item,
        count: 1,
    };
    let old = used.mob.body.equipment.slots[usize::from(slot)];
    used.mob.body.equipment.slots[usize::from(slot)] = Some(one);
    let did = on_equip_item(slot, old, Some(one), used.position, used.draws);
    used.did.extend(did);
    guarantee_drop(used.mob, slot);
}

/// `doPlayerRide(player)`: eating ends, the rearing too, and the player gets on (the
/// riding's, [`Did::Ride`]).
fn do_player_ride(used: &mut Used<'_>) {
    let now = used.moment;
    update(used.mob, |horse| {
        horse.set_eating(false, now);
        horse.clear_standing();
    });
    used.did.push(Did::Ride);
}

/// `generateMaxHealth(r::nextInt)`: `15 + nextInt(8) + nextInt(9)` in floats, 15 to 30.
pub(crate) fn generate_max_health(r: &mut Random) -> f64 {
    let health = 15.0_f32 + r.next_int(8) as f32 + r.next_int(9) as f32;
    f64::from(health)
}

/// `generateSpeed(r::nextDouble)`: `(0.45 + a·0.3 + b·0.3 + c·0.3) · 0.25`.
pub(crate) fn generate_speed(r: &mut Random) -> f64 {
    let mut speed = 0.449_999_988_079_071_04;
    for _ in 0..3 {
        speed += r.next_double() * 0.3;
    }
    speed * 0.25
}

/// `generateJumpStrength(r::nextDouble)`: `0.4 + a·0.2 + b·0.2 + c·0.2`.
pub(crate) fn generate_jump_strength(r: &mut Random) -> f64 {
    let mut jump = 0.400_000_005_960_464_5;
    for _ in 0..3 {
        jump += r.next_double() * 0.2;
    }
    jump
}

/// `setBaseValue` of the attribute `id` of a horse of the type `kind`.
pub(crate) fn set_base(mob: &mut Mob, kind: u16, id: i32, value: f64) {
    if let Some(instance) = mob.body.attributes.instance(kind, attribute(id)) {
        instance.base = value;
    }
}

/// `getAttributeBaseValue(id)`.
fn base_of(mob: &Mob, kind: u16, id: i32) -> f64 {
    let attribute = attribute(id);
    mob.body.attributes.made(attribute).map_or_else(
        || crate::mobs::body::attributes::supplied(kind, attribute).unwrap_or(0.0),
        |instance| instance.base,
    )
}

/// The health a mob made at its type's most is left with once its maximum changed: the
/// game's first tick clamps it (`refreshDirtyAttributes`, `onAttributeUpdated`).
pub(crate) fn clamp_health(mob: &mut Mob, kind: u16) {
    let max = max_health(mob, kind);
    if mob.body.vitals.health > max {
        mob.body.vitals.health = max;
    }
}

/// `createOffspringAttribute(a, b, min, max, random)` (2082–2168): about the parents'
/// mean, spread by their difference and a margin, by three draws, reflected at the
/// bounds.
pub(crate) fn offspring_attribute(a: f64, b: f64, min: f64, max: f64, r: &mut Random) -> f64 {
    let a = a.clamp(min, max);
    let b = b.clamp(min, max);
    let margin = 0.15 * (max - min);
    let spread = (a - b).abs() + margin * 2.0;
    let mean = (a + b) / 2.0;
    let roll = (r.next_double() + r.next_double() + r.next_double()) / 3.0 - 0.5;
    let value = mean + spread * roll;
    if value > max {
        return max - (value - max);
    }
    if value < min {
        return min + (min - value);
    }
    value
}

/// `setOffspringAttributes(partner, foal)` (2030–2080): the foal's health, jump and
/// speed, each from its parents' bases by [`offspring_attribute`] of the breeding
/// parent's random, in that order.
pub(crate) fn set_offspring_attributes(
    this: (&Mob, u16),
    partner: (&Mob, u16),
    foal: (&mut Mob, u16),
    r: &mut Random,
) {
    let bounds = [
        (ids::MAX_HEALTH, 15.0, 30.0),
        (ids::JUMP_STRENGTH, f64::from(0.4_f32), f64::from(1.0_f32)),
        (
            ids::MOVEMENT_SPEED,
            f64::from(0.1125_f32),
            f64::from(0.3375_f32),
        ),
    ];
    for (id, min, max) in bounds {
        let a = base_of(this.0, this.1, id);
        let b = base_of(partner.0, partner.1, id);
        let value = offspring_attribute(a, b, min, max, r);
        set_base(foal.0, foal.1, id, value);
    }
}

/// A new foal of the type `kind`: a fresh mob with nothing of its own yet.
pub(crate) fn foal() -> Mob {
    Mob::fresh()
}

/// `canParent()` of a horse of the family seen as `entity` and `mob` at its `lived`:
/// it carries no one and rides nothing, is tamed, grown, at its full health and in
/// love (as others see it, for a partner).
pub(crate) fn can_parent(entity: &Entity, mob: &Mob, partner: bool) -> bool {
    let now = entity.lived;
    let (shared, horse) = read(mob);
    let in_love = if partner {
        shared.in_love_for_others(now)
    } else {
        shared.in_love(now)
    };
    entity.riding.passengers.is_empty()
        && entity.riding.vehicle.is_none()
        && horse.tame
        && !shared.is_baby()
        && mob.body.vitals.health >= max_health(mob, entity.kind)
        && in_love
}

/// `canMate(other)` of a horse or a donkey: with a horse or a donkey other than itself,
/// both able to parent.
pub(crate) fn horse_or_donkey_mates(this: Mate<'_>, partner: Mate<'_>) -> bool {
    let kind = |mate: Mate<'_>| i32::from(mate.entity.kind);
    let fits = |kind: i32| kind == entity_types::HORSE || kind == entity_types::DONKEY;
    this.entity.key != partner.entity.key
        && fits(kind(partner))
        && can_parent(this.entity, this.mob, false)
        && can_parent(partner.entity, partner.mob, true)
}

/// `Llama.canMate(other)`: with another llama (a trader llama is one), both able to
/// parent.
pub(crate) fn llama_mates(this: Mate<'_>, partner: Mate<'_>) -> bool {
    this.entity.key != partner.entity.key
        && is_llama(partner.entity.kind)
        && can_parent(this.entity, this.mob, false)
        && can_parent(partner.entity, partner.mob, true)
}

/// `RunAroundLikeCrazyGoal`'s state: where it runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Running {
    pub(crate) wanted: Vec3,
}

/// `RunAroundLikeCrazyGoal(horse, speed)`: a wild horse with a rider runs about, and now
/// and then (one in 25 of its ticks) a player on it tames it, by the chance its temper
/// gives, or angers it: its temper up by five, the rider thrown and the horse rearing
/// with smoke.
pub(crate) struct RunAroundLikeCrazyGoal {
    pub(crate) speed: f64,
    pub(crate) equine: &'static Equine,
}

impl RunAroundLikeCrazyGoal {
    /// `getFirstPassenger()` as its seat says, from the horse's own record.
    fn first_seat(ai: &Ai<'_, '_>) -> Option<Seat> {
        let me = ai.tick.world.entity(ai.tick.key)?;
        me.entity.riding.passengers.first().cloned()
    }
}

impl Goal for RunAroundLikeCrazyGoal {
    type State = Running;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, state: &mut Running, ai: &mut Ai<'_, '_>) -> bool {
        if is_tamed(&ai.tick.mob) || !ai.tick.carries {
            return false;
        }
        match random_pos::default_pos(ai, 5, 4) {
            Some(wanted) => {
                state.wanted = wanted;
                true
            }
            None => false,
        }
    }

    fn can_continue(&self, _: &mut Running, ai: &mut Ai<'_, '_>) -> bool {
        !is_tamed(&ai.tick.mob) && !navigation::is_done(ai) && ai.tick.carries
    }

    fn start(&self, state: &mut Running, ai: &mut Ai<'_, '_>) {
        navigation::move_to(ai, state.wanted, self.speed);
    }

    fn tick(&self, _: &mut Running, ai: &mut Ai<'_, '_>) {
        if is_tamed(&ai.tick.mob) {
            return;
        }
        let bound = self.adjusted_tick_delay(50).max(1) as u32;
        if ai.tick.draws.next_int(bound) != 0 {
            return;
        }
        let Some(seat) = Self::first_seat(ai) else {
            return;
        };
        if seat.player {
            let owner = ai
                .tick
                .world
                .players()
                .iter()
                .find(|player| player.key == seat.key)
                .map(|player| player.uuid);
            let max = self.equine.max_temper;
            let temper = horse_of(&ai.tick.mob).temper;
            if max > 0 && (ai.tick.draws.next_int(max as u32) as i32) < temper {
                if let Some(owner) = owner {
                    update(&mut ai.tick.mob, |horse| horse.tame_with_name(owner));
                    ai.tick.out.events.push(Event::Entity(7));
                }
                return;
            }
            update(&mut ai.tick.mob, |horse| {
                horse.modify_temper(5, max);
            });
        }
        ai.tick.out.local.push(Local::EjectPassengers);
        make_mad_in_tick(ai.tick, self.equine);
        ai.tick.out.events.push(Event::Entity(6));
    }
}

/// A horse of the family a goal reads in the view.
struct Seen {
    looked: Looked,
    entity: Entity,
    mob: Mob,
}

impl Seen {
    fn of(seen: &crate::mobs::view::Seen<'_>) -> Option<Self> {
        if !matches!(seen.entity.data, EntityData::Mob { .. }) || !is_equine(seen.entity.kind) {
            return None;
        }
        let mob = Mob::decode(&seen.entity.data).ok()?;
        Some(Self {
            looked: Looked::of_seen(seen),
            entity: seen.entity.clone(),
            mob,
        })
    }
}

/// The horse `key` of the view, if it is one.
fn horse_seen(ai: &Ai<'_, '_>, key: EntityId) -> Option<Seen> {
    ai.tick.world.entity(key).as_ref().and_then(Seen::of)
}

/// `BreedGoal(animal, speed, AbstractHorse.class)`: as N7e's breeding goal, its partner
/// any of the family it may mate with (`canMate`: a horse with a donkey, a llama with a
/// trader llama), its own record as of its tick's start.
pub(crate) struct HorseBreedGoal {
    pub(crate) speed: f64,
    /// The types its partner class takes (`AbstractHorse`; a llama's own class, which a
    /// trader llama is of, a trader llama's its own).
    pub(crate) partners: fn(u16) -> bool,
}

impl HorseBreedGoal {
    /// Whether it may mate with `other`, its own record as the view has it.
    fn mates(ai: &Ai<'_, '_>, other: &Seen) -> bool {
        let Some(me) = ai.tick.world.entity(ai.tick.key) else {
            return false;
        };
        let mut entity = me.entity.clone();
        // Its love and its health as its tick has them so far.
        entity.lived = ai.tick.lived;
        crate::mobs::types::of(ai.tick.kind).can_mate(
            Mate {
                entity: &entity,
                mob: &ai.tick.mob,
            },
            Mate {
                entity: &other.entity,
                mob: &other.mob,
            },
        )
    }

    /// `getFreePartner()`: of the family within 8 blocks that it may mate with and that
    /// do not panic, the nearest; the earlier of two as near.
    fn free_partner(&self, ai: &mut Ai<'_, '_>) -> Option<EntityId> {
        let bounds = Aabb::of_entity(ai.position(), ai.width(), ai.height()).inflate([8.0; 3]);
        let found: Vec<Seen> = ai
            .tick
            .world
            .entities_in(&bounds)
            .iter()
            .filter(|seen| seen.entity.key != ai.tick.key && (self.partners)(seen.entity.kind))
            .filter_map(Seen::of)
            .collect();
        let conditions = Conditions::non_combat().range(8.0).ignore_line_of_sight();
        let from = ai.position();
        let mut best: Option<(f64, EntityId)> = None;
        for other in found {
            if !conditions.test(ai, &other.looked, &|_| true) {
                continue;
            }
            if !Self::mates(ai, &other) || ai::panicking_record(&other.entity) {
                continue;
            }
            let distance = other.looked.distance_sqr(from);
            if best.is_none_or(|(nearest, _)| distance < nearest) {
                best = Some((distance, other.looked.key));
            }
        }
        best.map(|(_, key)| key)
    }

    /// `breed()`: the request to breed, as N7e's goal makes it.
    fn breed(state: &mut Courting, ai: &mut Ai<'_, '_>, partner: &Seen) {
        let key = partner.looked.key;
        if partner.looked.copied && ai.tick.key < key {
            return;
        }
        let now = ai.tick.lived;
        let mut number = 0;
        animal::update(&mut ai.tick.mob, |shared| {
            shared.requests = shared.requests.wrapping_add(1);
            number = shared.requests;
            shared.asked = Some(Asked {
                partner: key,
                number,
                at: now,
            });
        });
        ai.tick.out.local.push(Local::BreedAsk {
            partner: key,
            number,
        });
        navigation::stop(ai);
        state.partner = None;
    }
}

impl Goal for HorseBreedGoal {
    type State = Courting;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    fn can_use(&self, state: &mut Courting, ai: &mut Ai<'_, '_>) -> bool {
        let now = ai.tick.lived;
        let shared = animal::animal_of(&ai.tick.mob.state);
        if !shared.in_love(now) || shared.waiting(now).is_some() {
            return false;
        }
        state.partner = self.free_partner(ai);
        state.partner.is_some()
    }

    fn can_continue(&self, state: &mut Courting, ai: &mut Ai<'_, '_>) -> bool {
        let Some(partner) = state.partner.and_then(|key| horse_seen(ai, key)) else {
            return false;
        };
        let shared = animal::animal_of(&partner.mob.state);
        partner.looked.alive
            && shared.in_love_for_others(partner.entity.lived)
            && state.love_time < 60
            && !ai::panicking_record(&partner.entity)
    }

    fn stop(&self, state: &mut Courting, _: &mut Ai<'_, '_>) {
        state.partner = None;
        state.love_time = 0;
    }

    fn tick(&self, state: &mut Courting, ai: &mut Ai<'_, '_>) {
        let Some(partner) = state.partner.and_then(|key| horse_seen(ai, key)) else {
            return;
        };
        let at = partner.looked.position;
        let max_pitch = ai.head().max_x_rot as f32;
        controls::set_look_at(ai, [at[0], partner.looked.eye_y, at[2]], 10.0, max_pitch);
        navigation::move_to_entity(ai, partner.looked.block_position(), self.speed);
        state.love_time += 1;
        if state.love_time >= self.adjusted_tick_delay(60)
            && partner.looked.distance_sqr(ai.position()) < 9.0
        {
            Self::breed(state, ai, &partner);
        }
    }
}

#[cfg(test)]
mod tests;
