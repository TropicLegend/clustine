// Adapted from SteelMC's `steel-core/src/worldgen/feature/configured.rs` (`place_template`).

//! `template`: one of weighted structure templates, turned by a rotation drawn from the
//! entry's, centred on the origin and placed with the feature's processors (26.x's
//! `TemplateFeature`, which places the desert well and its suspicious sand, and the
//! sulfur springs).
//!
//! The blocks of the templates come from the operator's jar
//! (`FeatureSettings::templates`); without them the feature places nothing.

use clustine_noise::Random;
use clustine_structures::geometry::{BoundingBox, Direction, IVec3, Rotation};
use clustine_structures::place::{StructurePlaceSettings, place_in_world};
use clustine_structures::processors::{PlaceRandom, ProcessorList};
use clustine_world::BlockPos;

use super::templates::{FeatureShapes, read_processors, read_template_name};
use crate::context::Context;
use crate::feature::Feature;
use crate::reader::{Read, Reader};
use crate::values::{read_weighted, weighted_index};

/// One template the feature may place, and the rotations it may be turned by.
#[derive(Debug)]
struct Entry {
    id: &'static str,
    rotations: Vec<Rotation>,
}

const ALL_ROTATIONS: [Rotation; 4] = [
    Rotation::None,
    Rotation::Clockwise90,
    Rotation::Clockwise180,
    Rotation::CounterClockwise90,
];

fn rotation_named(reader: &Reader) -> Read<Rotation> {
    Ok(match reader.text()? {
        "none" => Rotation::None,
        "clockwise_90" => Rotation::Clockwise90,
        "180" => Rotation::Clockwise180,
        "counterclockwise_90" => Rotation::CounterClockwise90,
        other => return Err(reader.error(format!("{other} is not a rotation"))),
    })
}

impl Entry {
    fn read(reader: &Reader) -> Read<Self> {
        let id = read_template_name(&reader.field("id")?)?;
        let rotations = match reader.optional("rotations") {
            Some(rotations) => rotations
                .list()?
                .iter()
                .map(rotation_named)
                .collect::<Read<Vec<_>>>()?,
            None => ALL_ROTATIONS.to_vec(),
        };
        if rotations.is_empty() {
            return Err(reader.error("a template with no rotation"));
        }
        reader.finish()?;
        Ok(Self { id, rotations })
    }
}

#[derive(Debug)]
pub struct Template {
    entries: Vec<Entry>,
    weights: Vec<i32>,
    processors: ProcessorList,
}

impl Template {
    pub fn read(reader: &Reader) -> Read<Self> {
        let (entries, weights) = read_weighted(&reader.field("templates")?, Entry::read)?;
        if weights.iter().sum::<i32>() <= 0 {
            return Err(reader.error("a template feature with no weight"));
        }
        let processors = match reader.optional("processors") {
            Some(processors) => read_processors(&processors)?,
            None => ProcessorList::default(),
        };
        reader.finish()?;
        Ok(Self {
            entries,
            weights,
            processors,
        })
    }
}

/// Where the template's corner goes so that, turned, its middle (to a block, rounding
/// towards its corner) lies at `origin`.
fn centred(origin: BlockPos, rotation: Rotation, size: [i32; 3]) -> BlockPos {
    let west = rotation.rotate(Direction::West).offset();
    let north = rotation.rotate(Direction::North).offset();
    let (half_x, half_z) = (size[0] / 2, size[2] / 2);
    origin.offset(
        west.0 * half_x + north.0 * half_z,
        0,
        west.2 * half_x + north.2 * half_z,
    )
}

impl Feature for Template {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let settings = context.settings;
        let Some(library) = settings.templates() else {
            return false;
        };
        let index = weighted_index(&self.weights, &mut *context.random)
            .expect("a template feature read from the data has weight");
        let entry = &self.entries[index];
        let count = i32::try_from(entry.rotations.len()).expect("at most four rotations");
        let rotation = entry.rotations[context.random.next_i32_bounded(count) as usize];
        // The name was checked against the jar's table when it was read.
        let Some(template) = library.get(entry.id) else {
            return false;
        };
        let position = centred(origin, rotation, template.size);

        let shapes = FeatureShapes { settings };
        let mut placing = StructurePlaceSettings::new(settings.seed());
        placing.rotation = rotation;
        // Clipped to the world's height only: the view refuses what lies beyond the
        // chunks the run may write.
        placing.bounding_box = Some(BoundingBox::new(
            IVec3::new(i32::MIN, context.level.min_y(), i32::MIN),
            IVec3::new(i32::MAX, context.level.top_y() - 1, i32::MAX),
        ));
        placing.processors = self.processors.processors.iter().collect();
        placing.random = PlaceRandom::Given;
        placing.shapes = &shapes;
        let Some(entities) = place_in_world(
            template,
            context.level.view(),
            position,
            position,
            &placing,
            &mut *context.random,
        ) else {
            return false;
        };
        // None of the game's templates of this feature holds an entity; one that did
        // would be made as saved, as the feature does not finalise.
        context.level.make_template_entities(settings, entities);
        true
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use clustine_data::{BlockState, blocks};
    use clustine_structures::template_blocks::{TemplateBlock, TemplateBlocks, TemplateLibrary};
    use clustine_terrain::biome::Dimension;
    use clustine_worldgen_data::feature::Data;

