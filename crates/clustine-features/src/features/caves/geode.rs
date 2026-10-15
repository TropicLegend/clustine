// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/geode.rs`.

//! `geode`: a hollow ball of layers in the stone (smooth basalt, calcite, amethyst with
//! budding amethyst among it), its shape the sum of distances to a few points drawn
//! about the origin, roughened by a noise, often with a crack through its shell to the
//! cave beside it; buds and clusters grow into the hollow from its inner layer
//! (`GeodeFeature`).
//!
//! A geode reaches up to 16 blocks from its origin, and a bud at its edge one further,
//! so it can reach beyond the three by three chunks its run may write: those writes are
//! refused by the view, as the game's `ensureCanWrite` refuses them. ADR-0021's corner
//! case is the read of a block 17 away and a fluid tick scheduled there, which the
//! view answers from terrain and drops.

use std::sync::OnceLock;

use clustine_data::{BlockState, Face, blocks};
use clustine_noise::{LegacyRandom, NormalNoise, Random};
use clustine_world::BlockPos;

use super::shared::{DIRECTIONS, int_max};
use crate::blocks::{BlockSet, fluid_id, read_state, relative, same_block};
use crate::context::{Context, Level};
use crate::feature::Feature;
use crate::features::ground::shared::between_closed;
use crate::providers::Provider;
use crate::reader::{Read, Reader};
use crate::values::IntProvider;

/// `GeodeBlockSettings`.
#[derive(Debug)]
struct Blocks {
    filling: Provider,
    inner_layer: Provider,
    alternate_inner_layer: Provider,
    middle_layer: Provider,
    outer_layer: Provider,
    inner_placements: Vec<BlockState>,
    cannot_replace: BlockSet,
    invalid_blocks: BlockSet,
}

/// `GeodeLayerSettings`: the thresholds of the sum of distances for each layer.
#[derive(Debug)]
struct Layers {
    filling: f64,
    inner_layer: f64,
    middle_layer: f64,
    outer_layer: f64,
}

/// `GeodeCrackSettings`.
#[derive(Debug)]
struct Crack {
    generate_crack_chance: f64,
    base_crack_size: f64,
    crack_point_offset: i32,
}

#[derive(Debug)]
pub struct Geode {
    blocks: Blocks,
    layers: Layers,
    crack: Crack,
    use_potential_placements_chance: f64,
    use_alternate_layer0_chance: f64,
    placements_require_layer0_alternate: bool,
    outer_wall_distance: IntProvider,
    distribution_points: IntProvider,
    point_offset: IntProvider,
    min_gen_offset: i32,
    max_gen_offset: i32,
    invalid_blocks_threshold: i32,
    noise_multiplier: f64,
    /// The noise that roughens the shell, made from the world's seed when first asked
    /// for, with that seed: settings are made for one world, so it is made once.
    noise: OnceLock<(i64, NormalNoise)>,
}

fn double_or(reader: &Reader, key: &str, default: f64) -> Read<f64> {
    match reader.optional(key) {
        Some(value) => value.double(),
        None => Ok(default),
    }
}

fn int_provider_or(reader: &Reader, key: &str, min: i32, max: i32) -> Read<IntProvider> {
    match reader.optional(key) {
        Some(value) => IntProvider::read(&value),
        None => Ok(IntProvider::Uniform { min, max }),
    }
}

impl Blocks {
    fn read(reader: &Reader) -> Read<Self> {
        let provider = |key: &str| Provider::read(&reader.field(key)?);
        let blocks = Self {
            filling: provider("filling_provider")?,
            inner_layer: provider("inner_layer_provider")?,
            alternate_inner_layer: provider("alternate_inner_layer_provider")?,
            middle_layer: provider("middle_layer_provider")?,
            outer_layer: provider("outer_layer_provider")?,
            inner_placements: reader
                .field("inner_placements")?
                .list()?
                .iter()
                .map(read_state)
                .collect::<Read<_>>()?,
            cannot_replace: BlockSet::read(&reader.field("cannot_replace")?)?,
            invalid_blocks: BlockSet::read(&reader.field("invalid_blocks")?)?,
        };
        reader.finish()?;
        Ok(blocks)
    }
}

impl Layers {
    fn read(reader: &Reader) -> Read<Self> {
        let layers = Self {
            filling: double_or(reader, "filling", 1.7)?,
            inner_layer: double_or(reader, "inner_layer", 2.2)?,
            middle_layer: double_or(reader, "middle_layer", 3.2)?,
            outer_layer: double_or(reader, "outer_layer", 4.2)?,
        };
        reader.finish()?;
        Ok(layers)
    }
}

