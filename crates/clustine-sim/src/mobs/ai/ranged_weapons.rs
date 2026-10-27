//! A mob's bow and crossbow (`docs/groundwork/n9-plan.md`, sections 4.6 and 8.4; part
//! S7's): `RangedBowAttackGoal` (n7 RangedBowAttackGoal), the skeletons'; and
//! `RangedCrossbowAttackGoal` with its `CrossbowState` (n7 RangedCrossbowAttackGoal), the
//! pillager's; with what both shoot through: `ProjectileUtil.getMobArrow` and the
//! weapon's hand (n7k ProjectileUtil), the crossbow's charge by the mob's use of it (n9
//! CrossbowItem `onUseTick`, `releaseUsing`, `useOnRelease`) and its shooting
//! (`CrossbowAttackMob.performCrossbowAttack`, `CrossbowItem.performShooting`,
//! `ProjectileWeaponItem.shoot`).
//!
//! A mob shoots through its `performRangedAttack` alone: the bow's `releaseUsing` acts
//! only for a player (finding 12), so a skeleton whose use of its bow ends runs nothing
//! of the item's. The crossbow's rules serve both: a mob's use of one loads it at its
//! end as a player's does.
//!
//! Named differences: a skeleton riding a mob does not turn its vehicle towards its
//! target while it strafes (`getControlledVehicle().lookAt`; no mob is steered by a mob
//! in Clustine); a mob's charged rocket is not made (the region makes a mob's shot of an
//! arrow or a thrown item, `region/projectiles.rs`); a mob's weapon worn out is gone
//! without its break's sound and particles (`onEquippedItemBroken`), which no crossbow
//! of 465 uses reaches in a play; a mob's weapon gives its arrow none of its
//! enchantments' making (Flame's fire, Piercing; `AbstractArrow`'s constructor with a
//! weapon), no mob's weapon being enchanted at easy; and the shot of a projectile turned off its aim by
//! Multishot (an angle other than 0, which no mob's crossbow has at easy) is turned by
//! JOML's `rotateAxis` as its source has it, which the dumps do not show (**Inferred**).

use clustine_data::{items, sounds};
use clustine_world::components::{Component, Kind};
use clustine_world::{Difficulty, ItemStack};
use serde::{Deserialize, Serialize};

use super::Ai;
use super::controls;
use super::goals::look_at_thing;
use super::maths;
use super::navigation;
use super::selector::{Goal, flags};
use super::sensing::{self, Looked};
use crate::api::Hand;
use crate::items::{bow, crossbow, enchantments};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::body::Event;
use crate::mobs::body::using::{self, InUse};
use crate::mobs::tick::{Local, MobTick, Shot};
use crate::physics::thrown::BELOW_THE_EYES;
use crate::physics::{self, Vec3};

/// `RangedAttackMob.rangedAttackUncertainty(level)`: `14 − 4 · difficulty`, 10 at easy.
pub(crate) fn ranged_attack_uncertainty() -> f32 {
    (14 - Difficulty::WORLD.id() * 4) as f32
}

/// The stack in the mob's hand `hand`, none for an empty slot.
pub(crate) fn in_hand(mob: &Mob, hand: Hand) -> Option<&ItemStack> {
    let slot = match hand {
        Hand::Main => slots::MAIN_HAND,
        Hand::Off => slots::OFF_HAND,
    };
    mob.body.equipment.slots[usize::from(slot)]
        .as_ref()
        .filter(|stack| stack.count > 0)
}

/// `setItemInHand(hand, stack)`, none emptying it.
fn set_in_hand(mob: &mut Mob, hand: Hand, stack: Option<ItemStack>) {
    let slot = match hand {
        Hand::Main => slots::MAIN_HAND,
        Hand::Off => slots::OFF_HAND,
    };
    mob.body.equipment.slots[usize::from(slot)] = stack.filter(|stack| stack.count > 0);
}

/// `isHolding(item)`: in either hand.
pub(crate) fn is_holding(mob: &Mob, item: i32) -> bool {
    [Hand::Main, Hand::Off]
        .into_iter()
        .any(|hand| in_hand(mob, hand).is_some_and(|stack| stack.item == item))
}

