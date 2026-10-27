//! What a minecart's movement reads of the blocks and computes on its own, as the 26.3
//! jar's `AbstractMinecart` and `OldMinecartBehavior` have it
//! (`docs/groundwork/n5-brief.md`, sections 3.3 to 3.6 and 3.10): the shapes of rails and
//! where they lead (`exits`), the point of a rail a cart rides at (`getPos`), the block
//! a cart's rules look at (`getCurrentBlockPosOrRailBelow`), the game's `Mth.atan2`,
//! which turns a cart, the rider's push (`getLastClientMoveIntent`), the slowdowns and
//! top speeds of the kinds, a container's signal, and carts pushing one another.
//!
//! Only `OldMinecartBehavior` is ported (`docs/groundwork/n5-plan.md`, decision 1): the
//! experiment `minecart_improvements` is not offered. Nothing here draws a random number
//! or reads anything but the blocks a [`World`] gives, so that a cart's move is a
//! function of them; the arithmetic is the game's, `f64` for its `double` and `f32` for
//! its `float`, widened where Java widens, with no fused multiply-add.

use std::sync::LazyLock;

use clustine_blocks::data::{in_tag, is_class};
use clustine_data::{BlockClass, BlockState};
use clustine_noise::trig;
use clustine_world::{BlockPos, Contents};

use super::{Vec3, World, floor, horizontal_sqr, length_sqr, normalize};
use crate::api::Keys;

/// The game's `Mth.DEG_TO_RAD`, a float.
pub(crate) const DEGREES_TO_RADIANS: f32 = 0.017_453_292;

/// `getDefaultGravity()` of a cart, in water and out of it.
pub(crate) const GRAVITY: f64 = 0.04;
pub(crate) const GRAVITY_IN_WATER: f64 = 0.005;

/// `getAirDrag()`: the float 0.95.
pub(crate) const AIR_DRAG: f32 = 0.95;

/// `RailShape`, in the order of its ordinals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
    NorthSouth,
    EastWest,
    AscendingEast,
    AscendingWest,
    AscendingNorth,
    AscendingSouth,
    SouthEast,
    SouthWest,
    NorthWest,
    NorthEast,
}

impl Shape {
    const ALL: [Shape; 10] = [
        Shape::NorthSouth,
        Shape::EastWest,
        Shape::AscendingEast,
        Shape::AscendingWest,
        Shape::AscendingNorth,
        Shape::AscendingSouth,
        Shape::SouthEast,
        Shape::SouthWest,
        Shape::NorthWest,
        Shape::NorthEast,
    ];

    fn name(self) -> &'static str {
        match self {
            Shape::NorthSouth => "north_south",
            Shape::EastWest => "east_west",
            Shape::AscendingEast => "ascending_east",
            Shape::AscendingWest => "ascending_west",
            Shape::AscendingNorth => "ascending_north",
            Shape::AscendingSouth => "ascending_south",
            Shape::SouthEast => "south_east",
            Shape::SouthWest => "south_west",
            Shape::NorthWest => "north_west",
            Shape::NorthEast => "north_east",
        }
    }

    /// The shape of a rail's state (its `shape` property, which the plain rail's and the
    /// straight rails' shape properties are both called).
    pub(crate) fn of(state: BlockState) -> Option<Shape> {
        let name = state.property("shape")?;
        Shape::ALL.into_iter().find(|shape| shape.name() == name)
    }

    /// `RailShape.isSlope`.
    pub(crate) fn is_slope(self) -> bool {
        matches!(
            self,
            Shape::AscendingEast
                | Shape::AscendingWest
                | Shape::AscendingNorth
                | Shape::AscendingSouth
        )
    }

    /// `AbstractMinecart.exits(shape)`: the two ends of the rail as steps from its block,
    /// the low end of a slope one down.
    pub(crate) fn exits(self) -> ([i32; 3], [i32; 3]) {
        const W: [i32; 3] = [-1, 0, 0];
        const E: [i32; 3] = [1, 0, 0];
        const N: [i32; 3] = [0, 0, -1];
        const S: [i32; 3] = [0, 0, 1];
        const W_DOWN: [i32; 3] = [-1, -1, 0];
        const E_DOWN: [i32; 3] = [1, -1, 0];
        const N_DOWN: [i32; 3] = [0, -1, -1];
        const S_DOWN: [i32; 3] = [0, -1, 1];
        match self {
            Shape::NorthSouth => (N, S),
            Shape::EastWest => (W, E),
            Shape::AscendingEast => (W_DOWN, E),
            Shape::AscendingWest => (W, E_DOWN),
            Shape::AscendingNorth => (N, S_DOWN),
            Shape::AscendingSouth => (N_DOWN, S),
            Shape::SouthEast => (S, E),
            Shape::SouthWest => (S, W),
            Shape::NorthWest => (N, W),
            Shape::NorthEast => (N, E),
        }
    }
}

