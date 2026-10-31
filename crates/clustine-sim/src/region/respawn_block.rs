//! The check of a player's respawn block and the place they stand up at
//! (`ServerPlayer.findRespawnAndUseSpawnBlock`, lines 2286–2432 of d ServerPlayer.txt,
//! and what it calls; `docs/groundwork/p-respawn-facts.md`, sections 1 to 3; `p-plan.md`,
//! 19.7), as a
//! function of the blocks a region reads through [`World`] and nothing else, so that the
//! region that places a respawned player can call it on its own chunk.
//!
//! [`World`] is the physics' read of blocks: the stand-up search needs block states, the
//! game's collision shapes of the blocks a player's box meets (`getBlockCollisions`) and
//! nothing more, which is exactly what [`block_collisions`] reads through it.
//!
//! Named differences and what is left out:
//! - **Forced respawns** (lines 2384–2432) are left out: Clustine's respawn has no `forced`,
//!   and an unforced one gives nothing for a block that is neither an anchor nor a bed.
//! - **The world border** (`WorldBorder.isWithinBounds`, offsets 198–213 of
//!   `findSafeDismountLocation`) is not checked: Clustine has none, and the game's
//!   default lies thirty million blocks out.
//! - **A bed's rule that sets the spawn only when dark** (`BedRule.Rule.WHEN_DARK`) is
//!   taken as never: whether the level is dark is not an input here, and no dimension's
//!   rule has it (clustine-data's `dimension_types.rs`: the overworld's beds always set
//!   it, its straw beds and the Nether's and the End's beds never do).
//! - The rules are the dimension's (`DimensionType`), where the game reads environment
//!   attributes at the block (`EnvironmentAttributeSystem.getValue(attribute, pos)`),
//!   as `sleep.rs` and `anchors.rs` already take them (**Inferred**: that no biome
//!   changes the two attributes was not read from the dumps).
//!
//! Documentation comments cite lines of the dump files; bare numbers in the comments
//! inside a function are bytecode offsets of the game's method its documentation names.

use clustine_blocks::data::{facing, flag, in_tag, is_block, is_class, number, set};
use clustine_data::{BedRule, BedWhen, BlockClass, BlockState, Face, blocks};
use clustine_world::{BlockPos, Dimension};

use crate::api::Respawn;
use crate::mobs::ai::maths;
use crate::physics::aabb::Aabb;
use crate::physics::collide::{Context, block_collisions};
use crate::physics::{self, World};
use crate::region::riding::{block_floor_height, is_floor_valid, non_climbable};

/// What a respawn's block gives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RespawnBlock {
    /// The player stands up at `position` (their feet), facing the block (`yaw`, pitch 0);
    /// `spent` is the anchor's state after one charge is spent, where one is (an anchor,
    /// `use_spawn_block` true): the region sets it with `UPDATE_ALL`
    /// (`setBlockAndUpdate`, offsets 111–144 of `findRespawnAndUseSpawnBlock`).
    Placed {
        position: [f64; 3],
        yaw: f32,
        pitch: f32,
        spent: Option<(BlockPos, BlockState)>,
    },
    /// No bed or anchor that can be used: the world spawn, with the event
    /// `NO_RESPAWN_BLOCK_AVAILABLE` owed (`TeleportTransition.missingRespawnBlock`).
    Missing,
}

/// The width and height of a player's standing box (`EntityType.PLAYER`'s dimensions),
/// which `findSafeDismountLocation` makes the box from (its offsets 88–97).
const PLAYER_SIZE: (f32, f32) = (0.6, 1.8);

/// `CollisionContext.empty()`: `getBlockCollisions(null, box)` is asked for no entity
/// (`findSafeDismountLocation`, offsets 99–103), so no block's shape depends on who
/// stands there.
const NO_ENTITY: Context = Context {
    stands_on_lava: false,
    bottom: -f64::MAX,
    fall_distance: 0.0,
    falling_block: false,
    noclip: None,
};

