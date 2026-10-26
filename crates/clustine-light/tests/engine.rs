//! The light engine judged by its specification (`docs/groundwork/n8-plan.md`, sections
//! 3, 4.5, 10 part A and 14; ADR-0029), written by someone who did not write the engine.
//!
//! The judge is the whole-chunk function the crate already had: after every
//! [`Lighting::run`], every present chunk must hold what [`clustine_light::light`] gives
//! for it and its eight neighbours as present to the engine, an absent one a wall (the
//! plan's 3.1). It is asked twice, by [`Lighting::light`] against the function's light
//! and by [`Lighting::at`] at every block of the chunk and of the light sections below
//! and above it against the function's levels of every section, listed or not
//! ([`clustine_light::light_with`]), since the engine keeps those too (3.3, 3.6).
//!
//! The property tests draw worlds and histories from a seed. `CLUSTINE_LIGHT_ENGINE_CASES`
//! sets how many cases each property runs (by default [`CASES`]), and
//! `CLUSTINE_LIGHT_ENGINE_SEED` runs one case alone: a failure names its seed.

use std::collections::{BTreeMap, BTreeSet};

use clustine_data::{BlockState, blocks};
use clustine_light::engine::{Chunks, Kept, LightAt, Lighting};
use clustine_light::{Around, Scratch, Unlisted, light, light_with};
use clustine_world::{Biome, BlockPos, Chunk, ChunkPos, Light, Section, SectionLight};

/// Cases a property runs by default: chosen so that the whole file takes under twenty
/// seconds in a build without optimisation. 3,000 a property were run in a release
/// build when the file was written, and passed.
const CASES: u64 = 48;

// ---------------------------------------------------------------------------------------
// Numbers and blocks
// ---------------------------------------------------------------------------------------

/// SplitMix64: a generator of numbers that is the same everywhere.
struct Numbers(u64);

impl Numbers {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }

    fn between(&mut self, low: i32, high: i32) -> i32 {
        low + self.below((high - low + 1) as usize) as i32
    }

    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }

    fn pick<T: Copy>(&mut self, from: &[T]) -> T {
        from[self.below(from.len())]
    }
}

fn state(text: &str) -> BlockState {
    BlockState::parse(text).unwrap_or_else(|| panic!("{text} names a block state"))
}

/// States light tells apart: air of both kinds, a block that stops light, ones that
/// dampen it by one or more (glass by none, leaves, water, ice, cobweb), ones that give
/// light of several levels, and slabs and stairs whose faces stop it on some sides.
fn palette() -> Vec<BlockState> {
    let mut states = vec![
        blocks::AIR,
        blocks::AIR,
        blocks::AIR,
        blocks::CAVE_AIR,
        blocks::STONE,
        blocks::STONE,
        blocks::GLASS,
        blocks::WATER,
        blocks::OAK_LEAVES,
        blocks::TORCH,
        blocks::GLOWSTONE,
        state("minecraft:ice"),
        state("minecraft:cobweb"),
        state("minecraft:lantern"),
        state("minecraft:magma_block"),
        state("minecraft:sea_lantern"),
        state("minecraft:redstone_lamp[lit=true]"),
        state("minecraft:oak_slab[type=bottom]"),
        state("minecraft:oak_slab[type=top]"),
        state("minecraft:oak_slab[type=double]"),
        state("minecraft:oak_slab[type=bottom,waterlogged=true]"),
        state("minecraft:snow[layers=1]"),
    ];
    for facing in ["north", "east", "south", "west"] {
        for half in ["bottom", "top"] {
            states.push(state(&format!(
                "minecraft:oak_stairs[facing={facing},half={half}]"
            )));
        }
    }
    states
}

// ---------------------------------------------------------------------------------------
// The oracle
// ---------------------------------------------------------------------------------------

/// The chunks a test holds. The engine is handed all of them, also those it was not told
/// of: a chunk the region has but keeps waiting is absent to the engine all the same
/// (the plan's 3.1).
#[derive(Clone, Default)]
struct Held(BTreeMap<ChunkPos, Chunk>);

impl Chunks for Held {
    fn chunk(&self, at: ChunkPos) -> Option<&Chunk> {
        self.0.get(&at)
    }
}

/// The eight chunks around `at`, in `Around::neighbours`' order: row by row from the
/// north, each row from the west.
fn around(at: ChunkPos) -> [ChunkPos; 8] {
    [
        ChunkPos::new(at.x - 1, at.z - 1),
        ChunkPos::new(at.x, at.z - 1),
        ChunkPos::new(at.x + 1, at.z - 1),
        ChunkPos::new(at.x - 1, at.z),
        ChunkPos::new(at.x + 1, at.z),
        ChunkPos::new(at.x - 1, at.z + 1),
        ChunkPos::new(at.x, at.z + 1),
        ChunkPos::new(at.x + 1, at.z + 1),
    ]
}

/// What the function gives for `at` beside the neighbours that are `present`: the light
/// as listed, and the light with every section listed.
fn oracle(
    held: &Held,
    present: &dyn Fn(ChunkPos) -> bool,
    at: ChunkPos,
    sky: bool,
    scratch: &mut Scratch,
) -> (Light, Light) {
    let neighbours = around(at).map(|near| present(near).then(|| &held.0[&near]));
    let centre = &held.0[&at];
    let around = Around {
        centre,
        neighbours,
        sky,
    };
    let entries = centre.sections().len() + 2;
    let every = (1u64 << entries) - 1;
    let (listed, unlisted) = light_with(
        &around,
        Unlisted {
            sky: every,
            block: every,
        },
        scratch,
    );
    let mut whole = listed.clone();
    unlisted.put_into(&mut whole);
    (listed, whole)
}

/// Where the engine and the function first differ for a chunk, or `None`.
fn difference(
    lighting: &Lighting,
    held: &Held,
    present: &dyn Fn(ChunkPos) -> bool,
    at: ChunkPos,
    sky: bool,
    scratch: &mut Scratch,
) -> Option<String> {
    let (listed, whole) = oracle(held, present, at, sky, scratch);
    let given = lighting.light(at, held);
    if given.as_ref() != Some(&listed) {
        let what = match &given {
            None => "none".to_string(),
            Some(given) => {
                let sky_differs: Vec<usize> = (0..listed.sky.len())
                    .filter(|&i| given.sky.get(i) != listed.sky.get(i))
                    .collect();
                let block_differs: Vec<usize> = (0..listed.block.len())
                    .filter(|&i| given.block.get(i) != listed.block.get(i))
                    .collect();
                format!(
                    "made_by {} beside {}, sky entries {sky_differs:?} and block entries \
                     {block_differs:?} differ (sky {:?} against {:?}; block {:?} against {:?})",
                    given.made_by,
                    given.beside,
                    forms(&given.sky),
                    forms(&listed.sky),
                    forms(&given.block),
                    forms(&listed.block),
                )
            }
        };
        return Some(format!("Lighting::light of {at:?}: {what}"));
    }
    let centre = &held.0[&at];
    let min_y = centre.min_y();
    let top = min_y + centre.height() as i32;
    for y in min_y - 16..top + 16 {
        for z in 0..16 {
            for x in 0..16 {
                let want = LightAt {
                    sky: if sky { whole.sky_at(min_y, x, y, z) } else { 0 },
                    block: whole.block_at(min_y, x, y, z),
                };
                let pos = BlockPos::new(at.x * 16 + x as i32, y, at.z * 16 + z as i32);
                let got = lighting.at(pos);
                if got != Some(want) {
                    return Some(format!(
                        "Lighting::at {pos:?} (in {at:?}) gave {got:?}, the function {want:?}"
                    ));
                }
            }
        }
    }
    None
}