/// Whether `state` is in `#minecraft:rails`.
pub(crate) fn in_rails_tag(state: BlockState) -> bool {
    in_tag(state, "minecraft:rails")
}

/// `BaseRailBlock.isRail(state)`: of the rails tag, and a rail.
pub(crate) fn is_rail(state: BlockState) -> bool {
    in_rails_tag(state) && is_class(state, BlockClass::BaseRailBlock)
}

/// `getCurrentBlockPosOrRailBelow()` under the old behaviour: the block of the
/// position, or the one below it where that is of the rails tag.
pub(crate) fn rail_block_or_below<W: World + ?Sized>(world: &W, position: Vec3) -> BlockPos {
    let x = floor(position[0]);
    let mut y = floor(position[1]);
    let z = floor(position[2]);
    if in_rails_tag(world.state(BlockPos::new(x, y - 1, z))) {
        y -= 1;
    }
    BlockPos::new(x, y, z)
}

/// The steps of a shape's two ends as doubles: `x0 = pos + 0.5 + exit * 0.5` and the
/// rest of the line through them, as `moveAlongTrack` and `getPos` work it out.
fn widened(step: i32) -> f64 {
    f64::from(step)
}

/// `OldMinecartBehavior.getPos(x, y, z)`: the point of the rail at or just below the
/// position that a cart there rides at, or `None` where there is no rail.
pub(crate) fn get_pos<W: World + ?Sized>(world: &W, [x, y, z]: Vec3) -> Option<Vec3> {
    let bx = floor(x);
    let mut by = floor(y);
    let bz = floor(z);
    if in_rails_tag(world.state(BlockPos::new(bx, by - 1, bz))) {
        by -= 1;
    }
    let state = world.state(BlockPos::new(bx, by, bz));
    if !is_rail(state) {
        return None;
    }
    let shape = Shape::of(state)?;
    let (first, second) = shape.exits();
    let (fx, fy, fz) = (f64::from(bx), f64::from(by), f64::from(bz));
    let x0 = fx + 0.5 + widened(first[0]) * 0.5;
    let y0 = fy + 0.0625 + widened(first[1]) * 0.5;
    let z0 = fz + 0.5 + widened(first[2]) * 0.5;
    let x1 = fx + 0.5 + widened(second[0]) * 0.5;
    let y1 = fy + 0.0625 + widened(second[1]) * 0.5;
    let z1 = fz + 0.5 + widened(second[2]) * 0.5;
    let ex = x1 - x0;
    let ey = (y1 - y0) * 2.0;
    let ez = z1 - z0;
    let t = if ex == 0.0 {
        z - fz
    } else if ez == 0.0 {
        x - fx
    } else {
        let dx = x - x0;
        let dz = z - z0;
        (dx * ex + dz * ez) * 2.0
    };
    let x = x0 + ex * t;
    let mut y = y0 + ey * t;
    let z = z0 + ez * t;
    if ey < 0.0 {
        y += 1.0;
    } else if ey > 0.0 {
        y += 0.5;
    }
    Some([x, y, z])
}

