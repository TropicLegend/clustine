// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/fossil.rs`.

//! `fossil`: the bones of a spine or a skull buried 15 to 24 blocks under the lowest
//! ground above it, with an overlay of ore in their shape, unless too many of the box's
//! corners lie in air or fluid (`FossilFeature`). Both templates are placed with their
//! processors (the bones rot away in part; the overlay is mostly rotted away, and in
//! `fossil_diamonds` its coal becomes diamond ore).
//!
//! The blocks of the templates come from the operator's jar
//! (`FeatureSettings::templates`); without them a fossil places nothing.

use clustine_data::blocks;
use clustine_noise::Random;
use clustine_structures::geometry::{BoundingBox, IVec3, Rotation};
use clustine_structures::piece::StructureMirror;
use clustine_structures::place::{StructurePlaceSettings, place_in_world, template_bounding_box};
use clustine_structures::processors::{PlaceRandom, ProcessorList};
use clustine_world::BlockPos;

use super::templates::{FeatureShapes, read_processors, read_template_name};
use crate::blocks::same_block;
use crate::context::{Context, Heightmap, Level};
use crate::feature::Feature;
use crate::reader::{Read, Reader};

#[derive(Debug)]
pub struct Fossil {
    fossil_structures: Vec<&'static str>,
    overlay_structures: Vec<&'static str>,
    fossil_processors: ProcessorList,
    overlay_processors: ProcessorList,
    max_empty_corners_allowed: i32,
}

impl Fossil {
    pub fn read(reader: &Reader) -> Read<Self> {
        let names = |key: &str| -> Read<Vec<&'static str>> {
            reader
                .field(key)?
                .list()?
                .iter()
                .map(read_template_name)
                .collect()
        };
        let fossil = Self {
            fossil_structures: names("fossil_structures")?,
            overlay_structures: names("overlay_structures")?,
            fossil_processors: read_processors(&reader.field("fossil_processors")?)?,
            overlay_processors: read_processors(&reader.field("overlay_processors")?)?,
            max_empty_corners_allowed: reader.field("max_empty_corners_allowed")?.int()?,
        };
        reader.finish()?;
        // The game's codec asks for as many overlays as fossils, and at least one.
        if fossil.fossil_structures.is_empty()
            || fossil.fossil_structures.len() != fossil.overlay_structures.len()
        {
            return Err(reader.error("a fossil needs as many overlays as structures, and one"));
        }
        Ok(fossil)
    }
}

/// `StructureTemplate.getZeroPositionWithTransform` without a mirror: where the
/// corner of a template of `size` goes so that, turned, it covers the box whose
/// lowest corner is `pos`.
pub fn zero_position_with_transform(pos: BlockPos, size: [i32; 3], rotation: Rotation) -> BlockPos {
    let x = size[0] - 1;
    let z = size[2] - 1;
    match rotation {
        Rotation::None => pos,
        Rotation::Clockwise90 => pos.offset(z, 0, 0),
        Rotation::Clockwise180 => pos.offset(x, 0, z),
        Rotation::CounterClockwise90 => pos.offset(0, 0, x),
    }
}

/// `countEmptyCorners`: the box's eight corners that are air, water or lava.
fn count_empty_corners(level: &mut Level<'_>, bounds: BoundingBox) -> i32 {
    let mut count = 0;
    for x in [bounds.min_x(), bounds.max_x()] {
        for y in [bounds.min_y(), bounds.max_y()] {
            for z in [bounds.min_z(), bounds.max_z()] {
                let state = level.block(BlockPos::new(x, y, z));
                if state.props().is_air()
                    || same_block(state, blocks::LAVA)
                    || same_block(state, blocks::WATER)
                {
                    count += 1;
                }
            }
        }
    }
    count
}

impl Feature for Fossil {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let settings = context.settings;
        let Some(library) = settings.templates() else {
            return false;
        };
        let rotation = Rotation::get_random(&mut *context.random);
        let count = i32::try_from(self.fossil_structures.len()).expect("a fossil has few");
        let index = context.random.next_i32_bounded(count) as usize;
        // The names were checked against the jar's table when they were read.
        let (Some(fossil), Some(overlay)) = (
            library.get(self.fossil_structures[index]),
            library.get(self.overlay_structures[index]),
        ) else {
            return false;
        };

