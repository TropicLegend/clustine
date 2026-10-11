//! Turning world state into packets.

use clustine_data::{BLOCK_STATE_COUNT, synced_registry};
use clustine_protocol::chunk::{
    PaletteKind, PalettedContainer, SectionData, bits_for, encode_sections, pack_heightmap,
};
use clustine_protocol::packets::play::{
    BlockEntity, Heightmap, LevelChunkWithLight, LightData, heightmap,
};
use clustine_world::{Chunk, ChunkPos, LIGHT_ARRAY_LENGTH, Section, SectionLight, sky_light};

/// The palette layout for block states.
pub(crate) const BLOCK_PALETTE: PaletteKind = PaletteKind::blocks(BLOCK_STATE_COUNT as usize);

/// The palette layout for biomes, which depends on how many biomes the client was sent.
pub(crate) fn biome_palette() -> PaletteKind {
    let biomes = synced_registry("minecraft:worldgen/biome").expect("biomes are synchronised");
    PaletteKind::biomes(biomes.entries.len())
}

/// The packet that sends `chunk`, located at `position`, to a client. It is made from
/// what the chunk carries (`docs/adr/0022-what-a-chunk-carries.md`, section 5): its
/// heightmaps, its sections' counts, blocks and biomes, its block entities and its
/// light.
pub(crate) fn chunk_packet(position: ChunkPos, chunk: &Chunk) -> LevelChunkWithLight {
    let heights = chunk.heights();
    let bits = bits_for(chunk.height() + 1);
    let heightmaps = [
        (
            heightmap::MOTION_BLOCKING_NO_LEAVES,
            &heights.motion_blocking_no_leaves,
        ),
        (heightmap::MOTION_BLOCKING, &heights.motion_blocking),
        (heightmap::WORLD_SURFACE, &heights.world_surface),
    ]
    .map(|(kind, map)| Heightmap {
        kind,
        data: pack_heightmap(map, bits),
    });

    // Nothing of a block entity's data is sent until its type is known to need
    // something: a chest shows as a chest without, and what a generated chest carries
    // (the seed of what is in it) is nobody's to be told. The list of types that are
    // sent something starts empty.
    let block_entities = chunk
        .block_entities()
        .iter()
        .map(|(at, entity)| BlockEntity {
            packed_xz: at.x << 4 | at.z,
            y: at.y,
            kind: entity.kind.into(),
            data: None,
        });

    let sections: Vec<_> = chunk.sections().iter().map(section_data).collect();
    LevelChunkWithLight {
        chunk_x: position.x,
        chunk_z: position.z,
        heightmaps: heightmaps.into(),
        sections: encode_sections(&sections, BLOCK_PALETTE, biome_palette()),
        block_entities: block_entities.collect(),
        light: light_data(chunk),
    }
}

fn section_data(section: &Section) -> SectionData {
    let blocks = match section.uniform_state() {
        Some(state) => PalettedContainer::Single(state.0.into()),
        None => {
            PalettedContainer::from_values(section.states().map(|state| state.0.into()).collect())
        }
    };
    let biomes = match section.uniform_biome() {
        Some(biome) => PalettedContainer::Single(biome.0.into()),
        None => {
            PalettedContainer::from_values(section.biomes().map(|biome| biome.0.into()).collect())
        }
    };
    SectionData {
        block_count: section.non_air_count() as i16,
        fluid_count: section.fluid_count() as i16,
        blocks,
        biomes,
    }
}