impl Crack {
    fn read(reader: &Reader) -> Read<Self> {
        let crack = Self {
            generate_crack_chance: double_or(reader, "generate_crack_chance", 1.0)?,
            base_crack_size: double_or(reader, "base_crack_size", 2.0)?,
            crack_point_offset: match reader.optional("crack_point_offset") {
                Some(offset) => offset.int()?,
                None => 2,
            },
        };
        reader.finish()?;
        Ok(crack)
    }
}

impl Geode {
    pub fn read(reader: &Reader) -> Read<Self> {
        let int_or = |key: &str, default: i32| -> Read<i32> {
            match reader.optional(key) {
                Some(value) => value.int(),
                None => Ok(default),
            }
        };
        let geode = Self {
            blocks: Blocks::read(&reader.field("blocks")?)?,
            layers: Layers::read(&reader.field("layers")?)?,
            crack: Crack::read(&reader.field("crack")?)?,
            use_potential_placements_chance: double_or(
                reader,
                "use_potential_placements_chance",
                0.35,
            )?,
            use_alternate_layer0_chance: double_or(reader, "use_alternate_layer0_chance", 0.0)?,
            placements_require_layer0_alternate: match reader
                .optional("placements_require_layer0_alternate")
            {
                Some(value) => value.boolean()?,
                None => true,
            },
            outer_wall_distance: int_provider_or(reader, "outer_wall_distance", 4, 5)?,
            distribution_points: int_provider_or(reader, "distribution_points", 3, 4)?,
            point_offset: int_provider_or(reader, "point_offset", 1, 2)?,
            min_gen_offset: int_or("min_gen_offset", -16)?,
            max_gen_offset: int_or("max_gen_offset", 16)?,
            invalid_blocks_threshold: reader.field("invalid_blocks_threshold")?.int()?,
            noise_multiplier: double_or(reader, "noise_multiplier", 0.05)?,
            noise: OnceLock::new(),
        };
        reader.finish()?;
        Ok(geode)
    }

    /// `safeSetBlock` with the geode's `cannot_replace`.
    fn safe_set(&self, level: &mut Level<'_>, pos: BlockPos, state: BlockState) {
        if !self.blocks.cannot_replace.contains(level.block(pos)) {
            level.set_block(pos, state);
        }
    }

    /// Where the shell's points lie, each with its offset; none where more of them than
    /// the threshold lie in air or in what a geode may not be in.
    fn distribution_points(
        &self,
        context: &mut Context<'_, '_>,
        origin: BlockPos,
        count: i32,
    ) -> Option<Vec<(BlockPos, i32)>> {
        let mut points = Vec::new();
        let mut invalid = 0;
        for _ in 0..count {
            let random = &mut *context.random;
            let x = self.outer_wall_distance.sample(random);
            let y = self.outer_wall_distance.sample(random);
            let z = self.outer_wall_distance.sample(random);
            let point = origin.offset(x, y, z);
            let state = context.level.block(point);
            if state.props().is_air() || self.blocks.invalid_blocks.contains(state) {
                invalid += 1;
                if invalid > self.invalid_blocks_threshold {
                    return None;
                }
            }
            points.push((point, self.point_offset.sample(&mut *context.random)));
        }
        Some(points)
    }

    /// `BuddingAmethystBlock.canClusterGrowAtState` and the placing of one bud or
    /// cluster on each chosen block of the inner layer: the first face, in the game's
    /// order, with air or still water beside it.
    fn place_crystals(&self, context: &mut Context<'_, '_>, placements: &[BlockPos]) {
        let placements_count = self.blocks.inner_placements.len();
        if placements_count == 0 {
            return;
        }
        let bound = i32::try_from(placements_count).expect("a geode lists few crystals");
        for pos in placements {
            let index = context.random.next_i32_bounded(bound) as usize;
            let mut state = self.blocks.inner_placements[index];
            for direction in DIRECTIONS {
                if let Some(facing) = state.with_property("facing", face_name(direction)) {
                    state = facing;
                }
                let at = relative(*pos, direction);
                let there = context.level.block(at);
                if state.property("waterlogged").is_some() {
                    let still_water = there.props().fluid().is_some_and(|fluid| fluid.source);
                    state = state
                        .with_property("waterlogged", if still_water { "true" } else { "false" })
                        .expect("the state has the property");
                }
                if can_cluster_grow_at(there) {
                    self.safe_set(context.level, at, state);
                    break;
                }
            }
        }
    }
}

/// The noise that roughens a geode's shell:
/// `NormalNoise.create(new WorldgenRandom(new LegacyRandomSource(seed)), -4, 1.0)`.
fn geode_noise(seed: i64) -> NormalNoise {
    NormalNoise::create_from_random(&mut LegacyRandom::from_seed(seed as u64), -4, &[1.0])
}

/// `canClusterGrowAtState`: air, or a full block of water.
fn can_cluster_grow_at(state: BlockState) -> bool {
    state.props().is_air()
        || (same_block(state, blocks::WATER)
            && state.props().fluid().is_some_and(|fluid| fluid.source))
}

