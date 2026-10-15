// Adapted from SteelMC's `steel-core/src/worldgen/feature/providers.rs`.

//! The game's block-state providers (`BlockStateProvider`): what a feature places at a
//! position, drawn from its random source, from noise or by rules about the world.

use std::sync::Arc;

use clustine_data::{BLOCKS, BlockState, Face};
use clustine_noise::{LegacyRandom, NormalNoise, Random, math};
use clustine_world::BlockPos;
use clustine_worldgen_data::feature as data;

use crate::blocks::{BlockSet, axis_name, face_name, read_face, read_state};
use crate::context::Context;
use crate::predicates::BlockPredicate;
use crate::reader::{Read, Reader};
use crate::values::{IntProvider, read_weighted, weighted_index};

/// The directions in the game's order (`Direction.values()`).
const DIRECTIONS: [Face; 6] = Face::ALL;

/// A noise a provider reads (`NormalNoise` made from the provider's seed).
#[derive(Debug, Clone)]
pub struct ProviderNoise {
    noise: Arc<NormalNoise>,
    scale: f32,
}

impl ProviderNoise {
    /// Reads the noise's parameters (26.3's form) and makes it, as the game does,
    /// from a legacy generator seeded with `seed`.
    fn read(reader: &Reader, seed: i64, scale: f32) -> Read<Self> {
        let base_octave = reader.field("base_octave")?.int()?;
        let base_amplitude = match reader.optional("base_amplitude") {
            Some(amplitude) => amplitude.double()?,
            None => 1.0,
        };
        let octave_count = match reader.optional("octave_count") {
            Some(count) => count.int()?,
            None => 1,
        };
        let normalise = match reader.optional("normalize") {
            Some(normalise) => normalise.boolean()?,
            None => true,
        };
        let modifiers = match reader.optional("amplitude_modifiers") {
            Some(list) => list
                .list()?
                .iter()
                .map(Reader::double)
                .collect::<Read<Vec<f64>>>()?,
            None => Vec::new(),
        };
        reader.finish()?;
        let noise = NormalNoise::create_from_random_with_params(
            &mut LegacyRandom::from_seed(seed as u64),
            base_octave,
            base_amplitude,
            octave_count,
            normalise,
            &modifiers,
        );
        Ok(Self {
            noise: Arc::new(noise),
            scale,
        })
    }

    /// The noise at a block, its coordinates scaled in single precision as the
    /// game's `float` scale scales them.
    fn at(&self, pos: BlockPos) -> f32 {
        self.noise.get(
            f64::from(pos.x as f32 * self.scale),
            f64::from(pos.y as f32 * self.scale),
            f64::from(pos.z as f32 * self.scale),
        )
    }
}

/// The state a noise value picks from a list (`getRandomState`).
fn by_noise(states: &[BlockState], noise: f32) -> BlockState {
    let placement = ((1.0_f32 + noise) / 2.0).clamp(0.0, 0.9999);
    states[(placement * states.len() as f32) as usize]
}