/// `ProjectileUtil.getWeaponHoldingHand(mob, item)`: the main hand where it holds the
/// item, else the off hand, whatever that holds.
pub(crate) fn weapon_hand(mob: &Mob, item: i32) -> Hand {
    if in_hand(mob, Hand::Main).is_some_and(|stack| stack.item == item) {
        Hand::Main
    } else {
        Hand::Off
    }
}

/// `Monster.getProjectile(weapon)`: for a bow or a crossbow, what it fires from a hand
/// (`getHeldProjectile`: the off hand first, then the main hand), with the hand it is
/// in, else a fresh arrow of no hand; nothing for anything else.
pub(crate) fn projectile_for(mob: &Mob, weapon: &ItemStack) -> Option<(Option<Hand>, ItemStack)> {
    if weapon.item != items::BOW && weapon.item != items::CROSSBOW {
        return None;
    }
    for hand in [Hand::Off, Hand::Main] {
        if let Some(stack) = in_hand(mob, hand)
            && bow::fires_from_hand(weapon.item, stack.item)
        {
            return Some((Some(hand), stack.clone()));
        }
    }
    Some((None, ItemStack::new(items::ARROW, 1)))
}

/// `Mob.lookAt(entity, maxYaw, maxPitch)`: its yaw and pitch turned at once towards the
/// thing's eyes, each by at most so much.
fn look_at(ai: &mut Ai<'_, '_>, target: &Looked, max_yaw: f32, max_pitch: f32) {
    let [x, _, z] = ai.position();
    let dx = target.position[0] - x;
    let dz = target.position[2] - z;
    let dy = target.eye_y - ai.eye_y();
    let horizontal = (dx * dx + dz * dz).sqrt();
    let yaw = maths::yaw_towards(dx, dz);
    let pitch = maths::pitch_towards(dy, horizontal);
    // `Mob.rotlerp`: turned by the wrapped difference, at most `max`.
    let turn =
        |from: f32, to: f32, max: f32| from + maths::wrap_degrees(to - from).clamp(-max, max);
    ai.tick.pitch = turn(ai.tick.pitch, pitch, max_pitch);
    ai.tick.yaw = turn(ai.tick.yaw, yaw, max_yaw);
}

/// `LivingEntity.getY(1/3)` of a thing: its feet and a third of its box's height.
fn third_up(target: &Looked) -> f64 {
    let height = target.bounds.max[1] - target.bounds.min[1];
    target.position[1] + height * 0.333_333_333_333_333_3
}

/// Where an arrow a mob shoots is made (`AbstractArrow(type, owner, …)`): at its feet's x
/// and z, a tenth below its eyes.
fn below_the_eyes(ai: &Ai<'_, '_>, below: f64) -> Vec3 {
    let [x, _, z] = ai.position();
    [x, ai.eye_y() - below, z]
}

/// The arrow `ArrowItem.createArrow` makes of `projectile` (`getMobArrow`'s: the
/// projectile's own arrow item, else the plain arrow's): its entity type and the pickup
/// stack, one of the projectile.
pub(crate) fn mob_arrow(projectile: &ItemStack) -> (u16, ItemStack) {
    let item = if bow::is_arrow(projectile.item) {
        projectile.item
    } else {
        items::ARROW
    };
    let kind = u16::try_from(bow::arrow_kind(item)).expect("entity type ids fit 16 bits");
    let mut stack = projectile.with_count(1);
    stack.item = item;
    (kind, stack)
}

/// `Arrow.addEffect(MobEffectInstance(effect, duration))` on an arrow's pickup stack: the
/// effect, of amplifier 0 and shown with its icon, added to its `potion_contents`
/// (`withEffectAdded`), which colours the arrow and is given by its hit.
pub(crate) fn with_effect_added(stack: &ItemStack, effect: &str, duration: i32) -> ItemStack {
    let mut contents = match stack.components.get(Kind::PotionContents) {
        Some(Component::PotionContents(contents)) => contents.clone(),
        _ => clustine_world::components::PotionContents::default(),
    };
    contents
        .custom_effects
        .push(clustine_world::components::EffectInstance {
            effect: effect.to_owned(),
            amplifier: 0,
            duration,
            ambient: false,
            visible: true,
            show_icon: true,
        });
    let mut stack = stack.clone();
    stack.components.set(Component::PotionContents(contents));
    stack
}

