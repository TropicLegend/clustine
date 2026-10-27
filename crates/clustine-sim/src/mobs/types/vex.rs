//! The vex (`Vex` and its inner classes, n7 Vex; `docs/groundwork/n9-plan.md`, section
//! 8.4; the brief's 2.7): a small spirit that flies through everything, made by an
//! evoker's summoning (`mobs/ai/spells.rs`) and by its egg.
//!
//! Its tick moves it through blocks (`noPhysics` during the living tick), no gravity
//! pulls it once its first tick is over (`setNoGravity(true)` at the tick's end), and a
//! summoned one's life runs out: every tick it counts down, and at nothing it is hurt
//! by `starve` 1 and starts another 20 ticks. Its move control pushes it straight at
//! where it wants to be; it charges at its target's eyes one asked tick in four, hitting
//! it where their boxes meet; otherwise it drifts, one asked tick in four, towards an
//! empty block within 7 of its bound origin. Its targets are what hurt it (but a raider,
//! alerting vexes), its owner's target, then players. It holds an iron sword it never
//! drops.
//!
//! Named differences: the blocks it passes through do nothing to it (the game's
//! `isAffectedByBlocks` keeps a vex in fire burning while it moves through everything;
//! Clustine's body does not look at the blocks of a body that moves so); its owner is
//! held by key, as the fangs hold theirs, so an owner the region does not read is none
//! (the game's `getOwner()` of an owner not loaded is null too); its sword's attack
//! damage is not added to its own, as no mob's equipment changes its attributes yet
//! (its hit is 4).

use clustine_data::{entity_types, items, sounds};
use clustine_world::{BlockPos, EntityData, EntityId, ItemStack, TypeState};
use serde::{Deserialize, Serialize};

use super::{MobType, Spawning};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::ai::goals::{FloatGoal, LookAtPlayerGoal, TargetRules, Targeting, ai_bounds};
use crate::mobs::ai::maths::{self, reduced_tick_delay};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::{self, Class, Conditions};
use crate::mobs::ai::targets::{self, RAIDERS, Target};
use crate::mobs::ai::{Ai, Operation, Setup, controls};
use crate::mobs::body::hurt::{DamageSource, DamageType};
use crate::mobs::body::{self, Event};
use crate::mobs::tick::MobTick;
use crate::region::Random;
use crate::shown::DataShown;

/// Whether the type is whole, so that `built()` gives it and an egg makes it.
pub(crate) const BUILT: bool = true;

/// The vex's entity type, by id.
pub(crate) const VEX: u16 = entity_types::VEX as u16;

/// The synced data's index of `Entity.DATA_NO_GRAVITY`.
const NO_GRAVITY_INDEX: u8 = 5;

/// The synced data's index of `Vex.DATA_FLAGS_ID` (D9 counts it 16).
pub(crate) const FLAGS_INDEX: u8 = 16;

/// `Vex.FLAG_IS_CHARGING`.
pub(crate) const CHARGING: u8 = 1;

/// The version of [`VexState`]'s encoding.
const VERSION: u8 = 1;

/// The ticks between two `starve` hurts once a summoned vex's life ran out.
const STARVE_EVERY: i32 = 20;

/// What a vex keeps beyond a monster: its saved fields (`owner`, `bound_pos`,
/// `life_ticks`), its synced flags and `NoGravity`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct VexState {
    /// `owner`: the mob that summoned it.
    pub(crate) owner: Option<EntityId>,
    /// `boundOrigin` (`bound_pos`): what its drifting keeps near.
    pub(crate) bound_origin: Option<BlockPos>,
    /// `limitedLifeTicks` (`life_ticks`), where it `hasLimitedLife`.
    pub(crate) life: Option<i32>,
    /// `DATA_FLAGS_ID`, synced: the charging bit.
    pub(crate) flags: u8,
    /// `isNoGravity()`, synced: set at the end of its every tick.
    pub(crate) no_gravity: bool,
}

impl VexState {
    /// `isCharging()`.
    pub(crate) fn charging(&self) -> bool {
        self.flags & CHARGING != 0
    }

    /// `setIsCharging(charging)`.
    pub(crate) fn set_charging(&mut self, charging: bool) {
        if charging {
            self.flags |= CHARGING;
        } else {
            self.flags &= !CHARGING;
        }
    }
}

