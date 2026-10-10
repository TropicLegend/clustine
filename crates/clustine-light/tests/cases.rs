//! Hand-built worlds and the light the game's rules give them.

use clustine_data::{BlockState, Face, blocks};
use clustine_light::{Blocks, ChunkLight, SectionLight, light};

/// Nine chunks of a small world, 64 blocks high from y 0, all air to begin with.
#[derive(Clone)]
struct World {
    height: usize,
    sky: bool,
    /// By `(chunk_z + 1) * 3 + chunk_x + 1`.
    present: [bool; 9],
    states: Vec<BlockState>,
}

impl World {
    fn new() -> World {
        World::of_height(64)
    }

    fn of_height(height: usize) -> World {
        World {
            height,
            sky: true,
            present: [true; 9],
            states: vec![blocks::AIR; 48 * 48 * height],
        }
    }

    fn without_sky(mut self) -> World {
        self.sky = false;
        self
    }

    fn without_chunk(mut self, chunk_x: i32, chunk_z: i32) -> World {
        self.present[((chunk_z + 1) * 3 + chunk_x + 1) as usize] = false;
        self
    }

    fn index(&self, x: i32, y: i32, z: i32) -> usize {
        assert!((-16..32).contains(&x) && (-16..32).contains(&z));
        assert!((0..self.height as i32).contains(&y));
        (y as usize * 48 + (z + 16) as usize) * 48 + (x + 16) as usize
    }

    fn get(&self, x: i32, y: i32, z: i32) -> BlockState {
        self.states[self.index(x, y, z)]
    }

    fn set(&mut self, x: i32, y: i32, z: i32, state: BlockState) {
        let index = self.index(x, y, z);
        self.states[index] = state;
    }

    /// Fills the box from `from` up to but not including `to`.
    fn fill(&mut self, from: [i32; 3], to: [i32; 3], state: BlockState) {
        for y in from[1]..to[1] {
            for z in from[2]..to[2] {
                for x in from[0]..to[0] {
                    self.set(x, y, z, state);
                }
            }
        }
    }

    /// A whole layer of all nine chunks.
    fn layers(&mut self, from_y: i32, to_y: i32, state: BlockState) {
        self.fill([-16, from_y, -16], [32, to_y, 32], state);
    }
}

impl Blocks for World {
    fn min_y(&self) -> i32 {
        0
    }

    fn height(&self) -> u32 {
        self.height as u32
    }

    fn has_sky(&self) -> bool {
        self.sky
    }

    fn has_chunk(&self, chunk_x: i32, chunk_z: i32) -> bool {
        self.present[((chunk_z + 1) * 3 + chunk_x + 1) as usize]
    }

    fn state(&self, x: i32, y: i32, z: i32) -> BlockState {
        assert!(
            self.has_chunk(x.div_euclid(16), z.div_euclid(16)),
            "a chunk that is not there is not asked for its blocks"
        );
        self.get(x, y, z)
    }
}

fn state(text: &str) -> BlockState {
    BlockState::parse(text).unwrap()
}

/// The level at a block of the centre chunk; `None` where the section has no light.
fn at(sections: &[SectionLight], x: i32, y: i32, z: i32) -> Option<u8> {
    sections[(y / 16 + 1) as usize].level(x as usize, (y % 16) as usize, z as usize)
}

fn sky(light: &ChunkLight, x: i32, y: i32, z: i32) -> u8 {
    at(&light.sky, x, y, z).unwrap()
}

fn block(light: &ChunkLight, x: i32, y: i32, z: i32) -> u8 {
    at(&light.block, x, y, z).unwrap()
}

