//! Random positions (`ai.util`, the brief's 2.5): `RandomPos`, `GoalUtils`,
//! `DefaultRandomPos`, `LandRandomPos`, `HoverRandomPos`, `AirAndWaterRandomPos`,
//! `AirRandomPos`, and `BehaviorUtils.getRandomSwimmablePos`. Every draw is the mob's
//! own, at its place; the ten tries are always made.

use clustine_blocks::data::has_water;
use clustine_data::PathComputation;
use clustine_world::BlockPos;

use super::Ai;
use super::maths::{self, hotspot_cos, hotspot_sin};
use super::navigation::{self, OVERWORLD_HEIGHT};
use super::pathfinder;
use crate::physics::{self, Vec3};

/// `(double)(float)(π / 2)`.
const HALF_PI: f64 = 1.570_796_370_506_286_6;

/// `Mth.SQRT_OF_TWO`, the float widened.
const SQRT_OF_TWO: f64 = 1.414_213_538_169_861;

/// `RandomPos.generateRandomDirection(random, h, v)`: x, then y, then z.
pub(crate) fn random_direction(ai: &mut Ai<'_, '_>, horizontal: i32, vertical: i32) -> BlockPos {
    let draws = &mut *ai.tick.draws;
    let x = draws.next_int((2 * horizontal + 1) as u32) as i32 - horizontal;
    let y = draws.next_int((2 * vertical + 1) as u32) as i32 - vertical;
    let z = draws.next_int((2 * horizontal + 1) as u32) as i32 - horizontal;
    BlockPos::new(x, y, z)
}

/// `RandomPos.generateRandomDirectionWithinRadians`: a direction within `angle` of
/// `(dir_x, dir_z)`, its distance between `min` and `max` (HotSpot's sine and cosine);
/// none, and no third draw, when it falls outside `max`.
#[allow(
    clippy::too_many_arguments,
    reason = "the game's own arguments, each read as it reads them"
)]
pub(crate) fn random_direction_within_radians(
    ai: &mut Ai<'_, '_>,
    min: f64,
    max: f64,
    vertical: i32,
    y_offset: i32,
    dir_x: f64,
    dir_z: f64,
    angle: f64,
) -> Option<BlockPos> {
    let draws = &mut *ai.tick.draws;
    let facing = maths::atan2(dir_z, dir_x) - HALF_PI;
    let turned = facing + f64::from(2.0 * draws.next_float() - 1.0) * angle;
    let distance = maths::lerp(draws.next_double().sqrt(), min, max) * SQRT_OF_TWO;
    let x = -distance * hotspot_sin(turned);
    let z = distance * hotspot_cos(turned);
    if x.abs() > max || z.abs() > max {
        return None;
    }
    let y = draws.next_int((2 * vertical + 1) as u32) as i32 - vertical + y_offset;
    Some(physics::containing([x, f64::from(y), z]))
}

/// `RandomPos.generateRandomPosTowardDirection(mob, h, random, offset)`: the offset
/// from the mob, drawn back towards its home's side where it has one.
pub(crate) fn random_pos_toward_direction(
    ai: &mut Ai<'_, '_>,
    horizontal: f64,
    offset: BlockPos,
) -> BlockPos {
    let [x, y, z] = ai.position();
    let mut ox = f64::from(offset.x);
    let mut oz = f64::from(offset.z);
    if let Some((home, _)) = ai.home()
        && horizontal > 1.0
    {
        let draws = &mut *ai.tick.draws;
        if x > f64::from(home.x) {
            ox -= draws.next_double() * horizontal / 2.0;
        } else {
            ox += draws.next_double() * horizontal / 2.0;
        }
        if z > f64::from(home.z) {
            oz -= draws.next_double() * horizontal / 2.0;
        } else {
            oz += draws.next_double() * horizontal / 2.0;
        }
    }
    physics::containing([ox + x, f64::from(offset.y) + y, oz + z])
}