/// What a skeleton's arrow becomes of its type (`getArrow` after `getMobArrow`): the
/// pickup stack of a plain arrow (an `Arrow`, not a spectral one) given its effect.
pub(crate) type ArrowEffect = fn(u16, ItemStack) -> ItemStack;

/// `AbstractSkeleton.performRangedAttack(target, power)` (n7k AbstractSkeleton 339–355):
/// the bow's hand, the projectile it would fire (`getProjectile`), the arrow made of it
/// with its damage from `power` (`getMobArrow`, `setBaseDamageFromMob`, the triangle the
/// arrow draws after its UUID) and what the type gives it (`effect`), shot from a tenth
/// below the eyes at the target's third up, lifted by a fifth of the way across, at 1.6
/// and the difficulty's uncertainty; then `skeleton_shoot` at a pitch of the mob's own
/// draw. The arrow's draws are its own stream's, made by the region.
pub(crate) fn skeleton_shot(ai: &mut Ai<'_, '_>, target: &Looked, power: f32, effect: ArrowEffect) {
    let hand = weapon_hand(&ai.tick.mob, items::BOW);
    let weapon = in_hand(&ai.tick.mob, hand).cloned();
    let projectile = weapon
        .as_ref()
        .and_then(|weapon| projectile_for(&ai.tick.mob, weapon))
        .map_or_else(|| ItemStack::new(items::ARROW, 1), |(_, stack)| stack);
    let (kind, stack) = mob_arrow(&projectile);
    let stack = effect(kind, stack);
    let at = below_the_eyes(ai, BELOW_THE_EYES);
    let [x, _, z] = ai.position();
    let dx = target.position[0] - x;
    let dy = third_up(target) - at[1];
    let dz = target.position[2] - z;
    let across = (dx * dx + dz * dz).sqrt();
    ai.tick.out.local.push(Local::Shoot(Shot {
        kind,
        stack,
        weapon,
        at,
        direction: [dx, dy + across * 0.200_000_002_980_232_24, dz],
        power: 1.6,
        uncertainty: ranged_attack_uncertainty(),
        from_mob: Some(power),
    }));
    let pitch = 1.0 / (ai.tick.draws.next_float() * 0.4 + 0.8);
    ai.tick.out.events.push(Event::Sound {
        sound: sounds::ENTITY_SKELETON_SHOOT,
        volume: 1.0,
        pitch,
    });
}

/// `RangedBowAttackGoal`'s state (`attackTime`, `seeTime`, the strafing's direction and
/// its time), kept between targets as the game's fields are.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct BowAttacking {
    pub(crate) attack_time: i32,
    pub(crate) see_time: i32,
    pub(crate) strafing_clockwise: bool,
    pub(crate) strafing_backwards: bool,
    pub(crate) strafing_time: i32,
}

impl Default for BowAttacking {
    fn default() -> Self {
        Self {
            attack_time: -1,
            see_time: 0,
            strafing_clockwise: false,
            strafing_backwards: false,
            strafing_time: -1,
        }
    }
}

/// `RangedBowAttackGoal(mob, speed, attackIntervalMin, attackRadius)` (n7
/// RangedBowAttackGoal): with a target and a bow, it walks up until within the radius and
/// seen for a second, then stands and strafes, turning now and then (two floats drawn
/// every twentieth tick of it, whatever they turn); it draws the bow while it sees the
/// target, lets it go after 20 ticks of the use with the power of their time
/// (`perform`, the type's `performRangedAttack`), and draws again `attackIntervalMin`
/// ticks after. Its slot is the melee goal's when the skeleton holds no bow
/// (`reassessWeaponGoal`), so its state's version is not the melee goal's.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RangedBowAttackGoal {
    pub(crate) speed: f64,
    pub(crate) attack_interval_min: i32,
    pub(crate) radius: f32,
    pub(crate) perform: fn(&mut Ai<'_, '_>, &Looked, f32),
}

impl RangedBowAttackGoal {
    fn radius_sqr(&self) -> f32 {
        self.radius * self.radius
    }

    fn holding_bow(ai: &Ai<'_, '_>) -> bool {
        is_holding(&ai.tick.mob, items::BOW)
    }
}