    use super::*;
    use crate::FeatureSettings;
    use crate::context::Level;
    use crate::testing::World;

    fn single(size: [i32; 3], at: [i32; 3], state: BlockState) -> TemplateBlocks {
        TemplateBlocks {
            size,
            data_version: clustine_data::DATA_VERSION,
            palettes: vec![vec![TemplateBlock {
                pos: BlockPos::new(at[0], at[1], at[2]),
                state,
                nbt: None,
            }]],
            entities: Vec::new(),
        }
    }

    fn settings_with(templates: Vec<(String, TemplateBlocks)>) -> FeatureSettings {
        FeatureSettings::new(13579, Dimension::Overworld)
            .unwrap()
            .with_templates(Arc::new(TemplateLibrary::from_templates(templates)))
    }

    fn place(world: &mut World, settings: &FeatureSettings, seed: u64, feature: &Template) -> bool {
        let mut random = clustine_noise::WorldgenRandom::from_seed(seed);
        let mut level = Level::new(world, settings.zoom(), -64, 384);
        let mut context = Context {
            level: &mut level,
            random: &mut random,
            settings,
            top: None,
        };
        feature.place(&mut context, BlockPos::new(8, 70, 8))
    }

    static WELL: Data = Data::Object(&[(
        "templates",
        Data::List(&[Data::Object(&[
            (
                "data",
                Data::Object(&[
                    ("id", Data::Text("minecraft:desert_well/well")),
                    ("rotations", Data::List(&[Data::Text("clockwise_90")])),
                ]),
            ),
            ("weight", Data::Number("1")),
        ])]),
    )]);

    #[test]
    fn the_template_is_centred_on_the_origin_whichever_way_it_turns() {
        let origin = BlockPos::new(0, 0, 0);
        // A template five by seven: its middle block, turned, lands on the origin.
        for rotation in ALL_ROTATIONS {
            let corner = centred(origin, rotation, [5, 1, 7]);
            let middle = clustine_structures::place::transform_pos(
                BlockPos::new(2, 0, 3),
                clustine_structures::piece::StructureMirror::None,
                rotation,
                BlockPos::new(0, 0, 0),
            );
            assert_eq!(
                corner.offset(middle.x, middle.y, middle.z),
                origin,
                "{rotation:?}"
            );
        }
    }

    #[test]
    fn an_entry_turns_only_by_its_rotations_and_places_its_template() {
        let reader = Reader::new(&WELL, "test");
        let feature = Template::read(&reader).unwrap();
        assert_eq!(feature.entries[0].rotations, [Rotation::Clockwise90]);
        // The well, here one block of sandstone at the template's corner.
        let settings = settings_with(vec![(
            "desert_well/well".to_string(),
            single([5, 1, 7], [0, 0, 0], blocks::SANDSTONE),
        )]);
        let mut world = World::flat(blocks::SAND, 64);
        assert!(place(&mut world, &settings, 1, &feature));
        // Turned a quarter clockwise, the corner lies at x + 3, z - 2 from the origin.
        assert_eq!(
            world.writes,
            [(BlockPos::new(11, 70, 6), blocks::SANDSTONE)]
        );
    }

    #[test]
    fn without_the_jars_templates_the_feature_places_nothing() {
        let feature = Template::read(&Reader::new(&WELL, "test")).unwrap();
        let mut world = World::flat(blocks::SAND, 64);
        assert!(!world.with_context(1, |context| feature.place(context, BlockPos::new(8, 70, 8))));
        assert!(world.writes.is_empty());
    }

    #[test]
    fn the_desert_wells_suspicious_sand_is_read_with_its_processors() {
        let (_, entry) =
            clustine_worldgen_data::feature::configured("minecraft:desert_well").unwrap();
        let Some(Data::List(features)) = entry.data.get("features") else {
            panic!("a list");
        };
        let sand = features[1].get("feature").unwrap();
        let reader = Reader::new(sand, "test");
        reader.kind().unwrap();
        let feature = Template::read(&reader).unwrap();
        assert_eq!(feature.processors.processors.len(), 1);
        assert_eq!(feature.entries[0].rotations.len(), 4);
    }

    #[test]
    fn a_rotation_the_game_does_not_name_is_refused() {
        static BAD: Data = Data::Object(&[(
            "templates",
            Data::List(&[Data::Object(&[
                (
                    "data",
                    Data::Object(&[
                        ("id", Data::Text("minecraft:desert_well/well")),
                        ("rotations", Data::List(&[Data::Text("clockwise_180")])),
                    ]),
                ),
                ("weight", Data::Number("1")),
            ])]),
        )]);
        assert!(Template::read(&Reader::new(&BAD, "test")).is_err());
    }
}
