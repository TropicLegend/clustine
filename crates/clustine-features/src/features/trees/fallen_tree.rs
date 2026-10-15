// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/tree/fallen.rs`.

//! `fallen_tree` (`FallenTreeFeature`): a stump, and a log lying beside it where the
//! ground holds it, each with its decorators.

use clustine_data::{Face, SupportType};
use clustine_noise::Random;
use clustine_world::BlockPos;

use super::common::{Axis, random_horizontal, sorted_by_y, valid_tree_pos, with_axis};
use super::decorators::{Decorating, Decorator, decorate};
use crate::blocks::{relative, relative_by};
use crate::context::{Context, Level};
use crate::feature::Feature;
use crate::java_set::JavaSet;
use crate::providers::Provider;
use crate::reader::{Read, Reader};
use crate::survive::sturdy;
use crate::values::IntProvider;

#[derive(Debug)]
pub struct FallenTree {
    trunk: Provider,
    log_length: IntProvider,
    stump_decorators: Vec<Decorator>,
    log_decorators: Vec<Decorator>,
}

fn read_decorators(reader: &Reader, key: &str) -> Read<Vec<Decorator>> {
    match reader.optional(key) {
        Some(list) => list.list()?.iter().map(Decorator::read).collect(),
        None => Ok(Vec::new()),
    }
}

impl FallenTree {
    pub fn read(reader: &Reader) -> Read<Self> {
        let tree = Self {
            trunk: Provider::read(&reader.field("trunk_provider")?)?,
            log_length: IntProvider::read(&reader.field("log_length")?)?,
            stump_decorators: read_decorators(reader, "stump_decorators")?,
            log_decorators: read_decorators(reader, "log_decorators")?,
        };
        reader.finish()?;
        Ok(tree)
    }

    /// `placeLogBlock`: a log, lying along `axis` if one is given, and what stands on
    /// it marked to take its shape later.
    fn log(&self, context: &mut Context<'_, '_>, pos: BlockPos, axis: Option<Axis>) -> BlockPos {
        let mut state = self.trunk.state(context, pos);
        if let Some(axis) = axis {
            state = with_axis(state, axis);
        }
        context.level.set_block(pos, state);
        mark_above(context.level, pos);
        pos
    }

    fn decorate_logs(
        &self,
        context: &mut Context<'_, '_>,
        logs: &JavaSet,
        decorators: &[Decorator],
    ) {
        if decorators.is_empty() {
            return;
        }
        let mut with = Decorating {
            logs: sorted_by_y(logs),
            leaves: Vec::new(),
            roots: Vec::new(),
            decorations: None,
        };
        decorate(context, decorators, &mut with);
    }
}

/// `markAboveForPostProcessing`: the two blocks above, up to the first air.
fn mark_above(level: &mut Level<'_>, pos: BlockPos) {
    let mut above = pos;
    for _ in 0..2 {
        above = above.offset(0, 1, 0);
        if level.block(above).props().is_air() {
            return;
        }
        level.mark_for_post_processing(above);
    }
}

/// `isOverSolidGround`.
fn over_solid_ground(level: &mut Level<'_>, pos: BlockPos) -> bool {
    sturdy(
        level.block(pos.offset(0, -1, 0)),
        Face::Up,
        SupportType::Full,
    )
}

impl Feature for FallenTree {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let stump = self.log(context, origin, None);
        let mut stump_set = JavaSet::new();
        stump_set.insert(stump);
        self.decorate_logs(context, &stump_set, &self.stump_decorators);

        let direction = random_horizontal(&mut *context.random);
        let length = self.log_length.sample(&mut *context.random) - 2;
        let distance = 2 + context.random.next_i32_bounded(2);
        let mut start = relative_by(origin, direction, distance);
        // `setGroundHeightForFallenLogStartPos`: from one above, down to the first
        // place a log may lie on solid ground, six tries.
        start = start.offset(0, 1, 0);
        for _ in 0..6 {
            if valid_tree_pos(context.level, start) && over_solid_ground(context.level, start) {
                break;
            }
            start = start.offset(0, -1, 0);
        }
        // `canPlaceEntireFallenLog`: room all along, and no gap in the ground longer
        // than two.
        let mut gap = 0;
        let mut pos = start;
        for _ in 0..length {
            if !valid_tree_pos(context.level, pos) {
                return true;
            }
            if over_solid_ground(context.level, pos) {
                gap = 0;
            } else {
                gap += 1;
                if gap > 2 {
                    return true;
                }
            }
            pos = relative(pos, direction);
        }
        let mut logs = JavaSet::new();
        let mut pos = start;
        let axis = Axis::of(direction);
        for _ in 0..length {
            logs.insert(self.log(context, pos, Some(axis)));
            pos = relative(pos, direction);
        }
        self.decorate_logs(context, &logs, &self.log_decorators);
        true
    }
}
