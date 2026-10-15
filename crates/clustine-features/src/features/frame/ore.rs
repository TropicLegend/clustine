// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/ore.rs`.

//! `ore`: a vein of blocks along a line through the origin, each replacing what its
//! targets allow (`OreFeature`).
//!
//! The game writes an ore's blocks straight into the chunks' sections
//! (`BulkSectionAccess`): it reads them back from there, asks only whether the run
//! may write a block's chunk, and neither updates a heightmap nor marks a block for
//! later. The ores replace stone with stone, which leaves the heightmaps as they were,
//! and [`Level::set_block`](crate::context::Level::set_block) marks nothing; so the
//! level's own writes stand for the section's.

use clustine_data::{BlockState, Face};
use clustine_noise::{Random, math, trig};
use clustine_world::BlockPos;

use crate::blocks::{read_state, relative};
use crate::context::{Context, Heightmap, Level};
use crate::feature::Feature;
use crate::predicates::RuleTest;
use crate::reader::{Read, Reader};

/// What an ore replaces with what (`OreConfiguration.TargetBlockState`), and how
/// often a block beside air is left out.
#[derive(Debug)]
pub struct OreConfiguration {
    pub targets: Vec<(RuleTest, BlockState)>,
    pub size: i32,
    pub discard_chance_on_air_exposure: f32,
}

impl OreConfiguration {
    pub fn read(reader: &Reader) -> Read<Self> {
        let mut targets = Vec::new();
        for target in reader.field("targets")?.list()? {
            targets.push((
                RuleTest::read(&target.field("target")?)?,
                read_state(&target.field("state")?)?,
            ));
            target.finish()?;
        }
        let size = reader.field("size")?;
        let value = size.int()?;
        if !(0..=64).contains(&value) {
            return Err(size.error("an ore's size is from 0 to 64"));
        }
        let discard = reader.field("discard_chance_on_air_exposure")?.float()?;
        Ok(Self {
            targets,
            size: value,
            discard_chance_on_air_exposure: discard,
        })
    }

    /// `OreFeature.canPlaceOre` for each target in turn: the state of the first whose
    /// test passes and that is not left out for air beside it.
    pub fn replacement<R: Random + ?Sized>(
        &self,
        level: &mut Level<'_>,
        random: &mut R,
        state: BlockState,
        pos: BlockPos,
    ) -> Option<BlockState> {
        for (test, ore) in &self.targets {
            if !test.test(state, pos.y, random) {
                continue;
            }
            if skip_air_check(random, self.discard_chance_on_air_exposure)
                || !adjacent_to_air(level, pos)
            {
                return Some(*ore);
            }
        }
        None
    }
}

/// `OreFeature.shouldSkipAirCheck`: draws only for a chance strictly between nought
/// and one.
fn skip_air_check<R: Random + ?Sized>(random: &mut R, chance: f32) -> bool {
    if chance <= 0.0 {
        true
    } else if chance >= 1.0 {
        false
    } else {
        random.next_f32() >= chance
    }
}

/// Whether any of the six blocks beside `pos` is air (`isAdjacentToAir`). Beyond the
/// height of the world the game's sections answer air.
fn adjacent_to_air(level: &mut Level<'_>, pos: BlockPos) -> bool {
    Face::ALL
        .into_iter()
        .any(|face| level.block(relative(pos, face)).props().is_air())
}

#[derive(Debug)]
pub struct Ore(OreConfiguration);

impl Ore {
    pub fn read(reader: &Reader) -> Read<Self> {
        Ok(Self(OreConfiguration::read(reader)?))
    }
}

impl Feature for Ore {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let size = self.0.size;
        let direction = context.random.next_f32() * std::f32::consts::PI;
        let spread = size as f32 / 8.0;
        let max_radius = ((size as f32 / 16.0 * 2.0 + 1.0) / 2.0).ceil() as i32;
        // `Math.sin` and `Math.cos` of the angle widened to a double.
        let sin = libm::sin(f64::from(direction));
        let cos = libm::cos(f64::from(direction));
        let spread_wide = f64::from(spread);
        let x0 = f64::from(origin.x) + sin * spread_wide;
        let x1 = f64::from(origin.x) - sin * spread_wide;
        let z0 = f64::from(origin.z) + cos * spread_wide;
        let z1 = f64::from(origin.z) - cos * spread_wide;
        let y0 = f64::from(origin.y + context.random.next_i32_bounded(3) - 2);
        let y1 = f64::from(origin.y + context.random.next_i32_bounded(3) - 2);
        let spread_ceil = spread.ceil() as i32;
        let x_start = origin.x - spread_ceil - max_radius;
        let y_start = origin.y - 2 - max_radius;
        let z_start = origin.z - spread_ceil - max_radius;
        let size_xz = 2 * (spread_ceil + max_radius);
        let size_y = 2 * (2 + max_radius);
        for x in x_start..=x_start + size_xz {
            for z in z_start..=z_start + size_xz {
                if y_start <= context.level.height_at(Heightmap::OceanFloorWg, x, z) {
                    let vein = Vein {
                        from: (x0, y0, z0),
                        to: (x1, y1, z1),
                        start: (x_start, y_start, z_start),
                        size_xz,
                        size_y,
                    };
                    return self.place_vein(context, &vein);
                }
            }
        }
        false
    }
}