#[test]
fn an_open_flat_world_is_lit_from_the_sky_down_to_its_ground() {
    let mut world = World::new();
    world.layers(0, 4, blocks::STONE);
    let light = light(&world);

    // The cases are those of today's `clustine_world::light::sky_light` for the flat
    // world, which the official server's packet was compared with: dark below the
    // world, levels in the ground's section, full in the one above, nothing further up.
    assert_eq!(light.sky.len(), 6);
    assert_eq!(light.sky[0], SectionLight::Dark);
    assert!(matches!(light.sky[1], SectionLight::Levels(_)));
    assert_eq!(light.sky[2], SectionLight::Full);
    assert_eq!(light.sky[3..], [const { SectionLight::Absent }; 3]);
    for y in 0..16 {
        for z in 0..16 {
            for x in 0..16 {
                assert_eq!(sky(&light, x, y, z), if y < 4 { 0 } else { 15 });
            }
        }
    }
    // Block light is dark wherever the sky's is listed.
    assert_eq!(light.block[..3], [const { SectionLight::Dark }; 3]);
    assert_eq!(light.block[3..], [const { SectionLight::Absent }; 3]);
}

#[test]
fn a_world_of_air_has_no_light_sections_at_all() {
    let light = light(&World::new());
    assert_eq!(light.sky, vec![SectionLight::Absent; 6]);
    assert_eq!(light.block, vec![SectionLight::Absent; 6]);
}

#[test]
fn it_is_dark_under_a_roof_that_covers_everything() {
    let mut world = World::new();
    world.layers(0, 1, blocks::STONE);
    world.layers(10, 11, blocks::STONE);
    let light = light(&world);
    for y in 0..16 {
        assert_eq!(sky(&light, 8, y, 8), if y > 10 { 15 } else { 0 }, "y {y}");
    }
}

#[test]
fn light_comes_in_under_the_edge_of_a_roof_and_falls_by_one_a_block() {
    let mut world = World::new();
    world.layers(0, 1, blocks::STONE);
    world.fill([4, 10, -16], [32, 11, 32], blocks::STONE);
    let light = light(&world);
    for z in [0, 8, 15] {
        for y in 1..10 {
            for x in 0..16 {
                let expected = if x < 4 { 15 } else { (18 - x).max(0) as u8 };
                assert_eq!(sky(&light, x, y, z), expected, "{x} {y} {z}");
            }
        }
        for x in 0..16 {
            assert_eq!(sky(&light, x, 10, z), if x < 4 { 15 } else { 0 });
            assert_eq!(sky(&light, x, 11, z), 15);
        }
    }
}

#[test]
fn a_hole_of_one_block_in_a_roof_lets_one_column_of_full_light_through() {
    let mut world = World::new();
    world.layers(0, 1, blocks::STONE);
    world.layers(10, 11, blocks::STONE);
    world.set(8, 10, 8, blocks::AIR);
    let light = light(&world);
    for y in 1..=10 {
        assert_eq!(sky(&light, 8, y, 8), 15, "y {y}");
    }
    for y in 1..10 {
        for z in 0..16 {
            for x in 0..16 {
                let away = i32::abs(x - 8) + i32::abs(z - 8);
                assert_eq!(
                    sky(&light, x, y, z),
                    (15 - away).max(0) as u8,
                    "{x} {y} {z}"
                );
            }
        }
    }
    assert_eq!(sky(&light, 7, 10, 8), 0);
}

#[test]
fn a_torch_in_a_closed_room_lights_each_block_by_its_distance() {
    let mut world = World::new();
    world.fill([2, 2, 2], [14, 12, 14], blocks::STONE);
    world.fill([3, 3, 3], [13, 11, 13], blocks::AIR);
    world.set(8, 3, 8, blocks::TORCH);
    let light = light(&world);
    assert_eq!(block(&light, 8, 3, 8), 14);
    for y in 0..16 {
        for z in 0..16 {
            for x in 0..16 {
                let inside = (3..13).contains(&x) && (3..11).contains(&y) && (3..13).contains(&z);
                let away = i32::abs(x - 8) + i32::abs(y - 3) + i32::abs(z - 8);
                let expected = if inside { (14 - away).max(0) as u8 } else { 0 };
                assert_eq!(block(&light, x, y, z), expected, "{x} {y} {z}");
                if inside {
                    assert_eq!(sky(&light, x, y, z), 0, "{x} {y} {z}");
                }
            }
        }
    }
    // The levels at each distance along the floor, written out.
    let along: Vec<u8> = (8..13).map(|x| block(&light, x, 3, 8)).collect();
    assert_eq!(along, [14, 13, 12, 11, 10]);
}