/// `RandomPos.generateRandomPos(mob, supplier)`: ten tries, always; of those that give a
/// place, the one the mob likes best, the earlier where two are liked as well; at the
/// middle of its bottom.
pub(crate) fn generate(
    ai: &mut Ai<'_, '_>,
    supplier: impl FnMut(&mut Ai<'_, '_>) -> Option<BlockPos>,
) -> Option<Vec3> {
    generate_scored(ai, supplier, |ai, pos| {
        f64::from(ai.tick.mob_type.walk_target_value(ai, pos))
    })
}

/// `RandomPos.generateRandomPos(supplier, scorer)`: as [`generate`], the places liked by
/// `scorer`.
fn generate_scored(
    ai: &mut Ai<'_, '_>,
    mut supplier: impl FnMut(&mut Ai<'_, '_>) -> Option<BlockPos>,
    scorer: impl Fn(&Ai<'_, '_>, BlockPos) -> f64,
) -> Option<Vec3> {
    let mut best: Option<BlockPos> = None;
    let mut best_score = f64::NEG_INFINITY;
    for _ in 0..10 {
        let Some(pos) = supplier(ai) else {
            continue;
        };
        let score = scorer(ai, pos);
        if score > best_score {
            best_score = score;
            best = Some(pos);
        }
    }
    best.map(|pos| {
        [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        ]
    })
}

/// `RandomPos.moveUpOutOfSolid(pos, maxY, predicate)`: up while the predicate holds.
fn move_up_out_of_solid(
    pos: BlockPos,
    max_y: i32,
    mut solid: impl FnMut(BlockPos) -> bool,
) -> BlockPos {
    if !solid(pos) {
        return pos;
    }
    let mut at = BlockPos::new(pos.x, pos.y + 1, pos.z);
    while at.y <= max_y && solid(at) {
        at = BlockPos::new(at.x, at.y + 1, at.z);
    }
    at
}

/// `RandomPos.moveUpToAboveSolid(pos, above, maxY, predicate)`: up out of the solid,
/// then up to `above` more blocks while under the top, back down one where it meets a
/// solid.
fn move_up_to_above_solid(
    pos: BlockPos,
    above: i32,
    max_y: i32,
    mut solid: impl FnMut(BlockPos) -> bool,
) -> BlockPos {
    if !solid(pos) {
        return pos;
    }
    let mut at = BlockPos::new(pos.x, pos.y + 1, pos.z);
    while at.y <= max_y && solid(at) {
        at = BlockPos::new(at.x, at.y + 1, at.z);
    }
    let start = at.y;
    while at.y <= max_y && at.y - start < above {
        let up = BlockPos::new(at.x, at.y + 1, at.z);
        if solid(up) {
            break;
        }
        at = up;
    }
    at
}

/// `Level.getMaxY()`.
fn max_y(ai: &Ai<'_, '_>) -> i32 {
    ai.tick.world.min_y() + OVERWORLD_HEIGHT - 1
}

/// `GoalUtils.mobRestricted(mob, h)`: it has a home whose centre is within its radius
/// and `h` and one of where it stands.
pub(crate) fn mob_restricted(ai: &Ai<'_, '_>, horizontal: f64) -> bool {
    let Some((home, radius)) = ai.home() else {
        return false;
    };
    let [x, y, z] = ai.position();
    let (dx, dy, dz) = (
        f64::from(home.x) + 0.5 - x,
        f64::from(home.y) + 0.5 - y,
        f64::from(home.z) + 0.5 - z,
    );
    let reach = f64::from(radius) + horizontal + 1.0;
    dx * dx + dy * dy + dz * dz < reach * reach
}

/// `GoalUtils.isOutsideLimits(pos)`: outside the world's height.
fn outside_limits(ai: &Ai<'_, '_>, pos: BlockPos) -> bool {
    pos.y < ai.tick.world.min_y() || pos.y > max_y(ai)
}

/// `GoalUtils.isRestricted(restricted, mob, pos)`.
fn restricted_out(ai: &Ai<'_, '_>, restricted: bool, pos: BlockPos) -> bool {
    restricted && !ai.within_home(pos)
}