        let size = rotation.rotate_size(IVec3::from(fossil.size));
        let low = origin.offset(-size.x / 2, 0, -size.z / 2);
        let mut lowest = origin.y;
        for dx in 0..size.x {
            for dz in 0..size.z {
                let height =
                    context
                        .level
                        .height_at(Heightmap::OceanFloorWg, low.x + dx, low.z + dz);
                lowest = lowest.min(height);
            }
        }
        let y = (lowest - 15 - context.random.next_i32_bounded(10)).max(context.level.min_y() + 10);
        let target =
            zero_position_with_transform(BlockPos::new(low.x, y, low.z), fossil.size, rotation);
        let bounds = template_bounding_box(
            fossil.size,
            target,
            StructureMirror::None,
            rotation,
            BlockPos::new(0, 0, 0),
        );
        if count_empty_corners(context.level, bounds) > self.max_empty_corners_allowed {
            return false;
        }

        // The blocks are clipped to the three by three chunks about the origin's.
        let chunk = origin.chunk();
        let clip = BoundingBox::new(
            IVec3::new(chunk.x * 16 - 16, context.level.min_y(), chunk.z * 16 - 16),
            IVec3::new(
                chunk.x * 16 + 15 + 16,
                context.level.top_y() - 1,
                chunk.z * 16 + 15 + 16,
            ),
        );
        let seed = settings.seed();
        let shapes = FeatureShapes { settings };
        for (template, processors) in [
            (fossil, &self.fossil_processors),
            (overlay, &self.overlay_processors),
        ] {
            let mut placing = StructurePlaceSettings::new(seed);
            placing.rotation = rotation;
            placing.bounding_box = Some(clip);
            placing.processors = processors.processors.iter().collect();
            placing.random = PlaceRandom::Given;
            placing.shapes = &shapes;
            // No fossil holds an entity; one that did would be made as saved.
            if let Some(entities) = place_in_world(
                template,
                context.level.view(),
                target,
                target,
                &placing,
                &mut *context.random,
            ) {
                context.level.make_template_entities(settings, entities);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use clustine_data::BlockState;
    use clustine_structures::template_blocks::{TemplateBlock, TemplateBlocks, TemplateLibrary};
    use clustine_terrain::biome::Dimension;
    use clustine_worldgen_data::feature as data;

    use super::*;
    use crate::FeatureSettings;
    use crate::testing::World;

    /// A template of `size` filled with `state`.
    fn solid(size: [i32; 3], state: BlockState) -> TemplateBlocks {
        let mut blocks = Vec::new();
        for y in 0..size[1] {
            for x in 0..size[0] {
                for z in 0..size[2] {
                    blocks.push(TemplateBlock {
                        pos: BlockPos::new(x, y, z),
                        state,
                        nbt: None,
                    });
                }
            }
        }
        TemplateBlocks {
            size,
            data_version: clustine_data::DATA_VERSION,
            palettes: vec![blocks],
            entities: Vec::new(),
        }
    }

    /// The overworld's features with every fossil a block of bone and every overlay
    /// a block of coal ore.
    fn settings_with_fossils() -> FeatureSettings {
        let mut templates = Vec::new();
        for kind in ["spine", "skull"] {
            for number in 1..=4 {
                let name = format!("fossil/{kind}_{number}");
                templates.push((name.clone(), solid([3, 3, 3], blocks::BONE_BLOCK)));
                templates.push((format!("{name}_coal"), solid([3, 3, 3], blocks::COAL_ORE)));
            }
        }
        FeatureSettings::new(13579, Dimension::Overworld)
            .unwrap()
            .with_templates(Arc::new(TemplateLibrary::from_templates(templates)))
    }

    fn fossil(name: &str) -> Box<dyn Feature> {
        let (_, entry) = data::configured(name).unwrap();
        crate::feature::read_configured(&Reader::new(&entry.data, entry.name)).unwrap()
    }

    fn place(
        world: &mut World,
        settings: &FeatureSettings,
        seed: u64,
        feature: &dyn Feature,
    ) -> bool {
        let mut random = clustine_noise::WorldgenRandom::from_seed(seed);
        let mut level = Level::new(world, settings.zoom(), -64, 384);
        let mut context = Context {
            level: &mut level,
            random: &mut random,
            settings,
            top: None,
        };
        feature.place(&mut context, BlockPos::new(8, 100, 8))
    }

    #[test]
    fn the_zero_position_turns_with_the_template_as_the_games_does() {
        let zero = BlockPos::new(100, 64, 200);
        let size = [6, 10, 8];
        assert_eq!(
            zero_position_with_transform(zero, size, Rotation::None),
            zero
        );
        assert_eq!(
            zero_position_with_transform(zero, size, Rotation::Clockwise90),
            BlockPos::new(107, 64, 200)
        );
        assert_eq!(
            zero_position_with_transform(zero, size, Rotation::Clockwise180),
            BlockPos::new(105, 64, 207)
        );
        assert_eq!(
            zero_position_with_transform(zero, size, Rotation::CounterClockwise90),
            BlockPos::new(100, 64, 205)
        );
    }

    #[test]
    fn without_the_jars_templates_a_fossil_places_nothing() {
        let feature = fossil("minecraft:fossil_coal");
        let mut world = World::flat(blocks::STONE, 64);
        assert!(!world.with_context(1, |context| feature.place(context, BlockPos::new(8, 0, 8))));
        assert!(world.writes.is_empty());
    }

    #[test]
    fn a_fossil_is_buried_fifteen_to_twenty_four_blocks_under_the_ground() {
        let settings = settings_with_fossils();
        let feature = fossil("minecraft:fossil_coal");
        for seed in 0..6 {
            let mut world = World::flat(blocks::STONE, 64);
            assert!(place(&mut world, &settings, seed, feature.as_ref()));
            // The ground's surface is at 65; the template is three high.
            let ys: Vec<i32> = world.writes.iter().map(|(pos, _)| pos.y).collect();
            assert!(!ys.is_empty());
            let bottom = *ys.iter().min().unwrap();
            assert!((65 - 24..=65 - 15).contains(&bottom), "{bottom}");
            assert!(ys.iter().all(|y| (bottom..bottom + 3).contains(y)));
            // The bones rot by a tenth; the overlay keeps a tenth of its coal.
            let bones = world
                .writes
                .iter()
                .filter(|(_, state)| *state == blocks::BONE_BLOCK)
                .count();
            assert!(bones > 0);
            assert!(
                world.writes.iter().all(|(_, state)| {
                    *state == blocks::BONE_BLOCK || *state == blocks::COAL_ORE
                })
            );
        }
    }

    #[test]
    fn the_diamond_overlay_turns_its_coal_to_diamond_ore() {
        let settings = settings_with_fossils();
        let feature = fossil("minecraft:fossil_diamonds");
        let mut diamonds = 0;
        for seed in 0..6 {
            let mut world = World::flat(blocks::STONE, 64);
            place(&mut world, &settings, seed, feature.as_ref());
            assert!(
                world
                    .writes
                    .iter()
                    .all(|(_, state)| *state != blocks::COAL_ORE)
            );
            diamonds += world
                .writes
                .iter()
                .filter(|(_, state)| *state == blocks::DEEPSLATE_DIAMOND_ORE)
                .count();
        }
        assert!(diamonds > 0);
    }

    #[test]
    fn a_fossil_in_a_cave_is_not_placed() {
        let settings = settings_with_fossils();
        let feature = fossil("minecraft:fossil_coal");
        // Air everywhere under the surface: every corner is empty.
        let mut world = World::flat(blocks::AIR, 64);
        world.set(BlockPos::new(8, 64, 8), blocks::STONE);
        assert!(!place(&mut world, &settings, 3, feature.as_ref()));
        assert!(world.writes.is_empty());
    }
}
