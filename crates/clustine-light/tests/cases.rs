//! Hand-built worlds and the light the game's rules give them.
//!
//! The worlds are plain arrays of blocks. [`World::lit`] makes the nine chunks of one
//! and hands them to the crate as anyone would, so that every case goes through
//! [`Around`] and through a chunk's packed sections.

use clustine_data::{BlockState, Face, blocks};
use clustine_light::{Around, VERSION};
use clustine_world::{Biome, Chunk, LIGHT_ARRAY_LENGTH, Light, Section, SectionLight, sky_light};

/// How the blocks of a test's world are put into chunks. The light may not depend on it.
#[derive(Clone, Copy, PartialEq)]
enum Built {
    /// Each section from its 4,096 states at once: a section of one state is uniform.
    FromStates,
    /// Block by block into chunks of air: no section but one of air says of itself
    /// that it is uniform, and every other has air in its palette, used or not.
    BlockByBlock,
}

/// The chunks of a small world, 64 blocks high from y 0, all air to begin with: a
/// centre chunk with `reach` chunks to each side of it. Blocks are named by `x` and `z`
/// from the centre chunk's north-west corner, so that nine chunks span -16 to 31.
#[derive(Clone)]
struct World {
    height: usize,
    /// The `y` of the lowest block as the chunks have it. The tests count from 0
    /// whatever it is.
    min_y: i32,
    reach: i32,
    sky: bool,
    /// Of the nine chunks light is made from, by `(chunk_z + 1) * 3 + chunk_x + 1`.
    present: [bool; 9],
    states: Vec<BlockState>,
}

