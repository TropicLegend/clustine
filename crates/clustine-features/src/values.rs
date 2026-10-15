// Adapted from SteelMC's `steel-utils/src/value_providers/{int,float,height,vertical_anchor}.rs`.

//! The game's value providers (`net.minecraft.util.valueproviders` and
//! `net.minecraft.world.level.levelgen.heightproviders`): how a feature or a modifier
//! draws a number, a float or a height, and the vertical anchors heights are given by.
//!
//! Each `sample` draws exactly the random numbers the game's class draws, in its order:
//! a feature's random source is shared by everything after it.

use clustine_noise::Random;

use crate::reader::{Read, Reader};

/// The heights a height is resolved against (the game's `WorldGenerationContext`):
/// the lowest block the generator makes, how many blocks high it makes them and the
/// sea level. In the Nether and the End the generator makes 128 blocks of the 256
/// the dimension has, so a height "below the top" counts from 127 there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Generation {
    pub min_y: i32,
    pub height: i32,
    pub sea_level: i32,
}

/// `Mth.nextInt`: a number from `min` to `max`, both included, drawing nothing when
/// the range holds one number or none.
pub fn next_int_between<R: Random + ?Sized>(random: &mut R, min: i32, max: i32) -> i32 {
    if min >= max {
        min
    } else {
        random.next_i32_bounded(max - min + 1) + min
    }
}

/// `Mth.randomBetweenInclusive`: unlike [`next_int_between`], it always draws.
pub fn random_between_inclusive<R: Random + ?Sized>(random: &mut R, min: i32, max: i32) -> i32 {
    random.next_i32_bounded(max - min + 1) + min
}

/// The index a weighted list picks (`WeightedList.getRandom`): one draw below the
/// total weight, which falls in an entry's share. `None` for a list of no weight,
/// which draws nothing.
pub fn weighted_index<R: Random + ?Sized>(weights: &[i32], random: &mut R) -> Option<usize> {
    let total: i32 = weights.iter().sum();
    if total <= 0 {
        return None;
    }
    let mut selection = random.next_i32_bounded(total);
    for (index, weight) in weights.iter().enumerate() {
        if selection < *weight {
            return Some(index);
        }
        selection -= weight;
    }
    None
}

/// Reads the entries of a weighted list (`distribution`, `entries`, `features`):
/// `{"data": ..., "weight": ...}` each, the data read by `read`.
pub fn read_weighted<T>(
    list: &Reader,
    read: impl Fn(&Reader) -> Read<T>,
) -> Read<(Vec<T>, Vec<i32>)> {
    let mut values = Vec::new();
    let mut weights = Vec::new();
    for entry in list.list()? {
        values.push(read(&entry.field("data")?)?);
        let weight = entry.field("weight")?.int()?;
        if weight < 0 {
            return Err(entry.error(format!("a weight of {weight}")));
        }
        weights.push(weight);
        entry.finish()?;
    }
    Ok((values, weights))
}

/// An `int` drawn by one of the game's `IntProvider`s.
#[derive(Debug, Clone, PartialEq)]
pub enum IntProvider {
    Constant(i32),
    Uniform {
        min: i32,
        max: i32,
    },
    BiasedToBottom {
        min: i32,
        max: i32,
    },
    VeryBiasedToBottom {
        min: i32,
        max: i32,
    },
    Trapezoid {
        min: i32,
        max: i32,
        plateau: i32,
    },
    ClampedNormal {
        mean: f32,
        deviation: f32,
        min: i32,
        max: i32,
    },
    Clamped {
        source: Box<IntProvider>,
        min: i32,
        max: i32,
    },
    WeightedList {
        values: Vec<IntProvider>,
        weights: Vec<i32>,
    },
}

