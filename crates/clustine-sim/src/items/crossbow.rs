//! The crossbow (`CrossbowItem`; `docs/groundwork/n9-plan.md`, section 4.6; part S4's):
//! its charge by the ticks it was held (Quick Charge), the projectiles kept in
//! `charged_projectiles`, and the shot of each (Multishot, Piercing), arrows and rockets.
//!
//! Named differences: the game keeps whether the charge's first two sounds have played
//! on the one `CrossbowItem` of the server, shared by every holder (n9 CrossbowItem 3–30);
//! Clustine works them out from the ticks the use has run, so that each use hears each
//! once. The player's shot along their look is turned about their up vector by JOML's
//! quaternion (`Quaternionf.setAngleAxis` and `Vector3f.rotate`), which is not in the
//! dumps: ported from JOML's source (**Inferred**); a shot of one projectile, at angle
//! 0, is the look itself.

use clustine_data::{items, sounds};
use clustine_world::ItemStack;
use clustine_world::components::{Component, Kind};

use crate::mobs::ai::maths::{RADIANS, hotspot_sin, table_cos, table_sin};
use crate::physics::Vec3;

/// `CrossbowItem.MAX_CHARGE_DURATION`, in seconds.
const CHARGE_SECONDS: f32 = 1.25;

/// The progress of the charge at which its first and second sounds play
/// (`START_SOUND_PERCENT`, `MID_SOUND_PERCENT`), and at which it loads.
pub(crate) const START_SOUND_AT: f32 = 0.2;
pub(crate) const MIDDLE_SOUND_AT: f32 = 0.5;
pub(crate) const LOADED_AT: f32 = 1.0;

/// `ARROW_POWER` and `FIREWORK_POWER`: the speed of a shot of arrows, and of one with a
/// rocket among them (`getShootingPower`, n9 CrossbowItem 93–102).
const ARROW_POWER: f32 = 3.15;
const FIREWORK_POWER: f32 = 1.6;

/// A player's shot's uncertainty (`use`'s `performShooting(…, 1.0f, null)`).
pub(crate) const PLAYER_UNCERTAINTY: f32 = 1.0;

/// How far below the shooter's eyes a crossbow's rocket starts (`createProjectile`'s
/// `getEyeY() − 0.15000000596046448`, n9 CrossbowItem 342–383).
pub(crate) const ROCKET_BELOW_THE_EYES: f64 = 0.150_000_005_960_464_48;

/// `CrossbowItem.getChargeDuration`: 1.25 seconds, a quarter of a second less a level
/// of Quick Charge (its `crossbow_charge_time` effect), at least none, in ticks, rounded
/// down.
pub(crate) fn charge_duration(stack: &ItemStack) -> i32 {
    let seconds = super::enchantments::crossbow_charge_seconds(stack, CHARGE_SECONDS);
    (seconds * 20.0).floor() as i32
}

/// The charge's progress after `ticks` of use: the ticks over the charge's duration, in
/// floats (`onUseTick`'s `(useDuration − remaining) / (float) chargeDuration`).
pub(crate) fn progress(ticks: i32, stack: &ItemStack) -> f32 {
    ticks as f32 / charge_duration(stack) as f32
}

/// Whether the charge reaches `threshold` at the use's tick `ticks`, having not reached
/// it at the tick before: where the game's flag, false since the use began, is set.
pub(crate) fn reaches(ticks: i32, stack: &ItemStack, threshold: f32) -> bool {
    progress(ticks, stack) >= threshold && !(ticks >= 1 && progress(ticks - 1, stack) >= threshold)
}

/// `CrossbowItem.isCharged`: its `charged_projectiles` hold something.
pub(crate) fn is_charged(stack: &ItemStack) -> bool {
    !charged(stack).is_empty()
}

/// The projectiles a crossbow is charged with (`CHARGED_PROJECTILES`, empty by
/// default).
pub(crate) fn charged(stack: &ItemStack) -> Vec<ItemStack> {
    match stack.components.get(Kind::ChargedProjectiles) {
        Some(Component::ChargedProjectiles(projectiles)) => projectiles.clone(),
        _ => Vec::new(),
    }
}

/// The crossbow `stack` charged with `projectiles` (`tryLoadProjectiles`'
/// `ChargedProjectiles.ofNonEmpty`), or emptied of its charge where there are none
/// (`performShooting`'s `set(CHARGED_PROJECTILES, EMPTY)`: the default, so the patch
/// says nothing of it).
pub(crate) fn with_charge(stack: &ItemStack, projectiles: Vec<ItemStack>) -> ItemStack {
    let mut stack = stack.clone();
    let projectiles: Vec<ItemStack> = projectiles
        .into_iter()
        .filter(|projectile| projectile.count > 0)
        .collect();
    if projectiles.is_empty() {
        stack.components.reset(Kind::ChargedProjectiles);
    } else {
        stack
            .components
            .set(Component::ChargedProjectiles(projectiles));
    }
    stack
}

