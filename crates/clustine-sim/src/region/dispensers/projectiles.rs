//! A dispenser's projectiles (`ProjectileDispenseBehavior`; `docs/groundwork/n9-plan.md`,
//! section 4.9; part S5's): arrows, eggs, snowballs, the potions, the experience bottle
//! and rockets, made in front along the facing at the configuration's power and
//! uncertainty, their draws their own.
//!
//! `execute` makes the projectile by its item's `asProjectile` at the configuration's
//! position, shoots it along the facing's normal (`spawnProjectileUsingShoot`) and
//! shrinks the stack by one; `dispense` then plays the configuration's level event (1002,
//! a rocket's 1004) and the smoke. Nothing in it reads the level's random, so the
//! dispenser's stream gives only the slot's draws: the projectile's UUID, its
//! construction and its shot are drawn from its own (`make_projectile`). What the
//! projectile is made with: no owner, and an arrow's pickup allowed (`asProjectile`).
//!
//! A projectile whose making fails (a full chunk, no key left, a place the region cannot
//! read) is lost and the stack shrunk all the same, as the game's `addFreshEntity` that
//! fails is not looked at. A rocket that nothing makes keeps its stack and clicks a
//! failure: `make_firework` makes nothing until the rocket is built (part S3), and a
//! dispenser that ate its rockets for nothing would be a seam a player sees at once. The
//! game has no such failure (its rocket is always made), so the click is Clustine's.

use clustine_data::{Face, entity_types};
use clustine_world::{BlockPos, EntityData, Pickup};

use super::super::Region;
use super::super::projectiles::{Aim, Making, shot_record};
use super::{Dispense, FAILED};
use crate::api::{ItemStack, TickOutput};
use crate::mobs::tick::Shot;
use crate::physics::Vec3;

#[cfg(test)]
#[path = "projectiles_n9_tests.rs"]
mod projectiles_n9_tests;

/// `LevelEvent.SOUND_DISPENSER_PROJECTILE_LAUNCH`: the click of a projectile shot.
pub(crate) const PROJECTILE_LAUNCHED: i32 = 1002;
/// `LevelEvent.SOUND_FIREWORK_SHOOT`, a rocket's `overrideDispenseEvent`.
pub(crate) const FIREWORK_SHOT: i32 = 1004;

/// `ProjectileItem$DispenseConfig$Builder`'s defaults (its constructor, 13–22).
const DEFAULT_POWER: f32 = 1.1;
const DEFAULT_UNCERTAINTY: f32 = 6.0;

/// How far out of the dispenser's middle a projectile is made (the builder's
/// `lambda$new$0`: `getDispensePosition(source, 0.7, (0, 0.1, 0))`), and a rocket
/// (`FireworkRocketItem.getEntityJustOutsideOfBlockPos`, the double as the dump has it).
const OUT: f64 = 0.7;
const LIFT: f64 = 0.1;
const ROCKET_OUT: f64 = 0.500_009_999_999_747_4;

/// `ProjectileItem.createDispenseConfig` of the item that makes `kind`: the power and
/// uncertainty of its shot and its level event.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Config {
    pub(crate) power: f32,
    pub(crate) uncertainty: f32,
    pub(crate) event: i32,
}

/// The configuration of the projectile `kind`: the default for arrows, eggs and
/// snowballs; the potions' and the bottle's a quarter more power and half the
/// uncertainty (`ThrowablePotionItem` and `ExperienceBottleItem.createDispenseConfig`,
/// computed in floats as there); a rocket's 0.5 and 1.0 with its own event
/// (`FireworkRocketItem.createDispenseConfig`).
pub(crate) fn config(kind: i32) -> Config {
    let default = Config {
        power: DEFAULT_POWER,
        uncertainty: DEFAULT_UNCERTAINTY,
        event: PROJECTILE_LAUNCHED,
    };
    match kind {
        entity_types::SPLASH_POTION
        | entity_types::LINGERING_POTION
        | entity_types::EXPERIENCE_BOTTLE => Config {
            power: DEFAULT_POWER * 1.25,
            uncertainty: DEFAULT_UNCERTAINTY * 0.5,
            ..default
        },
        entity_types::FIREWORK_ROCKET => Config {
            power: 0.5,
            uncertainty: 1.0,
            event: FIREWORK_SHOT,
        },
        _ => default,
    }
}

