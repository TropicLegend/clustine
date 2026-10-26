//! The golem patterns' search, its expectations worked out by hand from
//! `BlockPattern.find`, `translateAndRotate` and `CarvedPumpkinBlock` in the dumps
//! (`docs/groundwork/n7k-brief.md`, 11.7), not from the code.

use std::collections::BTreeMap;

use clustine_blocks::data::set;
use clustine_data::{BlockState, Face, blocks};
use clustine_world::BlockPos;

use super::{
    COPPER_GOLEM_FULL, Golem, IRON_GOLEM_FULL, SNOW_GOLEM_FULL, can_spawn_golem, find, first_golem,
    orientations, pattern_cells,
};

/// A world of air but for the blocks set.
#[derive(Default)]
struct World(BTreeMap<BlockPos, BlockState>);

impl World {
    fn set(&mut self, x: i32, y: i32, z: i32, state: BlockState) -> &mut Self {
        self.0.insert(BlockPos::new(x, y, z), state);
        self
    }

    fn read(&self) -> impl Fn(BlockPos) -> Option<BlockState> + '_ {
        |pos| Some(self.0.get(&pos).copied().unwrap_or(blocks::AIR))
    }
}

fn at(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

fn pumpkin(facing: &str) -> BlockState {
    set(blocks::CARVED_PUMPKIN, "facing", facing)
}

/// An upright iron golem with its head at (0, 64, 0), its arms along x.
fn iron_golem_arms_along_x(head: BlockState) -> World {
    let mut world = World::default();
    world
        .set(0, 64, 0, head)
        .set(-1, 63, 0, blocks::IRON_BLOCK)
        .set(0, 63, 0, blocks::IRON_BLOCK)
        .set(1, 63, 0, blocks::IRON_BLOCK)
        .set(0, 62, 0, blocks::IRON_BLOCK);
    world
}

#[test]
fn the_orientations_are_tried_forwards_first_in_the_order_of_direction() {
    let all: Vec<(Face, Face)> = orientations().collect();
    assert_eq!(all.len(), 24);
    use Face::{Down, East, North, South, Up, West};
    assert_eq!(
        all,
        vec![
            (Down, North),
            (Down, South),
            (Down, West),
            (Down, East),
            (Up, North),
            (Up, South),
            (Up, West),
            (Up, East),
            (North, Down),
            (North, Up),
            (North, West),
            (North, East),
            (South, Down),
            (South, Up),
            (South, West),
            (South, East),
            (West, Down),
            (West, Up),
            (West, North),
            (West, South),
            (East, Down),
            (East, Up),
            (East, North),
            (East, South),
        ]
    );
}

#[test]
fn upright_iron_golems_are_found_whichever_way_the_pumpkin_faces() {
    // Arms along x: the corner can only be the shoulder at +x, (1, 64, 0), the second
    // corner tried; columns run west from it, so forwards × up is west: south and up.
    for facing in ["north", "south"] {
        let world = iron_golem_arms_along_x(pumpkin(facing));
        let (golem, found) = first_golem(at(0, 64, 0), world.read()).expect("a golem");
        assert_eq!(golem, Golem::Iron);
        assert_eq!(found.front_top_left, at(1, 64, 0));
        assert_eq!((found.forwards, found.up), (Face::South, Face::Up));
        assert_eq!(
            (found.width, found.height, found.depth),
            (3, 3, 1),
            "facing {facing}"
        );
        assert_eq!(
            pattern_cells(&found),
            vec![
                at(1, 64, 0),
                at(1, 63, 0),
                at(1, 62, 0),
                at(0, 64, 0),
                at(0, 63, 0),
                at(0, 62, 0),
                at(-1, 64, 0),
                at(-1, 63, 0),
                at(-1, 62, 0),
            ]
        );
        assert_eq!(golem.stand_cell(&found), at(0, 62, 0));
        assert_eq!(golem.stand(&found), [0.5, 62.05, 0.5]);
        assert_eq!(found.state(1, 0, 0), Some(pumpkin(facing)));
        assert_eq!(found.state(0, 0, 0), Some(blocks::AIR));
    }
    // Arms along z: the corner is the shoulder at +z, (0, 64, 1), the tenth corner;
    // columns run north, which forwards west with up up gives (west × up = north). The
    // orientations before it at that corner, down and west among them, fail.
    for facing in ["east", "west"] {
        let head = if facing == "east" {
            set(blocks::JACK_O_LANTERN, "facing", facing)
        } else {
            pumpkin(facing)
        };
        let mut world = World::default();
        world
            .set(0, 64, 0, head)
            .set(0, 63, -1, blocks::IRON_BLOCK)
            .set(0, 63, 0, blocks::IRON_BLOCK)
            .set(0, 63, 1, blocks::IRON_BLOCK)
            .set(0, 62, 0, blocks::IRON_BLOCK);
        let (golem, found) = first_golem(at(0, 64, 0), world.read()).expect("a golem");
        assert_eq!(golem, Golem::Iron);
        assert_eq!(found.front_top_left, at(0, 64, 1));
        assert_eq!((found.forwards, found.up), (Face::West, Face::Up));
        assert_eq!(
            pattern_cells(&found),
            vec![
                at(0, 64, 1),
                at(0, 63, 1),
                at(0, 62, 1),
                at(0, 64, 0),
                at(0, 63, 0),
                at(0, 62, 0),
                at(0, 64, -1),
                at(0, 63, -1),
                at(0, 62, -1),
            ]
        );
        assert_eq!(golem.stand(&found), [0.5, 62.05, 0.5]);
    }
}

#[test]
fn an_iron_golem_lying_on_the_ground_is_found() {
    // All at y 64: the head at (0, 64, 0), the body at z 1 and the leg at z 2. Rows run
    // south, so up is north; columns run west from (1, 64, 0), and up × north is west.
    let mut world = World::default();
    world
        .set(0, 64, 0, pumpkin("north"))
        .set(-1, 64, 1, blocks::IRON_BLOCK)
        .set(0, 64, 1, blocks::IRON_BLOCK)
        .set(1, 64, 1, blocks::IRON_BLOCK)
        .set(0, 64, 2, blocks::IRON_BLOCK);
    let (golem, found) = first_golem(at(0, 64, 0), world.read()).expect("a golem");
    assert_eq!(golem, Golem::Iron);
    assert_eq!(found.front_top_left, at(1, 64, 0));
    assert_eq!((found.forwards, found.up), (Face::Up, Face::North));
    assert_eq!(found.block(1, 1, 0), at(0, 64, 1));
    assert_eq!(found.block(2, 2, 0), at(-1, 64, 2));
    assert_eq!(golem.stand(&found), [0.5, 64.05, 2.5]);
}

#[test]
fn an_upside_down_iron_golem_is_found() {
    // The head at (0, 64, 0) under its body at y 65 and its leg at y 66. Up is down;
    // north × down is west, the columns' way from (1, 64, 0).
    let mut world = World::default();
    world
        .set(0, 64, 0, pumpkin("south"))
        .set(-1, 65, 0, blocks::IRON_BLOCK)
        .set(0, 65, 0, blocks::IRON_BLOCK)
        .set(1, 65, 0, blocks::IRON_BLOCK)
        .set(0, 66, 0, blocks::IRON_BLOCK);
    let (golem, found) = first_golem(at(0, 64, 0), world.read()).expect("a golem");
    assert_eq!(golem, Golem::Iron);
    assert_eq!(found.front_top_left, at(1, 64, 0));
    assert_eq!((found.forwards, found.up), (Face::North, Face::Down));
    assert_eq!(golem.stand_cell(&found), at(0, 66, 0));
    assert_eq!(golem.stand(&found), [0.5, 66.05, 0.5]);
}

#[test]
fn a_snow_golem_is_built_before_an_iron_golem_where_both_match() {
    // One pumpkin heads both a snow golem below it and an iron golem lying south of it.
    let mut world = World::default();
    world
        .set(0, 64, 0, pumpkin("north"))
        .set(0, 63, 0, blocks::SNOW_BLOCK)
        .set(0, 62, 0, blocks::SNOW_BLOCK)
        .set(-1, 64, 1, blocks::IRON_BLOCK)
        .set(0, 64, 1, blocks::IRON_BLOCK)
        .set(1, 64, 1, blocks::IRON_BLOCK)
        .set(0, 64, 2, blocks::IRON_BLOCK);
    assert!(find(&IRON_GOLEM_FULL, at(0, 64, 0), world.read()).is_some());
    let (golem, found) = first_golem(at(0, 64, 0), world.read()).expect("a golem");
    assert_eq!(golem, Golem::Snow);
    // At the head, the orientations with forwards down or up put the rows sideways, on
    // the iron or on air; north with down puts them above; north with up is the first.
    assert_eq!(found.front_top_left, at(0, 64, 0));
    assert_eq!((found.forwards, found.up), (Face::North, Face::Up));
    assert_eq!((found.width, found.height, found.depth), (1, 3, 1));
    assert_eq!(
        pattern_cells(&found),
        vec![at(0, 64, 0), at(0, 63, 0), at(0, 62, 0)]
    );
    assert_eq!(golem.stand(&found), [0.5, 62.05, 0.5]);
}

#[test]
fn an_iron_golem_with_a_torch_at_a_shoulder_is_not_built() {
    let mut world = iron_golem_arms_along_x(pumpkin("north"));
    world.set(1, 64, 0, blocks::TORCH);
    assert_eq!(find(&IRON_GOLEM_FULL, at(0, 64, 0), world.read()), None);
    assert_eq!(first_golem(at(0, 64, 0), world.read()), None);
    // Cave air is air.
    let mut world = iron_golem_arms_along_x(pumpkin("north"));
    world.set(1, 64, 0, blocks::CAVE_AIR);
    assert!(first_golem(at(0, 64, 0), world.read()).is_some());
}

#[test]
fn a_cell_the_reader_cannot_read_fails_the_match() {
    let world = iron_golem_arms_along_x(pumpkin("north"));
    let read = world.read();
    let unreadable = |pos: BlockPos| (pos != at(-1, 62, 0)).then(|| read(pos)).flatten();
    assert_eq!(first_golem(at(0, 64, 0), unreadable), None);
    // One the pattern does not reach changes nothing.
    let far = |pos: BlockPos| (pos != at(5, 64, 0)).then(|| read(pos)).flatten();
    assert!(first_golem(at(0, 64, 0), far).is_some());
}

#[test]
fn the_corners_are_tried_x_first_then_y_then_z() {
    // Three snow golems whose heads are corners 1 (x + 1), 3 (y + 1) and 9 (z + 1) of
    // the search from (0, 64, 0); the corner of the lowest index is found.
    let mut world = World::default();
    world
        .set(0, 64, 1, pumpkin("north"))
        .set(0, 63, 1, blocks::SNOW_BLOCK)
        .set(0, 62, 1, blocks::SNOW_BLOCK)
        .set(0, 65, 0, pumpkin("north"))
        .set(0, 64, 0, blocks::SNOW_BLOCK)
        .set(0, 63, 0, blocks::SNOW_BLOCK)
        .set(1, 64, 0, pumpkin("north"))
        .set(1, 63, 0, blocks::SNOW_BLOCK)
        .set(1, 62, 0, blocks::SNOW_BLOCK);
    let corner = |world: &World| {
        find(&SNOW_GOLEM_FULL, at(0, 64, 0), world.read())
            .expect("a snow golem")
            .front_top_left
    };
    assert_eq!(corner(&world), at(1, 64, 0));
    world.set(1, 64, 0, blocks::AIR);
    assert_eq!(corner(&world), at(0, 65, 0));
    world.set(0, 65, 0, blocks::AIR);
    assert_eq!(corner(&world), at(0, 64, 1));
}

#[test]
fn of_two_corners_that_match_the_first_is_built() {
    // Searched from the shoulder at -x, an upright iron golem matches at the first
    // corner, columns running east (north × up), and again at the third, columns
    // running west (south × up); the first wins.
    let world = iron_golem_arms_along_x(pumpkin("north"));
    let found = find(&IRON_GOLEM_FULL, at(-1, 64, 0), world.read()).expect("a golem");
    assert_eq!(found.front_top_left, at(-1, 64, 0));
    assert_eq!((found.forwards, found.up), (Face::North, Face::Up));
    assert_eq!(found.block(2, 0, 0), at(1, 64, 0));
    assert_eq!(Golem::Iron.stand(&found), [0.5, 62.05, 0.5]);
}

#[test]
fn of_two_orientations_that_match_at_one_corner_the_first_is_built() {
    // A pumpkin with two snow blocks below it and two east of it: upright (north with
    // up) and lying east (down with west, rows running east) both match at the head.
    // Forwards is the outer loop, so down with west comes first, and the golem stands
    // at the far end of the lying one.
    let mut world = World::default();
    world
        .set(0, 64, 0, pumpkin("north"))
        .set(0, 63, 0, blocks::SNOW_BLOCK)
        .set(0, 62, 0, blocks::SNOW_BLOCK)
        .set(1, 64, 0, blocks::SNOW_BLOCK)
        .set(2, 64, 0, blocks::SNOW_BLOCK);
    let (golem, found) = first_golem(at(0, 64, 0), world.read()).expect("a golem");
    assert_eq!(golem, Golem::Snow);
    assert_eq!(found.front_top_left, at(0, 64, 0));
    assert_eq!((found.forwards, found.up), (Face::Down, Face::West));
    assert_eq!(
        pattern_cells(&found),
        vec![at(0, 64, 0), at(1, 64, 0), at(2, 64, 0)]
    );
    assert_eq!(golem.stand(&found), [2.5, 64.05, 0.5]);
}

#[test]
fn a_copper_golem_is_built_on_copper_of_any_weathering_waxed_or_not() {
    for copper in [blocks::WEATHERED_COPPER, blocks::WAXED_OXIDIZED_COPPER] {
        let mut world = World::default();
        world.set(0, 64, 0, pumpkin("east")).set(0, 63, 0, copper);
        let (golem, found) = first_golem(at(0, 64, 0), world.read()).expect("a golem");
        assert_eq!(golem, Golem::Copper);
        assert_eq!(found.front_top_left, at(0, 64, 0));
        assert_eq!((found.forwards, found.up), (Face::North, Face::Up));
        assert_eq!((found.width, found.height, found.depth), (1, 2, 1));
        assert_eq!(pattern_cells(&found), vec![at(0, 64, 0), at(0, 63, 0)]);
        // `trySpawnGolem` stands it at the pumpkin's cell, `getBlock(0, 0, 0)`, over the
        // chest `replaceCopperBlockWithChest` puts at `getBlock(0, 1, 0)`.
        assert_eq!(golem.stand(&found), [0.5, 64.05, 0.5]);
        assert_eq!(found.block(0, 1, 0), at(0, 63, 0));
        assert_eq!(found.state(0, 1, 0), Some(copper));
    }
    let mut world = World::default();
    world
        .set(0, 64, 0, pumpkin("east"))
        .set(0, 63, 0, blocks::IRON_BLOCK);
    assert_eq!(find(&COPPER_GOLEM_FULL, at(0, 64, 0), world.read()), None);
}

#[test]
fn the_base_patterns_ask_for_no_pumpkin() {
    // `canSpawnGolem`'s patterns have a space where the head goes.
    let mut world = World::default();
    world
        .set(0, 63, 0, blocks::SNOW_BLOCK)
        .set(0, 62, 0, blocks::SNOW_BLOCK);
    assert!(can_spawn_golem(at(0, 64, 0), world.read()));
    let mut world = iron_golem_arms_along_x(blocks::STONE);
    assert!(can_spawn_golem(at(0, 64, 0), world.read()));
    world.set(1, 64, 0, blocks::TORCH);
    assert!(!can_spawn_golem(at(0, 64, 0), world.read()));
    let mut world = World::default();
    world.set(0, 63, 0, blocks::OXIDIZED_COPPER);
    assert!(can_spawn_golem(at(0, 64, 0), world.read()));
    assert!(!can_spawn_golem(at(0, 64, 0), World::default().read()));
}
