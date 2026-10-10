//! The committed tables against what is known of the game: the heads of the packed
//! tables as ADR-0019 lays them out, and values of well-known blocks.
//!
//! The values here are written from knowledge of Minecraft, not read off the tables, so
//! that an extract program that asks the game the wrong question, or a packer that puts
//! an answer into the wrong bit, fails here.

use clustine_data::{
    BLOCK_ENTITY_TYPES, BLOCK_INFO, BLOCK_STATE_COUNT, BLOCKS, BiomeParameterList, BlockClass,
    BlockState, CollisionShape, FLUIDS, Face, FaceShape, PushReaction, SERVER_JAR_SHA1,
    SERVER_JAR_SHA1_BYTES, SERVER_JAR_URL, SYNCED_REGISTRIES, SupportType, block_id_by_name,
    block_properties, blocks,
};

const BLOCK_STATES: &[u8] = include_bytes!("../src/generated/block_states.bin");
const BIOME_PARAMETERS: &[u8] = include_bytes!("../src/generated/biome_parameters.bin");

const SUPPORTS: [SupportType; 3] = [SupportType::Full, SupportType::Center, SupportType::Rigid];

/// The state of `name` with these properties; the others have their default values.
fn state(name: &str, properties: &[(&str, &str)]) -> BlockState {
    BlockState::from_name_and_properties(name, properties.iter().copied())
        .unwrap_or_else(|| panic!("no state {name} {properties:?}"))
}

/// The head of a packed table, read here from the record's words and not by the
/// crate's own reader: the kind, the layout, the SHA-1, and for each section its
/// number of rows and the bytes of a row.
fn head(bytes: &[u8]) -> (u16, u16, &[u8], Vec<(usize, usize)>) {
    assert_eq!(&bytes[..4], b"CLT1");
    let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let u32_at = |at: usize| {
        u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize
    };
    let sections = (0..usize::from(u16_at(28)))
        .map(|section| (u32_at(30 + 8 * section), u32_at(34 + 8 * section)))
        .collect();
    (u16_at(4), u16_at(6), &bytes[8..28], sections)
}

