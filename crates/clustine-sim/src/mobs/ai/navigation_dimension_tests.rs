//! The level's heights in navigation by dimension (`docs/groundwork/p-plan.md`, 17.2
//! D2): `findSurfacePosition` searches between the level's `getMinY` and `getMaxY`, its
//! whole height, in each of the three dimensions. Declared in `navigation.rs`.

use clustine_data::{BlockState, blocks};
use clustine_world::{BlockPos, ChunkPos, Dimension, EntityId};

use super::{height_limits, surface_position};
use crate::mobs::view::{LightAt, MobWorld, Seen, SeenPlayer};
use crate::physics::World;
use crate::physics::aabb::Aabb;
use crate::physics::item::Region as Holds;

/// A level of `dimension` that is stone up to `ground` (none where it is `None`) and air
/// above, within the level's height.
struct Column {
    dimension: Dimension,
    ground: Option<i32>,
}

impl World for Column {
    fn block(&self, pos: BlockPos) -> Option<BlockState> {
        let (low, high) = height_limits(self.dimension);
        if pos.y < low || pos.y > high {
            return None;
        }
        Some(match self.ground {
            Some(ground) if pos.y <= ground => blocks::STONE,
            _ => blocks::AIR,
        })
    }

    fn has_chunk(&self, _: ChunkPos) -> bool {
        true
    }
}

impl Holds for Column {
    fn may_hold(&self, _: ChunkPos) -> bool {
        true
    }

    fn min_y(&self) -> i32 {
        self.dimension.rules().min_y
    }
}

impl MobWorld for Column {
    fn is_loaded(&self, _: BlockPos) -> bool {
        true
    }

    fn entities_in(&self, _: &Aabb) -> Vec<Seen<'_>> {
        Vec::new()
    }

    fn entity(&self, _: EntityId) -> Option<Seen<'_>> {
        None
    }

    fn players(&self) -> Vec<SeenPlayer> {
        Vec::new()
    }

    fn light(&self, _: BlockPos) -> Option<LightAt> {
        None
    }

    fn dimension(&self) -> Dimension {
        self.dimension
    }
}

#[test]
fn each_level_s_heights_are_its_whole_height() {
    let limits: Vec<(i32, i32)> = Dimension::ALL.into_iter().map(height_limits).collect();
    assert_eq!(limits, [(-64, 319), (0, 255), (0, 255)]);
}

#[test]
fn a_surface_over_no_ground_is_found_above_the_level_s_top() {
    for (dimension, top) in [
        (Dimension::Overworld, 320),
        (Dimension::Nether, 256),
        (Dimension::End, 256),
    ] {
        let world = Column {
            dimension,
            ground: None,
        };
        let found = surface_position(&world, BlockPos::new(3, 40, 3));
        assert_eq!(found, BlockPos::new(3, top, 3), "{dimension:?}");
    }
}

#[test]
fn a_surface_inside_solid_ground_is_its_first_air_or_above_the_level_s_top() {
    for dimension in Dimension::ALL {
        let world = Column {
            dimension,
            ground: Some(60),
        };
        let found = surface_position(&world, BlockPos::new(0, 10, 0));
        assert_eq!(found, BlockPos::new(0, 61, 0), "{dimension:?}");
        let found = surface_position(&world, BlockPos::new(0, 90, 0));
        assert_eq!(found, BlockPos::new(0, 61, 0), "{dimension:?}");
        let (_, high) = height_limits(dimension);
        let world = Column {
            dimension,
            ground: Some(high),
        };
        let found = surface_position(&world, BlockPos::new(0, 10, 0));
        assert_eq!(found, BlockPos::new(0, high + 1, 0), "{dimension:?}");
    }
}
