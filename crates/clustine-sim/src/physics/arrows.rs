//! The arithmetic of arrows and tridents (`AbstractArrow`, `ThrownTrident`;
//! `docs/groundwork/n9-plan.md`, sections 4.2 and 4.3; part S2's): the drag in air and
//! water, gravity, the damage by speed, the position moved back at a block hit, the
//! deflection off a target that refuses the hurt, the fall from a block that changed,
//! the trident's return, the enchantments' values and the order in which the blocks an
//! arrow passes through apply their effects. The region's part is `region/arrows.rs`.
//!
//! Every number is the game's to the bit: `f32` where it uses `float`, widened where Java
//! widens (n9 AbstractArrow, ThrownTrident; n7k ProjectileDeflection).

use super::Vec3;

/// `getDefaultGravity()` of every arrow (n9 AbstractArrow 754–757).
pub(crate) const GRAVITY: f64 = 0.05;

/// `getAirDrag()` (n9 AbstractArrow 567–570).
pub(crate) const AIR_DRAG: f32 = 0.99;

/// `getWaterInertia()`: an arrow's, and a trident's (n9 AbstractArrow 1959–1962,
/// ThrownTrident 569–572).
pub(crate) const WATER_INERTIA: f32 = 0.6;
pub(crate) const TRIDENT_WATER_INERTIA: f32 = 0.99;

/// `SHAKE_TIME`: the ticks an arrow shakes after it sticks.
pub(crate) const SHAKE_TIME: u32 = 7;

/// `tickDespawn`: the ticks in the ground after which an arrow is gone.
pub(crate) const DESPAWN: u32 = 1200;

/// `Arrow.tick`: the ticks in the ground after which a tipped arrow is a plain one.
pub(crate) const TIPPED_FADES: u32 = 600;

/// `ThrownTrident.tick`: past this many ticks in the ground a trident has dealt its
/// damage.
pub(crate) const TRIDENT_SETTLES: u32 = 4;

/// What `onHitBlock` moves an arrow back by along each axis of its motion
/// (`0.05000000074505806`, 0.05f widened).
pub(crate) const MOVE_BACK: f64 = 0.050_000_000_745_058_06;

/// `shouldFall`'s box about the arrow's position.
pub(crate) const FALL_MARGIN: f64 = 0.06;

/// The trident's damage before its enchantments (`ThrownTrident.onHitEntity`).
pub(crate) const TRIDENT_DAMAGE: f32 = 8.0;

/// `ProjectileDeflection.REVERSE`'s scale for an arrow refused by its target, and for a
/// trident's bounce.
pub(crate) const ARROW_BOUNCE: Vec3 = [0.2, 0.2, 0.2];
pub(crate) const TRIDENT_BOUNCE: Vec3 = [0.02, 0.2, 0.02];

/// `LivingEntity`'s ticks for the next arrow stuck in it to go: `20 · (30 − count)`
/// (`removeArrowTime`), which the arrow count's part (the lead's, N9f) counts down.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the arrow count's part (the lead's, N9f) reads it"
    )
)]
pub(crate) fn arrow_goes_after(count: i32) -> i32 {
    20 * (30 - count)
}

/// `Vec3.scale((double) f)`, as `applyInertia` keeps a share of the motion.
pub(crate) fn inertia(motion: Vec3, share: f32) -> Vec3 {
    super::scale(motion, f64::from(share))
}

/// `applyGravity`: the motion less 0.05 down.
pub(crate) fn gravity([x, y, z]: Vec3) -> Vec3 {
    [x + 0.0, y + -GRAVITY, z + 0.0]
}

/// `Math.signum`, which is 0 for 0 (Rust's `f64::signum` is 1 there).
fn signum(value: f64) -> f64 {
    if value > 0.0 {
        1.0
    } else if value < 0.0 {
        -1.0
    } else {
        value
    }
}