/// The vex's state of a type state; nothing, or what this build cannot read, as a
/// fresh vex's.
pub(crate) fn read(state: &TypeState) -> VexState {
    if state.bytes.is_empty() || state.version != VERSION {
        return VexState::default();
    }
    postcard::from_bytes(&state.bytes).unwrap_or_default()
}

/// The type state of `own`: nothing for a fresh vex's.
pub(crate) fn write(own: &VexState) -> TypeState {
    if *own == VexState::default() {
        return TypeState::default();
    }
    TypeState {
        version: VERSION,
        bytes: postcard::to_allocvec(own).expect("a type state is encoded into memory"),
    }
}

pub(crate) fn state_of(mob: &Mob) -> VexState {
    read(&mob.state)
}

/// Changes the vex's state, written back only where it changed.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut VexState)) {
    let mut own = state_of(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        mob.state = write(&own);
    }
}

/// The owner of the vex `entity`, as its record keeps it (`getOwnerReference()`); none
/// for anything that is no vex.
pub(crate) fn owner_of(entity: &clustine_world::Entity) -> Option<EntityId> {
    if entity.kind != VEX {
        return None;
    }
    let EntityData::Mob { state, .. } = &entity.data else {
        return None;
    };
    read(state).owner
}

/// The vex.
pub(crate) struct Vex;

impl MobType for Vex {
    /// `Vex.registerGoals` (Vex.txt 94–164).
    fn register_goals(&self, setup: &mut Setup) {
        setup.add_goal(0, FloatGoal);
        setup.add_goal(4, VexChargeAttackGoal);
        setup.add_goal(8, VexRandomMoveGoal);
        setup.add_goal(
            9,
            LookAtPlayerGoal {
                probability: 1.0,
                ..LookAtPlayerGoal::new(Class::Player, 3.0)
            },
        );
        setup.add_goal(10, LookAtPlayerGoal::new(Class::Mob, 8.0));
        // `HurtByTargetGoal(Raider).setAlertOthers()`, `VexCopyOwnerTargetGoal`, then
        // `NearestAttackableTargetGoal(Player, true)`.
        targets::register(
            setup,
            &[(
                1,
                Target::HurtBy {
                    ignore: RAIDERS,
                    alert: true,
                },
            )],
        );
        setup.add_target(2, VexCopyOwnerTargetGoal);
        targets::register(
            setup,
            &[(3, Target::Nearest(targets::NearestTarget::players(true)))],
        );
    }

    /// `Vex.finalizeSpawn` before `Monster`'s (Vex.txt 406–426):
    /// `populateDefaultEquipmentSlots`, an iron sword in the main hand that it never
    /// drops, with no draw; then `populateDefaultEquipmentEnchantments`, the level's
    /// draw for the sword (the armour's slots hold nothing).
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let hand = usize::from(slots::MAIN_HAND);
        spawn.mob.body.equipment.slots[hand] = Some(ItemStack::new(items::IRON_SWORD, 1));
        let chances = spawn
            .mob
            .body
            .equipment
            .drop_chances
            .get_or_insert([0.085; 8]);
        chances[hand] = 0.0;
        crate::mobs::making::populate_default_equipment_enchantments(
            spawn.mob,
            spawn.level,
            spawn.difficulty.1,
        );
    }

    /// `Vex.tick`'s head: `noPhysics` through the living tick.
    fn before_tick(&self, tick: &mut MobTick<'_>) {
        tick.living.no_physics = true;
    }

    /// `Vex.tick`'s tail (Vex.txt 59–92): `noPhysics` off, no gravity from now on, and a
    /// limited life counted down, hurt by `starve` 1 at nothing and set to 20 again.
    fn after_tick(&self, tick: &mut MobTick<'_>) {
        tick.living.no_physics = false;
        let mut starves = false;
        update(&mut tick.mob, |own| {
            own.no_gravity = true;
            if let Some(life) = &mut own.life {
                *life -= 1;
                if *life <= 0 {
                    *life = STARVE_EVERY;
                    starves = true;
                }
            }
        });
        if starves {
            body::hurt_by(tick, &DamageSource::of(DamageType("minecraft:starve")), 1.0);
        }
    }

    /// `getLightLevelDependentMagicValue()` is 1 (Vex.txt 401–404): `Monster.aiStep`
    /// counts its idle time two more every tick, wherever it is. The body adds them
    /// only where the light's own value is above a half, so the vex adds them where it
    /// is not.
    fn before_ai_step(&self, tick: &mut MobTick<'_>) {
        if tick.mob.body.idle.no_action_since.is_some()
            && crate::mobs::body::burning::magic_value(tick) <= 0.5
        {
            tick.mob.body.idle_gained.gained = tick.mob.body.idle_gained.gained.saturating_add(2);
        }
    }

    fn no_gravity(&self, mob: &Mob) -> bool {
        state_of(mob).no_gravity
    }

    /// `VexMoveControl.tick` (n7 Vex$VexMoveControl 19–146).
    fn tick_move_control(&self, ai: &mut Ai<'_, '_>) {
        tick_move_control(ai);
    }

    /// `Mob.getBaseExperienceReward` of an `xpReward` of 3, its sword counted.
    fn experience_reward(&self, _: u16, mob: &Mob, draws: &mut Random) -> i32 {
        super::illagers::experience(3, mob, draws)
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_VEX_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_VEX_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_VEX_DEATH)
    }

    /// `NoGravity` and its flags.
    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let own = state_of(mob);
        vec![
            (
                NO_GRAVITY_INDEX,
                DataShown::Boolean(own.no_gravity),
                !own.no_gravity,
            ),
            (FLAGS_INDEX, DataShown::Byte(own.flags), own.flags == 0),
        ]
    }
}