#[test]
fn an_emitter_beside_the_border_lights_the_chunk_from_the_neighbour() {
    for (emitter, nearest) in [
        ([16, 5, 8], [15, 5, 8]),
        ([-1, 5, 8], [0, 5, 8]),
        ([8, 5, 16], [8, 5, 15]),
        ([8, 5, -1], [8, 5, 0]),
    ] {
        let mut world = World::new().without_sky();
        world.set(emitter[0], emitter[1], emitter[2], blocks::GLOWSTONE);
        let light = light(&world);
        for z in 0..16 {
            for x in 0..16 {
                let away = i32::abs(x - emitter[0]) + i32::abs(z - emitter[2]);
                assert_eq!(block(&light, x, 5, z), (15 - away).max(0) as u8, "{x} {z}");
            }
        }
        assert_eq!(block(&light, nearest[0], nearest[1], nearest[2]), 14);
    }
    // And from a chunk that touches at a corner only.
    let mut world = World::new().without_sky();
    world.set(16, 5, 16, blocks::GLOWSTONE);
    assert_eq!(block(&light(&world), 15, 5, 15), 13);
}

/// A wall through the centre chunk and its western neighbours that ends at the eastern
/// border, with an emitter against it at the border.
fn wall_to_the_border() -> World {
    let mut world = World::new().without_sky();
    world.fill([-16, 0, 9], [16, 64, 10], blocks::STONE);
    world.set(15, 30, 8, blocks::GLOWSTONE);
    world
}

#[test]
fn light_goes_round_a_wall_through_the_neighbour_and_comes_back() {
    let light = light(&wall_to_the_border());
    // Out to the east, two blocks south, and back in: four steps.
    assert_eq!(block(&light, 15, 30, 10), 11);
    assert_eq!(block(&light, 14, 30, 10), 10);
    assert_eq!(block(&light, 15, 30, 9), 0);
}

#[test]
fn a_neighbour_that_is_not_there_gives_no_light_and_takes_none() {
    let light_without = light(&wall_to_the_border().without_chunk(1, 0));
    // The way round the wall led through the chunk that is missing.
    assert_eq!(block(&light_without, 15, 30, 10), 0);
    assert_eq!(block(&light_without, 15, 30, 8), 15);
    assert_eq!(block(&light_without, 15, 30, 7), 14);

    // An emitter in a chunk that is not there is never looked at.
    let mut world = World::new().without_sky().without_chunk(1, 0);
    world.set(16, 5, 8, blocks::GLOWSTONE);
    assert_eq!(light(&world).block, vec![SectionLight::Absent; 6]);

    // The sky still comes down beside a chunk that is missing.
    let mut world = World::new().without_chunk(-1, 0).without_chunk(0, 1);
    world.layers(0, 4, blocks::STONE);
    let light = light(&world);
    assert_eq!(sky(&light, 0, 4, 15), 15);
    assert_eq!(sky(&light, 0, 3, 15), 0);
}

/// A shaft one block wide through stone, closed all round, with an emitter at its top,
/// and `inside` put at y 10 of it.
fn shaft_with(inside: BlockState) -> ChunkLight {
    let mut world = World::new().without_sky();
    world.fill([6, 0, 6], [11, 20, 11], blocks::STONE);
    world.fill([8, 1, 8], [9, 18, 9], blocks::AIR);
    world.set(8, 18, 8, blocks::GLOWSTONE);
    world.set(8, 10, 8, inside);
    light(&world)
}

#[test]
fn a_slab_stops_light_at_its_whole_face_only() {
    let open = shaft_with(blocks::AIR);
    assert_eq!(block(&open, 8, 11, 8), 8);
    assert_eq!(block(&open, 8, 10, 8), 7);
    assert_eq!(block(&open, 8, 9, 8), 6);

    // Light from above enters a lower slab and does not leave it downwards.
    let lower = shaft_with(state("minecraft:oak_slab[type=bottom]"));
    assert_eq!(block(&lower, 8, 11, 8), 8);
    assert_eq!(block(&lower, 8, 10, 8), 7);
    assert_eq!(block(&lower, 8, 9, 8), 0);

    // It does not enter an upper slab at all.
    let upper = shaft_with(state("minecraft:oak_slab[type=top]"));
    assert_eq!(block(&upper, 8, 11, 8), 8);
    assert_eq!(block(&upper, 8, 10, 8), 0);
    assert_eq!(block(&upper, 8, 9, 8), 0);
}

