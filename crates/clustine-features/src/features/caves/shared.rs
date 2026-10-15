// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/speleothem.rs` and
// `steel-core/src/worldgen/feature/features/dripstone/cluster.rs`.

//! What the cave features share: the game's `Column` of air between a floor and a
//! ceiling, the speleothems' blocks (26.x made pointed dripstone one of a family with
//! sulfur spikes, `SpeleothemUtils`), the bounds of an int provider and `Mth`'s
//! mapping between ranges.

use clustine_data::{BlockState, Face, blocks};
use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::blocks::{BlockSet, block_default, has_water, in_block_tag, relative, same_block};
use crate::context::Level;
use crate::reader::{Read, Reader};
use crate::values::IntProvider;

/// `isEmptyOrWater`: air of any kind, or the water block.
pub fn is_empty_or_water(state: BlockState) -> bool {
    state.props().is_air() || same_block(state, blocks::WATER)
}

/// `isEmptyOrWaterOrLava`.
pub fn is_empty_or_water_or_lava(state: BlockState) -> bool {
    is_empty_or_water(state) || same_block(state, blocks::LAVA)
}

/// `isNeitherEmptyNorWater`.
pub fn is_neither_empty_nor_water(state: BlockState) -> bool {
    !is_empty_or_water(state)
}

/// Whether the block at `pos` is lava.
pub fn is_lava(level: &mut Level<'_>, pos: BlockPos) -> bool {
    same_block(level.block(pos), blocks::LAVA)
}

/// `BlockTags.BASE_STONE_OVERWORLD`, where a large dripstone stops once it has left the
/// stone, and what a speleothem cluster's pool may lie in.
pub fn is_base_stone_overworld(state: BlockState) -> bool {
    in_block_tag(state, "minecraft:base_stone_overworld")
}

/// The game's `Column`: where the air (or what `inside` accepts) around a position
/// ends above and below. Each side is the y of the edge block, where `edge` accepts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Column {
    pub floor: Option<i32>,
    pub ceiling: Option<i32>,
}

impl Column {
    /// `Column.scan`: none where `pos` itself is not inside. Each direction looks up to
    /// `search_range - 1` blocks on.
    pub fn scan(
        level: &mut Level<'_>,
        pos: BlockPos,
        search_range: i32,
        inside: impl Fn(BlockState) -> bool,
        edge: impl Fn(BlockState) -> bool,
    ) -> Option<Self> {
        if !inside(level.block(pos)) {
            return None;
        }
        let ceiling = scan_direction(level, pos, search_range, &inside, &edge, 1);
        let floor = scan_direction(level, pos, search_range, &inside, &edge, -1);
        Some(Self { floor, ceiling })
    }

    /// `Column.Range.height`: the blocks between the floor and the ceiling.
    pub fn height(self) -> Option<i32> {
        Some(self.ceiling? - self.floor? - 1)
    }
}

fn scan_direction(
    level: &mut Level<'_>,
    mut pos: BlockPos,
    search_range: i32,
    inside: &impl Fn(BlockState) -> bool,
    edge: &impl Fn(BlockState) -> bool,
    dy: i32,
) -> Option<i32> {
    let mut i = 1;
    while i < search_range && inside(level.block(pos)) {
        pos = pos.offset(0, dy, 0);
        i += 1;
    }
    edge(level.block(pos)).then_some(pos.y)
}

/// `pos` at another height (`atY`).
pub fn at_y(pos: BlockPos, y: i32) -> BlockPos {
    BlockPos::new(pos.x, y, pos.z)
}

/// `Mth.clampedMap` in single precision.
pub fn clamped_map(value: f32, from_min: f32, from_max: f32, to_min: f32, to_max: f32) -> f32 {
    let factor = (value - from_min) / (from_max - from_min);
    if factor < 0.0 {
        to_min
    } else if factor > 1.0 {
        to_max
    } else {
        to_min + factor * (to_max - to_min)
    }
}

/// `Mth.clamp` of ints, which does not ask that `min` be below `max`.
pub fn clamp(value: i32, min: i32, max: i32) -> i32 {
    value.max(min).min(max)
}

/// `IntProvider.getMinValue`.
pub fn int_min(provider: &IntProvider) -> i32 {
    match provider {
        IntProvider::Constant(value) => *value,
        IntProvider::Uniform { min, .. }
        | IntProvider::BiasedToBottom { min, .. }
        | IntProvider::VeryBiasedToBottom { min, .. }
        | IntProvider::Trapezoid { min, .. }
        | IntProvider::ClampedNormal { min, .. } => *min,
        IntProvider::Clamped { source, min, .. } => (*min).max(int_min(source)),
        IntProvider::WeightedList { values, .. } => {
            values.iter().map(int_min).min().unwrap_or(i32::MAX)
        }
    }
}

