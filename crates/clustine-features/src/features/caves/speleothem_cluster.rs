// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/speleothem.rs`.

//! `speleothem_cluster`: a field of stalactites and stalagmites in a cave, the ground
//! and ceiling under them turned to the base block, with small pools of water among
//! them where it is wet (`SpeleothemClusterFeature`, 26.x's `DripstoneClusterFeature`
//! made general: `dripstone_cluster` and `sulfur_spike_cluster`).

use clustine_data::{Face, blocks};
use clustine_noise::Random;
use clustine_world::BlockPos;

use super::shared::{
    Column, SpeleothemBlocks, at_y, clamped_map, is_base_stone_overworld, is_empty_or_water,
    is_lava, is_neither_empty_nor_water, random_between_biased,
};
use crate::blocks::{HORIZONTAL, has_water, relative, same_block};
use crate::context::{Context, Level};
use crate::feature::Feature;
use crate::reader::{Read, Reader};
use crate::values::{FloatProvider, IntProvider};

#[derive(Debug)]
pub struct SpeleothemCluster {
    blocks: SpeleothemBlocks,
    floor_to_ceiling_search_range: i32,
    height: IntProvider,
    radius: IntProvider,
    max_stalagmite_stalactite_height_diff: i32,
    height_deviation: i32,
    layer_thickness: IntProvider,
    density: FloatProvider,
    wetness: FloatProvider,
    chance_at_max_distance_from_center: f32,
    max_distance_from_edge_affecting_chance: i32,
    max_distance_from_center_affecting_height_bias: i32,
}

/// What one cluster drew for all its columns.
struct Drawn {
    height: i32,
    wetness: f32,
    density: f32,
}

impl SpeleothemCluster {
    pub fn read(reader: &Reader) -> Read<Self> {
        let int = |key: &str| reader.field(key)?.int();
        let cluster = Self {
            blocks: SpeleothemBlocks::read(reader)?,
            floor_to_ceiling_search_range: int("floor_to_ceiling_search_range")?,
            height: IntProvider::read(&reader.field("height")?)?,
            radius: IntProvider::read(&reader.field("radius")?)?,
            max_stalagmite_stalactite_height_diff: int("max_stalagmite_stalactite_height_diff")?,
            height_deviation: int("height_deviation")?,
            layer_thickness: IntProvider::read(&reader.field("speleothem_block_layer_thickness")?)?,
            density: FloatProvider::read(&reader.field("density")?)?,
            wetness: FloatProvider::read(&reader.field("wetness")?)?,
            chance_at_max_distance_from_center: reader
                .field("chance_of_speleothem_at_max_distance_from_center")?
                .float()?,
            max_distance_from_edge_affecting_chance: int(
                "max_distance_from_edge_affecting_chance_of_speleothem",
            )?,
            max_distance_from_center_affecting_height_bias: int(
                "max_distance_from_center_affecting_height_bias",
            )?,
        };
        reader.finish()?;
        Ok(cluster)
    }

    /// `getChanceOfStalagmiteOrStalactite`: less towards the edges of the cluster.
    fn chance(&self, x_radius: i32, z_radius: i32, dx: i32, dz: i32) -> f64 {
        let from_edge = (x_radius - dx.abs()).min(z_radius - dz.abs());
        f64::from(clamped_map(
            from_edge as f32,
            0.0,
            self.max_distance_from_edge_affecting_chance as f32,
            self.chance_at_max_distance_from_center,
            1.0,
        ))
    }

    /// `getSpeleothemHeight`: none where the density says so, else a normal draw about
    /// a mean that falls off from the centre.
    fn column_height<R: Random + ?Sized>(
        &self,
        random: &mut R,
        dx: i32,
        dz: i32,
        density: f32,
        max_height: i32,
    ) -> i32 {
        if random.next_f32() > density {
            return 0;
        }
        let from_centre = dx.abs() + dz.abs();
        let mean = clamped_map(
            from_centre as f32,
            0.0,
            self.max_distance_from_center_affecting_height_bias as f32,
            max_height as f32 / 2.0,
            0.0,
        );
        random_between_biased(
            random,
            0.0,
            max_height as f32,
            mean,
            self.height_deviation as f32,
        ) as i32
    }