impl Goal for RangedBowAttackGoal {
    type State = BowAttacking;
    const VERSION: u8 = 2;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, _: &mut BowAttacking, ai: &mut Ai<'_, '_>) -> bool {
        sensing::target(ai).is_some() && Self::holding_bow(ai)
    }

    fn can_continue(&self, state: &mut BowAttacking, ai: &mut Ai<'_, '_>) -> bool {
        (self.can_use(state, ai) || !navigation::is_done(ai)) && Self::holding_bow(ai)
    }

    fn start(&self, _: &mut BowAttacking, ai: &mut Ai<'_, '_>) {
        ai.tick.mob.body.combat.aggressive = true;
    }

    fn stop(&self, state: &mut BowAttacking, ai: &mut Ai<'_, '_>) {
        ai.tick.mob.body.combat.aggressive = false;
        state.see_time = 0;
        state.attack_time = -1;
        using::stop_using(ai.tick);
    }

    fn tick(&self, state: &mut BowAttacking, ai: &mut Ai<'_, '_>) {
        let Some(target) = sensing::target(ai) else {
            return;
        };
        let distance = physics::distance_sqr(ai.position(), target.position);
        let sees = sensing::has_line_of_sight(ai, &target);
        if sees != (state.see_time > 0) {
            state.see_time = 0;
        }
        if sees {
            state.see_time += 1;
        } else {
            state.see_time -= 1;
        }
        let radius_sqr = self.radius_sqr();
        if distance > f64::from(radius_sqr) || state.see_time < 20 {
            navigation::move_to_entity(ai, target.block_position(), self.speed);
            state.strafing_time = -1;
        } else {
            navigation::stop(ai);
            state.strafing_time += 1;
        }
        if state.strafing_time >= 20 {
            if f64::from(ai.tick.draws.next_float()) < 0.3 {
                state.strafing_clockwise = !state.strafing_clockwise;
            }
            if f64::from(ai.tick.draws.next_float()) < 0.3 {
                state.strafing_backwards = !state.strafing_backwards;
            }
            state.strafing_time = 0;
        }
        if state.strafing_time > -1 {
            if distance > f64::from(radius_sqr * 0.75) {
                state.strafing_backwards = false;
            } else if distance < f64::from(radius_sqr * 0.25) {
                state.strafing_backwards = true;
            }
            let forwards = if state.strafing_backwards { -0.5 } else { 0.5 };
            let right = if state.strafing_clockwise { 0.5 } else { -0.5 };
            controls::strafe(ai, forwards, right);
            look_at(ai, &target, 30.0, 30.0);
        } else {
            look_at_thing(ai, &target, 30.0, 30.0);
        }
        if using::is_using(&ai.tick.mob) {
            if !sees && state.see_time < -60 {
                using::stop_using(ai.tick);
            } else if sees {
                let ticks = using::ticks_using(&ai.tick.mob);
                if ticks >= 20 {
                    using::stop_using(ai.tick);
                    (self.perform)(ai, &target, bow::power_for_time(ticks));
                    state.attack_time = self.attack_interval_min;
                }
            }
        } else {
            state.attack_time -= 1;
            if state.attack_time <= 0 && state.see_time >= -60 {
                let hand = weapon_hand(&ai.tick.mob, items::BOW);
                using::start_using(ai.tick, hand);
            }
        }
    }
}

/// `RangedCrossbowAttackGoal$CrossbowState`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum CrossbowState {
    #[default]
    Uncharged,
    Charging,
    Charged,
    ReadyToAttack,
}

/// `RangedCrossbowAttackGoal`'s state, kept between targets as the game's fields are.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct CrossbowAttacking {
    pub(crate) state: CrossbowState,
    pub(crate) see_time: i32,
    pub(crate) attack_delay: i32,
    pub(crate) update_path_delay: i32,
}

/// `RangedCrossbowAttackGoal(mob, speed, attackRadius)` (n7 RangedCrossbowAttackGoal):
/// with a living target and a crossbow, it walks up (at half its speed once charging)
/// until within the radius and seen for five ticks, a new path every one or two seconds
/// (`PATHFINDING_DELAY_RANGE`, a draw of `nextInt(21)` at each); it charges its crossbow
/// by using it (`setChargingCrossbow`, the use's ticks against the charge's duration,
/// then `releaseUsingItem`, whose extra use tick loads it), waits 20 to 39 ticks
/// (`nextInt(20)`, drawn after the load's sound), and shoots when it sees the target
/// (`perform`, the type's `performRangedAttack`). It has no `start`; its `stop` drops the
/// target.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RangedCrossbowAttackGoal {
    pub(crate) speed: f64,
    pub(crate) radius: f32,
    pub(crate) perform: fn(&mut Ai<'_, '_>, &Looked, f32),
    /// `CrossbowAttackMob.setChargingCrossbow(charging)` of the type.
    pub(crate) set_charging: fn(&mut Mob, bool),
}

