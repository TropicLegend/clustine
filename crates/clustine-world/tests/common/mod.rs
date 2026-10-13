//! What the tests of a chunk share: real states of each kind the record names, and the
//! record's rules written out a second time, plainly, to judge the chunk by
//! (`docs/adr/0022-what-a-chunk-carries.md`, sections 1 and 2).

// Each file of tests is a program of its own and uses some of this, not all.
#![allow(dead_code)]

use clustine_data::block_states::Face;
use clustine_data::{BLOCK_INFO, BlockState, DIMENSION_TYPES, DimensionType, blocks};
use clustine_world::{
    Biome, COLUMNS_PER_CHUNK, Chunk, InChunk, Light, ScheduledTick, Section, SectionLight, Ticked,
};

pub const PLAINS: Biome = Biome(0);

/// A sequence of numbers that looks random and is the same every time, so that a
/// failure can be had again.
pub fn numbers(mut seed: u64) -> impl Iterator<Item = usize> {
    std::iter::repeat_with(move || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) as usize
    })
}

pub fn overworld() -> &'static DimensionType {
    DIMENSION_TYPES
        .iter()
        .find(|dimension| dimension.name == "minecraft:overworld")
        .expect("there is an overworld")
}

pub fn state(text: &str) -> BlockState {
    BlockState::parse(text).unwrap_or_else(|| panic!("{text} is a block state"))
}

pub fn waterlogged_slab() -> BlockState {
    state("minecraft:oak_slab[waterlogged=true]")
}

pub fn flowing_water() -> BlockState {
    state("minecraft:water[level=3]")
}

pub fn waterlogged_leaves() -> BlockState {
    state("minecraft:oak_leaves[waterlogged=true]")
}

pub fn chest_facing_east() -> BlockState {
    state("minecraft:chest[facing=east]")
}

pub fn lit_furnace() -> BlockState {
    state("minecraft:furnace[lit=true]")
}

pub fn powered_door() -> BlockState {
    state("minecraft:oak_door[powered=true]")
}

/// States of every kind a chunk treats differently: the airs, blocks that light
/// cannot tell apart, fluids and blocks with a fluid in them, leaves, blocks with
/// block entities of several types and two states of one type, blocks that give
/// light, that let it through and that stop it by their shape.
pub fn kinds() -> Vec<BlockState> {
    vec![
        blocks::AIR,
        blocks::CAVE_AIR,
        blocks::STONE,
        blocks::DIRT,
        blocks::GRASS_BLOCK,
        blocks::IRON_ORE,
        blocks::COAL_ORE,
        blocks::WATER,
        flowing_water(),
        blocks::LAVA,
        waterlogged_slab(),
        blocks::OAK_SLAB,
        blocks::OAK_LEAVES,
        waterlogged_leaves(),
        blocks::CHEST,
        chest_facing_east(),
        blocks::TRAPPED_CHEST,
        blocks::OAK_SIGN,
        blocks::FURNACE,
        lit_furnace(),
        blocks::BARREL,
        blocks::TORCH,
        blocks::GLASS,
        blocks::GLOWSTONE,
        blocks::OAK_DOOR,
        powered_door(),
    ]
}

/// The type of block entity the block of `state` has, by the record's rule I2:
/// `BLOCK_INFO[..].block_entity_type` of the state's block.
pub fn entity_type(state: BlockState) -> Option<u8> {
    BLOCK_INFO[state.block_id().expect("a state of the game")].block_entity_type
}

/// Whether the two states differ in anything light sees, by the record's list as
/// ADR-0023 changed it: the light they give, the light they take, whether and with
/// which faces they stop light, whether the sky passes down through them, and
/// whether they are air.
pub fn light_sees(old: BlockState, new: BlockState) -> bool {
    let (old, new) = (old.props(), new.props());
    old.is_air() != new.is_air()
        || old.light_emission() != new.light_emission()
        || old.light_dampening() != new.light_dampening()
        || old.occludes() != new.occludes()
        || old.occludes_by_shape() != new.occludes_by_shape()
        || Face::ALL
            .into_iter()
            .any(|face| old.occlusion_face(face) != new.occlusion_face(face))
        || old.propagates_skylight_down() != new.propagates_skylight_down()
}

/// For the four heightmaps, in the order world surface, ocean floor, motion blocking,
/// motion blocking without leaves, whether `state` counts: by the record, "world
/// surface is not air; the ocean floor is the first heightmap tag; motion blocking is
/// that tag or a fluid; motion blocking without leaves is the second tag or a fluid".
pub fn counts_for(state: BlockState) -> [bool; 4] {
    let props = state.props();
    let fluid = props.fluid().is_some();
    [
        !props.is_air(),
        props.in_motion_heightmap(),
        props.in_motion_heightmap() || fluid,
        props.in_motion_heightmap_no_leaves() || fluid,
    ]
}