/// The game's `Mth.atan2(y, x)`: not the library's, but an approximation from a table of
/// arcsines and a fast inverse square root, which the yaw of a cart is made with.
/// The table is filled with `libm`'s arcsine and cosine, those of Java's `StrictMath`;
/// the game fills it with `Math.asin` and `Math.cos`, which the plan takes as
/// `StrictMath` (ADR-0028, section 4). **Inferred**: HotSpot's own `Math.cos` could
/// differ from it in the last bit of an entry.
pub(crate) fn atan2(y: f64, x: f64) -> f64 {
    let squared = x * x + y * y;
    if squared.is_nan() {
        return f64::NAN;
    }
    let (mut y, mut x) = (y, x);
    let negative_y = y < 0.0;
    if negative_y {
        y = -y;
    }
    let negative_x = x < 0.0;
    if negative_x {
        x = -x;
    }
    let steep = y > x;
    if steep {
        std::mem::swap(&mut x, &mut y);
    }
    let inverse = fast_inverse_sqrt(squared);
    x *= inverse;
    y *= inverse;
    let biased = frac_bias() + y;
    let index = biased.to_bits() as i32;
    let (asin, cos) = ATAN_TABLES[usize::try_from(index).expect("an index into the tables")];
    let delta = biased - frac_bias();
    let sin_delta = y * cos - x * delta;
    let correction = (6.0 + sin_delta * sin_delta) * sin_delta * 0.166_666_666_666_666_66;
    let mut angle = asin + correction;
    if steep {
        angle = std::f64::consts::FRAC_PI_2 - angle;
    }
    if negative_x {
        angle = std::f64::consts::PI - angle;
    }
    if negative_y {
        angle = -angle;
    }
    angle
}

/// `Mth.FRAC_BIAS`: 2 to the 44th, whose last place is a 256th.
fn frac_bias() -> f64 {
    f64::from_bits(4_805_340_802_404_319_232)
}

/// `Mth.fastInvSqrt`.
fn fast_inverse_sqrt(value: f64) -> f64 {
    let half = 0.5 * value;
    let bits = 6_910_469_410_427_058_090_i64.wrapping_sub((value.to_bits() as i64) >> 1);
    let guess = f64::from_bits(bits as u64);
    guess * (1.5 - half * guess * guess)
}

/// `Mth.ASIN_TAB` and `Mth.COS_TAB`, entry by entry.
static ATAN_TABLES: LazyLock<Box<[(f64, f64)]>> = LazyLock::new(|| {
    (0..257)
        .map(|index| {
            let asin = libm::asin(f64::from(index) / 256.0);
            (asin, libm::cos(asin))
        })
        .collect()
});

/// `Entity.getInputVector(input, speed, yaw)`: nothing for a nearly empty input, the
/// input made of length one where it is longer, scaled by `speed`, and turned by `yaw`
/// in degrees through the game's table of sines.
pub(crate) fn input_vector(input: Vec3, speed: f32, yaw: f32) -> Vec3 {
    let squared = length_sqr(input);
    if squared < 1.0e-7 {
        return [0.0; 3];
    }
    let unit = if squared > 1.0 {
        normalize(input)
    } else {
        input
    };
    let speed = f64::from(speed);
    let [x, y, z] = [unit[0] * speed, unit[1] * speed, unit[2] * speed];
    let sin = f64::from(trig::sin(f64::from(yaw * DEGREES_TO_RADIANS)));
    let cos = f64::from(trig::cos(f64::from(yaw * DEGREES_TO_RADIANS)));
    [x * cos - z * sin, y, z * cos + x * sin]
}

/// `ServerPlayer.getLastClientMoveIntent()`: the keys a rider holds as `(strafe, 0,
/// forward)`, each -1, 0 or 1, of length one (or none) and turned by the rider's yaw,
/// which is what a rider's keys do to the cart under them (`moveAlongTrack`'s push, the
/// brief's finding 3). The riders' step keeps the keys and calls this with them.
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "for the riders' step, which keeps a rider's keys")
)]
pub(crate) fn move_intent(keys: &Keys, yaw: f32) -> Vec3 {
    let strafe: f32 = if keys.left == keys.right {
        0.0
    } else if keys.left {
        1.0
    } else {
        -1.0
    };
    let forward: f32 = if keys.forward == keys.backward {
        0.0
    } else if keys.forward {
        1.0
    } else {
        -1.0
    };
    input_vector([f64::from(strafe), 0.0, f64::from(forward)], 1.0, yaw)
}

/// The rider's push of `moveAlongTrack` (its step 10) to the motion `motion` of a cart
/// whose first passenger is a player with the move intent `intent`: 0.001 of the intent
/// while the cart moves under 0.1 a tick. Whether it pushed, which lifts a powered
/// rail's brake.
pub(crate) fn rider_push(motion: &mut Vec3, intent: Vec3) -> bool {
    if length_sqr(intent) <= 0.0 {
        return false;
    }
    let unit = normalize(intent);
    let speed = horizontal_sqr(*motion);
    if length_sqr(unit) > 0.0 && speed < 0.01 {
        *motion = [
            motion[0] + intent[0] * 0.001,
            motion[1] + 0.0,
            motion[2] + intent[2] * 0.001,
        ];
        return true;
    }
    false
}

