// Adapted from clustine-sim's `src/placement/survive.rs`, itself adapted from SteelMC's
// `canSurvive` of the blocks under `steel-core/src/behavior/blocks/`.

//! The game's `canSurvive` for what features place and ask about: whether a block can
//! stay where it is, by its class.
//!
//! clustine-sim has the same rules for blocks a player places, but they are private to
//! it and read the world through its own view of a click, so they are ported here
//! against [`Level`]. The rules a feature needs and the player's do not are added:
//! coral plants, fire, the spore blossom, tall seagrass and the pale moss carpet. A
//! block whose class has a `canSurvive` that is not ported cannot stay anywhere, so a
//! gap shows as a missing block and not as a misplaced one.

use clustine_data::{BlockClass, BlockState, Face, SupportType, blocks, faces};
use clustine_world::BlockPos;

use crate::blocks::{
    HORIZONTAL, face_name, fluid_in_tag, has_water, in_block_tag, relative, same_block,
};
use crate::context::Level;

/// The rules for staying, by the classes that have one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Survival {
    Always,
    /// A plant on a block of this tag (`VegetationBlock.mayPlaceOn`).
    OnTag(&'static str),
    /// A stem, a nether fungus or nether roots: on the tag its block's name gives.
    OnTagByName,
    Mushroom,
    SugarCane,
    Cactus,
    CactusFlower,
    MangrovePropagule,
    Seagrass,
    TallSeagrass,
    LilyPad,
    Frogspawn,
    SeaPickle,
    /// The lower half of a two-block plant on the tag, the upper half on the lower.
    DoublePlant(&'static str),
    SmallDripleaf,
    BigDripleaf,
    /// Anything but air below: carpets.
    NotOnAir,
    MossyCarpet,
    /// A face of this support on top of the block below.
    OnTop(SupportType),
    PressurePlate,
    OnSolid,
    BehindSolid,
    BehindSturdy,
    Hanging,
    SporeBlossom,
    FaceAttached,
    Lantern,
    SnowLayer,
    Door,
    RedstoneWire,
    Multiface,
    Fire,
    SoulFire,
    /// A class with a `canSurvive` that is not ported: the block stays nowhere.
    NotPorted,
}

fn rule(state: BlockState) -> Option<Survival> {
    let info = state.block_info()?;
    if !info.survival_rule {
        return None;
    }
    Some(
        std::iter::once(info.class)
            .chain(info.class.ancestors().iter().copied())
            .find_map(class_rule)
            .unwrap_or(Survival::NotPorted),
    )
}

fn class_rule(class: BlockClass) -> Option<Survival> {
    use BlockClass as C;
    use Survival as S;
    Some(match class {
        C::GrindstoneBlock => S::Always,
        C::DryVegetationBlock => S::OnTag("minecraft:supports_dry_vegetation"),
        C::AzaleaBlock => S::OnTag("minecraft:supports_azalea"),
        C::WitherRoseBlock => S::OnTag("minecraft:supports_wither_rose"),
        C::NetherSproutsBlock => S::OnTag("minecraft:supports_nether_sprouts"),
        C::NetherWartBlock => S::OnTag("minecraft:supports_nether_wart"),
        C::CropBlock => S::OnTag("minecraft:supports_crops"),
        C::StemBlock | C::NetherFungusBlock | C::NetherRootsBlock => S::OnTagByName,
        C::MushroomBlock => S::Mushroom,
        C::CactusFlowerBlock => S::CactusFlower,
        C::MangrovePropaguleBlock => S::MangrovePropagule,
        C::SeagrassBlock => S::Seagrass,
        C::TallSeagrassBlock => S::TallSeagrass,
        C::LilyPadBlock => S::LilyPad,
        C::SeaPickleBlock => S::SeaPickle,
        C::LeafLitterBlock => S::OnTop(SupportType::Full),
        C::PitcherCropBlock => S::DoublePlant("minecraft:supports_crops"),
        C::SmallDripleafBlock => S::SmallDripleaf,
        C::DoublePlantBlock => S::DoublePlant("minecraft:supports_vegetation"),
        C::VegetationBlock => S::OnTag("minecraft:supports_vegetation"),
        C::FrogspawnBlock => S::Frogspawn,
        C::BambooSaplingBlock | C::BambooStalkBlock => S::OnTag("minecraft:supports_bamboo"),
        C::BigDripleafBlock => S::BigDripleaf,
        C::SugarCaneBlock => S::SugarCane,
        C::CactusBlock => S::Cactus,
        C::MossyCarpetBlock => S::MossyCarpet,
        C::CarpetBlock => S::NotOnAir,
        C::BaseRailBlock | C::DiodeBlock => S::OnTop(SupportType::Rigid),
        C::BasePressurePlateBlock => S::PressurePlate,
        C::WallTorchBlock | C::RedstoneWallTorchBlock | C::LadderBlock => S::BehindSturdy,
        C::BaseTorchBlock | C::CandleBlock => S::OnTop(SupportType::Center),
        C::StandingSignBlock | C::BannerBlock => S::OnSolid,
        C::WallSignBlock | C::WallBannerBlock => S::BehindSolid,
        C::CeilingHangingSignBlock => S::Hanging,
        C::SporeBlossomBlock => S::SporeBlossom,
        C::FaceAttachedHorizontalDirectionalBlock => S::FaceAttached,
        C::LanternBlock => S::Lantern,
        C::SnowLayerBlock => S::SnowLayer,
        C::DoorBlock => S::Door,
        C::RedstoneWireBlock => S::RedstoneWire,
        C::MultifaceBlock => S::Multiface,
        C::BaseCoralWallFanBlock => S::BehindSturdy,
        // Coral plants and fans: a sturdy top below (`BaseCoralPlantTypeBlock`).
        C::BaseCoralPlantTypeBlock => S::OnTop(SupportType::Full),
        C::SoulFireBlock => S::SoulFire,
        C::FireBlock => S::Fire,
        _ => return None,
    })
}

/// Whether `state` can stay at `pos` (`BlockState.canSurvive`).
pub fn can_survive(state: BlockState, pos: BlockPos, level: &mut Level<'_>) -> bool {
    let Some(rule) = rule(state) else {
        return true;
    };
    let below_pos = relative(pos, Face::Down);
    let below = level.block(below_pos);
    match rule {
        Survival::Always => true,
        Survival::NotPorted => false,
        Survival::OnTag(tag) => in_block_tag(below, tag),
        Survival::OnTagByName => tag_by_name(state).is_some_and(|tag| in_block_tag(below, tag)),
        Survival::Mushroom => {
            // The game asks for less light than 13; nothing is lit while features are
            // placed, and the game's region answers 0 then (the plan, section 3).
            in_block_tag(below, "minecraft:overrides_mushroom_light_requirement")
                || below.props().is_solid_render()
        }
        Survival::SugarCane => sugar_cane(below_pos, below, level),
        Survival::Cactus => cactus(pos, below, level),
        Survival::CactusFlower => {
            in_block_tag(below, "minecraft:support_override_cactus_flower")
                || sturdy(below, Face::Up, SupportType::Center)
        }
        Survival::MangrovePropagule => {
            if state.property("hanging") == Some("true") {
                let above = level.block(relative(pos, Face::Up));
                in_block_tag(above, "minecraft:supports_hanging_mangrove_propagule")
            } else {
                in_block_tag(below, "minecraft:supports_mangrove_propagule")
            }
        }
        Survival::Seagrass => seagrass_ground(below),
        Survival::TallSeagrass => {
            if state.property("half") == Some("upper") {
                same_block(below, state) && below.property("half") == Some("lower")
            } else {
                let here = level.block(pos);
                seagrass_ground(below)
                    && has_water(here)
                    && here.props().fluid().is_some_and(|fluid| fluid.amount == 8)
            }
        }
        Survival::LilyPad => {
            let here = level.block(pos);
            on_water(below, here, "minecraft:supports_lily_pad")
        }
        Survival::Frogspawn => {
            let here = level.block(pos);
            on_water(below, here, "minecraft:supports_frogspawn")
        }
        Survival::SeaPickle => {
            below
                .props()
                .collision_shape()
                .boxes()
                .any(|b| b[4] >= 1.0 && b[0] < b[3] && b[1] < b[4] && b[2] < b[5])
                || sturdy(below, Face::Up, SupportType::Full)
        }
        Survival::DoublePlant(tag) => {
            if state.property("half") == Some("upper") {
                same_block(below, state) && below.property("half") == Some("lower")
            } else {
                in_block_tag(below, tag)
            }
        }
        Survival::SmallDripleaf => {
            if state.property("half") == Some("upper") {
                same_block(below, state) && below.property("half") == Some("lower")
            } else {
                let here = level.block(pos);
                in_block_tag(below, "minecraft:supports_small_dripleaf")
                    || has_water(here)
                        && here.props().fluid().is_some_and(|fluid| fluid.amount == 8)
                        && in_block_tag(below, "minecraft:supports_vegetation")
            }
        }
        Survival::BigDripleaf => {
            same_block(below, state)
                || same_block(below, blocks::BIG_DRIPLEAF_STEM)
                || in_block_tag(below, "minecraft:supports_big_dripleaf")
        }
        Survival::NotOnAir => !below.props().is_air(),
        Survival::MossyCarpet => {
            if state.property("bottom") == Some("true") {
                !below.props().is_air()
            } else {
                same_block(below, state) && below.property("bottom") == Some("true")
            }
        }
        Survival::OnTop(support) => sturdy(below, Face::Up, support),
        Survival::PressurePlate => {
            sturdy(below, Face::Up, SupportType::Rigid)
                || sturdy(below, Face::Up, SupportType::Center)
        }
        Survival::OnSolid => below.props().is_solid(),
        Survival::BehindSolid | Survival::BehindSturdy => {
            let Some(facing) = state.property("facing").and_then(crate::blocks::face_named) else {
                return false;
            };
            let behind = level.block(relative(pos, facing.opposite()));
            if rule == Survival::BehindSolid {
                behind.props().is_solid()
            } else {
                sturdy(behind, facing, SupportType::Full)
            }
        }
        Survival::Hanging => sturdy(
            level.block(relative(pos, Face::Up)),
            Face::Down,
            SupportType::Center,
        ),
        Survival::SporeBlossom => {
            let above = level.block(relative(pos, Face::Up));
            let here = level.block(pos);
            sturdy(above, Face::Down, SupportType::Center) && !has_water(here)
        }
        Survival::FaceAttached => {
            let towards = match state.property("face") {
                Some("ceiling") => Some(Face::Up),
                Some("floor") => Some(Face::Down),
                _ => state
                    .property("facing")
                    .and_then(crate::blocks::face_named)
                    .map(Face::opposite),
            };
            let Some(towards) = towards else {
                return false;
            };
            sturdy(
                level.block(relative(pos, towards)),
                towards.opposite(),
                SupportType::Full,
            )
        }
        Survival::Lantern => {
            let towards = if state.property("hanging") == Some("true") {
                Face::Up
            } else {
                Face::Down
            };
            sturdy(
                level.block(relative(pos, towards)),
                towards.opposite(),
                SupportType::Center,
            )
        }
        Survival::SnowLayer => snow_layer(below),
        Survival::Door => {
            if state.property("half") == Some("upper") {
                same_block(below, state)
            } else {
                sturdy(below, Face::Up, SupportType::Full)
            }
        }
        Survival::RedstoneWire => {
            sturdy(below, Face::Up, SupportType::Full) || same_block(below, blocks::HOPPER)
        }
        Survival::Multiface => {
            let mut any = false;
            for face in Face::ALL {
                if state.property(face_name(face)) == Some("true") {
                    if !multiface_can_attach(level.block(relative(pos, face)), face) {
                        return false;
                    }
                    any = true;
                }
            }
            any
        }
        // Fire also stays beside a block that burns (`isValidFireLocation`). Which
        // blocks burn is not in Clustine's data; the features that place fire put it
        // on netherrack, whose top holds it.
        Survival::Fire => sturdy(below, Face::Up, SupportType::Full),
        Survival::SoulFire => in_block_tag(below, "minecraft:soul_fire_base_blocks"),
    }
}

/// Whether `state` offers `support` on its side `face`.
pub fn sturdy(state: BlockState, face: Face, support: SupportType) -> bool {
    state.props().is_face_sturdy(face, support)
}

/// `MultifaceBlock.canAttachTo`: whether the side of `neighbour` that faces back across
/// `face` is full, by its support or its collision.
pub fn multiface_can_attach(neighbour: BlockState, face: Face) -> bool {
    let facing = face.opposite();
    if sturdy(neighbour, facing, SupportType::Full) {
        return true;
    }
    let collision: Vec<[f64; 6]> = neighbour.props().collision_shape().boxes().collect();
    faces::face_full(&collision, facing)
}

fn seagrass_ground(below: BlockState) -> bool {
    sturdy(below, Face::Up, SupportType::Full)
        && !in_block_tag(below, "minecraft:cannot_support_seagrass")
}

/// The tag a stem, fungus or roots grows on, from the block's name.
fn tag_by_name(state: BlockState) -> Option<&'static str> {
    let name = state.block()?.name;
    Some(match name {
        "minecraft:pumpkin_stem" => "minecraft:supports_pumpkin_stem",
        "minecraft:melon_stem" => "minecraft:supports_melon_stem",
        "minecraft:crimson_fungus" => "minecraft:supports_crimson_fungus",
        "minecraft:warped_fungus" => "minecraft:supports_warped_fungus",
        "minecraft:crimson_roots" => "minecraft:supports_crimson_roots",
        "minecraft:warped_roots" => "minecraft:supports_warped_roots",
        _ => return None,
    })
}

