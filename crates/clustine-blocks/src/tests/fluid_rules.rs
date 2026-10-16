//! Tests of what a fluid reads of the world (`crate::fluid`), written from the
//! specification of the game's fluids by someone who did not write the code.

use clustine_data::{BlockState, Face, blocks};

use super::{World, at, state};
use crate::fluid::{
    self, FLOWING_LAVA, FLOWING_WATER, FluidState, Kind, LAVA, WATER, can_be_replaced_with,
    can_hold_fluid, can_pass_through_wall, can_place_liquid, concrete_of, is_container, new_liquid,
    touches_water,
};
use crate::{BlockView, update_shape};

fn water(level: u8) -> BlockState {
    blocks::WATER
        .with_property("level", &level.to_string())
        .expect("water has every level")
}

fn lava(level: u8) -> BlockState {
    blocks::LAVA
        .with_property("level", &level.to_string())
        .expect("lava has every level")
}

/// A world with a stone floor at y = 63 from -12 to 12 each way, for fluids at y = 64
/// around the origin, further from its edges than water looks for a way down.
fn on_stone() -> World {
    let mut world = World::placing();
    for x in -12..=12 {
        for z in -12..=12 {
            world = world.with(x, 63, z, blocks::STONE);
        }
    }
    world
}

/// What a fluid of `kind` would make of the block at the origin at y = 64.
fn new_at_origin(world: &mut World, kind: Kind) -> FluidState {
    let here = world.block(at(0, 64, 0));
    new_liquid(world, kind, at(0, 64, 0), here)
}

// ---------------------------------------------------------------------------------------
// Ids, amounts and levels (specification 1 and 3)
// ---------------------------------------------------------------------------------------

#[test]
fn the_fluids_have_the_games_ids() {
    assert_eq!(
        (fluid::EMPTY, FLOWING_WATER, WATER, FLOWING_LAVA, LAVA),
        (0, 1, 2, 3, 4)
    );
    assert_eq!(Kind::Water.source(), WATER);
    assert_eq!(Kind::Water.flowing(), FLOWING_WATER);
    assert_eq!(Kind::Lava.source(), LAVA);
    assert_eq!(Kind::Lava.flowing(), FLOWING_LAVA);
}

#[test]
fn water_and_lava_have_their_delays_drop_offs_and_reach() {
    assert_eq!(Kind::Water.tick_delay(), 5);
    assert_eq!(Kind::Lava.tick_delay(), 30);
    assert_eq!(Kind::Water.drop_off(), 1);
    assert_eq!(Kind::Lava.drop_off(), 2);
    assert_eq!(Kind::Water.slope_find_distance(), 4);
    assert_eq!(Kind::Lava.slope_find_distance(), 2);
    assert!(Kind::Water.converts_to_source());
    assert!(!Kind::Lava.converts_to_source());
}

#[test]
fn a_source_holds_amount_eight_at_level_zero() {
    for (block, kind) in [(blocks::WATER, Kind::Water), (blocks::LAVA, Kind::Lava)] {
        let held = FluidState::of(block);
        assert_eq!(held, FluidState::source(kind, false));
        assert_eq!(held.amount, 8);
        assert!(held.is_source());
        assert_eq!(held.legacy_block(), block);
    }
}

#[test]
fn flowing_fluid_is_at_level_eight_less_its_amount() {
    for amount in 1..=7_u8 {
        let level = 8 - amount;
        let held = FluidState::of(water(level));
        assert_eq!(
            held,
            FluidState::flowing(Kind::Water, amount, false),
            "{level}"
        );
        assert_eq!(held.legacy_block(), water(level));
        assert_eq!(
            FluidState::flowing(Kind::Lava, amount, false).legacy_block(),
            lava(level)
        );
    }
}

#[test]
fn falling_fluid_has_amount_eight_at_level_eight() {
    let falling = FluidState::flowing(Kind::Water, 8, true);
    assert_eq!(falling.legacy_block(), water(8));
    assert_eq!(FluidState::of(water(8)), falling);
    assert_eq!(
        FluidState::flowing(Kind::Lava, 8, true).legacy_block(),
        lava(8)
    );
}

