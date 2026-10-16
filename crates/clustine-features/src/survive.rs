//! The game's `canSurvive` for what features place and ask about, from
//! `clustine-blocks`, which holds the one set of these rules the simulation shares.
//! Features read it through [`Level`](crate::context::Level), which answers for
//! generation: the sky reads 15 while features are placed, and the rules only
//! generation had ported are taken (`clustine_blocks::Rules::Generation`).

pub use clustine_blocks::{can_survive, multiface_can_attach, sturdy};

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use clustine_world::BlockPos;

    use super::*;
    use crate::testing::World;

    #[test]
    fn a_flower_stays_on_grass_and_not_on_stone() {
        let mut world = World::flat(blocks::GRASS_BLOCK, 64);
        let at = BlockPos::new(3, 65, 3);
        assert!(world.with_level(|level| can_survive(blocks::POPPY, at, level)));
        world.set(BlockPos::new(3, 64, 3), blocks::STONE);
        assert!(!world.with_level(|level| can_survive(blocks::POPPY, at, level)));
    }

    #[test]
    fn stone_stays_anywhere_and_a_mushroom_only_where_the_block_below_overrides_light() {
        let mut world = World::flat(blocks::STONE, 64);
        let at = BlockPos::new(0, 65, 0);
        assert!(world.with_level(|level| can_survive(
            blocks::STONE,
            BlockPos::new(0, 200, 0),
            level
        )));
        // The sky reads 15 while features are placed, under ground as well, so stone
        // does not hold a mushroom then; mycelium does.
        assert!(!world.with_level(|level| can_survive(blocks::RED_MUSHROOM, at, level)));
        let mut world = World::flat(blocks::MYCELIUM, 64);
        assert!(world.with_level(|level| can_survive(blocks::RED_MUSHROOM, at, level)));
        assert!(!world.with_level(|level| can_survive(
            blocks::RED_MUSHROOM,
            BlockPos::new(0, 70, 0),
            level
        )));
    }

    #[test]
    fn coral_needs_a_sturdy_top_and_tall_seagrass_still_water() {
        let mut world = World::flat(blocks::SAND, 40);
        let at = BlockPos::new(1, 41, 1);
        assert!(world.with_level(|level| can_survive(blocks::TUBE_CORAL, at, level)));
        let tall = blocks::TALL_SEAGRASS
            .with_property("half", "lower")
            .unwrap();
        assert!(!world.with_level(|level| can_survive(tall, at, level)));
        world.set(at, blocks::WATER);
        assert!(world.with_level(|level| can_survive(tall, at, level)));
    }

    #[test]
    fn a_spore_blossom_hangs_from_a_ceiling_out_of_water() {
        let mut world = World::flat(blocks::STONE, 0);
        let at = BlockPos::new(2, 30, 2);
        assert!(!world.with_level(|level| can_survive(blocks::SPORE_BLOSSOM, at, level)));
        world.set(BlockPos::new(2, 31, 2), blocks::STONE);
        assert!(world.with_level(|level| can_survive(blocks::SPORE_BLOSSOM, at, level)));
    }

    #[test]
    fn sugar_cane_needs_water_beside_its_ground() {
        let mut world = World::flat(blocks::SAND, 62);
        let at = BlockPos::new(5, 63, 5);
        assert!(!world.with_level(|level| can_survive(blocks::SUGAR_CANE, at, level)));
        world.set(BlockPos::new(6, 62, 5), blocks::WATER);
        assert!(world.with_level(|level| can_survive(blocks::SUGAR_CANE, at, level)));
    }
}
