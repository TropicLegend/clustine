//! `end_podium`: the exit portal's podium, which the dragon fight places at the fight's
//! podium location, inactive when the fight begins and active once the dragon is dead
//! (`EnderDragonFight.spawnExitPortal`, d-javap's EnderDragonFight.txt, 1164-1251). No
//! biome places it, so the features' reader leaves it unbuilt and the fight
//! (`clustine-sim`'s `region/end_fight.rs`) sets these blocks itself.
//!
//! Read from d-javap's `net.minecraft.world.level.levelgen.feature.EndPodiumFeature.txt`;
//! the offsets named below are of its `place`. The feature reads neither the chunk
//! generator nor the random source it is given (`place` never loads its second or third
//! argument), so the `RandomSource.create()` the fight passes draws nothing.
//!
//! How the fight sets them: `Feature.setBlock`, which `place` calls for every plain
//! write, is not in any dump; it is the game's `level.setBlock(pos, state, 3)`, so
//! flags 3 (`UPDATE_ALL`: neighbours told and clients sent the change). The level is
//! the `ServerLevel` itself, as the fight passes `this.level`. Where the podium is
//! active, the end stone, the air and the portal go through `dropPreviousAndSetBlock`
//! (offsets 0-34 of that method): a block that is already the one wanted is left as it
//! is; any other is first broken with its drops (`destroyBlock(pos, true, null)`) and
//! then set with flags 3. [`PodiumWrite::drop_previous`] says which writes those are.

use clustine_data::{BlockState, blocks};
use clustine_world::BlockPos;

/// One block the podium's `place` sets, in the order it sets them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PodiumWrite {
    pub pos: BlockPos,
    pub state: BlockState,
    /// Whether the write is `dropPreviousAndSetBlock`'s: skipped where the block there
    /// is already `state`'s block, and otherwise preceded by breaking the block there
    /// with its drops. Only an active podium's end stone, air and portal are.
    pub drop_previous: bool,
}

/// The horizontal directions in `Direction.Plane.HORIZONTAL`'s order (n6-javap's
/// `Direction$Plane`, its static initialiser, offsets 6-31): north, east, south, west,
/// with their steps in x and z.
const HORIZONTAL: [(&str, i32, i32); 4] = [
    ("north", 0, -1),
    ("east", 1, 0),
    ("south", 0, 1),
    ("west", -1, 0),
];

/// Every write `EndPodiumFeature.place` makes for a podium at `origin`, in its order,
/// with whether each drops what was there before (see the module's comment).
///
/// The box from four across and one down to four across and 32 up is walked in
/// `BlockPos.betweenClosed`'s order (offsets 0-67: x fastest, then y, then z); only
/// blocks closer than 3.5 to the origin are written, so nothing above three up is
/// touched. Below the origin, those closer than 2.5 are bedrock and the rest end stone
/// (offsets 119-199); above it, all are air (202-248); on the origin's layer, the rim is
/// bedrock and the inside is the portal where active and air where not (251-302). Then
/// the pillar of bedrock, four high from the origin (308-338), and a wall torch on each
/// side of the pillar two above the origin, facing away from it (341-408).
pub fn podium_writes(origin: BlockPos, active: bool) -> Vec<PodiumWrite> {
    let mut writes = Vec::new();
    let mut set = |pos: BlockPos, state: BlockState, drop_previous: bool| {
        writes.push(PodiumWrite {
            pos,
            state,
            drop_previous,
        });
    };
    for dz in -4..=4 {
        for dy in -1..=32 {
            for dx in -4..=4 {
                let pos = origin.offset(dx, dy, dz);
                // `Vec3i.closerThan`: the squared distance between the two blocks'
                // corners, in doubles, below the square of the radius (n7-javap's
                // Vec3i, `closerThan` and `distSqr`).
                let distance =
                    f64::from(dx).powi(2) + f64::from(dy).powi(2) + f64::from(dz).powi(2);
                let inner = distance < 2.5 * 2.5;
                if !inner && distance >= 3.5 * 3.5 {
                    continue;
                }
                if dy < 0 {
                    if inner {
                        set(pos, blocks::BEDROCK, false);
                    } else {
                        set(pos, blocks::END_STONE, active);
                    }
                } else if dy > 0 {
                    set(pos, blocks::AIR, active);
                } else if !inner {
                    set(pos, blocks::BEDROCK, false);
                } else if active {
                    set(pos, blocks::END_PORTAL, true);
                } else {
                    set(pos, blocks::AIR, false);
                }
            }
        }
    }
    for up in 0..4 {
        set(origin.offset(0, up, 0), blocks::BEDROCK, false);
    }
    let torches = origin.offset(0, 2, 0);
    for (facing, dx, dz) in HORIZONTAL {
        let torch = blocks::WALL_TORCH
            .with_property("facing", facing)
            .expect("a wall torch faces every horizontal direction");
        set(torches.offset(dx, 0, dz), torch, false);
    }
    writes
}