/// The game keeps one falling fluid state, of amount 8, for every level from 8 up.
#[test]
fn every_level_from_eight_up_is_falling_fluid_of_amount_eight() {
    for level in 8..=15 {
        assert_eq!(
            FluidState::of(water(level)),
            FluidState::flowing(Kind::Water, 8, true),
            "{level}"
        );
    }
}

#[test]
fn air_and_stone_hold_no_fluid_and_a_waterlogged_block_holds_still_water() {
    assert!(FluidState::of(blocks::AIR).is_empty());
    assert!(FluidState::of(blocks::STONE).is_empty());
    assert_eq!(
        FluidState::of(state("minecraft:oak_slab[type=bottom,waterlogged=true]")),
        FluidState::source(Kind::Water, false)
    );
    assert_eq!(FluidState::EMPTY.legacy_block(), blocks::AIR);
}

// ---------------------------------------------------------------------------------------
// getNewLiquid (specification 4)
// ---------------------------------------------------------------------------------------

#[test]
fn new_liquid_is_the_highest_neighbour_less_the_drop_off() {
    let mut world = on_stone()
        .with(1, 64, 0, water(3))
        .with(-1, 64, 0, water(5))
        .with(0, 64, 1, water(1));
    // Amounts 5, 3 and 7: the highest is 7, so 6, level 2.
    assert_eq!(
        new_at_origin(&mut world, Kind::Water),
        FluidState::flowing(Kind::Water, 6, false)
    );

    let mut world = on_stone().with(1, 64, 0, lava(2)).with(-1, 64, 0, lava(4));
    // Amounts 6 and 4: 6 less 2.
    assert_eq!(
        new_at_origin(&mut world, Kind::Lava),
        FluidState::flowing(Kind::Lava, 4, false)
    );
}

#[test]
fn new_liquid_beside_one_source_is_one_less() {
    let mut world = on_stone().with(1, 64, 0, blocks::WATER);
    assert_eq!(
        new_at_origin(&mut world, Kind::Water),
        FluidState::flowing(Kind::Water, 7, false)
    );
    let mut world = on_stone().with(1, 64, 0, blocks::LAVA);
    assert_eq!(
        new_at_origin(&mut world, Kind::Lava),
        FluidState::flowing(Kind::Lava, 6, false)
    );
}

#[test]
fn new_liquid_that_would_be_nothing_is_empty() {
    let mut world = on_stone().with(1, 64, 0, water(7));
    assert!(new_at_origin(&mut world, Kind::Water).is_empty());
    let mut world = on_stone().with(1, 64, 0, lava(6));
    assert!(new_at_origin(&mut world, Kind::Lava).is_empty());
    let mut world = on_stone();
    assert!(new_at_origin(&mut world, Kind::Water).is_empty());
}

#[test]
fn new_liquid_beside_falling_fluid_counts_it_as_amount_eight() {
    let mut world = on_stone().with(1, 64, 0, water(8));
    assert_eq!(
        new_at_origin(&mut world, Kind::Water),
        FluidState::flowing(Kind::Water, 7, false)
    );
}

#[test]
fn new_liquid_ignores_the_other_kind() {
    let mut world = on_stone().with(1, 64, 0, blocks::LAVA);
    assert!(new_at_origin(&mut world, Kind::Water).is_empty());
    let mut world = on_stone().with(1, 64, 0, blocks::WATER);
    assert!(new_at_origin(&mut world, Kind::Lava).is_empty());
}

#[test]
fn two_water_sources_on_stone_make_a_source() {
    let mut world = on_stone()
        .with(1, 64, 0, blocks::WATER)
        .with(-1, 64, 0, blocks::WATER);
    assert_eq!(
        new_at_origin(&mut world, Kind::Water),
        FluidState::source(Kind::Water, false)
    );
}

#[test]
fn two_water_sources_at_a_corner_make_a_source_too() {
    let mut world = on_stone()
        .with(1, 64, 0, blocks::WATER)
        .with(0, 64, 1, blocks::WATER);
    assert_eq!(
        new_at_origin(&mut world, Kind::Water),
        FluidState::source(Kind::Water, false)
    );
}