/// A lily pad or frogspawn: on a fluid or block of the tag, with no fluid where it is.
fn on_water(below: BlockState, here: BlockState, tag: &str) -> bool {
    (fluid_in_tag(below, tag) || in_block_tag(below, tag)) && here.props().fluid().is_none()
}

/// Sugar cane: on sugar cane, or on its ground with water or the like beside that.
fn sugar_cane(below_pos: BlockPos, below: BlockState, level: &mut Level<'_>) -> bool {
    if same_block(below, blocks::SUGAR_CANE) {
        return true;
    }
    if !in_block_tag(below, "minecraft:supports_sugar_cane") {
        return false;
    }
    HORIZONTAL.into_iter().any(|face| {
        let beside = level.block(relative(below_pos, face));
        in_block_tag(beside, "minecraft:supports_sugar_cane_adjacently")
            || fluid_in_tag(beside, "minecraft:supports_sugar_cane_adjacently")
    })
}

/// A cactus: nothing solid and no lava beside it, cactus or its ground below, and no
/// liquid above.
fn cactus(pos: BlockPos, below: BlockState, level: &mut Level<'_>) -> bool {
    for face in HORIZONTAL {
        let beside = level.block(relative(pos, face));
        if beside.props().is_solid() || fluid_in_tag(beside, "minecraft:lava") {
            return false;
        }
    }
    (same_block(below, blocks::CACTUS) || in_block_tag(below, "minecraft:supports_cactus"))
        && !level.block(relative(pos, Face::Up)).props().is_liquid()
}

/// A snow layer: not on ice, and on a full top, a block that holds snow anyway, or
/// eight layers of snow.
fn snow_layer(below: BlockState) -> bool {
    if in_block_tag(below, "minecraft:cannot_support_snow_layer") {
        return false;
    }
    if in_block_tag(below, "minecraft:support_override_snow_layer") {
        return true;
    }
    let collision: Vec<[f64; 6]> = below.props().collision_shape().boxes().collect();
    faces::face_full(&collision, Face::Up)
        || same_block(below, blocks::SNOW) && below.property("layers") == Some("8")
}

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
    fn stone_stays_anywhere_and_a_mushroom_on_any_full_opaque_block() {
        let mut world = World::flat(blocks::STONE, 64);
        let at = BlockPos::new(0, 65, 0);
        assert!(world.with_level(|level| can_survive(
            blocks::STONE,
            BlockPos::new(0, 200, 0),
            level
        )));
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
