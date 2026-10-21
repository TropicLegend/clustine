//! The dolphin (`Dolphin` with its goals, `BreathAirGoal`, `DolphinJumpGoal` and
//! `FollowPlayerRiddenEntityGoal`; `docs/groundwork/n7-brief.md`, section 6.9): an
//! ageable water creature that breathes air (4800 ticks of it, all back at once at the
//! surface), dries out on land in 2400 ticks and flops there, swims with the smooth
//! swimming controls, leaps out of the water, plays with the items it finds in water,
//! swims beside boats, and goes for treasure when fed a fish.
//!
//! Named differences: no structure is ever found for its treasure (Clustine has no
//! structure search: the brief's W9), so a dolphin fed a fish gives the fish up at once,
//! as the game's does where none is within 50 chunks; it never swims with a swimming
//! player and gives no Dolphin's Grace (the edge does not tell a region who swims, and
//! there are no mob effects until N9); rain does not keep it moist (the weather is
//! always clear); it swims beside a boat only where the region holds the boat's rider;
//! it picks up only items of its own region (ADR-0028, section 7); the treasure's
//! particles and the swimming ones are a client's.

use clustine_blocks::data::{fluid_in_tag, in_tag, item_in_tag};
use clustine_data::{EntityTypeInfo, entity_types, sounds};
use clustine_world::{EntityData, EntityId, ItemStack};
use serde::{Deserialize, Serialize};

use super::animal::{self, Own};
use super::{Breeding, Did, Dimensions, Interaction, MobType, Used, water};
use crate::living::{Damage, Sinks, Travel, slots};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    AvoidEntityGoal, BreathAirGoal, HurtByTargetGoal, LookAtPlayerGoal, MeleeAttackGoal,
    RandomLookAroundGoal, RandomStrollGoal, TryFindLiquidGoal,
};
use crate::mobs::ai::maths::{self, reduced_tick_delay, table_cos, table_sin, wrap_degrees};
use crate::mobs::ai::navigation::{self, Kind, Settings};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::Class;
use crate::mobs::ai::{Ai, HeadLimits, LookKind, MoveKind, Setup};
use crate::mobs::body::Event;
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::{Local, MobTick};
use crate::physics::aabb::Aabb;
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::physics::{self, Vec3};
use crate::region::Random;
use crate::shown::DataShown;

/// `getMaxAirSupply()` and `TOTAL_MOISTNESS_LEVEL`.
pub(crate) const MAX_AIR: i32 = 4800;
pub(crate) const MOISTNESS: i32 = 2400;

/// The synced indices of `GOT_FISH` and `MOISTNESS_LEVEL`.
const GOT_FISH_INDEX: u8 = 18;
const MOISTNESS_INDEX: u8 = 19;

/// The guardians, whom it flees and whose hurts it does not answer (`Guardian.class`,
/// the elder one its subclass).
const GUARDIANS: &[u16] = &[
    entity_types::GUARDIAN as u16,
    entity_types::ELDER_GUARDIAN as u16,
];

/// The nautili, the other class of mount it swims beside (`AbstractNautilus`).
const NAUTILI: &[u16] = &[
    entity_types::NAUTILUS as u16,
    entity_types::ZOMBIE_NAUTILUS as u16,
];

/// What a dolphin keeps beyond an ageable mob: whether it was fed a fish (`GOT_FISH`)
/// and its moistness, down a tick on land.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct DolphinState {
    pub(crate) got_fish: bool,
    pub(crate) moistness: i32,
}

impl Default for DolphinState {
    /// `defineSynchedData`'s: no fish, and wet through.
    fn default() -> Self {
        Self {
            got_fish: false,
            moistness: MOISTNESS,
        }
    }
}

impl Own for DolphinState {}

/// A dolphin's state.
pub(crate) fn read(mob: &Mob) -> DolphinState {
    animal::read::<DolphinState>(&mob.state).1
}

/// Changes a dolphin's own state.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut DolphinState)) {
    let (shared, mut own) = animal::read::<DolphinState>(&mob.state);
    let before = own.clone();
    change(&mut own);
    if own != before {
        mob.state = animal::write(&shared, &own);
    }
}