#[test]
fn two_water_sources_over_a_water_source_make_a_source() {
    let mut world = World::placing()
        .with(1, 64, 0, blocks::WATER)
        .with(-1, 64, 0, blocks::WATER)
        .with(0, 63, 0, blocks::WATER);
    assert_eq!(
        new_at_origin(&mut world, Kind::Water),
        FluidState::source(Kind::Water, false)
    );
}

#[test]
fn two_water_sources_over_air_or_flowing_water_make_flowing_water() {
    for below in [blocks::AIR, water(1), water(8)] {
        let mut world = World::placing()
            .with(1, 64, 0, blocks::WATER)
            .with(-1, 64, 0, blocks::WATER)
            .with(0, 63, 0, below);
        assert_eq!(
            new_at_origin(&mut world, Kind::Water),
            FluidState::flowing(Kind::Water, 7, false),
            "{below:?}"
        );
    }
}

#[test]
fn two_lava_sources_on_stone_make_flowing_lava() {
    let mut world = on_stone()
        .with(1, 64, 0, blocks::LAVA)
        .with(-1, 64, 0, blocks::LAVA);
    assert_eq!(
        new_at_origin(&mut world, Kind::Lava),
        FluidState::flowing(Kind::Lava, 6, false)
    );
    let mut world = on_stone()
        .with(1, 64, 0, blocks::LAVA)
        .with(-1, 64, 0, blocks::LAVA)
        .with(0, 64, 1, blocks::LAVA)
        .with(0, 64, -1, blocks::LAVA);
    assert_eq!(
        new_at_origin(&mut world, Kind::Lava),
        FluidState::flowing(Kind::Lava, 6, false)
    );
}

#[test]
fn the_same_fluid_above_makes_falling_fluid() {
    for above in [blocks::WATER, water(5), water(8)] {
        let mut world = on_stone().with(0, 65, 0, above);
        assert_eq!(
            new_at_origin(&mut world, Kind::Water),
            FluidState::flowing(Kind::Water, 8, true),
            "{above:?}"
        );
    }
    let mut world = on_stone().with(0, 65, 0, lava(3));
    assert_eq!(
        new_at_origin(&mut world, Kind::Lava),
        FluidState::flowing(Kind::Lava, 8, true)
    );
}

/// Two sources on solid ground come before the fluid above.
#[test]
fn two_sources_on_solid_ground_make_a_source_even_under_water() {
    let mut world = on_stone()
        .with(1, 64, 0, blocks::WATER)
        .with(-1, 64, 0, blocks::WATER)
        .with(0, 65, 0, blocks::WATER);
    assert_eq!(
        new_at_origin(&mut world, Kind::Water),
        FluidState::source(Kind::Water, false)
    );
}

/// A neighbour counts only where water can pass between it and the block.
#[test]
fn new_liquid_ignores_water_behind_a_closed_face() {
    // A stair facing west has its full-height half on its west side, so its face
    // towards the origin, which is west of it, is full and closes the way.
    let closed = state("minecraft:stone_stairs[facing=west,half=bottom,waterlogged=true]");
    let mut world = on_stone().with(1, 64, 0, closed);
    assert!(new_at_origin(&mut world, Kind::Water).is_empty());

    let open = state("minecraft:stone_stairs[facing=east,half=bottom,waterlogged=true]");
    let mut world = on_stone().with(1, 64, 0, open);
    assert_eq!(
        new_at_origin(&mut world, Kind::Water),
        FluidState::flowing(Kind::Water, 7, false)
    );
}

// ---------------------------------------------------------------------------------------
// canPassThroughWall (specification 8)
// ---------------------------------------------------------------------------------------

const SIDES: [Face; 4] = [Face::North, Face::South, Face::West, Face::East];

fn bottom_slab() -> BlockState {
    state("minecraft:stone_slab[type=bottom]")
}

fn top_slab() -> BlockState {
    state("minecraft:stone_slab[type=top]")
}