/// A block-state provider.
#[derive(Debug, Clone)]
pub enum Provider {
    /// One state (`simple_state_provider`, or a state written in its place).
    Simple(BlockState),
    Weighted {
        states: Vec<BlockState>,
        weights: Vec<i32>,
    },
    /// A state turned to a direction: the one given, or one drawn.
    Rotated {
        source: Box<Provider>,
        direction: Option<Face>,
    },
    RandomizedInt {
        source: Box<Provider>,
        property: &'static str,
        values: IntProvider,
    },
    /// The first rule whose predicate holds; else the fallback, if any.
    RuleBased {
        rules: Vec<(BlockPredicate, Provider)>,
        fallback: Option<Box<Provider>>,
    },
    /// The default state of a block drawn from a set.
    RandomBlock(BlockSet),
    Noise {
        noise: ProviderNoise,
        states: Vec<BlockState>,
    },
    NoiseThreshold {
        noise: ProviderNoise,
        threshold: f32,
        high_chance: f32,
        default: BlockState,
        low: Vec<BlockState>,
        high: Vec<BlockState>,
    },
    DualNoise {
        noise: ProviderNoise,
        slow: ProviderNoise,
        variety: (i32, i32),
        states: Vec<BlockState>,
    },
    /// A provider of `worldgen/block_state_provider`, by its name.
    Named(&'static str, Box<Provider>),
}

impl Provider {
    /// Reads a provider where the game's codec takes one: the name of a named
    /// provider, a block state written out, or a provider with its `type`.
    pub fn read(reader: &Reader) -> Read<Self> {
        if reader.is_text() {
            let name = reader.text()?;
            if let Some((_, entry)) = data::provider(name) {
                let provider = Self::read(&Reader::new(&entry.data, entry.name))?;
                return Ok(Self::Named(entry.name, Box::new(provider)));
            }
            return Ok(Self::Simple(read_state(reader)?));
        }
        if !reader.has("type") {
            return Ok(Self::Simple(read_state(reader)?));
        }
        let states = |key: &str| -> Read<Vec<BlockState>> {
            let states = reader
                .field(key)?
                .list()?
                .iter()
                .map(read_state)
                .collect::<Read<Vec<_>>>()?;
            if states.is_empty() {
                return Err(reader.error(format!("{key} is empty")));
            }
            Ok(states)
        };
        let noise = |key: &str, scale: &str| -> Read<ProviderNoise> {
            let seed = reader.field("seed")?.long()?;
            let scale = reader.field(scale)?.float()?;
            ProviderNoise::read(&reader.field(key)?, seed, scale)
        };
        let provider = match reader.kind()? {
            "minecraft:simple_state_provider" => Self::Simple(read_state(&reader.field("state")?)?),
            "minecraft:weighted" => {
                let (states, weights) = read_weighted(&reader.field("entries")?, read_state)?;
                if weights.iter().sum::<i32>() <= 0 {
                    return Err(reader.error("a weighted provider with no weight"));
                }
                Self::Weighted { states, weights }
            }
            "minecraft:rotated" => Self::Rotated {
                source: Box::new(Self::read(&reader.field("state")?)?),
                direction: match reader.optional("direction") {
                    Some(direction) => Some(read_face(&direction)?),
                    None => None,
                },
            },
            "minecraft:randomized_int" => {
                let property = reader.field("property")?.text()?;
                Self::RandomizedInt {
                    source: Box::new(Self::read(&reader.field("source")?)?),
                    property,
                    values: IntProvider::read(&reader.field("values")?)?,
                }
            }
            "minecraft:rule_based" => {
                let mut rules = Vec::new();
                for rule in reader.field("rules")?.list()? {
                    rules.push((
                        BlockPredicate::read(&rule.field("if_true")?)?,
                        Self::read(&rule.field("then")?)?,
                    ));
                    rule.finish()?;
                }
                let fallback = match reader.optional("fallback") {
                    Some(fallback) => Some(Box::new(Self::read(&fallback)?)),
                    None => None,
                };
                Self::RuleBased { rules, fallback }
            }
            "minecraft:random_block" => {
                let blocks = BlockSet::read(&reader.field("blocks")?)?;
                if blocks.blocks().is_empty() {
                    return Err(reader.error("a random block from no blocks"));
                }
                Self::RandomBlock(blocks)
            }
            "minecraft:noise" => Self::Noise {
                noise: noise("noise", "scale")?,
                states: states("states")?,
            },
            "minecraft:noise_threshold" => Self::NoiseThreshold {
                noise: noise("noise", "scale")?,
                threshold: reader.field("threshold")?.float()?,
                high_chance: reader.field("high_chance")?.float()?,
                default: read_state(&reader.field("default_state")?)?,
                low: states("low_states")?,
                high: states("high_states")?,
            },
            "minecraft:dual_noise" => {
                let variety = reader.field("variety")?;
                let variety = match variety.list()?.as_slice() {
                    [min, max] => (min.int()?, max.int()?),
                    _ => return Err(variety.error("a variety is two numbers")),
                };
                Self::DualNoise {
                    noise: noise("noise", "scale")?,
                    slow: noise("slow_noise", "slow_scale")?,
                    variety,
                    states: states("states")?,
                }
            }
            other => return Err(reader.error(format!("{other} is not a block-state provider"))),
        };
        reader.finish()?;
        Ok(provider)
    }