fn face_name(face: Face) -> &'static str {
    match face {
        Face::Down => "down",
        Face::Up => "up",
        Face::North => "north",
        Face::South => "south",
        Face::West => "west",
        Face::East => "east",
    }
}

/// The sum over the points of the inverse of the distance to each, offset, plus the
/// noise for each.
fn distance_sum(pos: BlockPos, points: &[(BlockPos, i32)], noise: f64) -> f64 {
    points
        .iter()
        .map(|(point, offset)| {
            let dx = f64::from(pos.x - point.x);
            let dy = f64::from(pos.y - point.y);
            let dz = f64::from(pos.z - point.z);
            1.0 / (dx * dx + dy * dy + dz * dz + f64::from(*offset)).sqrt() + noise
        })
        .sum()
}

impl Feature for Geode {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let points_count = self.distribution_points.sample(&mut *context.random);
        let seed = context.settings.seed();
        let owned;
        let noise = match self.noise.get_or_init(|| (seed, geode_noise(seed))) {
            (made_for, noise) if *made_for == seed => noise,
            _ => {
                owned = geode_noise(seed);
                &owned
            }
        };
        let crack_size_adjustment =
            f64::from(points_count) / f64::from(int_max(&self.outer_wall_distance));
        let inner_air = 1.0 / self.layers.filling.sqrt();
        let innermost_layer = 1.0 / (self.layers.inner_layer + crack_size_adjustment).sqrt();
        let inner_crust = 1.0 / (self.layers.middle_layer + crack_size_adjustment).sqrt();
        let outer_crust = 1.0 / (self.layers.outer_layer + crack_size_adjustment).sqrt();
        let crack_size = 1.0
            / (self.crack.base_crack_size
                + context.random.next_f64() / 2.0
                + if points_count > 3 {
                    crack_size_adjustment
                } else {
                    0.0
                })
            .sqrt();
        // A float drawn and compared with a double, as Java compares them.
        let with_crack = f64::from(context.random.next_f32()) < self.crack.generate_crack_chance;

        let Some(points) = self.distribution_points(context, origin, points_count) else {
            return false;
        };
        let crack_points: Vec<(BlockPos, i32)> = if with_crack {
            let offset = points_count * 2 + 1;
            let (x, z) = match context.random.next_i32_bounded(4) {
                0 => (offset, 0),
                1 => (0, offset),
                2 => (offset, offset),
                _ => (0, 0),
            };
            [7, 5, 1]
                .into_iter()
                .map(|y| (origin.offset(x, y, z), self.crack.crack_point_offset))
                .collect()
        } else {
            Vec::new()
        };

        let mut potential_placements = Vec::new();
        let min = origin.offset(
            self.min_gen_offset,
            self.min_gen_offset,
            self.min_gen_offset,
        );
        let max = origin.offset(
            self.max_gen_offset,
            self.max_gen_offset,
            self.max_gen_offset,
        );
        for pos in between_closed(min, max) {
            let noise_offset =
                f64::from(noise.get(f64::from(pos.x), f64::from(pos.y), f64::from(pos.z)))
                    * self.noise_multiplier;
            let shell = distance_sum(pos, &points, noise_offset);
            let crack = distance_sum(pos, &crack_points, noise_offset);
            if shell < outer_crust {
                continue;
            }
            if with_crack && crack >= crack_size && shell < inner_air {
                self.safe_set(context.level, pos, blocks::AIR);
                for direction in DIRECTIONS {
                    let beside = relative(pos, direction);
                    let fluid = fluid_id(context.level.block(beside));
                    if fluid != 0 {
                        context.level.schedule_fluid_tick(beside, fluid, 0);
                    }
                }
            } else if shell >= inner_air {
                let state = self.blocks.filling.state(context, pos);
                self.safe_set(context.level, pos, state);
            } else if shell >= innermost_layer {
                let alternate =
                    f64::from(context.random.next_f32()) < self.use_alternate_layer0_chance;
                let provider = if alternate {
                    &self.blocks.alternate_inner_layer
                } else {
                    &self.blocks.inner_layer
                };
                let state = provider.state(context, pos);
                self.safe_set(context.level, pos, state);
                if (!self.placements_require_layer0_alternate || alternate)
                    && f64::from(context.random.next_f32()) < self.use_potential_placements_chance
                {
                    potential_placements.push(pos);
                }
            } else if shell >= inner_crust {
                let state = self.blocks.middle_layer.state(context, pos);
                self.safe_set(context.level, pos, state);
            } else {
                let state = self.blocks.outer_layer.state(context, pos);
                self.safe_set(context.level, pos, state);
            }
        }

        self.place_crystals(context, &potential_placements);
        true
    }
}

#[cfg(test)]
mod tests {
    use clustine_worldgen_data::feature as data;