/// The forms of a chunk's light sections, a letter each: absent, dark, full, levels.
fn forms(sections: &[SectionLight]) -> String {
    sections
        .iter()
        .map(|section| match section {
            SectionLight::Absent => '.',
            SectionLight::Dark => 'd',
            SectionLight::Full => 'f',
            SectionLight::Levels(_) => 'L',
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// A scene for the cases by hand
// ---------------------------------------------------------------------------------------

/// Chunks held, the engine, and which chunks it was told of.
struct Scene {
    held: Held,
    lighting: Lighting,
    present: BTreeSet<ChunkPos>,
    sky: bool,
    scratch: Scratch,
}

impl Scene {
    fn new(sky: bool) -> Self {
        Self {
            held: Held::default(),
            lighting: Lighting::new(sky),
            present: BTreeSet::new(),
            sky,
            scratch: Scratch::new(),
        }
    }

    /// Holds `chunk` at `at` without telling the engine.
    fn hold(&mut self, at: ChunkPos, chunk: Chunk) {
        self.held.0.insert(at, chunk);
    }

    /// Holds `chunk` at `at` and tells the engine it arrived.
    fn arrive(&mut self, at: ChunkPos, chunk: Chunk) {
        self.hold(at, chunk);
        self.lighting.arrived(at);
        self.present.insert(at);
    }

    fn leave(&mut self, at: ChunkPos) {
        self.lighting.left(at);
        self.present.remove(&at);
    }

    fn adopt(&mut self, at: ChunkPos, kept: Kept) {
        self.lighting.adopt(at, kept);
        self.present.insert(at);
    }

    /// Sets a block and tells the engine where light can see it.
    fn set(&mut self, x: i32, y: i32, z: i32, to: BlockState) {
        let pos = BlockPos::new(x, y, z);
        let chunk = self
            .held
            .0
            .get_mut(&pos.chunk())
            .expect("the block's chunk is held");
        let old = chunk
            .set((x & 15) as usize, y, (z & 15) as usize, to)
            .expect("the block is within the chunk's heights");
        if clustine_world::light_tells_apart(old, to) {
            self.lighting.changed(pos);
        }
    }

    /// Runs the engine and checks every present chunk against the function.
    fn run(&mut self) -> clustine_light::engine::Work {
        let work = self.lighting.run(&self.held);
        self.check();
        work
    }

    fn check(&mut self) {
        let present = self.present.clone();
        let is_present = |at: ChunkPos| present.contains(&at);
        for &at in &present {
            assert!(self.lighting.has(at), "{at:?} is present to the engine");
            if let Some(difference) = difference(
                &self.lighting,
                &self.held,
                &is_present,
                at,
                self.sky,
                &mut self.scratch,
            ) {
                panic!("{difference}");
            }
        }
    }

    fn block(&self, x: i32, y: i32, z: i32) -> u8 {
        self.lighting
            .at(BlockPos::new(x, y, z))
            .expect("the block's chunk is lit")
            .block
    }

    fn sky_at(&self, x: i32, y: i32, z: i32) -> u8 {
        self.lighting
            .at(BlockPos::new(x, y, z))
            .expect("the block's chunk is lit")
            .sky
    }
}

/// Four sections from y 0, filled with `fill` up to `height` (not included) and air
/// above.
fn filled_to(fill: BlockState, height: i32) -> Chunk {
    let sections = (0..4)
        .map(|_| Section::filled(blocks::AIR, Biome(0)))
        .collect();
    let mut chunk = Chunk::from_sections(0, sections);
    for y in 0..height {
        for z in 0..16 {
            for x in 0..16 {
                chunk.set(x, y, z, fill);
            }
        }
    }
    chunk
}

/// Sets a block of a chunk by world coordinates (the chunk at `at`).
fn put(chunk: &mut Chunk, at: ChunkPos, x: i32, y: i32, z: i32, to: BlockState) {
    assert_eq!(
        BlockPos::new(x, y, z).chunk(),
        at,
        "the block is in the chunk"
    );
    chunk.set((x & 15) as usize, y, (z & 15) as usize, to);
}

fn every_chunk(from: i32, to: i32) -> Vec<ChunkPos> {
    let mut all = Vec::new();
    for x in from..=to {
        for z in from..=to {
            all.push(ChunkPos::new(x, z));
        }
    }
    all
}

// ---------------------------------------------------------------------------------------
// The cases by hand
// ---------------------------------------------------------------------------------------

#[test]
fn a_torch_placed_and_broken_at_a_chunks_corner_lights_all_four_chunks_and_then_none() {
    let mut scene = Scene::new(true);
    for at in every_chunk(-1, 1) {
        scene.arrive(at, filled_to(blocks::STONE, 4));
    }
    scene.run();
    // The corner block of the chunk at (0, 0) that touches (1, 0), (0, 1) and (1, 1).
    scene.set(15, 4, 15, blocks::TORCH);
    scene.run();
    assert_eq!(scene.block(15, 4, 15), 14);
    assert_eq!(scene.block(16, 4, 15), 13);
    assert_eq!(scene.block(15, 4, 16), 13);
    assert_eq!(scene.block(16, 4, 16), 12);
    assert_eq!(scene.block(16, 5, 16), 11);
    scene.set(15, 4, 15, blocks::AIR);
    scene.run();
    for (x, z) in [(15, 15), (16, 15), (15, 16), (16, 16), (20, 20)] {
        assert_eq!(scene.block(x, 4, z), 0, "no light left at {x}, {z}");
    }
}

#[test]
fn light_passes_through_a_gap_in_a_wall_of_slabs_and_stops_when_it_is_closed() {
    let mut scene = Scene::new(true);
    // Stone to y 20, with a corridor two high along x at z 8 from x 4 to x 28, across
    // the face between (0, 0) and (1, 0).
    for at in every_chunk(-1, 1) {
        scene.arrive(at, filled_to(blocks::STONE, 20));
    }
    scene.run();
    for x in 4..=28 {
        scene.set(x, 5, 8, blocks::AIR);
        scene.set(x, 6, 8, blocks::AIR);
    }
    // A wall of slabs on the far side of the face: whole double slabs, but a lower slab
    // at the bottom, whose upper half is open.
    scene.set(16, 6, 8, state("minecraft:oak_slab[type=double]"));
    scene.set(16, 5, 8, state("minecraft:oak_slab[type=bottom]"));
    scene.set(6, 5, 8, blocks::GLOWSTONE);
    scene.run();
    // The sky stays out: the corridor is under stone.
    assert_eq!(scene.sky_at(10, 5, 8), 0);
    assert!(scene.block(17, 5, 8) > 0, "light passes the lower slab");
    assert_eq!(scene.block(16, 6, 8), 0, "the double slab takes no light");
    // The lower slab made an upper one.
    scene.set(16, 5, 8, state("minecraft:oak_slab[type=top]"));
    scene.run();
    // An upper slab's lower half is open on both sides, so light may still pass under
    // it; the function judges how much.
    let through_top = scene.block(17, 5, 8);
    scene.set(16, 5, 8, state("minecraft:oak_slab[type=double]"));
    scene.run();
    assert_eq!(scene.block(17, 5, 8), 0, "a whole wall stops the light");
    assert_eq!(
        scene.block(15, 5, 8),
        15 - 9,
        "and the near side is still lit"
    );
    // Open again: the light comes back as it was with the upper slab.
    scene.set(16, 5, 8, state("minecraft:oak_slab[type=top]"));
    scene.run();
    assert_eq!(scene.block(17, 5, 8), through_top);
}

#[test]
fn the_sky_reaches_down_a_shaft_and_under_an_overhang() {
    let mut scene = Scene::new(true);
    for at in every_chunk(-1, 1) {
        scene.arrive(at, filled_to(blocks::STONE, 4));
    }
    scene.run();
    // A plate of stone at y 10 over x 8 to 20 across the face between (0, *) and (1, *),
    // over every z of the three rows of chunks.
    for x in 8..=20 {
        for z in -16..32 {
            scene.set(x, 10, z, blocks::STONE);
        }
    }
    scene.run();
    assert_eq!(scene.sky_at(7, 5, 0), 15);
    assert_eq!(scene.sky_at(8, 5, 0), 14);
    assert_eq!(scene.sky_at(14, 5, 0), 8);
    assert_eq!(scene.sky_at(16, 5, 0), 10);
    assert_eq!(scene.sky_at(20, 9, 0), 14);
    assert_eq!(scene.sky_at(12, 3, 0), 0, "nothing passes the floor");
    // A shaft through the plate at x 14, and a tunnel from its foot under the floor
    // towards the east, across the face.
    scene.set(14, 10, 4, blocks::AIR);
    for y in 0..4 {
        scene.set(14, y, 4, blocks::AIR);
    }
    for x in 15..=24 {
        scene.set(x, 1, 4, blocks::AIR);
    }
    scene.run();
    for y in 0..=10 {
        assert_eq!(
            scene.sky_at(14, y, 4),
            15,
            "straight down the shaft at y {y}"
        );
    }
    assert_eq!(scene.sky_at(16, 1, 4), 13);
    assert_eq!(scene.sky_at(24, 1, 4), 5);
    // The shaft closed at the top: the tunnel is lit only from under the plate.
    scene.set(14, 10, 4, blocks::STONE);
    scene.run();
    assert_eq!(scene.sky_at(14, 9, 4), 8);
    assert!(scene.sky_at(24, 1, 4) < 5);
}

#[test]
fn a_sections_first_and_last_block_are_lit_as_the_function_lights_them() {
    let mut scene = Scene::new(true);
    for at in every_chunk(-1, 1) {
        scene.arrive(at, filled_to(blocks::AIR, 0));
    }
    scene.run();
    // The first block of the chunk's second section, at its north-west corner, and the
    // last, at its south-east corner.
    scene.set(0, 16, 0, blocks::TORCH);
    scene.set(15, 31, 15, blocks::GLOWSTONE);
    // The world's lowest and highest blocks: light goes on into the sections below and
    // above the chunk.
    scene.set(5, 0, 5, blocks::GLOWSTONE);
    scene.set(9, 63, 9, blocks::GLOWSTONE);
    scene.run();
    assert_eq!(scene.block(0, 16, 0), 14);
    assert_eq!(scene.block(0, 15, 0), 13);
    assert_eq!(scene.block(-1, 16, 0), 13);
    assert_eq!(scene.block(0, 16, -1), 13);
    assert_eq!(scene.block(15, 31, 15), 15);
    assert_eq!(scene.block(15, 32, 15), 14);
    assert_eq!(scene.block(16, 31, 16), 13);
    assert_eq!(scene.block(5, -1, 5), 14);
    assert_eq!(scene.block(9, 64, 9), 14);
    assert_eq!(scene.block(9, 70, 9), 8);
    // And all of it taken away again.
    for (x, y, z) in [(0, 16, 0), (15, 31, 15), (5, 0, 5), (9, 63, 9)] {
        scene.set(x, y, z, blocks::AIR);
    }
    scene.run();
    assert_eq!(scene.block(5, -1, 5), 0);
    assert_eq!(scene.block(9, 64, 9), 0);
}

/// A chunk of stone to y 20 with a tunnel at y 5 along z 8 from `from` to `to` (x within
/// the chunk at `at`), and a glowstone at x `light` if it is given.
fn tunnelled(at: ChunkPos, from: i32, to: i32, z: i32, light: Option<i32>) -> Chunk {
    let mut chunk = filled_to(blocks::STONE, 20);
    let base = at.x * 16;
    for x in from..=to {
        put(&mut chunk, at, base + x, 5, at.z * 16 + z, blocks::AIR);
    }
    if let Some(x) = light {
        put(
            &mut chunk,
            at,
            base + x,
            5,
            at.z * 16 + z,
            blocks::GLOWSTONE,
        );
    }
    chunk
}

#[test]
fn a_chunk_arriving_beside_a_lit_cave_is_lit_from_it() {
    let mut scene = Scene::new(true);
    let (west, east) = (ChunkPos::new(0, 0), ChunkPos::new(1, 0));
    scene.arrive(west, tunnelled(west, 4, 15, 8, Some(10)));
    scene.run();
    assert_eq!(scene.block(15, 5, 8), 10);
    scene.arrive(east, tunnelled(east, 0, 9, 8, None));
    scene.run();
    assert_eq!(scene.block(16, 5, 8), 9);
    assert_eq!(scene.block(24, 5, 8), 1);
    assert_eq!(scene.block(25, 5, 8), 0);
}

#[test]
fn a_chunk_leaving_that_carried_the_light_of_its_neighbours_cave_takes_that_light_away() {
    let mut scene = Scene::new(true);
    let (west, east) = (ChunkPos::new(0, 0), ChunkPos::new(1, 0));
    // Two tunnels in the west chunk, at z 4 and z 10, each reaching the face; the light
    // is in the first, and the second is joined to it only through the east chunk.
    let mut chunk = tunnelled(west, 8, 15, 4, Some(13));
    for x in 8..=15 {
        put(&mut chunk, west, x, 5, 10, blocks::AIR);
    }
    scene.arrive(west, chunk);
    let mut joint = filled_to(blocks::STONE, 20);
    for z in 4..=10 {
        put(&mut joint, east, 16, 5, z, blocks::AIR);
    }
    scene.arrive(east, joint);
    scene.run();
    // From the glowstone at x 13, z 4: three steps east, six south, one west.
    assert_eq!(scene.block(15, 5, 10), 15 - 10);
    assert_eq!(scene.block(12, 5, 10), 15 - 13);
    scene.leave(east);
    scene.run();
    assert_eq!(scene.block(15, 5, 10), 0);
    assert_eq!(scene.block(12, 5, 10), 0);
    assert_eq!(
        scene.block(15, 5, 4),
        13,
        "the near tunnel keeps its own light"
    );
    assert_eq!(scene.lighting.at(BlockPos::new(16, 5, 4)), None);
}

#[test]
fn a_neighbours_sky_column_and_emitter_on_the_face_are_kept_when_the_chunk_beside_leaves() {
    let mut scene = Scene::new(true);
    let (west, east) = (ChunkPos::new(0, 0), ChunkPos::new(1, 0));
    // The west chunk, stone to y 20, has a shaft open to the sky in its column on the
    // face (x 15, z 3) down to y 5, and a glowstone on the face at x 15, z 12, y 5,
    // in a tunnel along the face.
    let mut chunk = filled_to(blocks::STONE, 20);
    for y in 5..20 {
        put(&mut chunk, west, 15, y, 3, blocks::AIR);
    }
    for z in 3..=12 {
        put(&mut chunk, west, 15, 5, z, blocks::AIR);
    }
    put(&mut chunk, west, 15, 5, 12, blocks::GLOWSTONE);
    scene.arrive(west, chunk);
    // The east chunk is open along the face, so that light passes both ways.
    let mut beside = filled_to(blocks::STONE, 20);
    for z in 0..16 {
        put(&mut beside, east, 16, 5, z, blocks::AIR);
    }
    scene.arrive(east, beside);
    scene.run();
    assert_eq!(scene.sky_at(16, 5, 3), 14);
    assert_eq!(scene.block(16, 5, 12), 14);
    scene.leave(east);
    scene.run();
    for y in 5..20 {
        assert_eq!(scene.sky_at(15, y, 3), 15, "the column on the face, y {y}");
    }
    assert_eq!(scene.sky_at(15, 5, 4), 14);
    assert_eq!(scene.block(15, 5, 12), 15, "the emitter on the face");
    assert_eq!(scene.block(15, 5, 10), 13);
}

#[test]
fn a_chunk_replaced_whole_whose_old_blocks_held_a_torch_by_the_face_leaves_no_light_beside_it() {
    let mut scene = Scene::new(true);
    let (west, east) = (ChunkPos::new(0, 0), ChunkPos::new(1, 0));
    scene.arrive(west, tunnelled(west, 4, 15, 8, None));
    scene.arrive(east, {
        let mut chunk = tunnelled(east, 0, 9, 8, None);
        put(&mut chunk, east, 16, 5, 8, blocks::TORCH);
        chunk
    });
    scene.run();
    assert_eq!(scene.block(15, 5, 8), 13);
    assert_eq!(scene.block(4, 5, 8), 2);
    // The east chunk's blocks are replaced whole (a copy's new base): the same tunnel,
    // no torch.
    scene.arrive(east, tunnelled(east, 0, 9, 8, None));
    scene.run();
    for x in 4..=25 {
        assert_eq!(scene.block(x, 5, 8), 0, "no light left at x {x}");
    }
    // And replaced again, with the torch: it comes back.
    scene.arrive(east, {
        let mut chunk = tunnelled(east, 0, 9, 8, None);
        put(&mut chunk, east, 16, 5, 8, blocks::TORCH);
        chunk
    });
    scene.run();
    assert_eq!(scene.block(15, 5, 8), 13);
}

/// Nine chunks with light crossing every face: glowstones near the faces and corners,
/// shafts open to the sky on faces, tunnels across, all under a stone roof at y 20.
fn crossed() -> Held {
    let mut held = Held::default();
    for at in every_chunk(-1, 1) {
        let mut chunk = filled_to(blocks::STONE, 20);
        let (bx, bz) = (at.x * 16, at.z * 16);
        // A hall at y 4 to 8 over the whole chunk but a pillar, open to its neighbours.
        for y in 4..=8 {
            for z in 0..16 {
                for x in 0..16 {
                    if !(6..=9).contains(&x) || !(6..=9).contains(&z) {
                        put(&mut chunk, at, bx + x, y, bz + z, blocks::AIR);
                    }
                }
            }
        }
        let k = (at.x + 2 * at.z).rem_euclid(5);
        put(&mut chunk, at, bx + 15 - k, 4, bz + k, blocks::GLOWSTONE);
        put(&mut chunk, at, bx + k, 8, bz + 15, blocks::TORCH);
        put(
            &mut chunk,
            at,
            bx,
            6,
            bz,
            state("minecraft:oak_slab[type=bottom]"),
        );
        // A shaft to the sky on the north face.
        for y in 9..20 {
            put(&mut chunk, at, bx + 3 + k, y, bz, blocks::AIR);
        }
        held.0.insert(at, chunk);
    }
    held
}

/// The nine chunks of [`crossed`] lit in one engine, and each one's kept levels from a
/// hand-over.
fn handed_over(taken: (u64, u64)) -> (Held, BTreeMap<ChunkPos, Kept>) {
    let mut scene = Scene::new(true);
    for (at, chunk) in crossed().0 {
        scene.arrive(at, chunk);
    }
    scene.run();
    let kept = every_chunk(-1, 1)
        .into_iter()
        .map(|at| {
            let kept = scene.lighting.kept(at, taken).expect("a lit chunk is kept");
            assert_eq!(kept.walls, walls_of(at, &every_chunk(-1, 1)));
            (at, kept)
        })
        .collect();
    (scene.held, kept)
}

/// The bits of `Kept::walls` for `at` where only `present` are present.
fn walls_of(at: ChunkPos, present: &[ChunkPos]) -> u8 {
    around(at)
        .iter()
        .enumerate()
        .filter(|(_, near)| !present.contains(near))
        .fold(0, |walls, (bit, _)| walls | 1 << bit)
}

#[test]
fn a_chunk_adopted_with_its_neighbours_from_the_same_hand_over_is_lit_as_the_function_lights_it() {
    let (held, mut kept) = handed_over((7, 100));
    let mut scene = Scene::new(true);
    scene.held = held;
    for at in every_chunk(-1, 1) {
        scene.adopt(at, kept.remove(&at).expect("kept"));
    }
    let work = scene.run();
    assert_eq!(work.adopted, 9);
    assert_eq!(
        work.chunks_lit_whole, 0,
        "no chunk handed back is lit whole"
    );
}

#[test]
fn a_chunk_adopted_before_its_neighbours_from_the_same_hand_over_is_mended_when_they_come() {
    let (held, mut kept) = handed_over((7, 100));
    let mut scene = Scene::new(true);
    scene.held = held;
    let centre = ChunkPos::new(0, 0);
    scene.adopt(centre, kept.remove(&centre).expect("kept"));
    scene.run();
    // The east and the north next, then the rest.
    for at in [ChunkPos::new(1, 0), ChunkPos::new(0, -1)] {
        scene.adopt(at, kept.remove(&at).expect("kept"));
    }
    scene.run();
    for (at, kept) in std::mem::take(&mut kept) {
        scene.adopt(at, kept);
    }
    let work = scene.run();
    assert_eq!(work.chunks_lit_whole, 0);
}

#[test]
fn a_chunk_adopted_after_its_neighbours_from_the_same_hand_over_is_mended_against_them() {
    let (held, mut kept) = handed_over((7, 100));
    let mut scene = Scene::new(true);
    scene.held = held;
    let centre = ChunkPos::new(0, 0);
    let middle = kept.remove(&centre).expect("kept");
    // The corners first, alone (each beside walls only), then the faces, then the
    // centre.
    for at in [(-1, -1), (1, -1), (-1, 1), (1, 1)].map(|(x, z)| ChunkPos::new(x, z)) {
        scene.adopt(at, kept.remove(&at).expect("kept"));
    }
    scene.run();
    for (at, kept) in std::mem::take(&mut kept) {
        scene.adopt(at, kept);
    }
    scene.run();
    scene.adopt(centre, middle);
    scene.run();
}

#[test]
fn a_chunk_adopted_beside_neighbours_that_changed_or_came_whole_since_the_hand_over_is_mended() {
    let (held, mut kept) = handed_over((7, 100));
    let mut scene = Scene::new(true);
    scene.held = held;
    // The east chunk comes back from the store afresh, with its glowstone taken out
    // and a torch at its west face: it is lit whole. The north chunk's kept levels are
    // dropped and it is not present at all.
    let east = ChunkPos::new(1, 0);
    kept.remove(&east);
    kept.remove(&ChunkPos::new(0, -1));
    {
        let chunk = scene.held.0.get_mut(&east).expect("held");
        let k = (1i32).rem_euclid(5) as usize;
        chunk.set(15 - k, 4, k, blocks::AIR);
        chunk.set(0, 5, 3, blocks::TORCH);
    }
    scene.lighting.arrived(east);
    scene.present.insert(east);
    for (at, kept) in std::mem::take(&mut kept) {
        scene.adopt(at, kept);
    }
    scene.run();
}

#[test]
fn a_block_changed_in_an_adopted_chunk_before_the_run_is_lit_with_the_adoption() {
    // The region adopts at its light step, after the players' actions of the tick
    // (the plan's 4.1): a chunk handed back may be written before the run that adopts
    // it, its kept levels then of its blocks as they were.
    let (held, kept) = handed_over((7, 100));
    let mut scene = Scene::new(true);
    scene.held = held;
    for (at, kept) in kept {
        scene.adopt(at, kept);
    }
    // A torch by the east face of the centre, and the glowstone of the west chunk
    // taken out.
    scene.set(15, 5, 12, blocks::TORCH);
    let k = (-1i32).rem_euclid(5);
    scene.set(-16 + 15 - k, 4, k, blocks::AIR);
    scene.run();
    assert_eq!(scene.block(16, 5, 12), 13);
}

#[test]
fn levels_kept_from_one_hand_over_and_adopted_beside_another_s_are_mended_at_their_shared_faces() {
    // The west column of chunks handed over at one tick, the rest at a later one after
    // a torch was placed by the face between them: the faces between the two do not
    // stand.
    let mut scene = Scene::new(true);
    for (at, chunk) in crossed().0 {
        scene.arrive(at, chunk);
    }
    scene.run();
    let west: Vec<ChunkPos> = every_chunk(-1, 1)
        .into_iter()
        .filter(|at| at.x == -1)
        .collect();
    let mut kept = BTreeMap::new();
    for &at in &west {
        kept.insert(at, scene.lighting.kept(at, (3, 10)).expect("lit"));
        scene.leave(at);
    }
    scene.run();
    // A torch on the east side of the face, beside the west column that has gone.
    scene.set(0, 5, 2, blocks::TORCH);
    scene.run();
    for at in every_chunk(-1, 1).into_iter().filter(|at| at.x != -1) {
        kept.insert(at, scene.lighting.kept(at, (3, 11)).expect("lit"));
    }
    let mut fresh = Scene::new(true);
    fresh.held = scene.held.clone();
    for (at, kept) in kept {
        fresh.adopt(at, kept);
    }
    fresh.run();
    assert_eq!(
        fresh.block(-1, 5, 2),
        13,
        "the torch lights across the face"
    );
}

// ---------------------------------------------------------------------------------------
// Carried light (the plan's 3.5)
// ---------------------------------------------------------------------------------------

/// The generator version the chunks of the carried cases are marked with.
const GENERATOR: u16 = 7;

/// The plan's 3.5, as the region is to ask it of a chunk arriving with light: the light
/// is this crate's, made beside generated chunks (1); all eight neighbours are present
/// and as generated by that version (2); every section the light lists as absent lies
/// above every section of the chunk that holds a block (3).
fn carried_light_may_be_adopted(
    chunk: &Chunk,
    neighbours: [Option<&Chunk>; 8],
) -> Result<(), &'static str> {
    let light = chunk.light().ok_or("no light")?;
    if light.made_by != clustine_light::VERSION || light.beside == 0 {
        return Err("clause 1");
    }
    if !neighbours
        .iter()
        .all(|near| near.is_some_and(|near| near.generated() == light.beside))
    {
        return Err("clause 2");
    }
    if !clause_three_holds(chunk, light, true) {
        return Err("clause 3");
    }
    Ok(())
}

/// Whether every section `light` lists as absent lies above every section of `chunk`
/// that holds a block. In a dimension without a sky every sky entry is absent, and only
/// the blocks' light is asked (the plan does not say).
fn clause_three_holds(chunk: &Chunk, light: &Light, sky: bool) -> bool {
    // Light entry i is section i - 1; the highest section holding a block.
    let highest = chunk
        .sections()
        .iter()
        .rposition(|section| section.non_air_count() > 0);
    let lowest_absent_allowed = highest.map_or(0, |highest| highest + 2);
    let layers: &[&Vec<SectionLight>] = if sky {
        &[&light.sky, &light.block]
    } else {
        &[&light.block]
    };
    layers.iter().all(|layer| {
        layer.iter().enumerate().all(|(entry, section)| {
            *section != SectionLight::Absent || entry >= lowest_absent_allowed
        })
    })
}

/// Nine generated chunks: stone to y 6 with glowstones in and on it, and the
/// centre's light made beside them as the generation pool makes it. `float` puts a
/// plate of stone high in the centre with nothing between it and the ground, so that
/// sections between are absent under something that floats.
fn generated(float: bool) -> Held {
    let mut held = Held::default();
    for at in every_chunk(-1, 1) {
        let sections = (0..8)
            .map(|_| Section::filled(blocks::AIR, Biome(0)))
            .collect();
        let mut chunk = Chunk::from_sections(0, sections);
        for y in 0..6 {
            for z in 0..16 {
                for x in 0..16 {
                    chunk.set(x, y, z, blocks::STONE);
                }
            }
        }
        chunk.set(8, 2, 8, blocks::GLOWSTONE);
        chunk.set(15, 5, 4, blocks::GLOWSTONE);
        if float && at == ChunkPos::new(0, 0) {
            for z in 0..16 {
                for x in 0..16 {
                    chunk.set(x, 100, z, blocks::STONE);
                }
            }
        }
        chunk.mark_generated(GENERATOR, false);
        held.0.insert(at, chunk);
    }
    let centre = ChunkPos::new(0, 0);
    let mut light = {
        let neighbours = around(centre).map(|near| held.0.get(&near));
        light(&Around {
            centre: &held.0[&centre],
            neighbours,
            sky: true,
        })
    };
    light.beside = GENERATOR;
    held.0
        .get_mut(&centre)
        .expect("held")
        .set_light(light.clone());
    held
}

/// A carried light handed to the engine as a kept one, as the region adopts it: no
/// neighbour a wall when it was made, a hand-over of its own.
fn carried(chunk: &Chunk) -> Kept {
    Kept {
        light: chunk.light().expect("carried").clone(),
        walls: 0,
        taken: (u64::MAX, 1),
    }
}

/// Lights the eight neighbours of the centre, then has the centre arrive: adopted with
/// its carried light where 3.5 lets it be, lit whole where not. Returns the scene and
/// what 3.5 said.
fn carried_scene(held: Held, present: &[ChunkPos]) -> (Scene, Result<(), &'static str>) {
    let mut scene = Scene::new(true);
    scene.held = held;
    let centre = ChunkPos::new(0, 0);
    for &at in present {
        scene.lighting.arrived(at);
        scene.present.insert(at);
    }
    scene.run();
    let neighbours =
        around(centre).map(|near| scene.present.contains(&near).then(|| &scene.held.0[&near]));
    let verdict = carried_light_may_be_adopted(&scene.held.0[&centre], neighbours);
    if verdict.is_ok() {
        let kept = carried(&scene.held.0[&centre]);
        scene.adopt(centre, kept);
    } else {
        scene.lighting.arrived(centre);
        scene.present.insert(centre);
    }
    let work = scene.run();
    if verdict.is_ok() {
        assert_eq!((work.adopted, work.chunks_lit_whole), (1, 0));
    }
    (scene, verdict)
}

fn ring() -> Vec<ChunkPos> {
    around(ChunkPos::new(0, 0)).to_vec()
}

#[test]
fn carried_light_is_adopted_where_every_clause_holds_and_equals_the_function() {
    let (_, verdict) = carried_scene(generated(false), &ring());
    assert_eq!(verdict, Ok(()));
}

#[test]
fn carried_light_made_by_another_version_or_beside_unknown_chunks_is_refused_by_clause_one() {
    for (made_by, beside) in [
        (Light::IMPORTED, GENERATOR),
        (clustine_light::VERSION + 1, GENERATOR),
        (clustine_light::VERSION, 0),
    ] {
        let mut held = generated(false);
        let centre = held.0.get_mut(&ChunkPos::new(0, 0)).expect("held");
        let mut light = centre.light().expect("carried").clone();
        light.made_by = made_by;
        light.beside = beside;
        centre.set_light(light);
        let (_, verdict) = carried_scene(held, &ring());
        assert_eq!(
            verdict,
            Err("clause 1"),
            "made by {made_by} beside {beside}"
        );
    }
}

#[test]
fn carried_light_beside_an_absent_or_changed_neighbour_is_refused_by_clause_two() {
    // A neighbour absent: refused, lit whole beside the wall.
    let mut fewer = ring();
    fewer.retain(|at| *at != ChunkPos::new(1, 0));
    let (_, verdict) = carried_scene(generated(false), &fewer);
    assert_eq!(verdict, Err("clause 2"));
    // A neighbour changed since it was generated: refused.
    let mut held = generated(false);
    held.0
        .get_mut(&ChunkPos::new(1, 0))
        .expect("held")
        .set(0, 5, 4, blocks::AIR);
    let (_, verdict) = carried_scene(held, &ring());
    assert_eq!(verdict, Err("clause 2"));
}

#[test]
fn carried_light_with_an_absent_section_under_something_floating_is_refused_by_clause_three() {
    let (scene, verdict) = carried_scene(generated(true), &ring());
    assert_eq!(verdict, Err("clause 3"));
    // Why: under the plate the sky is not 15, which is what an absent section would be
    // adopted as.
    assert!(scene.sky_at(8, 60, 8) < 15);
}

#[test]
fn carried_light_adopted_despite_clause_three_would_read_full_sky_under_the_plate() {
    // What clause 3 protects against: the engine adopts an absent section as full sky
    // (`Kept`'s documentation), so a carried light with absent sections under something
    // floating must not reach it.
    let held = generated(true);
    let centre = ChunkPos::new(0, 0);
    let mut scene = Scene::new(true);
    scene.held = held;
    for at in ring() {
        scene.lighting.arrived(at);
        scene.present.insert(at);
    }
    scene.run();
    let kept = carried(&scene.held.0[&centre]);
    scene.adopt(centre, kept);
    scene.lighting.run(&scene.held);
    assert_eq!(scene.sky_at(8, 60, 8), 15);
}

#[test]
fn carried_light_adopted_despite_clause_two_beside_an_absent_neighbour_is_mended_at_its_face() {
    // Clause 2 asks more than the engine needs: `adopt` mends every face towards a
    // neighbour that is not from the same hand-over, a wall among them (the plan's
    // 3.2), so a carried light adopted beside an absent neighbour still comes out as the
    // function gives it.
    let held = generated(false);
    let centre = ChunkPos::new(0, 0);
    let mut scene = Scene::new(true);
    scene.held = held;
    for at in ring().into_iter().filter(|at| *at != ChunkPos::new(-1, 0)) {
        scene.lighting.arrived(at);
        scene.present.insert(at);
    }
    scene.run();
    let kept = carried(&scene.held.0[&centre]);
    scene.adopt(centre, kept);
    scene.run();
}

// ---------------------------------------------------------------------------------------
// Determinism, and reads beyond the chunk
// ---------------------------------------------------------------------------------------

#[test]
fn two_histories_to_the_same_blocks_give_the_same_light() {
    // One engine gets there by arrivals, changes, a departure and a return; the other
    // has every chunk arrive once with the blocks as they end.
    let mut long = Scene::new(true);
    for (at, chunk) in crossed().0 {
        long.arrive(at, chunk);
    }
    long.run();
    long.set(3, 5, 3, blocks::GLOWSTONE);
    long.set(-3, 5, -3, blocks::TORCH);
    long.run();
    long.set(3, 5, 3, blocks::AIR);
    long.leave(ChunkPos::new(1, 1));
    long.run();
    long.set(20, 5, 20, state("minecraft:sea_lantern"));
    long.lighting.arrived(ChunkPos::new(1, 1));
    long.present.insert(ChunkPos::new(1, 1));
    long.set(-12, 9, 5, blocks::STONE);
    long.run();
    let mut short = Scene::new(true);
    for (at, chunk) in long.held.0.clone() {
        short.arrive(at, chunk);
    }
    short.run();
    for at in every_chunk(-1, 1) {
        assert_eq!(
            long.lighting.light(at, &long.held),
            short.lighting.light(at, &short.held)
        );
        for y in -16..80 {
            for z in 0..16 {
                for x in 0..16 {
                    let pos = BlockPos::new(at.x * 16 + x, y, at.z * 16 + z);
                    assert_eq!(long.lighting.at(pos), short.lighting.at(pos), "{pos:?}");
                }
            }
        }
    }
}

#[test]
fn a_read_above_the_kept_sections_is_full_sky_and_no_block_light() {
    // The plan's 3.2: "Above and below the sections it keeps: sky 15, blocks 0".
    let mut scene = Scene::new(true);
    for at in every_chunk(-1, 1) {
        scene.arrive(at, filled_to(blocks::STONE, 64));
    }
    scene.set(5, 63, 5, blocks::GLOWSTONE);
    scene.run();
    assert_eq!(scene.block(5, 64, 5), 14);
    for y in [80, 95, 200] {
        assert_eq!(
            scene.lighting.at(BlockPos::new(5, y, 5)),
            Some(LightAt { sky: 15, block: 0 }),
            "above the kept sections at y {y}"
        );
    }
}

#[test]
#[ignore = "finding: below the kept sections `Lighting::at` reads the lowest kept section \
            at the same height within it, as `Light::block_at` does, so a glowstone at the \
            world's lowest block shows 14 at y -17; the plan's 3.2 says blocks 0 there, as \
            the game's block storage gives where a section has no data"]
fn a_read_below_the_kept_sections_has_no_block_light() {
    // The sky below is not asked: the plan's 3.2 says 15, but the game reads the
    // lowest section with data there (n8 SkyLightSectionStorage, the plan's 2 row 4),
    // which under a world of stone is dark; the plan is silent on which is meant.
    let mut scene = Scene::new(true);
    for at in every_chunk(-1, 1) {
        scene.arrive(at, filled_to(blocks::STONE, 64));
    }
    scene.set(3, 0, 3, blocks::GLOWSTONE);
    scene.run();
    // The section below the chunk is lit by the glowstone at the bottom; below that is
    // outside what is kept.
    assert_eq!(scene.block(3, -1, 3), 14);
    for y in [-17, -32, -100] {
        assert_eq!(scene.block(3, y, 3), 0, "no block light at y {y}");
    }
}

#[test]
fn a_dimension_without_a_sky_has_no_sky_light_anywhere() {
    let mut scene = Scene::new(false);
    for at in every_chunk(-1, 1) {
        scene.arrive(at, filled_to(blocks::STONE, 4));
    }
    scene.set(8, 4, 8, blocks::GLOWSTONE);
    scene.run();
    assert_eq!(scene.sky_at(8, 40, 8), 0);
    assert_eq!(scene.block(8, 5, 8), 14);
}

// ---------------------------------------------------------------------------------------
// The properties
// ---------------------------------------------------------------------------------------

/// How many cases a property runs, and the one seed to run alone if one is given.
fn seeds(property: u64) -> Vec<u64> {
    if let Ok(seed) = std::env::var("CLUSTINE_LIGHT_ENGINE_SEED") {
        return vec![
            seed.parse()
                .expect("CLUSTINE_LIGHT_ENGINE_SEED is a number"),
        ];
    }
    let cases = std::env::var("CLUSTINE_LIGHT_ENGINE_CASES")
        .map(|cases| {
            cases
                .parse()
                .expect("CLUSTINE_LIGHT_ENGINE_CASES is a number")
        })
        .unwrap_or(CASES);
    (0..cases).map(|case| property << 32 | case).collect()
}

/// A chunk of `sections` sections from `min_y`, its sections drawn each of a kind: air,
/// stone, water, sparse blocks in air, or dense blocks; the top sections air, so that
/// there are sections without light.
///
/// Where `floating`, mostly a ground in the lowest section and a layer high up with
/// two sections or more of nothing between, so that sections without light lie under
/// blocks (the plan's 3.5, clause 3; the crate's named difference from the game).
fn random_chunk(
    numbers: &mut Numbers,
    palette: &[BlockState],
    min_y: i32,
    sections: usize,
    floating: bool,
) -> Chunk {
    let kinds: Vec<usize> = if floating && numbers.chance(97) {
        (0..sections)
            .map(|index| match index {
                0 => numbers.pick(&[1, 3, 4]),
                _ if index == sections - 2 => numbers.pick(&[1, 4]),
                _ => 5,
            })
            .collect()
    } else {
        let top = numbers.below(sections + 1);
        (0..sections)
            .map(|index| if index >= top { 0 } else { numbers.below(5) })
            .collect()
    };
    let built = kinds
        .iter()
        .map(|kind| {
            let fill = match kind {
                1 => blocks::STONE,
                2 => blocks::WATER,
                4 => blocks::STONE,
                _ => blocks::AIR,
            };
            Section::filled(fill, Biome(0))
        })
        .collect();
    let mut chunk = Chunk::from_sections(min_y, built);
    for (index, kind) in kinds.iter().enumerate() {
        let base = min_y + 16 * index as i32;
        let count = match kind {
            3 => numbers.below(40),
            4 => 300 + numbers.below(500),
            1 | 2 => numbers.below(6),
            5 => 0,
            _ => numbers.below(2) * numbers.below(4),
        };
        for _ in 0..count {
            let block = if *kind == 4 && numbers.chance(60) {
                blocks::AIR
            } else {
                numbers.pick(palette)
            };
            chunk.set(
                numbers.below(16),
                base + numbers.below(16) as i32,
                numbers.below(16),
                block,
            );
        }
    }
    chunk
}

/// One random world and history, checked after every run.
struct Case {
    numbers: Numbers,
    palette: Vec<BlockState>,
    sky: bool,
    min_y: i32,
    sections: usize,
    /// Whether chunks are made mostly of a ground and a layer floating over it.
    floating: bool,
    /// The chunks' positions: a square of `side` chunks from `base`.
    base: (i32, i32),
    side: i32,
    held: Held,
    present: BTreeSet<ChunkPos>,
    /// Chunks handed out with their levels, waiting to be adopted; not written while
    /// they wait (a warm chunk is not).
    warm: BTreeMap<ChunkPos, Kept>,
    lighting: Lighting,
    hand_overs: u64,
    scratch: Scratch,
    /// What was done, for the message of a failure.
    log: Vec<String>,
}

impl Case {
    fn new(seed: u64) -> Self {
        let mut numbers = Numbers::new(seed);
        let sky = !numbers.chance(20);
        let min_y = numbers.pick(&[0, -16, -64]);
        let sections = 4 + numbers.below(5);
        let side = 3 + numbers.below(3) as i32;
        let base = (numbers.between(-3, 1), numbers.between(-3, 1));
        let floating = numbers.chance(30);
        Self {
            numbers,
            palette: palette(),
            sky,
            min_y,
            sections,
            floating,
            base,
            side,
            held: Held::default(),
            present: BTreeSet::new(),
            warm: BTreeMap::new(),
            lighting: Lighting::new(sky),
            hand_overs: 0,
            scratch: Scratch::new(),
            log: vec![format!(
                "sky {sky}, min_y {min_y}, {sections} sections, floating {floating}, \
                 {side}×{side} chunks from {base:?}"
            )],
        }
    }

    fn positions(&self) -> Vec<ChunkPos> {
        let mut all = Vec::new();
        for x in 0..self.side {
            for z in 0..self.side {
                all.push(ChunkPos::new(self.base.0 + x, self.base.1 + z));
            }
        }
        all
    }

    fn new_chunk(&mut self) -> Chunk {
        random_chunk(
            &mut self.numbers,
            &self.palette,
            self.min_y,
            self.sections,
            self.floating,
        )
    }

    fn pick_where(&mut self, keep: impl Fn(&Self, ChunkPos) -> bool) -> Option<ChunkPos> {
        let all: Vec<ChunkPos> = self
            .positions()
            .into_iter()
            .filter(|at| keep(self, *at))
            .collect();
        (!all.is_empty()).then(|| self.numbers.pick(&all))
    }

    fn arrive(&mut self, at: ChunkPos) {
        self.lighting.arrived(at);
        self.present.insert(at);
    }

    /// Some blocks changed in a chunk held and not warm, present or not; near its faces
    /// and corners more often than not.
    fn change(&mut self) {
        let Some(at) = self
            .pick_where(|case, at| case.held.0.contains_key(&at) && !case.warm.contains_key(&at))
        else {
            return;
        };
        let count = 1 + self.numbers.below(6);
        for _ in 0..count {
            let edge = [0, 1, 14, 15];
            let x = if self.numbers.chance(50) {
                self.numbers.pick(&edge)
            } else {
                self.numbers.below(16)
            };
            let z = if self.numbers.chance(50) {
                self.numbers.pick(&edge)
            } else {
                self.numbers.below(16)
            };
            let y = self.min_y + self.numbers.below(self.sections * 16) as i32;
            let to = self.numbers.pick(&self.palette);
            let chunk = self.held.0.get_mut(&at).expect("held");
            let old = chunk.set(x, y, z, to).expect("within the heights");
            let pos = BlockPos::new(at.x * 16 + x as i32, y, at.z * 16 + z as i32);
            if clustine_world::light_tells_apart(old, to) {
                self.lighting.changed(pos);
            } else if self.numbers.chance(10) {
                // A change noted that light cannot see changes nothing.
                self.lighting.changed(pos);
            }
            self.log.push(format!("set {pos:?} to {to:?}"));
        }
    }

    fn step(&mut self) {
        let roll = self.numbers.below(100);
        match roll {
            0..=39 => self.change(),
            40..=51 => self.run(),
            52..=59 => {
                // An arrival: a chunk held and absent, or one made anew where none is.
                if let Some(at) = self.pick_where(|case, at| {
                    !case.present.contains(&at) && !case.warm.contains_key(&at)
                }) {
                    if !self.held.0.contains_key(&at) || self.numbers.chance(20) {
                        let chunk = self.new_chunk();
                        self.held.0.insert(at, chunk);
                    }
                    self.arrive(at);
                    self.log.push(format!("arrived {at:?}"));
                }
            }
            60..=66 => {
                if let Some(at) = self.pick_where(|case, at| case.present.contains(&at)) {
                    self.lighting.left(at);
                    self.present.remove(&at);
                    if self.numbers.chance(30) {
                        self.held.0.remove(&at);
                    }
                    self.log.push(format!("left {at:?}"));
                }
            }
            67..=72 => {
                // A chunk's blocks replaced whole: a new chunk, or the same one with
                // blocks about its faces changed.
                if let Some(at) = self.pick_where(|case, at| case.present.contains(&at)) {
                    if self.numbers.chance(50) {
                        let chunk = self.new_chunk();
                        self.held.0.insert(at, chunk);
                    } else {
                        let chunk = self.held.0.get_mut(&at).expect("held");
                        for _ in 0..8 {
                            let x = self.numbers.pick(&[0, 15]);
                            let z = self.numbers.below(16);
                            let y = self.min_y + self.numbers.below(self.sections * 16) as i32;
                            let to = self.numbers.pick(&self.palette);
                            let (x, z) = if self.numbers.chance(50) {
                                (x, z)
                            } else {
                                (z, x)
                            };
                            chunk.set(x, y, z, to);
                        }
                    }
                    self.lighting.arrived(at);
                    self.log.push(format!("replaced {at:?}"));
                }
            }
            73..=78 => self.hand_over(),
            79..=89 => self.adopt(),
            90..=92 => {
                // A warm chunk's levels dropped: it is an ordinary absent chunk again.
                if let Some(at) = self.pick_where(|case, at| case.warm.contains_key(&at)) {
                    self.warm.remove(&at);
                    self.log.push(format!("dropped the kept levels of {at:?}"));
                }
            }
            93..=96 => self.carried(),
            _ => {
                // A change noted in a chunk that is not present is passed over.
                if let Some(at) = self.pick_where(|case, at| !case.present.contains(&at)) {
                    self.lighting.changed(BlockPos::new(
                        at.x * 16 + 3,
                        self.min_y + 5,
                        at.z * 16 + 7,
                    ));
                    self.log.push(format!("noted a change in absent {at:?}"));
                }
            }
        }
    }

    /// A merge or a split: a run, then some present chunks handed out with their kept
    /// levels and let go, one after the other or all kept first; at times the whole
    /// engine is begun anew, as a region replaced whole is.
    fn hand_over(&mut self) {
        self.run();
        self.hand_overs += 1;
        let taken = (1, self.hand_overs);
        let anew = self.numbers.chance(40);
        let present: Vec<ChunkPos> = self.present.iter().copied().collect();
        let chosen: Vec<ChunkPos> = present
            .iter()
            .copied()
            .filter(|_| anew || self.numbers.chance(50))
            .collect();
        let interleave = self.numbers.chance(50);
        let mut kept = Vec::new();
        for &at in &chosen {
            kept.push((
                at,
                self.lighting.kept(at, taken).expect("a lit chunk is kept"),
            ));
            if interleave {
                self.lighting.left(at);
            }
        }
        if !interleave {
            for &at in &chosen {
                self.lighting.left(at);
            }
        }
        for (at, kept) in kept {
            self.present.remove(&at);
            // Levels kept from a chunk not handed back: dropped with some chance.
            if self.numbers.chance(85) {
                self.warm.insert(at, kept);
            }
        }
        if anew {
            for at in std::mem::take(&mut self.present) {
                self.lighting.left(at);
            }
            self.lighting = Lighting::new(self.sky);
        }
        self.log.push(format!(
            "handed over {chosen:?} as {taken:?}, anew {anew}, interleaved {interleave}"
        ));
    }

    /// Some warm chunks adopted, from one hand-over or several, at times run at once.
    fn adopt(&mut self) {
        let warm: Vec<ChunkPos> = self.warm.keys().copied().collect();
        if warm.is_empty() {
            return;
        }
        let all = self.numbers.chance(30);
        let mut adopted = Vec::new();
        for at in warm {
            if all || self.numbers.chance(40) {
                let kept = self.warm.remove(&at).expect("warm");
                self.lighting.adopt(at, kept);
                self.present.insert(at);
                adopted.push(at);
            }
        }
        self.log.push(format!("adopted {adopted:?}"));
    }

    /// A chunk arriving with light made beside its eight neighbours as they are, all
    /// present: adopted where 3.5's third clause lets it be, else lit whole. Run at once,
    /// as the region decides at its step.
    fn carried(&mut self) {
        // A chunk inside the square, none of the nine warm; those about it made present
        // (lit before, or in the same run), and it made absent.
        let Some(at) = self.pick_where(|case, at| {
            let inside = at.x > case.base.0
                && at.z > case.base.1
                && at.x < case.base.0 + case.side - 1
                && at.z < case.base.1 + case.side - 1;
            inside
                && std::iter::once(at)
                    .chain(around(at))
                    .all(|near| !case.warm.contains_key(&near))
        }) else {
            return;
        };
        if self.present.remove(&at) {
            self.lighting.left(at);
        }
        if !self.held.0.contains_key(&at) {
            let chunk = self.new_chunk();
            self.held.0.insert(at, chunk);
        }
        for near in around(at) {
            if !self.present.contains(&near) {
                if !self.held.0.contains_key(&near) {
                    let chunk = self.new_chunk();
                    self.held.0.insert(near, chunk);
                }
                self.arrive(near);
                self.log.push(format!("arrived {near:?}"));
            }
        }
        if self.numbers.chance(50) {
            self.run();
        }
        let neighbours = around(at).map(|near| self.held.0.get(&near));
        let mut made = light(&Around {
            centre: &self.held.0[&at],
            neighbours,
            sky: self.sky,
        });
        made.beside = GENERATOR;
        let chunk = self.held.0.get_mut(&at).expect("held");
        chunk.set_light(made);
        let clause_three = clause_three_holds(chunk, chunk.light().expect("just made"), self.sky);
        let kept = Kept {
            light: chunk.take_light().expect("just made"),
            walls: 0,
            taken: (2, self.hand_overs * 1000 + self.log.len() as u64),
        };
        if clause_three {
            self.lighting.adopt(at, kept);
            self.log.push(format!("carried light adopted for {at:?}"));
        } else {
            self.lighting.arrived(at);
            self.log.push(format!("carried light refused for {at:?}"));
        }
        self.present.insert(at);
        self.run();
    }

    fn run(&mut self) {
        self.lighting.run(&self.held);
        self.log.push("run".to_string());
        let present = self.present.clone();
        let is_present = |at: ChunkPos| present.contains(&at);
        for at in self.positions() {
            assert_eq!(
                self.lighting.has(at),
                present.contains(&at),
                "presence of {at:?}"
            );
            if !present.contains(&at) {
                let pos = BlockPos::new(at.x * 16, self.min_y, at.z * 16);
                assert_eq!(
                    self.lighting.at(pos),
                    None,
                    "no light where {at:?} is absent"
                );
                continue;
            }
            if let Some(difference) = difference(
                &self.lighting,
                &self.held,
                &is_present,
                at,
                self.sky,
                &mut self.scratch,
            ) {
                panic!("{difference}\n{}", self.log.join("\n"));
            }
        }
    }
}

/// Runs `body` on a case of each seed of the property, naming the seed of one that fails.
/// With `CLUSTINE_LIGHT_ENGINE_STATS` set, says what the cases did.
fn each_case(property: u64, body: impl Fn(&mut Case)) {
    let mut done: BTreeMap<String, usize> = BTreeMap::new();
    for seed in seeds(property) {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut case = Case::new(seed);
            body(&mut case);
            case.run();
            case.log
        }));
        match outcome {
            Ok(log) => {
                for line in log.iter().skip(1) {
                    let words = if line.starts_with("carried") { 3 } else { 1 };
                    let kind = line
                        .split_whitespace()
                        .take(words)
                        .collect::<Vec<_>>()
                        .join(" ");
                    *done.entry(kind).or_default() += 1;
                }
            }
            Err(failure) => {
                eprintln!("the case of seed {seed} failed (CLUSTINE_LIGHT_ENGINE_SEED={seed})");
                std::panic::resume_unwind(failure);
            }
        }
    }
    if std::env::var_os("CLUSTINE_LIGHT_ENGINE_STATS").is_some() {
        eprintln!("property {property}: {done:#?}");
    }
}

