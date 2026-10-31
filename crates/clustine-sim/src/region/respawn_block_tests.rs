//! The respawn block's check and the place a player stands up at, each case worked out by
//! hand from the bytecode (`ServerPlayer.findRespawnAndUseSpawnBlock`,
//! `AbstractBedBlock.findStandUpPosition`, `RespawnAnchorBlock.findStandUpPosition`,
//! `DismountHelper.findSafeDismountLocation`; `docs/groundwork/p-respawn-facts.md`).

use std::collections::BTreeMap;

use clustine_blocks::data::set;
use clustine_data::{BlockState, blocks};
use clustine_world::{BlockPos, ChunkPos, Dimension};

use super::{RespawnBlock, respawn_block};
use crate::api::Respawn;
use crate::physics::World;

/// Stone below `floor`, air above it, and the blocks set over both.
struct Land {
    floor: i32,
    blocks: BTreeMap<BlockPos, BlockState>,
}

impl Land {
    fn new(floor: i32) -> Self {
        Self {
            floor,
            blocks: BTreeMap::new(),
        }
    }

    fn put(&mut self, x: i32, y: i32, z: i32, state: BlockState) {
        self.blocks.insert(BlockPos::new(x, y, z), state);
    }

    /// A red bed whose head is at `head`, facing `facing` (its foot one step behind).
    fn bed(&mut self, head: BlockPos, facing: &str) {
        let (dx, dz) = step(facing);
        let state = set(blocks::RED_BED, "facing", facing);
        self.put(head.x, head.y, head.z, set(state, "part", "head"));
        self.put(head.x - dx, head.y, head.z - dz, set(state, "part", "foot"));
    }
}

impl World for Land {
    fn block(&self, pos: BlockPos) -> Option<BlockState> {
        Some(
            self.blocks
                .get(&pos)
                .copied()
                .unwrap_or(if pos.y < self.floor {
                    blocks::STONE
                } else {
                    blocks::AIR
                }),
        )
    }

    fn has_chunk(&self, _: ChunkPos) -> bool {
        true
    }
}

fn step(facing: &str) -> (i32, i32) {
    match facing {
        "north" => (0, -1),
        "south" => (0, 1),
        "west" => (-1, 0),
        "east" => (1, 0),
        _ => unreachable!("a horizontal facing"),
    }
}

/// The yaw that looks along a horizontal step: south 0, west 90, north 180, east -90.
fn yaw_along((dx, dz): (i32, i32)) -> f32 {
    match (dx, dz) {
        (0, 1) => 0.0,
        (-1, 0) => 90.0,
        (0, -1) => 180.0,
        (1, 0) => -90.0,
        _ => unreachable!("a horizontal step"),
    }
}

fn respawn_at(pos: BlockPos, yaw: f32, dimension: Dimension) -> Respawn {
    Respawn {
        pos,
        yaw,
        pitch: 0.0,
        dimension,
    }
}

const HEAD: BlockPos = BlockPos::new(0, 64, 0);

/// The place and yaw of a placed respawn; a panic for a missing one.
fn placed(answer: RespawnBlock) -> ([f64; 3], f32, Option<(BlockPos, BlockState)>) {
    match answer {
        RespawnBlock::Placed {
            position,
            yaw,
            pitch,
            spent,
        } => {
            assert_eq!(pitch, 0.0, "a respawn at a block looks level");
            (position, yaw, spent)
        }
        RespawnBlock::Missing => panic!("the respawn found no place"),
    }
}

/// Whether two yaws are the same angle to within the table's error (180 and -180 alike).
fn same_angle(a: f32, b: f32) -> bool {
    let difference = (a - b).rem_euclid(360.0);
    difference < 1.0e-3 || difference > 360.0 - 1.0e-3
}

fn bed_respawn(land: &Land, yaw: f32) -> RespawnBlock {
    respawn_block(
        land,
        Dimension::Overworld,
        &respawn_at(HEAD, yaw, Dimension::Overworld),
        true,
    )
}