/// `GoalUtils.hasMalus(mob, pos)`: the static type of the cell costs something.
pub(crate) fn has_malus(ai: &Ai<'_, '_>, pos: BlockPos) -> bool {
    let kind = pathfinder::static_path_type(ai.tick.world, ai.tick.world.min_y(), pos);
    ai.malus(kind) != 0.0
}

/// `GoalUtils.isWater(mob, pos)`.
fn is_water(ai: &Ai<'_, '_>, pos: BlockPos) -> bool {
    has_water(ai.tick.world.state(pos))
}

/// `GoalUtils.isSolid(mob, pos)`: the legacy solid flag.
fn is_solid(ai: &Ai<'_, '_>, pos: BlockPos) -> bool {
    ai.tick.world.state(pos).props().is_solid()
}

/// `DefaultRandomPos.generateRandomPosTowardDirection`: in the world, within the home,
/// somewhere to stand, and free.
fn default_towards(
    ai: &mut Ai<'_, '_>,
    horizontal: i32,
    restricted: bool,
    direction: BlockPos,
) -> Option<BlockPos> {
    let pos = random_pos_toward_direction(ai, f64::from(horizontal), direction);
    (!outside_limits(ai, pos)
        && !restricted_out(ai, restricted, pos)
        && navigation::is_stable_destination(ai, pos)
        && !has_malus(ai, pos))
    .then_some(pos)
}

/// `DefaultRandomPos.getPos(mob, h, v)`.
pub(crate) fn default_pos(ai: &mut Ai<'_, '_>, horizontal: i32, vertical: i32) -> Option<Vec3> {
    let restricted = mob_restricted(ai, f64::from(horizontal));
    generate(ai, |ai| {
        let direction = random_direction(ai, horizontal, vertical);
        default_towards(ai, horizontal, restricted, direction)
    })
}

/// `DefaultRandomPos.getPosTowards(mob, h, v, target, angle)`.
pub(crate) fn default_pos_towards(
    ai: &mut Ai<'_, '_>,
    horizontal: i32,
    vertical: i32,
    target: Vec3,
    angle: f64,
) -> Option<Vec3> {
    let direction = physics::sub(target, ai.position());
    let restricted = mob_restricted(ai, f64::from(horizontal));
    generate(ai, |ai| {
        let towards = random_direction_within_radians(
            ai,
            0.0,
            f64::from(horizontal),
            vertical,
            0,
            direction[0],
            direction[2],
            angle,
        )?;
        default_towards(ai, horizontal, restricted, towards)
    })
}

/// `DefaultRandomPos.getPosAway(mob, h, v, from)`.
pub(crate) fn default_pos_away(
    ai: &mut Ai<'_, '_>,
    horizontal: i32,
    vertical: i32,
    from: Vec3,
) -> Option<Vec3> {
    let direction = physics::sub(ai.position(), from);
    let restricted = mob_restricted(ai, f64::from(horizontal));
    generate(ai, |ai| {
        let away = random_direction_within_radians(
            ai,
            0.0,
            f64::from(horizontal),
            vertical,
            0,
            direction[0],
            direction[2],
            HALF_PI,
        )?;
        default_towards(ai, horizontal, restricted, away)
    })
}

/// `LandRandomPos.generateRandomPosTowardDirection`: as the default's, any cost
/// allowed.
fn land_towards(
    ai: &mut Ai<'_, '_>,
    horizontal: f64,
    restricted: bool,
    direction: BlockPos,
) -> Option<BlockPos> {
    let pos = random_pos_toward_direction(ai, horizontal, direction);
    (!outside_limits(ai, pos)
        && !restricted_out(ai, restricted, pos)
        && navigation::is_stable_destination(ai, pos))
    .then_some(pos)
}

/// `LandRandomPos.movePosUpOutOfSolid(mob, pos)`: up out of what is solid; none in water
/// or at a cost.
fn land_up_out_of_solid(ai: &Ai<'_, '_>, pos: BlockPos) -> Option<BlockPos> {
    let top = max_y(ai);
    let up = move_up_out_of_solid(pos, top, |at| is_solid(ai, at));
    (!is_water(ai, up) && !has_malus(ai, up)).then_some(up)
}