/// Every block `EndPodiumFeature.place` sets for a podium at `origin`, in the order it
/// sets them, with their states exact; [`podium_writes`] says which of an active
/// podium's are set only where the block differs.
pub fn podium_blocks(origin: BlockPos, active: bool) -> Vec<(BlockPos, BlockState)> {
    podium_writes(origin, active)
        .into_iter()
        .map(|write| (write.pos, write.state))
        .collect()
}

// No `egg_position`: `EnderDragonFight.setDragonKilled` (offsets 47-73) sets the egg
// at `getHeightmapPos(MOTION_BLOCKING, getPodiumLocation(origin))`, the fight's own
// location, not anything this feature defines.

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    const ORIGIN: BlockPos = BlockPos::new(0, 64, 0);

    fn torch(facing: &str) -> BlockState {
        blocks::WALL_TORCH
            .with_property("facing", facing)
            .expect("a facing")
    }

    /// What stands after every write, as the last write to each place.
    fn result(active: bool) -> BTreeMap<(i32, i32, i32), BlockState> {
        podium_blocks(ORIGIN, active)
            .into_iter()
            .map(|(pos, state)| ((pos.x, pos.y, pos.z), state))
            .collect()
    }

    /// The writes on one layer, counted by state.
    fn count(active: bool, y: i32, state: BlockState) -> usize {
        podium_blocks(ORIGIN, active)
            .iter()
            .take(145)
            .filter(|(pos, written)| pos.y == y && *written == state)
            .count()
    }

    #[test]
    fn the_sphere_writes_145_blocks_then_the_pillar_and_the_torches() {
        // Worked out by hand: squared distances below 12.25 per layer are 37 on the
        // three layers within one of the origin, 25 two up and 9 three up.
        for active in [false, true] {
            let writes = podium_blocks(ORIGIN, active);
            assert_eq!(writes.len(), 145 + 4 + 4);
            assert!(writes.iter().all(|(pos, _)| (63..=67).contains(&pos.y)
                && pos.x.abs() <= 3
                && pos.z.abs() <= 3));
        }
    }

    #[test]
    fn the_first_writes_follow_between_closed_order() {
        let writes = podium_blocks(ORIGIN, false);
        // z = -3 first: only the layer of the origin and one above and below reach
        // it, at x = -1, 0, 1 (1 + 9 < 12.25; 4 + 9 does not), x fastest.
        let first: Vec<_> = writes[..9]
            .iter()
            .map(|(pos, state)| (pos.x, pos.y, pos.z, *state))
            .collect();
        assert_eq!(
            first,
            vec![
                (-1, 63, -3, blocks::END_STONE),
                (0, 63, -3, blocks::END_STONE),
                (1, 63, -3, blocks::END_STONE),
                (-1, 64, -3, blocks::BEDROCK),
                (0, 64, -3, blocks::BEDROCK),
                (1, 64, -3, blocks::BEDROCK),
                (-1, 65, -3, blocks::AIR),
                (0, 65, -3, blocks::AIR),
                (1, 65, -3, blocks::AIR),
            ]
        );
    }

    #[test]
    fn the_layers_hold_bedrock_inside_and_end_stone_round_below_the_origin() {
        for active in [false, true] {
            // Below: squared horizontal distance up to 5 inside, 8 to 10 round it.
            assert_eq!(count(active, 63, blocks::BEDROCK), 21);
            assert_eq!(count(active, 63, blocks::END_STONE), 16);
            // On the origin's layer: the rim of 16 is bedrock.
            assert_eq!(count(active, 64, blocks::BEDROCK), 16);
            // Above: air only.
            assert_eq!(count(active, 65, blocks::AIR), 37);
            assert_eq!(count(active, 66, blocks::AIR), 25);
            assert_eq!(count(active, 67, blocks::AIR), 9);
        }
        assert_eq!(count(false, 64, blocks::AIR), 21);
        assert_eq!(count(false, 64, blocks::END_PORTAL), 0);
        assert_eq!(count(true, 64, blocks::END_PORTAL), 21);
        assert_eq!(count(true, 64, blocks::AIR), 0);
    }

    #[test]
    fn an_inactive_podium_stands_as_worked_out_by_hand() {
        let world = result(false);
        // The centre column: bedrock under the origin, the pillar four high from it,
        // and nothing written four up.
        assert_eq!(world[&(0, 63, 0)], blocks::BEDROCK);
        for y in 64..=67 {
            assert_eq!(world[&(0, y, 0)], blocks::BEDROCK, "{y}");
        }
        assert!(!world.contains_key(&(0, 68, 0)));
        // A ring on the origin's layer: two out is inside (air), three out is the rim
        // (bedrock), and a corner at (2, 2) is the rim too (8 > 6.25).
        assert_eq!(world[&(2, 64, 0)], blocks::AIR);
        assert_eq!(world[&(2, 64, 1)], blocks::AIR);
        assert_eq!(world[&(3, 64, 0)], blocks::BEDROCK);
        assert_eq!(world[&(-3, 64, 1)], blocks::BEDROCK);
        assert_eq!(world[&(2, 64, -2)], blocks::BEDROCK);
        assert!(!world.contains_key(&(3, 64, 2)));
        // Below: (2, 1) inside, (3, 1) end stone, (3, 2) out of reach (1 + 13).
        assert_eq!(world[&(2, 63, 1)], blocks::BEDROCK);
        assert_eq!(world[&(3, 63, 1)], blocks::END_STONE);
        assert_eq!(world[&(2, 63, 2)], blocks::END_STONE);
        assert!(!world.contains_key(&(3, 63, 2)));
        // Nothing two below the origin.
        assert!(world.keys().all(|&(_, y, _)| y >= 63));
        // The torches, two above the origin, on the pillar's four sides facing out.
        assert_eq!(world[&(0, 66, -1)], torch("north"));
        assert_eq!(world[&(1, 66, 0)], torch("east"));
        assert_eq!(world[&(0, 66, 1)], torch("south"));
        assert_eq!(world[&(-1, 66, 0)], torch("west"));
        // Three up, only the plus round the pillar and its corners are cleared.
        assert_eq!(world[&(1, 67, 1)], blocks::AIR);
        assert!(!world.contains_key(&(2, 67, 0)));
    }

    #[test]
    fn an_active_podium_holds_the_portal_round_the_pillar() {
        let world = result(true);
        for (x, z) in [(1, 0), (-1, 0), (0, 2), (2, 1), (-1, -2), (1, 1)] {
            assert_eq!(world[&(x, 64, z)], blocks::END_PORTAL, "{x} {z}");
        }
        // The origin's portal is overwritten by the pillar.
        assert_eq!(world[&(0, 64, 0)], blocks::BEDROCK);
        assert_eq!(world[&(3, 64, 0)], blocks::BEDROCK);
        assert_eq!(world[&(2, 64, 2)], blocks::BEDROCK);
        assert_eq!(world[&(3, 63, 0)], blocks::END_STONE);
        assert_eq!(world[&(0, 66, -1)], torch("north"));
        assert_eq!(world[&(-1, 66, 0)], torch("west"));
    }

    #[test]
    fn the_last_eight_writes_are_the_pillar_then_the_torches_in_the_plane_s_order() {
        for active in [false, true] {
            let writes = podium_writes(ORIGIN, active);
            let tail: Vec<_> = writes[145..]
                .iter()
                .map(|write| (write.pos, write.state, write.drop_previous))
                .collect();
            assert_eq!(
                tail,
                vec![
                    (BlockPos::new(0, 64, 0), blocks::BEDROCK, false),
                    (BlockPos::new(0, 65, 0), blocks::BEDROCK, false),
                    (BlockPos::new(0, 66, 0), blocks::BEDROCK, false),
                    (BlockPos::new(0, 67, 0), blocks::BEDROCK, false),
                    (BlockPos::new(0, 66, -1), torch("north"), false),
                    (BlockPos::new(1, 66, 0), torch("east"), false),
                    (BlockPos::new(0, 66, 1), torch("south"), false),
                    (BlockPos::new(-1, 66, 0), torch("west"), false),
                ]
            );
        }
    }

    #[test]
    fn only_an_active_podium_s_end_stone_air_and_portal_drop_what_was_there() {
        assert!(
            podium_writes(ORIGIN, false)
                .iter()
                .all(|write| !write.drop_previous)
        );
        for write in podium_writes(ORIGIN, true) {
            let expected =
                [blocks::END_STONE, blocks::AIR, blocks::END_PORTAL].contains(&write.state);
            assert_eq!(write.drop_previous, expected, "{write:?}");
        }
    }

    #[test]
    fn a_podium_elsewhere_is_the_same_shape_moved() {
        let at = BlockPos::new(-37, 70, 12);
        let moved: Vec<_> = podium_blocks(at, true)
            .into_iter()
            .map(|(pos, state)| (BlockPos::new(pos.x + 37, pos.y - 6, pos.z - 12), state))
            .collect();
        assert_eq!(moved, podium_blocks(ORIGIN, true));
    }
}
