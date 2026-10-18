//! What an entity meets on a block: the plain fields the game's `Block` keeps from its
//! properties (`docs/groundwork/n4-brief.md`, section 1.8), and how the block's shapes
//! are shifted by its position.

use crate::BlockState;
use crate::generated::block_physics::{BLOCK_PHYSICS, PHYSICS};

/// How the game shifts a block by its position (its `OffsetType`): plants sideways,
/// some up and down too.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum OffsetType {
    None,
    /// Sideways, by up to [`BlockPhysics::max_horizontal_offset`].
    Xz,
    /// Sideways, and down by up to [`BlockPhysics::max_vertical_offset`].
    Xyz,
}

/// The fields of a block that movement reads, as the game holds them.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BlockPhysics {
    /// `getFriction`: 0.6 for most blocks, more for ice, which is what slides.
    pub friction: f32,
    /// `getSpeedFactor`: below 1 on soul sand and honey.
    pub speed_factor: f32,
    /// `getJumpFactor`: below 1 on honey.
    pub jump_factor: f32,
    /// `getBounceRestitution`: what part of the fall a block gives back, as a slime
    /// block and a bed do.
    pub bounce_restitution: f32,
    /// `getFallDistanceReduction`: what part of the fall a bed takes away.
    pub fall_distance_reduction: f32,
    pub offset_type: OffsetType,
    /// `getMaxHorizontalOffset`, which bounds the sideways shift.
    pub max_horizontal_offset: f32,
    /// `getMaxVerticalOffset`, which scales the shift down of [`OffsetType::Xyz`].
    pub max_vertical_offset: f32,
    /// `getExplosionResistance`: what an explosion's ray loses in the block, 6 for stone,
    /// 1200 for obsidian.
    pub explosion_resistance: f32,
}

/// What an entity meets on the block of this id. Panics if there is no such block.
pub fn block_physics(block_id: usize) -> &'static BlockPhysics {
    &PHYSICS[usize::from(BLOCK_PHYSICS[block_id])]
}

impl BlockState {
    /// What an entity meets on the block of this state, or `None` if the id is not a
    /// block state.
    pub fn physics(self) -> Option<&'static BlockPhysics> {
        Some(block_physics(self.block_id()?))
    }
}

#[cfg(test)]
mod tests {
    use crate::{BlockState, OffsetType, blocks};

    fn physics(name: &str) -> &'static super::BlockPhysics {
        BlockState::parse(name).unwrap().physics().unwrap()
    }

    /// The values of the brief's table (section 1.8), which `Blocks` sets.
    #[test]
    fn friction_speed_jump_and_bounce_are_the_games() {
        let stone = blocks::STONE.physics().unwrap();
        assert_eq!(stone.friction, 0.6);
        assert_eq!(stone.friction.to_bits(), 0x3f19_999a);
        assert_eq!(
            (
                stone.speed_factor,
                stone.jump_factor,
                stone.bounce_restitution
            ),
            (1.0, 1.0, 0.0)
        );
        assert_eq!(stone.fall_distance_reduction, 0.0);
        for ice in [
            "minecraft:ice",
            "minecraft:packed_ice",
            "minecraft:frosted_ice",
        ] {
            assert_eq!(physics(ice).friction, 0.98, "{ice}");
        }
        assert_eq!(physics("minecraft:blue_ice").friction, 0.989);
        let slime = physics("minecraft:slime_block");
        assert_eq!((slime.friction, slime.bounce_restitution), (0.8, 1.0));
        assert_eq!(physics("minecraft:soul_sand").speed_factor, 0.4);
        let honey = physics("minecraft:honey_block");
        assert_eq!((honey.speed_factor, honey.jump_factor), (0.4, 0.5));
        let bed = physics("minecraft:red_bed");
        assert_eq!(
            (bed.bounce_restitution, bed.fall_distance_reduction),
            (0.75, 0.5)
        );
        // The ground friction of an item, which the brief works out from these.
        assert_eq!((0.98f32 * stone.friction).to_bits(), 0x3f16_872c);
        assert_eq!(
            (0.98f32 * physics("minecraft:blue_ice").friction).to_bits(),
            0x3f78_1ece
        );
    }

    #[test]
    fn blocks_and_fluids_resist_explosions_as_the_brief_reads_the_game() {
        // `docs/groundwork/n6-brief.md`, section 2.2, read from `Blocks`' properties.
        for (name, resistance) in [
            ("minecraft:stone", 6.0),
            ("minecraft:oak_planks", 3.0),
            ("minecraft:glass", 0.3),
            ("minecraft:obsidian", 1200.0),
            ("minecraft:bedrock", 3_600_000.0),
            ("minecraft:water", 100.0),
            ("minecraft:lava", 100.0),
            ("minecraft:tnt", 0.0),
            ("minecraft:air", 0.0),
        ] {
            assert_eq!(physics(name).explosion_resistance, resistance, "{name}");
        }
        let fluid = |name: &str| {
            let id = crate::FLUIDS
                .iter()
                .position(|fluid| *fluid == name)
                .expect("a fluid of the game");
            crate::FLUID_EXPLOSION_RESISTANCE[id]
        };
        assert_eq!(fluid("minecraft:empty"), 0.0);
        assert_eq!(fluid("minecraft:water"), 100.0);
        assert_eq!(fluid("minecraft:flowing_lava"), 100.0);
    }

    #[test]
    fn plants_are_shifted_by_their_offset_type() {
        assert_eq!(
            blocks::STONE.physics().unwrap().offset_type,
            OffsetType::None
        );
        let bamboo = physics("minecraft:bamboo");
        assert_eq!(bamboo.offset_type, OffsetType::Xz);
        assert_eq!(bamboo.max_horizontal_offset, 0.25);
        assert_eq!(
            physics("minecraft:pointed_dripstone").max_horizontal_offset,
            0.125
        );
        assert_eq!(
            physics("minecraft:short_grass").offset_type,
            OffsetType::Xyz
        );
        assert_eq!(physics("minecraft:poppy").offset_type, OffsetType::Xz);
    }
}