/// `VexMoveControl.tick`: only while it moves to a place, which it reaches within its
/// box's size (`AABB.getSize`, the mean of its sides), waiting then with its motion
/// halved; short of it, its motion is pushed towards the place by `speed · 0.05`, and it
/// faces along its motion, or towards its target where it has one, its body with it.
pub(crate) fn tick_move_control(ai: &mut Ai<'_, '_>) {
    let movement = ai.tick.mob.body.controls.movement.clone();
    if movement.operation != Operation::MoveTo {
        return;
    }
    let at = ai.position();
    let v = [
        movement.wanted[0] - at[0],
        movement.wanted[1] - at[1],
        movement.wanted[2] - at[2],
    ];
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let [sx, sy, sz] = ai_bounds(ai).size();
    let size = (sx + sy + sz) / 3.0;
    if length < size {
        ai.tick.mob.body.controls.movement.operation = Operation::Wait;
        let motion = &mut ai.tick.living.body.motion;
        *motion = motion.map(|component| component * 0.5);
        return;
    }
    let scale = movement.speed * 0.05 / length;
    let motion = &mut ai.tick.living.body.motion;
    for axis in 0..3 {
        motion[axis] += v[axis] * scale;
    }
    let yaw = match sensing::target(ai) {
        None => {
            let motion = ai.tick.living.body.motion;
            facing(motion[0], motion[2])
        }
        Some(target) => facing(target.position[0] - at[0], target.position[2] - at[2]),
    };
    ai.tick.yaw = yaw;
    ai.tick.mob.body.head.body_yaw = yaw;
}

/// `-((float) Mth.atan2(x, z)) * 57.295776f`.
fn facing(x: f64, z: f64) -> f32 {
    -(maths::atan2(x, z) as f32) * 57.295_776
}

/// `Vex$VexChargeAttackGoal` (n7 Vex$VexChargeAttackGoal, MOVE, every tick): at a living
/// target, while it wants to be nowhere, one asked tick in four (its own
/// `nextInt(reducedTickDelay(7))`), farther than 2, it charges at the target's eyes at
/// speed 1, its flag on and its charge heard; it hits the target where their boxes
/// meet, and aims again within 3.
#[derive(Debug, Clone, Copy)]
pub(crate) struct VexChargeAttackGoal;