/// `RespawnAnchorBlock.RESPAWN_OFFSETS` (d RespawnAnchorBlock.txt 511–590): the eight
/// horizontal neighbours in the game's order, the same eight a block lower, the same
/// eight a block higher, then straight above. The bootstrap methods of the two streams
/// (`Vec3i::below`, then `Vec3i::above`) put below before above
/// (`p-respawn-facts.md`, 1.3).
const ANCHOR_OFFSETS: [(i32, i32, i32); 25] = {
    const AROUND: [(i32, i32); 8] = [
        (0, -1),
        (-1, 0),
        (0, 1),
        (1, 0),
        (-1, -1),
        (1, -1),
        (-1, 1),
        (1, 1),
    ];
    let mut offsets = [(0, 1, 0); 25];
    let mut i = 0;
    while i < 8 {
        let (x, z) = AROUND[i];
        offsets[i] = (x, 0, z);
        offsets[8 + i] = (x, -1, z);
        offsets[16 + i] = (x, 1, z);
        i += 1;
    }
    offsets
};

/// `ServerPlayer.findRespawnAndUseSpawnBlock(level, config, useSpawnBlock)` of an unforced
/// `respawn` in `dimension`, whose rules say where beds and anchors set the spawn: the
/// place the player stands up at beside the anchor or bed at `respawn.pos`, and, for an
/// anchor where `use_spawn_block`, the anchor with one charge less.
pub(crate) fn respawn_block<W: World + ?Sized>(
    world: &W,
    dimension: Dimension,
    respawn: &Respawn,
    use_spawn_block: bool,
) -> RespawnBlock {
    let pos = respawn.pos;
    let state = world.state(pos);
    let rules = dimension.rules();
    // 44–160: a charged anchor where anchors work. An anchor that passes both checks
    // answers for itself even where nobody can stand beside it (160 returns the empty
    // search); one that fails either falls through to the bed's test (49, 71, 80 to
    // 161), which it cannot pass, and so to nothing.
    if is_class(state, BlockClass::RespawnAnchorBlock) {
        let charge = number(state, "charges").unwrap_or(0);
        if charge > 0 && rules.respawn_anchor_works {
            let Some(position) = anchor_stand_up_position(world, pos) else {
                return RespawnBlock::Missing;
            };
            // 94–147: a charge spent only when the respawn uses its block and a place
            // was found.
            let spent =
                use_spawn_block.then(|| (pos, set(state, "charges", &(charge - 1).to_string())));
            return placed_facing(pos, position, spent);
        }
    }
    // 161–223: any bed (its part is not read) whose rule sets the spawn here, stood up
    // from as its facing at `pos` and the stored yaw say.
    if is_class(state, BlockClass::AbstractBedBlock) && can_set_spawn(bed_rule(dimension, state)) {
        let facing = facing(state).expect("a bed has a facing");
        if let Some(position) = bed_stand_up_position(world, pos, facing, respawn.yaw) {
            return placed_facing(pos, position, None);
        }
    }
    RespawnBlock::Missing
}

/// `AbstractBedBlock.getBedRule(level, pos)` (n7d AbstractBedBlock 53–62): the
/// dimension's rule for a straw bed (`StrawBedBlock`'s attribute) or for any other bed.
fn bed_rule(dimension: Dimension, state: BlockState) -> BedRule {
    let rules = dimension.rules();
    if is_class(state, BlockClass::StrawBedBlock) {
        rules.straw_bed_rule
    } else {
        rules.bed_rule
    }
}

/// `BedRule.canSetSpawn(level)` (n7d BedRule 50–): its rule's test, with "when dark"
/// taken as never (named in the module's comment).
fn can_set_spawn(rule: BedRule) -> bool {
    match rule.can_set_spawn {
        BedWhen::Always => true,
        BedWhen::WhenDark | BedWhen::Never => false,
    }
}

/// `RespawnPosAngle.of(position, block, 0)` (d ServerPlayer.txt 5789–5803 and
/// ServerPlayer$RespawnPosAngle 24–35): standing at `position`, facing `block`, pitch 0.
fn placed_facing(
    block: BlockPos,
    position: [f64; 3],
    spent: Option<(BlockPos, BlockState)>,
) -> RespawnBlock {
    RespawnBlock::Placed {
        position,
        yaw: look_at_yaw(position, block),
        pitch: 0.0,
        spent,
    }
}

