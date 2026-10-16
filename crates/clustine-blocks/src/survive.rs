// Adapted from SteelMC's `canSurvive` of the blocks under
// `steel-core/src/behavior/blocks/` (vegetation, building, decoration, redstone), by
// way of clustine-sim's `src/placement/survive.rs` and clustine-features'
// `src/survive.rs`, which were made one here.

//! The game's `canSurvive`: whether a block can stay where it is, by its class. A
//! block whose class has a `canSurvive` that is not ported stays nowhere, so that a gap
//! shows as a block missing and not as one misplaced: a player cannot place it, and a
//! feature does not place it.

use clustine_data::{BlockClass, BlockState, Face, SupportType, blocks, faces};
use clustine_world::BlockPos;

use crate::data::{
    HORIZONTAL, face_name, facing, fluid_in_tag, has_lava, has_water, in_tag, number, relative,
    same_block,
};
use crate::{BlockView, Rules};

/// The rules for staying, by the classes that have one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Survival {
    /// The grindstone's, which overrides its family's with "anywhere".
    Always,
    /// A plant on a block of this tag (the game's `VegetationBlock.mayPlaceOn`).
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
    /// A pressure plate: a rigid or a central top below.
    PressurePlate,
    /// A solid block below, the game's deprecated `isSolid`: standing signs and banners.
    OnSolid,
    /// A solid block behind, against which the block faces away: wall signs and
    /// banners.
    BehindSolid,
    /// A sturdy face behind: wall torches, ladders and the wall fans of coral.
    BehindSturdy,
    /// Below a block that holds it at the centre of its bottom.
    Hanging,
    SporeBlossom,
    FaceAttached,
    Lantern,
    SnowLayer,
    Door,
    RedstoneWire,
    Multiface,
    Scaffolding,
    Fire,
    SoulFire,
    /// A class with a `canSurvive` that is not ported: the block stays nowhere.
    NotPorted,
}

/// The rule of the nearest class that has one, for a block whose class says where it
/// can stay; `None` for a block that can stay anywhere. Where rules for two classes
/// could match a block, the one for the nearer class wins.
pub fn survival_rule(state: BlockState, rules: Rules) -> Option<Survival> {
    let info = state.block_info()?;
    if !info.survival_rule {
        return None;
    }
    Some(
        std::iter::once(info.class)
            .chain(info.class.ancestors().iter().copied())
            .find_map(|class| class_rule(class, rules))
            .unwrap_or(Survival::NotPorted),
    )
}

/// The rule of one class.
///
/// The arms with a guard are where the two former copies differed, one having a rule
/// for the class and the other not (it fell to a class further up, or said "not
/// ported"). Each caller keeps its own answer until the game judges between them; a
/// rule taken for placing lets a player place a block that placed nothing before.
fn class_rule(class: BlockClass, rules: Rules) -> Option<Survival> {
    use BlockClass as C;
    use Survival as S;
    let generation = rules == Rules::Generation;
    let placing = rules == Rules::Placing;
    Some(match class {
        C::GrindstoneBlock => S::Always,
        // The plants: the game's `VegetationBlock` and what is made from it.
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
        // Tall seagrass comes from no item; placing never ported its rule.
        C::TallSeagrassBlock if generation => S::TallSeagrass,
        C::TallSeagrassBlock => S::NotPorted,
        C::LilyPadBlock => S::LilyPad,
        C::SeaPickleBlock => S::SeaPickle,
        C::LeafLitterBlock => S::OnTop(SupportType::Full),
        C::PitcherCropBlock => S::DoublePlant("minecraft:supports_crops"),
        C::SmallDripleafBlock => S::SmallDripleaf,
        C::DoublePlantBlock => S::DoublePlant("minecraft:supports_vegetation"),
        // A stem attached to its fruit comes from no item; placing never ported its
        // rule, and generation takes the plant's it is made from.
        C::AttachedStemBlock if placing => S::NotPorted,
        C::VegetationBlock => S::OnTag("minecraft:supports_vegetation"),
        C::FrogspawnBlock => S::Frogspawn,
        C::BambooSaplingBlock | C::BambooStalkBlock => S::OnTag("minecraft:supports_bamboo"),
        C::BigDripleafBlock => S::BigDripleaf,
        C::SugarCaneBlock => S::SugarCane,
        C::CactusBlock => S::Cactus,
        C::MossyCarpetBlock if generation => S::MossyCarpet,
        C::CarpetBlock => S::NotOnAir,
        C::BaseRailBlock | C::DiodeBlock => S::OnTop(SupportType::Rigid),
        C::BasePressurePlateBlock => S::PressurePlate,
        C::WallTorchBlock | C::RedstoneWallTorchBlock | C::LadderBlock => S::BehindSturdy,
        C::BaseTorchBlock | C::CandleBlock => S::OnTop(SupportType::Center),
        C::StandingSignBlock | C::BannerBlock => S::OnSolid,
        C::WallSignBlock | C::WallBannerBlock => S::BehindSolid,
        C::CeilingHangingSignBlock => S::Hanging,
        C::SporeBlossomBlock if generation => S::SporeBlossom,
        C::FaceAttachedHorizontalDirectionalBlock => S::FaceAttached,
        C::LanternBlock => S::Lantern,
        C::SnowLayerBlock => S::SnowLayer,
        C::DoorBlock => S::Door,
        C::RedstoneWireBlock => S::RedstoneWire,
        C::MultifaceBlock => S::Multiface,
        C::ScaffoldingBlock if placing => S::Scaffolding,
        C::BaseCoralWallFanBlock => S::BehindSturdy,
        C::BaseCoralFanBlock => S::OnTop(SupportType::Full),
        // Coral plants and fans: a sturdy top below (`BaseCoralPlantTypeBlock`).
        C::BaseCoralPlantTypeBlock if generation => S::OnTop(SupportType::Full),
        C::SoulFireBlock if generation => S::SoulFire,
        C::FireBlock if generation => S::Fire,
        _ => return None,
    })
}