impl Goal for VexChargeAttackGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        let Some(target) = sensing::target(ai) else {
            return false;
        };
        if !target.alive || controls::has_wanted(ai) {
            return false;
        }
        if ai.tick.draws.next_int(reduced_tick_delay(7) as u32) != 0 {
            return false;
        }
        target.distance_sqr(ai.position()) > 4.0
    }

    fn can_continue(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        controls::has_wanted(ai)
            && state_of(&ai.tick.mob).charging()
            && sensing::target(ai).is_some_and(|target| target.alive)
    }

    fn start(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        if let Some(target) = sensing::target(ai) {
            let eyes = [target.position[0], target.eye_y, target.position[2]];
            controls::set_wanted_position(ai, eyes, 1.0);
        }
        update(&mut ai.tick.mob, |own| own.set_charging(true));
        ai.tick.out.events.push(Event::Sound {
            sound: sounds::ENTITY_VEX_CHARGE,
            volume: 1.0,
            pitch: 1.0,
        });
    }

    fn stop(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        update(&mut ai.tick.mob, |own| own.set_charging(false));
    }

    fn tick(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        let Some(target) = sensing::target(ai) else {
            return;
        };
        if ai_bounds(ai).intersects(target.bounds) {
            let mob_type = ai.tick.mob_type;
            mob_type.do_hurt_target(ai, &target);
            update(&mut ai.tick.mob, |own| own.set_charging(false));
        } else if target.distance_sqr(ai.position()) < 9.0 {
            let eyes = [target.position[0], target.eye_y, target.position[2]];
            controls::set_wanted_position(ai, eyes, 1.0);
        }
    }
}

/// `Vex$VexRandomMoveGoal` (n7 Vex$VexRandomMoveGoal, MOVE): while it wants to be
/// nowhere, one asked tick in four (its own `nextInt(reducedTickDelay(7))`), it tries
/// three times a block within 7 across and 5 up or down of its bound origin (or of
/// itself), drifting at 0.25 to the middle of the first that is air and, with no
/// target, looking there. It runs for one tick.
#[derive(Debug, Clone, Copy)]
pub(crate) struct VexRandomMoveGoal;

impl Goal for VexRandomMoveGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        !controls::has_wanted(ai) && ai.tick.draws.next_int(reduced_tick_delay(7) as u32) == 0
    }

    fn can_continue(&self, _: &mut (), _: &mut Ai<'_, '_>) -> bool {
        false
    }

    fn tick(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        let origin = state_of(&ai.tick.mob)
            .bound_origin
            .unwrap_or_else(|| ai.block_position());
        for _ in 0..3 {
            let dx = ai.tick.draws.next_int(15) as i32 - 7;
            let dy = ai.tick.draws.next_int(11) as i32 - 5;
            let dz = ai.tick.draws.next_int(15) as i32 - 7;
            let pos = BlockPos::new(origin.x + dx, origin.y + dy, origin.z + dz);
            if !ai.tick.world.state(pos).props().is_air() {
                continue;
            }
            let centre = [
                f64::from(pos.x) + 0.5,
                f64::from(pos.y) + 0.5,
                f64::from(pos.z) + 0.5,
            ];
            controls::set_wanted_position(ai, centre, 0.25);
            if sensing::target(ai).is_none() {
                controls::set_look_at(ai, centre, 180.0, 20.0);
            }
            break;
        }
    }
}

/// `Vex$VexCopyOwnerTargetGoal` (n7 Vex$VexCopyOwnerTargetGoal, a `TargetGoal` that
/// need not see): its owner's target, where the owner is a mob with one that a
/// non-combat test passes (seen by anyone, not itself; no sight or visibility asked)
/// and that is within its home. No draw.
#[derive(Debug, Clone, Copy)]
pub(crate) struct VexCopyOwnerTargetGoal;

impl VexCopyOwnerTargetGoal {
    const RULES: TargetRules = TargetRules {
        must_see: false,
        must_reach: false,
    };

    /// The owner's target (`Targeting.getTarget()` of the owner the vex reads), by key.
    fn owners_target(ai: &Ai<'_, '_>) -> Option<EntityId> {
        let owner = state_of(&ai.tick.mob).owner?;
        let seen = ai.tick.world.entity(owner)?;
        Mob::decode(&seen.entity.data).ok()?.body.combat.target
    }
}

impl Goal for VexCopyOwnerTargetGoal {
    type State = Targeting;

    fn flags(&self) -> u8 {
        flags::TARGET
    }

    fn can_use(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        let Some(target) = Self::owners_target(ai).and_then(|key| sensing::find(ai, key)) else {
            return false;
        };
        let conditions = Conditions::non_combat()
            .ignore_line_of_sight()
            .ignore_invisibility();
        Self::RULES.can_attack(state, ai, Some(&target), &conditions, &|_| true)
    }

    fn can_continue(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        Self::RULES.can_continue(state, ai)
    }

    fn start(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        let target = Self::owners_target(ai).and_then(|key| sensing::find(ai, key));
        sensing::set_target(ai, target.as_ref());
        TargetRules::start(state);
    }

    fn stop(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        TargetRules::stop(state, ai);
    }
}