/// The light of `chunk` for its packet: the light the chunk carries, where it has any,
/// and otherwise the sky light reckoned by columns, with block light dark wherever
/// sky light is said. What an edge does once a change has taken a chunk's light is
/// the record of light at the edge's.
fn light_data(chunk: &Chunk) -> LightData {
    let reckoned;
    let (sky, block) = match chunk.light() {
        Some(light) => (&light.sky, Some(&light.block)),
        None => {
            reckoned = sky_light(chunk);
            (&reckoned, None)
        }
    };
    let mut light = LightData::default();
    let (mut sky_mask, mut empty_sky_mask) = (0u64, 0u64);
    let (mut block_mask, mut empty_block_mask) = (0u64, 0u64);
    for (index, section) in sky.iter().enumerate() {
        let bit = 1 << index;
        list(
            section,
            bit,
            &mut sky_mask,
            &mut empty_sky_mask,
            &mut light.sky,
        );
        match block {
            Some(block) => {
                let arrays = &mut light.block;
                list(
                    &block[index],
                    bit,
                    &mut block_mask,
                    &mut empty_block_mask,
                    arrays,
                );
            }
            // No light sources are reckoned: dark wherever sky light is said.
            None if *section != SectionLight::Absent => empty_block_mask |= bit,
            None => {}
        }
    }
    light.sky_mask = vec![sky_mask];
    light.empty_sky_mask = vec![empty_sky_mask];
    if block_mask != 0 {
        light.block_mask = vec![block_mask];
    }
    light.empty_block_mask = vec![empty_block_mask];
    light
}

