// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/spring.rs`.

//! `spring_feature`: one block of water or lava in a hole of rock, open to air on as
//! many sides as the settings say, with a tick of its fluid so that it flows when the
//! chunk is live (`SpringFeature`).

use clustine_data::Face;
use clustine_world::BlockPos;

use crate::blocks::{BlockSet, relative};
use crate::context::Context;
use crate::feature::Feature;
use crate::features::ground::shared::FluidState;
use crate::reader::{Read, Reader};

#[derive(Debug)]
pub struct Spring {
    state: FluidState,
    requires_block_below: bool,
    rock_count: i32,
    hole_count: i32,
    valid_blocks: BlockSet,
}

impl Spring {
    pub fn read(reader: &Reader) -> Read<Self> {
        let optional_int = |key: &str, default: i32| -> Read<i32> {
            reader
                .optional(key)
                .map_or(Ok(default), |value| value.int())
        };
        let spring = Self {
            state: FluidState::read(&reader.field("state")?)?,
            requires_block_below: reader
                .optional("requires_block_below")
                .map_or(Ok(true), |value| value.boolean())?,
            rock_count: optional_int("rock_count", 4)?,
            hole_count: optional_int("hole_count", 1)?,
            valid_blocks: BlockSet::read(&reader.field("valid_blocks")?)?,
        };
        reader.finish()?;
        Ok(spring)
    }
}

/// The five blocks around a spring that it counts, in the game's order.
const AROUND: [Face; 5] = [Face::West, Face::East, Face::North, Face::South, Face::Down];

impl Feature for Spring {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let level = &mut *context.level;
        if !self
            .valid_blocks
            .contains(level.block(relative(origin, Face::Up)))
        {
            return false;
        }
        if self.requires_block_below
            && !self
                .valid_blocks
                .contains(level.block(relative(origin, Face::Down)))
        {
            return false;
        }
        let here = level.block(origin);
        if !here.props().is_air() && !self.valid_blocks.contains(here) {
            return false;
        }
        let mut rocks = 0;
        let mut holes = 0;
        for face in AROUND {
            if self
                .valid_blocks
                .contains(level.block(relative(origin, face)))
            {
                rocks += 1;
            }
        }
        for face in AROUND {
            if level.block(relative(origin, face)).props().is_air() {
                holes += 1;
            }
        }
        if rocks != self.rock_count || holes != self.hole_count {
            return false;
        }
        if !level.set_block(origin, self.state.block) {
            return false;
        }
        level.schedule_fluid_tick(origin, self.state.fluid, 0);
        true
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::{FLUIDS, blocks};
    use clustine_world::Ticked;
    use clustine_worldgen_data::feature as data;

    use super::*;
    use crate::testing::World;

    fn named(name: &str) -> Box<dyn Feature> {
        let (_, entry) = data::configured(name).unwrap();
        crate::feature::read_configured(&Reader::new(&entry.data, entry.name)).unwrap()
    }

    #[test]
    fn a_spring_in_a_wall_with_one_side_open_places_water_and_ticks_it() {
        let spring = named("minecraft:spring_water");
        let mut world = World::flat(blocks::STONE, 100);
        let origin = BlockPos::new(5, 40, 7);
        world.set(origin, blocks::AIR);
        world.set(origin.offset(1, 0, 0), blocks::AIR);
        assert!(world.with_context(0, |context| spring.place(context, origin)));
        assert_eq!(world.writes, vec![(origin, blocks::WATER)]);
        let [(at, tick)] = world.ticks.as_slice() else {
            panic!("one tick: {:?}", world.ticks);
        };
        assert_eq!(*at, origin);
        let Ticked::Fluid(fluid) = tick.of else {
            panic!("{tick:?}");
        };
        assert_eq!(FLUIDS[usize::from(fluid)], "minecraft:water");
        assert_eq!((tick.delay, tick.priority), (0, 0));
        assert_eq!((tick.at.x, tick.at.y, tick.at.z), (5, 40, 7));
        assert!(world.marks.is_empty());
    }

    #[test]
    fn a_spring_with_two_sides_open_or_none_is_not_placed() {
        let spring = named("minecraft:spring_water");
        let mut world = World::flat(blocks::STONE, 100);
        let origin = BlockPos::new(5, 40, 7);
        world.set(origin.offset(1, 0, 0), blocks::AIR);
        world.set(origin.offset(-1, 0, 0), blocks::AIR);
        assert!(!world.with_context(0, |context| spring.place(context, origin)));
        let mut closed = World::flat(blocks::STONE, 100);
        assert!(!closed.with_context(0, |context| spring.place(context, origin)));
        assert!(world.ticks.is_empty() && closed.ticks.is_empty());
    }

    #[test]
    fn a_closed_spring_of_the_nether_needs_five_rocks_and_no_hole() {
        let spring = named("minecraft:spring_nether_closed");
        let mut world = World::flat(blocks::NETHERRACK, 100);
        let origin = BlockPos::new(-3, 40, 20);
        assert!(world.with_context(0, |context| spring.place(context, origin)));
        assert_eq!(world.writes, vec![(origin, blocks::LAVA)]);
        let Ticked::Fluid(fluid) = world.ticks[0].1.of else {
            panic!();
        };
        assert_eq!(FLUIDS[usize::from(fluid)], "minecraft:lava");
    }
}