/// `Vec3.horizontalDistance`.
pub(crate) fn horizontal_distance(v: Vec3) -> f64 {
    horizontal_sqr(v).sqrt()
}

/// `Mth.clamp(double, double, double)`.
pub(crate) fn clamp(value: f64, min: f64, max: f64) -> f64 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

/// `AbstractMinecartContainer`'s `getRedstoneSignalFromContainer` of what a container
/// cart of `size` slots holds: the share of each stack of its most, averaged over the
/// slots, as a signal from 0 to 15 (`Mth.lerpDiscrete`), in floats. **Inferred**: a
/// container's most for a stack is the smaller of 99 and the item's own
/// (`Container.getMaxStackSize(stack)`, not in the dump).
pub(crate) fn container_signal(contents: &Contents, size: usize) -> u8 {
    if size == 0 {
        return 0;
    }
    let mut share = 0.0_f32;
    for (_, stack) in contents {
        if stack.count <= 0 || stack.item == clustine_data::items::AIR {
            continue;
        }
        let most = super::item::max_stack_size(stack).min(99);
        share += stack.count as f32 / most as f32;
    }
    share /= size as f32;
    let lifted = i32::from(share > 0.0);
    let signal = (share * 14.0).floor() as i32 + lifted;
    u8::try_from(signal.clamp(0, 15)).expect("a signal within 15")
}

/// What one cart is to another as they push (`AbstractMinecart.push`,
/// `pushOtherMinecart`): where it is, how it moves and faces, whether it is a furnace
/// cart, and whether a push set `needsSync`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Pushing {
    pub(crate) position: Vec3,
    pub(crate) motion: Vec3,
    pub(crate) yaw: f32,
    pub(crate) furnace: bool,
    pub(crate) needs_sync: bool,
}

impl Pushing {
    /// `Entity.push(x, y, z)`.
    fn push(&mut self, by: Vec3) {
        if by.iter().all(|component| component.is_finite()) {
            self.motion = [
                self.motion[0] + by[0],
                self.motion[1] + by[1],
                self.motion[2] + by[2],
            ];
            self.needs_sync = true;
        }
    }
}

/// `AbstractMinecart.push(other)`'s distance part: from the cart at `this` to `other`,
/// a tenth of the way of length one (closer than a block, the whole way), halved; `None`
/// where they are within a hundredth of a block, which pushes nothing.
fn push_step(this: Vec3, other: Vec3) -> Option<(f64, f64)> {
    let mut dx = other[0] - this[0];
    let mut dz = other[2] - this[2];
    let squared = dx * dx + dz * dz;
    if squared < 9.999_999_747_378_752e-5 {
        return None;
    }
    let distance = squared.sqrt();
    dx /= distance;
    dz /= distance;
    let mut k = 1.0 / distance;
    if k > 1.0 {
        k = 1.0;
    }
    dx *= k;
    dz *= k;
    dx *= 0.100_000_001_490_116_12;
    dz *= 0.100_000_001_490_116_12;
    dx *= 0.5;
    dz *= 0.5;
    Some((dx, dz))
}