    /// The state to place at `pos` (`getState`). A rule-based provider whose rules
    /// all fail and that has no fallback leaves the block that is there.
    pub fn state(&self, context: &mut Context<'_, '_>, pos: BlockPos) -> BlockState {
        match self.optional_state(context, pos) {
            Some(state) => state,
            None => context.level.block(pos),
        }
    }

    /// The state to place at `pos`, or none where a rule-based provider has nothing
    /// to say (`getOptionalState`): a simple block then places nothing.
    pub fn optional_state(
        &self,
        context: &mut Context<'_, '_>,
        pos: BlockPos,
    ) -> Option<BlockState> {
        Some(match self {
            Self::Simple(state) => *state,
            Self::Weighted { states, weights } => {
                let index = weighted_index(weights, &mut *context.random)
                    .expect("a weighted provider read from the data has weight");
                states[index]
            }
            Self::Rotated { source, direction } => {
                let direction = match direction {
                    Some(direction) => *direction,
                    None => DIRECTIONS[context.random.next_i32_bounded(6) as usize],
                };
                let state = source.state(context, pos);
                rotate(state, direction)
            }
            Self::RandomizedInt {
                source,
                property,
                values,
            } => {
                let state = source.state(context, pos);
                // The game draws a value only for a state that has the property.
                if state.property(property).is_none() {
                    return Some(state);
                }
                let value = values.sample(&mut *context.random);
                state
                    .with_property(property, &value.to_string())
                    .unwrap_or_else(|| {
                        panic!("{value} is not a value of {property} of {state:?}, which the game refuses")
                    })
            }
            Self::RuleBased { rules, fallback } => {
                for (predicate, then) in rules {
                    if predicate.test(context, pos) {
                        return then.optional_state(context, pos);
                    }
                }
                return fallback
                    .as_ref()
                    .and_then(|fallback| fallback.optional_state(context, pos));
            }
            Self::RandomBlock(blocks) => {
                let ids = blocks.blocks();
                let index = context.random.next_i32_bounded(ids.len() as i32) as usize;
                BLOCKS[ids[index]].default_state
            }
            Self::Noise { noise, states } => by_noise(states, noise.at(pos)),
            Self::NoiseThreshold {
                noise,
                threshold,
                high_chance,
                default,
                low,
                high,
            } => {
                if noise.at(pos) < *threshold {
                    low[context.random.next_i32_bounded(low.len() as i32) as usize]
                } else if context.random.next_f32() < *high_chance {
                    high[context.random.next_i32_bounded(high.len() as i32) as usize]
                } else {
                    *default
                }
            }
            Self::DualNoise {
                noise,
                slow,
                variety,
                states,
            } => {
                let local = math::map_clamped(
                    f64::from(slow.at(pos)),
                    -1.0,
                    1.0,
                    f64::from(variety.0),
                    f64::from(variety.1 + 1),
                ) as i32;
                let possible: Vec<BlockState> = (0..local)
                    .map(|i| by_noise(states, slow.at(pos.offset(i * 54_545, 0, i * 34_234))))
                    .collect();
                by_noise(&possible, noise.at(pos))
            }
            Self::Named(_, provider) => return provider.optional_state(context, pos),
        })
    }
}

/// `state` turned to `direction`: its axis, its facing, or its horizontal facing
/// where the direction is horizontal.
fn rotate(state: BlockState, direction: Face) -> BlockState {
    let mut state = state;
    if state.property("axis").is_some() {
        state = state
            .with_property("axis", axis_name(direction))
            .unwrap_or(state);
    }
    // A block that faces any way takes every direction; one that faces only along
    // the ground takes the horizontal ones, which are all its property has.
    if let Some(turned) = state
        .property("facing")
        .and(state.with_property("facing", face_name(direction)))
    {
        state = turned;
    }
    state
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;

    use super::*;
    use crate::testing::World;

    fn provider(data: &'static data::Data) -> Provider {
        Provider::read(&Reader::new(data, "test")).unwrap()
    }

    #[test]
    fn a_state_written_out_is_a_simple_provider_and_draws_nothing() {
        static CACTUS: data::Data = data::Data::Object(&[
            ("id", data::Data::Text("minecraft:cactus")),
            (
                "properties",
                data::Data::Object(&[("age", data::Data::Text("0"))]),
            ),
        ]);
        let cactus = provider(&CACTUS);
        let mut world = World::flat(blocks::SAND, 62);
        world.with_context(5, |context| {
            let mut untouched = context.random.clone();
            assert_eq!(
                cactus.state(context, BlockPos::new(0, 63, 0)),
                blocks::CACTUS
            );
            assert_eq!(context.random.next_i64(), untouched.next_i64());
        });
    }

    #[test]
    fn a_weighted_provider_draws_below_its_total_weight() {
        static VINES: data::Data = data::Data::Text("minecraft:cave_vines_body");
        let vines = provider(&VINES);
        assert!(matches!(
            vines,
            Provider::Named("minecraft:cave_vines_body", _)
        ));
        let mut world = World::flat(blocks::STONE, 0);
        for seed in 0..30 {
            world.with_context(seed, |context| {
                let mut expected = context.random.clone();
                let berries = expected.next_i32_bounded(5) >= 4;
                let state = vines.state(context, BlockPos::new(0, 10, 0));
                assert_eq!(
                    state.property("berries"),
                    Some(if berries { "true" } else { "false" })
                );
            });
        }
    }

    #[test]
    fn a_randomized_int_sets_the_property_after_the_source_draws() {
        static HEAD: data::Data = data::Data::Text("minecraft:cave_vines_head");
        let head = provider(&HEAD);
        let mut world = World::flat(blocks::STONE, 0);
        world.with_context(17, |context| {
            let mut expected = context.random.clone();
            let berries = expected.next_i32_bounded(5) >= 4;
            let age = expected.next_i32_bounded(3) + 23;
            let state = head.state(context, BlockPos::new(0, 10, 0));
            assert_eq!(state.property("age"), Some(age.to_string().as_str()));
            assert_eq!(
                state.property("berries"),
                Some(if berries { "true" } else { "false" })
            );
        });
    }

    #[test]
    fn a_rule_based_provider_without_a_fallback_says_nothing_or_leaves_the_block() {
        static PODZOL: data::Data = data::Data::Text("minecraft:podzol_beneath_tree");
        let podzol = provider(&PODZOL);
        let mut world = World::flat(blocks::GRASS_BLOCK, 64);
        world.set(BlockPos::new(1, 64, 0), blocks::STONE);
        world.with_context(0, |context| {
            let grass = BlockPos::new(0, 64, 0);
            let state = podzol.optional_state(context, grass).unwrap();
            assert!(crate::blocks::same_block(state, blocks::PODZOL));
            let stone = BlockPos::new(1, 64, 0);
            assert_eq!(podzol.optional_state(context, stone), None);
            assert_eq!(podzol.state(context, stone), blocks::STONE);
        });
    }

    #[test]
    fn a_rotated_state_takes_the_direction_s_axis_or_facing() {
        assert_eq!(
            rotate(blocks::BASALT, Face::East).property("axis"),
            Some("x")
        );
        let fan = blocks::TUBE_CORAL_WALL_FAN;
        assert_eq!(rotate(fan, Face::South).property("facing"), Some("south"));
        assert_eq!(rotate(fan, Face::Up), fan);
        assert_eq!(rotate(blocks::STONE, Face::West), blocks::STONE);
    }

    #[test]
    fn a_random_block_is_drawn_from_its_set_in_the_game_s_order() {
        static CORALS: data::Data = data::Data::Object(&[
            ("type", data::Data::Text("minecraft:random_block")),
            ("blocks", data::Data::Text("#minecraft:corals")),
        ]);
        let corals = provider(&CORALS);
        let tag = crate::blocks::block_tag("minecraft:corals").unwrap();
        let mut world = World::flat(blocks::SAND, 0);
        world.with_context(2, |context| {
            let mut expected = context.random.clone();
            let index = expected.next_i32_bounded(tag.len() as i32) as usize;
            let state = corals.state(context, BlockPos::new(0, 1, 0));
            assert_eq!(state, BLOCKS[tag[index] as usize].default_state);
        });
    }

    #[test]
    fn noise_picks_its_state_in_single_precision() {
        // SteelMC's cases, from the game's formula in floats.
        let states: Vec<BlockState> = (0..32).map(|i| BlockState(i + 1)).collect();
        for (count, noise, expected) in [
            (2, -1.5_f32, 0),
            (4, -0.5, 1),
            (8, 0.0, 4),
            (16, 0.75, 14),
            (32, 1.5, 31),
            (2, -(2.0_f32.powi(-25)), 1),
            (8, -(2.0_f32.powi(-25)), 4),
        ] {
            assert_eq!(by_noise(&states[..count], noise), states[expected]);
        }
    }

    /// Calls `visit` with every member of every object in `data`, depth first.
    fn walk(data: &'static data::Data, visit: &mut dyn FnMut(&'static str, &'static data::Data)) {
        match data {
            data::Data::Object(members) => {
                for (key, value) in members.iter() {
                    visit(key, value);
                    walk(value, visit);
                }
            }
            data::Data::List(items) => {
                for item in items.iter() {
                    walk(item, visit);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn every_provider_predicate_and_modifier_in_the_data_is_read_whole_wherever_it_stands() {
        // Kinds not built yet skip their settings; the later steps read their
        // providers, predicates and modifiers with this step's code, so each is read
        // here wherever it stands in the data.
        const PREDICATES: [&str; 14] = [
            "all_of",
            "any_of",
            "not",
            "true",
            "matching_blocks",
            "matching_block_tag",
            "matching_fluids",
            "solid",
            "replaceable",
            "would_survive",
            "has_sturdy_face",
            "inside_world_bounds",
            "height_range",
            "volume_match",
        ];
        const PROVIDERS: [&str; 9] = [
            "weighted",
            "rotated",
            "randomized_int",
            "rule_based",
            "random_block",
            "noise",
            "dual_noise",
            "noise_threshold",
            "simple_state_provider",
        ];
        let kind = |value: &data::Data| {
            value
                .kind()
                .and_then(|kind| kind.strip_prefix("minecraft:"))
        };
        let (mut predicates, mut providers, mut modifiers) = (0, 0, 0);
        let entries = data::configured_features()
            .iter()
            .chain(data::placed_features())
            .chain(data::providers());
        for entry in entries {
            walk(&entry.data, &mut |key, value| {
                let at = format!("{}.{key}", entry.name);
                if key == "placement" {
                    if let data::Data::List(items) = value {
                        for item in items.iter() {
                            crate::placement::read_modifier(&Reader::new(item, at.clone()))
                                .unwrap_or_else(|error| panic!("{error}"));
                            modifiers += 1;
                        }
                    }
                } else if key == "placements" {
                    // The modifiers `randomly_selected` chooses from.
                } else if kind(value).is_some_and(|kind| PREDICATES.contains(&kind))
                    && !matches!(key, "predicates" | "match")
                {
                    BlockPredicate::read(&Reader::new(value, at))
                        .unwrap_or_else(|error| panic!("{error}"));
                    predicates += 1;
                } else if kind(value).is_some_and(|kind| PROVIDERS.contains(&kind)) {
                    Provider::read(&Reader::new(value, at))
                        .unwrap_or_else(|error| panic!("{error}"));
                    providers += 1;
                }
            });
        }
        assert!(predicates > 300 && providers > 50 && modifiers > 1000);
    }

    #[test]
    fn every_named_provider_is_read_whole() {
        for entry in data::providers() {
            Provider::read(&Reader::new(&entry.data, entry.name))
                .unwrap_or_else(|error| panic!("{error}"));
        }
    }

    #[test]
    fn the_flowers_of_the_plains_are_chosen_by_noise_and_draws() {
        static PLAIN: data::Data = data::Data::Text("minecraft:flower_plain");
        let plain = provider(&PLAIN);
        let mut world = World::flat(blocks::GRASS_BLOCK, 64);
        let mut seen = std::collections::BTreeSet::new();
        for x in 0..64 {
            world.with_context(x as u64, |context| {
                let state = plain.state(context, BlockPos::new(x * 37, 65, x * 11));
                seen.insert(state.block().unwrap().name);
            });
        }
        assert!(seen.len() > 1, "{seen:?}");
    }
}
