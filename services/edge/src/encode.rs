//! Turning world state into packets.

use clustine_data::{BLOCK_ENTITY_TYPES, BLOCK_STATE_COUNT, synced_registry};
use clustine_protocol::chunk::{
    PaletteKind, PalettedContainer, SectionData, bits_for, encode_sections, pack_heightmap,
};
use clustine_protocol::codec::Reader;
use clustine_protocol::nbt::Nbt;
use clustine_protocol::packets::play::{
    BlockEntity, Heightmap, LevelChunkWithLight, LightData, heightmap,
};
use clustine_world::{
    Chunk, ChunkPos, LIGHT_ARRAY_LENGTH, Light, Section, SectionLight, sky_light,
};

/// The block entity types a client is sent data of with a chunk, and the keys of the
/// data it is sent (ADR-0022, section 5). A spawner shows the mob that turns in its
/// cage: the game sends what it saves of one but the list of what it may spawn
/// (`SpawnerBlockEntity.getUpdateTag`: `saveCustomOnly` without `SpawnPotentials`, and
/// so without `components`), read from the pinned jar's code while W4's comparison of
/// packets is not built; what the store keeps of a spawner is the game's saved form,
/// which the judge of features compares.
const SENT: [(&str, &[&str]); 1] = [(
    "minecraft:mob_spawner",
    &[
        "Delay",
        "MinSpawnDelay",
        "MaxSpawnDelay",
        "SpawnCount",
        "MaxNearbyEntities",
        "RequiredPlayerRange",
        "SpawnRange",
        "SpawnData",
    ],
)];

/// What a client is sent of a block entity's data: the keys `SENT` names for its
/// type, or nothing.
fn sent(entity: &clustine_world::BlockEntity) -> Option<Nbt> {
    let name = BLOCK_ENTITY_TYPES.get(usize::from(entity.kind))?;
    let (_, keys) = SENT.iter().find(|(kind, _)| kind == name)?;
    let Ok(Some(Nbt::Compound(entries))) = Reader::new(&entity.data).nbt() else {
        return None;
    };
    Some(Nbt::Compound(
        entries
            .into_iter()
            .filter(|(key, _)| keys.contains(&key.as_str()))
            .collect(),
    ))
}

/// The palette layout for block states.
pub(crate) const BLOCK_PALETTE: PaletteKind = PaletteKind::blocks(BLOCK_STATE_COUNT as usize);

/// The palette layout for biomes, which depends on how many biomes the client was sent.
pub(crate) fn biome_palette() -> PaletteKind {
    let biomes = synced_registry("minecraft:worldgen/biome").expect("biomes are synchronised");
    PaletteKind::biomes(biomes.entries.len())
}

