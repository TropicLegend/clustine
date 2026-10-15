// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/single_block_pillar.rs`.

//! `single_block_pillar`: a pillar one block wide from the origin in a direction,
//! through what it may replace and on by a chance at each block, with a feature
//! placed at its last block (`SingleBlockPillarFeature`): the basalt pillars of the
//! Nether's deltas.

use clustine_data::Face;
use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::blocks::{read_face, relative};
use crate::context::Context;
use crate::feature::{Feature, PlacedRef};
use crate::predicates::BlockPredicate;
use crate::providers::Provider;
use crate::reader::{Read, Reader};

#[derive(Debug)]
pub struct SingleBlockPillar {
    block: Provider,
    can_replace: BlockPredicate,
    direction: Face,
    chance_to_continue: f32,
    cap_feature: Option<PlacedRef>,
}

impl SingleBlockPillar {
    pub fn read(reader: &Reader) -> Read<Self> {
        let pillar = Self {
            block: Provider::read(&reader.field("block")?)?,
            can_replace: BlockPredicate::read(&reader.field("can_replace")?)?,
            direction: read_face(&reader.field("direction")?)?,
            chance_to_continue: reader
                .optional("chance_to_continue")
                .map_or(Ok(1.0), |chance| chance.float())?,
            cap_feature: reader
                .optional("cap_feature")
                .map(|cap| PlacedRef::read(&cap))
                .transpose()?,
        };
        reader.finish()?;
        Ok(pillar)
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

impl Feature for SingleBlockPillar {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let mut pos = origin;
        // The chance is drawn for every block the pillar may take, even at a chance of
        // one.
        while self.can_replace.test(context, pos)
            && context.random.next_f32() < self.chance_to_continue
            && !context.level.is_outside_build_height(pos.y)
        {
            let state = self.block.state(context, pos);
            context.level.set_block(pos, state);
            pos = relative(pos, self.direction);
        }
        let last = relative(pos, opposite(self.direction));
        if let Some(cap) = &self.cap_feature {
            cap.place(context, last);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use clustine_worldgen_data::feature::Data;

    use super::*;
    use crate::blocks::same_block;
    use crate::testing::World;

    static PILLAR: Data = Data::Object(&[
        ("type", Data::Text("minecraft:single_block_pillar")),
        (
            "block",
            Data::Object(&[
                ("id", Data::Text("minecraft:basalt")),
                ("properties", Data::Object(&[("axis", Data::Text("y"))])),
            ]),
        ),
        (
            "can_replace",
            Data::Object(&[
                ("type", Data::Text("minecraft:matching_block_tag")),
                ("tag", Data::Text("minecraft:air")),
            ]),
        ),
        ("direction", Data::Text("down")),
        (
            "cap_feature",
            Data::Object(&[
                (
                    "feature",
                    Data::Object(&[
                        ("type", Data::Text("minecraft:simple_block")),
                        (
                            "to_place",
                            Data::Object(&[("id", Data::Text("minecraft:glowstone"))]),
                        ),
                    ]),
                ),
                (
                    "placement",
                    Data::List(&[Data::Object(&[
                        ("type", Data::Text("minecraft:offset")),
                        ("x", Data::Number("1")),
                        ("y", Data::Number("0")),
                        ("z", Data::Number("0")),
                    ])]),
                ),
            ]),
        ),
    ]);

    fn pillar() -> Box<dyn Feature> {
        crate::feature::read_configured(&Reader::new(&PILLAR, "test")).unwrap()
    }

    #[test]
    fn a_pillar_hangs_down_through_air_to_the_ground_and_is_capped_at_its_end() {
        let pillar = pillar();
        let mut world = World::flat(blocks::NETHERRACK, 40);
        assert!(world.with_context(0, |context| pillar.place(context, BlockPos::new(3, 50, 3))));
        for y in 41..=50 {
            assert!(
                same_block(world.get(BlockPos::new(3, y, 3)), blocks::BASALT),
                "{y}"
            );
        }
        assert_eq!(world.get(BlockPos::new(4, 41, 3)), blocks::GLOWSTONE);
    }

    #[test]
    fn a_pillar_with_a_chance_to_continue_stops_early_by_its_draws() {
        static SHORT: Data = Data::Object(&[
            ("type", Data::Text("minecraft:single_block_pillar")),
            ("block", Data::Text("minecraft:basalt")),
            (
                "can_replace",
                Data::Object(&[
                    ("type", Data::Text("minecraft:matching_block_tag")),
                    ("tag", Data::Text("minecraft:air")),
                ]),
            ),
            ("chance_to_continue", Data::Number("0.5")),
            ("direction", Data::Text("down")),
        ]);
        let pillar = crate::feature::read_configured(&Reader::new(&SHORT, "test")).unwrap();
        let mut lengths = Vec::new();
        for seed in 0..10 {
            let mut world = World::flat(blocks::NETHERRACK, 0);
            world.with_context(seed, |context| {
                pillar.place(context, BlockPos::new(3, 50, 3))
            });
            lengths.push(world.writes.len());
        }
        assert!(lengths.contains(&0) && lengths.iter().any(|length| *length > 1));
    }
}
