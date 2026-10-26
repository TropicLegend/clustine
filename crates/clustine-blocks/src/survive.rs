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
    /// A bell: under a ceiling that holds it at its centre, or against the floor or
    /// the wall its attachment names (`BellBlock.canSurvive`).
    Bell,
    Lantern,
    SnowLayer,
    /// Nothing solid above, or what keeps farmland (`FarmlandBlock.canSurvive`).
    Farmland,
    Door,
    RedstoneWire,
    Multiface,
    Scaffolding,
    Fire,
    SoulFire,
    /// What grows: a growing plant or a speleothem, by `growth.rs`.
    Growing,
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
        // A stem attached to its fruit stays on its stem's ground (`AttachedStemBlock`'s
        // `supportBlocks`), which the simulation's shape updates ask; it comes from no
        // item, so no player places it. Generation takes the plant's it is made from.
        C::AttachedStemBlock if placing => S::OnTagByName,
        C::VegetationBlock => S::OnTag("minecraft:supports_vegetation"),
        C::FrogspawnBlock => S::Frogspawn,
        C::BambooSaplingBlock | C::BambooStalkBlock => S::OnTag("minecraft:supports_bamboo"),
        C::BigDripleafBlock => S::BigDripleaf,
        C::SugarCaneBlock => S::SugarCane,
        // Kelp, the vines that grow and speleothems: the simulation's growth needs
        // them (`growth.rs`); generation never asked.
        C::GrowingPlantBlock if placing => S::Growing,
        C::SpeleothemBlock if placing => S::Growing,
        C::CactusBlock => S::Cactus,
        C::MossyCarpetBlock if generation => S::MossyCarpet,
        C::CarpetBlock => S::NotOnAir,
        C::BaseRailBlock | C::DiodeBlock => S::OnTop(SupportType::Rigid),
        C::BasePressurePlateBlock => S::PressurePlate,
        C::WallTorchBlock | C::RedstoneWallTorchBlock | C::LadderBlock | C::TripWireHookBlock => {
            S::BehindSturdy
        }
        C::BellBlock => S::Bell,
        C::BaseTorchBlock | C::CandleBlock => S::OnTop(SupportType::Center),
        C::StandingSignBlock | C::BannerBlock => S::OnSolid,
        C::WallSignBlock | C::WallBannerBlock => S::BehindSolid,
        C::CeilingHangingSignBlock => S::Hanging,
        C::SporeBlossomBlock if generation => S::SporeBlossom,
        C::FaceAttachedHorizontalDirectionalBlock => S::FaceAttached,
        C::LanternBlock => S::Lantern,
        C::SnowLayerBlock => S::SnowLayer,
        C::FarmlandBlock => S::Farmland,
        C::DoorBlock => S::Door,
        C::RedstoneWireBlock => S::RedstoneWire,
        C::MultifaceBlock => S::Multiface,
        C::ScaffoldingBlock if placing => S::Scaffolding,
        C::BaseCoralWallFanBlock => S::BehindSturdy,
        C::BaseCoralFanBlock => S::OnTop(SupportType::Full),
        // Coral plants and fans: a sturdy top below (`BaseCoralPlantTypeBlock`).
        C::BaseCoralPlantTypeBlock if generation => S::OnTop(SupportType::Full),
        C::SoulFireBlock => S::SoulFire,
        C::FireBlock => S::Fire,
        _ => return None,
    })
}