/// The four heightmaps of `chunk` in that order, each found by a scan of every column
/// from the top: the height above the highest block that counts, from the bottom of
/// the chunk, and 0 where none does.
pub fn scanned(chunk: &Chunk) -> [[u16; COLUMNS_PER_CHUNK]; 4] {
    let mut maps = [[0; COLUMNS_PER_CHUNK]; 4];
    let top = chunk.min_y() + chunk.height() as i32;
    for z in 0..16 {
        for x in 0..16 {
            for (map, heights) in maps.iter_mut().enumerate() {
                let highest = (chunk.min_y()..top).rev().find(|y| {
                    let state = chunk.get(x, *y, z).expect("inside the chunk");
                    counts_for(state)[map]
                });
                heights[z << 4 | x] = highest.map_or(0, |y| (y - chunk.min_y() + 1) as u16);
            }
        }
    }
    maps
}

/// The chunk's own four heightmaps, in the same order.
pub fn kept(chunk: &Chunk) -> [[u16; COLUMNS_PER_CHUNK]; 4] {
    let heights = chunk.heights();
    [
        heights.world_surface,
        heights.ocean_floor,
        heights.motion_blocking,
        heights.motion_blocking_no_leaves,
    ]
}

/// Fails unless the rule I3 holds of `chunk`: its heightmaps and its sections' counts
/// are those of its blocks.
pub fn counts_and_heights_follow_the_blocks(chunk: &Chunk) {
    let (kept, scanned) = (kept(chunk), scanned(chunk));
    let names = [
        "world surface",
        "ocean floor",
        "motion blocking",
        "motion blocking without leaves",
    ];
    for map in 0..4 {
        for column in 0..COLUMNS_PER_CHUNK {
            assert_eq!(
                kept[map][column],
                scanned[map][column],
                "{} at x {} z {}",
                names[map],
                column & 15,
                column >> 4
            );
        }
    }
    assert_eq!(chunk.surface_heights(), scanned[0]);
    for (index, section) in chunk.sections().iter().enumerate() {
        let non_air = section.states().filter(|s| !s.props().is_air()).count();
        let fluids = section
            .states()
            .filter(|s| s.props().fluid().is_some())
            .count();
        assert_eq!(
            usize::from(section.non_air_count()),
            non_air,
            "section {index}"
        );
        assert_eq!(
            usize::from(section.fluid_count()),
            fluids,
            "section {index}"
        );
    }
}

/// Fails unless the rule I2 holds of `chunk`: a position has a block entity exactly
/// if the block of its state has a block entity type, and the entity is of that type.
pub fn block_entities_follow_the_blocks(chunk: &Chunk) {
    let mut owed = 0;
    let top = chunk.min_y() + chunk.height() as i32;
    for y in chunk.min_y()..top {
        for z in 0..16 {
            for x in 0..16 {
                let state = chunk.get(x, y, z).expect("inside the chunk");
                let entity = chunk.block_entities().get(&InChunk::new(x, y, z));
                assert_eq!(
                    entity.map(|entity| entity.kind),
                    entity_type(state),
                    "at {x} {y} {z}, where {state:?} is"
                );
                owed += usize::from(entity.is_some());
            }
        }
    }
    // None beside those, such as one outside the chunk.
    assert_eq!(chunk.block_entities().len(), owed);
}

/// Fails unless what the rule I1 says of the failure bit holds: it is set only where
/// the version is not 0.
pub fn the_failure_bit_is_only_of_a_generated_chunk(chunk: &Chunk) {
    assert!(!chunk.with_failure() || chunk.generated() != 0);
}

/// Light of the right shape for `chunk`, with something in it that is neither dark
/// nor full nor absent.
pub fn some_light(chunk: &Chunk, made_by: u16) -> Light {
    let entries = chunk.sections().len() + 2;
    let mut levels = Box::new([0; clustine_world::LIGHT_ARRAY_LENGTH]);
    levels[17] = 0x5a;
    let mut sky = vec![SectionLight::Absent; entries];
    sky[0] = SectionLight::Dark;
    sky[1] = SectionLight::Levels(levels.clone());
    sky[2] = SectionLight::Full;
    let mut block = vec![SectionLight::Dark; entries];
    levels[18] = 0x03;
    block[1] = SectionLight::Levels(levels);
    Light {
        made_by,
        beside: 0,
        sky,
        block,
    }
}

pub fn some_ticks() -> Vec<ScheduledTick> {
    vec![
        ScheduledTick {
            at: InChunk::new(3, 40, 5),
            of: Ticked::Fluid(1),
            delay: 5,
            priority: 0,
        },
        ScheduledTick {
            at: InChunk::new(1, -20, 15),
            of: Ticked::Block(77),
            delay: 2,
            priority: -1,
        },
    ]
}

pub fn some_marks() -> Vec<InChunk> {
    vec![
        InChunk::new(0, -64, 0),
        InChunk::new(3, 40, 5),
        InChunk::new(15, 100, 15),
    ]
}

/// A chunk of `sections` sections of air whose lowest block is at `min_y`.
pub fn of_air(min_y: i32, sections: usize) -> Chunk {
    Chunk::from_sections(min_y, vec![Section::filled(blocks::AIR, PLAINS); sections])
}
