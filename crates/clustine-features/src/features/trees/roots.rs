// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/tree/roots.rs`.

//! The root placers (`RootPlacer`): the mangrove's, the one kind the data uses. Its
//! roots raise the trunk and spread down and out from it.

use clustine_data::{BlockState, Face};
use clustine_noise::Random;
use clustine_world::BlockPos;

use super::common::{HORIZONTAL, Placement, manhattan, valid_tree_pos};
use crate::blocks::{BlockSet, fluid_in_tag, relative};
use crate::context::{Context, Level};
use crate::providers::Provider;
use crate::reader::{Read, Reader};
use crate::values::IntProvider;

#[derive(Debug, Clone)]
struct AboveRoot {
    provider: Provider,
    chance: f32,
}

#[derive(Debug, Clone)]
pub struct RootPlacer {
    trunk_offset_y: IntProvider,
    root_provider: Provider,
    above_root: Option<AboveRoot>,
    can_grow_through: BlockSet,
    muddy_roots_in: BlockSet,
    muddy_roots_provider: Provider,
    max_root_width: i32,
    max_root_length: i32,
    random_skew_chance: f32,
}

impl RootPlacer {
    pub fn read(reader: &Reader) -> Read<Self> {
        match reader.kind()? {
            "minecraft:mangrove_root_placer" => {}
            other => return Err(reader.error(format!("{other} is not a root placer"))),
        }
        let above_root = match reader.optional("above_root_placement") {
            Some(above) => {
                let read = AboveRoot {
                    provider: Provider::read(&above.field("above_root_provider")?)?,
                    chance: above.field("above_root_placement_chance")?.float()?,
                };
                above.finish()?;
                Some(read)
            }
            None => None,
        };
        let mangrove = reader.field("mangrove_root_placement")?;
        let placer = Self {
            trunk_offset_y: IntProvider::read(&reader.field("trunk_offset_y")?)?,
            root_provider: Provider::read(&reader.field("root_provider")?)?,
            above_root,
            can_grow_through: BlockSet::read(&mangrove.field("can_grow_through")?)?,
            muddy_roots_in: BlockSet::read(&mangrove.field("muddy_roots_in")?)?,
            muddy_roots_provider: Provider::read(&mangrove.field("muddy_roots_provider")?)?,
            max_root_width: mangrove.field("max_root_width")?.int()?,
            max_root_length: mangrove.field("max_root_length")?.int()?,
            random_skew_chance: mangrove.field("random_skew_chance")?.float()?,
        };
        mangrove.finish()?;
        reader.finish()?;
        Ok(placer)
    }

    /// `getTrunkOrigin`: the trunk stands on its roots.
    pub fn trunk_origin<R: Random + ?Sized>(&self, origin: BlockPos, random: &mut R) -> BlockPos {
        origin.offset(0, self.trunk_offset_y.sample(random), 0)
    }

    /// `canPlaceRoot`: where a tree may grow, or through what the roots grow through.
    fn can_place(&self, level: &mut Level<'_>, pos: BlockPos) -> bool {
        valid_tree_pos(level, pos) || self.can_grow_through.contains(level.block(pos))
    }

    /// `placeRoots`: false, placing nothing, where the column under the trunk or a
    /// root would be blocked or too long.
    pub fn place(
        &self,
        context: &mut Context<'_, '_>,
        placement: &mut Placement,
        origin: BlockPos,
        trunk_origin: BlockPos,
    ) -> bool {
        let mut column = origin;
        while column.y < trunk_origin.y {
            if !self.can_place(context.level, column) {
                return false;
            }
            column = column.offset(0, 1, 0);
        }
        let mut roots = vec![trunk_origin.offset(0, -1, 0)];
        for direction in HORIZONTAL {
            let root = relative(trunk_origin, direction);
            let mut found = Vec::new();
            if !self.simulate(context, root, direction, trunk_origin, &mut found, 0) {
                return false;
            }
            roots.extend(found);
            roots.push(root);
        }
        for root in roots {
            self.place_root(context, placement, root);
        }
        true
    }

    /// `simulateRoots`: the positions one root reaches, depth first.
    fn simulate(
        &self,
        context: &mut Context<'_, '_>,
        pos: BlockPos,
        direction: Face,
        origin: BlockPos,
        found: &mut Vec<BlockPos>,
        layer: i32,
    ) -> bool {
        if layer == self.max_root_length || found.len() as i32 > self.max_root_length {
            return false;
        }
        for next in self.potential_positions(pos, direction, context.random, origin) {
            if self.can_place(context.level, next) {
                found.push(next);
                if !self.simulate(context, next, direction, origin, found, layer + 1) {
                    return false;
                }
            }
        }
        true
    }

    /// `potentialRootPositions`.
    fn potential_positions<R: Random + ?Sized>(
        &self,
        pos: BlockPos,
        direction: Face,
        random: &mut R,
        origin: BlockPos,
    ) -> Vec<BlockPos> {
        let below = pos.offset(0, -1, 0);
        let next_to = relative(pos, direction);
        let width = manhattan(pos, origin);
        if width > self.max_root_width - 3 && width <= self.max_root_width {
            if random.next_f32() < self.random_skew_chance {
                vec![below, next_to.offset(0, -1, 0)]
            } else {
                vec![below]
            }
        } else if width > self.max_root_width || random.next_f32() < self.random_skew_chance {
            vec![below]
        } else if random.next_bool() {
            vec![next_to]
        } else {
            vec![below]
        }
    }

    /// `placeRoot`: muddy roots in mud, else a root and maybe a block on top of it.
    fn place_root(&self, context: &mut Context<'_, '_>, placement: &mut Placement, pos: BlockPos) {
        if self.muddy_roots_in.contains(context.level.block(pos)) {
            let state = self.muddy_roots_provider.state(context, pos);
            let state = waterlogged_by(context.level, pos, state);
            placement.set_root(context.level, pos, state);
            return;
        }
        if !self.can_place(context.level, pos) {
            return;
        }
        let state = self.root_provider.state(context, pos);
        let state = waterlogged_by(context.level, pos, state);
        placement.set_root(context.level, pos, state);
        if let Some(above_root) = &self.above_root {
            let above = pos.offset(0, 1, 0);
            if context.random.next_f32() < above_root.chance
                && context.level.block(above).props().is_air()
            {
                let state = above_root.provider.state(context, above);
                let state = waterlogged_by(context.level, above, state);
                placement.set_root(context.level, above, state);
            }
        }
    }
}

/// `getPotentiallyWaterloggedState`: waterlogged where any water is.
fn waterlogged_by(level: &mut Level<'_>, pos: BlockPos, state: BlockState) -> BlockState {
    if state.property("waterlogged").is_none() {
        return state;
    }
    let water = fluid_in_tag(level.block(pos), "minecraft:water");
    state
        .with_property("waterlogged", if water { "true" } else { "false" })
        .unwrap_or(state)
}