/// `Direction.fromYRot(yaw)`'s steps along x and z: south, west, north, east.
pub(crate) fn direction_steps(yaw: f32) -> (i32, i32) {
    match physics::floor(f64::from(yaw) / 90.0 + 0.5) & 3 {
        0 => (0, 1),
        1 => (-1, 0),
        2 => (0, -1),
        _ => (1, 0),
    }
}

/// The dolphin.
pub(crate) struct Dolphin;

/// `BreathAirGoal` as the dolphin has it: towards air when its air runs low, with its
/// second move of the tick in every goal tick (`moveRelative(0.02f)` and a move).
#[derive(Debug, Clone, Copy)]
pub(crate) struct DolphinBreathGoal;

impl Goal for DolphinBreathGoal {
    type State = ();

    fn flags(&self) -> u8 {
        BreathAirGoal.flags()
    }

    fn interruptable(&self, state: &()) -> bool {
        BreathAirGoal.interruptable(state)
    }

    fn can_use(&self, state: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        BreathAirGoal.can_use(state, ai)
    }

    fn start(&self, state: &mut (), ai: &mut Ai<'_, '_>) {
        BreathAirGoal.start(state, ai);
    }

    fn tick(&self, state: &mut (), ai: &mut Ai<'_, '_>) {
        BreathAirGoal.tick(state, ai);
        water::move_again(ai, 0.02);
    }
}

/// `DolphinSwimToTreasureGoal`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Treasure {
    pub(crate) stuck: bool,
}

/// `DolphinSwimToTreasureGoal`: fed a fish, with air enough, it looks for the nearest
/// ocean ruin or shipwreck; found none, it is stuck and gives up the fish.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SwimToTreasureGoal;

impl Goal for SwimToTreasureGoal {
    type State = Treasure;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    fn interruptable(&self, _: &Treasure) -> bool {
        false
    }

    fn can_use(&self, _: &mut Treasure, ai: &mut Ai<'_, '_>) -> bool {
        read(&ai.tick.mob).got_fish && ai.tick.living.air >= 100
    }

    /// `treasurePos` is never set (no structure search: named), so the goal goes on
    /// only while it would swim to one.
    fn can_continue(&self, _: &mut Treasure, _: &mut Ai<'_, '_>) -> bool {
        false
    }

    /// `findNearestMapStructure` finds nothing: stuck, and no event 38.
    fn start(&self, state: &mut Treasure, ai: &mut Ai<'_, '_>) {
        state.stuck = false;
        navigation::stop(ai);
        state.stuck = true;
    }

    /// No treasure, or reached, or stuck: the fish is given up.
    fn stop(&self, _: &mut Treasure, ai: &mut Ai<'_, '_>) {
        update(&mut ai.tick.mob, |dolphin| dolphin.got_fish = false);
    }
}

/// `DolphinSwimWithPlayerGoal(dolphin, 4.0)`: beside the nearest swimming player within
/// ten blocks. **Named**: no region knows whether a player swims, so it never starts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SwimWithPlayerGoal;

impl Goal for SwimWithPlayerGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    fn can_use(&self, _: &mut (), _: &mut Ai<'_, '_>) -> bool {
        false
    }
}

/// `DolphinJumpGoal`'s state: `breached`, which the game never clears, so that the
/// jump's sound plays once in a dolphin's life.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Jumping {
    pub(crate) breached: bool,
}

/// `DolphinJumpGoal(dolphin, 10)`: one time in five, facing clear water with air above it
/// along seven blocks, it leaps forward out of the water, its pitch following its
/// flight.
#[derive(Debug, Clone, Copy)]
pub(crate) struct JumpGoal;

/// `STEPS_TO_CHECK`.
const STEPS: [i32; 6] = [0, 1, 4, 5, 6, 7];

impl JumpGoal {
    /// `waterIsClear(pos, dx, dz, step)` and `surfaceIsClear`: water not blocking the
    /// leap, and air at one and two above it.
    fn clear(ai: &Ai<'_, '_>, pos: clustine_world::BlockPos, dx: i32, dz: i32, step: i32) -> bool {
        let world = ai.tick.world;
        let at = pos.offset(dx * step, 0, dz * step);
        let state = world.state(at);
        let water = fluid_in_tag(state, "minecraft:water")
            && !in_tag(state, "minecraft:blocks_dolphin_jump");
        water
            && world.state(at.offset(0, 1, 0)).props().is_air()
            && world.state(at.offset(0, 2, 0)).props().is_air()
    }
}