#[test]
fn fluid_passes_between_two_empty_blocks_every_way() {
    for face in Face::ALL {
        assert!(can_pass_through_wall(face, blocks::AIR, blocks::AIR));
        assert!(can_pass_through_wall(face, blocks::WATER, blocks::AIR));
        assert!(can_pass_through_wall(face, blocks::AIR, blocks::TORCH));
    }
}

#[test]
fn fluid_never_passes_into_or_out_of_a_full_block() {
    for face in Face::ALL {
        assert!(!can_pass_through_wall(face, blocks::AIR, blocks::STONE));
        assert!(!can_pass_through_wall(face, blocks::STONE, blocks::AIR));
        assert!(!can_pass_through_wall(face, blocks::WATER, blocks::DIRT));
        assert!(!can_pass_through_wall(face, bottom_slab(), blocks::STONE));
        let double = state("minecraft:stone_slab[type=double]");
        assert!(!can_pass_through_wall(face, blocks::AIR, double));
    }
}

#[test]
fn a_bottom_slab_lets_fluid_over_its_top_and_out_of_its_half_sides() {
    // Up out of it, and down into it from above: its top is half a block down.
    assert!(can_pass_through_wall(Face::Up, bottom_slab(), blocks::AIR));
    assert!(can_pass_through_wall(
        Face::Down,
        blocks::AIR,
        bottom_slab()
    ));
    // Its full bottom closes the face below it.
    assert!(!can_pass_through_wall(
        Face::Down,
        bottom_slab(),
        blocks::AIR
    ));
    assert!(!can_pass_through_wall(Face::Up, blocks::AIR, bottom_slab()));
    for face in SIDES {
        assert!(
            can_pass_through_wall(face, bottom_slab(), blocks::AIR),
            "{face:?}"
        );
        assert!(
            can_pass_through_wall(face, blocks::AIR, bottom_slab()),
            "{face:?}"
        );
    }
}

#[test]
fn a_top_slab_closes_its_top_and_lets_fluid_under_it_and_through_its_sides() {
    assert!(!can_pass_through_wall(Face::Up, top_slab(), blocks::AIR));
    assert!(!can_pass_through_wall(Face::Down, blocks::AIR, top_slab()));
    assert!(can_pass_through_wall(Face::Down, top_slab(), blocks::AIR));
    assert!(can_pass_through_wall(Face::Up, blocks::AIR, top_slab()));
    for face in SIDES {
        assert!(
            can_pass_through_wall(face, top_slab(), blocks::AIR),
            "{face:?}"
        );
    }
}

/// The two halves of a face, each of one block, close it together.
#[test]
fn a_bottom_slab_beside_a_top_slab_closes_the_face_between_them() {
    for face in SIDES {
        assert!(
            !can_pass_through_wall(face, bottom_slab(), top_slab()),
            "{face:?}"
        );
        assert!(
            !can_pass_through_wall(face, top_slab(), bottom_slab()),
            "{face:?}"
        );
        assert!(
            can_pass_through_wall(face, bottom_slab(), bottom_slab()),
            "{face:?}"
        );
        assert!(
            can_pass_through_wall(face, top_slab(), top_slab()),
            "{face:?}"
        );
    }
    // A bottom slab over a top slab: the top slab's top and the bottom slab's bottom
    // are both full.
    assert!(!can_pass_through_wall(
        Face::Down,
        bottom_slab(),
        top_slab()
    ));
    // A top slab over a bottom slab: neither reaches the face between them.
    assert!(can_pass_through_wall(Face::Down, top_slab(), bottom_slab()));
}

#[test]
fn stairs_close_their_full_back_and_their_bottom_and_let_fluid_out_elsewhere() {
    let stairs = state("minecraft:stone_stairs[facing=east,half=bottom,shape=straight]");
    // Facing east: the high half is on the east side, whose face is full.
    assert!(!can_pass_through_wall(Face::East, stairs, blocks::AIR));
    assert!(!can_pass_through_wall(Face::West, blocks::AIR, stairs));
    assert!(can_pass_through_wall(Face::West, stairs, blocks::AIR));
    assert!(can_pass_through_wall(Face::North, stairs, blocks::AIR));
    assert!(can_pass_through_wall(Face::South, stairs, blocks::AIR));
    assert!(!can_pass_through_wall(Face::Down, stairs, blocks::AIR));
    assert!(can_pass_through_wall(Face::Up, stairs, blocks::AIR));
}