    /// `canPlacePool`: water may lie at `pos` in stone or beside water, not under it.
    fn can_place_pool(&self, level: &mut Level<'_>, pos: BlockPos) -> bool {
        let state = level.block(pos);
        if same_block(state, blocks::WATER)
            || same_block(state, self.blocks.base)
            || same_block(state, self.blocks.pointed)
        {
            return false;
        }
        if has_water(level.block(pos.offset(0, 1, 0))) {
            return false;
        }
        for direction in HORIZONTAL {
            if !can_be_beside_pool(level, relative(pos, direction)) {
                return false;
            }
        }
        can_be_beside_pool(level, pos.offset(0, -1, 0))
    }

    /// `placeColumn`.
    fn place_column(
        &self,
        context: &mut Context<'_, '_>,
        pos: BlockPos,
        dx: i32,
        dz: i32,
        chance: f64,
        drawn: &Drawn,
    ) {
        let Some(mut column) = Column::scan(
            context.level,
            pos,
            self.floor_to_ceiling_search_range,
            is_empty_or_water,
            is_neither_empty_nor_water,
        ) else {
            return;
        };
        let ceiling = column.ceiling;
        if ceiling.is_none() && column.floor.is_none() {
            return;
        }

        let want_pool = context.random.next_f32() < drawn.wetness;
        if let Some(floor) = column.floor
            && want_pool
            && self.can_place_pool(context.level, at_y(pos, floor))
        {
            context.level.set_block(at_y(pos, floor), blocks::WATER);
            column.floor = Some(floor - 1);
        }
        let floor = column.floor;

        let want_stalactite = context.random.next_f64() < chance;
        let stalactite_height = match ceiling {
            Some(ceiling) if want_stalactite && !is_lava(context.level, at_y(pos, ceiling)) => {
                let thickness = self.layer_thickness.sample(&mut *context.random);
                self.blocks
                    .replace_with_base(context.level, at_y(pos, ceiling), thickness, 1);
                let max_height = match floor {
                    Some(floor) => drawn.height.min(ceiling - floor),
                    None => drawn.height,
                };
                self.column_height(&mut *context.random, dx, dz, drawn.density, max_height)
            }
            _ => 0,
        };

        let want_stalagmite = context.random.next_f64() < chance;
        let stalagmite_height = match floor {
            Some(floor) if want_stalagmite && !is_lava(context.level, at_y(pos, floor)) => {
                let thickness = self.layer_thickness.sample(&mut *context.random);
                self.blocks
                    .replace_with_base(context.level, at_y(pos, floor), thickness, -1);
                if ceiling.is_some() {
                    let diff = self.max_stalagmite_stalactite_height_diff;
                    (stalactite_height + context.random.next_i32_between(-diff, diff)).max(0)
                } else {
                    self.column_height(&mut *context.random, dx, dz, drawn.density, drawn.height)
                }
            }
            _ => 0,
        };

        // Where the two would meet or pass, they are cut to meet at a height drawn
        // between them.
        let (stalactite_height, stalagmite_height) = match (ceiling, floor) {
            (Some(ceiling), Some(floor))
                if ceiling - stalactite_height <= floor + stalagmite_height =>
            {
                let lowest_bottom = (ceiling - stalactite_height).max(floor + 1);
                let highest_top = (floor + stalagmite_height).min(ceiling - 1);
                let bottom = context
                    .random
                    .next_i32_between(lowest_bottom, highest_top + 1);
                (ceiling - bottom, bottom - 1 - floor)
            }
            _ => (stalactite_height, stalagmite_height),
        };

        let column = Column {
            floor,
            ceiling: column.ceiling,
        };
        let merge_tips = context.random.next_bool()
            && stalactite_height > 0
            && stalagmite_height > 0
            && column.height() == Some(stalactite_height + stalagmite_height);

        if let Some(ceiling) = ceiling {
            self.blocks.grow(
                context.level,
                at_y(pos, ceiling - 1),
                Face::Down,
                stalactite_height,
                merge_tips,
            );
        }
        if let Some(floor) = floor {
            self.blocks.grow(
                context.level,
                at_y(pos, floor + 1),
                Face::Up,
                stalagmite_height,
                merge_tips,
            );
        }
    }
}