impl Goal for JumpGoal {
    type State = Jumping;

    fn flags(&self) -> u8 {
        crate::mobs::ai::goals::JUMP_GOAL_FLAGS
    }

    fn interruptable(&self, _: &Jumping) -> bool {
        false
    }

    fn can_use(&self, _: &mut Jumping, ai: &mut Ai<'_, '_>) -> bool {
        if ai.tick.draws.next_int(reduced_tick_delay(10) as u32) != 0 {
            return false;
        }
        let (dx, dz) = direction_steps(ai.tick.yaw);
        let pos = ai.block_position();
        STEPS.iter().all(|&step| Self::clear(ai, pos, dx, dz, step))
    }

    fn can_continue(&self, _: &mut Jumping, ai: &mut Ai<'_, '_>) -> bool {
        let motion = ai.tick.living.body.motion;
        let pitch = ai.tick.pitch;
        let level = motion[1] * motion[1] < 0.029_999_999_329_447_746
            && pitch != 0.0
            && pitch.abs() < 10.0
            && ai.in_water();
        !level && !ai.on_ground()
    }

    fn start(&self, _: &mut Jumping, ai: &mut Ai<'_, '_>) {
        let (dx, dz) = direction_steps(ai.tick.yaw);
        let motion = &mut ai.tick.living.body.motion;
        motion[0] += f64::from(dx) * 0.6;
        motion[1] += 0.7;
        motion[2] += f64::from(dz) * 0.6;
        navigation::stop(ai);
    }

    fn stop(&self, _: &mut Jumping, ai: &mut Ai<'_, '_>) {
        ai.tick.pitch = 0.0;
    }

    fn tick(&self, state: &mut Jumping, ai: &mut Ai<'_, '_>) {
        let was = state.breached;
        if !was {
            let at = ai.tick.world.state(ai.block_position());
            state.breached = fluid_in_tag(at, "minecraft:water");
        }
        if state.breached && !was {
            ai.tick.out.events.push(Event::Sound {
                sound: sounds::ENTITY_DOLPHIN_JUMP,
                volume: 1.0,
                pitch: 1.0,
            });
        }
        let motion = ai.tick.living.body.motion;
        let pitch = ai.tick.pitch;
        if motion[1] * motion[1] < 0.029_999_999_329_447_746 && pitch != 0.0 {
            // `Mth.rotLerp(0.2f, xRot, 0f)`.
            ai.tick.pitch = pitch + 0.2 * wrap_degrees(0.0 - pitch);
        } else if physics::length_sqr(motion).sqrt() > 9.999_999_747_378_752e-6 {
            let horizontal = (motion[0] * motion[0] + motion[2] * motion[2]).sqrt();
            // `Math.atan2`, HotSpot's (the brief's W3), through `libm`.
            ai.tick.pitch = (libm::atan2(-motion[1], horizontal) * maths::DEGREES) as f32;
        }
    }
}

/// Where the items within `grow` of the dolphin's box that it may play with lie
/// (`ALLOWED_ITEMS`: no pick-up delay, alive, in water; of the region's own), in the
/// view's order.
fn playthings(ai: &Ai<'_, '_>, grow: Vec3) -> Vec<Vec3> {
    let bounds = Aabb::of_entity(ai.position(), ai.width(), ai.height()).inflate(grow);
    let world = ai.tick.world;
    world
        .entities_in(&bounds)
        .iter()
        .filter(|seen| !seen.copied)
        .filter_map(|seen| {
            let EntityData::Item {
                stack,
                pick_up_delay,
                ..
            } = seen.entity.data
            else {
                return None;
            };
            let in_water = crate::physics::fluids::update(world, &seen.bounds).in_water();
            (pick_up_delay == 0 && stack.count > 0 && in_water).then_some(seen.entity.position)
        })
        .collect()
}