/// `IntProvider.getMaxValue`.
pub fn int_max(provider: &IntProvider) -> i32 {
    match provider {
        IntProvider::Constant(value) => *value,
        IntProvider::Uniform { max, .. }
        | IntProvider::BiasedToBottom { max, .. }
        | IntProvider::VeryBiasedToBottom { max, .. }
        | IntProvider::Trapezoid { max, .. }
        | IntProvider::ClampedNormal { max, .. } => *max,
        IntProvider::Clamped { source, max, .. } => (*max).min(int_max(source)),
        IntProvider::WeightedList { values, .. } => {
            values.iter().map(int_max).max().unwrap_or(i32::MIN)
        }
    }
}

/// `Mth.randomBetween`: a float from `min` up to `max`.
pub fn random_between<R: Random + ?Sized>(random: &mut R, min: f32, max: f32) -> f32 {
    random.next_f32() * (max - min) + min
}

/// `SpeleothemUtils.randomBetweenBiased`: a normal draw about `mean`, clamped.
pub fn random_between_biased<R: Random + ?Sized>(
    random: &mut R,
    min: f32,
    max: f32,
    mean: f32,
    deviation: f32,
) -> f32 {
    let sample = mean + deviation * random.next_gaussian() as f32;
    sample.clamp(min, max)
}

/// The six directions in the game's order (`Direction.values()`).
pub const DIRECTIONS: [Face; 6] = Face::ALL;

/// `Direction.getRandom`.
pub fn random_direction<R: Random + ?Sized>(random: &mut R) -> Face {
    DIRECTIONS[random.next_i32_bounded(6) as usize]
}

/// A speleothem's thickness (`SpeleothemThickness`), from its tip to its base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thickness {
    TipMerge,
    Tip,
    Frustum,
    Middle,
    Base,
}

impl Thickness {
    fn name(self) -> &'static str {
        match self {
            Self::TipMerge => "tip_merge",
            Self::Tip => "tip",
            Self::Frustum => "frustum",
            Self::Middle => "middle",
            Self::Base => "base",
        }
    }
}

/// The blocks of one family of speleothems: the block they grow from, the pointed
/// block itself, and what the base block may replace.
#[derive(Debug, Clone)]
pub struct SpeleothemBlocks {
    pub base: BlockState,
    pub pointed: BlockState,
    pub replaceable: BlockSet,
}

impl SpeleothemBlocks {
    /// Reads `base_block`, `pointed_block` and `replaceable_blocks`: two blocks by name
    /// and a set of blocks.
    pub fn read(reader: &Reader) -> Read<Self> {
        let block = |key: &str| -> Read<BlockState> {
            let field = reader.field(key)?;
            let name = field.text()?;
            block_default(name).ok_or_else(|| field.error(format!("{name} is not a block")))
        };
        let blocks = Self {
            base: block("base_block")?,
            pointed: block("pointed_block")?,
            replaceable: BlockSet::read(&reader.field("replaceable_blocks")?)?,
        };
        if blocks.pointed.property("thickness").is_none()
            || blocks.pointed.property("vertical_direction").is_none()
        {
            return Err(reader.error("the pointed block has no thickness and direction"));
        }
        Ok(blocks)
    }

    /// `isBase`: the base block itself, or one it may replace.
    pub fn is_base(&self, state: BlockState) -> bool {
        same_block(state, self.base) || self.replaceable.contains(state)
    }

    /// `placeBaseBlockIfPossible`: the base block where it may replace what is there.
    pub fn place_base_if_possible(&self, level: &mut Level<'_>, pos: BlockPos) -> bool {
        if !self.replaceable.contains(level.block(pos)) {
            return false;
        }
        level.set_block(pos, self.base)
    }

    /// `replaceBlocksWithBaseBlocks`: up to `count` base blocks from `pos` on, stopping
    /// at the first that cannot be placed.
    pub fn replace_with_base(&self, level: &mut Level<'_>, pos: BlockPos, count: i32, dy: i32) {
        let mut pos = pos;
        for _ in 0..count {
            if !self.place_base_if_possible(level, pos) {
                return;
            }
            pos = pos.offset(0, dy, 0);
        }
    }

    /// The pointed block pointing `direction` with `thickness`.
    pub fn pointed(&self, direction: Face, thickness: Thickness) -> BlockState {
        let direction = if direction == Face::Up { "up" } else { "down" };
        self.pointed
            .with_property("vertical_direction", direction)
            .and_then(|state| state.with_property("thickness", thickness.name()))
            .expect("the pointed block was read with a thickness and a direction")
    }

    /// `growSpeleothem`: a speleothem of `height` blocks from `start` towards its tip,
    /// where the block behind `start` is a base. Each block is waterlogged where it
    /// replaces water.
    pub fn grow(
        &self,
        level: &mut Level<'_>,
        start: BlockPos,
        tip: Face,
        height: i32,
        merged_tip: bool,
    ) {
        if !self.is_base(level.block(relative(start, tip.opposite()))) {
            return;
        }
        let mut pos = start;
        for state in self.column(tip, height, merged_tip) {
            let waterlogged = has_water(level.block(pos));
            let state = state
                .with_property("waterlogged", if waterlogged { "true" } else { "false" })
                .unwrap_or(state);
            level.set_block(pos, state);
            pos = relative(pos, tip);
        }
    }

