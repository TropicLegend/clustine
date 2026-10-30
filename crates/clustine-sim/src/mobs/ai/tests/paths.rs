//! Paths from the brief's 3.10 to 3.16: the walker's search on chosen layouts.

use clustine_data::{BlockState, PathType, blocks};
use clustine_world::{BlockPos, EntityId};

use super::super::pathfinder::{
    Counts, Evaluator, EvaluatorFlags, FoundPath, Searcher, default_malus, find_path,
    find_path_counted,
};
use super::Plot;
use crate::physics::Vec3;
use crate::region::Random;

/// A cow as a search reads it: 0.9 wide and 1.4 high, stepping 0.6, an animal's
/// maluses (`Animal`'s constructor: no fire, fire beside costs 16).
fn cow(position: Vec3) -> Searcher {
    let mut malus = default_malus();
    malus[PathType::Fire as usize] = -1.0;
    malus[PathType::FireInNeighbor as usize] = 16.0;
    Searcher {
        stands_on_lava: false,
        position,
        width: 0.9,
        height: 1.4,
        on_ground: true,
        in_floatable: false,
        in_water: false,
        fall_distance: 0.0,
        step_height: 0.6,
        max_fall: 3,
        malus,
        flags: EvaluatorFlags::default(),
        evaluator: Evaluator::Walk,
        min_y: -64,
        sea_level: 63,
    }
}

fn search(plot: &Plot, mob: &Searcher, target: BlockPos, reach: i32) -> (FoundPath, Counts) {
    search_limited(plot, mob, target, reach, 256)
}

fn search_limited(
    plot: &Plot,
    mob: &Searcher,
    target: BlockPos,
    reach: i32,
    limit: i32,
) -> (FoundPath, Counts) {
    let mut draws = Random::of_entity(7, EntityId(40), 0);
    let (found, counts) = find_path_counted(plot, mob, target, 16.0, reach, limit, &mut draws);
    (found.expect("a walker always starts somewhere"), counts)
}

fn cells(path: &FoundPath) -> Vec<(i32, i32, i32)> {
    path.nodes
        .iter()
        .map(|node| (node.x, node.y, node.z))
        .collect()
}

/// Every step of the path is to a neighbouring column.
fn steps_are_neighbours(path: &FoundPath) -> bool {
    path.nodes
        .windows(2)
        .all(|pair| (pair[0].x - pair[1].x).abs() <= 1 && (pair[0].z - pair[1].z).abs() <= 1)
}

/// A wall of stone at x = 2 from z = -3 to 3, `height` high, but where `gap` is.
fn walled(height: i32, gap: Option<i32>) -> Plot {
    walled_across(3, height, gap)
}

/// A wall of stone at x = 2 from z = `-half` to `half`.
fn walled_across(half: i32, height: i32, gap: Option<i32>) -> Plot {
    let mut plot = Plot::flat();
    for z in -half..=half {
        if gap == Some(z) {
            continue;
        }
        for y in 0..height {
            plot.set(2, y, z, blocks::STONE);
        }
    }
    plot
}

fn door(open: bool, half: &str) -> BlockState {
    BlockState::from_name_and_properties(
        "minecraft:oak_door",
        [
            ("facing", "east"),
            ("half", half),
            ("open", if open { "true" } else { "false" }),
        ],
    )
    .expect("an oak door")
}

#[test]
fn a_cow_on_flat_ground_walks_straight_to_its_target() {
    let plot = Plot::flat();
    let (path, _) = search(&plot, &cow([0.5, 0.0, 0.5]), BlockPos::new(4, 0, 0), 0);
    assert_eq!(
        cells(&path),
        [(0, 0, 0), (1, 0, 0), (2, 0, 0), (3, 0, 0), (4, 0, 0)]
    );
    assert!(
        path.nodes
            .iter()
            .all(|node| node.kind() == PathType::Walkable)
    );
    assert!(path.reached);
    assert_eq!(path.dist_to_target, 0.0);
}