/// `ItemGoal.dropItem()`: what its main hand holds thrown ahead of it from just below
/// its eyes, with a delay of 40 before anything picks it up, a little scattered (two
/// draws of its own); whether it held anything.
fn drop_item(ai: &mut Ai<'_, '_>) -> bool {
    let hand = usize::from(slots::MAIN_HAND);
    let Some(stack) = ai.tick.mob.body.equipment.slots[hand].take() else {
        return false;
    };
    let [x, _, z] = ai.position();
    let y = ai.eye_y() - 0.300_000_011_920_928_96;
    let draws = &mut *ai.tick.draws;
    let angle = draws.next_float() * 6.283_185_5;
    let spread = 0.02 * draws.next_float();
    let yaw = ai.tick.yaw * maths::RADIANS;
    let pitch = ai.tick.pitch * maths::RADIANS;
    let motion = [
        f64::from(0.3 * -table_sin(yaw) * table_cos(pitch) + table_cos(angle) * spread),
        f64::from(0.3 * table_sin(pitch) * 1.5),
        f64::from(0.3 * table_cos(yaw) * table_cos(pitch) + table_sin(angle) * spread),
    ];
    ai.tick.out.local.push(Local::Throw {
        stack,
        at: [x, y, z],
        pick_up_delay: 40,
        motion,
    });
    true
}

/// `MoveToItemGoal`'s state: the `tickCount` before which it does not look.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Fetching {
    pub(crate) cooldown: i64,
}

/// `Dolphin.MoveToItemGoal`: to the first item in water within eight blocks, with its
/// playing sound; once done, what it holds thrown again and a pause of up to five
/// seconds.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MoveToItemGoal;

impl Goal for MoveToItemGoal {
    type State = Fetching;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, state: &mut Fetching, ai: &mut Ai<'_, '_>) -> bool {
        if state.cooldown > i64::from(ai.tick_count()) {
            return false;
        }
        !playthings(ai, [8.0; 3]).is_empty()
    }

    fn start(&self, state: &mut Fetching, ai: &mut Ai<'_, '_>) {
        if let Some(first) = playthings(ai, [8.0; 3]).first() {
            navigation::move_to_entity(ai, physics::containing(*first), 1.200_000_047_683_715_8);
            ai.tick.out.events.push(Event::Sound {
                sound: sounds::ENTITY_DOLPHIN_PLAY,
                volume: 1.0,
                pitch: 1.0,
            });
        }
        state.cooldown = 0;
    }

    fn stop(&self, state: &mut Fetching, ai: &mut Ai<'_, '_>) {
        if drop_item(ai) {
            state.cooldown = i64::from(ai.tick_count()) + i64::from(ai.tick.draws.next_int(100));
        }
    }

    fn tick(&self, _: &mut Fetching, ai: &mut Ai<'_, '_>) {
        if let Some(first) = playthings(ai, [8.0; 3]).first() {
            navigation::move_to_entity(ai, physics::containing(*first), 1.200_000_047_683_715_8);
        }
    }
}

/// `Dolphin.PlayWithItemsGoal`: whatever it holds it throws again. No flags.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlayWithItemsGoal;

impl Goal for PlayWithItemsGoal {
    type State = ();

    fn flags(&self) -> u8 {
        0
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        ai.tick.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)].is_some()
    }

    fn stop(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        drop_item(ai);
    }

    fn tick(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        drop_item(ai);
    }
}

/// Which mounts a [`FollowRiddenGoal`] follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mount {
    /// `AbstractBoat`.
    Boat,
    /// `AbstractNautilus`.
    Nautilus,
}

/// `FollowPlayerRiddenEntityGoal`'s state: the player it follows, how it follows them,
/// and its path's timer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Escorting {
    pub(crate) following: Option<EntityId>,
    /// `GO_IN_ENTITY_DIRECTION` rather than `GO_TO_ENTITY`.
    pub(crate) ahead: bool,
    pub(crate) time_to_recalc: i32,
}

/// `FollowPlayerRiddenEntityGoal(mob, class)`: beside a boat (or a nautilus) within five
/// blocks whose rider has moved, first to behind them, then ahead along where they go,
/// with a second move of its own each goal tick. No flags.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FollowRiddenGoal {
    pub(crate) mount: Mount,
}

/// A rider a dolphin may follow, as the view gives them.
#[derive(Debug, Clone)]
struct Rider {
    key: EntityId,
    position: Vec3,
    yaw: f32,
    moved: bool,
    riding: bool,
}