impl IntProvider {
    /// Reads a provider: a bare number is a constant.
    pub fn read(reader: &Reader) -> Read<Self> {
        if reader.is_number() {
            return Ok(Self::Constant(reader.int()?));
        }
        let kind = reader.kind()?;
        let int = |key: &str| reader.field(key)?.int();
        let provider = match kind {
            "minecraft:constant" => Self::Constant(int("value")?),
            "minecraft:uniform" => Self::Uniform {
                min: int("min_inclusive")?,
                max: int("max_inclusive")?,
            },
            "minecraft:biased_to_bottom" => Self::BiasedToBottom {
                min: int("min_inclusive")?,
                max: int("max_inclusive")?,
            },
            "minecraft:very_biased_to_bottom" => Self::VeryBiasedToBottom {
                min: int("min_inclusive")?,
                max: int("max_inclusive")?,
            },
            "minecraft:trapezoid" => Self::Trapezoid {
                min: int("min")?,
                max: int("max")?,
                plateau: match reader.optional("plateau") {
                    Some(plateau) => plateau.int()?,
                    None => 0,
                },
            },
            "minecraft:clamped_normal" => Self::ClampedNormal {
                mean: reader.field("mean")?.float()?,
                deviation: reader.field("deviation")?.float()?,
                min: int("min_inclusive")?,
                max: int("max_inclusive")?,
            },
            "minecraft:clamped" => Self::Clamped {
                source: Box::new(Self::read(&reader.field("source")?)?),
                min: int("min_inclusive")?,
                max: int("max_inclusive")?,
            },
            "minecraft:weighted_list" => {
                let (values, weights) = read_weighted(&reader.field("distribution")?, Self::read)?;
                if weights.iter().sum::<i32>() <= 0 {
                    return Err(reader.error("a weighted list with no weight"));
                }
                Self::WeightedList { values, weights }
            }
            other => return Err(reader.error(format!("{other} is not an int provider"))),
        };
        reader.finish()?;
        Ok(provider)
    }

    pub fn sample<R: Random + ?Sized>(&self, random: &mut R) -> i32 {
        match self {
            Self::Constant(value) => *value,
            Self::Uniform { min, max } => random_between_inclusive(random, *min, *max),
            Self::BiasedToBottom { min, max } => {
                let bound = random.next_i32_bounded(max - min + 1) + 1;
                min + random.next_i32_bounded(bound)
            }
            Self::VeryBiasedToBottom { min, max } => {
                let outer = random.next_i32_bounded(max - min + 1);
                let middle = random.next_i32_bounded(outer + 1);
                min + random.next_i32_bounded(middle + 1)
            }
            Self::Trapezoid { min, max, plateau } => {
                // The game draws a symmetric spread without a plateau as the old
                // patches of plants did, so that their placement did not change.
                if *plateau == 0 && *max == -*min {
                    return random.next_i32_bounded(max + 1) - random.next_i32_bounded(max + 1);
                }
                let range = max - min;
                if *plateau >= range {
                    return random_between_inclusive(random, *min, *max);
                }
                let plateau_start = (range - plateau) / 2;
                let plateau_end = range - plateau_start;
                min + random_between_inclusive(random, 0, plateau_end)
                    + random_between_inclusive(random, 0, plateau_start)
            }
            Self::ClampedNormal {
                mean,
                deviation,
                min,
                max,
            } => {
                // `Mth.normal` in single precision, then clamped as a float.
                let value = mean + random.next_gaussian() as f32 * deviation;
                value.clamp(*min as f32, *max as f32) as i32
            }
            Self::Clamped { source, min, max } => source.sample(random).clamp(*min, *max),
            Self::WeightedList { values, weights } => {
                let index = weighted_index(weights, random)
                    .expect("a weighted list read from the data has weight");
                values[index].sample(random)
            }
        }
    }
}

/// A `float` drawn by one of the game's `FloatProvider`s.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FloatProvider {
    Constant(f32),
    Uniform {
        min: f32,
        max: f32,
    },
    ClampedNormal {
        mean: f32,
        deviation: f32,
        min: f32,
        max: f32,
    },
    Trapezoid {
        min: f32,
        max: f32,
        plateau: f32,
    },
}