/// Whether `state` can stay at `pos` (`BlockState.canSurvive`).
pub fn can_survive<V: BlockView + ?Sized>(state: BlockState, pos: BlockPos, view: &mut V) -> bool {
    let Some(rule) = survival_rule(state, view.rules()) else {
        return true;
    };
    let below_pos = relative(pos, Face::Down);
    let below = view.block(below_pos);
    match rule {
        Survival::Always => true,
        Survival::NotPorted => false,
        Survival::OnTag(tag) => in_tag(below, tag),
        Survival::OnTagByName => tag_by_name(state).is_some_and(|tag| in_tag(below, tag)),
        Survival::Mushroom => {
            // The game asks for less light than 13 where the block below does not
            // override it. Where the caller has no light it is not judged. While
            // features are placed the sky reads 15 everywhere, so there only the
            // blocks that override it hold a mushroom.
            in_tag(below, "minecraft:overrides_mushroom_light_requirement")
                || (view.raw_brightness(pos).is_none_or(|light| light < 13)
                    && below.props().is_solid_render())
        }
        Survival::SugarCane => sugar_cane(below_pos, below, view),
        Survival::Cactus => cactus(pos, below, view),
        Survival::CactusFlower => {
            in_tag(below, "minecraft:support_override_cactus_flower")
                || sturdy(below, Face::Up, SupportType::Center)
        }
        Survival::MangrovePropagule => {
            if state.property("hanging") == Some("true") {
                let above = view.block(relative(pos, Face::Up));
                in_tag(above, "minecraft:supports_hanging_mangrove_propagule")
            } else {
                in_tag(below, "minecraft:supports_mangrove_propagule")
            }
        }
        Survival::Seagrass => seagrass_ground(below),
        Survival::TallSeagrass => {
            if state.property("half") == Some("upper") {
                same_block(below, state) && below.property("half") == Some("lower")
            } else {
                let here = view.block(pos);
                seagrass_ground(below) && still_water(here)
            }
        }
        Survival::LilyPad => {
            let here = view.block(pos);
            on_water(below, here, "minecraft:supports_lily_pad")
        }
        Survival::Frogspawn => {
            let here = view.block(pos);
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
                in_tag(below, tag)
            }
        }
        Survival::SmallDripleaf => {
            if state.property("half") == Some("upper") {
                same_block(below, state) && below.property("half") == Some("lower")
            } else {
                let here = view.block(pos);
                in_tag(below, "minecraft:supports_small_dripleaf")
                    || still_water(here) && in_tag(below, "minecraft:supports_vegetation")
            }
        }
        Survival::BigDripleaf => {
            same_block(below, state)
                || same_block(below, blocks::BIG_DRIPLEAF_STEM)
                || in_tag(below, "minecraft:supports_big_dripleaf")
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
            let Some(facing) = facing(state) else {
                return false;
            };
            let behind = view.block(relative(pos, facing.opposite()));
            if rule == Survival::BehindSolid {
                behind.props().is_solid()
            } else {
                sturdy(behind, facing, SupportType::Full)
            }
        }
        Survival::Hanging => sturdy(
            view.block(relative(pos, Face::Up)),
            Face::Down,
            SupportType::Center,
        ),
        Survival::SporeBlossom => {
            let above = view.block(relative(pos, Face::Up));
            let here = view.block(pos);
            sturdy(above, Face::Down, SupportType::Center) && !has_water(here)
        }
        Survival::FaceAttached => {
            let Some(towards) = attached_towards(state) else {
                return false;
            };
            sturdy(
                view.block(relative(pos, towards)),
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
                view.block(relative(pos, towards)),
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
        Survival::RedstoneWire => wire_can_stand_on(below),
        Survival::Multiface => {
            let mut any = false;
            for face in Face::ALL {
                if state.property(face_name(face)) == Some("true") {
                    if !multiface_can_attach(view.block(relative(pos, face)), face) {
                        return false;
                    }
                    any = true;
                }
            }
            any
        }
        Survival::Scaffolding => scaffolding_distance(pos, view) < SCAFFOLDING_MAX_DISTANCE,
        // Fire also stays beside a block that burns (`isValidFireLocation`). Which
        // blocks burn is not in Clustine's data; the features that place fire put it
        // on netherrack, whose top holds it.
        Survival::Fire => sturdy(below, Face::Up, SupportType::Full),
        Survival::SoulFire => in_tag(below, "minecraft:soul_fire_base_blocks"),
    }
}

/// Whether `state` offers `support` on its side `face`.
pub fn sturdy(state: BlockState, face: Face, support: SupportType) -> bool {
    state.props().is_face_sturdy(face, support)
}

/// `MultifaceBlock.canAttachTo`: whether a face of glow lichen or the like can hold on
/// to `neighbour`, which lies on its side `face`: the neighbour's side that faces back
/// is full, by its support or its collision.
pub fn multiface_can_attach(neighbour: BlockState, face: Face) -> bool {
    let facing = face.opposite();
    if sturdy(neighbour, facing, SupportType::Full) {
        return true;
    }
    let collision: Vec<[f64; 6]> = neighbour.props().collision_shape().boxes().collect();
    faces::face_full(&collision, facing)
}

/// Whether redstone wire stays on `below`: a sturdy top, or a hopper.
pub fn wire_can_stand_on(below: BlockState) -> bool {
    sturdy(below, Face::Up, SupportType::Full) || same_block(below, blocks::HOPPER)
}

/// How far scaffolding can reach from what holds it.
pub const SCAFFOLDING_MAX_DISTANCE: u8 = 7;

/// The game's `ScaffoldingBlock.getDistance`: how many blocks of scaffolding lie
/// between `pos` and a block that holds it up.
pub fn scaffolding_distance<V: BlockView + ?Sized>(pos: BlockPos, view: &mut V) -> u8 {
    let below = view.block(relative(pos, Face::Down));
    let mut distance = SCAFFOLDING_MAX_DISTANCE;
    if same_block(below, blocks::SCAFFOLDING) {
        distance = number(below, "distance").unwrap_or(SCAFFOLDING_MAX_DISTANCE);
    } else if sturdy(below, Face::Up, SupportType::Full) {
        return 0;
    }
    for face in HORIZONTAL {
        let beside = view.block(relative(pos, face));
        if same_block(beside, blocks::SCAFFOLDING) {
            let theirs = number(beside, "distance").unwrap_or(SCAFFOLDING_MAX_DISTANCE);
            distance = distance.min(theirs + 1);
            if distance == 1 {
                break;
            }
        }
    }
    distance
}

/// The side of a button, lever or grindstone its support is at.
fn attached_towards(state: BlockState) -> Option<Face> {
    match state.property("face")? {
        "ceiling" => Some(Face::Up),
        "floor" => Some(Face::Down),
        _ => facing(state).map(Face::opposite),
    }
}

fn seagrass_ground(below: BlockState) -> bool {
    sturdy(below, Face::Up, SupportType::Full)
        && !in_tag(below, "minecraft:cannot_support_seagrass")
}

/// Whether `here` holds water to its full height, as tall seagrass and a small
/// dripleaf beside no ground of its own need.
fn still_water(here: BlockState) -> bool {
    has_water(here) && here.props().fluid().is_some_and(|fluid| fluid.amount == 8)
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
    (fluid_in_tag(below, tag) || in_tag(below, tag)) && here.props().fluid().is_none()
}

/// Sugar cane: on sugar cane, or on its ground with water or the like beside that.
fn sugar_cane<V: BlockView + ?Sized>(below_pos: BlockPos, below: BlockState, view: &mut V) -> bool {
    if same_block(below, blocks::SUGAR_CANE) {
        return true;
    }
    if !in_tag(below, "minecraft:supports_sugar_cane") {
        return false;
    }
    HORIZONTAL.into_iter().any(|face| {
        let beside = view.block(relative(below_pos, face));
        in_tag(beside, "minecraft:supports_sugar_cane_adjacently")
            || fluid_in_tag(beside, "minecraft:supports_sugar_cane_adjacently")
    })
}

/// A cactus: nothing solid and no lava beside it, cactus or its ground below, and no
/// liquid above.
fn cactus<V: BlockView + ?Sized>(pos: BlockPos, below: BlockState, view: &mut V) -> bool {
    for face in HORIZONTAL {
        let beside = view.block(relative(pos, face));
        if beside.props().is_solid() || has_lava(beside) {
            return false;
        }
    }
    (same_block(below, blocks::CACTUS) || in_tag(below, "minecraft:supports_cactus"))
        && !view.block(relative(pos, Face::Up)).props().is_liquid()
}

/// A snow layer: not on ice, and on a full top, a block that holds snow anyway, or
/// eight layers of snow.
fn snow_layer(below: BlockState) -> bool {
    if in_tag(below, "minecraft:cannot_support_snow_layer") {
        return false;
    }
    if in_tag(below, "minecraft:support_override_snow_layer") {
        return true;
    }
    let collision: Vec<[f64; 6]> = below.props().collision_shape().boxes().collect();
    faces::face_full(&collision, Face::Up)
        || same_block(below, blocks::SNOW) && number(below, "layers") == Some(8)
}