impl RangedCrossbowAttackGoal {
    /// `isValidTarget()`: a target, alive.
    fn valid_target(ai: &Ai<'_, '_>) -> bool {
        sensing::target(ai).is_some_and(|target| target.alive)
    }

    fn holding_crossbow(ai: &Ai<'_, '_>) -> bool {
        is_holding(&ai.tick.mob, items::CROSSBOW)
    }
}

impl Goal for RangedCrossbowAttackGoal {
    type State = CrossbowAttacking;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, _: &mut CrossbowAttacking, ai: &mut Ai<'_, '_>) -> bool {
        Self::valid_target(ai) && Self::holding_crossbow(ai)
    }

    fn can_continue(&self, state: &mut CrossbowAttacking, ai: &mut Ai<'_, '_>) -> bool {
        Self::valid_target(ai)
            && (self.can_use(state, ai) || !navigation::is_done(ai))
            && Self::holding_crossbow(ai)
    }

    fn stop(&self, state: &mut CrossbowAttacking, ai: &mut Ai<'_, '_>) {
        ai.tick.mob.body.combat.aggressive = false;
        sensing::set_target(ai, None);
        state.see_time = 0;
        if using::is_using(&ai.tick.mob) {
            using::stop_using(ai.tick);
            (self.set_charging)(&mut ai.tick.mob, false);
            // `getUseItem().set(CHARGED_PROJECTILES, EMPTY)`: the use item is empty once
            // the use has stopped, so this changes nothing.
        }
    }

    fn tick(&self, state: &mut CrossbowAttacking, ai: &mut Ai<'_, '_>) {
        let Some(target) = sensing::target(ai) else {
            return;
        };
        let sees = sensing::has_line_of_sight(ai, &target);
        if sees != (state.see_time > 0) {
            state.see_time = 0;
        }
        if sees {
            state.see_time += 1;
        } else {
            state.see_time -= 1;
        }
        let distance = physics::distance_sqr(ai.position(), target.position);
        let radius_sqr = self.radius * self.radius;
        let walks =
            (distance > f64::from(radius_sqr) || state.see_time < 5) && state.attack_delay == 0;
        if walks {
            state.update_path_delay -= 1;
            if state.update_path_delay <= 0 {
                // `canRun()`: at its speed only while uncharged.
                let speed = if state.state == CrossbowState::Uncharged {
                    self.speed
                } else {
                    self.speed * 0.5
                };
                navigation::move_to_entity(ai, target.block_position(), speed);
                // `PATHFINDING_DELAY_RANGE.sample`: 20 to 40 inclusive.
                state.update_path_delay = 20 + ai.tick.draws.next_int(21) as i32;
            }
        } else {
            state.update_path_delay = 0;
            navigation::stop(ai);
        }
        look_at_thing(ai, &target, 30.0, 30.0);
        match state.state {
            CrossbowState::Uncharged => {
                if !walks {
                    let hand = weapon_hand(&ai.tick.mob, items::CROSSBOW);
                    using::start_using(ai.tick, hand);
                    state.state = CrossbowState::Charging;
                    (self.set_charging)(&mut ai.tick.mob, true);
                }
            }
            CrossbowState::Charging => {
                if !using::is_using(&ai.tick.mob) {
                    state.state = CrossbowState::Uncharged;
                }
                let ticks = using::ticks_using(&ai.tick.mob);
                // `getUseItem()`: the stack in use, the empty one where none is.
                let use_item = ai
                    .tick
                    .mob
                    .body
                    .using
                    .using
                    .and_then(|InUse { hand, .. }| in_hand(&ai.tick.mob, hand).cloned())
                    .unwrap_or(ItemStack::new(items::AIR, 0));
                if ticks >= crossbow::charge_duration(&use_item) {
                    using::release_using(ai.tick);
                    state.state = CrossbowState::Charged;
                    state.attack_delay = 20 + ai.tick.draws.next_int(20) as i32;
                    (self.set_charging)(&mut ai.tick.mob, false);
                }
            }
            CrossbowState::Charged => {
                state.attack_delay -= 1;
                if state.attack_delay == 0 {
                    state.state = CrossbowState::ReadyToAttack;
                }
            }
            CrossbowState::ReadyToAttack => {
                if sees {
                    (self.perform)(ai, &target, 1.0);
                    state.state = CrossbowState::Uncharged;
                }
            }
        }
    }
}