/// `canBeAdjacentToWater`: stone, or water.
fn can_be_beside_pool(level: &mut Level<'_>, pos: BlockPos) -> bool {
    let state = level.block(pos);
    is_base_stone_overworld(state) || has_water(state)
}

impl Feature for SpeleothemCluster {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        if !is_empty_or_water(context.level.block(origin)) {
            return false;
        }
        let random = &mut *context.random;
        let drawn = Drawn {
            height: self.height.sample(random),
            wetness: self.wetness.sample(random),
            density: self.density.sample(random),
        };
        let x_radius = self.radius.sample(random);
        let z_radius = self.radius.sample(random);
        for dx in -x_radius..=x_radius {
            for dz in -z_radius..=z_radius {
                let chance = self.chance(x_radius, z_radius, dx, dz);
                let pos = origin.offset(dx, 0, dz);
                self.place_column(context, pos, dx, dz, chance, &drawn);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use clustine_worldgen_data::feature as data;

    use super::*;
    use crate::testing::World;

    fn cluster(name: &str) -> Box<dyn Feature> {
        let (_, entry) = data::configured(name).unwrap();
        crate::feature::read_configured(&Reader::new(&entry.data, entry.name)).unwrap()
    }

    /// `ground` with a cave from y 50 to y 59 under the origin's chunk.
    fn cave(ground: clustine_data::BlockState) -> World {
        let mut world = World::flat(ground, 80);
        for x in -12..=12 {
            for z in -12..=12 {
                for y in 50..60 {
                    world.set(BlockPos::new(x, y, z), blocks::AIR);
                }
            }
        }
        world
    }

    #[test]
    fn a_dripstone_cluster_grows_stalactites_and_stalagmites_in_a_cave() {
        let feature = cluster("minecraft:dripstone_cluster");
        let mut grown = 0;
        for seed in 0..6 {
            let mut world = cave(blocks::STONE);
            assert!(world.with_context(seed, |context| {
                feature.place(context, BlockPos::new(0, 55, 0))
            }));
            let pointed = world
                .writes
                .iter()
                .filter(|(_, state)| same_block(*state, blocks::POINTED_DRIPSTONE))
                .count();
            grown += pointed;
            // Every pointed dripstone stands in the cave, or in a pool sunk into its
            // floor.
            for (pos, state) in &world.writes {
                if same_block(*state, blocks::POINTED_DRIPSTONE) {
                    assert!((49..60).contains(&pos.y), "{pos:?}");
                }
            }
            // Only dripstone, water and pointed dripstone are written.
            assert!(world.writes.iter().all(|(_, state)| {
                same_block(*state, blocks::POINTED_DRIPSTONE)
                    || *state == blocks::DRIPSTONE_BLOCK
                    || *state == blocks::WATER
            }));
        }
        assert!(grown > 0);
    }

    #[test]
    fn a_sulfur_cluster_is_dry_and_grows_sulfur_spikes() {
        let feature = cluster("minecraft:sulfur_spike_cluster");
        // Sulfur grows only from sulfur and cinnabar.
        let mut world = cave(blocks::SULFUR);
        assert!(world.with_context(5, |context| {
            feature.place(context, BlockPos::new(0, 55, 0))
        }));
        assert!(
            world
                .writes
                .iter()
                .all(|(_, state)| *state != blocks::WATER)
        );
        assert!(
            world
                .writes
                .iter()
                .any(|(_, state)| same_block(*state, blocks::SULFUR_SPIKE))
        );
    }

    #[test]
    fn a_cluster_in_stone_places_nothing() {
        let feature = cluster("minecraft:dripstone_cluster");
        let mut world = World::flat(blocks::STONE, 80);
        assert!(!world.with_context(1, |context| {
            feature.place(context, BlockPos::new(0, 55, 0))
        }));
        assert!(world.writes.is_empty());
    }
}
