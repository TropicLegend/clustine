// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/underwater_magma.rs`.

//! `underwater_magma`: magma blocks scattered in the floor under water, each hidden
//! on its sides and below so that only its top shows (`UnderwaterMagmaFeature`).

use clustine_data::{Face, blocks, faces};
use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::blocks::{HORIZONTAL, relative, same_block};
use crate::context::{Context, Level};
use crate::feature::Feature;
use crate::features::ground::shared::between_closed;
use crate::reader::{Read, Reader};

#[derive(Debug)]
pub struct UnderwaterMagma {
    floor_search_range: i32,
    placement_radius_around_floor: i32,
    placement_probability_per_valid_position: f32,
}

impl UnderwaterMagma {
    pub fn read(reader: &Reader) -> Read<Self> {
        let magma = Self {
            floor_search_range: reader.field("floor_search_range")?.int()?,
            placement_radius_around_floor: reader.field("placement_radius_around_floor")?.int()?,
            placement_probability_per_valid_position: reader
                .field("placement_probability_per_valid_position")?
                .float()?,
        };
        reader.finish()?;
        Ok(magma)
    }

    /// The floor under the water at the origin (`Column.scan` downwards): the first
    /// block that is not water within the search, if there is one.
    fn floor(&self, level: &mut Level<'_>, origin: BlockPos) -> Option<i32> {
        if !is_water(level, origin) {
            return None;
        }
        let mut pos = origin;
        for _ in 1..self.floor_search_range {
            if !is_water(level, pos) {
                break;
            }
            pos = pos.offset(0, -1, 0);
        }
        (!is_water(level, pos)).then_some(pos.y)
    }
}

fn is_water(level: &mut Level<'_>, pos: BlockPos) -> bool {
    same_block(level.block(pos), blocks::WATER)
}

/// `isVisibleFromOutside`: whether the face of the block at `pos` towards the magma
/// leaves any of it open, by the block's occlusion shape on that face
/// (`getFaceOcclusionShape`), which is empty for a block that does not occlude.
fn visible_from_outside(level: &mut Level<'_>, pos: BlockPos, covered: Face) -> bool {
    let props = level.block(pos).props();
    if !props.occludes() {
        return true;
    }
    if props.occludes_by_shape() {
        // The table keeps the faces of a block that occludes by its shape.
        let face = props.occlusion_face(covered);
        let boxes: Vec<[f64; 6]> = face
            .rectangles()
            .map(|[u0, v0, u1, v1]| face_box(covered, [u0, v0, u1, v1]))
            .collect();
        return !faces::face_full(&boxes, covered);
    }
    // Any other block that occludes does so with its whole shape, which the game
    // takes from its outline.
    let boxes: Vec<[f64; 6]> = props.outline_shape().boxes().collect();
    !faces::face_full(&boxes, covered)
}

/// A box through the whole depth of the block whose face on `face` is the rectangle
/// `[min u, min v, max u, max v]`.
fn face_box(face: Face, [u0, v0, u1, v1]: [f64; 4]) -> [f64; 6] {
    match face {
        Face::Down | Face::Up => [u0, 0.0, v0, u1, 1.0, v1],
        Face::North | Face::South => [u0, v0, 0.0, u1, v1, 1.0],
        Face::West | Face::East => [0.0, u0, v0, 1.0, u1, v1],
    }
}

/// The opposite direction.
fn opposite(face: Face) -> Face {
    match face {
        Face::Down => Face::Up,
        Face::Up => Face::Down,
        Face::North => Face::South,
        Face::South => Face::North,
        Face::West => Face::East,
        Face::East => Face::West,
    }
}

/// `isValidPlacement`: a block that is neither water nor air, closed below and on its
/// four sides.
fn valid_placement(level: &mut Level<'_>, pos: BlockPos) -> bool {
    let state = level.block(pos);
    if same_block(state, blocks::WATER) || state.props().is_air() {
        return false;
    }
    if visible_from_outside(level, relative(pos, Face::Down), Face::Up) {
        return false;
    }
    HORIZONTAL
        .into_iter()
        .all(|face| !visible_from_outside(level, relative(pos, face), opposite(face)))
}

impl Feature for UnderwaterMagma {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let Some(floor) = self.floor(context.level, origin) else {
            return false;
        };
        let floor = BlockPos::new(origin.x, floor, origin.z);
        let r = self.placement_radius_around_floor;
        let mut placed = false;
        for pos in between_closed(floor.offset(-r, -r, -r), floor.offset(r, r, r)) {
            if context.random.next_f32() < self.placement_probability_per_valid_position
                && valid_placement(context.level, pos)
            {
                context.level.set_block(pos, blocks::MAGMA_BLOCK);
                placed = true;
            }
        }
        placed
    }
}

#[cfg(test)]
mod tests {
    use clustine_worldgen_data::feature as data;

    use super::*;
    use crate::testing::World;

    fn magma() -> Box<dyn Feature> {
        let (_, entry) = data::configured("minecraft:underwater_magma").unwrap();
        crate::feature::read_configured(&Reader::new(&entry.data, entry.name)).unwrap()
    }

    #[test]
    fn magma_lies_in_the_floor_under_water_and_marks_the_water_above_it() {
        let magma = magma();
        let mut world = World::flat(blocks::STONE, 40);
        for x in 0..16 {
            for z in 0..16 {
                for y in 41..=50 {
                    world.set(BlockPos::new(x, y, z), blocks::WATER);
                }
            }
        }
        world.writes.clear();
        assert!(world.with_context(9, |context| magma.place(context, BlockPos::new(8, 43, 8))));
        assert!(!world.writes.is_empty());
        for (pos, state) in &world.writes {
            assert_eq!(*state, blocks::MAGMA_BLOCK);
            // Only blocks under the floor's top, closed on every side but the top.
            assert!((39..=40).contains(&pos.y), "{pos:?}");
            assert!(world.marks.contains(&pos.offset(0, 1, 0)));
        }
        assert_eq!(world.marks.len(), world.writes.len());
    }

    #[test]
    fn magma_is_not_placed_where_the_origin_is_dry_or_the_floor_out_of_reach() {
        let magma = magma();
        let mut world = World::flat(blocks::STONE, 40);
        assert!(!world.with_context(9, |context| magma.place(context, BlockPos::new(8, 43, 8))));
        for y in 41..=60 {
            world.set(BlockPos::new(8, y, 8), blocks::WATER);
        }
        assert!(!world.with_context(9, |context| magma.place(context, BlockPos::new(8, 55, 8))));
    }

    #[test]
    fn a_full_block_covers_a_face_and_water_or_a_slab_s_top_does_not() {
        let mut world = World::flat(blocks::STONE, 40);
        world.set(BlockPos::new(0, 50, 0), blocks::WATER);
        world.set(BlockPos::new(1, 50, 0), clustine_data::blocks::STONE_SLAB);
        world.with_level(|level| {
            assert!(!visible_from_outside(
                level,
                BlockPos::new(0, 40, 0),
                Face::Up
            ));
            assert!(visible_from_outside(
                level,
                BlockPos::new(0, 50, 0),
                Face::Up
            ));
            // A bottom slab covers its bottom and not its top.
            assert!(visible_from_outside(
                level,
                BlockPos::new(1, 50, 0),
                Face::Up
            ));
            assert!(!visible_from_outside(
                level,
                BlockPos::new(1, 50, 0),
                Face::Down
            ));
        });
    }
}