impl FloatProvider {
    /// Reads a provider: a bare number is a constant.
    pub fn read(reader: &Reader) -> Read<Self> {
        if reader.is_number() {
            return Ok(Self::Constant(reader.float()?));
        }
        let float = |key: &str| reader.field(key)?.float();
        let provider = match reader.kind()? {
            "minecraft:constant" => Self::Constant(float("value")?),
            "minecraft:uniform" => Self::Uniform {
                min: float("min_inclusive")?,
                max: float("max_exclusive")?,
            },
            "minecraft:clamped_normal" => Self::ClampedNormal {
                mean: float("mean")?,
                deviation: float("deviation")?,
                min: float("min")?,
                max: float("max")?,
            },
            "minecraft:trapezoid" => Self::Trapezoid {
                min: float("min")?,
                max: float("max")?,
                plateau: float("plateau")?,
            },
            other => return Err(reader.error(format!("{other} is not a float provider"))),
        };
        reader.finish()?;
        Ok(provider)
    }

    pub fn sample<R: Random + ?Sized>(&self, random: &mut R) -> f32 {
        match *self {
            Self::Constant(value) => value,
            Self::Uniform { min, max } => random.next_f32() * (max - min) + min,
            Self::ClampedNormal {
                mean,
                deviation,
                min,
                max,
            } => (mean + random.next_gaussian() as f32 * deviation).clamp(min, max),
            Self::Trapezoid { min, max, plateau } => {
                let range = max - min;
                let plateau_start = (range - plateau) / 2.0;
                let plateau_end = range - plateau_start;
                min + random.next_f32() * plateau_end + random.next_f32() * plateau_start
            }
        }
    }
}

/// A height given relative to the world (`VerticalAnchor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalAnchor {
    Absolute(i32),
    AboveBottom(i32),
    BelowTop(i32),
    RelativeToSeaLevel(i32),
}

impl VerticalAnchor {
    /// Reads an anchor: an object with exactly one of its four members.
    pub fn read(reader: &Reader) -> Read<Self> {
        let mut found = Vec::new();
        for (key, make) in [
            ("absolute", Self::Absolute as fn(i32) -> Self),
            ("above_bottom", Self::AboveBottom),
            ("below_top", Self::BelowTop),
            ("relative_to_sea_level", Self::RelativeToSeaLevel),
        ] {
            if let Some(value) = reader.optional(key) {
                found.push(make(value.int()?));
            }
        }
        reader.finish()?;
        match found.as_slice() {
            [anchor] => Ok(*anchor),
            _ => Err(reader.error("an anchor has exactly one of its kinds")),
        }
    }

    /// The y the anchor stands for (`resolveY`).
    pub fn resolve(self, at: Generation) -> i32 {
        match self {
            Self::Absolute(y) => y,
            Self::AboveBottom(offset) => at.min_y + offset,
            Self::BelowTop(offset) => at.min_y + at.height - 1 - offset,
            Self::RelativeToSeaLevel(offset) => at.sea_level + offset,
        }
    }
}

/// A height drawn by one of the game's `HeightProvider`s.
#[derive(Debug, Clone, PartialEq)]
pub enum HeightProvider {
    Constant(VerticalAnchor),
    Uniform {
        min: VerticalAnchor,
        max: VerticalAnchor,
    },
    Trapezoid {
        min: VerticalAnchor,
        max: VerticalAnchor,
        plateau: i32,
    },
    BiasedToBottom {
        min: VerticalAnchor,
        max: VerticalAnchor,
        inner: i32,
    },
    VeryBiasedToBottom {
        min: VerticalAnchor,
        max: VerticalAnchor,
        inner: i32,
    },
    WeightedList {
        values: Vec<HeightProvider>,
        weights: Vec<i32>,
    },
}