/// `CrossbowItem.onUseTick` of a mob's use of a crossbow (n9 CrossbowItem 489–569), at
/// `ticks` of the use: the charge's first sound at 0.2 of it and its second at 0.5, both
/// of the players' source at half volume; at the whole charge, where it is not charged
/// already, the projectile `getProjectile` finds drawn into it (`tryLoadProjectiles`,
/// one taken from the hand it came from) and the end's sound at a pitch of the level's
/// draw, of the mob's own source.
pub(crate) fn crossbow_use_tick(tick: &mut MobTick<'_>, using: InUse) {
    let Some(stack) = in_hand(&tick.mob, using.hand).cloned() else {
        return;
    };
    if stack.item != items::CROSSBOW {
        return;
    }
    let ticks = using::use_duration_of(stack.item) - using.remaining;
    let charging = crossbow::charging_sounds(&stack);
    let at = tick.living.body.position;
    for (threshold, sound) in [
        (crossbow::START_SOUND_AT, charging.start),
        (crossbow::MIDDLE_SOUND_AT, charging.middle),
    ] {
        if crossbow::reaches(ticks, &stack, threshold)
            && let Some(sound) = sound
        {
            tick.out.local.push(Local::Sound {
                sound,
                source: crate::api::SOUND_SOURCE_PLAYERS,
                at,
                volume: 0.5,
                pitch: 1.0,
            });
        }
    }
    if crossbow::progress(ticks, &stack) < crossbow::LOADED_AT || crossbow::is_charged(&stack) {
        return;
    }
    let Some((from, ammo)) = projectile_for(&tick.mob, &stack) else {
        return;
    };
    // A mob has no infinite materials.
    let drawn = bow::draw(&stack, &ammo, false);
    if drawn.projectiles.is_empty() {
        return;
    }
    if let Some(from) = from {
        set_in_hand(
            &mut tick.mob,
            from,
            Some(ammo.with_count(ammo.count - drawn.taken)),
        );
    }
    let loaded = crossbow::with_charge(&stack, drawn.projectiles);
    set_in_hand(&mut tick.mob, using.hand, Some(loaded));
    let pitch = crossbow::loaded_pitch(tick.draws.next_float());
    if let Some(sound) = charging.end {
        tick.out.events.push(Event::Sound {
            sound,
            volume: 1.0,
            pitch,
        });
    }
}

/// `CrossbowItem.getProjectileShotVector(shooter, towards, angle)` (n9 CrossbowItem
/// 277–341): `towards` as floats, normalised (JOML's `normalize`, by the root of its
/// squares fused); turned by `angle` degrees about the vector that is it turned a right
/// angle about its cross with up. At angle 0 it is the normalised vector itself.
fn shot_vector(towards: Vec3, angle: f32) -> [f32; 3] {
    let [x, y, z] = towards.map(|v| v as f32);
    let length_sqr = x.mul_add(x, y.mul_add(y, z * z));
    let scale = 1.0 / length_sqr.sqrt();
    let v = [x * scale, y * scale, z * scale];
    if angle == 0.0 {
        return v;
    }
    // `cross(0, 1, 0)`, or with the shooter's up where the shot is straight up or down,
    // which a mob's aim at a target beside it never is.
    let cross = [-v[2], 0.0, v[0]];
    let side = rotate_axis(v, std::f32::consts::FRAC_PI_2, cross);
    rotate_axis(v, angle * maths::RADIANS, side)
}

