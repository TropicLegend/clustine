// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/tree/root_system.rs`.

//! `root_system` (`RootSystemFeature`): a tree on the surface above a cave, a column
//! of rooted dirt down to the cave, and roots hanging into it.

use clustine_data::{BlockClass, Face, SupportType};
use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::blocks::{BlockSet, fluid_in_tag, is_class, relative_by};
use crate::context::{Context, Heightmap};
use crate::feature::{Feature, PlacedRef};
use crate::predicates::BlockPredicate;
use crate::providers::Provider;
use crate::reader::{Read, Reader};
use crate::survive::{can_survive, sturdy};

#[derive(Debug)]
pub struct RootSystem {
    feature: PlacedRef,
    required_vertical_space_for_tree: i32,
    level_test_distance: i32,
    max_level_deviation: i32,
    root_radius: i32,
    root_placement_attempts: i32,
    root_column_max_height: i32,
    hanging_root_radius: i32,
    hanging_roots_vertical_span: i32,
    hanging_root_placement_attempts: i32,
    allowed_vertical_water_for_tree: i32,
    root_state_provider: Provider,
    hanging_root_state_provider: Provider,
    root_replaceable: BlockSet,
    allowed_tree_position: BlockPredicate,
}

impl RootSystem {
    pub fn read(reader: &Reader) -> Read<Self> {
        let int = |key: &str| reader.field(key)?.int();
        let int_or = |key: &str, default: i32| -> Read<i32> {
            match reader.optional(key) {
                Some(value) => value.int(),
                None => Ok(default),
            }
        };
        let system = Self {
            feature: PlacedRef::read(&reader.field("feature")?)?,
            required_vertical_space_for_tree: int("required_vertical_space_for_tree")?,
            level_test_distance: int_or("level_test_distance", 0)?,
            max_level_deviation: int_or("max_level_deviation", 0)?,
            root_radius: int("root_radius")?,
            root_placement_attempts: int("root_placement_attempts")?,
            root_column_max_height: int("root_column_max_height")?,
            hanging_root_radius: int("hanging_root_radius")?,
            hanging_roots_vertical_span: int("hanging_roots_vertical_span")?,
            hanging_root_placement_attempts: int("hanging_root_placement_attempts")?,
            allowed_vertical_water_for_tree: int("allowed_vertical_water_for_tree")?,
            root_state_provider: Provider::read(&reader.field("root_state_provider")?)?,
            hanging_root_state_provider: Provider::read(
                &reader.field("hanging_root_state_provider")?,
            )?,
            root_replaceable: BlockSet::read(&reader.field("root_replaceable")?)?,
            allowed_tree_position: BlockPredicate::read(&reader.field("allowed_tree_position")?)?,
        };
        reader.finish()?;
        Ok(system)
    }

    /// `placeDirtAndTree`: up the column to the first place the tree may stand, the
    /// tree there, and the rooted dirt below it if it was placed.
    fn dirt_and_tree(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let mut pos = origin;
        for y in 0..self.root_column_max_height {
            pos = pos.offset(0, 1, 0);
            if context
                .level
                .height_at(Heightmap::WorldSurfaceWg, pos.x, pos.z)
                < pos.y
            {
                return false;
            }
            if !self.allowed_tree_position.test(context, pos) || !self.space_for_tree(context, pos)
            {
                continue;
            }
            let below = context.level.block(pos.offset(0, -1, 0));
            if fluid_in_tag(below, "minecraft:lava") || !below.props().is_solid() {
                return false;
            }
            if self.feature.place(context, pos) {
                for dirt_y in origin.y..origin.y + y {
                    self.rooted_dirt(context, BlockPos::new(origin.x, dirt_y, origin.z));
                }
                return true;
            }
        }
        false
    }

    /// `spaceForTree`: air above, or water low enough, and level ground around where
    /// that is asked.
    fn space_for_tree(&self, context: &mut Context<'_, '_>, pos: BlockPos) -> bool {
        let mut column = pos;
        for above_origin in 1..=self.required_vertical_space_for_tree {
            column = column.offset(0, 1, 0);
            let state = context.level.block(column);
            // Water is allowed up to that many blocks above the ground, which lies one
            // below the origin.
            let above_ground = above_origin + 1;
            let allowed = state.props().is_air()
                || above_ground <= self.allowed_vertical_water_for_tree
                    && fluid_in_tag(state, "minecraft:water");
            if !allowed {
                return false;
            }
        }
        if self.level_test_distance > 0 {
            for face in [Face::South, Face::West, Face::North, Face::East] {
                let corner = relative_by(pos, face, self.level_test_distance);
                let below = context
                    .level
                    .block(corner.offset(0, -self.max_level_deviation, 0));
                let above = context
                    .level
                    .block(corner.offset(0, self.max_level_deviation, 0));
                if below.props().is_air() || !above.props().is_air() {
                    return false;
                }
            }
        }
        true
    }

    /// `placeRootedDirt`: tries around the column at one height, each from the column.
    fn rooted_dirt(&self, context: &mut Context<'_, '_>, column: BlockPos) {
        let radius = self.root_radius;
        for _ in 0..self.root_placement_attempts {
            let random = &mut *context.random;
            let x = random.next_i32_bounded(radius) - random.next_i32_bounded(radius);
            let z = random.next_i32_bounded(radius) - random.next_i32_bounded(radius);
            let pos = column.offset(x, 0, z);
            if self.root_replaceable.contains(context.level.block(pos)) {
                let state = self.root_state_provider.state(context, pos);
                context.level.set_block(pos, state);
            }
        }
    }

    /// `placeRoots`: hanging roots in air under a sturdy block.
    fn hanging_roots(&self, context: &mut Context<'_, '_>, origin: BlockPos) {
        let (radius, span) = (self.hanging_root_radius, self.hanging_roots_vertical_span);
        for _ in 0..self.hanging_root_placement_attempts {
            let random = &mut *context.random;
            let x = random.next_i32_bounded(radius) - random.next_i32_bounded(radius);
            let y = random.next_i32_bounded(span) - random.next_i32_bounded(span);
            let z = random.next_i32_bounded(radius) - random.next_i32_bounded(radius);
            let pos = origin.offset(x, y, z);
            if !context.level.block(pos).props().is_air() {
                continue;
            }
            let state = self.hanging_root_state_provider.state(context, pos);
            let above = context.level.block(pos.offset(0, 1, 0));
            let held = sturdy(above, Face::Down, SupportType::Full);
            // Hanging roots stay under a sturdy bottom, the test that follows.
            let survives = if is_class(state, BlockClass::HangingRootsBlock) {
                held
            } else {
                can_survive(state, pos, context.level)
            };
            if survives && held {
                context.level.set_block(pos, state);
            }
        }
    }
}

impl Feature for RootSystem {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        if !context.level.block(origin).props().is_air() {
            return false;
        }
        if self.dirt_and_tree(context, origin) {
            self.hanging_roots(context, origin);
        }
        true
    }
}