// ---------------------------------------------------------------------------------------
// What takes a fluid (specification 8)
// ---------------------------------------------------------------------------------------

#[test]
fn every_waterloggable_block_and_kelp_and_seagrass_are_containers() {
    for container in [
        bottom_slab(),
        state("minecraft:oak_fence[waterlogged=false]"),
        state("minecraft:stone_stairs[waterlogged=true]"),
        blocks::BARRIER,
        blocks::KELP,
        blocks::KELP_PLANT,
        blocks::SEAGRASS,
        blocks::TALL_SEAGRASS,
    ] {
        assert!(is_container(container), "{container:?}");
    }
    for other in [blocks::AIR, blocks::STONE, blocks::TORCH, blocks::WATER] {
        assert!(!is_container(other), "{other:?}");
    }
}

#[test]
fn a_container_takes_only_still_water() {
    for fluid in [FLOWING_WATER, FLOWING_LAVA, LAVA] {
        assert!(!can_place_liquid(bottom_slab(), fluid), "{fluid}");
        assert!(!can_hold_fluid(bottom_slab(), fluid), "{fluid}");
    }
    assert!(can_place_liquid(bottom_slab(), WATER));
    assert!(can_hold_fluid(bottom_slab(), WATER));
}

#[test]
fn a_double_slab_a_barrier_kelp_and_seagrass_take_nothing() {
    let double = state("minecraft:stone_slab[type=double]");
    for container in [
        double,
        blocks::BARRIER,
        blocks::KELP,
        blocks::KELP_PLANT,
        blocks::SEAGRASS,
        blocks::TALL_SEAGRASS,
    ] {
        for fluid in [WATER, FLOWING_WATER, LAVA, FLOWING_LAVA] {
            assert!(!can_place_liquid(container, fluid), "{container:?} {fluid}");
            assert!(!can_hold_fluid(container, fluid), "{container:?} {fluid}");
        }
    }
}

#[test]
fn what_fluids_wash_away_holds_any_fluid() {
    for washed in [
        blocks::AIR,
        blocks::TORCH,
        blocks::POPPY,
        blocks::SHORT_GRASS,
    ] {
        for fluid in [WATER, FLOWING_WATER, LAVA, FLOWING_LAVA] {
            assert!(can_hold_fluid(washed, fluid), "{washed:?} {fluid}");
        }
    }
}

#[test]
fn stone_and_dirt_are_walls() {
    for wall in [
        blocks::STONE,
        blocks::DIRT,
        blocks::COBBLESTONE,
        blocks::GLASS,
    ] {
        for fluid in [WATER, FLOWING_WATER, LAVA, FLOWING_LAVA] {
            assert!(!can_hold_fluid(wall, fluid), "{wall:?} {fluid}");
        }
    }
}

// ---------------------------------------------------------------------------------------
// canBeReplacedWith (specification 7)
// ---------------------------------------------------------------------------------------

#[test]
fn water_gives_way_only_to_another_fluid_from_above() {
    let mut world = World::placing();
    let pos = at(0, 64, 0);
    for held in [blocks::WATER, water(3), water(8)] {
        let held = FluidState::of(held);
        assert!(can_be_replaced_with(
            &mut world,
            held,
            pos,
            FLOWING_LAVA,
            Face::Down
        ));
        assert!(can_be_replaced_with(
            &mut world,
            held,
            pos,
            LAVA,
            Face::Down
        ));
        assert!(!can_be_replaced_with(
            &mut world,
            held,
            pos,
            FLOWING_WATER,
            Face::Down
        ));
        assert!(!can_be_replaced_with(
            &mut world,
            held,
            pos,
            WATER,
            Face::Down
        ));
        for face in SIDES {
            assert!(!can_be_replaced_with(
                &mut world,
                held,
                pos,
                FLOWING_LAVA,
                face
            ));
        }
    }
}

