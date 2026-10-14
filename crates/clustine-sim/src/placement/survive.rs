// Adapted from SteelMC's `canSurvive` of the blocks under
// `steel-core/src/behavior/blocks/` (vegetation, building, decoration, redstone).

//! Vanilla's `canSurvive`: whether a block can stay where it is placed, by its class
//! (section 4.3). A block whose class has a `canSurvive` that is not ported here places
//! nothing.

use clustine_data::Face as Dir;
use clustine_data::{BlockClass, BlockState, SupportType, blocks};
use clustine_world::BlockPos;

use super::context::World;
use super::data;
use super::geometry::{self, DirExt};

/// The rules for staying, by the classes that have one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Survival {
    /// The grindstone's, which overrides its family's with "anywhere".
    Always,
    /// A plant on a block of this tag (vanilla's `VegetationBlock.mayPlaceOn`).
    OnTag(&'static str),
    /// A stem, a nether fungus or nether roots: on the tag its block's name gives.
    OnTagByName,
    Mushroom,
    SugarCane,
    Cactus,
    CactusFlower,
    MangrovePropagule,
    Seagrass,
    LilyPad,
    Frogspawn,
    SeaPickle,
    /// The lower half of a two-block plant on the tag, the upper half on the lower.
    DoublePlant(&'static str),
    SmallDripleaf,
    BigDripleaf,
    /// Anything but air below: carpets.
    NotOnAir,
    /// A face of this support on top of the block below.
    OnTop(SupportType),
    /// A pressure plate: a rigid or a central top below.
    PressurePlate,
    /// A solid block below, vanilla's deprecated `isSolid`: standing signs and banners.
    OnSolid,
    /// A solid block behind, against which the block faces away: wall signs and
    /// banners.
    BehindSolid,
    /// A sturdy face behind: wall torches, ladders and the wall fans of coral.
    BehindSturdy,
    /// Below a block that holds it at the centre of its bottom.
    Hanging,
    FaceAttached,
    Lantern,
    SnowLayer,
    Door,
    RedstoneWire,
    Multiface,
    Scaffolding,
    /// A class with a `canSurvive` that is not ported: the block places nothing.
    NotPorted,
}

/// The rule of the nearest class that has one, for a block whose class says where it
/// can stay; `None` for a block that can stay anywhere.
pub(crate) fn survival_rule(state: BlockState) -> Option<Survival> {
    let info = data::info(state);
    if !info.survival_rule {
        return None;
    }
    Some(data::nearest(info, class_rule).unwrap_or(Survival::NotPorted))
}

fn class_rule(class: BlockClass) -> Option<Survival> {
    use BlockClass as C;
    use Survival as S;
    Some(match class {
        C::GrindstoneBlock => S::Always,
        // The plants: vanilla's `VegetationBlock` and what is made from it.
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
        C::LilyPadBlock => S::LilyPad,
        C::SeaPickleBlock => S::SeaPickle,
        C::LeafLitterBlock => S::OnTop(SupportType::Full),
        C::PitcherCropBlock => S::DoublePlant("minecraft:supports_crops"),
        C::SmallDripleafBlock => S::SmallDripleaf,
        C::DoublePlantBlock => S::DoublePlant("minecraft:supports_vegetation"),
        // A stem attached to its fruit and tall seagrass come from no item; their own
        // rules are not ported.
        C::AttachedStemBlock | C::TallSeagrassBlock => S::NotPorted,
        C::VegetationBlock => S::OnTag("minecraft:supports_vegetation"),
        C::FrogspawnBlock => S::Frogspawn,
        C::BambooSaplingBlock | C::BambooStalkBlock => S::OnTag("minecraft:supports_bamboo"),
        C::BigDripleafBlock => S::BigDripleaf,
        C::SugarCaneBlock => S::SugarCane,
        C::CactusBlock => S::Cactus,
        C::CarpetBlock => S::NotOnAir,
        C::BaseRailBlock | C::DiodeBlock => S::OnTop(SupportType::Rigid),
        C::BasePressurePlateBlock => S::PressurePlate,
        C::WallTorchBlock | C::RedstoneWallTorchBlock | C::LadderBlock => S::BehindSturdy,
        C::BaseTorchBlock | C::CandleBlock => S::OnTop(SupportType::Center),
        C::StandingSignBlock | C::BannerBlock => S::OnSolid,
        C::WallSignBlock | C::WallBannerBlock => S::BehindSolid,
        C::CeilingHangingSignBlock => S::Hanging,
        C::FaceAttachedHorizontalDirectionalBlock => S::FaceAttached,
        C::LanternBlock => S::Lantern,
        C::SnowLayerBlock => S::SnowLayer,
        C::DoorBlock => S::Door,
        C::RedstoneWireBlock => S::RedstoneWire,
        C::MultifaceBlock => S::Multiface,
        C::ScaffoldingBlock => S::Scaffolding,
        C::BaseCoralWallFanBlock => S::BehindSturdy,
        C::BaseCoralFanBlock => S::OnTop(SupportType::Full),
        _ => return None,
    })
}

/// Whether `state` can stay at `pos`.
pub(crate) fn can_survive(state: BlockState, pos: BlockPos, world: &World) -> bool {
    let Some(rule) = survival_rule(state) else {
        return true;
    };
    let below_pos = Dir::Down.relative(pos);
    let below = world.get(below_pos);
    match rule {
        Survival::Always => true,
        Survival::NotPorted => false,
        Survival::OnTag(tag) => data::in_tag(below, tag),
        Survival::OnTagByName => tag_by_name(state).is_some_and(|tag| data::in_tag(below, tag)),
        Survival::Mushroom => {
            // The light a mushroom needs is not judged (section 4.3).
            data::in_tag(below, "minecraft:overrides_mushroom_light_requirement")
                || below.props().is_solid_render()
        }
        Survival::SugarCane => sugar_cane(below_pos, below, world),
        Survival::Cactus => cactus(pos, below, world),
        Survival::CactusFlower => {
            data::in_tag(below, "minecraft:support_override_cactus_flower")
                || sturdy(below, Dir::Up, SupportType::Center)
        }
        Survival::MangrovePropagule => {
            if data::flag(state, "hanging") {
                data::in_tag(
                    world.get(Dir::Up.relative(pos)),
                    "minecraft:supports_hanging_mangrove_propagule",
                )
            } else {
                data::in_tag(below, "minecraft:supports_mangrove_propagule")
            }
        }
        Survival::Seagrass => {
            sturdy(below, Dir::Up, SupportType::Full)
                && !data::in_tag(below, "minecraft:cannot_support_seagrass")
        }
        Survival::LilyPad => on_water(below, world.get(pos), "minecraft:supports_lily_pad"),
        Survival::Frogspawn => on_water(below, world.get(pos), "minecraft:supports_frogspawn"),
        Survival::SeaPickle => {
            below
                .props()
                .collision_shape()
                .boxes()
                .any(|b| b[4] >= 1.0 && b[0] < b[3] && b[1] < b[4] && b[2] < b[5])
                || sturdy(below, Dir::Up, SupportType::Full)
        }
        Survival::DoublePlant(tag) => {
            if state.property("half") == Some("upper") {
                data::same_block(below, state) && below.property("half") == Some("lower")
            } else {
                data::in_tag(below, tag)
            }
        }
        Survival::SmallDripleaf => {
            if state.property("half") == Some("upper") {
                data::same_block(below, state) && below.property("half") == Some("lower")
            } else {
                let water_above_ground = world.get(pos);
                data::in_tag(below, "minecraft:supports_small_dripleaf")
                    || data::has_water(water_above_ground)
                        && water_above_ground
                            .props()
                            .fluid()
                            .is_some_and(|fluid| fluid.amount == 8)
                        && data::in_tag(below, "minecraft:supports_vegetation")
            }
        }
        Survival::BigDripleaf => {
            data::same_block(below, state)
                || data::same_block(below, blocks::BIG_DRIPLEAF_STEM)
                || data::in_tag(below, "minecraft:supports_big_dripleaf")
        }
        Survival::NotOnAir => !below.props().is_air(),
        Survival::OnTop(support) => sturdy(below, Dir::Up, support),
        Survival::PressurePlate => {
            sturdy(below, Dir::Up, SupportType::Rigid)
                || sturdy(below, Dir::Up, SupportType::Center)
        }
        Survival::OnSolid => below.props().is_solid(),
        Survival::BehindSolid | Survival::BehindSturdy => {
            let Some(facing) = facing(state) else {
                return false;
            };
            let behind = world.get(facing.opposite().relative(pos));
            if rule == Survival::BehindSolid {
                behind.props().is_solid()
            } else {
                sturdy(behind, facing, SupportType::Full)
            }
        }
        Survival::Hanging => sturdy(
            world.get(Dir::Up.relative(pos)),
            Dir::Down,
            SupportType::Center,
        ),
        Survival::FaceAttached => {
            let Some(towards) = attached_towards(state) else {
                return false;
            };
            sturdy(
                world.get(towards.relative(pos)),
                towards.opposite(),
                SupportType::Full,
            )
        }
        Survival::Lantern => {
            let towards = if data::flag(state, "hanging") {
                Dir::Up
            } else {
                Dir::Down
            };
            sturdy(
                world.get(towards.relative(pos)),
                towards.opposite(),
                SupportType::Center,
            )
        }
        Survival::SnowLayer => snow_layer(below),
        Survival::Door => {
            if state.property("half") == Some("upper") {
                data::same_block(below, state)
            } else {
                sturdy(below, Dir::Up, SupportType::Full)
            }
        }
        Survival::RedstoneWire => wire_can_stand_on(below),
        Survival::Multiface => {
            let mut any = false;
            for dir in Dir::ALL {
                if data::flag(state, dir.name()) {
                    if !multiface_can_attach(world.get(dir.relative(pos)), dir) {
                        return false;
                    }
                    any = true;
                }
            }
            any
        }
        Survival::Scaffolding => scaffolding_distance(pos, world) < SCAFFOLDING_MAX_DISTANCE,
    }
}

/// Whether `state` offers `support` on its side `face`.
pub(crate) fn sturdy(state: BlockState, face: Dir, support: SupportType) -> bool {
    state.props().is_face_sturdy(face, support)
}

/// The value of a horizontal `facing` property.
pub(crate) fn facing(state: BlockState) -> Option<Dir> {
    state.property("facing").and_then(Dir::from_name)
}

/// The side of a button, lever or grindstone its support is at.
fn attached_towards(state: BlockState) -> Option<Dir> {
    match state.property("face")? {
        "ceiling" => Some(Dir::Up),
        "floor" => Some(Dir::Down),
        _ => facing(state).map(Dir::opposite),
    }
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
    (data::fluid_in_tag(below, tag) || data::in_tag(below, tag)) && here.props().fluid().is_none()
}

/// Sugar cane: on sugar cane, or on its ground with water or the like beside that.
fn sugar_cane(below_pos: BlockPos, below: BlockState, world: &World) -> bool {
    if data::same_block(below, blocks::SUGAR_CANE) {
        return true;
    }
    if !data::in_tag(below, "minecraft:supports_sugar_cane") {
        return false;
    }
    [Dir::North, Dir::South, Dir::East, Dir::West]
        .into_iter()
        .any(|dir| {
            let beside = world.get(dir.relative(below_pos));
            data::in_tag(beside, "minecraft:supports_sugar_cane_adjacently")
                || data::fluid_in_tag(beside, "minecraft:supports_sugar_cane_adjacently")
        })
}

/// A cactus: nothing solid and no lava beside it, cactus or its ground below, and no
/// liquid above.
fn cactus(pos: BlockPos, below: BlockState, world: &World) -> bool {
    for dir in [Dir::North, Dir::South, Dir::East, Dir::West] {
        let beside = world.get(dir.relative(pos));
        if beside.props().is_solid() || data::has_lava(beside) {
            return false;
        }
    }
    (data::same_block(below, blocks::CACTUS) || data::in_tag(below, "minecraft:supports_cactus"))
        && !world.get(Dir::Up.relative(pos)).props().is_liquid()
}

/// A snow layer: not on ice, and on a full top, a block that holds snow anyway, or
/// eight layers of snow.
fn snow_layer(below: BlockState) -> bool {
    if data::in_tag(below, "minecraft:cannot_support_snow_layer") {
        return false;
    }
    if data::in_tag(below, "minecraft:support_override_snow_layer") {
        return true;
    }
    let collision: Vec<[f64; 6]> = below.props().collision_shape().boxes().collect();
    geometry::face_full(&collision, Dir::Up)
        || data::same_block(below, blocks::SNOW) && data::number(below, "layers") == Some(8)
}

/// Whether redstone wire stays on `below`: a sturdy top, or a hopper.
pub(crate) fn wire_can_stand_on(below: BlockState) -> bool {
    sturdy(below, Dir::Up, SupportType::Full) || data::same_block(below, blocks::HOPPER)
}

/// Whether a face of glow lichen or the like can hold on to `neighbour`, which lies on
/// the side `dir` of it: the neighbour's facing side is full, by its support or its
/// collision.
pub(crate) fn multiface_can_attach(neighbour: BlockState, dir: Dir) -> bool {
    let facing = dir.opposite();
    if sturdy(neighbour, facing, SupportType::Full) {
        return true;
    }
    let collision: Vec<[f64; 6]> = neighbour.props().collision_shape().boxes().collect();
    geometry::face_full(&collision, facing)
}

/// How far scaffolding can reach from what holds it.
pub(crate) const SCAFFOLDING_MAX_DISTANCE: u8 = 7;

/// Vanilla's `ScaffoldingBlock.getDistance`: how many blocks of scaffolding lie between
/// `pos` and a block that holds it up.
pub(crate) fn scaffolding_distance(pos: BlockPos, world: &World) -> u8 {
    let below_pos = Dir::Down.relative(pos);
    let below = world.get(below_pos);
    let mut distance = SCAFFOLDING_MAX_DISTANCE;
    if data::same_block(below, blocks::SCAFFOLDING) {
        distance = data::number(below, "distance").unwrap_or(SCAFFOLDING_MAX_DISTANCE);
    } else if sturdy(below, Dir::Up, SupportType::Full) {
        return 0;
    }
    for dir in geometry::HORIZONTAL {
        let beside = world.get(dir.relative(pos));
        if data::same_block(beside, blocks::SCAFFOLDING) {
            let theirs = data::number(beside, "distance").unwrap_or(SCAFFOLDING_MAX_DISTANCE);
            distance = distance.min(theirs + 1);
            if distance == 1 {
                break;
            }
        }
    }
    distance
}