/// A corridor one block wide through stone along x at y 5 and z 8, closed all round,
/// with an emitter at its western end and the given blocks put into it.
fn corridor_with(inside: &[(i32, &str)]) -> Vec<u8> {
    let mut world = World::new().without_sky();
    world.fill([0, 3, 6], [16, 8, 11], blocks::STONE);
    world.fill([1, 5, 8], [15, 6, 9], blocks::AIR);
    world.set(1, 5, 8, blocks::GLOWSTONE);
    for (x, text) in inside {
        world.set(*x, 5, 8, state(text));
    }
    let light = light(&world);
    (1..10).map(|x| block(&light, x, 5, 8)).collect()
}

#[test]
fn two_halves_that_cover_a_face_together_stop_light_between_them() {
    assert_eq!(corridor_with(&[]), [15, 14, 13, 12, 11, 10, 9, 8, 7]);
    // One slab leaves half of each side open.
    let one = corridor_with(&[(6, "minecraft:oak_slab[type=bottom]")]);
    assert_eq!(one, [15, 14, 13, 12, 11, 10, 9, 8, 7]);
    // Two lower slabs leave the same half open.
    let same = corridor_with(&[
        (6, "minecraft:oak_slab[type=bottom]"),
        (7, "minecraft:oak_slab[type=bottom]"),
    ]);
    assert_eq!(same, [15, 14, 13, 12, 11, 10, 9, 8, 7]);
    // A lower and an upper slab close the face between them.
    let both = corridor_with(&[
        (6, "minecraft:oak_slab[type=bottom]"),
        (7, "minecraft:oak_slab[type=top]"),
    ]);
    assert_eq!(both, [15, 14, 13, 12, 11, 10, 0, 0, 0]);
}

#[test]
fn a_stair_stops_light_at_its_back_and_lets_it_past_its_step() {
    // The back of the stair is to the east: light enters over the step and stops.
    let away = corridor_with(&[(6, "minecraft:oak_stairs[facing=east,half=bottom]")]);
    assert_eq!(away, [15, 14, 13, 12, 11, 10, 0, 0, 0]);
    // The back is to the west: light does not enter.
    let towards = corridor_with(&[(6, "minecraft:oak_stairs[facing=west,half=bottom]")]);
    assert_eq!(towards, [15, 14, 13, 12, 11, 0, 0, 0, 0]);
    // The stair stands sideways: light passes its L-shaped sides.
    let sideways = corridor_with(&[(6, "minecraft:oak_stairs[facing=north,half=bottom]")]);
    assert_eq!(sideways, [15, 14, 13, 12, 11, 10, 9, 8, 7]);
}

/// A shaft through stone that is open to the sky, with `inside` at y 10 of it.
fn well_with(inside: BlockState) -> ChunkLight {
    let mut world = World::new();
    world.fill([6, 0, 6], [11, 20, 11], blocks::STONE);
    world.fill([8, 1, 8], [9, 20, 9], blocks::AIR);
    world.set(8, 10, 8, inside);
    light(&world)
}

#[test]
fn the_sky_comes_straight_down_only_through_what_dampens_nothing_and_has_open_faces() {
    let levels = |light: &ChunkLight| [11, 10, 9, 8].map(|y| sky(light, 8, y, 8));
    assert_eq!(levels(&well_with(blocks::AIR)), [15, 15, 15, 15]);
    assert_eq!(levels(&well_with(blocks::GLASS)), [15, 15, 15, 15]);
    // Into a lower slab and no further; not into an upper slab.
    let lower = well_with(state("minecraft:oak_slab[type=bottom]"));
    assert_eq!(levels(&lower), [15, 15, 0, 0]);
    let upper = well_with(state("minecraft:oak_slab[type=top]"));
    assert_eq!(levels(&upper), [15, 0, 0, 0]);
    // Water and leaves take one, and below them the sky's light is like any other.
    assert_eq!(levels(&well_with(blocks::WATER)), [15, 14, 13, 12]);
    assert_eq!(levels(&well_with(blocks::OAK_LEAVES)), [15, 14, 13, 12]);
    assert_eq!(levels(&well_with(blocks::STONE)), [15, 0, 0, 0]);
}