#[test]
fn a_bed_on_open_ground_stands_the_player_on_its_clockwise_side_facing_it() {
    // f the facing, s = f.getClockWise(); a stored yaw that looks along -s does not face
    // s (-sin·s.x + cos·s.z = -1), so s is kept and the first candidate is head + s,
    // whose floor is the stone's top.
    for (facing, side) in [
        ("north", (1, 0)),
        ("east", (0, 1)),
        ("south", (-1, 0)),
        ("west", (0, -1)),
    ] {
        let mut land = Land::new(64);
        land.bed(HEAD, facing);
        let away = (-side.0, -side.1);
        let (position, yaw, spent) = placed(bed_respawn(&land, yaw_along(away)));
        assert_eq!(
            position,
            [f64::from(side.0) + 0.5, 64.0, f64::from(side.1) + 0.5],
            "a bed facing {facing}"
        );
        assert!(
            same_angle(yaw, yaw_along(away)),
            "facing the bed: {yaw} for a bed facing {facing}"
        );
        assert_eq!(spent, None, "a bed is never used up");
    }
}

#[test]
fn a_stored_yaw_that_faces_the_clockwise_side_turns_the_player_out_on_the_other() {
    for (facing, side) in [
        ("north", (1, 0)),
        ("east", (0, 1)),
        ("south", (-1, 0)),
        ("west", (0, -1)),
    ] {
        let mut land = Land::new(64);
        land.bed(HEAD, facing);
        let (position, yaw, _) = placed(bed_respawn(&land, yaw_along(side)));
        assert_eq!(
            position,
            [f64::from(-side.0) + 0.5, 64.0, f64::from(-side.1) + 0.5],
            "a bed facing {facing}"
        );
        assert!(same_angle(yaw, yaw_along(side)), "facing the bed: {yaw}");
    }
}

#[test]
fn walls_on_the_first_candidates_move_the_player_along_the_ring_in_its_order() {
    // Facing north, s east: s (1,0), then s-f (1,1), then s-2f (1,2). A stone block's
    // floor is its top, 1, which is not below 1.
    let mut land = Land::new(64);
    land.bed(HEAD, "north");
    land.put(1, 64, 0, blocks::STONE);
    let (position, yaw, _) = placed(bed_respawn(&land, 0.0));
    assert_eq!(position, [1.5, 64.0, 1.5]);
    // From (1.5, 1.5) the head's bottom centre lies at (-1, -1): atan2 gives -135
    // degrees, less 90 is -225, wrapped to 135.
    assert!(same_angle(yaw, 135.0), "the yaw faces the bed: {yaw}");

    land.put(1, 64, 1, blocks::STONE);
    let (position, _, _) = placed(bed_respawn(&land, 0.0));
    assert_eq!(position, [1.5, 64.0, 2.5]);
}

#[test]
fn a_block_over_the_candidate_that_the_box_meets_refuses_it() {
    // The box is 1.8 high: a stone at y 65 above the first candidate is met.
    let mut land = Land::new(64);
    land.bed(HEAD, "north");
    land.put(1, 65, 0, blocks::STONE);
    let (position, _, _) = placed(bed_respawn(&land, 0.0));
    assert_eq!(position, [1.5, 64.0, 1.5]);
}

/// A north-facing bed whose every candidate is refused but `open` at y 64: every other
/// ring place is stone, and stone lies above the bed's two blocks so that nobody stands
/// on it.
fn enclosed_bed(open: (i32, i32)) -> Land {
    let mut land = Land::new(64);
    land.bed(HEAD, "north");
    for (x, z) in [
        (1, 0),
        (1, 1),
        (1, 2),
        (0, 2),
        (-1, 2),
        (-1, 1),
        (-1, 0),
        (-1, -1),
        (0, -1),
        (1, -1),
    ] {
        if (x, z) != open {
            land.put(x, 64, z, blocks::STONE);
        }
    }
    land.put(0, 65, 0, blocks::STONE);
    land.put(0, 65, 1, blocks::STONE);
    land
}

#[test]
fn lava_is_refused_while_dangers_are_checked_and_taken_when_nothing_else_is_free() {
    // -s, (-1, 0), holds lava over stone: refused by the first pass (dangerous at the
    // place), nothing else stands, and the second pass takes it on the stone below.
    let mut land = enclosed_bed((-1, 0));
    land.put(-1, 64, 0, blocks::LAVA);
    let (position, yaw, _) = placed(bed_respawn(&land, 0.0));
    assert_eq!(position, [-0.5, 64.0, 0.5]);
    assert!(same_angle(yaw, -90.0), "facing east, to the bed: {yaw}");
}