/// `RespawnPosAngle.calculateLookAtYaw(position, block)` (ServerPlayer$RespawnPosAngle
/// 37–55): `wrapDegrees(Mth.atan2(d.z, d.x) * 57.2957763671875 - 90)` in double, then
/// narrowed, where `d` is the normalised way from `position` to the block's bottom
/// centre. `Mth.atan2` is the game's table approximation, which the crate has in
/// `physics/minecart.rs` (by way of `mobs::ai::maths::atan2`), not the library's.
pub(crate) fn look_at_yaw(position: [f64; 3], block: BlockPos) -> f32 {
    let bottom_centre = [
        f64::from(block.x) + 0.5,
        f64::from(block.y),
        f64::from(block.z) + 0.5,
    ];
    let d = physics::normalize(physics::sub(bottom_centre, position));
    wrap_degrees(maths::atan2(d[2], d[0]) * 57.295_776_367_187_5 - 90.0) as f32
}

/// `Mth.wrapDegrees(double)` (n5 Mth 493–516): into [-180, 180), the remainder taking the
/// dividend's sign as Java's `drem` does.
fn wrap_degrees(degrees: f64) -> f64 {
    let mut wrapped = degrees % 360.0;
    if wrapped >= 180.0 {
        wrapped -= 360.0;
    }
    if wrapped < -180.0 {
        wrapped += 360.0;
    }
    wrapped
}

/// `RespawnAnchorBlock.findStandUpPosition(PLAYER, level, pos)` (d RespawnAnchorBlock.txt
/// 442–498): the offsets in their order with the danger checks, then again without.
fn anchor_stand_up_position<W: World + ?Sized>(world: &W, anchor: BlockPos) -> Option<[f64; 3]> {
    [true, false].into_iter().find_map(|checked| {
        ANCHOR_OFFSETS.iter().find_map(|&(dx, dy, dz)| {
            let at = BlockPos::new(anchor.x + dx, anchor.y + dy, anchor.z + dz);
            safe_dismount_location(world, at, checked)
        })
    })
}

/// The x and z of one step towards a horizontal face (`Direction.getStepX`, `getStepZ`).
fn step_of(face: Face) -> (i32, i32) {
    match face {
        Face::North => (0, -1),
        Face::South => (0, 1),
        Face::West => (-1, 0),
        _ => (1, 0),
    }
}

/// `Direction.getClockWise()` of a horizontal step: north to east, east to south.
fn clockwise((x, z): (i32, i32)) -> (i32, i32) {
    (-z, x)
}

/// `Direction.isFacingAngle(yRot)` (n6 Direction 1151–1182): `-sin(r) * x + cos(r) * z >
/// 0` in float, with `r = yRot * 0.017453292f` and the game's sine table (`Mth.sin` and
/// `Mth.cos` of the widened `r`, n5 Mth 61–87).
fn is_facing_angle((x, z): (i32, i32), yaw: f32) -> bool {
    let radians = yaw * 0.017_453_292;
    let sin = -maths::table_sin(radians);
    let cos = maths::table_cos(radians);
    x as f32 * sin + z as f32 * cos > 0.0
}

/// `AbstractBedBlock.findStandUpPosition(PLAYER, level, pos, facing, yaw)` (n7d
/// AbstractBedBlock 432–613, the offsets 693–953): the side is the clockwise one unless
/// the stored yaw faces it; a bunk bed (a bed below) tries the ring about it at its
/// level and a level lower, then on it, with the danger checks and then without; any
/// other bed the ring and then on it, with and then without.
fn bed_stand_up_position<W: World + ?Sized>(
    world: &W,
    bed: BlockPos,
    facing: Face,
    yaw: f32,
) -> Option<[f64; 3]> {
    let f = step_of(facing);
    let mut s = clockwise(f);
    if is_facing_angle(s, yaw) {
        s = (-s.0, -s.1);
    }
    // `bedSurroundStandUpOffsets` and `bedAboveStandUpOffsets`.
    let surround = [
        (s.0, s.1),
        (s.0 - f.0, s.1 - f.1),
        (s.0 - 2 * f.0, s.1 - 2 * f.1),
        (-2 * f.0, -2 * f.1),
        (-s.0 - 2 * f.0, -s.1 - 2 * f.1),
        (-s.0 - f.0, -s.1 - f.1),
        (-s.0, -s.1),
        (-s.0 + f.0, -s.1 + f.1),
        (f.0, f.1),
        (s.0 + f.0, s.1 + f.1),
    ];
    let above = [(0, 0), (-f.0, -f.1)];
    // `findStandUpPositionAtOffset(type, level, pos, offsets, checked)`: every offset at
    // the level of `from`.
    let at_offsets = |from: BlockPos, offsets: &[(i32, i32)], checked: bool| {
        offsets.iter().find_map(|&(dx, dz)| {
            let at = BlockPos::new(from.x + dx, from.y, from.z + dz);
            safe_dismount_location(world, at, checked)
        })
    };
    let below = BlockPos::new(bed.x, bed.y - 1, bed.z);
    // `isBunkBed` (n7d AbstractBedBlock 422–430).
    if is_class(world.state(below), BlockClass::AbstractBedBlock) {
        return [true, false].into_iter().find_map(|checked| {
            at_offsets(bed, &surround, checked)
                .or_else(|| at_offsets(below, &surround, checked))
                .or_else(|| at_offsets(bed, &above, checked))
        });
    }
    [true, false].into_iter().find_map(|checked| {
        at_offsets(bed, &surround, checked).or_else(|| at_offsets(bed, &above, checked))
    })
}