    use super::*;
    use crate::testing::World;

    fn amethyst_geode() -> Box<dyn Feature> {
        let (_, entry) = data::configured("minecraft:amethyst_geode").unwrap();
        crate::feature::read_configured(&Reader::new(&entry.data, entry.name)).unwrap()
    }

    fn count(world: &World, state: BlockState) -> usize {
        world
            .writes
            .iter()
            .filter(|(_, written)| same_block(*written, state))
            .count()
    }

    #[test]
    fn the_amethyst_geode_reads_with_the_games_defaults() {
        let (_, entry) = data::configured("minecraft:amethyst_geode").unwrap();
        let reader = Reader::new(&entry.data, entry.name);
        reader.kind().unwrap();
        let geode = Geode::read(&reader).unwrap();
        assert_eq!(geode.layers.filling, 1.7);
        assert_eq!(geode.layers.outer_layer, 4.2);
        assert_eq!(geode.crack.generate_crack_chance, 0.95);
        assert_eq!(geode.crack.crack_point_offset, 2);
        assert_eq!(geode.use_alternate_layer0_chance, 0.083);
        assert_eq!(
            geode.distribution_points,
            IntProvider::Uniform { min: 3, max: 4 }
        );
        assert_eq!(int_max(&geode.outer_wall_distance), 6);
        assert!(geode.placements_require_layer0_alternate);
        assert_eq!(geode.blocks.inner_placements.len(), 4);
    }

    #[test]
    fn a_geode_in_stone_is_a_shell_of_basalt_calcite_and_amethyst_around_air() {
        let geode = amethyst_geode();
        let mut world = World::flat(blocks::STONE, 100);
        assert!(world.with_context(7, |context| {
            geode.place(context, BlockPos::new(8, 30, 8))
        }));
        assert!(count(&world, blocks::SMOOTH_BASALT) > 0);
        assert!(count(&world, blocks::CALCITE) > 0);
        assert!(count(&world, blocks::AMETHYST_BLOCK) > 0);
        assert!(count(&world, blocks::AIR) > 0);
        // Every write lies within the reach of a geode and its buds.
        assert!(world.writes.iter().all(|(pos, _)| {
            (pos.x - 8).abs() <= 17 && (pos.y - 30).abs() <= 17 && (pos.z - 8).abs() <= 17
        }));
        // A bud or cluster points away from the block it grows on.
        for (pos, state) in &world.writes {
            if let Some(facing) = state.property("facing") {
                let support = match facing {
                    "up" => pos.offset(0, -1, 0),
                    "down" => pos.offset(0, 1, 0),
                    "north" => pos.offset(0, 0, 1),
                    "south" => pos.offset(0, 0, -1),
                    "west" => pos.offset(1, 0, 0),
                    _ => pos.offset(-1, 0, 0),
                };
                assert_eq!(world.get(support), blocks::BUDDING_AMETHYST, "{pos:?}");
            }
        }
    }

    #[test]
    fn a_geode_whose_points_lie_in_air_is_not_placed() {
        let geode = amethyst_geode();
        let mut world = World::flat(blocks::STONE, 0);
        assert!(!world.with_context(7, |context| {
            geode.place(context, BlockPos::new(8, 60, 8))
        }));
        assert!(world.writes.is_empty());
    }

    #[test]
    fn a_geode_does_not_replace_bedrock_and_writes_nothing_beyond_the_three_by_three() {
        let geode = amethyst_geode();
        let mut world = World::flat(blocks::BEDROCK, 100);
        world.set(BlockPos::new(31, 30, 8), blocks::STONE);
        world.with_context(7, |context| geode.place(context, BlockPos::new(30, 30, 8)));
        assert!(world.writes.iter().all(|(pos, _)| pos.x < 32));
        let mut world = World::flat(blocks::STONE, 100);
        world.with_context(7, |context| geode.place(context, BlockPos::new(30, 30, 8)));
        assert!(!world.writes.is_empty());
        assert!(world.writes.iter().all(|(pos, _)| pos.x < 32));
    }

    #[test]
    fn a_crack_into_water_schedules_the_waters_ticks() {
        let geode = amethyst_geode();
        let mut ticked = false;
        for seed in 0..20 {
            let mut world = World::flat(blocks::STONE, 100);
            // Water all round the geode's crack's reach.
            for x in 0..=24 {
                for z in 0..=24 {
                    for y in 30..=40 {
                        if x > 18 || z > 18 {
                            world.set(BlockPos::new(x, y, z), blocks::WATER);
                        }
                    }
                }
            }
            world.with_context(seed, |context| {
                geode.place(context, BlockPos::new(8, 30, 8))
            });
            ticked |= world.ticks.iter().any(|(_, tick)| tick.delay == 0);
        }
        assert!(ticked);
    }
}