#[test]
fn a_path_goes_round_a_wall() {
    let plot = walled_across(1, 2, None);
    let (path, _) = search(&plot, &cow([0.5, 0.0, 0.5]), BlockPos::new(4, 0, 0), 0);
    assert!(path.reached, "{:?}", cells(&path));
    assert!(steps_are_neighbours(&path));
    assert!(
        path.nodes
            .iter()
            .all(|node| !(node.x == 2 && (-1..=1).contains(&node.z))),
        "{:?}",
        cells(&path)
    );
    // It leaves the wall at one of its ends.
    assert!(path.nodes.iter().any(|node| node.z.abs() >= 2));
}

#[test]
fn a_node_s_walked_distance_is_its_last_visitor_s_so_a_wide_wall_ends_the_search_short() {
    // Each visit overwrites a neighbour's walked distance, improving or not (the brief's
    // 3.0.7): the cells round a wall seven wide count as farther than they are, and the
    // search runs out of range just past it, four blocks from its target.
    let plot = walled(2, None);
    let (path, counts) = search(&plot, &cow([0.5, 0.0, 0.5]), BlockPos::new(4, 0, 0), 0);
    assert!(!path.reached);
    assert!(counts.pops < 255, "{counts:?}");
    assert_eq!(
        path.nodes.last().map(|node| (node.x, node.y, node.z)),
        Some((3, 0, -1))
    );
}

#[test]
fn a_path_passes_an_open_door_and_not_a_closed_one_but_for_a_mob_that_opens_doors() {
    let mut plot = walled(2, Some(0));
    plot.set(2, 0, 0, door(true, "lower"));
    plot.set(2, 1, 0, door(true, "upper"));
    let (path, _) = search(&plot, &cow([0.5, 0.0, 0.5]), BlockPos::new(4, 0, 0), 0);
    assert_eq!(
        cells(&path),
        [(0, 0, 0), (1, 0, 0), (2, 0, 0), (3, 0, 0), (4, 0, 0)]
    );
    assert_eq!(path.nodes[2].kind(), PathType::DoorOpen);

    plot.set(2, 0, 0, door(false, "lower"));
    plot.set(2, 1, 0, door(false, "upper"));
    let (path, _) = search(&plot, &cow([0.5, 0.0, 0.5]), BlockPos::new(4, 0, 0), 0);
    assert!(!cells(&path).contains(&(2, 0, 0)), "{:?}", cells(&path));

    let mut opener = cow([0.5, 0.0, 0.5]);
    opener.flags.can_open_doors = true;
    let (path, _) = search(&plot, &opener, BlockPos::new(4, 0, 0), 0);
    assert_eq!(
        cells(&path),
        [(0, 0, 0), (1, 0, 0), (2, 0, 0), (3, 0, 0), (4, 0, 0)]
    );
    assert_eq!(path.nodes[2].kind(), PathType::WalkableDoor);
}

#[test]
fn a_cow_steps_up_one_block_and_not_two() {
    let mut plot = Plot::flat();
    for x in 2..=6 {
        for z in -5..=5 {
            plot.set(x, 0, z, blocks::STONE);
        }
    }
    let (path, _) = search(&plot, &cow([0.5, 0.0, 0.5]), BlockPos::new(4, 1, 0), 0);
    assert_eq!(
        cells(&path),
        [(0, 0, 0), (1, 0, 0), (2, 1, 0), (3, 1, 0), (4, 1, 0)]
    );
    assert!(path.reached);

    for x in 2..=6 {
        for z in -5..=5 {
            plot.set(x, 1, z, blocks::STONE);
        }
    }
    let (path, _) = search(&plot, &cow([0.5, 0.0, 0.5]), BlockPos::new(4, 2, 0), 0);
    assert!(!path.reached);
    assert!(
        path.nodes.iter().all(|node| node.y == 0 && node.x <= 1),
        "{:?}",
        cells(&path)
    );
    // It goes to the node it saw nearest the target: the foot of the step.
    assert_eq!(
        path.nodes.last().map(|node| (node.x, node.y, node.z)),
        Some((1, 0, 0))
    );
}