/// `LandRandomPos.getPos(mob, h, v)`.
pub(crate) fn land_pos(ai: &mut Ai<'_, '_>, horizontal: i32, vertical: i32) -> Option<Vec3> {
    let restricted = mob_restricted(ai, f64::from(horizontal));
    generate(ai, |ai| {
        let direction = random_direction(ai, horizontal, vertical);
        let pos = land_towards(ai, f64::from(horizontal), restricted, direction)?;
        land_up_out_of_solid(ai, pos)
    })
}

/// `LandRandomPos.getPos(mob, h, v, scorer)`: as [`land_pos`], each place scored by
/// `scorer` in place of the mob's walk target value; a place scored no higher than
/// negative infinity is never taken.
pub(crate) fn land_pos_scored(
    ai: &mut Ai<'_, '_>,
    horizontal: i32,
    vertical: i32,
    scorer: impl Fn(&Ai<'_, '_>, BlockPos) -> f64,
) -> Option<Vec3> {
    let restricted = mob_restricted(ai, f64::from(horizontal));
    generate_scored(
        ai,
        |ai| {
            let direction = random_direction(ai, horizontal, vertical);
            let pos = land_towards(ai, f64::from(horizontal), restricted, direction)?;
            land_up_out_of_solid(ai, pos)
        },
        scorer,
    )
}

/// `LandRandomPos.getPosTowards(mob, h, v, target)`.
pub(crate) fn land_pos_towards(
    ai: &mut Ai<'_, '_>,
    horizontal: i32,
    vertical: i32,
    target: Vec3,
) -> Option<Vec3> {
    let direction = physics::sub(target, ai.position());
    let restricted = mob_restricted(ai, f64::from(horizontal));
    land_in_direction(
        ai,
        0.0,
        f64::from(horizontal),
        vertical,
        direction,
        restricted,
    )
}

/// `LandRandomPos.getPosAway(mob, h, v, from)`.
pub(crate) fn land_pos_away(
    ai: &mut Ai<'_, '_>,
    horizontal: i32,
    vertical: i32,
    from: Vec3,
) -> Option<Vec3> {
    land_pos_away_between(ai, 0.0, f64::from(horizontal), vertical, from)
}

/// `LandRandomPos.getPosAway(mob, minH, maxH, v, from)`: from a mob exactly at `from`, a
/// direction drawn first.
pub(crate) fn land_pos_away_between(
    ai: &mut Ai<'_, '_>,
    min: f64,
    max: f64,
    vertical: i32,
    from: Vec3,
) -> Option<Vec3> {
    let mut direction = physics::sub(ai.position(), from);
    if physics::length_sqr(direction).sqrt() == 0.0 {
        let draws = &mut *ai.tick.draws;
        let x = draws.next_double() - 0.5;
        let z = draws.next_double() - 0.5;
        direction = [x, 0.0, z];
    }
    let restricted = mob_restricted(ai, max);
    land_in_direction(ai, min, max, vertical, direction, restricted)
}

/// `LandRandomPos.getPosInDirection`.
fn land_in_direction(
    ai: &mut Ai<'_, '_>,
    min: f64,
    max: f64,
    vertical: i32,
    direction: Vec3,
    restricted: bool,
) -> Option<Vec3> {
    generate(ai, |ai| {
        let towards = random_direction_within_radians(
            ai,
            min,
            max,
            vertical,
            0,
            direction[0],
            direction[2],
            HALF_PI,
        )?;
        let pos = land_towards(ai, max, restricted, towards)?;
        land_up_out_of_solid(ai, pos)
    })
}