/// The line an ore follows and the box it may fill.
struct Vein {
    from: (f64, f64, f64),
    to: (f64, f64, f64),
    start: (i32, i32, i32),
    size_xz: i32,
    size_y: i32,
}

impl Ore {
    /// `OreFeature.doPlace`.
    fn place_vein(&self, context: &mut Context<'_, '_>, vein: &Vein) -> bool {
        let size = self.0.size;
        let count = size as usize;
        let mut spheres = vec![[0.0_f64; 4]; count];
        for (i, sphere) in spheres.iter_mut().enumerate() {
            let step = i as f32 / size as f32;
            let along = f64::from(step);
            let x = math::lerp(along, vein.from.0, vein.to.0);
            let y = math::lerp(along, vein.from.1, vein.to.1);
            let z = math::lerp(along, vein.from.2, vein.to.2);
            let scale = context.random.next_f64() * f64::from(size) / 16.0;
            let wave = trig::sin(f64::from(std::f32::consts::PI * step)) + 1.0;
            let radius = (f64::from(wave) * scale + 1.0) / 2.0;
            *sphere = [x, y, z, radius];
        }
        // A sphere inside another is dropped.
        for i1 in 0..count.saturating_sub(1) {
            if spheres[i1][3] <= 0.0 {
                continue;
            }
            for i2 in i1 + 1..count {
                if spheres[i2][3] <= 0.0 {
                    continue;
                }
                let dx = spheres[i1][0] - spheres[i2][0];
                let dy = spheres[i1][1] - spheres[i2][1];
                let dz = spheres[i1][2] - spheres[i2][2];
                let dr = spheres[i1][3] - spheres[i2][3];
                if dr * dr > dx * dx + dy * dy + dz * dz {
                    if dr > 0.0 {
                        spheres[i2][3] = -1.0;
                    } else {
                        spheres[i1][3] = -1.0;
                    }
                }
            }
        }

        let (x_start, y_start, z_start) = vein.start;
        let mut tested = Bits::default();
        let mut placed = 0;
        for [sx, sy, sz, radius] in spheres {
            if radius < 0.0 {
                continue;
            }
            let x_min = math::floor(sx - radius).max(x_start);
            let y_min = math::floor(sy - radius).max(y_start);
            let z_min = math::floor(sz - radius).max(z_start);
            let x_max = math::floor(sx + radius).max(x_min);
            let y_max = math::floor(sy + radius).max(y_min);
            let z_max = math::floor(sz + radius).max(z_min);
            for x in x_min..=x_max {
                let xd = (f64::from(x) + 0.5 - sx) / radius;
                if xd * xd >= 1.0 {
                    continue;
                }
                for y in y_min..=y_max {
                    let yd = (f64::from(y) + 0.5 - sy) / radius;
                    if xd * xd + yd * yd >= 1.0 {
                        continue;
                    }
                    for z in z_min..=z_max {
                        let zd = (f64::from(z) + 0.5 - sz) / radius;
                        if xd * xd + yd * yd + zd * zd >= 1.0
                            || context.level.is_outside_build_height(y)
                        {
                            continue;
                        }
                        // The game's index into its set of blocks tried; the box is
                        // one longer than its sizes each way, so two blocks can share
                        // an index, and the second is then not tried.
                        let index = i64::from(x - x_start)
                            + i64::from(y - y_start) * i64::from(vein.size_xz)
                            + i64::from(z - z_start)
                                * i64::from(vein.size_xz)
                                * i64::from(vein.size_y);
                        if !tested.insert(index) {
                            continue;
                        }
                        let pos = BlockPos::new(x, y, z);
                        if !context.level.ensure_can_write(pos) {
                            continue;
                        }
                        let state = context.level.block(pos);
                        if let Some(ore) =
                            self.0
                                .replacement(context.level, &mut *context.random, state, pos)
                        {
                            context.level.set_block(pos, ore);
                            placed += 1;
                        }
                    }
                }
            }
        }
        placed > 0
    }
}