impl FollowRiddenGoal {
    /// Of the mounts of its class within five blocks of its box, in the view's order,
    /// the steering player of each that the region holds.
    fn riders(&self, ai: &Ai<'_, '_>) -> Vec<Rider> {
        let bounds = Aabb::of_entity(ai.position(), ai.width(), ai.height()).inflate([5.0; 3]);
        let players = ai.tick.world.players();
        ai.tick
            .world
            .entities_in(&bounds)
            .iter()
            .filter(|seen| match self.mount {
                Mount::Boat => matches!(seen.entity.data, EntityData::Boat { .. }),
                Mount::Nautilus => NAUTILI.contains(&seen.entity.kind),
            })
            .filter_map(|seen| {
                let seat = seen.entity.riding.passengers.first()?;
                if !seat.player {
                    return None;
                }
                players
                    .iter()
                    .find(|player| player.key == seat.key)
                    .map(|player| Rider {
                        key: player.key,
                        position: player.position,
                        yaw: player.yaw,
                        moved: player.moved_horizontally,
                        riding: player.vehicle.is_some(),
                    })
            })
            .collect()
    }

    fn rider(ai: &Ai<'_, '_>, key: EntityId) -> Option<Rider> {
        ai.tick
            .world
            .players()
            .into_iter()
            .find(|player| player.key == key)
            .map(|player| Rider {
                key: player.key,
                position: player.position,
                yaw: player.yaw,
                moved: player.moved_horizontally,
                riding: player.vehicle.is_some(),
            })
    }
}

impl Goal for FollowRiddenGoal {
    type State = Escorting;

    fn flags(&self) -> u8 {
        0
    }

    fn can_use(&self, state: &mut Escorting, ai: &mut Ai<'_, '_>) -> bool {
        if state
            .following
            .and_then(|key| Self::rider(ai, key))
            .is_some_and(|rider| rider.moved)
        {
            return true;
        }
        self.riders(ai).iter().any(|rider| rider.moved)
    }

    fn can_continue(&self, state: &mut Escorting, ai: &mut Ai<'_, '_>) -> bool {
        state
            .following
            .and_then(|key| Self::rider(ai, key))
            .is_some_and(|rider| rider.riding && rider.moved)
    }

    fn start(&self, state: &mut Escorting, ai: &mut Ai<'_, '_>) {
        if let Some(first) = self.riders(ai).first() {
            state.following = Some(first.key);
        }
        state.time_to_recalc = 0;
        state.ahead = false;
    }

    fn stop(&self, state: &mut Escorting, _: &mut Ai<'_, '_>) {
        state.following = None;
    }

    fn tick(&self, state: &mut Escorting, ai: &mut Ai<'_, '_>) {
        let speed = if state.ahead { 0.01 } else { 0.015 };
        water::move_again(ai, speed);
        state.time_to_recalc -= 1;
        if state.time_to_recalc > 0 {
            return;
        }
        state.time_to_recalc = self.adjusted_tick_delay(10);
        let Some(rider) = state.following.and_then(|key| Self::rider(ai, key)) else {
            return;
        };
        let at = physics::containing(rider.position);
        let (dx, dz) = direction_steps(rider.yaw);
        let distance = physics::distance_sqr(ai.position(), rider.position).sqrt() as f32;
        if !state.ahead {
            // Behind them, a block down.
            let behind = at.offset(-dx, -1, -dz);
            navigation::move_to(
                ai,
                [
                    f64::from(behind.x),
                    f64::from(behind.y),
                    f64::from(behind.z),
                ],
                1.0,
            );
            if distance < 4.0 {
                state.time_to_recalc = 0;
                state.ahead = true;
            }
        } else {
            // Ten blocks ahead along where they go, a block down.
            let ahead = at.offset(dx * 10, 0, dz * 10);
            navigation::move_to(
                ai,
                [
                    f64::from(ahead.x),
                    f64::from(ahead.y - 1),
                    f64::from(ahead.z),
                ],
                1.0,
            );
            if distance > 12.0 {
                state.time_to_recalc = 0;
                state.ahead = false;
            }
        }
    }
}

