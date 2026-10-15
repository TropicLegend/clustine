// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/spike.rs`.

//! `spike`: a pointed spire of one block on the ground its predicate allows, mirrored
//! below the ground where it is wide, over a pillar that reaches down to y 50
//! (`SpikeFeature`, the game's ice spikes made general).
//!
//! A spike can reach beyond the chunks its run may write; those blocks are refused
//! by the view, as the game's `ensureCanWrite` refuses them.

use clustine_data::BlockState;
use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::blocks::read_state;
use crate::context::Context;
use crate::feature::Feature;
use crate::predicates::BlockPredicate;
use crate::reader::{Read, Reader};

#[derive(Debug)]
pub struct Spike {
    state: BlockState,
    can_place_on: BlockPredicate,
    can_replace: BlockPredicate,
}

impl Spike {
    pub fn read(reader: &Reader) -> Read<Self> {
        let spike = Self {
            state: read_state(&reader.field("state")?)?,
            can_place_on: BlockPredicate::read(&reader.field("can_place_on")?)?,
            can_replace: BlockPredicate::read(&reader.field("can_replace")?)?,
        };
        reader.finish()?;
        Ok(spike)
    }

    /// Sets the spike's block where there is air or what it may replace.
    fn place_block(&self, context: &mut Context<'_, '_>, pos: BlockPos) {
        let there = context.level.block(pos);
        if there.props().is_air() || self.can_replace.test(context, pos) {
            context.level.set_block(pos, self.state);
        }
    }

    /// The spire: rings narrowing upwards, their edges ragged, and the wider ones
    /// mirrored downwards.
    fn place_body(&self, context: &mut Context<'_, '_>, origin: BlockPos, height: i32, width: i32) {
        for y in 0..height {
            let scale = (1.0 - y as f32 / height as f32) * width as f32;
            let reach = scale.ceil() as i32;
            for x in -reach..=reach {
                let dx = x.abs() as f32 - 0.25;
                for z in -reach..=reach {
                    let dz = z.abs() as f32 - 0.25;
                    let inside = (x == 0 && z == 0) || dx * dx + dz * dz <= scale * scale;
                    if !inside {
                        continue;
                    }
                    let edge = x == -reach || x == reach || z == -reach || z == reach;
                    if edge && context.random.next_f32() > 0.75 {
                        continue;
                    }
                    self.place_block(context, origin.offset(x, y, z));
                    if y != 0 && reach > 1 {
                        self.place_block(context, origin.offset(x, -y, z));
                    }
                }
            }
        }
    }

    /// The pillar under the spire, down through what it may replace to y 50; the
    /// corners of a wide one are broken into runs with gaps.
    fn place_base(&self, context: &mut Context<'_, '_>, origin: BlockPos, width: i32) {
        let pillar = (width - 1).clamp(0, 1);
        for x in -pillar..=pillar {
            for z in -pillar..=pillar {
                let mut pos = origin.offset(x, -1, z);
                let mut run = 50;
                if x.abs() == 1 && z.abs() == 1 {
                    run = context.random.next_i32_bounded(5);
                }
                while pos.y > 50 {
                    let there = context.level.block(pos);
                    if !there.props().is_air()
                        && !self.can_replace.test(context, pos)
                        && there != self.state
                    {
                        break;
                    }
                    context.level.set_block(pos, self.state);
                    pos = pos.offset(0, -1, 0);
                    run -= 1;
                    if run <= 0 {
                        let gap = context.random.next_i32_bounded(5) + 1;
                        pos = pos.offset(0, -gap, 0);
                        run = context.random.next_i32_bounded(5);
                    }
                }
            }
        }
    }
}

impl Feature for Spike {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let mut origin = origin;
        let floor = context.level.min_y() + 2;
        while context.level.block(origin).props().is_air() && origin.y > floor {
            origin = origin.offset(0, -1, 0);
        }
        if !self.can_place_on.test(context, origin) {
            return false;
        }
        origin = origin.offset(0, context.random.next_i32_bounded(4), 0);
        let height = context.random.next_i32_bounded(4) + 7;
        let width = height / 4 + context.random.next_i32_bounded(2);
        if width > 1 && context.random.next_i32_bounded(60) == 0 {
            origin = origin.offset(0, 10 + context.random.next_i32_bounded(30), 0);
        }
        self.place_body(context, origin, height, width);
        self.place_base(context, origin, width);
        true
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use clustine_worldgen_data::feature as data;

    use super::*;
    use crate::testing::World;

    fn ice_spike() -> Box<dyn Feature> {
        let (_, entry) = data::configured("minecraft:ice_spike").unwrap();
        crate::feature::read_configured(&Reader::new(&entry.data, entry.name)).unwrap()
    }

    #[test]
    fn an_ice_spike_rises_from_snow_and_its_pillar_reaches_down_through_snow() {
        let spike = ice_spike();
        let mut world = World::flat(blocks::SNOW_BLOCK, 64);
        assert!(world.with_context(2, |context| spike.place(context, BlockPos::new(8, 90, 8))));
        assert!(
            world
                .writes
                .iter()
                .all(|(_, state)| *state == blocks::PACKED_ICE)
        );
        // The spire stands above the ground, and its pillar goes down to y 51.
        assert!(world.writes.iter().any(|(pos, _)| pos.y >= 70));
        assert!(world.writes.iter().any(|(pos, _)| pos.y == 51));
        assert!(world.writes.iter().all(|(pos, _)| pos.y > 50));
    }

    #[test]
    fn a_spike_over_stone_is_not_placed() {
        let spike = ice_spike();
        let mut world = World::flat(blocks::STONE, 64);
        assert!(!world.with_context(2, |context| spike.place(context, BlockPos::new(8, 90, 8))));
        assert!(world.writes.is_empty());
    }

    #[test]
    fn a_spike_at_the_edge_of_the_three_by_three_writes_nothing_beyond_it() {
        let spike = ice_spike();
        let mut world = World::flat(blocks::SNOW_BLOCK, 64);
        for seed in 0..20 {
            world.with_context(seed, |context| {
                spike.place(context, BlockPos::new(31, 90, -16))
            });
        }
        assert!(!world.writes.is_empty());
        assert!(
            world
                .writes
                .iter()
                .all(|(pos, _)| pos.x < 32 && pos.z >= -16)
        );
    }
}