/// Whether `state` can stay at `pos` (`BlockState.canSurvive`).
pub fn can_survive<V: BlockView + ?Sized>(state: BlockState, pos: BlockPos, view: &mut V) -> bool {
    let Some(rule) = survival_rule(state, view.rules()) else {
        return true;
    };
    // `CropBlock.canSurvive` and the lower half of the pitcher crop's ask for light of 8
    // or more first (`hasSufficientLight`), so that no seed is planted in the dark and a
    // crop whose room went dark breaks at its next shape update. Where the caller has no
    // light it is not judged.
    if needs_light_to_stay(state) && view.raw_brightness(pos).is_some_and(|light| light < 8) {
        return false;
    }
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
        Survival::Bell => {
            // `getConnectedDirection(state).getOpposite()`: the side its support is on.
            let towards = match state.property("attachment") {
                Some("floor") => Face::Down,
                Some("ceiling") => Face::Up,
                _ => match facing(state) {
                    Some(facing) => facing,
                    None => return false,
                },
            };
            let support = if towards == Face::Up {
                SupportType::Center
            } else {
                SupportType::Full
            };
            sturdy(
                view.block(relative(pos, towards)),
                towards.opposite(),
                support,
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
        Survival::Growing => crate::growth::grows_and_can_stay(view, state, pos).unwrap_or(true),
        Survival::Farmland => {
            let above = view.block(relative(pos, Face::Up));
            !above.props().is_solid() || in_tag(above, "minecraft:maintains_farmland")
        }
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
        // Fire also stays beside a block that burns (`isValidFireLocation`). The
        // features that place fire put it on netherrack, whose top holds it, and
        // generation keeps the rule it was judged by; the simulation takes the game's.
        Survival::Fire => {
            sturdy(below, Face::Up, SupportType::Full)
                || view.rules() == Rules::Placing && valid_fire_location(pos, view)
        }
        Survival::SoulFire => in_tag(below, "minecraft:soul_fire_base_blocks"),
    }
}

/// Whether `state` stays only in light of 8 or more: a crop (`CropBlock` and its kin by
/// class) or the lower half of the pitcher crop (`PitcherCropBlock.isLower`).
fn needs_light_to_stay(state: BlockState) -> bool {
    crate::data::is_class(state, BlockClass::CropBlock)
        || crate::data::is_class(state, BlockClass::PitcherCropBlock)
            && state.property("half") == Some("lower")
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
        "minecraft:pumpkin_stem" | "minecraft:attached_pumpkin_stem" => {
            "minecraft:supports_pumpkin_stem"
        }
        "minecraft:melon_stem" | "minecraft:attached_melon_stem" => "minecraft:supports_melon_stem",
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

/// `FireBlock.canBurn`: whether fire takes hold of `state`, which is whether it has
/// odds of catching (`getIgniteOdds` above nought).
pub fn fire_can_burn(state: BlockState) -> bool {
    ignite_odds(state) > 0
}

/// `FireBlock.isValidFireLocation`: whether a block beside `pos`, on any of its six
/// sides, burns.
pub fn valid_fire_location<V: BlockView + ?Sized>(pos: BlockPos, view: &mut V) -> bool {
    Face::ALL
        .into_iter()
        .any(|face| fire_can_burn(view.block(relative(pos, face))))
}

/// The odds that fire beside `state` sets it alight (`FireBlock.getIgniteOdds` of a
/// state): nought for a waterlogged block and for one that does not burn.
pub fn ignite_odds(state: BlockState) -> i32 {
    flammable(state).map_or(0, |(ignite, _)| ignite)
}

/// The odds that fire beside `state` burns it away (`FireBlock.getBurnOdds`): nought
/// for a waterlogged block and for one that does not burn.
pub fn burn_odds(state: BlockState) -> i32 {
    flammable(state).map_or(0, |(_, burn)| burn)
}

fn flammable(state: BlockState) -> Option<(i32, i32)> {
    if state.property("waterlogged") == Some("true") {
        return None;
    }
    let name = state.block()?.name;
    let found = FLAMMABLE
        .binary_search_by(|(entry, _, _)| entry.cmp(&name))
        .ok()?;
    let (_, ignite, burn) = FLAMMABLE[found];
    Some((i32::from(ignite), i32::from(burn)))
}

/// The blocks that burn, with the odds that fire sets each alight and that it burns
/// each away, as `FireBlock.bootStrap` sets them (`setFlammable`), read from the 26.3
/// jar; the colours of wool, woollen stairs and slabs, and carpets are written out.
/// Sorted by name, which a test checks.
pub const FLAMMABLE: &[(&str, u8, u8)] = &[
    ("minecraft:acacia_fence", 5, 20),
    ("minecraft:acacia_fence_gate", 5, 20),
    ("minecraft:acacia_leaves", 30, 60),
    ("minecraft:acacia_log", 5, 5),
    ("minecraft:acacia_planks", 5, 20),
    ("minecraft:acacia_shelf", 30, 20),
    ("minecraft:acacia_slab", 5, 20),
    ("minecraft:acacia_stairs", 5, 20),
    ("minecraft:acacia_wood", 5, 5),
    ("minecraft:allium", 60, 100),
    ("minecraft:azalea", 30, 60),
    ("minecraft:azalea_leaves", 30, 60),
    ("minecraft:azure_bluet", 60, 100),
    ("minecraft:bamboo", 60, 60),
    ("minecraft:bamboo_block", 5, 5),
    ("minecraft:bamboo_fence", 5, 20),
    ("minecraft:bamboo_fence_gate", 5, 20),
    ("minecraft:bamboo_mosaic", 5, 20),
    ("minecraft:bamboo_mosaic_slab", 5, 20),
    ("minecraft:bamboo_mosaic_stairs", 5, 20),
    ("minecraft:bamboo_planks", 5, 20),
    ("minecraft:bamboo_shelf", 30, 20),
    ("minecraft:bamboo_slab", 5, 20),
    ("minecraft:bamboo_stairs", 5, 20),
    ("minecraft:bee_nest", 30, 20),
    ("minecraft:beehive", 5, 20),
    ("minecraft:big_dripleaf", 60, 100),
    ("minecraft:big_dripleaf_stem", 60, 100),
    ("minecraft:birch_fence", 5, 20),
    ("minecraft:birch_fence_gate", 5, 20),
    ("minecraft:birch_leaves", 30, 60),
    ("minecraft:birch_log", 5, 5),
    ("minecraft:birch_planks", 5, 20),
    ("minecraft:birch_shelf", 30, 20),
    ("minecraft:birch_slab", 5, 20),
    ("minecraft:birch_stairs", 5, 20),
    ("minecraft:birch_wood", 5, 5),
    ("minecraft:black_carpet", 60, 20),
    ("minecraft:black_wool", 30, 60),
    ("minecraft:black_wool_slab", 30, 60),
    ("minecraft:black_wool_stairs", 30, 60),
    ("minecraft:blue_carpet", 60, 20),
    ("minecraft:blue_orchid", 60, 100),
    ("minecraft:blue_wool", 30, 60),
    ("minecraft:blue_wool_slab", 30, 60),
    ("minecraft:blue_wool_stairs", 30, 60),
    ("minecraft:bookshelf", 30, 20),
    ("minecraft:brown_carpet", 60, 20),
    ("minecraft:brown_wool", 30, 60),
    ("minecraft:brown_wool_slab", 30, 60),
    ("minecraft:brown_wool_stairs", 30, 60),
    ("minecraft:bush", 60, 100),
    ("minecraft:cactus_flower", 60, 100),
    ("minecraft:cave_vines", 15, 60),
    ("minecraft:cave_vines_plant", 15, 60),
    ("minecraft:cherry_fence", 5, 20),
    ("minecraft:cherry_fence_gate", 5, 20),
    ("minecraft:cherry_leaves", 30, 60),
    ("minecraft:cherry_log", 5, 5),
    ("minecraft:cherry_planks", 5, 20),
    ("minecraft:cherry_shelf", 30, 20),
    ("minecraft:cherry_slab", 5, 20),
    ("minecraft:cherry_stairs", 5, 20),
    ("minecraft:cherry_wood", 5, 5),
    ("minecraft:closed_eyeblossom", 60, 100),
    ("minecraft:coal_block", 5, 5),
    ("minecraft:composter", 5, 20),
    ("minecraft:cornflower", 60, 100),
    ("minecraft:cyan_carpet", 60, 20),
    ("minecraft:cyan_wool", 30, 60),
    ("minecraft:cyan_wool_slab", 30, 60),
    ("minecraft:cyan_wool_stairs", 30, 60),
    ("minecraft:dandelion", 60, 100),
    ("minecraft:dark_oak_fence", 5, 20),
    ("minecraft:dark_oak_fence_gate", 5, 20),
    ("minecraft:dark_oak_leaves", 30, 60),
    ("minecraft:dark_oak_log", 5, 5),
    ("minecraft:dark_oak_planks", 5, 20),
    ("minecraft:dark_oak_shelf", 30, 20),
    ("minecraft:dark_oak_slab", 5, 20),
    ("minecraft:dark_oak_stairs", 5, 20),
    ("minecraft:dark_oak_wood", 5, 5),
    ("minecraft:dead_bush", 60, 100),
    ("minecraft:dried_kelp_block", 30, 60),
    ("minecraft:fern", 60, 100),
    ("minecraft:firefly_bush", 60, 100),
    ("minecraft:flowering_azalea", 30, 60),
    ("minecraft:flowering_azalea_leaves", 30, 60),
    ("minecraft:glow_lichen", 15, 100),
    ("minecraft:golden_dandelion", 60, 100),
    ("minecraft:gray_carpet", 60, 20),
    ("minecraft:gray_wool", 30, 60),
    ("minecraft:gray_wool_slab", 30, 60),
    ("minecraft:gray_wool_stairs", 30, 60),
    ("minecraft:green_carpet", 60, 20),
    ("minecraft:green_wool", 30, 60),
    ("minecraft:green_wool_slab", 30, 60),
    ("minecraft:green_wool_stairs", 30, 60),
    ("minecraft:hanging_roots", 30, 60),
    ("minecraft:hay_block", 60, 20),
    ("minecraft:jungle_fence", 5, 20),
    ("minecraft:jungle_fence_gate", 5, 20),
    ("minecraft:jungle_leaves", 30, 60),
    ("minecraft:jungle_log", 5, 5),
    ("minecraft:jungle_planks", 5, 20),
    ("minecraft:jungle_shelf", 30, 20),
    ("minecraft:jungle_slab", 5, 20),
    ("minecraft:jungle_stairs", 5, 20),
    ("minecraft:jungle_wood", 5, 5),
    ("minecraft:large_fern", 60, 100),
    ("minecraft:leaf_litter", 60, 100),
    ("minecraft:lectern", 30, 20),
    ("minecraft:light_blue_carpet", 60, 20),
    ("minecraft:light_blue_wool", 30, 60),
    ("minecraft:light_blue_wool_slab", 30, 60),
    ("minecraft:light_blue_wool_stairs", 30, 60),
    ("minecraft:light_gray_carpet", 60, 20),
    ("minecraft:light_gray_wool", 30, 60),
    ("minecraft:light_gray_wool_slab", 30, 60),
    ("minecraft:light_gray_wool_stairs", 30, 60),
    ("minecraft:lilac", 60, 100),
    ("minecraft:lily_of_the_valley", 60, 100),
    ("minecraft:lime_carpet", 60, 20),
    ("minecraft:lime_wool", 30, 60),
    ("minecraft:lime_wool_slab", 30, 60),
    ("minecraft:lime_wool_stairs", 30, 60),
    ("minecraft:magenta_carpet", 60, 20),
    ("minecraft:magenta_wool", 30, 60),
    ("minecraft:magenta_wool_slab", 30, 60),
    ("minecraft:magenta_wool_stairs", 30, 60),
    ("minecraft:mangrove_fence", 5, 20),
    ("minecraft:mangrove_fence_gate", 5, 20),
    ("minecraft:mangrove_leaves", 30, 60),
    ("minecraft:mangrove_log", 5, 5),
    ("minecraft:mangrove_planks", 5, 20),
    ("minecraft:mangrove_roots", 5, 20),
    ("minecraft:mangrove_shelf", 30, 20),
    ("minecraft:mangrove_slab", 5, 20),
    ("minecraft:mangrove_stairs", 5, 20),
    ("minecraft:mangrove_wood", 5, 5),
    ("minecraft:oak_fence", 5, 20),
    ("minecraft:oak_fence_gate", 5, 20),
    ("minecraft:oak_leaves", 30, 60),
    ("minecraft:oak_log", 5, 5),
    ("minecraft:oak_planks", 5, 20),
    ("minecraft:oak_shelf", 30, 20),
    ("minecraft:oak_slab", 5, 20),
    ("minecraft:oak_stairs", 5, 20),
    ("minecraft:oak_wood", 5, 5),
    ("minecraft:open_eyeblossom", 60, 100),
    ("minecraft:orange_carpet", 60, 20),
    ("minecraft:orange_poplar_leaves", 30, 60),
    ("minecraft:orange_tulip", 60, 100),
    ("minecraft:orange_wool", 30, 60),
    ("minecraft:orange_wool_slab", 30, 60),
    ("minecraft:orange_wool_stairs", 30, 60),
    ("minecraft:oxeye_daisy", 60, 100),
    ("minecraft:pale_hanging_moss", 5, 100),
    ("minecraft:pale_moss_block", 5, 100),
    ("minecraft:pale_moss_carpet", 5, 100),
    ("minecraft:pale_oak_fence", 5, 20),
    ("minecraft:pale_oak_fence_gate", 5, 20),
    ("minecraft:pale_oak_leaves", 30, 60),
    ("minecraft:pale_oak_log", 5, 5),
    ("minecraft:pale_oak_planks", 5, 20),
    ("minecraft:pale_oak_shelf", 30, 20),
    ("minecraft:pale_oak_slab", 5, 20),
    ("minecraft:pale_oak_stairs", 5, 20),
    ("minecraft:pale_oak_wood", 5, 5),
    ("minecraft:peony", 60, 100),
    ("minecraft:pink_carpet", 60, 20),
    ("minecraft:pink_petals", 60, 100),
    ("minecraft:pink_tulip", 60, 100),
    ("minecraft:pink_wool", 30, 60),
    ("minecraft:pink_wool_slab", 30, 60),
    ("minecraft:pink_wool_stairs", 30, 60),
    ("minecraft:pitcher_plant", 60, 100),
    ("minecraft:poplar_fence", 5, 20),
    ("minecraft:poplar_fence_gate", 5, 20),
    ("minecraft:poplar_log", 5, 5),
    ("minecraft:poplar_planks", 5, 20),
    ("minecraft:poplar_shelf", 30, 20),
    ("minecraft:poplar_slab", 5, 20),
    ("minecraft:poplar_stairs", 5, 20),
    ("minecraft:poplar_wood", 5, 5),
    ("minecraft:poppy", 60, 100),
    ("minecraft:purple_carpet", 60, 20),
    ("minecraft:purple_wool", 30, 60),
    ("minecraft:purple_wool_slab", 30, 60),
    ("minecraft:purple_wool_stairs", 30, 60),
    ("minecraft:red_carpet", 60, 20),
    ("minecraft:red_poplar_leaves", 30, 60),
    ("minecraft:red_shrub", 60, 100),
    ("minecraft:red_tulip", 60, 100),
    ("minecraft:red_wool", 30, 60),
    ("minecraft:red_wool_slab", 30, 60),
    ("minecraft:red_wool_stairs", 30, 60),
    ("minecraft:rose_bush", 60, 100),
    ("minecraft:scaffolding", 60, 60),
    ("minecraft:short_dry_grass", 60, 100),
    ("minecraft:short_grass", 60, 100),
    ("minecraft:small_dripleaf", 60, 100),
    ("minecraft:spore_blossom", 60, 100),
    ("minecraft:spruce_fence", 5, 20),
    ("minecraft:spruce_fence_gate", 5, 20),
    ("minecraft:spruce_leaves", 30, 60),
    ("minecraft:spruce_log", 5, 5),
    ("minecraft:spruce_planks", 5, 20),
    ("minecraft:spruce_shelf", 30, 20),
    ("minecraft:spruce_slab", 5, 20),
    ("minecraft:spruce_stairs", 5, 20),
    ("minecraft:spruce_wood", 5, 5),
    ("minecraft:straw_bed", 30, 60),
    ("minecraft:stripped_acacia_log", 5, 5),
    ("minecraft:stripped_acacia_wood", 5, 5),
    ("minecraft:stripped_bamboo_block", 5, 5),
    ("minecraft:stripped_birch_log", 5, 5),
    ("minecraft:stripped_birch_wood", 5, 5),
    ("minecraft:stripped_cherry_log", 5, 5),
    ("minecraft:stripped_cherry_wood", 5, 5),
    ("minecraft:stripped_dark_oak_log", 5, 5),
    ("minecraft:stripped_dark_oak_wood", 5, 5),
    ("minecraft:stripped_jungle_log", 5, 5),
    ("minecraft:stripped_jungle_wood", 5, 5),
    ("minecraft:stripped_mangrove_log", 5, 5),
    ("minecraft:stripped_mangrove_wood", 5, 5),
    ("minecraft:stripped_oak_log", 5, 5),
    ("minecraft:stripped_oak_wood", 5, 5),
    ("minecraft:stripped_pale_oak_log", 5, 5),
    ("minecraft:stripped_pale_oak_wood", 5, 5),
    ("minecraft:stripped_poplar_log", 5, 5),
    ("minecraft:stripped_poplar_wood", 5, 5),
    ("minecraft:stripped_spruce_log", 5, 5),
    ("minecraft:stripped_spruce_wood", 5, 5),
    ("minecraft:sunflower", 60, 100),
    ("minecraft:sweet_berry_bush", 60, 100),
    ("minecraft:tall_dry_grass", 60, 100),
    ("minecraft:tall_grass", 60, 100),
    ("minecraft:target", 15, 20),
    ("minecraft:tnt", 15, 100),
    ("minecraft:torchflower", 60, 100),
    ("minecraft:vine", 15, 100),
    ("minecraft:white_carpet", 60, 20),
    ("minecraft:white_tulip", 60, 100),
    ("minecraft:white_wool", 30, 60),
    ("minecraft:white_wool_slab", 30, 60),
    ("minecraft:white_wool_stairs", 30, 60),
    ("minecraft:wildflowers", 60, 100),
    ("minecraft:wither_rose", 60, 100),
    ("minecraft:yellow_carpet", 60, 20),
    ("minecraft:yellow_poplar_leaves", 30, 60),
    ("minecraft:yellow_wool", 30, 60),
    ("minecraft:yellow_wool_slab", 30, 60),
    ("minecraft:yellow_wool_stairs", 30, 60),
];