/// `AbstractMinecart.push(other)` where the other is a cart too, with
/// `pushOtherMinecart` under the old behaviour: only carts nearly end to end along this
/// cart's yaw push, a furnace cart going on and the other slowed, and two of a kind
/// meeting at the mean of their motions.
pub(crate) fn push_carts(this: &mut Pushing, other: &mut Pushing) {
    let Some((dx, dz)) = push_step(this.position, other.position) else {
        return;
    };
    let towards = normalize([
        other.position[0] - this.position[0],
        0.0,
        other.position[2] - this.position[2],
    ]);
    let angle = f64::from(this.yaw * DEGREES_TO_RADIANS);
    let facing = normalize([
        f64::from(trig::cos(angle)),
        0.0,
        f64::from(trig::sin(angle)),
    ]);
    let dot = (towards[0] * facing[0] + towards[1] * facing[1] + towards[2] * facing[2]).abs();
    if dot < 0.800_000_011_920_929 {
        return;
    }
    let v = this.motion;
    let w = other.motion;
    if other.furnace && !this.furnace {
        this.motion = [v[0] * 0.2, v[1] * 1.0, v[2] * 0.2];
        this.push([w[0] - dx, 0.0, w[2] - dz]);
        other.motion = [w[0] * 0.95, w[1] * 1.0, w[2] * 0.95];
    } else if !other.furnace && this.furnace {
        other.motion = [w[0] * 0.2, w[1] * 1.0, w[2] * 0.2];
        other.push([v[0] + dx, 0.0, v[2] + dz]);
        this.motion = [v[0] * 0.95, v[1] * 1.0, v[2] * 0.95];
    } else {
        let mx = (w[0] + v[0]) / 2.0;
        let mz = (w[2] + v[2]) / 2.0;
        this.motion = [v[0] * 0.2, v[1] * 1.0, v[2] * 0.2];
        this.push([mx - dx, 0.0, mz - dz]);
        other.motion = [w[0] * 0.2, w[1] * 1.0, w[2] * 0.2];
        other.push([mx + dx, 0.0, mz + dz]);
    }
}

/// `AbstractMinecart.push(other)` where the other, at `other`, is no cart (a player, an
/// armour stand's own push): the cart is pushed away from it, and the push the other
/// is to be given, a quarter of it the other way, is returned.
pub(crate) fn push_cart_by(this: &mut Pushing, other: Vec3) -> Option<Vec3> {
    let (dx, dz) = push_step(this.position, other)?;
    this.push([-dx, 0.0, -dz]);
    Some([dx / 4.0, 0.0, dz / 4.0])
}