#[test]
fn water_and_leaves_dampen_the_sky_by_one_a_block() {
    let mut sea = World::new();
    sea.layers(0, 5, blocks::STONE);
    sea.layers(5, 10, blocks::WATER);
    let light_of_sea = light(&sea);
    let down: Vec<u8> = (4..=10)
        .rev()
        .map(|y| sky(&light_of_sea, 3, y, 12))
        .collect();
    assert_eq!(down, [15, 14, 13, 12, 11, 10, 0]);

    let mut wood = World::new();
    wood.layers(0, 1, blocks::STONE);
    wood.layers(20, 22, blocks::OAK_LEAVES);
    let light_of_wood = light(&wood);
    let down: Vec<u8> = (17..=22)
        .rev()
        .map(|y| sky(&light_of_wood, 3, y, 12))
        .collect();
    assert_eq!(down, [15, 14, 13, 12, 11, 10]);
    assert_eq!(sky(&light_of_wood, 3, 8, 12), 1);
    assert_eq!(sky(&light_of_wood, 3, 7, 12), 0);

    let mut greenhouse = World::new();
    greenhouse.layers(0, 1, blocks::STONE);
    greenhouse.layers(20, 22, blocks::GLASS);
    let light_of_greenhouse = light(&greenhouse);
    for y in 1..24 {
        assert_eq!(sky(&light_of_greenhouse, 3, y, 12), 15);
    }
}

#[test]
fn light_never_passes_a_block_that_dampens_fully() {
    assert_eq!(blocks::STONE.props().light_dampening(), 15);
    let mut world = World::new();
    world.layers(0, 1, blocks::STONE);
    // A closed box of stone one block thick, under the open sky, with emitters against
    // it on every side.
    world.fill([4, 4, 4], [12, 12, 12], blocks::STONE);
    world.fill([5, 5, 5], [11, 11, 11], blocks::AIR);
    for emitter in [
        [3, 8, 8],
        [12, 8, 8],
        [8, 3, 8],
        [8, 12, 8],
        [8, 8, 3],
        [8, 8, 12],
    ] {
        world.set(emitter[0], emitter[1], emitter[2], blocks::GLOWSTONE);
    }
    let light = light(&world);
    for y in 4..12 {
        for z in 4..12 {
            for x in 4..12 {
                assert_eq!(sky(&light, x, y, z), 0, "{x} {y} {z}");
                assert_eq!(block(&light, x, y, z), 0, "{x} {y} {z}");
            }
        }
    }
    // An emitter that dampens fully still has its own level, and no sky in it.
    assert_eq!(block(&light, 3, 8, 8), 15);
    assert_eq!(sky(&light, 3, 8, 8), 0);
    assert_eq!(block(&light, 2, 8, 8), 14);
    assert_eq!(sky(&light, 2, 8, 8), 15);
}

#[test]
fn a_dimension_without_a_sky_has_block_light_only() {
    let mut world = World::new().without_sky();
    world.layers(0, 4, blocks::STONE);
    world.set(8, 4, 8, blocks::TORCH);
    let light = light(&world);
    assert_eq!(light.sky, vec![SectionLight::Absent; 6]);
    assert_eq!(light.block[0], SectionLight::Dark);
    assert!(matches!(light.block[1], SectionLight::Levels(_)));
    // The torch at y 4 gives 14, which is 1 thirteen blocks above it, in the next section.
    assert_eq!(block(&light, 8, 17, 8), 1);
    assert_eq!(block(&light, 8, 18, 8), 0);
    assert_eq!(light.block[3..], [const { SectionLight::Absent }; 3]);
    assert_eq!(block(&light, 8, 5, 8), 13);
}