/// The game's `BitSet` of blocks tried, which grows as it is set.
#[derive(Default)]
struct Bits(Vec<u64>);

impl Bits {
    /// Sets the bit; false if it was set.
    fn insert(&mut self, index: i64) -> bool {
        let index = usize::try_from(index).expect("an ore's index is not negative");
        let (word, bit) = (index / 64, index % 64);
        if word >= self.0.len() {
            self.0.resize(word + 1, 0);
        }
        let mask = 1_u64 << bit;
        let was = self.0[word] & mask != 0;
        self.0[word] |= mask;
        !was
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use clustine_noise::WorldgenRandom;
    use clustine_worldgen_data::feature as data;

    use super::*;
    use crate::testing::World;

    fn iron() -> Box<dyn Feature> {
        let (_, entry) = data::configured("minecraft:ore_iron").unwrap();
        crate::feature::read_configured(&Reader::new(&entry.data, entry.name)).unwrap()
    }

    #[test]
    fn the_set_of_blocks_tried_keeps_the_game_s_layout() {
        let mut bits = Bits::default();
        assert!(bits.insert(4));
        assert!(!bits.insert(4));
        assert!(bits.insert(130));
        assert!(!bits.insert(130));
        assert!(bits.insert(0));
    }

    #[test]
    fn an_ore_in_stone_replaces_stone_around_its_origin_and_nothing_else() {
        let ore = iron();
        let mut world = World::flat(blocks::STONE, 100);
        let origin = BlockPos::new(8, 40, 8);
        let mut random = WorldgenRandom::from_seed(1234);
        assert!(world.with_random(&mut random, None, |context| ore.place(context, origin)));
        assert!(!world.writes.is_empty());
        for (pos, state) in &world.writes {
            assert_eq!(*state, blocks::IRON_ORE);
            assert!((pos.x - origin.x).abs() <= 8 && (pos.z - origin.z).abs() <= 8);
            assert!((pos.y - origin.y).abs() <= 6, "{pos:?}");
        }
    }

    #[test]
    fn an_ore_below_zero_turns_to_deepslate_ore_in_deepslate() {
        let ore = iron();
        let mut world = World::flat(blocks::DEEPSLATE, 100);
        let mut random = WorldgenRandom::from_seed(77);
        assert!(world.with_random(&mut random, None, |context| {
            ore.place(context, BlockPos::new(8, -20, 8))
        }));
        assert!(
            world
                .writes
                .iter()
                .all(|(_, state)| *state == blocks::DEEPSLATE_IRON_ORE)
        );
    }

    #[test]
    fn an_ore_above_the_ground_s_floor_is_not_placed_and_draws_three_numbers() {
        // Every column of the box has its ocean floor below the box: nothing is placed.
        let ore = iron();
        let mut world = World::flat(blocks::STONE, 10);
        let mut random = WorldgenRandom::from_seed(5);
        let mut expected = random.clone();
        assert!(!world.with_random(&mut random, None, |context| {
            ore.place(context, BlockPos::new(8, 100, 8))
        }));
        assert!(world.writes.is_empty());
        let _ = expected.next_f32();
        let _ = expected.next_i32_bounded(3);
        let _ = expected.next_i32_bounded(3);
        assert_eq!(random.next_i64(), expected.next_i64());
    }

    #[test]
    fn a_block_beside_air_is_left_out_with_the_discard_chance() {
        let config = OreConfiguration {
            targets: vec![(RuleTest::AlwaysTrue, blocks::GOLD_ORE)],
            size: 4,
            discard_chance_on_air_exposure: 1.0,
        };
        let mut world = World::flat(blocks::STONE, 10);
        let mut random = WorldgenRandom::from_seed(0);
        world.with_level(|level| {
            assert_eq!(
                config.replacement(level, &mut random, blocks::STONE, BlockPos::new(0, 10, 0)),
                None
            );
            assert_eq!(
                config.replacement(level, &mut random, blocks::STONE, BlockPos::new(0, 5, 0)),
                Some(blocks::GOLD_ORE)
            );
        });
    }

    #[test]
    fn the_writes_stop_at_the_edge_of_the_three_by_three() {
        let ore = iron();
        let mut world = World::flat(blocks::STONE, 100);
        let mut random = WorldgenRandom::from_seed(9);
        world.with_random(&mut random, None, |context| {
            ore.place(context, BlockPos::new(31, 40, 31))
        });
        assert!(world.writes.iter().all(|(pos, _)| pos.x < 32 && pos.z < 32));
    }
}