impl HeightProvider {
    /// Reads a provider: an anchor without a `type` is a constant.
    pub fn read(reader: &Reader) -> Read<Self> {
        if !reader.has("type") {
            return Ok(Self::Constant(VerticalAnchor::read(reader)?));
        }
        let anchor = |key: &str| VerticalAnchor::read(&reader.field(key)?);
        let inner = || match reader.optional("inner") {
            Some(inner) => inner.int(),
            None => Ok(1),
        };
        let provider = match reader.kind()? {
            "minecraft:constant" => Self::Constant(anchor("value")?),
            "minecraft:uniform" => Self::Uniform {
                min: anchor("min_inclusive")?,
                max: anchor("max_inclusive")?,
            },
            "minecraft:trapezoid" => Self::Trapezoid {
                min: anchor("min_inclusive")?,
                max: anchor("max_inclusive")?,
                plateau: match reader.optional("plateau") {
                    Some(plateau) => plateau.int()?,
                    None => 0,
                },
            },
            "minecraft:biased_to_bottom" => Self::BiasedToBottom {
                min: anchor("min_inclusive")?,
                max: anchor("max_inclusive")?,
                inner: inner()?,
            },
            "minecraft:very_biased_to_bottom" => Self::VeryBiasedToBottom {
                min: anchor("min_inclusive")?,
                max: anchor("max_inclusive")?,
                inner: inner()?,
            },
            "minecraft:weighted_list" => {
                let (values, weights) = read_weighted(&reader.field("distribution")?, Self::read)?;
                if weights.iter().sum::<i32>() <= 0 {
                    return Err(reader.error("a weighted list with no weight"));
                }
                Self::WeightedList { values, weights }
            }
            other => return Err(reader.error(format!("{other} is not a height provider"))),
        };
        reader.finish()?;
        Ok(provider)
    }