#[test]
fn a_section_has_light_if_it_or_one_of_the_26_around_it_holds_a_block() {
    // One block in the chunk to the north-west, in the world's third section.
    let mut world = World::new();
    world.set(-1, 40, -1, blocks::STONE);
    let lit = light(&world);
    let expected_sky = [
        SectionLight::Absent,
        SectionLight::Absent,
        SectionLight::Full,
        SectionLight::Full,
        SectionLight::Full,
        SectionLight::Absent,
    ];
    assert_eq!(lit.sky, expected_sky);
    let listed: Vec<bool> = lit.block.iter().map(|s| *s == SectionLight::Dark).collect();
    assert_eq!(listed, [false, false, true, true, true, false]);

    // Cave air is air: it makes no section hold anything.
    let mut world = World::new();
    world.layers(0, 64, blocks::CAVE_AIR);
    assert_eq!(light(&world).sky, vec![SectionLight::Absent; 6]);

    // A block in the top section lists the section above the world, one in the lowest
    // the section below it.
    let mut world = World::new();
    world.set(8, 63, 8, blocks::STONE);
    world.set(8, 0, 8, blocks::STONE);
    let light = light(&world);
    assert_eq!(light.sky[5], SectionLight::Full);
    assert!(matches!(light.sky[0], SectionLight::Levels(_)));
    // Below the world the sky comes down beside the block's column and creeps under it.
    assert_eq!(light.sky[0].level(7, 15, 8), Some(15));
    assert_eq!(light.sky[0].level(8, 15, 8), Some(14));
}

#[test]
fn an_array_of_one_level_is_dark_or_full_and_never_stored() {
    assert_eq!(
        SectionLight::from_array(Box::new([0; 2048])),
        SectionLight::Dark
    );
    assert_eq!(
        SectionLight::from_array(Box::new([0xff; 2048])),
        SectionLight::Full
    );
    let mut one = Box::new([0xff; 2048]);
    one[17] = 0xf7;
    let mixed = SectionLight::from_array(one.clone());
    assert_eq!(mixed, SectionLight::Levels(one.clone()));
    // Index 34 is x 2, z 2, y 0: the low half of byte 17.
    assert_eq!(mixed.level(2, 0, 2), Some(7));
    assert_eq!(mixed.level(3, 0, 2), Some(15));
    assert_eq!(mixed.to_array(), Some(one));
    assert_eq!(SectionLight::Absent.to_array(), None);
    assert_eq!(SectionLight::Full.to_array(), Some(Box::new([0xff; 2048])));
    assert_eq!(SectionLight::Dark.level(0, 0, 0), Some(0));
}

// ---------------------------------------------------------------------------------
// Random worlds against the rules written down a second time, slowly.

/// A generator of numbers that is the same everywhere.
struct Numbers(u64);

impl Numbers {
    fn below(&mut self, bound: u32) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as u32) % bound
    }
}

fn random_world(seed: u64, height: usize) -> World {
    let shapes = [
        "minecraft:oak_slab[type=bottom]",
        "minecraft:oak_slab[type=top]",
        "minecraft:oak_stairs[facing=east,half=bottom]",
        "minecraft:oak_stairs[facing=west,half=top]",
        "minecraft:oak_stairs[facing=north,half=bottom,shape=inner_left]",
        "minecraft:oak_stairs[facing=south,half=top,shape=outer_right]",
        "minecraft:snow[layers=3]",
        "minecraft:farmland",
        "minecraft:dirt_path",
        "minecraft:oak_slab[type=bottom,waterlogged=true]",
    ]
    .map(state);
    let mut numbers = Numbers(seed);
    let mut world = World::of_height(height);
    for index in 0..world.states.len() {
        world.states[index] = match numbers.below(1000) {
            0..520 => blocks::AIR,
            520..780 => blocks::STONE,
            780..820 => blocks::GLASS,
            820..860 => blocks::WATER,
            860..900 => blocks::OAK_LEAVES,
            900..985 => shapes[numbers.below(shapes.len() as u32) as usize],
            985..993 => blocks::TORCH,
            993..997 => blocks::GLOWSTONE,
            _ => blocks::CAVE_AIR,
        };
    }
    world
}