#[test]
fn lava_early_in_the_order_gives_way_to_a_free_place_later_in_the_first_pass() {
    // As above, but the foot's top is clear: the first pass reaches "above", whose first
    // offset (on the head) is still covered and whose second (on the foot, -f) stands on
    // the bed's top, 0.5625.
    let mut land = enclosed_bed((-1, 0));
    land.put(-1, 64, 0, blocks::LAVA);
    land.blocks.remove(&BlockPos::new(0, 65, 1));
    let (position, _, _) = placed(bed_respawn(&land, 0.0));
    assert_eq!(position, [0.5, 64.5625, 1.5]);
}

#[test]
fn magma_below_a_floor_at_the_block_bottom_is_refused_while_dangers_are_checked() {
    // The first candidate, (1, 64, 0), stands on a magma block (floor 0, dangerous
    // below); the next, (1, 64, 1), on stone.
    let mut land = Land::new(64);
    land.bed(HEAD, "north");
    land.put(1, 63, 0, blocks::MAGMA_BLOCK);
    let (position, _, _) = placed(bed_respawn(&land, 0.0));
    assert_eq!(position, [1.5, 64.0, 1.5]);
}

#[test]
fn a_bunk_bed_tries_the_ring_a_level_lower_before_lying_on_top() {
    // Stone below 63: the ring at 64 has air under it (no floor), the head's top would
    // do for an ordinary bed (0.5625), but a bunk bed tries the ring at 63 first.
    let mut land = Land::new(63);
    land.bed(HEAD, "north");
    land.bed(BlockPos::new(0, 63, 0), "north");
    let (position, _, _) = placed(bed_respawn(&land, 0.0));
    assert_eq!(position, [1.5, 63.0, 0.5]);

    // Without the lower bed the same place is not a bunk: the ring at 64 has no floor,
    // so the player stands on the head.
    let mut land = Land::new(63);
    land.bed(HEAD, "north");
    land.put(0, 63, 0, blocks::STONE);
    land.put(0, 63, 1, blocks::STONE);
    let (position, yaw, _) = placed(bed_respawn(&land, 0.0));
    assert_eq!(position, [0.5, 64.5625, 0.5]);
    // Straight above the bottom centre: d is (0, -1, 0), atan2(0, 0) is 0, less 90.
    assert!(same_angle(yaw, -90.0), "{yaw}");
}

#[test]
fn a_bed_whose_dimension_does_not_let_it_set_the_spawn_is_missing() {
    for dimension in [Dimension::Nether, Dimension::End] {
        let mut land = Land::new(64);
        land.bed(HEAD, "north");
        let answer = respawn_block(&land, dimension, &respawn_at(HEAD, 0.0, dimension), false);
        assert_eq!(answer, RespawnBlock::Missing, "{dimension:?}");
    }
    let mut land = Land::new(64);
    land.put(0, 64, 0, set(blocks::STRAW_BED, "part", "head"));
    land.put(0, 64, 1, set(blocks::STRAW_BED, "part", "foot"));
    assert_eq!(
        bed_respawn(&land, 0.0),
        RespawnBlock::Missing,
        "a straw bed"
    );
}

#[test]
fn a_bed_counts_by_any_of_its_parts() {
    // The stored position on the foot: the foot's own facing (north) is read there.
    let mut land = Land::new(64);
    land.bed(BlockPos::new(0, 64, -1), "north");
    let (position, _, _) = placed(bed_respawn(&land, 0.0));
    assert_eq!(position, [1.5, 64.0, 0.5]);
}

#[test]
fn no_bed_or_anchor_at_the_respawn_is_missing() {
    let land = Land::new(64);
    assert_eq!(bed_respawn(&land, 0.0), RespawnBlock::Missing, "air");
    let mut land = Land::new(64);
    land.put(0, 64, 0, blocks::STONE);
    assert_eq!(bed_respawn(&land, 0.0), RespawnBlock::Missing, "stone");
}

fn anchor(charges: u8) -> BlockState {
    set(blocks::RESPAWN_ANCHOR, "charges", &charges.to_string())
}