    /// A height. Where the range is empty the game warns and gives the lowest end
    /// without drawing.
    pub fn sample<R: Random + ?Sized>(&self, random: &mut R, at: Generation) -> i32 {
        match self {
            Self::Constant(anchor) => anchor.resolve(at),
            Self::Uniform { min, max } => {
                let (low, high) = (min.resolve(at), max.resolve(at));
                if low > high {
                    return low;
                }
                random_between_inclusive(random, low, high)
            }
            Self::Trapezoid { min, max, plateau } => {
                let (low, high) = (min.resolve(at), max.resolve(at));
                if low > high {
                    return low;
                }
                let range = high - low;
                if *plateau >= range {
                    return random_between_inclusive(random, low, high);
                }
                let plateau_start = (range - plateau) / 2;
                let plateau_end = range - plateau_start;
                low + random_between_inclusive(random, 0, plateau_end)
                    + random_between_inclusive(random, 0, plateau_start)
            }
            Self::BiasedToBottom { min, max, inner } => {
                let (low, high) = (min.resolve(at), max.resolve(at));
                if high - low - inner < 0 {
                    return low;
                }
                let limit = random.next_i32_bounded(high - low - inner + 1);
                random.next_i32_bounded(limit + inner) + low
            }
            Self::VeryBiasedToBottom { min, max, inner } => {
                let (low, high) = (min.resolve(at), max.resolve(at));
                if high - low - inner < 0 {
                    return low;
                }
                let upper = next_int_between(random, low + inner, high);
                let biased = next_int_between(random, low, upper - 1);
                next_int_between(random, low, biased - 1 + inner)
            }
            Self::WeightedList { values, weights } => {
                let index = weighted_index(weights, random)
                    .expect("a weighted list read from the data has weight");
                values[index].sample(random, at)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use clustine_noise::WorldgenRandom;
    use clustine_worldgen_data::feature::Data;

    use super::*;

    const OVERWORLD: Generation = Generation {
        min_y: -64,
        height: 384,
        sea_level: 63,
    };

    fn int(data: &'static Data) -> IntProvider {
        IntProvider::read(&Reader::new(data, "test")).unwrap()
    }

    #[test]
    fn a_bare_number_is_a_constant_and_draws_nothing() {
        static SEVEN: Data = Data::Number("7");
        let mut random = WorldgenRandom::from_seed(1);
        let mut untouched = random.clone();
        assert_eq!(int(&SEVEN).sample(&mut random), 7);
        assert_eq!(random.next_i64(), untouched.next_i64());
    }

    #[test]
    fn a_uniform_int_is_one_bounded_draw_from_its_lowest() {
        static UNIFORM: Data = Data::Object(&[
            ("type", Data::Text("minecraft:uniform")),
            ("max_inclusive", Data::Number("5")),
            ("min_inclusive", Data::Number("-3")),
        ]);
        let mut random = WorldgenRandom::from_seed(42);
        let mut expected = random.clone();
        let value = int(&UNIFORM).sample(&mut random);
        assert_eq!(value, expected.next_i32_bounded(9) - 3);
    }

    #[test]
    fn a_symmetric_trapezoid_without_a_plateau_is_the_difference_of_two_draws() {
        static SPREAD: Data = Data::Object(&[
            ("type", Data::Text("minecraft:trapezoid")),
            ("max", Data::Number("7")),
            ("min", Data::Number("-7")),
            ("plateau", Data::Number("0")),
        ]);
        let mut random = WorldgenRandom::from_seed(9);
        let mut expected = random.clone();
        let first = expected.next_i32_bounded(8);
        let second = expected.next_i32_bounded(8);
        assert_eq!(int(&SPREAD).sample(&mut random), first - second);
    }

    #[test]
    fn a_lopsided_trapezoid_adds_two_draws_to_its_lowest() {
        let provider = IntProvider::Trapezoid {
            min: 0,
            max: 10,
            plateau: 2,
        };
        let mut random = WorldgenRandom::from_seed(5);
        let mut expected = random.clone();
        let first = expected.next_i32_bounded(7);
        let second = expected.next_i32_bounded(5);
        assert_eq!(provider.sample(&mut random), first + second);
    }

    #[test]
    fn a_weighted_list_draws_the_entry_then_samples_it() {
        static CACTUS: Data = Data::Object(&[
            ("type", Data::Text("minecraft:weighted_list")),
            (
                "distribution",
                Data::List(&[
                    Data::Object(&[("data", Data::Number("0")), ("weight", Data::Number("3"))]),
                    Data::Object(&[("data", Data::Number("1")), ("weight", Data::Number("1"))]),
                ]),
            ),
        ]);
        let provider = int(&CACTUS);
        for seed in 0..20 {
            let mut random = WorldgenRandom::from_seed(seed);
            let mut expected = random.clone();
            let pick = expected.next_i32_bounded(4);
            assert_eq!(provider.sample(&mut random), i32::from(pick >= 3));
        }
    }

    #[test]
    fn a_clamped_int_keeps_its_source_within_bounds() {
        static CLAMPED: Data = Data::Object(&[
            ("type", Data::Text("minecraft:clamped")),
            ("max_inclusive", Data::Number("3")),
            ("min_inclusive", Data::Number("0")),
            (
                "source",
                Data::Object(&[
                    ("type", Data::Text("minecraft:uniform")),
                    ("max_inclusive", Data::Number("3")),
                    ("min_inclusive", Data::Number("-1")),
                ]),
            ),
        ]);
        let provider = int(&CLAMPED);
        for seed in 0..50 {
            let mut random = WorldgenRandom::from_seed(seed);
            let mut expected = random.clone();
            let raw = expected.next_i32_bounded(5) - 1;
            assert_eq!(provider.sample(&mut random), raw.max(0));
        }
    }

    #[test]
    fn a_clamped_normal_int_truncates_the_clamped_float() {
        let provider = IntProvider::ClampedNormal {
            mean: 0.0,
            deviation: 3.0,
            min: -10,
            max: 10,
        };
        let mut random = WorldgenRandom::from_seed(77);
        let mut expected = random.clone();
        let value = (expected.next_gaussian() as f32 * 3.0).clamp(-10.0, 10.0) as i32;
        assert_eq!(provider.sample(&mut random), value);
    }

    #[test]
    fn anchors_resolve_against_the_generation_heights() {
        assert_eq!(VerticalAnchor::Absolute(42).resolve(OVERWORLD), 42);
        assert_eq!(VerticalAnchor::AboveBottom(8).resolve(OVERWORLD), -56);
        assert_eq!(VerticalAnchor::BelowTop(8).resolve(OVERWORLD), 311);
        assert_eq!(
            VerticalAnchor::RelativeToSeaLevel(-2).resolve(OVERWORLD),
            61
        );
        let nether = Generation {
            min_y: 0,
            height: 128,
            sea_level: 32,
        };
        assert_eq!(VerticalAnchor::BelowTop(1).resolve(nether), 126);
    }

    #[test]
    fn an_anchor_without_a_type_is_a_constant_height() {
        static ANCHOR: Data = Data::Object(&[("above_bottom", Data::Number("4"))]);
        let provider = HeightProvider::read(&Reader::new(&ANCHOR, "test")).unwrap();
        assert_eq!(
            provider,
            HeightProvider::Constant(VerticalAnchor::AboveBottom(4))
        );
        let mut random = WorldgenRandom::from_seed(0);
        assert_eq!(provider.sample(&mut random, OVERWORLD), -60);
    }

    #[test]
    fn an_anchor_with_two_kinds_is_refused() {
        static TWO: Data = Data::Object(&[
            ("above_bottom", Data::Number("4")),
            ("absolute", Data::Number("4")),
        ]);
        assert!(VerticalAnchor::read(&Reader::new(&TWO, "test")).is_err());
    }

    #[test]
    fn a_trapezoid_height_draws_as_the_game_does() {
        // The game's ore of diamonds: from 80 below the bottom to 80 above it.
        let provider = HeightProvider::Trapezoid {
            min: VerticalAnchor::AboveBottom(-80),
            max: VerticalAnchor::AboveBottom(80),
            plateau: 0,
        };
        let mut random = WorldgenRandom::from_seed(3);
        let mut expected = random.clone();
        let first = expected.next_i32_bounded(81);
        let second = expected.next_i32_bounded(81);
        assert_eq!(
            provider.sample(&mut random, OVERWORLD),
            -144 + first + second
        );
    }

    #[test]
    fn a_very_biased_height_draws_three_times_inside_a_wide_range() {
        let provider = HeightProvider::VeryBiasedToBottom {
            min: VerticalAnchor::AboveBottom(0),
            max: VerticalAnchor::BelowTop(8),
            inner: 8,
        };
        let mut random = WorldgenRandom::from_seed(11);
        let mut expected = random.clone();
        let upper = expected.next_i32_bounded(311 - -56 + 1) + -56;
        let biased = expected.next_i32_bounded(upper - 1 - -64 + 1) + -64;
        let value = expected.next_i32_bounded(biased - 1 + 8 - -64 + 1) + -64;
        assert_eq!(provider.sample(&mut random, OVERWORLD), value);
    }

    #[test]
    fn an_empty_height_range_gives_its_lowest_end_without_drawing() {
        let provider = HeightProvider::Uniform {
            min: VerticalAnchor::Absolute(10),
            max: VerticalAnchor::Absolute(5),
        };
        let mut random = WorldgenRandom::from_seed(4);
        let mut untouched = random.clone();
        assert_eq!(provider.sample(&mut random, OVERWORLD), 10);
        assert_eq!(random.next_i64(), untouched.next_i64());
    }

    #[test]
    fn a_uniform_float_scales_one_draw() {
        let provider = FloatProvider::Uniform { min: 0.7, max: 1.4 };
        let mut random = WorldgenRandom::from_seed(8);
        let mut expected = random.clone();
        let value = expected.next_f32() * (1.4_f32 - 0.7) + 0.7;
        assert_eq!(provider.sample(&mut random).to_bits(), value.to_bits());
    }
}
