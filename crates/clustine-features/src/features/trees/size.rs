// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/tree/mod.rs`
// (`tree_size_at_height`, `tree_min_clipped_height`).

//! A tree's `minimum_size` (`FeatureSize`): how wide the room it needs is at each
//! height, and how short it may be cut where there is less.

use crate::reader::{Read, Reader};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeatureSize {
    TwoLayers {
        limit: i32,
        lower_size: i32,
        upper_size: i32,
        min_clipped_height: Option<i32>,
    },
    ThreeLayers {
        limit: i32,
        upper_limit: i32,
        lower_size: i32,
        middle_size: i32,
        upper_size: i32,
        min_clipped_height: Option<i32>,
    },
}

fn int_or(reader: &Reader, key: &str, default: i32) -> Read<i32> {
    match reader.optional(key) {
        Some(value) => value.int(),
        None => Ok(default),
    }
}

impl FeatureSize {
    pub fn read(reader: &Reader) -> Read<Self> {
        let min_clipped_height = match reader.optional("min_clipped_height") {
            Some(value) => Some(value.int()?),
            None => None,
        };
        let size = match reader.kind()? {
            "minecraft:two_layers_feature_size" => Self::TwoLayers {
                limit: int_or(reader, "limit", 1)?,
                lower_size: int_or(reader, "lower_size", 0)?,
                upper_size: int_or(reader, "upper_size", 1)?,
                min_clipped_height,
            },
            "minecraft:three_layers_feature_size" => Self::ThreeLayers {
                limit: int_or(reader, "limit", 1)?,
                upper_limit: int_or(reader, "upper_limit", 1)?,
                lower_size: int_or(reader, "lower_size", 0)?,
                middle_size: int_or(reader, "middle_size", 1)?,
                upper_size: int_or(reader, "upper_size", 1)?,
                min_clipped_height,
            },
            other => return Err(reader.error(format!("{other} is not a feature size"))),
        };
        reader.finish()?;
        Ok(size)
    }

    pub fn min_clipped_height(&self) -> Option<i32> {
        match self {
            Self::TwoLayers {
                min_clipped_height, ..
            }
            | Self::ThreeLayers {
                min_clipped_height, ..
            } => *min_clipped_height,
        }
    }

    /// `getSizeAtHeight`: the radius of room needed at `y` above the trunk's foot of
    /// a tree `height` tall.
    pub fn size_at_height(&self, height: i32, y: i32) -> i32 {
        match *self {
            Self::TwoLayers {
                limit,
                lower_size,
                upper_size,
                ..
            } => {
                if y < limit {
                    lower_size
                } else {
                    upper_size
                }
            }
            Self::ThreeLayers {
                limit,
                upper_limit,
                lower_size,
                middle_size,
                upper_size,
                ..
            } => {
                if y < limit {
                    lower_size
                } else if y >= height - upper_limit {
                    upper_size
                } else {
                    middle_size
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use clustine_worldgen_data::feature::Data;

    use super::*;

    #[test]
    fn a_three_layer_size_is_narrow_at_the_foot_and_the_top_as_the_game_s_defaults_say() {
        static DATA: Data = Data::Object(&[
            ("type", Data::Text("minecraft:three_layers_feature_size")),
            ("upper_size", Data::Number("2")),
        ]);
        let size = FeatureSize::read(&Reader::new(&DATA, "test")).unwrap();
        assert_eq!(size.size_at_height(10, 0), 0);
        assert_eq!(size.size_at_height(10, 1), 1);
        assert_eq!(size.size_at_height(10, 8), 1);
        assert_eq!(size.size_at_height(10, 9), 2);
        assert_eq!(size.min_clipped_height(), None);
    }

    #[test]
    fn a_two_layer_size_reads_its_clipped_height() {
        static DATA: Data = Data::Object(&[
            ("type", Data::Text("minecraft:two_layers_feature_size")),
            ("limit", Data::Number("1")),
            ("lower_size", Data::Number("0")),
            ("upper_size", Data::Number("2")),
            ("min_clipped_height", Data::Number("4")),
        ]);
        let size = FeatureSize::read(&Reader::new(&DATA, "test")).unwrap();
        assert_eq!(size.size_at_height(7, 0), 0);
        assert_eq!(size.size_at_height(7, 1), 2);
        assert_eq!(size.min_clipped_height(), Some(4));
    }
}
