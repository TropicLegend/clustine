//! The three dimensions of a world.

use clustine_data::{DIMENSION_TYPES, DimensionType};
use serde::{Deserialize, Serialize};

/// One of the three levels of a world. A region holds chunks of one of them, and the
/// store and the edge, which see all three, key chunks by it.
///
/// The variants are numbered as a generated entity's key numbers them
/// ([`crate::generation::dimension_number`]): the overworld 0, the Nether 1, the End 2.
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default, Serialize, Deserialize,
)]
pub enum Dimension {
    #[default]
    Overworld,
    Nether,
    End,
}

impl Dimension {
    pub const ALL: [Dimension; 3] = [Dimension::Overworld, Dimension::Nether, Dimension::End];

    /// The name of the dimension, and of its type, in the game's registries.
    pub const fn type_name(self) -> &'static str {
        match self {
            Dimension::Overworld => "minecraft:overworld",
            Dimension::Nether => "minecraft:the_nether",
            Dimension::End => "minecraft:the_end",
        }
    }

    /// The dimension called `name` in the game's registries.
    pub fn named(name: &str) -> Option<Dimension> {
        Dimension::ALL
            .into_iter()
            .find(|dimension| dimension.type_name() == name)
    }

    /// The dimension's number: the overworld 0, the Nether 1, the End 2.
    pub const fn number(self) -> u8 {
        self as u8
    }

    /// The dimension numbered `number`.
    pub fn numbered(number: u8) -> Option<Dimension> {
        Dimension::ALL.get(usize::from(number)).copied()
    }

    /// The dimension's type: its height, its light and the rules of the game that differ
    /// between dimensions, as the game's data gives them.
    pub fn rules(self) -> &'static DimensionType {
        DIMENSION_TYPES
            .iter()
            .find(|kind| kind.name == self.type_name())
            .expect("the three dimension types are in the game's registry")
    }

    /// The section coordinate of the lowest section of a chunk, and how many sections a
    /// chunk has.
    pub fn sections(self) -> (i32, usize) {
        let kind = self.rules();
        (kind.min_y >> 4, (kind.height / 16) as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::dimension_number;

    #[test]
    fn a_dimension_is_numbered_as_a_generated_key_numbers_it() {
        for dimension in Dimension::ALL {
            assert_eq!(dimension.number(), dimension_number(dimension.type_name()));
            assert_eq!(Dimension::numbered(dimension.number()), Some(dimension));
            assert_eq!(Dimension::named(dimension.type_name()), Some(dimension));
        }
        assert_eq!(Dimension::numbered(3), None);
        assert_eq!(Dimension::named("minecraft:overworld_caves"), None);
    }

    #[test]
    fn each_dimension_has_the_rules_of_its_own_type() {
        assert_eq!(Dimension::Overworld.sections(), (-4, 24));
        assert_eq!(Dimension::Nether.sections(), (0, 16));
        assert_eq!(Dimension::End.sections(), (0, 16));
        assert!(Dimension::Nether.rules().fast_lava);
        assert!(!Dimension::Nether.rules().has_skylight);
        assert!(Dimension::End.rules().has_ender_dragon_fight);
    }

    #[test]
    fn a_dimension_is_one_byte_when_serialised() {
        for dimension in Dimension::ALL {
            let bytes = postcard::to_allocvec(&dimension).unwrap();
            assert_eq!(bytes, [dimension.number()]);
            assert_eq!(
                postcard::from_bytes::<Dimension>(&bytes).unwrap(),
                dimension
            );
        }
    }
}