/// JOML's `Vector3f.rotateAxis(angle, x, y, z)` by its quaternion (`rotateAxisInternal`;
/// **Inferred**, the dumps lacking JOML).
fn rotate_axis(v: [f32; 3], angle: f32, axis: [f32; 3]) -> [f32; 3] {
    let half = angle * 0.5;
    let sin = (f64::from(half)).sin() as f32;
    let [qx, qy, qz] = axis.map(|a| a * sin);
    let qw = (f64::from(half)).cos() as f32;
    let (w2, x2, y2, z2) = (qw * qw, qx * qx, qy * qy, qz * qz);
    let (zw, xy, xz, yw, yz, xw) = (qz * qw, qx * qy, qx * qz, qy * qw, qy * qz, qx * qw);
    let [x, y, z] = v;
    [
        (w2 + x2 - z2 - y2) * x + (-zw + xy - zw + xy) * y + (yw + xz + xz + yw) * z,
        (xy + zw + zw + xy) * x + (y2 - z2 + w2 - x2) * y + (yz + yz - xw - xw) * z,
        (xz - yw + xz - yw) * x + (yz + yz + xw + xw) * y + (z2 - y2 - x2 + w2) * z,
    ]
}

/// `CrossbowAttackMob.performCrossbowAttack(mob, velocity)` (n9 CrossbowAttackMob):
/// where the crossbow's hand holds a crossbow, `performShooting` at `velocity` and the
/// difficulty's uncertainty at the target (n9 CrossbowItem 395–450, ProjectileWeaponItem
/// 49–200): the charge emptied, then each projectile at its angle (Multishot's spread)
/// made with the crossbow's hit sound and no crit, aimed at the target's third up from
/// where it is made and lifted by a fifth of the way across, `crossbow_shoot` at the
/// mob for each (a pitch drawn for every one after the first), and the crossbow worn by
/// each. The mob's `onCrossbowAttackPerformed` is the caller's.
pub(crate) fn perform_crossbow_attack(ai: &mut Ai<'_, '_>, target: &Looked, velocity: f32) {
    let hand = weapon_hand(&ai.tick.mob, items::CROSSBOW);
    let Some(stack) = in_hand(&ai.tick.mob, hand).cloned() else {
        return;
    };
    if stack.item != items::CROSSBOW {
        return;
    }
    let charged = crossbow::charged(&stack);
    let mut weapon = crossbow::with_charge(&stack, Vec::new());
    set_in_hand(&mut ai.tick.mob, hand, Some(weapon.clone()));
    if charged.is_empty() {
        return;
    }
    let uncertainty = ranged_attack_uncertainty();
    let spread = enchantments::projectile_spread(&weapon, 0.0);
    let angles = bow::angles(charged.len(), spread);
    for (index, (projectile, angle)) in charged.into_iter().zip(angles).enumerate() {
        if projectile.count <= 0 {
            continue;
        }
        let rocket = projectile.item == items::FIREWORK_ROCKET;
        let below = if rocket {
            crossbow::ROCKET_BELOW_THE_EYES
        } else {
            BELOW_THE_EYES
        };
        let at = below_the_eyes(ai, below);
        if !rocket {
            let [x, _, z] = ai.position();
            let dx = target.position[0] - x;
            let dz = target.position[2] - z;
            let across = (dx * dx + dz * dz).sqrt();
            let dy = third_up(target) - at[1] + across * 0.200_000_002_980_232_24;
            let direction = shot_vector([dx, dy, dz], angle).map(f64::from);
            let (kind, stack) = mob_arrow(&projectile);
            ai.tick.out.local.push(Local::Shoot(Shot {
                kind,
                stack,
                weapon: Some(weapon.clone()),
                at,
                direction,
                power: velocity,
                uncertainty,
                from_mob: None,
            }));
        }
        let pitch = crossbow::shot_pitch(index, || ai.tick.draws.next_float());
        ai.tick.out.events.push(Event::Sound {
            sound: sounds::ITEM_CROSSBOW_SHOOT,
            volume: 1.0,
            pitch,
        });
        // `weapon.hurtAndBreak(getDurabilityUse(projectile), shooter, hand)`.
        let change =
            enchantments::durability_change(&weapon, crossbow::durability_use(&projectile), || {
                ai.tick.draws.next_float()
            });
        let worn = enchantments::hurt_and_break(&weapon, change);
        set_in_hand(&mut ai.tick.mob, hand, worn.clone());
        match worn {
            Some(worn) => weapon = worn,
            None => return,
        }
    }
}