/// `EntityType.isBlockDangerous(state)` of the player (n7 EntityType 629–665): a player
/// is immune to `#default_immune_to`, which is empty, and not to fire, so what burns
/// (`NodeEvaluator.isBurningBlock`, n7 NodeEvaluator 210–232: `#fire`, lava, a magma
/// block, a lit campfire, a lava cauldron), a wither rose, a sweet berry bush, a cactus
/// and powder snow.
fn is_dangerous(state: BlockState) -> bool {
    in_tag(state, "minecraft:fire")
        || is_block(state, blocks::LAVA)
        || is_block(state, blocks::MAGMA_BLOCK)
        || (in_tag(state, "minecraft:campfires") && flag(state, "lit"))
        || is_block(state, blocks::LAVA_CAULDRON)
        || is_block(state, blocks::WITHER_ROSE)
        || is_block(state, blocks::SWEET_BERRY_BUSH)
        || is_block(state, blocks::CACTUS)
        || is_block(state, blocks::POWDER_SNOW)
}

/// `DismountHelper.findSafeDismountLocation(PLAYER, level, pos, checked)` (n5
/// DismountHelper 292–390): the player's feet on the floor of `pos` where the floor is
/// within the block, their box there meets no block, neither `pos` nor the block above
/// is in `#invalid_spawn_inside`, and, when `checked`, neither `pos` nor (for a floor no
/// higher than the block's bottom) the block below is dangerous.
fn safe_dismount_location<W: World + ?Sized>(
    world: &W,
    pos: BlockPos,
    checked: bool,
) -> Option<[f64; 3]> {
    let here = world.state(pos);
    let below_pos = BlockPos::new(pos.x, pos.y - 1, pos.z);
    // 0–19.
    if checked && is_dangerous(here) {
        return None;
    }
    // 20–49: `getBlockFloorHeight(nonClimbableShape(pos), () -> nonClimbableShape(below))`.
    let floor = block_floor_height(non_climbable(here), || {
        non_climbable(world.state(below_pos))
    });
    if !is_floor_valid(floor) {
        return None;
    }
    // 50–79.
    if checked && floor <= 0.0 && is_dangerous(world.state(below_pos)) {
        return None;
    }
    // 80–97: `Vec3.upFromBottomCenterOf(pos, floor)` and the type's box there.
    let feet = [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + floor,
        f64::from(pos.z) + 0.5,
    ];
    let (width, height) = PLAYER_SIZE;
    let bounds = Aabb::of_entity(feet, width, height);
    // 99–153: any shape the box meets refuses the place.
    if !block_collisions(world, NO_ENTITY, &bounds).is_empty() {
        return None;
    }
    // 154–197: the player's own check of the blocks it would stand in.
    let above = BlockPos::new(pos.x, pos.y + 1, pos.z);
    let invalid_inside = |at: BlockPos| in_tag(world.state(at), "minecraft:invalid_spawn_inside");
    if invalid_inside(pos) || invalid_inside(above) {
        return None;
    }
    Some(feet)
}

#[cfg(test)]
#[path = "respawn_block_tests.rs"]
mod tests;