#[test]
fn lava_gives_way_to_water_where_it_is_at_least_four_ninths_high() {
    let pos = at(0, 64, 0);
    let mut world = World::placing();
    // Amount 4 (level 4) is 4/9 high; amount 3 (level 5) is not.
    let high = FluidState::of(lava(4));
    let low = FluidState::of(lava(5));
    for face in Face::ALL {
        assert!(can_be_replaced_with(
            &mut world,
            high,
            pos,
            FLOWING_WATER,
            face
        ));
        assert!(!can_be_replaced_with(
            &mut world,
            low,
            pos,
            FLOWING_WATER,
            face
        ));
        assert!(!can_be_replaced_with(
            &mut world,
            high,
            pos,
            FLOWING_LAVA,
            face
        ));
    }
    // With lava above it, low lava is a whole block high.
    let mut world = World::placing().with(0, 65, 0, blocks::LAVA);
    assert!(can_be_replaced_with(
        &mut world,
        low,
        pos,
        WATER,
        Face::East
    ));
}

#[test]
fn nothing_gives_way_to_anything() {
    let mut world = World::placing();
    for face in Face::ALL {
        assert!(can_be_replaced_with(
            &mut world,
            FluidState::EMPTY,
            at(0, 64, 0),
            FLOWING_LAVA,
            face
        ));
    }
}

// ---------------------------------------------------------------------------------------
// Where it spreads (specification 5)
// ---------------------------------------------------------------------------------------

fn spread_from_origin(world: &mut World, kind: Kind) -> Vec<(Face, FluidState)> {
    let here = world.block(at(0, 64, 0));
    let mut found = fluid::spread(world, kind, at(0, 64, 0), here);
    found.sort_by_key(|(face, _)| *face as u8);
    found
}

fn every_side(fluid: FluidState) -> Vec<(Face, FluidState)> {
    let mut all: Vec<_> = SIDES.iter().map(|face| (*face, fluid)).collect();
    all.sort_by_key(|(face, _)| *face as u8);
    all
}

#[test]
fn a_source_on_flat_ground_spreads_to_every_side() {
    let mut world = on_stone().with(0, 64, 0, blocks::WATER);
    assert_eq!(
        spread_from_origin(&mut world, Kind::Water),
        every_side(FluidState::flowing(Kind::Water, 7, false))
    );
}

#[test]
fn a_source_spreads_only_towards_the_nearest_hole() {
    let mut world = on_stone()
        .with(0, 64, 0, blocks::WATER)
        .with(2, 63, 0, blocks::AIR)
        .with(-3, 63, 0, blocks::AIR);
    assert_eq!(
        spread_from_origin(&mut world, Kind::Water),
        [(Face::East, FluidState::flowing(Kind::Water, 7, false))]
    );
}

#[test]
fn a_source_spreads_to_every_side_that_ties_for_the_nearest_hole() {
    let mut world = on_stone()
        .with(0, 64, 0, blocks::WATER)
        .with(0, 63, 2, blocks::AIR)
        .with(0, 63, -2, blocks::AIR);
    let mut expected = vec![
        (Face::South, FluidState::flowing(Kind::Water, 7, false)),
        (Face::North, FluidState::flowing(Kind::Water, 7, false)),
    ];
    expected.sort_by_key(|(face, _)| *face as u8);
    assert_eq!(spread_from_origin(&mut world, Kind::Water), expected);
}

#[test]
fn a_hole_beside_is_nearer_than_any_other() {
    let mut world = on_stone()
        .with(0, 64, 0, blocks::WATER)
        .with(-1, 63, 0, blocks::AIR)
        .with(2, 63, 0, blocks::AIR);
    assert_eq!(
        spread_from_origin(&mut world, Kind::Water),
        [(Face::West, FluidState::flowing(Kind::Water, 7, false))]
    );
}

#[test]
fn flowing_water_does_not_spread_into_a_slab_beside_it() {
    let mut world = on_stone()
        .with(0, 64, 0, blocks::WATER)
        .with(1, 64, 0, bottom_slab());
    let found = spread_from_origin(&mut world, Kind::Water);
    assert!(
        found.iter().all(|(face, _)| *face != Face::East),
        "{found:?}"
    );
    assert_eq!(found.len(), 3);
}