#[test]
fn a_cow_that_does_not_float_goes_round_water() {
    let mut plot = Plot::flat();
    for x in 2..=4 {
        for z in -2..=2 {
            plot.set(x, -1, z, blocks::WATER);
        }
    }
    let (path, _) = search(&plot, &cow([0.5, 0.0, 0.5]), BlockPos::new(6, 0, 0), 0);
    assert!(path.reached, "{:?}", cells(&path));
    assert!(steps_are_neighbours(&path));
    assert!(
        path.nodes.iter().all(|node| node.kind() != PathType::Water
            && !((2..=4).contains(&node.x) && (-2..=2).contains(&node.z))),
        "{:?}",
        cells(&path)
    );
}

#[test]
fn a_search_pops_one_node_fewer_than_its_limit() {
    let mut plot = Plot::flat();
    // The target sealed in stone, where nothing reaches it.
    for x in 9..=11 {
        for y in -1..=2 {
            for z in -1..=1 {
                if (x, z) != (10, 0) || y == -1 || y == 2 {
                    plot.set(x, y, z, blocks::STONE);
                }
            }
        }
    }
    let (path, counts) =
        search_limited(&plot, &cow([0.5, 0.0, 0.5]), BlockPos::new(10, 0, 0), 0, 20);
    assert_eq!(counts.pops, 19);
    assert!(!path.reached);
}

#[test]
fn a_path_that_cannot_reach_its_target_goes_to_the_node_seen_nearest_it() {
    let mut plot = Plot::flat();
    for x in 5..=7 {
        for y in 0..=2 {
            for z in -1..=1 {
                if (x, z) != (6, 0) || y == 2 {
                    plot.set(x, y, z, blocks::STONE);
                }
            }
        }
    }
    let (path, _) = search(&plot, &cow([0.5, 0.0, 0.5]), BlockPos::new(6, 0, 0), 0);
    assert!(!path.reached);
    assert_eq!(
        path.nodes.last().map(|node| (node.x, node.y, node.z)),
        Some((4, 0, 0))
    );
    assert_eq!(path.dist_to_target, 2.0);
}

#[test]
fn land_beyond_the_land_and_copies_is_not_walked_onto() {
    let mut plot = Plot::flat();
    plot.edge = 16;
    let (path, _) = search(&plot, &cow([10.5, 0.0, 0.5]), BlockPos::new(20, 0, 0), 0);
    assert!(!path.reached);
    assert!(
        path.nodes.iter().all(|node| node.x < 16),
        "{:?}",
        cells(&path)
    );
    assert_eq!(path.nodes.last().map(|node| node.x), Some(15));
}

#[test]
fn a_search_is_a_function_of_the_mob_the_target_and_the_blocks() {
    let plot = walled(2, None);
    let mob = cow([0.5, 0.0, 0.5]);
    let once = search(&plot, &mob, BlockPos::new(4, 0, 0), 0);
    let again = search(&plot, &mob, BlockPos::new(4, 0, 0), 0);
    assert_eq!(once.0, again.0);
    assert_eq!(once.1, again.1);
    // And it draws nothing.
    let mut draws = Random::of_entity(7, EntityId(40), 0);
    let start = draws.clone();
    find_path(
        &plot,
        &mob,
        BlockPos::new(4, 0, 0),
        16.0,
        0,
        256,
        &mut draws,
    );
    assert_eq!(draws.draws_since(&start), 0);
}

#[test]
fn a_fence_is_never_stepped_over_by_a_cow() {
    let mut plot = Plot::flat();
    let fence = BlockState::from_name_and_properties("minecraft:oak_fence", []).expect("a fence");
    for z in -6..=6 {
        plot.set(2, 0, z, fence);
    }
    let (path, _) = search(&plot, &cow([0.5, 0.0, 0.5]), BlockPos::new(4, 0, 0), 0);
    assert!(
        path.nodes
            .iter()
            .all(|node| node.x != 2 || node.z.abs() > 6),
        "{:?}",
        cells(&path)
    );
}
