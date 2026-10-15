// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/bamboo.rs`.

//! `bamboo`: a stalk of bamboo five to sixteen high with leaves at its top, on a
//! disc of podzol by a chance (`BambooFeature`).

use clustine_data::{BlockState, blocks};
use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::blocks::in_block_tag;
use crate::context::{Context, Heightmap};
use crate::feature::Feature;
use crate::reader::{Read, Reader};
use crate::survive::can_survive;

#[derive(Debug)]
pub struct Bamboo {
    /// The chance of the disc of podzol.
    probability: f32,
}

impl Bamboo {
    pub fn read(reader: &Reader) -> Read<Self> {
        Ok(Self {
            probability: reader.field("probability")?.float()?,
        })
    }
}

/// A stalk without leaves, as the game's `BAMBOO_TRUNK`.
fn trunk() -> BlockState {
    bamboo("none", "0")
}

fn bamboo(leaves: &str, stage: &str) -> BlockState {
    let with = |state: BlockState, name: &str, value: &str| {
        state
            .with_property(name, value)
            .expect("bamboo has an age, leaves and a stage")
    };
    with(
        with(with(blocks::BAMBOO, "age", "1"), "leaves", leaves),
        "stage",
        stage,
    )
}

impl Feature for Bamboo {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        if !context.level.block(origin).props().is_air() {
            return false;
        }
        if can_survive(blocks::BAMBOO, origin, context.level) {
            let height = context.random.next_i32_bounded(12) + 5;
            if context.random.next_f32() < self.probability {
                podzol(context, origin);
            }
            let mut at = origin;
            for _ in 0..height {
                if !context.level.block(at).props().is_air() {
                    break;
                }
                context.level.set_block(at, trunk());
                at = at.offset(0, 1, 0);
            }
            if at.y - origin.y >= 3 {
                context.level.set_block(at, bamboo("large", "1"));
                at = at.offset(0, -1, 0);
                context.level.set_block(at, bamboo("large", "0"));
                at = at.offset(0, -1, 0);
                context.level.set_block(at, bamboo("small", "0"));
            }
        }
        // The game counts the stalk as placed once the origin is empty, whether it
        // could stand there or not.
        true
    }
}

/// A disc of one to four blocks' radius whose top blocks become podzol where they may.
fn podzol(context: &mut Context<'_, '_>, origin: BlockPos) {
    let radius = context.random.next_i32_bounded(4) + 1;
    for x in origin.x - radius..=origin.x + radius {
        for z in origin.z - radius..=origin.z + radius {
            let (dx, dz) = (x - origin.x, z - origin.z);
            if dx * dx + dz * dz > radius * radius {
                continue;
            }
            let y = context.level.height_at(Heightmap::WorldSurface, x, z) - 1;
            let at = BlockPos::new(x, y, z);
            if in_block_tag(
                context.level.block(at),
                "minecraft:beneath_bamboo_podzol_replaceable",
            ) {
                context.level.set_block(at, blocks::PODZOL);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use clustine_worldgen_data::feature::Data;

    use super::*;
    use crate::blocks::same_block;
    use crate::testing::World;

    static BAMBOO: Data = Data::Object(&[
        ("type", Data::Text("minecraft:bamboo")),
        ("probability", Data::Number("1.0")),
    ]);

    #[test]
    fn a_stalk_grows_up_from_grass_with_leaves_at_its_top_on_podzol() {
        let bamboo = crate::feature::read_configured(&Reader::new(&BAMBOO, "test")).unwrap();
        let mut world = World::flat(blocks::GRASS_BLOCK, 64);
        let at = BlockPos::new(5, 65, 5);
        assert!(world.with_context(7, |context| bamboo.place(context, at)));
        let mut top = 65;
        while same_block(world.get(BlockPos::new(5, top + 1, 5)), blocks::BAMBOO) {
            top += 1;
        }
        let height = top - 65 + 1;
        assert!((5..=17).contains(&height), "{height}");
        let leaves = |y| world.get(BlockPos::new(5, y, 5)).property("leaves");
        assert_eq!(leaves(top), Some("large"));
        assert_eq!(
            world.get(BlockPos::new(5, top, 5)).property("stage"),
            Some("1")
        );
        assert_eq!(leaves(top - 1), Some("large"));
        assert_eq!(leaves(top - 2), Some("small"));
        assert_eq!(leaves(65), Some("none"));
        assert_eq!(world.get(BlockPos::new(5, 64, 5)), blocks::PODZOL);
        // A stalk where one stands already is not placed.
        assert!(!world.with_context(7, |context| bamboo.place(context, at)));
    }

    #[test]
    fn a_stalk_on_stone_places_nothing_and_says_it_was_placed() {
        let bamboo = crate::feature::read_configured(&Reader::new(&BAMBOO, "test")).unwrap();
        let mut world = World::flat(blocks::STONE, 64);
        assert!(world.with_context(7, |context| bamboo.place(context, BlockPos::new(5, 65, 5))));
        assert!(world.writes.is_empty());
    }
}