/// `Mob.aiStep`'s pick-up of what a dolphin finds within a block of its box sideways
/// (`getPickupReach`), alive and grown or not: `Dolphin.pickUpItem`, the first item it
/// may take into an empty main hand, whole, its drop then guaranteed, shown as taken
/// (`take`).
fn pick_up(tick: &mut MobTick<'_>) {
    if !water::alive(&tick.mob) || !tick.mob.body.flags.picks_up_loot {
        return;
    }
    let body = &tick.living.body;
    let bounds = Aabb::of_entity(body.position, body.width, body.height).inflate([1.0, 0.0, 1.0]);
    let hand = usize::from(slots::MAIN_HAND);
    let found: Vec<(EntityId, ItemStack)> = tick
        .world
        .entities_in(&bounds)
        .iter()
        .filter(|seen| !seen.copied)
        .filter_map(|seen| match seen.entity.data {
            EntityData::Item {
                stack,
                pick_up_delay: 0,
                ..
            } if stack.count > 0 => Some((seen.entity.key, stack)),
            _ => None,
        })
        .collect();
    for (key, stack) in found {
        if tick.mob.body.equipment.slots[hand].is_some() {
            break;
        }
        tick.mob.body.equipment.slots[hand] = Some(stack);
        let mut chances = tick.mob.body.equipment.drop_chances.unwrap_or([0.085; 8]);
        chances[hand] = 2.0;
        tick.mob.body.equipment.drop_chances = Some(chances);
        tick.out.local.push(Local::TakeItem {
            item: key,
            count: stack.count,
        });
    }
}

/// `Dolphin.tick()` after the whole living tick: without AI its air stays full; in
/// water its moistness is full, on land it dries a tick at a time, hurt when dry, and
/// flops about on the ground (three draws).
fn moisten(tick: &mut MobTick<'_>) {
    if !tick.effective_ai() {
        tick.living.air = MAX_AIR;
        return;
    }
    if tick.living.body.in_water {
        update(&mut tick.mob, |dolphin| dolphin.moistness = MOISTNESS);
        return;
    }
    let mut moistness = 0;
    update(&mut tick.mob, |dolphin| {
        dolphin.moistness -= 1;
        moistness = dolphin.moistness;
    });
    if moistness <= 0 {
        crate::mobs::body::hurt_server(tick, Damage::DryOut, 1.0);
    }
    if tick.living.body.on_ground {
        let x = (tick.draws.next_float() * 2.0 - 1.0) * 0.2;
        let z = (tick.draws.next_float() * 2.0 - 1.0) * 0.2;
        let body = &mut tick.living.body;
        body.motion[0] += f64::from(x);
        body.motion[1] += 0.5;
        body.motion[2] += f64::from(z);
        tick.yaw = tick.draws.next_float() * 360.0;
        tick.living.body.on_ground = false;
        tick.out.needs_sync = true;
    }
}

/// `Dolphin.mobInteract`: a fish of `#fishes` eaten, with its sound, grows a baby by a
/// tenth of its time, or gives a grown one a fish to go for treasure with; anything
/// else, `AgeableMob`'s (the golden dandelion).
fn mob_interact(used: &mut Used<'_>) -> Interaction {
    let fish = used
        .held
        .is_some_and(|stack| stack.count > 0 && item_in_tag(stack.item, "minecraft:fishes"));
    if !fish {
        return super::squid::mob_interact(used);
    }
    used.did.push(Did::Sound {
        sound: sounds::ENTITY_DOLPHIN_EAT,
        source: SOUND_SOURCE_NEUTRAL,
        at: used.position,
        volume: 1.0,
        pitch: 1.0,
        but_the_player: false,
    });
    let now = used.moment;
    let mut shared = animal::animal_of(&used.mob.state);
    if shared.can_age_up() {
        let age = shared.age(now);
        shared.age_up(animal::speed_up_seconds(age.wrapping_neg()), true, now);
        used.mob.state = animal::with_animal(&used.mob.state, &shared);
    } else {
        update(used.mob, |dolphin| dolphin.got_fish = true);
    }
    Interaction::Success
}

