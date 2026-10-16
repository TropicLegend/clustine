//! What the two features built of structure templates share (`fossil` and
//! `template`): their processor lists read through `clustine-structures`, the lists
//! the data names, and the name of a template checked against the table of templates.
//!
//! The data names processor lists of the registry `worldgen/processor_list`, which
//! `cargo datagen` emits with the features.

use clustine_data::{BlockState, Face};
use clustine_structures::geometry::Direction;
use clustine_structures::place::ShapeUpdates;
use clustine_structures::processors::ProcessorList;
use clustine_world::BlockPos;
use clustine_world::generation::Neighbourhood;
use clustine_worldgen_data::feature::{self as data, Data};

use crate::context::Level;
use crate::reader::{DataError, Read, Reader};
use crate::settings::FeatureSettings;
use clustine_blocks::update_shape;

/// The shapes a template's blocks take beside their neighbours where its shape is not
/// known (`updateShapeAtEdge`), by `clustine-blocks`' `updateShape`: the one the
/// trees' edges use, so that gravel beside a fossil or a path under a tree's edge
/// schedule their ticks as the game's do.
pub struct FeatureShapes<'s> {
    pub settings: &'s FeatureSettings,
}

impl ShapeUpdates for FeatureShapes<'_> {
    fn update_shape(
        &self,
        world: &mut dyn Neighbourhood,
        at: BlockPos,
        state: BlockState,
        direction: Direction,
        _: BlockPos,
        neighbour: BlockState,
    ) -> BlockState {
        let face = match direction {
            Direction::Down => Face::Down,
            Direction::Up => Face::Up,
            Direction::North => Face::North,
            Direction::South => Face::South,
            Direction::West => Face::West,
            Direction::East => Face::East,
        };
        let settings = self.settings;
        let mut level = Level::new(world, settings.zoom(), settings.min_y(), settings.height());
        update_shape(&mut level, state, at, face, neighbour)
    }
}

fn named_list(name: &str) -> Option<&'static Data> {
    data::processor_list(name).map(|(_, entry)| &entry.data)
}

/// Reads a processor list where the game's codec takes a `Holder` of one: the name of
/// a list, or a list in place. The list's own members are checked by
/// `clustine-structures`' reader, as this crate's checks a feature's.
pub fn read_processors(reader: &Reader) -> Read<ProcessorList> {
    let inner = clustine_structures::reader::Reader::new(reader.data(), reader.at());
    ProcessorList::read(&inner, &named_list).map_err(|error| DataError {
        at: error.at,
        what: error.what,
    })
}

/// Reads a template's name and checks that the jar has it, by the table of templates
/// `cargo datagen` made: a name the table does not know is a data error, not a
/// template missing at a feature's first placing.
pub fn read_template_name(reader: &Reader) -> Read<&'static str> {
    let name = reader.text()?;
    let plain = name.strip_prefix("minecraft:").unwrap_or(name);
    let table = clustine_worldgen_data::templates();
    match table.find(plain) {
        Some(index) if table.get(index).exists => Ok(name),
        _ => Err(reader.error(format!("{name} is not a template of the jar"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every processor list of the game is read whole by `clustine-structures`'
    /// reader, as a structure's pool or a feature names it: every processor type and
    /// rule the data uses is built.
    #[test]
    fn every_processor_list_of_the_game_reads_whole() {
        for entry in data::processor_lists() {
            // A reader holds its data for as long as the data lives, which here is the
            // whole test.
            let named: &'static Data = Box::leak(Box::new(Data::Text(entry.name)));
            let list = read_processors(&Reader::new(named, entry.name))
                .unwrap_or_else(|error| panic!("{}: {error:?}", entry.name));
            assert_eq!(
                list,
                read_processors(&Reader::new(&entry.data, entry.name)).unwrap(),
                "{}: by name and in place",
                entry.name
            );
        }
        static UNKNOWN: Data = Data::Text("minecraft:no_such_list");
        assert!(read_processors(&Reader::new(&UNKNOWN, "test")).is_err());
    }

    #[test]
    fn a_template_the_jar_does_not_have_is_refused() {
        static WELL: Data = Data::Text("minecraft:desert_well/well");
        static NONE: Data = Data::Text("minecraft:desert_well/no_such_well");
        assert_eq!(
            read_template_name(&Reader::new(&WELL, "test")),
            Ok("minecraft:desert_well/well")
        );
        assert!(read_template_name(&Reader::new(&NONE, "test")).is_err());
    }
}