/// The packet that sends `chunk`, located at `position`, to a client. It is made from
/// what the chunk carries (`docs/adr/0022-what-a-chunk-carries.md`, section 5): its
/// heightmaps, its sections' counts, blocks and biomes and its block entities; and
/// from `light`, which an edge keeps beside the chunk
/// (`docs/adr/0023-light-at-the-edge.md`, section 1). The light the chunk itself
/// carries is not looked at.
pub(crate) fn chunk_packet(
    position: ChunkPos,
    chunk: &Chunk,
    light: Option<&Light>,
) -> LevelChunkWithLight {
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
    // something (`SENT`): a chest shows as a chest without, and what a generated chest
    // carries (the seed of what is in it) is nobody's to be told.
    let block_entities = chunk
        .block_entities()
        .iter()
        .map(|(at, entity)| BlockEntity {
            packed_xz: at.x << 4 | at.z,
            y: at.y,
            kind: entity.kind.into(),
            data: sent(entity),
        });

    let sections: Vec<_> = chunk.sections().iter().map(section_data).collect();
    LevelChunkWithLight {
        chunk_x: position.x,
        chunk_z: position.z,
        heightmaps: heightmaps.into(),
        sections: encode_sections(&sections, BLOCK_PALETTE, biome_palette()),
        block_entities: block_entities.collect(),
        light: chunk_light(chunk, light),
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

/// Which light sections a packet is to name: one bit a light section for the sky and
/// one for blocks, the lowest bit for the section below the chunk, as
/// [`clustine_world::Light`] counts them. A chunk's packet names them all; a light
/// update names those its sender chooses
/// (`docs/adr/0023-light-at-the-edge.md`, section 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Named {
    pub sky: u64,
    pub block: u64,
}

impl Named {
    /// Every section a list has.
    pub(crate) const ALL: Named = Named {
        sky: u64::MAX,
        block: u64::MAX,
    };
}

/// The light of `chunk` for its packet: `light`, where there is any, and otherwise the
/// sky light reckoned by columns, with block light dark wherever sky light is said.
fn chunk_light(chunk: &Chunk, light: Option<&Light>) -> LightData {
    match light {
        Some(light) => light_data(&light.sky, &light.block, Named::ALL),
        None => {
            let sky = sky_light(chunk);
            // No light sources are reckoned: dark wherever sky light is said.
            let block: Vec<_> = sky
                .iter()
                .map(|section| match section {
                    SectionLight::Absent => SectionLight::Absent,
                    _ => SectionLight::Dark,
                })
                .collect();
            light_data(&sky, &block, Named::ALL)
        }
    }
}

/// The light of a packet, a chunk's or a light update's: of the sections of `sky` and
/// of `block` that `named` names, each in its mask and, where it has levels, with its
/// array, lowest first. A section that is absent is in neither mask, named or not:
/// there is nothing to say it with.
pub(crate) fn light_data(sky: &[SectionLight], block: &[SectionLight], named: Named) -> LightData {
    let mut light = LightData::default();
    let (mut sky_mask, mut empty_sky_mask) = (0u64, 0u64);
    let (mut block_mask, mut empty_block_mask) = (0u64, 0u64);
    for (sections, named, mask, empty, arrays) in [
        (
            sky,
            named.sky,
            &mut sky_mask,
            &mut empty_sky_mask,
            &mut light.sky,
        ),
        (
            block,
            named.block,
            &mut block_mask,
            &mut empty_block_mask,
            &mut light.block,
        ),
    ] {
        for (index, section) in sections.iter().enumerate() {
            let bit = 1 << index;
            if named & bit != 0 {
                list(section, bit, mask, empty, arrays);
            }
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
        let packet = chunk_packet(ChunkPos::new(-3, 8), &chunk, chunk.light());
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
        let packet = chunk_packet(ChunkPos::new(1, 2), &flat_chunk(), flat_chunk().light());
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
        let packet = chunk_packet(ChunkPos::new(0, 0), &chunk, chunk.light());
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

    /// A spawner goes out with what the game sends of one: what it saves, without
    /// the list of what it may spawn and without its components.
    #[test]
    fn a_spawner_is_sent_its_mob_and_settings_and_nothing_else() {
        use clustine_protocol::codec::Writer;

        let mut chunk = flat_chunk();
        let spawner = clustine_data::BlockState::parse("minecraft:spawner").unwrap();
        chunk.set(2, -59, 3, spawner);
        let at = clustine_world::InChunk::new(2, -59, 3);
        let saved = Nbt::Compound(vec![
            ("Delay".into(), Nbt::Short(20)),
            ("SpawnPotentials".into(), Nbt::List(Vec::new())),
            (
                "SpawnData".into(),
                Nbt::Compound(vec![(
                    "entity".into(),
                    Nbt::Compound(vec![("id".into(), Nbt::String("minecraft:zombie".into()))]),
                )]),
            ),
            ("components".into(), Nbt::Compound(Vec::new())),
            ("SpawnRange".into(), Nbt::Short(4)),
        ]);
        let mut writer = Writer::new();
        writer.put_nbt(&saved);
        chunk
            .set_block_entity_data(at, writer.into_bytes())
            .unwrap();
        let packet = chunk_packet(ChunkPos::new(0, 0), &chunk, chunk.light());
        let Nbt::Compound(sent) = packet.block_entities[0].data.clone().unwrap() else {
            panic!("a spawner is sent a compound");
        };
        let keys: Vec<&str> = sent.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(keys, ["Delay", "SpawnData", "SpawnRange"]);
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
        let packet = chunk_packet(ChunkPos::new(0, 0), &chunk, chunk.light());
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
        let packet = chunk_packet(ChunkPos::new(0, 0), &chunk, chunk.light());
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
        let with_light = chunk_packet(ChunkPos::new(0, 0), &chunk, chunk.light()).light;
        assert_eq!(with_light.sky_mask, [0b110]);
        assert_eq!(with_light.empty_sky_mask, [0b001]);
        assert_eq!(with_light.empty_block_mask, [0b111]);
        assert!(with_light.block_mask.is_empty() && with_light.block.is_empty());
        assert_eq!(with_light.sky[1], vec![0xff; LIGHT_ARRAY_LENGTH]);

        // A torch gives light, which the chunk's own light then no longer has.
        chunk.set(3, -60, 3, blocks::TORCH);
        assert!(chunk.light().is_none());
        let reckoned = chunk_packet(ChunkPos::new(0, 0), &chunk, chunk.light()).light;
        assert_eq!(reckoned.sky_mask, [0b110]);
        assert_eq!(reckoned.empty_block_mask, [0b111]);

        // Block light that is carried is listed like sky light.
        let mut lit = flat_chunk();
        let mut light = carried;
        let mut levels = Box::new([0; LIGHT_ARRAY_LENGTH]);
        levels[100] = 0x0e;
        light.block[1] = SectionLight::Levels(levels.clone());
        lit.set_light(light);
        let packet = chunk_packet(ChunkPos::new(0, 0), &lit, lit.light()).light;
        assert_eq!(packet.block_mask, [0b010]);
        assert_eq!(packet.empty_block_mask, [0b101]);
        assert_eq!(packet.block, [levels.to_vec()]);
    }

    /// What `light_data` was before it became a function of the two lists and of the
    /// sections to name, kept word for word, so that the tests below can hold a
    /// chunk's packet to the bytes it had.
    fn light_data_as_before(chunk: &Chunk) -> LightData {
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

    /// A section of levels that is neither dark nor full, told apart by `seed`.
    fn levels(seed: u8) -> SectionLight {
        let mut levels = Box::new([seed; LIGHT_ARRAY_LENGTH]);
        levels[7] = 0x5a;
        SectionLight::Levels(levels)
    }

    /// Chunks whose light is of every kind a packet can be made from: carried and
    /// not, with and without block light, with every case of a section, and with
    /// sections listed for one of the two lights and not for the other.
    fn chunks_of_every_light() -> Vec<(&'static str, Chunk)> {
        let flat = flat_chunk();
        let carried = flat.light().cloned().unwrap();

        // A change light sees takes the light: the packet is lit by columns.
        let mut torch = flat_chunk();
        torch.set(3, -60, 3, blocks::TORCH);
        let mut tower = flat_chunk();
        for y in -60..40 {
            tower.set(8, y, 8, blocks::STONE);
        }
        tower.set(0, 200, 15, blocks::STONE);
        assert!(torch.light().is_none() && tower.light().is_none());

        let mut block_light = flat_chunk();
        let mut light = carried.clone();
        light.block[1] = levels(0x0e);
        block_light.set_light(light);

        // The rules' own light for a chunk with a torch and a tower in it, which has
        // levels of both lights.
        let mut ruled = tower.clone();
        ruled.set(3, -60, 3, blocks::TORCH);
        let light = clustine_light::light(&clustine_light::Around {
            centre: &ruled,
            neighbours: [Some(&flat); 8],
            sky: true,
        });
        let has_levels = |sections: &[SectionLight]| {
            sections
                .iter()
                .any(|section| matches!(section, SectionLight::Levels(_)))
        };
        assert!(has_levels(&light.sky) && has_levels(&light.block));
        ruled.set_light(light);

        // Every case in every place, and the two lists listing different sections.
        let mut mixed = flat_chunk();
        let mut light = carried.clone();
        let cases = [
            SectionLight::Absent,
            SectionLight::Dark,
            SectionLight::Full,
            levels(0x31),
        ];
        for index in 0..light.sky.len() {
            light.sky[index] = cases[index % 4].clone();
            light.block[index] = cases[(index / 4 + index) % 4].clone();
        }
        mixed.set_light(light);

        // No sky at all, and no light at all.
        let mut skyless = flat_chunk();
        let mut light = carried.clone();
        light.sky.fill(SectionLight::Absent);
        light.block[2] = levels(0x77);
        skyless.set_light(light);
        let mut unlit = flat_chunk();
        let mut light = carried;
        light.sky.fill(SectionLight::Absent);
        light.block.fill(SectionLight::Absent);
        unlit.set_light(light);

        vec![
            ("the flat chunk", flat),
            ("a chunk a torch took the light of", torch),
            ("a chunk a tower took the light of", tower),
            ("a chunk with carried block light", block_light),
            ("a chunk with the rules' light", ruled),
            ("a chunk with every case", mixed),
            ("a chunk without sky light", skyless),
            ("a chunk with no light listed", unlit),
        ]
    }

    /// The two lists a chunk's packet is made from: the chunk's own light, or the
    /// reckoning by columns with block light dark wherever that says sky light.
    fn lists(chunk: &Chunk) -> (Vec<SectionLight>, Vec<SectionLight>) {
        match chunk.light() {
            Some(light) => (light.sky.clone(), light.block.clone()),
            None => {
                let sky = sky_light(chunk);
                let block = sky
                    .iter()
                    .map(|section| match section {
                        SectionLight::Absent => SectionLight::Absent,
                        _ => SectionLight::Dark,
                    })
                    .collect();
                (sky, block)
            }
        }
    }

    /// `chunk_packet` gives the bytes it gave before `light_data` took the lists and
    /// the sections to name: its light is the old function's, field for field, and the
    /// packet encodes to the same bytes as one with the old function's light.
    #[test]
    fn a_chunks_packet_has_the_light_and_the_bytes_it_had() {
        for (what, chunk) in chunks_of_every_light() {
            let position = ChunkPos::new(-7, 12);
            let packet = chunk_packet(position, &chunk, chunk.light());
            let before = light_data_as_before(&chunk);
            assert_eq!(packet.light, before, "{what}");
            let as_before = LevelChunkWithLight {
                light: before,
                ..chunk_packet(position, &chunk, chunk.light())
            };
            assert_eq!(
                packets::encode(&packet),
                packets::encode(&as_before),
                "{what}"
            );
        }
    }

    /// With every section named, `light_data` over a chunk's two lists is the light
    /// its packet had: for the flat chunk, for one with block light, and for one
    /// without carried light.
    #[test]
    fn light_data_with_every_section_named_is_what_a_chunks_packet_gave() {
        for (what, chunk) in chunks_of_every_light() {
            let before = light_data_as_before(&chunk);
            let (sky, block) = lists(&chunk);
            assert_eq!(light_data(&sky, &block, Named::ALL), before, "{what}");
            // Naming exactly the sections the lists have is naming all of them.
            let all = Named {
                sky: (1 << sky.len()) - 1,
                block: (1 << block.len()) - 1,
            };
            assert_eq!(light_data(&sky, &block, all), before, "{what}");
        }
    }

    /// With some sections named, the packet's light has those and no others: each in
    /// the mask of its case, the arrays in the order of their sections, sky and block
    /// apart. An absent section that is named is in neither mask.
    #[test]
    fn light_data_names_the_sections_it_is_told_to_and_no_others() {
        let sky = vec![
            SectionLight::Dark,
            levels(1),
            SectionLight::Full,
            levels(3),
            SectionLight::Absent,
            SectionLight::Dark,
            levels(6),
        ];
        let block = vec![
            levels(10),
            SectionLight::Dark,
            SectionLight::Dark,
            SectionLight::Absent,
            levels(14),
            SectionLight::Full,
            SectionLight::Dark,
        ];
        let array = |section: &SectionLight| match section {
            SectionLight::Levels(levels) => levels.to_vec(),
            SectionLight::Full => vec![0xff; LIGHT_ARRAY_LENGTH],
            other => panic!("{other:?} has no array"),
        };

        let named = Named {
            sky: 0b111_1100,
            block: 0b011_1011,
        };
        let some = light_data(&sky, &block, named);
        assert_eq!(some.sky_mask, [0b100_1100]);
        assert_eq!(some.empty_sky_mask, [0b010_0000]);
        assert_eq!(some.sky, [array(&sky[2]), array(&sky[3]), array(&sky[6])]);
        assert_eq!(some.block_mask, [0b011_0001]);
        assert_eq!(some.empty_block_mask, [0b000_0010]);
        assert_eq!(
            some.block,
            [array(&block[0]), array(&block[4]), array(&block[5])]
        );

        // One light alone: the other has no bit and no array.
        let sky_alone = light_data(
            &sky,
            &block,
            Named {
                sky: 0b10,
                block: 0,
            },
        );
        assert_eq!(sky_alone.sky_mask, [0b10]);
        assert_eq!(sky_alone.sky, [array(&sky[1])]);
        assert_eq!(sky_alone.empty_sky_mask, [0]);
        assert!(sky_alone.block_mask.is_empty() && sky_alone.block.is_empty());
        assert_eq!(sky_alone.empty_block_mask, [0]);
        let block_alone = light_data(
            &sky,
            &block,
            Named {
                sky: 0,
                block: 0b100,
            },
        );
        assert_eq!(block_alone.sky_mask, [0]);
        assert_eq!(block_alone.empty_sky_mask, [0]);
        assert!(block_alone.sky.is_empty() && block_alone.block.is_empty());
        assert!(block_alone.block_mask.is_empty());
        assert_eq!(block_alone.empty_block_mask, [0b100]);

        // Nothing named, and only what is absent named: nothing said.
        let absent_alone = Named {
            sky: 0b1_0000,
            block: 0b1000,
        };
        for named in [Named { sky: 0, block: 0 }, absent_alone] {
            let none = light_data(&sky, &block, named);
            assert_eq!(none.sky_mask, [0]);
            assert_eq!(none.empty_sky_mask, [0]);
            assert!(none.block_mask.is_empty());
            assert_eq!(none.empty_block_mask, [0]);
            assert!(none.sky.is_empty() && none.block.is_empty());
        }

        // What is named survives the codec as a light update.
        let update = clustine_protocol::packets::play::LightUpdate {
            chunk_x: 4,
            chunk_z: -9,
            light: some,
        };
        assert_eq!(
            ClientboundPlay::decode(&packets::encode(&update)),
            Ok(ClientboundPlay::LightUpdate(update))
        );
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
            let ours = chunk_packet(
                ChunkPos::new(key.0, key.1),
                &flat_chunk(),
                flat_chunk().light(),
            );

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