fn anchor_respawn(land: &Land, dimension: Dimension, use_spawn_block: bool) -> RespawnBlock {
    respawn_block(
        land,
        dimension,
        &respawn_at(HEAD, 0.0, dimension),
        use_spawn_block,
    )
}

#[test]
fn a_charged_anchor_spends_a_charge_only_when_the_respawn_uses_its_block() {
    let mut land = Land::new(64);
    land.put(0, 64, 0, anchor(3));
    // The first offset is (0, 0, -1): on the stone, north of the anchor, facing south.
    let (position, yaw, spent) = placed(anchor_respawn(&land, Dimension::Nether, true));
    assert_eq!(position, [0.5, 64.0, -0.5]);
    assert!(same_angle(yaw, 0.0), "{yaw}");
    assert_eq!(spent, Some((HEAD, anchor(2))));
    let (position, _, spent) = placed(anchor_respawn(&land, Dimension::Nether, false));
    assert_eq!(position, [0.5, 64.0, -0.5]);
    assert_eq!(spent, None);
}

#[test]
fn an_uncharged_anchor_or_one_where_anchors_do_not_work_is_missing() {
    let mut land = Land::new(64);
    land.put(0, 64, 0, anchor(0));
    assert_eq!(
        anchor_respawn(&land, Dimension::Nether, true),
        RespawnBlock::Missing
    );
    land.put(0, 64, 0, anchor(4));
    for dimension in [Dimension::Overworld, Dimension::End] {
        assert_eq!(
            anchor_respawn(&land, dimension, true),
            RespawnBlock::Missing,
            "{dimension:?}"
        );
    }
}

#[test]
fn a_charged_anchor_with_nowhere_to_stand_is_missing_and_keeps_its_charge() {
    // Stone everywhere up to 70: every candidate's floor is a full block's top.
    let mut land = Land::new(70);
    land.put(0, 64, 0, anchor(1));
    assert_eq!(
        anchor_respawn(&land, Dimension::Nether, true),
        RespawnBlock::Missing
    );
}

#[test]
fn the_anchor_tries_the_ring_below_before_the_ring_above() {
    // Air everywhere but the anchor: (0, 64, -1) is stone, so its own place is refused
    // (floor 1) and the place above it, (0, 65, -1), stands on it; (-1, 62, 0) is
    // stone, so (-1, 63, 0), the second offset a block lower, stands on it. Below
    // comes first.
    let mut land = Land::new(-64);
    land.put(0, 64, 0, anchor(1));
    land.put(0, 64, -1, blocks::STONE);
    land.put(-1, 62, 0, blocks::STONE);
    let (position, _, _) = placed(anchor_respawn(&land, Dimension::Nether, false));
    assert_eq!(position, [-0.5, 63.0, 0.5]);

    land.blocks.remove(&BlockPos::new(-1, 62, 0));
    let (position, yaw, _) = placed(anchor_respawn(&land, Dimension::Nether, false));
    assert_eq!(position, [0.5, 65.0, -0.5]);
    // Looking south and down at the anchor's bottom centre: the yaw has no pitch in it.
    assert!(same_angle(yaw, 0.0), "{yaw}");
}

#[test]
fn an_anchor_in_the_air_is_stood_on_last() {
    let mut land = Land::new(-64);
    land.put(0, 64, 0, anchor(1));
    let (position, yaw, _) = placed(anchor_respawn(&land, Dimension::Nether, true));
    assert_eq!(position, [0.5, 65.0, 0.5]);
    assert!(same_angle(yaw, -90.0), "{yaw}");
}

#[test]
fn a_place_in_or_under_a_block_of_invalid_spawn_inside_is_refused() {
    // An end portal has no collision, so only the tag refuses it.
    assert!(blocks::END_PORTAL.voxel_collision_shape().is_empty());
    let mut land = Land::new(64);
    land.bed(HEAD, "north");
    land.put(1, 64, 0, blocks::END_PORTAL);
    let (position, _, _) = placed(bed_respawn(&land, 0.0));
    assert_eq!(position, [1.5, 64.0, 1.5], "in the portal");

    let mut land = Land::new(64);
    land.bed(HEAD, "north");
    land.put(1, 65, 0, blocks::END_PORTAL);
    let (position, _, _) = placed(bed_respawn(&land, 0.0));
    assert_eq!(position, [1.5, 64.0, 1.5], "under the portal");
}