/// Whether the rectangles of the two faces leave nothing of the face open, found by
/// looking at the middle of each of 32 by 32 squares.
fn faces_cover(near: clustine_data::FaceShape, far: clustine_data::FaceShape) -> bool {
    let rectangles: Vec<[f64; 4]> = near.rectangles().chain(far.rectangles()).collect();
    (0..32).all(|u| {
        (0..32).all(|v| {
            let (u, v) = ((f64::from(u) + 0.5) / 32.0, (f64::from(v) + 0.5) / 32.0);
            rectangles
                .iter()
                .any(|r| r[0] < u && u < r[2] && r[1] < v && v < r[3])
        })
    })
}

fn stops(from: BlockState, to: BlockState, towards: Face) -> bool {
    let shape = |state: BlockState, face: Face| {
        let props = state.props();
        if props.occludes() && props.occludes_by_shape() {
            props.occlusion_face(face)
        } else {
            clustine_data::FaceShape::EMPTY
        }
    };
    faces_cover(shape(from, towards), shape(to, towards.opposite()))
}

/// The light of the centre chunk by the rules as the head of the crate states them,
/// swept over all nine chunks until nothing changes. One level for each block from one
/// section below the world to one above it, by `(y * 16 + z) * 16 + x`.
fn slowly(world: &World, of_sky: bool) -> Vec<u8> {
    let tall = world.height as i32 + 32;
    let at = |x: i32, y: i32, z: i32| ((y * 48 + z + 16) * 48 + x + 16) as usize;
    // `None` is a block of a chunk that is not there.
    let state_at = |x: i32, y: i32, z: i32| -> Option<BlockState> {
        if !(-16..32).contains(&x) || !(-16..32).contains(&z) || !(0..tall).contains(&y) {
            return None;
        }
        if !world.has_chunk(x.div_euclid(16), z.div_euclid(16)) {
            return None;
        }
        if y < 16 || y >= tall - 16 {
            return Some(blocks::AIR);
        }
        Some(world.get(x, y - 16, z))
    };
    let mut levels = vec![0u8; (tall * 48 * 48) as usize];
    let mut fixed = vec![false; levels.len()];
    for z in -16..32 {
        for x in -16..32 {
            if of_sky {
                let mut above = blocks::AIR;
                for y in (0..tall).rev() {
                    let Some(here) = state_at(x, y, z) else { break };
                    if here.props().light_dampening() != 0 || stops(above, here, Face::Down) {
                        break;
                    }
                    levels[at(x, y, z)] = 15;
                    fixed[at(x, y, z)] = true;
                    above = here;
                }
            } else {
                for y in 0..tall {
                    if let Some(here) = state_at(x, y, z) {
                        levels[at(x, y, z)] = here.props().light_emission();
                    }
                }
            }
        }
    }
    let steps = [
        (Face::Down, [0, -1, 0]),
        (Face::Up, [0, 1, 0]),
        (Face::North, [0, 0, -1]),
        (Face::South, [0, 0, 1]),
        (Face::West, [-1, 0, 0]),
        (Face::East, [1, 0, 0]),
    ];
    loop {
        let mut changed = false;
        for y in 0..tall {
            for z in -16..32 {
                for x in -16..32 {
                    let Some(here) = state_at(x, y, z) else {
                        continue;
                    };
                    if fixed[at(x, y, z)] {
                        continue;
                    }
                    let takes = here.props().light_dampening().max(1);
                    let mut level = levels[at(x, y, z)];
                    for (face, [dx, dy, dz]) in steps {
                        // `face` is the way from here to the neighbour; light comes
                        // the other way.
                        let (nx, ny, nz) = (x + dx, y + dy, z + dz);
                        let Some(neighbour) = state_at(nx, ny, nz) else {
                            continue;
                        };
                        let offered = levels[at(nx, ny, nz)].saturating_sub(takes);
                        if offered > level && !stops(neighbour, here, face.opposite()) {
                            level = offered;
                        }
                    }
                    if level != levels[at(x, y, z)] {
                        levels[at(x, y, z)] = level;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
    let mut centre = Vec::new();
    for y in 0..tall {
        for z in 0..16 {
            for x in 0..16 {
                centre.push(levels[at(x, y, z)]);
            }
        }
    }
    centre
}

fn assert_equal_to_slowly(world: &World, what: &str) {
    let light = light(world);
    for (of_sky, sections) in [(true, &light.sky), (false, &light.block)] {
        if of_sky && !world.sky {
            continue;
        }
        let expected = slowly(world, of_sky);
        for (section, made) in sections.iter().enumerate() {
            for y in 0..16 {
                for z in 0..16 {
                    for x in 0..16 {
                        let Some(level) = made.level(x, y, z) else {
                            continue;
                        };
                        let slow = expected[((section * 16 + y) * 16 + z) * 16 + x];
                        assert_eq!(
                            level, slow,
                            "{what}, sky {of_sky}: section {section} at {x} {y} {z}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn random_worlds_are_lit_as_the_rules_swept_slowly_light_them() {
    for seed in 1..=3 {
        let world = random_world(seed, 32);
        assert!(light(&world).sky.iter().all(|s| *s != SectionLight::Absent));
        assert_equal_to_slowly(&world, &format!("seed {seed}"));
    }
    let world = random_world(4, 32)
        .without_chunk(1, 0)
        .without_chunk(-1, -1)
        .without_chunk(0, 1);
    assert_equal_to_slowly(&world, "with chunks missing");
}

#[test]
fn the_light_of_a_chunk_does_not_depend_on_blocks_fifteen_or_more_away() {
    let near = random_world(7, 32);
    let mut far = near.clone();
    let other = random_world(8, 32);
    for y in 0..32 {
        for z in -16..32 {
            for x in -16..32 {
                let beyond = |at: i32| (-at).max(at - 15).max(0);
                if beyond(x) + beyond(z) >= 15 {
                    far.set(x, y, z, other.get(x, y, z));
                }
            }
        }
    }
    assert!(near.states != far.states);
    assert_eq!(light(&near), light(&far));
    // One block nearer it does: an emitter of 15 fourteen blocks away gives 1.
    let mut dark = World::new().without_sky();
    dark.set(29, 5, 8, blocks::GLOWSTONE);
    assert_eq!(block(&light(&dark), 15, 5, 8), 1);
}

#[test]
fn the_same_blocks_give_the_same_light_however_they_are_handed_over() {
    /// The same world, which says of its uniform sections that they are.
    struct Knowing(World);
    impl Blocks for Knowing {
        fn min_y(&self) -> i32 {
            -64
        }
        fn height(&self) -> u32 {
            self.0.height()
        }
        fn has_sky(&self) -> bool {
            true
        }
        fn has_chunk(&self, chunk_x: i32, chunk_z: i32) -> bool {
            self.0.has_chunk(chunk_x, chunk_z)
        }
        fn state(&self, x: i32, y: i32, z: i32) -> BlockState {
            self.0.get(x, y + 64, z)
        }
        fn uniform_section(
            &self,
            chunk_x: i32,
            chunk_z: i32,
            section: usize,
        ) -> Option<BlockState> {
            let first = self.0.get(chunk_x * 16, section as i32 * 16, chunk_z * 16);
            let uniform = (0..4096).all(|index| {
                let (x, y, z) = (index & 15, index >> 8, index >> 4 & 15);
                self.0
                    .get(chunk_x * 16 + x, section as i32 * 16 + y, chunk_z * 16 + z)
                    == first
            });
            uniform.then_some(first)
        }
    }
    let mut world = random_world(11, 64);
    world.layers(16, 32, blocks::WATER);
    world.layers(32, 48, blocks::AIR);
    world.fill([0, 48, 0], [16, 64, 16], blocks::GLOWSTONE);
    let plain = light(&world);
    assert_eq!(plain, light(&world));
    assert_eq!(plain, light(&Knowing(world)));
}