/// `getShootingPower`: 1.6 where a rocket is among the charge, else 3.15, one speed for
/// every projectile.
pub(crate) fn shooting_power(charged: &[ItemStack]) -> f32 {
    if charged
        .iter()
        .any(|projectile| projectile.item == items::FIREWORK_ROCKET)
    {
        FIREWORK_POWER
    } else {
        ARROW_POWER
    }
}

/// `getDurabilityUse`: a rocket wears the crossbow by 3, anything else by 1.
pub(crate) fn durability_use(projectile: &ItemStack) -> i32 {
    if projectile.item == items::FIREWORK_ROCKET {
        3
    } else {
        1
    }
}

/// The charge's sounds (`CrossbowItem$ChargingSounds`): the default
/// `crossbow.loading_start`, `_middle` and `_end`, or Quick Charge's, which start with
/// its level's sound, have none in the middle and end as the default does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChargingSounds {
    pub(crate) start: Option<i32>,
    pub(crate) middle: Option<i32>,
    pub(crate) end: Option<i32>,
}

/// `getChargingSounds(stack)`.
pub(crate) fn charging_sounds(stack: &ItemStack) -> ChargingSounds {
    match super::enchantments::quick_charge_start_sound(stack) {
        Some(start) => ChargingSounds {
            start: Some(start),
            middle: None,
            end: Some(sounds::ITEM_CROSSBOW_LOADING_END),
        },
        None => ChargingSounds {
            start: Some(sounds::ITEM_CROSSBOW_LOADING_START),
            middle: Some(sounds::ITEM_CROSSBOW_LOADING_MIDDLE),
            end: Some(sounds::ITEM_CROSSBOW_LOADING_END),
        },
    }
}

/// The pitch of the charge's end, of a draw of the level's random: `1 / (r · 0.5 + 1)
/// + 0.2` (n9 CrossbowItem `lambda$onUseTick$2`).
pub(crate) fn loaded_pitch(drawn: f32) -> f32 {
    1.0 / (drawn * 0.5 + 1.0) + 0.2
}

/// `getShotPitch(random, index)`: the first projectile's `crossbow.shoot` at 1, without
/// a draw; each after it of a draw of the shooter's random, `1 / (r · 0.5 + 1.8)` and
/// 0.63 for the odd, 0.43 for the even (`getRandomShotPitch`).
pub(crate) fn shot_pitch(index: usize, next_float: impl FnOnce() -> f32) -> f32 {
    if index == 0 {
        return 1.0;
    }
    let base = if index & 1 == 1 { 0.63 } else { 0.43 };
    1.0 / (next_float() * 0.5 + 1.8) + base
}

/// `Entity.calculateViewVector(xRot, yRot)` by the game's sine table, in floats.
fn view_vector(pitch: f32, yaw: f32) -> [f32; 3] {
    let pitch = pitch * RADIANS;
    let yaw = -yaw * RADIANS;
    let (yaw_cos, yaw_sin) = (table_cos(yaw), table_sin(yaw));
    let (pitch_cos, pitch_sin) = (table_cos(pitch), table_sin(pitch));
    [yaw_sin * pitch_cos, -pitch_sin, yaw_cos * pitch_cos]
}

/// `CrossbowItem.shootProjectile` without a target, a player's: the look
/// (`getViewVector(1)`) turned by `angle` degrees about the up vector
/// (`getUpVector(1)`, the view of the pitch less 90): `new Quaternionf().setAngleAxis(
/// (double) (angle · DEG_TO_RAD), up)` and `Vector3f.rotate`, in JOML's floats.
pub(crate) fn player_shot_vector(yaw: f32, pitch: f32, angle: f32) -> Vec3 {
    let up = view_vector(pitch - 90.0, yaw);
    let look = view_vector(pitch, yaw);
    // `setAngleAxis(double angle, double x, double y, double z)`: the sine of half the
    // angle in doubles, the cosine from it (`Math.cosFromSin`, which is the root of one
    // less its square for so small an angle), each narrowed to a float.
    let half = f64::from(angle * RADIANS) * 0.5;
    let sine = hotspot_sin(half);
    let [qx, qy, qz] = up.map(|axis| (f64::from(axis) * sine) as f32);
    let qw = (1.0 - sine * sine).sqrt() as f32;
    // `Quaternionf.transform(x, y, z, dest)`.
    let (xx, yy, zz, ww) = (qx * qx, qy * qy, qz * qz, qw * qw);
    let (xy, xz, yz, xw) = (qx * qy, qx * qz, qy * qz, qx * qw);
    let (zw, yw) = (qz * qw, qy * qw);
    let k = 1.0 / (xx + yy + zz + ww);
    let [x, y, z] = look;
    let fma = f32::mul_add;
    [
        f64::from(fma(
            (xx - yy - zz + ww) * k,
            x,
            fma(2.0 * (xy - zw) * k, y, (2.0 * (xz + yw) * k) * z),
        )),
        f64::from(fma(
            2.0 * (xy + zw) * k,
            x,
            fma((yy - xx - zz + ww) * k, y, (2.0 * (yz - xw) * k) * z),
        )),
        f64::from(fma(
            2.0 * (xz - yw) * k,
            x,
            fma(2.0 * (yz + xw) * k, y, ((zz - xx - yy + ww) * k) * z),
        )),
    ]
}