#[test]
fn a_source_does_not_spread_into_another_source_or_a_wall() {
    let mut world = on_stone()
        .with(0, 64, 0, blocks::WATER)
        .with(1, 64, 0, blocks::WATER)
        .with(-1, 64, 0, blocks::STONE);
    let found = spread_from_origin(&mut world, Kind::Water);
    let faces: Vec<Face> = found.iter().map(|(face, _)| *face).collect();
    assert!(!faces.contains(&Face::East));
    assert!(!faces.contains(&Face::West));
    assert_eq!(faces.len(), 2);
}

// ---------------------------------------------------------------------------------------
// Concrete powder (specification 12)
// ---------------------------------------------------------------------------------------

#[test]
fn concrete_powder_hardens_into_its_own_concrete() {
    assert_eq!(
        concrete_of(blocks::WHITE_CONCRETE_POWDER),
        Some(blocks::WHITE_CONCRETE)
    );
    assert_eq!(
        concrete_of(blocks::BLACK_CONCRETE_POWDER),
        Some(blocks::BLACK_CONCRETE)
    );
    assert_eq!(concrete_of(blocks::SAND), None);
    assert_eq!(concrete_of(blocks::WHITE_CONCRETE), None);
}

#[test]
fn water_on_any_side_touches_concrete_powder() {
    let pos = at(0, 64, 0);
    for face in Face::ALL {
        let beside = crate::data::relative(pos, face);
        for wet in [blocks::WATER, water(5), bottom_slab_waterlogged()] {
            // Above the powder a bottom slab's sturdy bottom keeps its water off.
            if face == Face::Up && wet == bottom_slab_waterlogged() {
                continue;
            }
            let mut world = World::placing().with(beside.x, beside.y, beside.z, wet);
            assert!(touches_water(&mut world, pos), "{face:?} {wet:?}");
        }
    }
    let mut world = World::placing().with(0, 63, 0, blocks::STONE);
    assert!(!touches_water(&mut world, pos));
    let mut world = World::placing().with(0, 64, 1, blocks::LAVA);
    assert!(!touches_water(&mut world, pos));
}

fn bottom_slab_waterlogged() -> BlockState {
    state("minecraft:stone_slab[type=bottom,waterlogged=true]")
}

#[test]
fn water_behind_a_sturdy_face_does_not_touch_concrete_powder() {
    let pos = at(0, 64, 0);
    // Below the powder, a waterlogged top slab's top is sturdy; a bottom slab's top is
    // not.
    let top = state("minecraft:stone_slab[type=top,waterlogged=true]");
    let mut world = World::placing().with(0, 63, 0, top);
    assert!(!touches_water(&mut world, pos));
    let mut world = World::placing().with(0, 63, 0, bottom_slab_waterlogged());
    assert!(touches_water(&mut world, pos));
    // Above it, a waterlogged bottom slab's bottom is sturdy.
    let mut world = World::placing().with(0, 65, 0, bottom_slab_waterlogged());
    assert!(!touches_water(&mut world, pos));
}

#[test]
fn concrete_powder_told_of_water_beside_it_becomes_concrete() {
    let mut world = World::placing()
        .floor(63, blocks::STONE)
        .with(0, 64, 0, blocks::WHITE_CONCRETE_POWDER)
        .with(1, 64, 0, blocks::WATER);
    assert_eq!(
        update_shape(
            &mut world,
            blocks::WHITE_CONCRETE_POWDER,
            at(0, 64, 0),
            Face::East,
            blocks::WATER
        ),
        blocks::WHITE_CONCRETE
    );
}

#[test]
fn concrete_powder_placed_into_water_hardens() {
    let mut world = World::placing().floor(63, blocks::STONE);
    assert!(fluid::powder_hardens(
        &mut world,
        at(0, 64, 0),
        blocks::WATER
    ));
    assert!(!fluid::powder_hardens(
        &mut world,
        at(0, 64, 0),
        blocks::AIR
    ));
}
