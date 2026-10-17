//! Tests of what a piston moves (`piston.rs`), against the game's
//! `PistonStructureResolver`: a line of up to twelve, slime and honey that take what
//! sticks to them and not each other, what a piston leaves alone and what it breaks.

use clustine_data::{Face, blocks};
use clustine_world::BlockPos;

use super::World;
use crate::Rules;
use crate::piston::{Heights, MAX_PUSH_DEPTH, Structure, is_pushable, resolve};

const HEIGHTS: Heights = Heights {
    min_y: -64,
    max_y_exclusive: 320,
};

/// A piston at the origin facing east, and what stands to its east.
fn east_of_a_piston(blocks: &[(i32, clustine_data::BlockState)]) -> World {
    let mut world = World::new(Rules::Placing);
    for (x, state) in blocks {
        world = world.with(*x, 0, 0, *state);
    }
    world
}

fn pushed(world: &mut World) -> Option<Structure> {
    resolve(world, BlockPos::new(0, 0, 0), Face::East, true, HEIGHTS)
}

#[test]
fn a_piston_pushes_a_line_of_twelve_and_not_of_thirteen() {
    let line: Vec<(i32, _)> = (1..=12).map(|x| (x, blocks::STONE)).collect();
    let structure = pushed(&mut east_of_a_piston(&line)).expect("twelve move");
    assert_eq!(structure.to_push.len(), MAX_PUSH_DEPTH);
    assert_eq!(
        structure.to_push[0],
        BlockPos::new(1, 0, 0),
        "nearest first"
    );
    let mut longer = line.clone();
    longer.push((13, blocks::STONE));
    assert_eq!(pushed(&mut east_of_a_piston(&longer)), None);
}

#[test]
fn obsidian_and_bedrock_are_not_moved_and_a_flower_is_broken() {
    assert_eq!(
        pushed(&mut east_of_a_piston(&[(1, blocks::OBSIDIAN)])),
        None
    );
    assert_eq!(pushed(&mut east_of_a_piston(&[(1, blocks::BEDROCK)])), None);
    // A line that ends against obsidian does not move either.
    let blocked = [(1, blocks::STONE), (2, blocks::OBSIDIAN)];
    assert_eq!(pushed(&mut east_of_a_piston(&blocked)), None);
    let flower = pushed(&mut east_of_a_piston(&[
        (1, blocks::STONE),
        (2, blocks::POPPY),
    ]))
    .expect("the flower breaks");
    assert_eq!(flower.to_push, [BlockPos::new(1, 0, 0)]);
    assert_eq!(flower.to_destroy, [BlockPos::new(2, 0, 0)]);
}

#[test]
fn a_chest_keeps_its_block_entity_and_is_not_moved() {
    assert_eq!(pushed(&mut east_of_a_piston(&[(1, blocks::CHEST)])), None);
}

#[test]
fn slime_takes_the_blocks_stuck_to_it_and_honey_does_not_stick_to_slime() {
    let mut world = east_of_a_piston(&[(1, blocks::SLIME_BLOCK)])
        .with(1, 1, 0, blocks::STONE)
        .with(1, 0, 1, blocks::HONEY_BLOCK);
    let structure = pushed(&mut world).expect("it moves");
    assert!(
        structure.to_push.contains(&BlockPos::new(1, 1, 0)),
        "{structure:?}"
    );
    assert!(
        !structure.to_push.contains(&BlockPos::new(1, 0, 1)),
        "{structure:?}"
    );
}

#[test]
fn a_block_that_is_only_pushed_is_pushed_and_not_pulled() {
    // Glazed terracotta is pushed and never pulled.
    let glazed = blocks::WHITE_GLAZED_TERRACOTTA;
    assert!(is_pushable(
        glazed,
        BlockPos::new(1, 0, 0),
        Face::East,
        false,
        Face::East,
        HEIGHTS
    ));
    assert!(!is_pushable(
        glazed,
        BlockPos::new(1, 0, 0),
        Face::West,
        false,
        Face::East,
        HEIGHTS
    ));
}

#[test]
fn nothing_is_pushed_out_of_the_world() {
    let top = BlockPos::new(1, HEIGHTS.max_y_exclusive - 1, 0);
    assert!(!is_pushable(
        blocks::STONE,
        top,
        Face::Up,
        false,
        Face::Up,
        HEIGHTS
    ));
    assert!(is_pushable(
        blocks::STONE,
        top,
        Face::East,
        false,
        Face::East,
        HEIGHTS
    ));
}

#[test]
fn a_sticky_piston_pulls_the_block_two_ahead() {
    let mut world = east_of_a_piston(&[(2, blocks::STONE)]);
    let pulled = resolve(
        &mut world,
        BlockPos::new(0, 0, 0),
        Face::East,
        false,
        HEIGHTS,
    )
    .expect("stone is pulled");
    assert_eq!(pulled.push_direction, Face::West);
    assert_eq!(pulled.to_push, [BlockPos::new(2, 0, 0)]);
}
