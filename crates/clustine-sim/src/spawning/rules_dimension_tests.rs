//! `Monster.isDarkEnoughToSpawn` by dimension (`docs/groundwork/p-plan.md`, 17.2 D2;
//! the brief's 1.2): the block-light limit and the spawn light of each dimension's type,
//! with the draws the game makes for each. Declared in `rules.rs`.

use std::collections::VecDeque;

use clustine_data::BlockState;
use clustine_world::generation::Draws;
use clustine_world::{Biome, BlockPos, Dimension};

use super::{Light, SpawnWorld, is_dark_enough};

/// Air everywhere under one light, in one dimension, darkened by `darken`.
struct Lit {
    dimension: Dimension,
    light: Light,
    darken: i32,
}

impl SpawnWorld for Lit {
    fn block(&self, _: BlockPos) -> Option<BlockState> {
        Some(clustine_data::blocks::AIR)
    }

    fn light(&self, _: BlockPos) -> Option<Light> {
        Some(self.light)
    }

    fn sky_darken(&self) -> i32 {
        self.darken
    }

    fn biome(&self, _: BlockPos) -> Option<Biome> {
        None
    }

    fn surface(&self, _: i32, _: i32) -> Option<i32> {
        None
    }

    fn game_seed(&self) -> i64 {
        0
    }

    fn day(&self) -> u64 {
        18_000
    }

    fn dimension(&self) -> Dimension {
        self.dimension
    }
}

/// A random that gives the ints it is told, and notes the bounds asked of it.
struct Ints {
    values: VecDeque<u32>,
    bounds: Vec<u32>,
}

impl Ints {
    fn of(values: &[u32]) -> Self {
        Self {
            values: values.iter().copied().collect(),
            bounds: Vec::new(),
        }
    }
}

impl Draws for Ints {
    fn next_int(&mut self, bound: u32) -> u32 {
        self.bounds.push(bound);
        let value = self
            .values
            .pop_front()
            .unwrap_or_else(|| panic!("an int of {bound} was drawn past the script"));
        assert!(value < bound, "{value} is not below {bound}");
        value
    }

    fn next_float(&mut self) -> f32 {
        panic!("the darkness draws no float")
    }

    fn next_double(&mut self) -> f64 {
        panic!("the darkness draws no double")
    }

    fn next_bool(&mut self) -> bool {
        panic!("the darkness draws no boolean")
    }
}

/// What `isDarkEnoughToSpawn` answers in `dimension` under `sky` and `block` light (the
/// sky's darkened by the dimension's own darkening: the overworld's at midnight, 11 in
/// the Nether, none in the End), given `draws`, with the bounds it drew.
fn dark_enough(dimension: Dimension, sky: u8, block: u8, draws: &[u32]) -> (bool, Vec<u32>) {
    let darken = crate::time::level_sky_darken(dimension, 18_000);
    let world = Lit {
        dimension,
        light: Light { sky, block },
        darken,
    };
    let mut ints = Ints::of(draws);
    let dark =
        is_dark_enough(&world, BlockPos::new(0, 64, 0), &mut ints).expect("the light is known");
    assert!(ints.values.is_empty(), "every scripted draw is used");
    (dark, ints.bounds)
}

#[test]
fn the_overworld_refuses_any_block_light_and_draws_its_light_from_zero_to_seven() {
    let d = Dimension::Overworld;
    assert_eq!(dark_enough(d, 0, 1, &[0]), (false, vec![32]));
    assert_eq!(dark_enough(d, 0, 0, &[0, 0]), (true, vec![32, 8]));
    // At midnight the open sky's 15 is darkened to 4: up to a draw of 4 refuses it.
    assert_eq!(dark_enough(d, 15, 0, &[31, 3]), (false, vec![32, 8]));
    assert_eq!(dark_enough(d, 15, 0, &[31, 4]), (true, vec![32, 8]));
    // The sky's undarkened light above the first draw refuses before anything else.
    assert_eq!(dark_enough(d, 15, 0, &[14]), (false, vec![32]));
}

#[test]
fn the_nether_takes_block_light_up_to_seven_and_draws_no_light() {
    let d = Dimension::Nether;
    for block in 0..=7 {
        assert_eq!(dark_enough(d, 0, block, &[0]), (true, vec![32]), "{block}");
    }
    // The limit of 15 is never compared; the light of 8 and more is above the constant 7.
    for block in 8..=15 {
        assert_eq!(dark_enough(d, 0, block, &[0]), (false, vec![32]), "{block}");
    }
}

#[test]
fn the_end_refuses_any_block_light_and_takes_every_sky_light_without_a_draw() {
    let d = Dimension::End;
    assert_eq!(dark_enough(d, 15, 1, &[31]), (false, vec![32]));
    assert_eq!(dark_enough(d, 15, 0, &[31]), (true, vec![32]));
    assert_eq!(dark_enough(d, 15, 0, &[15]), (true, vec![32]));
    // Under the End's undarkened sky of 15 half the first draws refuse.
    assert_eq!(dark_enough(d, 15, 0, &[14]), (false, vec![32]));
    assert_eq!(dark_enough(d, 0, 0, &[0]), (true, vec![32]));
}

#[test]
fn a_constant_spawn_light_draws_nothing_and_a_uniform_one_draws_its_span() {
    let mut ints = Ints::of(&[]);
    assert_eq!(super::sample_spawn_light((7, 7), &mut ints), 7);
    assert_eq!(super::sample_spawn_light((15, 15), &mut ints), 15);
    assert!(ints.bounds.is_empty());
    let mut ints = Ints::of(&[5]);
    assert_eq!(super::sample_spawn_light((0, 7), &mut ints), 5);
    assert_eq!(ints.bounds, [8]);
    let mut ints = Ints::of(&[2]);
    assert_eq!(super::sample_spawn_light((3, 9), &mut ints), 5);
    assert_eq!(ints.bounds, [7]);
}
