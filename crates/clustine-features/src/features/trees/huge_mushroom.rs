// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/huge_mushroom.rs`.

//! `huge_brown_mushroom` and `huge_red_mushroom` (`AbstractHugeMushroomFeature` and
//! its two kinds): a stem under a flat cap or a domed one.

use clustine_data::BlockState;
use clustine_noise::Random;
use clustine_world::BlockPos;

use super::common::{LEAVES, REPLACEABLE_BY_MUSHROOMS};
use crate::context::{Context, Level};
use crate::feature::Feature;
use crate::predicates::BlockPredicate;
use crate::providers::Provider;
use crate::reader::{Read, Reader};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cap {
    Brown,
    Red,
}

#[derive(Debug)]
pub struct HugeMushroom {
    cap: Cap,
    cap_provider: Provider,
    stem_provider: Provider,
    foliage_radius: i32,
    can_place_on: BlockPredicate,
}

impl HugeMushroom {
    pub fn read(cap: Cap, reader: &Reader) -> Read<Self> {
        let mushroom = Self {
            cap,
            cap_provider: Provider::read(&reader.field("cap_provider")?)?,
            stem_provider: Provider::read(&reader.field("stem_provider")?)?,
            foliage_radius: match reader.optional("foliage_radius") {
                Some(radius) => radius.int()?,
                None => 2,
            },
            can_place_on: BlockPredicate::read(&reader.field("can_place_on")?)?,
        };
        reader.finish()?;
        Ok(mushroom)
    }

    /// `getTreeRadiusForHeight`, as the check of room calls it: with -1 for the
    /// tree's height, so the red cap is checked by its stem's width alone.
    fn radius_for_height(&self, tree_height: i32, y: i32) -> i32 {
        match self.cap {
            Cap::Brown => {
                if y <= 3 {
                    0
                } else {
                    self.foliage_radius
                }
            }
            Cap::Red => {
                if y < tree_height && y >= tree_height - 3 || y == tree_height {
                    self.foliage_radius
                } else {
                    0
                }
            }
        }
    }

    /// `isValidPosition`.
    fn valid(&self, context: &mut Context<'_, '_>, origin: BlockPos, height: i32) -> bool {
        let level = &*context.level;
        if origin.y < level.min_y() + 1 || origin.y + height + 2 > level.top_y() {
            return false;
        }
        if !self.can_place_on.test(context, origin.offset(0, -1, 0)) {
            return false;
        }
        for y in 0..=height {
            let radius = self.radius_for_height(-1, y);
            for x in -radius..=radius {
                for z in -radius..=radius {
                    let state = context.level.block(origin.offset(x, y, z));
                    if !state.props().is_air() && !LEAVES.contains(state) {
                        return false;
                    }
                }
            }
        }
        true
    }

    fn brown_cap(&self, context: &mut Context<'_, '_>, origin: BlockPos, height: i32) {
        let radius = self.foliage_radius;
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                let (min_x, max_x, min_z, max_z) =
                    (dx == -radius, dx == radius, dz == -radius, dz == radius);
                let x_edge = min_x || max_x;
                let z_edge = min_z || max_z;
                if x_edge && z_edge {
                    continue;
                }
                let pos = origin.offset(dx, height, dz);
                let west = min_x || z_edge && dx == 1 - radius;
                let east = max_x || z_edge && dx == radius - 1;
                let north = min_z || x_edge && dz == 1 - radius;
                let south = max_z || x_edge && dz == radius - 1;
                let mut state = self.cap_provider.state(context, origin);
                if has_sides(state) {
                    state = with_sides(state, west, east, north, south);
                }
                place_block(context.level, pos, state);
            }
        }
    }

    fn red_cap(&self, context: &mut Context<'_, '_>, origin: BlockPos, height: i32) {
        for dy in height - 3..=height {
            let radius = if dy < height {
                self.foliage_radius
            } else {
                self.foliage_radius - 1
            };
            let centre = self.foliage_radius - 2;
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    let x_edge = dx == -radius || dx == radius;
                    let z_edge = dz == -radius || dz == radius;
                    if dy < height && x_edge == z_edge {
                        continue;
                    }
                    let pos = origin.offset(dx, dy, dz);
                    let mut state = self.cap_provider.state(context, origin);
                    if has_sides(state) && state.property("up").is_some() {
                        let up = if dy >= height - 1 { "true" } else { "false" };
                        state = state.with_property("up", up).unwrap_or(state);
                        state =
                            with_sides(state, dx < -centre, dx > centre, dz < -centre, dz > centre);
                    }
                    place_block(context.level, pos, state);
                }
            }
        }
    }
}

fn has_sides(state: BlockState) -> bool {
    ["west", "east", "north", "south"]
        .iter()
        .all(|side| state.property(side).is_some())
}

fn with_sides(state: BlockState, west: bool, east: bool, north: bool, south: bool) -> BlockState {
    let mut state = state;
    for (side, value) in [
        ("west", west),
        ("east", east),
        ("north", north),
        ("south", south),
    ] {
        state = state
            .with_property(side, if value { "true" } else { "false" })
            .unwrap_or(state);
    }
    state
}

/// `placeMushroomBlock`: only into air or what a mushroom may replace.
fn place_block(level: &mut Level<'_>, pos: BlockPos, state: BlockState) {
    let here = level.block(pos);
    if here.props().is_air() || REPLACEABLE_BY_MUSHROOMS.contains(here) {
        level.set_block(pos, state);
    }
}

impl Feature for HugeMushroom {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let mut height = context.random.next_i32_bounded(3) + 4;
        if context.random.next_i32_bounded(12) == 0 {
            height *= 2;
        }
        if !self.valid(context, origin, height) {
            return false;
        }
        match self.cap {
            Cap::Brown => self.brown_cap(context, origin, height),
            Cap::Red => self.red_cap(context, origin, height),
        }
        for y in 0..height {
            let state = self.stem_provider.state(context, origin);
            place_block(context.level, origin.offset(0, y, 0), state);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mushroom(cap: Cap) -> HugeMushroom {
        HugeMushroom {
            cap,
            cap_provider: Provider::Simple(clustine_data::blocks::RED_MUSHROOM_BLOCK),
            stem_provider: Provider::Simple(clustine_data::blocks::MUSHROOM_STEM),
            foliage_radius: 3,
            can_place_on: BlockPredicate::True,
        }
    }

    #[test]
    fn the_red_cap_s_room_is_checked_by_its_stem_alone_as_the_game_s_call_does() {
        assert_eq!(mushroom(Cap::Red).radius_for_height(-1, 4), 0);
        assert_eq!(mushroom(Cap::Red).radius_for_height(-1, -1), 3);
    }

    #[test]
    fn the_brown_cap_s_room_is_checked_wide_above_three() {
        assert_eq!(mushroom(Cap::Brown).radius_for_height(-1, 3), 0);
        assert_eq!(mushroom(Cap::Brown).radius_for_height(-1, 4), 3);
    }
}