/// `onHitBlock`: the position less `signum(m) · 0.05` of the motion `m`, along each axis
/// on its own (n9 AbstractArrow 1317–1408).
pub(crate) fn move_back(position: Vec3, motion: Vec3) -> Vec3 {
    std::array::from_fn(|axis| position[axis] - signum(motion[axis]) * MOVE_BACK)
}

/// `Mth.ceil(Mth.clamp((double) speed · damage, 0, 2.147483647E9))`: the hurt of an arrow
/// moving at `speed` (its motion's length narrowed) with `damage`.
pub(crate) fn damage_by_speed(speed: f32, damage: f64) -> i32 {
    let product = (f64::from(speed) * damage).clamp(0.0, 2.147_483_647e9);
    product.ceil() as i32
}

/// The bound of a critical arrow's draw, `nextInt(j / 2 + 2)`.
pub(crate) fn crit_bound(damage: i32) -> u32 {
    u32::try_from(damage / 2 + 2).unwrap_or(2)
}

/// `(int) Math.min((long) drawn + j, Integer.MAX_VALUE)`: a critical arrow's hurt.
pub(crate) fn crit_damage(damage: i32, drawn: u32) -> i32 {
    let sum = i64::from(drawn) + i64::from(damage);
    i32::try_from(sum.min(i64::from(i32::MAX))).unwrap_or(i32::MAX)
}

/// `1.2f / (nextFloat() · 0.2f + 0.9f)`: the pitch of an arrow's hit sounds.
pub(crate) fn hit_pitch(draw: f32) -> f32 {
    1.2 / (draw * 0.2 + 0.9)
}

/// `ProjectileDeflection.REVERSE` (n7k ProjectileDeflection `lambda$static$1`): the
/// motion times `-scale · 0.5` on each axis, and the yaw turned by `170 + 20 · draw`.
pub(crate) fn reverse(motion: Vec3, scale: Vec3, yaw: f32, draw: f32) -> (Vec3, f32) {
    let turn = 170.0 + draw * 20.0;
    let motion = std::array::from_fn(|axis| motion[axis] * (-scale[axis] * 0.5));
    (motion, yaw + turn)
}

/// `startFalling`: the motion times three of the arrow's floats, each times 0.2f, drawn
/// in the order x, y, z.
pub(crate) fn start_falling(motion: Vec3, draws: [f32; 3]) -> Vec3 {
    std::array::from_fn(|axis| motion[axis] * f64::from(draws[axis] * 0.2))
}

/// Power on the bow (`minecraft:damage`, `add` of `linear` 1 + 0.5 a level above the
/// first, for an arrow as the direct attacker): `modifyDamage` of `damage`, in floats.
pub(crate) fn power_damage(damage: f64, level: i32) -> f64 {
    let mut damage = damage as f32;
    if level > 0 {
        damage += 1.0 + 0.5 * (level - 1) as f32;
    }
    f64::from(damage)
}

/// Impaling on the trident (`add` of `linear` 2.5 a level, against
/// `#sensitive_to_impaling`).
pub(crate) fn impaling_damage(damage: f32, level: i32, sensitive: bool) -> f32 {
    if level > 0 && sensitive {
        damage + 2.5 + 2.5 * (level - 1) as f32
    } else {
        damage
    }
}

/// Punch on the bow (`minecraft:knockback`, `add` of `linear` 1 a level): the
/// knockback `modifyKnockback(…, 0f)` gives.
pub(crate) fn punch_knockback(level: i32) -> f32 {
    if level > 0 {
        0.0 + (1.0 + 1.0 * (level - 1) as f32)
    } else {
        0.0
    }
}

/// `doKnockback`: the push of a target of knockback resistance `resistance` by an arrow
/// moving by `motion` with `knockback`: along its horizontal motion made of length one,
/// times `k · 0.6 · max(0, 1 − resistance)`; `None` for no push.
pub(crate) fn punch_push(motion: Vec3, knockback: f32, resistance: f64) -> Option<Vec3> {
    let k = f64::from(knockback);
    if k <= 0.0 {
        return None;
    }
    let left = 0.0_f64.max(1.0 - resistance);
    let flat = [motion[0] * 1.0, motion[1] * 0.0, motion[2] * 1.0];
    let push = super::scale(normalise(flat), k * 0.6 * left);
    (super::length_sqr(push) > 0.0).then_some(push)
}