    /// `buildBaseToTipColumn`: the states from the base to the tip.
    fn column(&self, direction: Face, length: i32, merged_tip: bool) -> Vec<BlockState> {
        let mut states = Vec::new();
        if length >= 3 {
            states.push(self.pointed(direction, Thickness::Base));
            for _ in 0..length - 3 {
                states.push(self.pointed(direction, Thickness::Middle));
            }
        }
        if length >= 2 {
            states.push(self.pointed(direction, Thickness::Frustum));
        }
        if length >= 1 {
            let tip = if merged_tip {
                Thickness::TipMerge
            } else {
                Thickness::Tip
            };
            states.push(self.pointed(direction, tip));
        }
        states
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::World;

    fn dripstone() -> SpeleothemBlocks {
        SpeleothemBlocks {
            base: blocks::DRIPSTONE_BLOCK,
            pointed: blocks::POINTED_DRIPSTONE,
            replaceable: BlockSet::of(&["minecraft:stone"]),
        }
    }

    #[test]
    fn a_column_of_four_runs_from_its_base_to_its_tip() {
        let blocks = dripstone();
        let names: Vec<_> = blocks
            .column(Face::Down, 4, false)
            .iter()
            .map(|state| state.property("thickness").unwrap())
            .collect();
        assert_eq!(names, ["base", "middle", "frustum", "tip"]);
        let names: Vec<_> = blocks
            .column(Face::Up, 1, true)
            .iter()
            .map(|state| state.property("thickness").unwrap())
            .collect();
        assert_eq!(names, ["tip_merge"]);
    }

    #[test]
    fn a_stalactite_grows_down_from_stone_and_is_waterlogged_in_water() {
        let blocks = dripstone();
        let mut world = World::flat(blocks::STONE, 64);
        world.set(BlockPos::new(0, 63, 0), blocks::AIR);
        world.set(BlockPos::new(0, 62, 0), blocks::WATER);
        world.set(BlockPos::new(0, 61, 0), blocks::AIR);
        world.with_level(|level| blocks.grow(level, BlockPos::new(0, 63, 0), Face::Down, 2, false));
        assert_eq!(world.writes.len(), 2);
        let (pos, state) = world.writes[1];
        assert_eq!(pos, BlockPos::new(0, 62, 0));
        assert_eq!(state.property("waterlogged"), Some("true"));
        assert_eq!(state.property("vertical_direction"), Some("down"));
        assert_eq!(world.writes[0].1.property("waterlogged"), Some("false"));
    }

    #[test]
    fn nothing_grows_from_a_block_that_is_not_a_base() {
        let blocks = dripstone();
        let mut world = World::flat(blocks::DIRT, 64);
        world.set(BlockPos::new(0, 63, 0), blocks::AIR);
        world.with_level(|level| blocks.grow(level, BlockPos::new(0, 63, 0), Face::Down, 2, false));
        assert!(world.writes.is_empty());
    }

    #[test]
    fn a_column_is_scanned_to_its_floor_and_ceiling_within_the_range() {
        let mut world = World::flat(blocks::STONE, 80);
        for y in 60..=64 {
            world.set(BlockPos::new(0, y, 0), blocks::AIR);
        }
        let column = world.with_level(|level| {
            Column::scan(
                level,
                BlockPos::new(0, 62, 0),
                12,
                is_empty_or_water,
                is_neither_empty_nor_water,
            )
        });
        assert_eq!(
            column,
            Some(Column {
                floor: Some(59),
                ceiling: Some(65)
            })
        );
        assert_eq!(column.unwrap().height(), Some(5));
        // With a range of 2 a side looks one block on, which is still air.
        let short = world.with_level(|level| {
            Column::scan(
                level,
                BlockPos::new(0, 62, 0),
                2,
                is_empty_or_water,
                is_neither_empty_nor_water,
            )
        });
        assert_eq!(
            short,
            Some(Column {
                floor: None,
                ceiling: None
            })
        );
    }

    #[test]
    fn the_bounds_of_a_clamped_provider_are_those_of_the_clamp_and_its_source() {
        let provider = IntProvider::Clamped {
            source: Box::new(IntProvider::Uniform { min: 3, max: 19 }),
            min: 3,
            max: 16,
        };
        assert_eq!((int_min(&provider), int_max(&provider)), (3, 16));
    }

    #[test]
    fn a_clamped_map_holds_at_its_ends() {
        assert_eq!(clamped_map(-1.0, 0.0, 3.0, 0.1, 1.0), 0.1);
        assert_eq!(clamped_map(5.0, 0.0, 3.0, 0.1, 1.0), 1.0);
        assert_eq!(clamped_map(1.5, 0.0, 3.0, 0.0, 1.0), 0.5);
    }
}