impl MobType for Dolphin {
    fn register_goals(&self, setup: &mut Setup) {
        setup.config.navigation = Settings::of(Kind::WaterBound { breach: true });
        setup.config.move_kind = MoveKind::SmoothSwimming {
            max_turn_x: 85,
            max_turn_y: 10,
            in_water: 0.02,
            outside_water: 0.1,
            gravity: true,
        };
        setup.config.look_kind = LookKind::SmoothSwimming {
            max_y_rot_from_center: 10,
        };
        setup.set_malus(clustine_data::PathType::Water, 0.0);
        setup.add_goal(0, DolphinBreathGoal);
        setup.add_goal(
            0,
            TryFindLiquidGoal {
                fluid_tag: "minecraft:dolphin_tries_to_find",
            },
        );
        setup.add_goal(1, SwimToTreasureGoal);
        setup.add_goal(2, SwimWithPlayerGoal);
        setup.add_goal(4, RandomStrollGoal::swimming(1.0, 10));
        setup.add_goal(4, RandomLookAroundGoal);
        setup.add_goal(5, LookAtPlayerGoal::new(Class::Player, 6.0));
        setup.add_goal(5, JumpGoal);
        setup.add_goal(
            6,
            MeleeAttackGoal {
                speed: 1.200_000_047_683_715_8,
                following_even_if_not_seen: true,
            },
        );
        setup.add_goal(7, MoveToItemGoal);
        setup.add_goal(8, PlayWithItemsGoal);
        setup.add_goal(8, FollowRiddenGoal { mount: Mount::Boat });
        setup.add_goal(
            8,
            FollowRiddenGoal {
                mount: Mount::Nautilus,
            },
        );
        setup.add_goal(
            9,
            AvoidEntityGoal {
                class: Class::Kinds(GUARDIANS),
                max_distance: 8.0,
                walk_speed: 1.0,
                sprint_speed: 1.0,
            },
        );
        setup.add_target(1, HurtByTargetGoal { ignore: GUARDIANS });
    }

    /// `getMaxHeadXRot()` and `getMaxHeadYRot()`: a degree each; its swimming look
    /// control turns the rest.
    fn head(&self) -> HeadLimits {
        HeadLimits {
            max_x_rot: 1,
            max_y_rot: 1,
            ..HeadLimits::default()
        }
    }

    /// `setCanPickUpLoot(true)`.
    fn construct(&self, mob: &mut Mob, _: &mut Random) {
        mob.body.flags.picks_up_loot = true;
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        pick_up(tick);
        animal::after_ai_step(tick);
    }

    fn after_tick(&self, tick: &mut MobTick<'_>) {
        moisten(tick);
    }

    /// `Dolphin.travelInWater`: at its own speed, sinking while it has no target.
    fn travel(&self, tick: &MobTick<'_>) -> Travel {
        let sinks = if tick.mob.body.combat.target.is_none() {
            Sinks::Always
        } else {
            Sinks::Never
        };
        Travel::Swims {
            speed: tick.mob.body.inputs.speed,
            sinks,
        }
    }

    fn pushed_by_fluid(&self) -> bool {
        false
    }

    fn max_air(&self) -> i32 {
        MAX_AIR
    }

    fn refills_air_at_once(&self) -> bool {
        true
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        mob_interact(used)
    }

    fn breed_offspring(&self, _: &mut Breeding<'_>) -> Option<Mob> {
        let mut child = Mob::fresh();
        child.body.flags.picks_up_loot = true;
        Some(child)
    }

    /// A baby's: the type's scaled by 0.65, its eyes at `0.09375f`.
    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        let info = EntityTypeInfo::of(entity_types::DOLPHIN);
        let mut baby = water::scaled(info.width, info.height, info.eye_height, 0.65);
        baby.eye_height = 0.093_75;
        animal::dimensions(mob, baby)
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    /// Out of water, as a fresh dolphin's `isInWater` is.
    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_DOLPHIN_AMBIENT)
    }

    fn ambient_sound_at(&self, _: &Mob, in_water: bool, _: bool) -> Option<i32> {
        Some(if in_water {
            sounds::ENTITY_DOLPHIN_AMBIENT_WATER
        } else {
            sounds::ENTITY_DOLPHIN_AMBIENT
        })
    }

    fn ambient_interval(&self) -> u32 {
        120
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_DOLPHIN_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_DOLPHIN_DEATH)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let own = read(mob);
        let mut values = animal::shown(mob);
        values.push((
            GOT_FISH_INDEX,
            DataShown::Boolean(own.got_fish),
            !own.got_fish,
        ));
        values.push((
            MOISTNESS_INDEX,
            DataShown::Int(own.moistness),
            own.moistness == MOISTNESS,
        ));
        values
    }
}