/// Lists one light section in a packet's masks and arrays.
fn list(
    section: &SectionLight,
    bit: u64,
    mask: &mut u64,
    empty: &mut u64,
    arrays: &mut Vec<Vec<u8>>,
) {
    match section {
        // Listed in neither mask: for sky light, the client assumes full light.
        SectionLight::Absent => {}
        SectionLight::Dark => *empty |= bit,
        // Full light is an array of fifteens to a client.
        SectionLight::Full => {
            *mask |= bit;
            arrays.push(vec![0xff; LIGHT_ARRAY_LENGTH]);
        }
        SectionLight::Levels(levels) => {
            *mask |= bit;
            arrays.push(levels.to_vec());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use clustine_botswarm::{Bot, Oracle};
    use clustine_data::{BlockState, blocks};
    use clustine_protocol::chunk::{decode_sections, unpack_heightmap};
    use clustine_protocol::codec::Reader;
    use clustine_protocol::packets::play::ClientboundPlay;
    use clustine_protocol::packets::{self};
    use clustine_world::ChunkGenerator;
    use clustine_worldgen::FlatGenerator;

    use super::*;

    fn flat_chunk() -> Chunk {
        FlatGenerator::classic().generate(ChunkPos::new(0, 0))
    }

    #[test]
    fn flat_chunk_packet_has_the_expected_shape() {
        let chunk = flat_chunk();
        let packet = chunk_packet(ChunkPos::new(-3, 8), &chunk);
        assert_eq!((packet.chunk_x, packet.chunk_z), (-3, 8));

        // Nine bits per height, seven heights per word.
        assert_eq!(packet.heightmaps.len(), 3);
        for heightmap in &packet.heightmaps {
            assert_eq!(heightmap.data.len(), 37);
            assert_eq!(unpack_heightmap(&heightmap.data, 9), Some([4; 256]));
        }

        let sections =
            decode_sections(&packet.sections, 24, BLOCK_PALETTE, biome_palette()).unwrap();
        assert_eq!(sections[0].block_count, 1024);
        assert_eq!(sections[0].blocks.get(0), i32::from(blocks::BEDROCK.0));
        assert_eq!(
            sections[0].blocks.get(3 << 8),
            i32::from(blocks::GRASS_BLOCK.0)
        );
        assert_eq!(sections[0].blocks.get(4 << 8), i32::from(blocks::AIR.0));
        assert!(sections[1..].iter().all(|section| section.block_count == 0));
        // An empty section takes eight bytes; the bottom one has a 4-bit palette of four.
        assert_eq!(packet.sections.len(), 2060 + 23 * 8);

        assert_eq!(packet.light.sky_mask, [0b110]);
        assert_eq!(packet.light.empty_sky_mask, [0b001]);
        assert_eq!(packet.light.empty_block_mask, [0b111]);
        assert_eq!(packet.light.sky.len(), 2);
        assert!(packet.light.block.is_empty());
    }

    #[test]
    fn chunk_packet_survives_the_codec() {
        let packet = chunk_packet(ChunkPos::new(1, 2), &flat_chunk());
        let bytes = packets::encode(&packet);
        assert_eq!(
            ClientboundPlay::decode(&bytes),
            Ok(ClientboundPlay::LevelChunkWithLight(packet))
        );
        // The packet id is followed by the chunk coordinates.
        let mut reader = Reader::new(&bytes);
        reader.var_int().unwrap();
        assert_eq!((reader.i32(), reader.i32()), (Ok(1), Ok(2)));
    }

    #[test]
    fn fluids_are_counted() {
        let mut chunk = flat_chunk();
        chunk.set(0, -60, 0, blocks::WATER);
        chunk.set(1, -60, 0, blocks::LAVA);
        let data = section_data(&chunk.sections()[0]);
        assert_eq!(data.block_count, 1026);
        assert_eq!(data.fluid_count, 2);
    }

    /// A waterlogged block has a fluid, and is counted as one.
    #[test]
    fn a_waterlogged_block_counts_as_a_fluid() {
        let waterlogged = {
            let slab = blocks::OAK_SLAB.block().unwrap();
            let mut states = (slab.first_state.0..=slab.last_state.0).map(BlockState);
            states.find(|state| state.has_fluid()).unwrap()
        };
        let mut chunk = flat_chunk();
        chunk.set(0, -60, 0, waterlogged);
        let data = section_data(&chunk.sections()[0]);
        assert_eq!((data.block_count, data.fluid_count), (1025, 1));
    }

    /// The three heightmaps are the chunk's own: water counts for the two of motion
    /// and leaves for one of them, and all count for the world's surface.
    #[test]
    fn the_packets_heightmaps_are_the_chunks_own() {
        let mut chunk = flat_chunk();
        chunk.set(1, -50, 0, blocks::WATER);
        chunk.set(2, -40, 0, blocks::OAK_LEAVES);
        chunk.set(3, -30, 0, blocks::SHORT_GRASS);
        let packet = chunk_packet(ChunkPos::new(0, 0), &chunk);
        let heights = |kind| {
            let map = packet
                .heightmaps
                .iter()
                .find(|map| map.kind == kind)
                .unwrap();
            let heights = unpack_heightmap(&map.data, 9).unwrap();
            [heights[0], heights[1], heights[2], heights[3]]
        };
        assert_eq!(heights(heightmap::WORLD_SURFACE), [4, 15, 25, 35]);
        assert_eq!(heights(heightmap::MOTION_BLOCKING), [4, 15, 25, 4]);
        assert_eq!(heights(heightmap::MOTION_BLOCKING_NO_LEAVES), [4, 15, 4, 4]);
    }

    /// A block entity goes out with its place and its type and without its data,
    /// whatever data it has.
    #[test]
    fn a_block_entity_is_sent_without_its_data() {
        let mut chunk = flat_chunk();
        chunk.set(5, -60, 9, blocks::CHEST);
        let at = clustine_world::InChunk::new(5, -60, 9);
        chunk.set_block_entity_data(at, vec![1, 2, 3]).unwrap();
        let kind = chunk.block_entities()[&at].kind;
        let packet = chunk_packet(ChunkPos::new(0, 0), &chunk);
        let expected = BlockEntity {
            packed_xz: 5 << 4 | 9,
            y: -60,
            kind: kind.into(),
            data: None,
        };
        assert_eq!(packet.block_entities, [expected]);
        let bytes = packets::encode(&packet);
        assert_eq!(
            ClientboundPlay::decode(&bytes),
            Ok(ClientboundPlay::LevelChunkWithLight(packet))
        );
    }

    /// A section's 64 biomes go out as they are.
    #[test]
    fn a_section_of_several_biomes_is_sent_with_each_cells() {
        let mut sections = flat_chunk().sections().to_vec();
        let cells = std::array::from_fn(|cell| clustine_world::Biome((cell % 5) as u16));
        sections[3].set_biomes(Box::new(cells));
        let chunk = Chunk::from_sections(-64, sections);
        let packet = chunk_packet(ChunkPos::new(0, 0), &chunk);
        let decoded =
            decode_sections(&packet.sections, 24, BLOCK_PALETTE, biome_palette()).unwrap();
        for cell in 0..64 {
            assert_eq!(decoded[3].biomes.get(cell), (cell % 5) as i32);
        }
    }

    /// The light is the chunk's where it carries any: full light is an array of
    /// fifteens, dark is said by the mask, and absent is in neither. A chunk that a
    /// change has taken the light from is lit by the reckoning of columns, as before.
    #[test]
    fn the_packets_light_is_the_chunks_where_it_carries_any() {
        let mut chunk = flat_chunk();
        let carried = chunk
            .light()
            .cloned()
            .expect("the flat generator lights its chunks");
        assert_eq!(carried.sky[2], SectionLight::Full);
        let with_light = chunk_packet(ChunkPos::new(0, 0), &chunk).light;
        assert_eq!(with_light.sky_mask, [0b110]);
        assert_eq!(with_light.empty_sky_mask, [0b001]);
        assert_eq!(with_light.empty_block_mask, [0b111]);
        assert!(with_light.block_mask.is_empty() && with_light.block.is_empty());
        assert_eq!(with_light.sky[1], vec![0xff; LIGHT_ARRAY_LENGTH]);

        // A torch gives light, which the chunk's own light then no longer has.
        chunk.set(3, -60, 3, blocks::TORCH);
        assert!(chunk.light().is_none());
        let reckoned = chunk_packet(ChunkPos::new(0, 0), &chunk).light;
        assert_eq!(reckoned.sky_mask, [0b110]);
        assert_eq!(reckoned.empty_block_mask, [0b111]);

        // Block light that is carried is listed like sky light.
        let mut lit = flat_chunk();
        let mut light = carried;
        let mut levels = Box::new([0; LIGHT_ARRAY_LENGTH]);
        levels[100] = 0x0e;
        light.block[1] = SectionLight::Levels(levels.clone());
        lit.set_light(light);
        let packet = chunk_packet(ChunkPos::new(0, 0), &lit).light;
        assert_eq!(packet.block_mask, [0b010]);
        assert_eq!(packet.empty_block_mask, [0b101]);
        assert_eq!(packet.block, [levels.to_vec()]);
    }

    /// The chunk Clustine would send for a classic flat world is what the official
    /// server sends for one: the same blocks, biome, heightmaps and light.
    #[tokio::test]
    #[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
    async fn flat_chunk_matches_the_official_server() {
        let oracle = Oracle::start(false).await.unwrap();
        let mut bot = Bot::join(oracle.address(), "Surveyor").await.unwrap();
        bot.wait_for_chunks(9, Duration::from_secs(30))
            .await
            .unwrap();

        for (key, theirs) in &bot.chunks {
            let ours = chunk_packet(ChunkPos::new(key.0, key.1), &flat_chunk());

            let heights = |packet: &LevelChunkWithLight, kind| {
                let heightmap = packet.heightmaps.iter().find(|map| map.kind == kind)?;
                unpack_heightmap(&heightmap.data, 9)
            };
            for kind in [
                heightmap::WORLD_SURFACE,
                heightmap::MOTION_BLOCKING,
                heightmap::MOTION_BLOCKING_NO_LEAVES,
            ] {
                assert!(
                    heights(theirs, kind).is_some(),
                    "heightmap {kind} of {key:?}"
                );
                assert_eq!(heights(&ours, kind), heights(theirs, kind), "{key:?}");
            }

            // Biome ids depend on each server's registry order, so compare names.
            let their_biomes = bot.info.registry("minecraft:worldgen/biome").unwrap();
            let our_biomes = synced_registry("minecraft:worldgen/biome").unwrap().entries;
            let their_sections = bot.sections(*key).unwrap().unwrap();
            let our_sections =
                decode_sections(&ours.sections, 24, BLOCK_PALETTE, biome_palette()).unwrap();
            assert_eq!(our_sections.len(), their_sections.len());
            for (index, (our, their)) in our_sections.iter().zip(&their_sections).enumerate() {
                let context = format!("section {index} of {key:?}");
                assert_eq!(our.block_count, their.block_count, "{context}");
                assert_eq!(our.fluid_count, their.fluid_count, "{context}");
                for position in 0..4096 {
                    assert_eq!(
                        our.blocks.get(position),
                        their.blocks.get(position),
                        "{context}"
                    );
                }
                for cell in 0..64 {
                    assert_eq!(
                        our_biomes[our.biomes.get(cell) as usize],
                        their_biomes[their.biomes.get(cell) as usize],
                        "{context}"
                    );
                }
            }

            assert_eq!(ours.block_entities, theirs.block_entities, "{key:?}");
            assert_eq!(ours.light, theirs.light, "light of {key:?}");
        }
    }
}