/// The middle of the block at `pos` (`BlockSource.center`).
fn centre(pos: BlockPos) -> Vec3 {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5,
        f64::from(pos.z) + 0.5,
    ]
}

/// The facing's normal, as `getStepX`, `getStepY` and `getStepZ` give it.
fn normal(facing: Face) -> Vec3 {
    crate::physics::piston::step(facing).map(f64::from)
}

/// Where a dispenser at `pos` facing `facing` makes the projectile `kind`: seven tenths
/// out of its middle and a tenth up (`getDispensePosition`: the middle plus
/// `0.7 · step + offset`, added in that order), a rocket just outside the block
/// (the middle plus `step · 0.50001`).
pub(crate) fn projectile_position(pos: BlockPos, facing: Face, kind: i32) -> Vec3 {
    let middle = centre(pos);
    let [dx, dy, dz] = normal(facing);
    if kind == entity_types::FIREWORK_ROCKET {
        return [
            middle[0] + dx * ROCKET_OUT,
            middle[1] + dy * ROCKET_OUT,
            middle[2] + dz * ROCKET_OUT,
        ];
    }
    [
        middle[0] + OUT * dx,
        middle[1] + (OUT * dy + LIFT),
        middle[2] + OUT * dz,
    ]
}

impl Region {
    /// `ProjectileDispenseBehavior.execute` of the dispensed stack, a projectile of the
    /// entity type `kind`, and `dispense`'s level event and smoke. What the slot is left
    /// with.
    pub(crate) fn dispense_projectile(
        &mut self,
        dispense: &mut Dispense,
        kind: i32,
        output: &mut TickOutput,
    ) -> Option<ItemStack> {
        let Some(stack) = dispense.stack() else {
            return self.default_dispense(dispense, output);
        };
        let config = config(kind);
        let at = projectile_position(dispense.pos, dispense.facing, kind);
        let direction = normal(dispense.facing);
        if kind == entity_types::FIREWORK_ROCKET {
            // `asProjectile`: one of the stack, shot at an angle, with no owner.
            let one = stack.with_count(1);
            let shot = Some((Aim::Direction(direction), config.power, config.uncertainty));
            if self
                .make_firework(&one, at, shot, true, None, output)
                .is_none()
            {
                self.click_and_smoke(dispense, FAILED, output);
                return dispense.stack();
            }
            dispense.shrink(1);
            self.click_and_smoke(dispense, config.event, output);
            return dispense.stack();
        }
        let Ok(projectile) = u16::try_from(kind) else {
            return self.default_dispense(dispense, output);
        };
        let shot = Shot {
            fire_ticks: 0,
            kind: projectile,
            stack,
            weapon: None,
            at,
            direction,
            power: config.power,
            uncertainty: config.uncertainty,
            from_mob: None,
        };
        let Some(mut data) = shot_record(&shot, None) else {
            // No kind but the ones `behaviour_of` names comes here.
            return self.default_dispense(dispense, output);
        };
        if let EntityData::Arrow(arrow) = &mut data {
            arrow.pickup = Pickup::Allowed;
        }
        let making = Making {
            kind: projectile,
            at,
            data,
            aim: Aim::Direction(direction),
            power: config.power,
            uncertainty: config.uncertainty,
        };
        self.make_projectile(making, |_, _| {}, output);
        dispense.shrink(1);
        self.click_and_smoke(dispense, config.event, output);
        dispense.stack()
    }
}