/// `Entity.push(other)` of an entity at `this` pushed by the entity at `other`, a
/// twentieth of the way of length one: the push to give this one (unless it carries
/// someone or is not pushable) and the push to give the other, the opposite; `None` for
/// two within a hundredth of a block along both axes.
pub(crate) fn entity_push(this: Vec3, other: Vec3) -> Option<(Vec3, Vec3)> {
    let mut dx = other[0] - this[0];
    let mut dz = other[2] - this[2];
    let mut d = dx.abs().max(dz.abs());
    if d < 0.009_999_999_776_482_582 {
        return None;
    }
    d = d.sqrt();
    dx /= d;
    dz /= d;
    let mut k = 1.0 / d;
    if k > 1.0 {
        k = 1.0;
    }
    dx *= k;
    dz *= k;
    dx *= 0.050_000_000_745_058_06;
    dz *= 0.050_000_000_745_058_06;
    Some(([-dx, 0.0, -dz], [dx, 0.0, dz]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_games_atan2_is_within_its_approximations_error_of_the_libraries() {
        for (y, x) in [
            (1.0, 1.0),
            (0.3, -0.7),
            (-0.2, -0.9),
            (-1.0, 0.01),
            (0.0, 1.0),
            (0.0, -1.0),
            (5.0, 0.0),
        ] {
            let ours = atan2(y, x);
            let exact = libm::atan2(y, x);
            // The fast inverse square root's one Newton step leaves a few millionths.
            assert!((ours - exact).abs() < 1.0e-5, "{y} {x}: {ours} {exact}");
        }
        // Straight along an axis it is exact: the table's first entry and its quarter
        // turns.
        assert_eq!(atan2(0.0, 1.0), 0.0);
        assert_eq!(atan2(1.0, 0.0), std::f64::consts::FRAC_PI_2);
        assert_eq!(atan2(0.0, -1.0), std::f64::consts::PI);
    }

    #[test]
    fn a_riders_forward_key_points_where_they_look_and_a_diagonal_is_of_length_one() {
        let forward = Keys {
            forward: true,
            ..Keys::default()
        };
        // Yaw 0 looks along positive z.
        assert_eq!(move_intent(&forward, 0.0), [0.0, 0.0, 1.0]);
        // Yaw -90 looks along positive x (east): the table's sine of -90 degrees.
        let east = move_intent(&forward, -90.0);
        let sin = f64::from(trig::sin(f64::from(-90.0_f32 * DEGREES_TO_RADIANS)));
        let cos = f64::from(trig::cos(f64::from(-90.0_f32 * DEGREES_TO_RADIANS)));
        assert_eq!(east.map(f64::to_bits), [-sin, 0.0, cos].map(f64::to_bits));
        assert!((east[0] - 1.0).abs() < 1.0e-6 && east[2].abs() < 1.0e-6);
        // Forward and left at once: normalised before it is turned.
        let diagonal = move_intent(
            &Keys {
                forward: true,
                left: true,
                ..Keys::default()
            },
            0.0,
        );
        let half = 1.0 / 2.0_f64.sqrt();
        assert_eq!(
            diagonal.map(f64::to_bits),
            [half, 0.0, half].map(f64::to_bits)
        );
        // Opposite keys cancel; no key is no intent.
        let both = Keys {
            forward: true,
            backward: true,
            ..Keys::default()
        };
        assert_eq!(move_intent(&both, 30.0), [0.0; 3]);
        assert_eq!(move_intent(&Keys::default(), 30.0), [0.0; 3]);
        // Backward and right: (-1, 0, -1) normalised.
        let back_right = move_intent(
            &Keys {
                backward: true,
                right: true,
                ..Keys::default()
            },
            0.0,
        );
        assert_eq!(
            back_right.map(f64::to_bits),
            [-half, 0.0, -half].map(f64::to_bits)
        );
    }

    #[test]
    fn a_rider_pushes_a_cart_by_a_thousandth_only_while_it_moves_under_a_tenth() {
        let intent = [0.0, 0.0, 1.0];
        let mut slow = [0.0, 0.0, 0.05];
        assert!(rider_push(&mut slow, intent));
        assert_eq!(slow, [0.0, 0.0, 0.05 + 0.001]);
        // At a tenth a tick (0.01 squared) it does nothing.
        let mut fast = [0.0, 0.0, 0.1];
        assert!(!rider_push(&mut fast, intent));
        assert_eq!(fast, [0.0, 0.0, 0.1]);
        let mut still = [0.0; 3];
        assert!(!rider_push(&mut still, [0.0; 3]));
    }

    #[test]
    fn a_container_signals_its_fill_by_the_share_of_each_stack() {
        use clustine_world::ItemStack;
        let dirt = |count| ItemStack::new(clustine_data::items::DIRT, count);
        assert_eq!(container_signal(&Vec::new(), 27), 0);
        // One item in 27 slots: a little above nothing is 1.
        assert_eq!(container_signal(&vec![(0, dirt(1))], 27), 1);
        // Full: every slot 64 of 64, `floor(1 * 14) + 1`.
        let full: Contents = (0..27).map(|slot| (slot, dirt(64))).collect();
        assert_eq!(container_signal(&full, 27), 15);
        // Half the slots full: 13.5 / 27 * 14 = 7, plus one.
        let half: Contents = (0..13).map(|slot| (slot, dirt(64))).collect();
        let share = 13.0_f32 / 27.0;
        assert_eq!(
            container_signal(&half, 27),
            (share * 14.0).floor() as u8 + 1
        );
    }

    #[test]
    fn two_carts_meeting_end_to_end_share_the_mean_of_their_motions() {
        // Facing east (yaw 0: cos 1, sin 0), the other just east of it.
        let mut this = Pushing {
            position: [0.0, 0.0, 0.0],
            motion: [0.3, 0.0, 0.0],
            yaw: 0.0,
            furnace: false,
            needs_sync: false,
        };
        let mut other = Pushing {
            position: [0.5, 0.0, 0.0],
            motion: [0.0; 3],
            ..this
        };
        push_carts(&mut this, &mut other);
        // d = 0.5: the step is 1 * min(1, 2) * 0.1f * 0.5.
        let dx = 1.0 * 1.0 * 0.100_000_001_490_116_12 * 0.5;
        let mean = (0.0 + 0.3) / 2.0;
        assert_eq!(this.motion[0], 0.3 * 0.2 + (mean - dx));
        assert_eq!(other.motion[0], 0.0 * 0.2 + (mean + dx));
        assert!(this.needs_sync && other.needs_sync);
        // Side by side across the yaw, they do not push.
        let mut beside = Pushing {
            position: [0.0, 0.0, 0.5],
            motion: [0.0; 3],
            ..this
        };
        let before = this;
        push_carts(&mut this, &mut beside);
        assert_eq!(this, before);
    }
}
