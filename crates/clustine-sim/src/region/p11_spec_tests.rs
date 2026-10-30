//! Tests of step P11's monster spawn light by dimension, written from the specification
//! by someone who did not write the code (`docs/groundwork/p-plan.md`, section 17.2 D2;
//! `docs/groundwork/p-brief.md`, sections 1.1 and 1.2; n7 `Monster.isDarkEnoughToSpawn`):
//! the rule reads the level's `monster_spawn_light_level` and
//! `monster_spawn_block_light_limit` through the spawning's view of the level. The
//! rest of P11's tests, through the public interface, are in
//! `tests/p11_specification.rs`.
//!
//! `isDarkEnoughToSpawn`: the sky's light above `nextInt(32)` refuses; then, where the
//! limit is below 15, the blocks' light above it refuses; then the darkened brightness
//! must be at most the level's light level, sampled: the overworld's uniform 0 to 7 (a
//! `nextInt(8)`), the Nether's constant 7 and the End's constant 15 (no draw).

use clustine_data::{BlockState, blocks, entity_types};
use clustine_world::generation::Draws;
use clustine_world::{Biome, BlockPos, Dimension};

use crate::spawning::rules::{Light, SpawnWorld, check_spawn_rules};

/// Where the zombie of these tests would spawn, on stone.
const AT: BlockPos = BlockPos::new(0, 41, 0);

/// One position of a level of `dimension`, with the light `light` there.
struct Place {
    dimension: Dimension,
    light: Light,
}

impl SpawnWorld for Place {
    fn block(&self, pos: BlockPos) -> Option<BlockState> {
        Some(if pos == AT.offset(0, -1, 0) {
            blocks::STONE
        } else {
            blocks::AIR
        })
    }

    fn light(&self, _: BlockPos) -> Option<Light> {
        Some(self.light)
    }

    /// The level's own darkening (`15 − sky_light_level`): the Nether's 11, none in the
    /// End, and none at the overworld's noon.
    fn sky_darken(&self) -> i32 {
        if self.dimension == Dimension::Nether {
            11
        } else {
            0
        }
    }

    fn biome(&self, _: BlockPos) -> Option<Biome> {
        Some(Biome(0))
    }

    fn surface(&self, _: i32, _: i32) -> Option<i32> {
        Some(AT.y)
    }

    fn game_seed(&self) -> i64 {
        0
    }

    fn day(&self) -> u64 {
        6_000
    }

    fn dimension(&self) -> Dimension {
        self.dimension
    }
}

/// Draws that answer every `nextInt(bound)` with its highest, `bound − 1` (so the sky's
/// test and the overworld's sample let the darkest pass), and note each bound drawn.
#[derive(Default)]
struct Highest {
    bounds: Vec<u32>,
}

impl Draws for Highest {
    fn next_int(&mut self, bound: u32) -> u32 {
        self.bounds.push(bound);
        bound - 1
    }

    fn next_float(&mut self) -> f32 {
        0.999
    }

    fn next_double(&mut self) -> f64 {
        0.999
    }

    fn next_bool(&mut self) -> bool {
        true
    }
}

/// Whether a zombie may spawn at `AT` in `dimension` with the sky's light `sky` and the
/// blocks' `block`, and the bounds of the ints drawn for it.
fn zombie_may_spawn(dimension: Dimension, sky: u8, block: u8) -> (bool, Vec<u32>) {
    let place = Place {
        dimension,
        light: Light { sky, block },
    };
    let mut draws = Highest::default();
    let kind = u16::try_from(entity_types::ZOMBIE).expect("an id");
    let may = check_spawn_rules(kind, &place, AT, &mut draws);
    (may, draws.bounds)
}

/// Brief 1.1: the overworld's limit of the blocks' light is 0 and its light level a
/// uniform 0 to 7, drawn: a zombie spawns in the dark, after two draws, and never where
/// a block gives any light.
#[test]
fn in_the_overworld_any_block_light_keeps_a_monster_from_spawning() {
    assert_eq!(
        zombie_may_spawn(Dimension::Overworld, 0, 0),
        (true, vec![32, 8])
    );
    assert!(!zombie_may_spawn(Dimension::Overworld, 0, 1).0);
}

/// Brief 1.2: the Nether rejects nothing by the blocks' light (its limit is 15) and
/// compares the brightness, its blocks' light alone, with its constant 7, drawing no
/// sample: a zombie spawns at a block light of 5 or 7 and not at 8 or 15, after one draw.
#[test]
fn in_the_nether_a_monster_spawns_at_a_block_light_up_to_seven() {
    for (block, may) in [(0, true), (5, true), (7, true), (8, false), (15, false)] {
        let (spawned, bounds) = zombie_may_spawn(Dimension::Nether, 0, block);
        assert_eq!(spawned, may, "a block light of {block}");
        if may {
            assert_eq!(bounds, [32], "no sample drawn for the constant 7");
        }
    }
}

/// Brief 1.2: the End rejects any block light above its limit of 0 and accepts every
/// brightness up to its constant 15, drawing no sample: a zombie spawns under the full
/// sky (where the sky's own `nextInt(32)` lets it) and never beside a light.
#[test]
fn in_the_end_a_monster_spawns_under_the_full_sky_and_never_by_a_light() {
    assert_eq!(zombie_may_spawn(Dimension::End, 15, 0), (true, vec![32]));
    assert!(!zombie_may_spawn(Dimension::End, 15, 1).0);
    assert!(!zombie_may_spawn(Dimension::End, 0, 1).0);
}