/// Every chunk of the square held and most present, lit once.
fn most_present(case: &mut Case) {
    for at in case.positions() {
        let chunk = case.new_chunk();
        case.held.0.insert(at, chunk);
        if case.numbers.chance(70) {
            case.arrive(at);
        }
    }
    case.run();
}

#[test]
fn after_any_history_of_changes_arrivals_departures_and_hand_overs_every_present_chunk_is_lit_as_the_function_lights_it()
 {
    each_case(1, |case| {
        most_present(case);
        for _ in 0..40 {
            case.step();
        }
    });
}

#[test]
fn after_changes_alone_every_present_chunk_is_lit_as_the_function_lights_it() {
    // Many changes between runs, no chunk coming or going: the checks of 3.4 alone.
    each_case(2, |case| {
        most_present(case);
        for _ in 0..4 {
            let changes = 1 + case.numbers.below(30);
            for _ in 0..changes {
                case.change();
            }
            case.run();
        }
    });
}

#[test]
fn chunks_handed_over_and_adopted_in_any_order_are_lit_as_the_function_lights_them() {
    // Every chunk present, then handed over (all of them, or some) and adopted in
    // batches, with changes, runs and other steps between: the hand-over's ordering at
    // its densest.
    each_case(3, |case| {
        for at in case.positions() {
            let chunk = case.new_chunk();
            case.held.0.insert(at, chunk);
            case.arrive(at);
        }
        case.run();
        for _ in 0..2 {
            case.hand_over();
            while !case.warm.is_empty() {
                case.adopt();
                if case.numbers.chance(50) {
                    case.change();
                }
                if case.numbers.chance(30) {
                    case.step();
                }
                if case.numbers.chance(60) {
                    case.run();
                }
            }
            case.run();
        }
    });
}