/// The offsets of the eight neighbours in the order [`Around`] names them: north-west,
/// north, north-east, west, east, south-west, south, south-east. North is negative z.
const NEIGHBOURS: [(i32, i32); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

impl World {
    fn new() -> World {
        World::of_height(64)
    }

    fn of_height(height: usize) -> World {
        World::reaching(1, height)
    }

    /// A world of `reach` chunks to each side of the centre chunk. Light is made from
    /// the inner nine whatever the reach.
    fn reaching(reach: i32, height: usize) -> World {
        let width = (2 * reach as usize + 1) * 16;
        World {
            height,
            min_y: 0,
            reach,
            sky: true,
            present: [true; 9],
            states: vec![blocks::AIR; width * width * height],
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

    /// Whether one of the nine chunks is there. Chunks further out always are.
    fn has_chunk(&self, chunk_x: i32, chunk_z: i32) -> bool {
        if chunk_x.abs() > 1 || chunk_z.abs() > 1 {
            return true;
        }
        self.present[((chunk_z + 1) * 3 + chunk_x + 1) as usize]
    }

    /// The lowest `x` and `z` of the world, and the one beyond the highest.
    fn span(&self) -> std::ops::Range<i32> {
        -16 * self.reach..16 * (self.reach + 1)
    }

    fn width(&self) -> usize {
        self.span().len()
    }

    fn index(&self, x: i32, y: i32, z: i32) -> usize {
        assert!(self.span().contains(&x) && self.span().contains(&z));
        assert!((0..self.height as i32).contains(&y));
        let low = self.span().start;
        (y as usize * self.width() + (z - low) as usize) * self.width() + (x - low) as usize
    }

    /// One of the world's chunks, by its offset from the centre.
    fn chunk(&self, chunk_x: i32, chunk_z: i32, built: Built) -> Chunk {
        let sections = self.height / 16;
        let air = vec![Section::filled(blocks::AIR, Biome(0)); sections];
        let mut chunk = Chunk::from_sections(self.min_y, air);
        if built == Built::BlockByBlock {
            for y in 0..self.height as i32 {
                for z in 0..16 {
                    for x in 0..16 {
                        let state = self.get(chunk_x * 16 + x, y, chunk_z * 16 + z);
                        chunk.set(x as usize, self.min_y + y, z as usize, state);
                    }
                }
            }
            return chunk;
        }
        let sections = (0..sections as i32).map(|section| {
            let states: Vec<BlockState> = (0..4096)
                .map(|at| {
                    let (x, y, z) = (at & 15, at >> 8, at >> 4 & 15);
                    self.get(chunk_x * 16 + x, section * 16 + y, chunk_z * 16 + z)
                })
                .collect();
            let states: Box<[BlockState; 4096]> = states.into_boxed_slice().try_into().unwrap();
            Section::from_states(states, Biome(0))
        });
        Chunk::from_sections(self.min_y, sections.collect())
    }

    /// The light of the centre chunk, from the nine chunks built in the given way. A
    /// chunk that is not there is not built at all, so nothing can ask for its blocks.
    fn lit_as(&self, built: Built) -> Light {
        let centre = self.chunk(0, 0, built);
        let neighbours = NEIGHBOURS.map(|(chunk_x, chunk_z)| {
            self.has_chunk(chunk_x, chunk_z)
                .then(|| self.chunk(chunk_x, chunk_z, built))
        });
        clustine_light::light(&Around {
            centre: &centre,
            neighbours: neighbours.each_ref().map(Option::as_ref),
            sky: self.sky,
        })
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

    /// A whole layer of all the world's chunks.
    fn layers(&mut self, from_y: i32, to_y: i32, state: BlockState) {
        let span = self.span();
        self.fill(
            [span.start, from_y, span.start],
            [span.end, to_y, span.end],
            state,
        );
    }
}

/// The light of the centre chunk of a world.
fn light(world: &World) -> Light {
    world.lit_as(Built::FromStates)
}

fn state(text: &str) -> BlockState {
    BlockState::parse(text).unwrap()
}

/// The level at a position within a light section, each coordinate below 16: the
/// levels are four bits a block by `y << 8 | z << 4 | x`, the even index in the low
/// half of its byte. `None` for a section that has no light of its own.
fn level(light: &SectionLight, x: usize, y: usize, z: usize) -> Option<u8> {
    assert!(x < 16 && y < 16 && z < 16, "a position within a section");
    match light {
        SectionLight::Absent => None,
        SectionLight::Dark => Some(0),
        SectionLight::Full => Some(15),
        SectionLight::Levels(levels) => {
            let index = y << 8 | z << 4 | x;
            Some(levels[index / 2] >> (index % 2 * 4) & 15)
        }
    }
}

/// The level at a block of the centre chunk; `None` where the section has no light.
fn at(sections: &[SectionLight], x: i32, y: i32, z: i32) -> Option<u8> {
    level(
        &sections[(y / 16 + 1) as usize],
        x as usize,
        (y % 16) as usize,
        z as usize,
    )
}

fn sky(light: &Light, x: i32, y: i32, z: i32) -> u8 {
    at(&light.sky, x, y, z).unwrap()
}

fn block(light: &Light, x: i32, y: i32, z: i32) -> u8 {
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
fn shaft_with(inside: BlockState) -> Light {
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
fn well_with(inside: BlockState) -> Light {
    let mut world = World::new();
    world.fill([6, 0, 6], [11, 20, 11], blocks::STONE);
    world.fill([8, 1, 8], [9, 20, 9], blocks::AIR);
    world.set(8, 10, 8, inside);
    light(&world)
}

#[test]
fn the_sky_comes_straight_down_only_through_what_dampens_nothing_and_has_open_faces() {
    let levels = |light: &Light| [11, 10, 9, 8].map(|y| sky(light, 8, y, 8));
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
    assert_eq!(level(&light.sky[0], 7, 15, 8), Some(15));
    assert_eq!(level(&light.sky[0], 8, 15, 8), Some(14));
}

/// Whether an entry is in its one form: levels are never all 0 and never all 15.
fn in_its_one_form(light: &SectionLight) -> bool {
    match light {
        SectionLight::Levels(levels) => {
            levels.iter().any(|byte| *byte != 0) && levels.iter().any(|byte| *byte != 0xff)
        }
        _ => true,
    }
}

#[test]
fn a_section_that_is_all_15_is_full_and_one_that_is_all_0_is_dark() {
    // Stone in the two lowest sections, and a torch in the middle of the fourth, which
    // the sky passes through.
    let mut world = World::new();
    world.layers(0, 32, blocks::STONE);
    world.set(8, 56, 8, blocks::TORCH);
    let made = light(&world);
    // The sky reaches nothing of the stone and nothing below the world, and all of
    // what is above the stone.
    assert_eq!(made.sky[..3], [const { SectionLight::Dark }; 3]);
    assert_eq!(made.sky[3..], [const { SectionLight::Full }; 3]);
    // The torch's light ends 13 blocks below it, at y 43 of the third section, and
    // never comes near the stone.
    assert_eq!(made.block[..3], [const { SectionLight::Dark }; 3]);
    assert!(matches!(made.block[3], SectionLight::Levels(_)));
    assert_eq!(block(&made, 8, 43, 8), 1);
    assert_eq!(block(&made, 8, 42, 8), 0);
    assert!(matches!(made.block[4], SectionLight::Levels(_)));
    assert!(matches!(made.block[5], SectionLight::Levels(_)));
    assert!(made.sky.iter().chain(&made.block).all(in_its_one_form));

    // A section of lamps is full of block light, and no array stands for that.
    let mut lamps = World::new().without_sky();
    lamps.layers(16, 32, blocks::GLOWSTONE);
    assert_eq!(light(&lamps).block[2], SectionLight::Full);

    // Nor does one in any light of a world of everything.
    let random = light(&random_world(5, 32));
    assert!(random.sky.iter().chain(&random.block).all(in_its_one_form));
}

#[test]
fn levels_are_four_bits_a_block_by_y_then_z_then_x_with_the_even_index_low() {
    let mut world = World::new().without_sky();
    world.set(2, 16, 3, blocks::GLOWSTONE);
    let light = light(&world);
    let SectionLight::Levels(levels) = &light.block[2] else {
        panic!("levels, not {:?}", light.block[2]);
    };
    assert_eq!(levels.len(), LIGHT_ARRAY_LENGTH);
    // The lamp is at index 3 << 4 | 2 = 50 of its section: the low half of byte 25,
    // whose high half is the block to the east of it.
    assert_eq!(levels[25], 14 << 4 | 15);
    // One to the south is 16 further, one up 256 further.
    assert_eq!(levels[33], 13 << 4 | 14);
    assert_eq!(levels[128 + 25], 13 << 4 | 14);
    // The byte before holds x 0 and 1 of the lamp's row.
    assert_eq!(levels[24], 14 << 4 | 13);
}

#[test]
fn light_says_which_version_of_the_rules_made_it_and_has_an_entry_for_each_section() {
    for height in [16, 64, 384] {
        let mut world = World::of_height(height);
        world.min_y = -64;
        world.layers(0, 1, blocks::STONE);
        let light = light(&world);
        assert_eq!(light.made_by, VERSION);
        assert_eq!(light.sky.len(), height / 16 + 2);
        assert_eq!(light.block.len(), height / 16 + 2);
    }
}

/// Test 6 of ADR-0022: until the edge's packet is made from what a chunk carries it is
/// made from `sky_light`, and the flat world must not look otherwise afterwards.
#[test]
fn the_flat_chunk_gives_exactly_the_sky_light_that_is_reckoned_by_columns_today() {
    // The overworld's heights, and the layers of the classic flat world.
    let mut world = World::of_height(384);
    world.min_y = -64;
    world.layers(0, 1, blocks::BEDROCK);
    world.layers(1, 3, blocks::DIRT);
    world.layers(3, 4, blocks::GRASS_BLOCK);
    for built in [Built::FromStates, Built::BlockByBlock] {
        let made = world.lit_as(built);
        let by_columns = sky_light(&world.chunk(0, 0, built));
        assert_eq!(by_columns.len(), 26);
        for (entry, (made, by_columns)) in made.sky.iter().zip(&by_columns).enumerate() {
            assert_eq!(made, by_columns, "the sky's entry {entry}");
        }
        assert_eq!(made.sky, by_columns);
        // What the two agree on, written out.
        assert_eq!(made.sky[0], SectionLight::Dark);
        assert!(matches!(made.sky[1], SectionLight::Levels(_)));
        assert_eq!(made.sky[2], SectionLight::Full);
        assert_eq!(made.sky[3..], [const { SectionLight::Absent }; 23]);
        // And block light as the edge's packet has it today: dark wherever the sky's
        // is listed.
        assert_eq!(made.block[..3], [const { SectionLight::Dark }; 3]);
        assert_eq!(made.block[3..], [const { SectionLight::Absent }; 23]);
    }
}

#[test]
fn a_pillar_shades_its_own_column_and_nothing_beside_it() {
    let mut world = World::new();
    world.layers(0, 1, blocks::STONE);
    world.fill([3, 1, 5], [4, 40, 6], blocks::STONE);
    let light = light(&world);
    for y in 1..48 {
        assert_eq!(sky(&light, 3, y, 5), if y < 40 { 0 } else { 15 }, "y {y}");
        for (x, z) in [(2, 5), (4, 5), (3, 4), (3, 6)] {
            assert_eq!(sky(&light, x, y, z), 15, "{x} {y} {z}");
        }
    }
    // The sections the pillar stands in are levels, the one above it is full.
    assert!(matches!(light.sky[1], SectionLight::Levels(_)));
    assert!(matches!(light.sky[3], SectionLight::Levels(_)));
    assert_eq!(light.sky[4], SectionLight::Full);
    assert_eq!(light.sky[5], SectionLight::Absent);
}

#[test]
fn a_torch_one_block_inside_a_neighbour_lights_the_chunk_across_the_border() {
    let mut world = World::new();
    world.layers(0, 1, blocks::STONE);
    world.layers(6, 7, blocks::STONE);
    world.set(-1, 1, 8, blocks::TORCH);
    let light = light(&world);
    for z in 0..16 {
        for x in 0..16 {
            // One step to the border, then by distance.
            let away = x + 1 + i32::abs(z - 8);
            assert_eq!(block(&light, x, 1, z), (14 - away).max(0) as u8, "{x} {z}");
        }
    }
    assert_eq!(block(&light, 0, 1, 8), 13);
    assert_eq!(block(&light, 0, 5, 8), 9);
    // Under the roof there is no light but the torch's.
    assert_eq!(sky(&light, 0, 1, 8), 0);
}

/// One lamp in one neighbour, just across the border or the corner, with each of the
/// eight chunks built by hand in the order [`Around`] names them.
#[test]
fn the_neighbours_are_named_from_the_north_west_row_by_row_and_north_is_negative_z() {
    let air = || Chunk::from_sections(0, vec![Section::filled(blocks::AIR, Biome(0)); 2]);
    let empty = air();
    // For each neighbour: where the lamp is in it, and the block of the centre chunk
    // that is nearest to it, with the steps between them.
    let cases: [(&str, [usize; 2], [usize; 2], u8); 8] = [
        ("north-west", [15, 15], [0, 0], 2),
        ("north", [8, 15], [8, 0], 1),
        ("north-east", [0, 15], [15, 0], 2),
        ("west", [15, 8], [0, 8], 1),
        ("east", [0, 8], [15, 8], 1),
        ("south-west", [15, 0], [0, 15], 2),
        ("south", [8, 0], [8, 15], 1),
        ("south-east", [0, 0], [15, 15], 2),
    ];
    for (slot, (name, lamp, nearest, steps)) in cases.into_iter().enumerate() {
        let mut lit = air();
        lit.set(lamp[0], 5, lamp[1], blocks::GLOWSTONE);
        let mut neighbours = [Some(&empty); 8];
        neighbours[slot] = Some(&lit);
        let light = clustine_light::light(&Around {
            centre: &empty,
            neighbours,
            sky: false,
        });
        let brightest = (0..256)
            .map(|column| (level(&light.block[1], column & 15, 5, column >> 4), column))
            .max()
            .unwrap();
        let column = nearest[1] << 4 | nearest[0];
        assert_eq!(brightest, (Some(15 - steps), column), "{name}");
    }
}

#[test]
#[should_panic(expected = "span the same heights")]
fn a_neighbour_of_another_height_is_refused() {
    let centre = Chunk::from_sections(0, vec![Section::filled(blocks::AIR, Biome(0)); 4]);
    let lower = Chunk::from_sections(0, vec![Section::filled(blocks::AIR, Biome(0)); 3]);
    let mut neighbours = [Some(&centre); 8];
    neighbours[6] = Some(&lower);
    clustine_light::light(&Around {
        centre: &centre,
        neighbours,
        sky: true,
    });
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
    random_world_reaching(1, seed, height)
}

fn random_world_reaching(reach: i32, seed: u64, height: usize) -> World {
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
    let mut world = World::reaching(reach, height);
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
/// swept over all the world's chunks, the nine or more, until nothing changes. One
/// level for each block from one section below the world to one above it, by
/// `(y * 16 + z) * 16 + x`.
fn slowly(world: &World, of_sky: bool) -> Vec<u8> {
    let tall = world.height as i32 + 32;
    let span = world.span();
    let (low, wide) = (span.start, span.len() as i32);
    let at = |x: i32, y: i32, z: i32| ((y * wide + z - low) * wide + x - low) as usize;
    // `None` is a block of a chunk that is not there.
    let state_at = |x: i32, y: i32, z: i32| -> Option<BlockState> {
        if !span.contains(&x) || !span.contains(&z) || !(0..tall).contains(&y) {
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
    let mut levels = vec![0u8; (tall * wide * wide) as usize];
    let mut fixed = vec![false; levels.len()];
    for z in span.clone() {
        for x in span.clone() {
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
            for z in span.clone() {
                for x in span.clone() {
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
                        let Some(level) = level(made, x, y, z) else {
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
    let mut world = random_world(11, 64);
    world.layers(16, 32, blocks::WATER);
    world.layers(32, 48, blocks::AIR);
    world.fill([0, 48, 0], [16, 64, 16], blocks::GLOWSTONE);
    let plain = light(&world);
    assert_eq!(plain, light(&world));

    // Sections that know they hold one state, and the same sections filled block by
    // block, which do not.
    let water = |built| world.chunk(0, 0, built).sections()[1].uniform_state();
    assert_eq!(water(Built::FromStates), Some(blocks::WATER));
    assert_eq!(water(Built::BlockByBlock), None);
    assert_eq!(plain, world.lit_as(Built::BlockByBlock));

    // The same blocks at another height of the world.
    let mut lower = world.clone();
    lower.min_y = -64;
    assert_eq!(plain, light(&lower));

    // Of a chunk only its blocks are read: not the light it carries, nor what else
    // it does.
    let mut alone = world.clone();
    alone.present = [false; 9];
    let mut centre = world.chunk(0, 0, Built::FromStates);
    centre.set_light(Light {
        made_by: 7,
        beside: 9,
        sky: vec![SectionLight::Full; 6],
        block: vec![SectionLight::Full; 6],
    });
    centre.mark_generated(3, true);
    let made = clustine_light::light(&Around {
        centre: &centre,
        neighbours: [None; 8],
        sky: true,
    });
    assert_eq!(made, light(&alone));
    assert_ne!(made, plain);
}

#[test]
fn stone_with_its_ores_is_lit_as_stone_is() {
    let build = |ground: &dyn Fn(i32) -> BlockState| {
        let mut world = World::new();
        for y in 0..20 {
            for z in world.span() {
                for x in world.span() {
                    world.set(x, y, z, ground(x + 3 * y + 7 * z));
                }
            }
        }
        // A pit down to the top of the lowest section, with a torch at its bottom and
        // a slab half way up, so that there is light against that section.
        world.fill([6, 16, 6], [10, 20, 10], blocks::AIR);
        world.set(7, 16, 7, blocks::TORCH);
        world.set(8, 18, 8, state("minecraft:oak_slab[type=top]"));
        world
    };
    let stone = build(&|_| blocks::STONE);
    let ores = build(&|at| match at.rem_euclid(5) {
        0 => state("minecraft:coal_ore"),
        1 => blocks::DIRT,
        2 => blocks::BEDROCK,
        _ => blocks::STONE,
    });
    // The lowest section holds four states and nothing light could tell apart.
    let centre = ores.chunk(0, 0, Built::FromStates);
    let lowest = &centre.sections()[0];
    assert_eq!(lowest.uniform_state(), None);
    assert_eq!(lowest.non_air_count(), 4096);
    let lit = light(&stone);
    assert_eq!(block(&lit, 7, 17, 7), 13);
    assert_eq!(block(&lit, 7, 15, 7), 0);
    assert_eq!(sky(&lit, 7, 16, 7), 15);
    assert_eq!(sky(&lit, 8, 17, 8), 14);
    assert_eq!(sky(&lit, 8, 15, 8), 0);
    for built in [Built::FromStates, Built::BlockByBlock] {
        assert_eq!(ores.lit_as(built), lit);
    }
}

/// Test 6 of ADR-0022. [`clustine_light::light`] is handed nine chunks and can read no
/// other, so by itself it cannot be shown a chunk two away. What can be shown is that
/// the nine are enough: the rules swept slowly over five chunks by five, where light
/// from the outer sixteen is free to come in, give the centre chunk the light that is
/// made from the inner nine alone, whatever the outer sixteen hold.
#[test]
fn the_light_of_a_chunk_does_not_change_when_a_chunk_two_away_changes() {
    let world = random_world_reaching(2, 21, 16);
    let made = light(&world);
    assert!(made.sky.iter().all(|s| *s != SectionLight::Absent));
    assert_equal_to_slowly(&world, "beside sixteen chunks of everything");

    // The sixteen chunks two away become lamps throughout: the brightest and the
    // darkest a chunk can be, at once.
    let mut changed = world.clone();
    for y in 0..16 {
        for z in world.span() {
            for x in world.span() {
                if x.div_euclid(16).abs() == 2 || z.div_euclid(16).abs() == 2 {
                    changed.set(x, y, z, blocks::GLOWSTONE);
                }
            }
        }
    }
    assert!(changed.states != world.states);
    for (chunk_x, chunk_z) in NEIGHBOURS {
        assert!(
            changed.chunk(chunk_x, chunk_z, Built::FromStates)
                == world.chunk(chunk_x, chunk_z, Built::FromStates)
        );
    }
    assert_equal_to_slowly(&changed, "beside sixteen chunks of lamps");
    assert_eq!(light(&changed), made);
}