fn hexadecimal(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn the_jar_is_pinned_by_version_checksum_and_url() {
    assert_eq!(SERVER_JAR_SHA1, "33680f5f2ac32864d6d7cf5e56a705fdb3e05f4c");
    assert_eq!(hexadecimal(&SERVER_JAR_SHA1_BYTES), SERVER_JAR_SHA1);
    assert!(SERVER_JAR_URL.starts_with("https://piston-data.mojang.com/"));
    assert!(SERVER_JAR_URL.contains(SERVER_JAR_SHA1));
}

#[test]
fn the_table_of_block_states_has_the_head_the_record_gives() {
    let (kind, layout, sha1, sections) = head(BLOCK_STATES);
    assert_eq!(kind, clustine_data::packed::KIND_BLOCK_STATES);
    assert_eq!(layout, clustine_data::block_states::LAYOUT);
    assert_eq!(hexadecimal(sha1), SERVER_JAR_SHA1);
    assert_eq!(sections.len(), 7);
    assert_eq!(sections[0], (BLOCK_STATE_COUNT as usize, 16));
    let row_bytes: Vec<usize> = sections.iter().map(|section| section.1).collect();
    assert_eq!(row_bytes, clustine_data::block_states::ROW_BYTES);
    let data: usize = sections.iter().map(|(rows, bytes)| rows * bytes).sum();
    assert_eq!(30 + 8 * sections.len() + data, BLOCK_STATES.len());
}

#[test]
fn the_table_of_biome_parameters_has_the_head_the_record_gives() {
    let (kind, layout, sha1, sections) = head(BIOME_PARAMETERS);
    assert_eq!(kind, clustine_data::packed::KIND_BIOME_PARAMETERS);
    assert_eq!(layout, clustine_data::biome_parameters::LAYOUT);
    assert_eq!(hexadecimal(sha1), SERVER_JAR_SHA1);
    assert_eq!(sections.len(), 2);
    assert!(sections.iter().all(|section| section.1 == 27));
    let data: usize = sections.iter().map(|(rows, bytes)| rows * bytes).sum();
    assert_eq!(30 + 8 * sections.len() + data, BIOME_PARAMETERS.len());
    assert_eq!(sections[0].0, BiomeParameterList::Overworld.len());
    assert_eq!(sections[1].0, BiomeParameterList::Nether.len());
}

#[test]
fn air_is_air_gives_no_light_and_is_in_nobodys_way() {
    for air in [blocks::AIR, blocks::CAVE_AIR, blocks::VOID_AIR] {
        let props = air.props();
        assert!(props.is_air());
        assert_eq!(props.light_emission(), 0);
        assert_eq!(props.light_dampening(), 0);
        assert!(props.propagates_skylight_down());
        assert!(props.can_be_replaced());
        assert!(!props.is_solid() && !props.is_solid_render() && !props.is_liquid());
        assert!(!props.occludes() && !props.occludes_by_shape());
        assert!(props.collision_is_empty() && !props.collision_is_full_block());
        assert_eq!(props.collision_shape(), CollisionShape::EMPTY);
        assert_eq!(props.collision_shape().boxes().count(), 0);
        assert!(!props.in_motion_heightmap() && !props.in_motion_heightmap_no_leaves());
        assert_eq!(props.fluid(), None);
        assert_eq!(props.post_process(), None);
        assert!(!props.has_block_entity());
        for support in SUPPORTS {
            assert_eq!(props.sturdy_faces(support), 0);
        }
        for face in Face::ALL {
            assert!(props.occlusion_face(face).is_empty());
        }
    }
}

#[test]
fn stone_is_a_full_solid_cube_that_stops_light() {
    let props = blocks::STONE.props();
    assert!(!props.is_air());
    assert!(props.is_solid() && props.is_solid_render());
    assert!(props.occludes());
    assert_eq!(props.light_emission(), 0);
    assert_eq!(props.light_dampening(), 15);
    assert!(!props.propagates_skylight_down());
    assert!(!props.can_be_replaced());
    assert!(props.in_motion_heightmap() && props.in_motion_heightmap_no_leaves());
    assert!(props.collision_is_full_block() && !props.collision_is_empty());
    assert_eq!(props.collision_shape(), CollisionShape::FULL_BLOCK);
    let boxes: Vec<[f64; 6]> = props.collision_shape().boxes().collect();
    assert_eq!(boxes, [[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]);
    for support in SUPPORTS {
        assert_eq!(props.sturdy_faces(support), 0b11_1111);
        for face in Face::ALL {
            assert!(props.is_face_sturdy(face, support));
        }
    }
    assert_eq!(props.push_reaction(), PushReaction::PushPull);
    assert_eq!(props.fluid(), None);
    assert!(!props.has_block_entity());
    assert!(!props.answers_beyond_its_state());
    // It stops light by its dampening; the shapes are for blocks that do not.
    assert!(!props.occludes_by_shape());
    assert_eq!(props.occlusion_face_set(), 0);
}

#[test]
fn water_at_each_level_is_the_fluid_the_game_has_there() {
    let water = FLUIDS.iter().position(|name| *name == "minecraft:water");
    let flowing = FLUIDS
        .iter()
        .position(|name| *name == "minecraft:flowing_water");
    assert_eq!(
        BLOCKS[block_id_by_name("minecraft:water").unwrap()].default_state,
        blocks::WATER
    );

    for level in 0..16u8 {
        let state = state("minecraft:water", &[("level", &level.to_string())]);
        let props = state.props();
        assert!(props.is_liquid() && props.can_be_replaced());
        assert!(!props.is_air() && !props.is_solid() && !props.is_solid_render());
        assert!(props.collision_is_empty());
        assert_eq!(props.light_dampening(), 1);
        assert_eq!(props.light_emission(), 0);
        // The heightmaps count water by its fluid; the tag is for what blocks motion.
        assert!(!props.in_motion_heightmap());

        let fluid = props.fluid().expect("water holds a fluid");
        match level {
            // The source: still water, full.
            0 => {
                assert_eq!(Some(usize::from(fluid.fluid)), water);
                assert!(fluid.source && !fluid.falling);
                assert_eq!(fluid.amount, 8);
            }
            // Flowing water thins out with the level.
            1..=7 => {
                assert_eq!(Some(usize::from(fluid.fluid)), flowing);
                assert!(!fluid.source && !fluid.falling);
                assert_eq!(fluid.amount, 8 - level);
            }
            // From 8 on the water falls and fills the block.
            _ => {
                assert_eq!(Some(usize::from(fluid.fluid)), flowing);
                assert!(!fluid.source && fluid.falling);
                assert_eq!(fluid.amount, 8);
            }
        }
    }
    assert_eq!(blocks::WATER.property("level"), Some("0"));
}

#[test]
fn lava_is_a_source_of_lava_that_gives_full_light() {
    let props = blocks::LAVA.props();
    let fluid = props.fluid().expect("lava holds a fluid");
    assert_eq!(FLUIDS[usize::from(fluid.fluid)], "minecraft:lava");
    assert!(fluid.source && !fluid.falling);
    assert_eq!(fluid.amount, 8);
    assert_eq!(props.light_emission(), 15);
    assert!(props.is_liquid() && props.can_be_replaced() && props.collision_is_empty());

    let flowing = state("minecraft:lava", &[("level", "3")]).props();
    let fluid = flowing.fluid().unwrap();
    assert_eq!(FLUIDS[usize::from(fluid.fluid)], "minecraft:flowing_lava");
    assert_eq!(
        (fluid.amount, fluid.source, fluid.falling),
        (5, false, false)
    );
    assert_eq!(flowing.light_emission(), 15);
}

#[test]
fn a_waterlogged_block_holds_a_water_source_and_is_no_liquid() {
    let dry = state("minecraft:oak_slab", &[("waterlogged", "false")]).props();
    let wet = state("minecraft:oak_slab", &[("waterlogged", "true")]).props();
    assert_eq!(dry.fluid(), None);
    let fluid = wet.fluid().expect("a waterlogged slab holds water");
    assert_eq!(FLUIDS[usize::from(fluid.fluid)], "minecraft:water");
    assert!(fluid.source && fluid.amount == 8);
    assert!(!wet.is_liquid() && !dry.is_liquid());
}

#[test]
fn light_sources_give_the_light_the_game_gives_them() {
    assert_eq!(blocks::GLOWSTONE.props().light_emission(), 15);
    assert_eq!(blocks::GLOWSTONE.props().light_dampening(), 15);
    assert_eq!(blocks::TORCH.props().light_emission(), 14);
    assert_eq!(blocks::SEA_LANTERN.props().light_emission(), 15);
    assert_eq!(blocks::MAGMA_BLOCK.props().light_emission(), 3);
    assert_eq!(blocks::REDSTONE_TORCH.props().light_emission(), 7);
    assert_eq!(blocks::BROWN_MUSHROOM.props().light_emission(), 1);
    assert_eq!(blocks::OBSIDIAN.props().light_emission(), 0);
    // A furnace gives light only while it burns.
    assert_eq!(
        state("minecraft:furnace", &[("lit", "false")])
            .props()
            .light_emission(),
        0
    );
    assert_eq!(
        state("minecraft:furnace", &[("lit", "true")])
            .props()
            .light_emission(),
        13
    );
}

#[test]
fn a_lower_slab_covers_the_bottom_face_and_the_lower_half_of_each_side() {
    let slab = state(
        "minecraft:oak_slab",
        &[("type", "bottom"), ("waterlogged", "false")],
    );
    let props = slab.props();
    assert!(props.occludes() && props.occludes_by_shape());
    assert!(!props.is_solid_render());
    assert!(props.occlusion_face(Face::Down).is_full());
    assert!(props.occlusion_face(Face::Up).is_empty());
    // North and south lie in x and y, west and east in y and z; y is the height.
    for face in [Face::North, Face::South] {
        let rectangles: Vec<[f64; 4]> = props.occlusion_face(face).rectangles().collect();
        assert_eq!(rectangles, [[0.0, 0.0, 1.0, 0.5]], "{face:?}");
    }
    for face in [Face::West, Face::East] {
        let rectangles: Vec<[f64; 4]> = props.occlusion_face(face).rectangles().collect();
        assert_eq!(rectangles, [[0.0, 0.0, 0.5, 1.0]], "{face:?}");
    }
    assert_eq!(
        FaceShape::FULL.rectangles().collect::<Vec<_>>(),
        [[0.0, 0.0, 1.0, 1.0]]
    );
    assert_eq!(FaceShape::EMPTY.rectangles().count(), 0);

    // Only its bottom carries anything.
    for support in SUPPORTS {
        assert_eq!(props.sturdy_faces(support), 1 << Face::Down as u8);
    }
    let boxes: Vec<[f64; 6]> = props.collision_shape().boxes().collect();
    assert_eq!(boxes, [[0.0, 0.0, 0.0, 1.0, 0.5, 1.0]]);
    assert!(!props.collision_is_empty() && !props.collision_is_full_block());
    assert!(props.in_motion_heightmap() && props.in_motion_heightmap_no_leaves());
}

#[test]
fn an_upper_slab_and_a_double_slab_cover_what_they_fill() {
    let top = state(
        "minecraft:oak_slab",
        &[("type", "top"), ("waterlogged", "false")],
    )
    .props();
    assert!(top.occlusion_face(Face::Up).is_full());
    assert!(top.occlusion_face(Face::Down).is_empty());
    let north: Vec<[f64; 4]> = top.occlusion_face(Face::North).rectangles().collect();
    assert_eq!(north, [[0.0, 0.5, 1.0, 1.0]]);
    for support in SUPPORTS {
        assert_eq!(top.sturdy_faces(support), 1 << Face::Up as u8);
    }

    let double = state(
        "minecraft:oak_slab",
        &[("type", "double"), ("waterlogged", "false")],
    )
    .props();
    assert!(double.is_solid_render());
    assert_eq!(double.light_dampening(), 15);
    assert!(double.collision_is_full_block());
    assert_eq!(double.sturdy_faces(SupportType::Full), 0b11_1111);
}

#[test]
fn a_straight_stair_covers_its_bottom_its_back_and_half_of_its_top() {
    // A stair that faces north rises towards the north: its back is the north side.
    let stair = state(
        "minecraft:oak_stairs",
        &[
            ("facing", "north"),
            ("half", "bottom"),
            ("shape", "straight"),
            ("waterlogged", "false"),
        ],
    );
    let props = stair.props();
    assert!(props.occludes_by_shape());
    assert!(props.occlusion_face(Face::Down).is_full());
    assert!(props.occlusion_face(Face::North).is_full());
    // The top is covered where the upper step is: the northern half (low z).
    let top: Vec<[f64; 4]> = props.occlusion_face(Face::Up).rectangles().collect();
    assert_eq!(top, [[0.0, 0.0, 1.0, 0.5]]);
    // The front shows the lower step only.
    let south: Vec<[f64; 4]> = props.occlusion_face(Face::South).rectangles().collect();
    assert_eq!(south, [[0.0, 0.0, 1.0, 0.5]]);
    // Each side is the profile of the stair: three quarters of the square.
    for face in [Face::West, Face::East] {
        let shape = props.occlusion_face(face);
        assert!(!shape.is_empty() && !shape.is_full());
        let area: f64 = shape
            .rectangles()
            .map(|[min_u, min_v, max_u, max_v]| (max_u - min_u) * (max_v - min_v))
            .sum();
        assert_eq!(area, 0.75, "{face:?}");
    }
    assert_eq!(
        props.occlusion_face(Face::West),
        props.occlusion_face(Face::East)
    );
    for support in SUPPORTS {
        assert_eq!(
            props.sturdy_faces(support),
            1 << Face::Down as u8 | 1 << Face::North as u8
        );
    }
    assert_eq!(props.collision_shape().boxes().count(), 2);

    // Turned upside down it covers the top instead, and the same back.
    let upside_down = stair.with_property("half", "top").unwrap().props();
    assert!(upside_down.occlusion_face(Face::Up).is_full());
    assert!(upside_down.occlusion_face(Face::North).is_full());
    assert!(!upside_down.occlusion_face(Face::Down).is_full());
}

#[test]
fn leaves_block_motion_in_one_heightmap_and_not_in_the_other() {
    for leaves in [
        blocks::OAK_LEAVES,
        blocks::SPRUCE_LEAVES,
        blocks::AZALEA_LEAVES,
    ] {
        let props = leaves.props();
        assert!(props.in_motion_heightmap());
        assert!(!props.in_motion_heightmap_no_leaves());
        assert!(!props.is_solid_render());
        assert_eq!(props.light_dampening(), 1);
        assert!(!props.propagates_skylight_down());
        assert!(props.collision_is_full_block());
        assert_eq!(props.push_reaction(), PushReaction::Popped);
    }
    let class = blocks::OAK_LEAVES.block_info().unwrap().class;
    assert_eq!(class, blocks::BIRCH_LEAVES.block_info().unwrap().class);
    assert_ne!(class, BlockClass::Block);
}

#[test]
fn a_door_is_thin_and_only_a_wooden_one_opens_by_a_click() {
    let oak = blocks::OAK_DOOR;
    let props = oak.props();
    assert!(!props.is_solid_render() && !props.is_air());
    assert!(!props.collision_is_empty() && !props.collision_is_full_block());
    let boxes: Vec<[f64; 6]> = props.collision_shape().boxes().collect();
    assert_eq!(boxes.len(), 1);
    let [min_x, min_y, min_z, max_x, max_y, max_z] = boxes[0];
    // Three sixteenths thick, the full height.
    assert_eq!((min_y, max_y), (0.0, 1.0));
    let thinnest = (max_x - min_x).min(max_z - min_z);
    assert_eq!(thinnest, 0.1875);
    assert_eq!(props.push_reaction(), PushReaction::Popped);
    assert_eq!(props.light_dampening(), 0);
    assert!(!props.has_block_entity());

    let oak_info = oak.block_info().unwrap();
    let iron_info = blocks::IRON_DOOR.block_info().unwrap();
    assert_eq!(oak_info.class, BlockClass::DoorBlock);
    assert_eq!(iron_info.class, BlockClass::DoorBlock);
    assert!(oak_info.used_by_click);
    assert!(!iron_info.used_by_click);
    assert_eq!(oak_info.block_entity_type, None);

    // An open door stands along another side than a closed one.
    let closed = state(
        "minecraft:oak_door",
        &[("facing", "north"), ("open", "false")],
    );
    let open = closed.with_property("open", "true").unwrap();
    assert_ne!(
        closed.props().collision_shape(),
        open.props().collision_shape()
    );

    assert!(blocks::OAK_TRAPDOOR.block_info().unwrap().used_by_click);
    assert!(!blocks::IRON_TRAPDOOR.block_info().unwrap().used_by_click);
}

#[test]
fn a_chest_keeps_a_block_entity_and_is_used_by_a_click() {
    let props = blocks::CHEST.props();
    assert!(props.has_block_entity());
    assert!(!props.is_solid_render());
    assert!(!props.collision_is_full_block());
    let info = blocks::CHEST.block_info().unwrap();
    assert_eq!(info.class, BlockClass::ChestBlock);
    assert!(info.used_by_click);
    let entity_type = info
        .block_entity_type
        .expect("a chest has a block entity type");
    assert_eq!(
        BLOCK_ENTITY_TYPES[usize::from(entity_type)],
        "minecraft:chest"
    );

    let furnace = blocks::FURNACE.block_info().unwrap();
    assert_eq!(
        BLOCK_ENTITY_TYPES[usize::from(furnace.block_entity_type.unwrap())],
        "minecraft:furnace"
    );
    assert!(furnace.used_by_click);

    // Plain blocks have neither.
    for plain in [
        blocks::STONE,
        blocks::DIRT,
        blocks::OAK_PLANKS,
        blocks::GLASS,
    ] {
        let info = plain.block_info().unwrap();
        assert_eq!(info.block_entity_type, None);
        assert!(!info.used_by_click);
        assert!(!plain.props().has_block_entity());
    }
    assert!(blocks::CRAFTING_TABLE.block_info().unwrap().used_by_click);
    assert!(blocks::LEVER.block_info().unwrap().used_by_click);
}

#[test]
fn every_block_with_a_block_entity_type_has_states_that_keep_one() {
    for (block, info) in BLOCKS.iter().zip(&BLOCK_INFO) {
        let keeps = block.default_state.props().has_block_entity();
        assert_eq!(keeps, info.block_entity_type.is_some(), "{}", block.name);
        if let Some(entity_type) = info.block_entity_type {
            assert!(usize::from(entity_type) < BLOCK_ENTITY_TYPES.len());
        }
    }
    assert_eq!(BLOCK_INFO.len(), BLOCKS.len());
    assert_eq!(FLUIDS[0], "minecraft:empty");
}

#[test]
fn magma_and_soul_sand_mark_the_block_above_and_a_mushroom_itself() {
    assert_eq!(blocks::MAGMA_BLOCK.props().post_process(), Some([0, 1, 0]));
    assert_eq!(blocks::SOUL_SAND.props().post_process(), Some([0, 1, 0]));
    assert_eq!(
        blocks::BROWN_MUSHROOM.props().post_process(),
        Some([0, 0, 0])
    );
    assert_eq!(blocks::RED_MUSHROOM.props().post_process(), Some([0, 0, 0]));
    assert_eq!(blocks::STONE.props().post_process(), None);
    assert_eq!(blocks::SAND.props().post_process(), None);
}

#[test]
fn replaceable_blocks_are_the_ones_placing_may_overwrite() {
    for replaceable in [
        blocks::AIR,
        blocks::WATER,
        blocks::SHORT_GRASS,
        blocks::FIRE,
    ] {
        assert!(replaceable.props().can_be_replaced());
    }
    for kept in [
        blocks::STONE,
        blocks::OAK_SAPLING,
        blocks::TORCH,
        blocks::OAK_LEAVES,
    ] {
        assert!(!kept.props().can_be_replaced());
    }
    assert!(
        state("minecraft:snow", &[("layers", "1")])
            .props()
            .can_be_replaced()
    );
}

#[test]
fn a_blocks_push_reaction_is_what_its_state_answers_and_no_more() {
    assert_eq!(
        blocks::OBSIDIAN.props().push_reaction(),
        PushReaction::Immovable
    );
    assert_eq!(
        blocks::PISTON.props().push_reaction(),
        PushReaction::Immovable
    );
    // A piston moves neither bedrock nor a chest, but not by this value: the game
    // refuses what cannot be broken and what keeps a block entity before it asks the
    // state. Their states answer like any other block's, and so does the table. (The
    // first version of this test expected `Immovable` of bedrock and was wrong.)
    assert_eq!(
        blocks::BEDROCK.props().push_reaction(),
        PushReaction::PushPull
    );
    assert_eq!(
        blocks::CHEST.props().push_reaction(),
        PushReaction::PushPull
    );
    assert_eq!(blocks::DIRT.props().push_reaction(), PushReaction::PushPull);
    assert_eq!(blocks::TORCH.props().push_reaction(), PushReaction::Popped);
    // Glazed terracotta is pushed and does not stick to a pulling piston.
    assert_eq!(
        blocks::WHITE_GLAZED_TERRACOTTA.props().push_reaction(),
        PushReaction::Push
    );
}

#[test]
fn a_fence_is_taller_than_its_block_and_carries_only_in_its_middle() {
    let post = state(
        "minecraft:oak_fence",
        &[
            ("north", "false"),
            ("south", "false"),
            ("east", "false"),
            ("west", "false"),
            ("waterlogged", "false"),
        ],
    )
    .props();
    let boxes: Vec<[f64; 6]> = post.collision_shape().boxes().collect();
    assert_eq!(boxes, [[0.375, 0.0, 0.375, 0.625, 1.5, 0.625]]);
    assert!(post.is_face_sturdy(Face::Up, SupportType::Center));
    assert!(!post.is_face_sturdy(Face::Up, SupportType::Full));
    assert!(!post.is_face_sturdy(Face::Up, SupportType::Rigid));
}

#[test]
fn light_permeable_is_the_opposite_of_solid_render_for_every_state() {
    for id in 0..BLOCK_STATE_COUNT {
        let props = BlockState(id as u16).props();
        assert_eq!(
            props.is_light_permeable(),
            !props.is_solid_render(),
            "state {id}"
        );
    }
}

#[test]
fn every_row_is_consistent_in_itself() {
    let mut beyond = Vec::new();
    for id in 0..BLOCK_STATE_COUNT {
        let state = BlockState(id as u16);
        let props = state.props();
        assert!(props.light_emission() <= 15 && props.light_dampening() <= 15);
        assert_eq!(
            props.collision_is_empty(),
            props.collision_shape() == CollisionShape::EMPTY,
            "state {id}"
        );
        if props.collision_is_full_block() {
            assert_eq!(
                props.collision_shape(),
                CollisionShape::FULL_BLOCK,
                "state {id}"
            );
        }
        if !props.occludes_by_shape() {
            assert_eq!(props.occlusion_face_set(), 0, "state {id}");
        }
        if props.is_solid_render() {
            assert_eq!(props.light_dampening(), 15, "state {id}");
        }
        if props.is_air() {
            assert!(
                props.collision_is_empty() && props.fluid().is_none(),
                "state {id}"
            );
        }
        if let Some(fluid) = props.fluid() {
            assert!(usize::from(fluid.fluid) < FLUIDS.len(), "state {id}");
            assert_ne!(FLUIDS[usize::from(fluid.fluid)], "minecraft:empty");
            assert!((1..=8).contains(&fluid.amount), "state {id}");
        }
        // What the no-leaves tag has, the tag with leaves has too.
        if props.in_motion_heightmap_no_leaves() {
            assert!(props.in_motion_heightmap(), "state {id}");
        }
        for face in Face::ALL {
            for [min_u, min_v, max_u, max_v] in props.occlusion_face(face).rectangles() {
                assert!(0.0 <= min_u && min_u < max_u && max_u <= 1.0, "state {id}");
                assert!(0.0 <= min_v && min_v < max_v && max_v <= 1.0, "state {id}");
            }
        }
        if props.answers_beyond_its_state() {
            let name = state.block().unwrap().name;
            if !beyond.contains(&name) {
                beyond.push(name);
            }
        }
    }
    // The blocks that look at their block entity or stand shifted by their position.
    assert_eq!(beyond.len(), 21, "{beyond:?}");
    assert!(beyond.contains(&"minecraft:shulker_box"));
    assert!(beyond.contains(&"minecraft:moving_piston"));
    assert!(beyond.contains(&"minecraft:bamboo"));
    assert!(beyond.contains(&"minecraft:pointed_dripstone"));
    assert!(blocks::SHULKER_BOX.props().collision_is_full_block());
}

#[test]
fn every_shape_a_row_points_at_is_there() {
    use clustine_data::block_states::{
        collision_shape_count, face_shape_count, occlusion_face_set_count,
    };
    assert!(occlusion_face_set_count() >= 2 && face_shape_count() >= 2);
    for id in 0..BLOCK_STATE_COUNT {
        let props = BlockState(id as u16).props();
        assert!(usize::from(props.occlusion_face_set()) < occlusion_face_set_count());
        assert!(usize::from(props.collision_shape().index()) < collision_shape_count());
        for face in Face::ALL {
            assert!(usize::from(props.occlusion_face(face).index()) < face_shape_count());
        }
        // Reading the boxes walks the sections of starts and of boxes.
        let _ = props.collision_shape().boxes().count();
        let _ = props.post_process();
    }
}

#[test]
fn a_state_is_found_again_by_its_name_and_properties() {
    for block in &BLOCKS {
        for id in block.first_state.0..=block.last_state.0 {
            let state = BlockState(id);
            let found = BlockState::from_name_and_properties(block.name, state.properties());
            assert_eq!(found, Some(state), "{}", block.name);
        }
        assert_eq!(BlockState::parse(block.name), Some(block.default_state));
        assert_eq!(
            BLOCKS[block_id_by_name(block.name).unwrap()].name,
            block.name
        );
    }
    assert_eq!(block_id_by_name("minecraft:no_such_block"), None);
    assert_eq!(block_id_by_name("stone"), None);
}

#[test]
fn states_are_written_and_read_the_ways_the_games_data_writes_them() {
    // The default state of grass is the one without snow.
    assert_eq!(
        BlockState::parse("minecraft:grass_block"),
        Some(blocks::GRASS_BLOCK)
    );
    assert_eq!(blocks::GRASS_BLOCK.property("snowy"), Some("false"));
    let snowy = BlockState::parse("minecraft:grass_block[snowy=true]").unwrap();
    assert_eq!(snowy, BLOCKS[8].first_state);
    assert_eq!(snowy.properties().collect::<Vec<_>>(), [("snowy", "true")]);

    // A property that is left out has its default value.
    let stairs = BlockState::parse("minecraft:oak_stairs[facing=east,half=top]").unwrap();
    assert_eq!(stairs.property("facing"), Some("east"));
    assert_eq!(stairs.property("half"), Some("top"));
    assert_eq!(stairs.property("shape"), Some("straight"));
    assert_eq!(stairs.property("waterlogged"), Some("false"));
    assert_eq!(
        stairs,
        state(
            "minecraft:oak_stairs",
            &[("half", "top"), ("facing", "east")]
        )
    );
    assert_eq!(stairs.block().unwrap().name, "minecraft:oak_stairs");

    // What the game does not have is not a state.
    assert_eq!(BlockState::parse("minecraft:oak_stairs[facing=up]"), None);
    assert_eq!(BlockState::parse("minecraft:oak_stairs[colour=red]"), None);
    assert_eq!(BlockState::parse("minecraft:oak_stairs[facing]"), None);
    assert_eq!(BlockState::parse("minecraft:oak_stairs[facing=east"), None);
    assert_eq!(BlockState::parse("minecraft:stone[snowy=true]"), None);
    assert_eq!(blocks::STONE.properties().count(), 0);
    assert_eq!(blocks::STONE.property("snowy"), None);
    assert_eq!(BlockState(u16::MAX).properties().count(), 0);

    // The game numbers the states of a block by its properties sorted by name.
    let names: Vec<&str> = block_properties(block_id_by_name("minecraft:chest").unwrap())
        .map(|property| property.name)
        .collect();
    assert_eq!(names, ["facing", "type", "waterlogged"]);
    let facing = block_properties(block_id_by_name("minecraft:chest").unwrap())
        .next()
        .unwrap();
    assert_eq!(facing.values, ["north", "south", "west", "east"]);
}

#[test]
fn the_nether_has_the_five_climates_the_game_gives_it() {
    let biomes = SYNCED_REGISTRIES
        .iter()
        .find(|registry| registry.name == "minecraft:worldgen/biome")
        .unwrap();
    let list = BiomeParameterList::Nether;
    assert_eq!(list.len(), 5);
    assert!(!list.is_empty());
    // Temperature, humidity and the offset; everything else is zero in the Nether.
    let expected = [
        ("minecraft:nether_wastes", 0, 0, 0),
        ("minecraft:soul_sand_valley", 0, -5000, 0),
        ("minecraft:crimson_forest", 4000, 0, 0),
        ("minecraft:warped_forest", 0, 5000, 3750),
        ("minecraft:basalt_deltas", -5000, 0, 1750),
    ];
    for (entry, (biome, temperature, humidity, offset)) in list.iter().zip(expected) {
        assert_eq!(biomes.entries[usize::from(entry.biome)], biome);
        assert_eq!(entry.bounds[0], [temperature, temperature], "{biome}");
        assert_eq!(entry.bounds[1], [humidity, humidity], "{biome}");
        for parameter in 2..6 {
            assert_eq!(entry.bounds[parameter], [0, 0], "{biome}");
        }
        assert_eq!(entry.offset, offset, "{biome}");
    }
}

#[test]
fn the_overworld_has_its_climates_in_the_games_order() {
    let biomes = SYNCED_REGISTRIES
        .iter()
        .find(|registry| registry.name == "minecraft:worldgen/biome")
        .unwrap();
    let list = BiomeParameterList::Overworld;
    assert_eq!(list.len(), 7594);

    // The game's list begins with the mushroom fields, far out at sea: the lowest
    // continentalness there is, at the surface and at depth one.
    let first = list.get(0);
    assert_eq!(
        biomes.entries[usize::from(first.biome)],
        "minecraft:mushroom_fields"
    );
    assert_eq!(first.bounds[2], [-12000, -10500]);
    assert_eq!(first.bounds[4], [0, 0]);
    assert_eq!(list.get(1).bounds[4], [10000, 10000]);
    assert_eq!(first.offset, 0);

    let mut seen = vec![false; biomes.entries.len()];
    for entry in list.iter() {
        for [low, high] in entry.bounds {
            assert!(low <= high);
        }
        seen[usize::from(entry.biome)] = true;
    }
    let found = |name: &str| seen[biomes.id_of(name).unwrap() as usize];
    for overworld in [
        "minecraft:plains",
        "minecraft:deep_dark",
        "minecraft:frozen_peaks",
    ] {
        assert!(found(overworld), "{overworld}");
    }
    for elsewhere in [
        "minecraft:nether_wastes",
        "minecraft:the_end",
        "minecraft:the_void",
    ] {
        assert!(!found(elsewhere), "{elsewhere}");
    }
}
