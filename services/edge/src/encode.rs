//! Turning world state into packets.

use clustine_data::{BLOCK_STATE_COUNT, BlockState, blocks, synced_registry};
use clustine_protocol::chunk::{
    PaletteKind, PalettedContainer, SectionData, bits_for, encode_sections, pack_heightmap,
};
use clustine_protocol::packets::play::{Heightmap, LevelChunkWithLight, LightData, heightmap};
use clustine_world::{Chunk, ChunkPos, Section, SectionLight, sky_light};

/// The palette layout for block states.
pub(crate) const BLOCK_PALETTE: PaletteKind = PaletteKind::blocks(BLOCK_STATE_COUNT as usize);

/// The palette layout for biomes, which depends on how many biomes the client was sent.
pub(crate) fn biome_palette() -> PaletteKind {
    let biomes = synced_registry("minecraft:worldgen/biome").expect("biomes are synchronised");
    PaletteKind::biomes(biomes.entries.len())
}

/// The packet that sends `chunk`, located at `position`, to a client.
pub(crate) fn chunk_packet(position: ChunkPos, chunk: &Chunk) -> LevelChunkWithLight {
    // All three heightmaps the client uses are approximated by the highest block that is
    // not air. They differ for fluids, leaves and blocks without collision.
    let heights = chunk.surface_heights();
    let data = pack_heightmap(&heights, bits_for(chunk.height() + 1));
    let heightmaps = [
        heightmap::MOTION_BLOCKING_NO_LEAVES,
        heightmap::MOTION_BLOCKING,
        heightmap::WORLD_SURFACE,
    ]
    .map(|kind| Heightmap {
        kind,
        data: data.clone(),
    });

    let sections: Vec<_> = chunk.sections().iter().map(section_data).collect();
    LevelChunkWithLight {
        chunk_x: position.x,
        chunk_z: position.z,
        heightmaps: heightmaps.into(),
        sections: encode_sections(&sections, BLOCK_PALETTE, biome_palette()),
        block_entities: Vec::new(),
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
    let fluid_count = match section.uniform_state() {
        Some(state) if !is_fluid(state) => 0,
        _ => section.states().filter(|state| is_fluid(*state)).count(),
    };
    SectionData {
        block_count: section.non_air_count() as i16,
        fluid_count: fluid_count as i16,
        blocks,
        biomes: PalettedContainer::Single(section.biome().0.into()),
    }
}

/// Whether `state` is water or lava. Waterlogged blocks are not recognised yet.
fn is_fluid(state: BlockState) -> bool {
    [blocks::WATER, blocks::LAVA].into_iter().any(|fluid| {
        let block = fluid.block().expect("water and lava are blocks");
        (block.first_state..=block.last_state).contains(&state)
    })
}

fn light_data(chunk: &Chunk) -> LightData {
    let mut light = LightData::default();
    let (mut sky_mask, mut empty_sky_mask, mut empty_block_mask) = (0u64, 0u64, 0u64);
    for (index, section) in sky_light(chunk).into_iter().enumerate() {
        let bit = 1 << index;
        match section {
            SectionLight::Dark => empty_sky_mask |= bit,
            SectionLight::Levels(levels) => {
                sky_mask |= bit;
                light.sky.push(levels.to_vec());
            }
            // Listed in neither mask: the client assumes full sky light.
            SectionLight::OpenSky => continue,
        }
        // There are no light sources yet, so block light is dark wherever light is stored.
        empty_block_mask |= bit;
    }
    light.sky_mask = vec![sky_mask];
    light.empty_sky_mask = vec![empty_sky_mask];
    light.empty_block_mask = vec![empty_block_mask];
    light
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use clustine_botswarm::{Bot, Oracle};
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