/// `Vec3.normalize()`: of length one, or nothing where it is shorter than `1.0E-5f`.
pub(crate) fn normalise([x, y, z]: Vec3) -> Vec3 {
    let length = (x * x + y * y + z * z).sqrt();
    if length < f64::from(1.0e-5_f32) {
        return [0.0; 3];
    }
    [x / length, y / length, z / length]
}

/// Loyalty on the trident (`minecraft:trident_return_acceleration`, `add` of `linear`
/// 1 a level), as `ID_LOYALTY` keeps it: clamped to 0 to 127.
pub(crate) fn loyalty(level: i32) -> i32 {
    level.clamp(0, 127)
}

/// `ThrownTrident.tick`'s pull towards the owner's eyes `eyes` of a trident at
/// `position` moving by `motion` with loyalty `loyalty`: its height raised by
/// `d.y · 0.015 · L`, and its motion `· 0.95 + d̂ · 0.05 · L`.
pub(crate) fn trident_return(position: Vec3, motion: Vec3, eyes: Vec3, loyalty: i32) -> [Vec3; 2] {
    let d = super::sub(eyes, position);
    let level = f64::from(loyalty);
    let raised = [position[0], position[1] + d[1] * 0.015 * level, position[2]];
    let pull = 0.05 * level;
    let motion = super::add(super::scale(motion, 0.95), super::scale(normalise(d), pull));
    [raised, motion]
}

/// The yaw and pitch an arrow turns towards in its flight (`AbstractArrow.tick`): of its
/// motion `v` at the start of the tick, the yaw of `atan2(−x, −z)` instead for one that
/// passes through blocks, whose pitch is the same (the horizontal length is).
pub(crate) fn turn(yaw: f32, pitch: f32, v: Vec3, no_physics: bool) -> [f32; 2] {
    let towards = if no_physics { [-v[0], v[1], -v[2]] } else { v };
    super::thrown::update_rotation(yaw, pitch, towards)
}

/// What the blocks an arrow's box passes through do to it, as
/// `InsideBlockEffectApplier.StepBasedCollector` orders it: each once in a step,
/// however many blocks of the step apply it, in the order of its types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum InsideEffect {
    /// `FIRE_IGNITE`: fire.
    FireIgnite,
    /// `LAVA_IGNITE`: lava, or a cauldron of it.
    LavaIgnite,
    /// `EXTINGUISH`: water, powder snow, a cauldron of water or snow.
    Extinguish,
}

/// The effects `met`, each with the step of the walk it was met in, in the order the
/// collector applies them: step by step, and within a step each type once, in the
/// order of the types.
pub(crate) fn inside_effects(met: &[(i32, InsideEffect)]) -> Vec<InsideEffect> {
    let mut applied = Vec::new();
    let mut step: Option<i32> = None;
    let mut in_step: Vec<InsideEffect> = Vec::new();
    for &(at, effect) in met {
        if step != Some(at) {
            in_step.sort();
            applied.append(&mut in_step);
            step = Some(at);
        }
        if !in_step.contains(&effect) {
            in_step.push(effect);
        }
    }
    in_step.sort();
    applied.append(&mut in_step);
    applied
}

/// The fire ticks an entity of `fire` has after `effect` (`BaseFireBlock.fireIgnite`,
/// `lavaIgnite`, `clearFire`): no arrow is immune to fire.
pub(crate) fn after_inside(fire: i32, effect: InsideEffect) -> i32 {
    match effect {
        InsideEffect::FireIgnite => {
            let mut fire = fire;
            if fire < 0 {
                fire += 1;
            }
            if fire >= 0 { fire.max(160) } else { fire }
        }
        InsideEffect::LavaIgnite => fire.max(300),
        InsideEffect::Extinguish => fire.min(0),
    }
}