/// `HoverRandomPos.getPos(mob, h, v, dirX, dirZ, angle, maxAbove, minAbove)`.
#[allow(
    clippy::too_many_arguments,
    reason = "the game's own arguments, each read as it reads them"
)]
pub(crate) fn hover_pos(
    ai: &mut Ai<'_, '_>,
    horizontal: i32,
    vertical: i32,
    dir_x: f64,
    dir_z: f64,
    angle: f32,
    max_above: i32,
    min_above: i32,
) -> Option<Vec3> {
    let restricted = mob_restricted(ai, f64::from(horizontal));
    generate(ai, |ai| {
        let towards = random_direction_within_radians(
            ai,
            0.0,
            f64::from(horizontal),
            vertical,
            0,
            dir_x,
            dir_z,
            f64::from(angle),
        )?;
        let pos = land_towards(ai, f64::from(horizontal), restricted, towards)?;
        let above = ai.tick.draws.next_int((max_above - min_above + 1) as u32) as i32 + min_above;
        let top = max_y(ai);
        let up = move_up_to_above_solid(pos, above, top, |at| is_solid(ai, at));
        (!is_water(ai, up) && !has_malus(ai, up)).then_some(up)
    })
}

/// `AirAndWaterRandomPos.generateRandomPos`.
#[allow(
    clippy::too_many_arguments,
    reason = "the game's own arguments, each read as it reads them"
)]
fn air_and_water(
    ai: &mut Ai<'_, '_>,
    horizontal: i32,
    vertical: i32,
    y_offset: i32,
    dir_x: f64,
    dir_z: f64,
    angle: f64,
    restricted: bool,
) -> Option<BlockPos> {
    let towards = random_direction_within_radians(
        ai,
        0.0,
        f64::from(horizontal),
        vertical,
        y_offset,
        dir_x,
        dir_z,
        angle,
    )?;
    let pos = random_pos_toward_direction(ai, f64::from(horizontal), towards);
    if outside_limits(ai, pos) || restricted_out(ai, restricted, pos) {
        return None;
    }
    let top = max_y(ai);
    let up = move_up_out_of_solid(pos, top, |at| is_solid(ai, at));
    (!has_malus(ai, up)).then_some(up)
}

/// `AirAndWaterRandomPos.getPos(mob, h, v, yOffset, dirX, dirZ, angle)`.
#[allow(
    clippy::too_many_arguments,
    reason = "the game's own arguments, each read as it reads them"
)]
pub(crate) fn air_and_water_pos(
    ai: &mut Ai<'_, '_>,
    horizontal: i32,
    vertical: i32,
    y_offset: i32,
    dir_x: f64,
    dir_z: f64,
    angle: f64,
) -> Option<Vec3> {
    let restricted = mob_restricted(ai, f64::from(horizontal));
    generate(ai, |ai| {
        air_and_water(
            ai, horizontal, vertical, y_offset, dir_x, dir_z, angle, restricted,
        )
    })
}

/// `AirRandomPos.getPosTowards(mob, h, v, yOffset, target, angle)`: as the air and
/// water one, but never in water.
pub(crate) fn air_pos_towards(
    ai: &mut Ai<'_, '_>,
    horizontal: i32,
    vertical: i32,
    y_offset: i32,
    target: Vec3,
    angle: f64,
) -> Option<Vec3> {
    let direction = physics::sub(target, ai.position());
    let restricted = mob_restricted(ai, f64::from(horizontal));
    generate(ai, |ai| {
        let pos = air_and_water(
            ai,
            horizontal,
            vertical,
            y_offset,
            direction[0],
            direction[2],
            angle,
            restricted,
        )?;
        (!is_water(ai, pos)).then_some(pos)
    })
}

/// `BehaviorUtils.getRandomSwimmablePos(mob, h, v)`: a default position, drawn again up
/// to ten times while it is not somewhere one may swim.
pub(crate) fn swimmable_pos(ai: &mut Ai<'_, '_>, horizontal: i32, vertical: i32) -> Option<Vec3> {
    let mut pos = default_pos(ai, horizontal, vertical);
    let mut tries = 0;
    while let Some(at) = pos {
        let state = ai.tick.world.state(physics::containing(at));
        if state.is_pathfindable(PathComputation::Water) {
            break;
        }
        let retry = tries < 10;
        tries += 1;
        if !retry {
            break;
        }
        pos = default_pos(ai, horizontal, vertical);
    }
    pos
}
