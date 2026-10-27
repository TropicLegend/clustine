//! Tests of clicks, placing and buckets against small worlds.

use std::collections::BTreeMap;

use clustine_data::{BLOCKS, BlockClass, BlockState, blocks, items};
use clustine_world::{BlockPos, Vec3};

use super::replace::{Replace, replace_rule};
use super::rules::{Rule, rule_for};
use super::survive::{Survival, survival_rule};
use super::*;
use crate::api::Face;

/// A world of air with the blocks set in it, loaded for x below 16 and within the
/// overworld's height.
#[derive(Default)]
struct TestWorld {
    set: BTreeMap<(i32, i32, i32), BlockState>,
}

impl TestWorld {
    fn with(mut self, x: i32, y: i32, z: i32, state: BlockState) -> Self {
        self.set.insert((x, y, z), state);
        self
    }

    fn floor(mut self, y: i32, state: BlockState) -> Self {
        for x in -4..8 {
            for z in -4..8 {
                self.set.insert((x, y, z), state);
            }
        }
        self
    }
}

impl Blocks for TestWorld {
    fn block(&self, at: BlockPos) -> Option<BlockState> {
        if at.x >= 16 || !(-64..320).contains(&at.y) {
            return None;
        }
        Some(*self.set.get(&(at.x, at.y, at.z)).unwrap_or(&blocks::AIR))
    }
}

fn state(text: &str) -> BlockState {
    BlockState::parse(text).unwrap_or_else(|| panic!("no state {text}"))
}

fn at(x: i32, y: i32, z: i32) -> BlockPos {
    BlockPos::new(x, y, z)
}

/// A click on the top of the block at `pos`, by a player standing two blocks to the
/// north looking south and down at it.
fn click_top(pos: BlockPos, held: i32) -> Click {
    Click {
        position: pos,
        face: Face::Top,
        cursor: [0.5, 1.0, 0.5],
        sneaking: false,
        held: Some(held),
        feet: Vec3::new(
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 1.0,
            f64::from(pos.z) - 1.5,
        ),
        yaw: 0.0,
        pitch: 45.0,
    }
}

/// What a click and the placement it leads to change, in one region.
fn click_and_place(click: &Click, blocks: &dyn Blocks) -> Option<Vec<(BlockPos, BlockState)>> {
    match super::click(click, blocks) {
        Clicked::Place {
            target,
            replace_clicked,
            clicked,
        } => place(click, target, replace_clicked, clicked, blocks, &[]),
        Clicked::Used { changes, .. } => Some(changes),
        Clicked::Nothing | Clicked::ByRule { .. } => None,
    }
}

#[test]
fn every_named_class_and_every_class_made_from_one_finds_its_rule() {
    use BlockClass as C;
    // The classes sections 4.2, 4.3 and 4.4 name, with the rule each is meant to have.
    let place: &[(C, Rule)] = &[
        (C::RotatedPillarBlock, Rule::Axis),
        (C::AbstractFurnaceBlock, Rule::FacingAway),
        (C::CarvedPumpkinBlock, Rule::FacingAway),
        (C::BeehiveBlock, Rule::FacingAway),
        (C::LoomBlock, Rule::FacingAway),
        (C::StonecutterBlock, Rule::FacingAway),
        (C::LecternBlock, Rule::FacingAway),
        (C::GlazedTerracottaBlock, Rule::FacingAway),
        (C::EnderChestBlock, Rule::FacingAway),
        (C::ChiseledBookShelfBlock, Rule::FacingAway),
        (C::EndPortalFrameBlock, Rule::FacingAway),
        (C::CalibratedSculkSensorBlock, Rule::FacingAlong),
        (C::DiodeBlock, Rule::Diode),
        (C::SmallDripleafBlock, Rule::SmallDripleaf),
        (C::BigDripleafBlock, Rule::BigDripleaf),
        (C::ChestBlock, Rule::Chest),
        (C::CampfireBlock, Rule::Campfire),
        (C::AnvilBlock, Rule::Anvil),
        (C::BarrelBlock, Rule::AwayFromLook),
        (C::DispenserBlock, Rule::AwayFromLook),
        (C::PistonBaseBlock, Rule::AwayFromLook),
        (C::ObserverBlock, Rule::TowardsLook),
        (C::HopperBlock, Rule::Hopper),
        (C::EndRodBlock, Rule::EndRod),
        (C::LightningRodBlock, Rule::LightningRod),
        (C::StairBlock, Rule::Stairs),
        (C::SlabBlock, Rule::Slab),
        (C::DoorBlock, Rule::Door),
        (C::TrapDoorBlock, Rule::TrapDoor),
        (C::FenceGateBlock, Rule::FenceGate),
        (C::FenceBlock, Rule::Fence),
        (C::IronBarsBlock, Rule::Pane),
        (C::WallBlock, Rule::Wall),
        (C::RedstoneWireBlock, Rule::RedstoneWire),
        (C::BaseRailBlock, Rule::Rail),
        (C::AbstractBedBlock, Rule::Bed),
        (
            C::FaceAttachedHorizontalDirectionalBlock,
            Rule::FaceAttached,
        ),
        (C::LadderBlock, Rule::Ladder),
        (C::LanternBlock, Rule::Lantern),
        (C::DoublePlantBlock, Rule::DoublePlant),
        (C::PitcherCropBlock, Rule::Default),
        (C::SnowLayerBlock, Rule::SnowLayer),
        (C::CandleBlock, Rule::Additive),
        (C::SeaPickleBlock, Rule::Additive),
        (C::TurtleEggBlock, Rule::Additive),
        (C::FlowerBedBlock, Rule::Segmented),
        (C::LeafLitterBlock, Rule::Segmented),
        (C::MultifaceBlock, Rule::Multiface),
        (C::ScaffoldingBlock, Rule::Scaffolding),
        (C::WallTorchBlock, Rule::OnWall),
        (C::RedstoneWallTorchBlock, Rule::OnWall),
        (C::WallSignBlock, Rule::OnWall),
        (C::WallBannerBlock, Rule::OnWall),
        (C::BaseCoralWallFanBlock, Rule::OnWall),
        (C::StandingSignBlock, Rule::StandingRotation),
        (C::BannerBlock, Rule::StandingRotation),
        (C::CeilingHangingSignBlock, Rule::CeilingHangingSign),
        (C::WallHangingSignBlock, Rule::WallHangingSign),
        (C::CropBlock, Rule::Default),
        (C::StemBlock, Rule::Default),
        (C::SugarCaneBlock, Rule::Default),
        (C::CactusBlock, Rule::Default),
        (C::BaseTorchBlock, Rule::Default),
    ];
    let survival: &[(C, Survival)] = &[
        (
            C::SaplingBlock,
            Survival::OnTag("minecraft:supports_vegetation"),
        ),
        (C::MangrovePropaguleBlock, Survival::MangrovePropagule),
        (C::TallSeagrassBlock, Survival::NotPorted),
        (
            C::PitcherCropBlock,
            Survival::DoublePlant("minecraft:supports_crops"),
        ),
        (C::AttachedStemBlock, Survival::OnTagByName),
        (
            C::FlowerBlock,
            Survival::OnTag("minecraft:supports_vegetation"),
        ),
        (
            C::BushBlock,
            Survival::OnTag("minecraft:supports_vegetation"),
        ),
        (
            C::WitherRoseBlock,
            Survival::OnTag("minecraft:supports_wither_rose"),
        ),
        (
            C::DoublePlantBlock,
            Survival::DoublePlant("minecraft:supports_vegetation"),
        ),
        (
            C::TallFlowerBlock,
            Survival::DoublePlant("minecraft:supports_vegetation"),
        ),
        (C::CropBlock, Survival::OnTag("minecraft:supports_crops")),
        (C::StemBlock, Survival::OnTagByName),
        (C::SugarCaneBlock, Survival::SugarCane),
        (C::CactusBlock, Survival::Cactus),
        (C::CarpetBlock, Survival::NotOnAir),
        (
            C::BaseRailBlock,
            Survival::OnTop(clustine_data::SupportType::Rigid),
        ),
        (C::BasePressurePlateBlock, Survival::PressurePlate),
        (C::SnowLayerBlock, Survival::SnowLayer),
        (C::DoorBlock, Survival::Door),
        (
            C::DiodeBlock,
            Survival::OnTop(clustine_data::SupportType::Rigid),
        ),
        (
            C::BaseTorchBlock,
            Survival::OnTop(clustine_data::SupportType::Center),
        ),
        (C::WallTorchBlock, Survival::BehindSturdy),
        (C::RedstoneWallTorchBlock, Survival::BehindSturdy),
        (C::StandingSignBlock, Survival::OnSolid),
        (C::WallSignBlock, Survival::BehindSolid),
        (C::CeilingHangingSignBlock, Survival::Hanging),
        (C::BannerBlock, Survival::OnSolid),
        (C::WallBannerBlock, Survival::BehindSolid),
        (
            C::FaceAttachedHorizontalDirectionalBlock,
            Survival::FaceAttached,
        ),
        (C::GrindstoneBlock, Survival::Always),
        (C::LadderBlock, Survival::BehindSturdy),
        (C::LanternBlock, Survival::Lantern),
        (C::RedstoneWireBlock, Survival::RedstoneWire),
        (C::LilyPadBlock, Survival::LilyPad),
        (
            C::CandleBlock,
            Survival::OnTop(clustine_data::SupportType::Center),
        ),
        (C::SeaPickleBlock, Survival::SeaPickle),
        (C::MultifaceBlock, Survival::Multiface),
        (
            C::FlowerBedBlock,
            Survival::OnTag("minecraft:supports_vegetation"),
        ),
        (
            C::LeafLitterBlock,
            Survival::OnTop(clustine_data::SupportType::Full),
        ),
        (C::SmallDripleafBlock, Survival::SmallDripleaf),
        (C::BigDripleafBlock, Survival::BigDripleaf),
        (C::ScaffoldingBlock, Survival::Scaffolding),
        (
            C::BaseCoralFanBlock,
            Survival::OnTop(clustine_data::SupportType::Full),
        ),
        (C::BaseCoralWallFanBlock, Survival::BehindSturdy),
        (
            C::BambooSaplingBlock,
            Survival::OnTag("minecraft:supports_bamboo"),
        ),
        (
            C::BambooStalkBlock,
            Survival::OnTag("minecraft:supports_bamboo"),
        ),
    ];
    let replace: &[(C, Replace)] = &[
        (C::SnowLayerBlock, Replace::SnowLayer),
        (C::SlabBlock, Replace::Slab),
        (C::CandleBlock, Replace::Additive),
        (C::SeaPickleBlock, Replace::Additive),
        (C::TurtleEggBlock, Replace::Additive),
        (C::FlowerBedBlock, Replace::Additive),
        (C::LeafLitterBlock, Replace::Additive),
        (C::MultifaceBlock, Replace::Multiface),
        (C::LiquidBlock, Replace::Fluid),
    ];
    /// The entry of `table` for the nearest of the block's classes it names.
    fn nearest<T: Copy>(info: &clustine_data::BlockInfo, table: &[(C, T)]) -> Option<T> {
        std::iter::once(info.class)
            .chain(info.class.ancestors().iter().copied())
            .find_map(|class| table.iter().find(|(c, _)| *c == class).map(|(_, t)| *t))
    }
    let mut checked = 0;
    for (block, info) in BLOCKS.iter().zip(clustine_data::BLOCK_INFO.iter()) {
        let state = block.default_state;
        if let Some(expected) = nearest(info, place) {
            assert_eq!(rule_for(state), expected, "{}", block.name);
            checked += 1;
        }
        if let Some(expected) = nearest(info, survival)
            && info.survival_rule
        {
            assert_eq!(survival_rule(state), Some(expected), "{}", block.name);
            checked += 1;
        }
        if let Some(expected) = nearest(info, replace) {
            assert_eq!(replace_rule(state), Some(expected), "{}", block.name);
            checked += 1;
        }
    }
    assert!(checked > 500, "{checked} blocks checked");
}

#[test]
fn every_tag_the_rules_ask_is_in_the_games_data() {
    // A tag that is missing panics when a rule first asks it; ask them all here.
    for tag in [
        "all_hanging_signs",
        "cannot_support_seagrass",
        "cannot_support_snow_layer",
        "fences",
        "logs",
        "overrides_mushroom_light_requirement",
        "shulker_boxes",
        "support_override_cactus_flower",
        "support_override_snow_layer",
        "supports_azalea",
        "supports_bamboo",
        "supports_big_dripleaf",
        "supports_cactus",
        "supports_crimson_fungus",
        "supports_crimson_roots",
        "supports_crops",
        "supports_dry_vegetation",
        "supports_frogspawn",
        "supports_hanging_mangrove_propagule",
        "supports_lily_pad",
        "supports_mangrove_propagule",
        "supports_melon_stem",
        "supports_nether_sprouts",
        "supports_nether_wart",
        "supports_pumpkin_stem",
        "supports_small_dripleaf",
        "supports_sugar_cane",
        "supports_sugar_cane_adjacently",
        "supports_vegetation",
        "supports_warped_fungus",
        "supports_warped_roots",
        "supports_wither_rose",
        "wall_hanging_signs",
        "wall_post_override",
        "walls",
        "wooden_fences",
    ] {
        data::in_tag(blocks::STONE, &format!("minecraft:{tag}"));
    }
    for tag in [
        "water",
        "lava",
        "supports_lily_pad",
        "supports_frogspawn",
        "supports_sugar_cane_adjacently",
    ] {
        data::fluid_in_tag(blocks::WATER, &format!("minecraft:{tag}"));
    }
}

#[test]
fn every_block_with_a_survival_rule_is_ported_or_places_nothing() {
    let world = TestWorld::default().floor(63, blocks::GRASS_BLOCK);
    let mut not_ported = Vec::new();
    for item in clustine_data::ITEMS.iter() {
        let Some(block) = item.places else { continue };
        let state = data::default_state(block);
        if survival_rule(state) != Some(Survival::NotPorted) {
            continue;
        }
        not_ported.push(BLOCKS[usize::from(block)].name);
        let id = clustine_data::ITEMS
            .iter()
            .position(|other| std::ptr::eq(other, item))
            .unwrap() as i32;
        let click = click_top(at(0, 63, 0), id);
        if let Clicked::Place {
            target,
            replace_clicked,
            clicked,
        } = super::click(&click, &world)
        {
            assert_eq!(
                place(&click, target, replace_clicked, clicked, &world, &[]),
                None,
                "{} is placed",
                item.name
            );
        }
    }
    // Every survival rule that is ported is one of the list; what is not is named.
    for (block, info) in BLOCKS.iter().zip(clustine_data::BLOCK_INFO.iter()) {
        if info.survival_rule {
            assert!(
                survival_rule(block.default_state).is_some(),
                "{}",
                block.name
            );
        }
    }
    assert!(not_ported.contains(&"minecraft:vine"), "{not_ported:?}");
    assert!(!not_ported.contains(&"minecraft:bamboo"), "{not_ported:?}");
}

#[test]
fn a_door_opens_with_its_other_half() {
    let lower = state("minecraft:oak_door[half=lower,facing=north]");
    let upper = state("minecraft:oak_door[half=upper,facing=north]");
    let world = TestWorld::default()
        .with(0, 64, 0, lower)
        .with(0, 65, 0, upper);
    let mut click = click_top(at(0, 65, 0), items::STONE);
    click.held = None;
    // Each half is set telling no neighbour, as `DoorBlock.useWithoutItem` sets the
    // clicked one (`docs/adr/0026-redstone.md`, section 7).
    assert_eq!(
        super::click(&click, &world),
        Clicked::Used {
            changes: vec![
                (
                    at(0, 65, 0),
                    state("minecraft:oak_door[half=upper,facing=north,open=true]")
                ),
                (
                    at(0, 64, 0),
                    state("minecraft:oak_door[half=lower,facing=north,open=true]")
                ),
            ],
            flags: 10,
        }
    );
}

#[test]
fn a_block_in_hand_places_against_an_iron_door_and_sneaking_against_an_oak_one() {
    let world = TestWorld::default()
        .with(0, 64, 0, state("minecraft:iron_door[half=lower]"))
        .with(1, 64, 0, state("minecraft:oak_door[half=lower]"));
    let iron = click_top(at(0, 64, 0), items::STONE);
    assert!(matches!(super::click(&iron, &world), Clicked::Place { .. }));
    let mut oak = click_top(at(1, 64, 0), items::STONE);
    assert!(matches!(super::click(&oak, &world), Clicked::Used { .. }));
    oak.sneaking = true;
    assert!(matches!(super::click(&oak, &world), Clicked::Place { .. }));
}

#[test]
fn a_closed_gate_opened_from_behind_turns_to_the_player() {
    let gate = state("minecraft:oak_fence_gate[facing=north]");
    let world = TestWorld::default().with(0, 64, 0, gate);
    let mut click = click_top(at(0, 64, 0), items::STONE);
    click.held = None;
    click.yaw = 0.0; // facing south
    assert_eq!(
        super::click(&click, &world),
        Clicked::Used {
            changes: vec![(
                at(0, 64, 0),
                state("minecraft:oak_fence_gate[facing=south,open=true]")
            )],
            flags: 10,
        }
    );
}

#[test]
fn a_flower_goes_into_an_empty_pot_and_not_into_a_full_one() {
    let world = TestWorld::default()
        .with(0, 64, 0, blocks::FLOWER_POT)
        .with(1, 64, 0, blocks::POTTED_FERN);
    let click = click_top(at(0, 64, 0), items::POPPY);
    assert_eq!(
        super::click(&click, &world),
        Clicked::Used {
            changes: vec![(at(0, 64, 0), blocks::POTTED_POPPY)],
            flags: 3,
        }
    );
    let full = click_top(at(1, 64, 0), items::POPPY);
    assert_eq!(super::click(&full, &world), Clicked::Nothing);
}

#[test]
fn a_click_on_a_fence_places_a_torch_on_it_and_a_chest_takes_the_click() {
    let world =
        TestWorld::default()
            .with(0, 64, 0, blocks::OAK_FENCE)
            .with(2, 64, 0, blocks::CHEST);
    let changes = click_and_place(&click_top(at(0, 64, 0), items::TORCH), &world);
    assert_eq!(changes, Some(vec![(at(0, 65, 0), blocks::TORCH)]));
    // A chest takes the click for its menu, which the region opens.
    assert!(matches!(
        super::click(&click_top(at(2, 64, 0), items::TORCH), &world),
        Clicked::ByRule { .. }
    ));
}

#[test]
fn placing_into_grass_replaces_it_and_onto_stone_goes_above() {
    let world = TestWorld::default()
        .floor(63, blocks::STONE)
        .with(0, 64, 0, blocks::SHORT_GRASS);
    let grass = click_top(at(0, 64, 0), items::DIRT);
    assert_eq!(
        click_and_place(&grass, &world),
        Some(vec![(at(0, 64, 0), blocks::DIRT)])
    );
    let stone = click_top(at(1, 63, 0), items::DIRT);
    assert_eq!(
        click_and_place(&stone, &world),
        Some(vec![(at(1, 64, 0), blocks::DIRT)])
    );
}

#[test]
fn a_log_lies_along_the_clicked_face() {
    let world = TestWorld::default().with(0, 64, 0, blocks::STONE);
    let mut click = click_top(at(0, 64, 0), items::OAK_LOG);
    click.face = Face::East;
    click.cursor = [1.0, 0.5, 0.5];
    assert_eq!(
        click_and_place(&click, &world),
        Some(vec![(at(1, 64, 0), state("minecraft:oak_log[axis=x]"))])
    );
}

#[test]
fn a_slab_on_a_slab_makes_a_double_and_a_high_click_on_a_side_a_top_slab() {
    let world = TestWorld::default()
        .with(0, 64, 0, state("minecraft:oak_slab[type=bottom]"))
        .with(2, 64, 0, blocks::STONE);
    let on_slab = click_top(at(0, 64, 0), items::OAK_SLAB);
    let mut on_slab = on_slab;
    on_slab.cursor = [0.5, 0.5, 0.5];
    assert_eq!(
        click_and_place(&on_slab, &world),
        Some(vec![(
            at(0, 64, 0),
            state("minecraft:oak_slab[type=double]")
        )])
    );
    let mut side = click_top(at(2, 64, 0), items::OAK_SLAB);
    side.face = Face::West;
    side.cursor = [0.0, 0.75, 0.5];
    assert_eq!(
        click_and_place(&side, &world),
        Some(vec![(at(1, 64, 0), state("minecraft:oak_slab[type=top]"))])
    );
}

#[test]
fn stairs_beside_stairs_turned_across_make_a_corner() {
    // Stairs facing east, and new stairs facing south placed to the west of them: the
    // stairs behind the new ones (to the south) are not there, in front (north) none;
    // so put east-facing stairs to the south.
    let world = TestWorld::default().floor(63, blocks::STONE).with(
        0,
        64,
        1,
        state("minecraft:oak_stairs[facing=east,half=bottom]"),
    );
    let click = click_top(at(0, 63, 0), items::OAK_STAIRS);
    let changes = click_and_place(&click, &world).unwrap();
    assert_eq!(
        changes,
        vec![(
            at(0, 64, 0),
            state("minecraft:oak_stairs[facing=south,half=bottom,shape=outer_left]")
        )]
    );
}

/// `HopperBlock.getStateForPlacement`: a hopper faces into the block whose side was
/// clicked, and down from a click on a top.
#[test]
fn a_hopper_faces_into_the_side_clicked_and_down_from_a_top() {
    let world = TestWorld::default().with(0, 64, 0, blocks::STONE);
    let mut click = click_top(at(0, 64, 0), items::HOPPER);
    click.face = Face::East;
    click.cursor = [1.0, 0.5, 0.5];
    assert_eq!(
        click_and_place(&click, &world),
        Some(vec![(
            at(1, 64, 0),
            state("minecraft:hopper[enabled=true,facing=west]")
        )])
    );
    let click = click_top(at(0, 64, 0), items::HOPPER);
    assert_eq!(
        click_and_place(&click, &world),
        Some(vec![(
            at(0, 65, 0),
            state("minecraft:hopper[enabled=true,facing=down]")
        )])
    );
}

#[test]
fn a_torch_goes_on_the_wall_clicked() {
    let world = TestWorld::default().with(0, 64, 0, blocks::STONE);
    let mut click = click_top(at(0, 64, 0), items::TORCH);
    click.face = Face::North;
    click.cursor = [0.5, 0.5, 0.0];
    assert_eq!(
        click_and_place(&click, &world),
        Some(vec![(
            at(0, 64, -1),
            state("minecraft:wall_torch[facing=north]")
        )])
    );
}

#[test]
fn a_door_is_two_blocks_and_is_not_placed_into_its_placer() {
    let world = TestWorld::default().floor(63, blocks::STONE);
    let click = click_top(at(0, 63, 0), items::OAK_DOOR);
    let changes = click_and_place(&click, &world).unwrap();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0].0, at(0, 64, 0));
    assert_eq!(changes[0].1.property("half"), Some("lower"));
    assert_eq!(changes[0].1.property("facing"), Some("south"));
    assert_eq!(changes[1].0, at(0, 65, 0));
    assert_eq!(changes[1].1.property("half"), Some("upper"));
    let mut inside = click;
    inside.feet = Vec3::new(0.5, 64.0, 0.1);
    assert_eq!(click_and_place(&inside, &world), None);
}

#[test]
fn snow_on_snow_grows_and_wheat_needs_farmland() {
    let world = TestWorld::default()
        .floor(63, blocks::STONE)
        .with(0, 64, 0, state("minecraft:snow[layers=2]"))
        .with(2, 63, 0, blocks::FARMLAND);
    let snow = click_top(at(0, 64, 0), items::SNOW);
    assert_eq!(
        click_and_place(&snow, &world),
        Some(vec![(at(0, 64, 0), state("minecraft:snow[layers=3]"))])
    );
    let on_stone = click_top(at(4, 63, 0), items::WHEAT_SEEDS);
    assert_eq!(click_and_place(&on_stone, &world), None);
    let on_farmland = click_top(at(2, 63, 0), items::WHEAT_SEEDS);
    assert_eq!(
        click_and_place(&on_farmland, &world),
        Some(vec![(at(2, 64, 0), blocks::WHEAT)])
    );
}

#[test]
fn a_fence_joins_a_fence_and_a_wall_but_not_leaves() {
    let world = TestWorld::default()
        .floor(63, blocks::STONE)
        .with(1, 64, 0, blocks::SPRUCE_FENCE)
        .with(-1, 64, 0, blocks::OAK_LEAVES)
        .with(0, 64, 1, blocks::NETHER_BRICK_FENCE)
        .with(0, 64, -1, blocks::STONE);
    let click = click_top(at(0, 63, 0), items::OAK_FENCE);
    assert_eq!(
        click_and_place(&click, &world),
        Some(vec![(
            at(0, 64, 0),
            state("minecraft:oak_fence[east=true,west=false,south=false,north=true]")
        )])
    );
}

#[test]
fn a_placement_into_a_chunk_not_loaded_places_nothing() {
    let world = TestWorld::default().with(15, 64, 0, blocks::STONE);
    let mut click = click_top(at(15, 64, 0), items::STONE);
    click.face = Face::East;
    let Clicked::Place {
        target,
        replace_clicked,
        clicked,
    } = super::click(&click, &world)
    else {
        panic!("no placement");
    };
    assert_eq!(target, at(16, 64, 0));
    assert_eq!(
        place(&click, target, replace_clicked, clicked, &world, &[]),
        None
    );
}

#[test]
fn the_clicked_block_carried_with_a_click_holds_a_torch_across_a_border() {
    // The clicked block is in a chunk this world does not have; it travels with the
    // click.
    let world = TestWorld::default();
    let mut click = click_top(at(16, 64, 0), items::TORCH);
    click.face = Face::West;
    click.cursor = [0.0, 0.5, 0.5];
    click.yaw = 90.0;
    click.pitch = 0.0;
    assert_eq!(
        place(&click, at(15, 64, 0), false, blocks::STONE, &world, &[]),
        Some(vec![(
            at(15, 64, 0),
            state("minecraft:wall_torch[facing=west]")
        )])
    );
}

#[test]
fn a_block_that_can_hold_water_keeps_the_source_it_is_placed_into() {
    let world = TestWorld::default()
        .floor(63, blocks::STONE)
        .with(0, 64, 0, blocks::WATER);
    let click = click_top(at(0, 64, 0), items::OAK_STAIRS);
    let changes = click_and_place(&click, &world).unwrap();
    assert_eq!(changes[0].1.property("waterlogged"), Some("true"));
}

#[test]
fn bamboo_is_planted_as_a_shoot_on_grass() {
    let world = TestWorld::default().floor(63, blocks::GRASS_BLOCK);
    let click = click_top(at(0, 63, 0), items::BAMBOO);
    assert_eq!(
        click_and_place(&click, &world),
        Some(vec![(at(0, 64, 0), blocks::BAMBOO_SAPLING)])
    );
}

fn item_use(held: i32, feet: Vec3, pitch: f32) -> ItemUse {
    ItemUse {
        held: Some(held),
        feet,
        yaw: 0.0,
        pitch,
        sneaking: false,
        inventory: Inventory::default(),
    }
}

#[test]
fn a_water_bucket_empties_onto_the_block_looked_at_and_an_empty_one_takes_it_back() {
    let world = TestWorld::default().floor(63, blocks::STONE);
    let feet = Vec3::new(0.5, 64.0, 0.5);
    let used = use_item(&item_use(items::WATER_BUCKET, feet, 60.0), &world, &[]).unwrap();
    assert_eq!(used.changes.len(), 1);
    let (pos, placed) = used.changes[0];
    assert_eq!(placed, blocks::WATER);
    assert_eq!(pos.y, 64);
    assert_eq!(used.slot, None);

    let world = world.with(pos.x, pos.y, pos.z, blocks::WATER);
    let used = use_item(&item_use(items::BUCKET, feet, 60.0), &world, &[]).unwrap();
    assert_eq!(used.changes, vec![(pos, blocks::AIR)]);
    assert_eq!(
        used.slot,
        Some((0, Some(ItemStack::new(items::WATER_BUCKET, 1))))
    );
}

#[test]
fn a_sneaking_water_bucket_fills_flowing_water_on_vanillas_second_try() {
    // Sneaking, the first try asks for air; vanilla's second try at the same block
    // passes no hit and so does not ask, and the flowing water becomes a source. The
    // ray of a full bucket passes through fluids to the stone.
    let feet = Vec3::new(0.5, 64.0, 0.5);
    let flowing = state("minecraft:water[level=1]");
    let world = TestWorld::default()
        .floor(63, blocks::STONE)
        .floor(64, flowing);
    let mut aimed = item_use(items::WATER_BUCKET, feet, 60.0);
    aimed.sneaking = true;
    let used = use_item(&aimed, &world, &[]).unwrap();
    assert_eq!(used.changes.len(), 1);
    assert_eq!(used.changes[0].0.y, 64);
    assert_eq!(used.changes[0].1, blocks::WATER);
}

#[test]
fn a_water_bucket_fills_a_block_that_can_hold_water() {
    let feet = Vec3::new(0.5, 64.0, 0.5);
    let probe = TestWorld::default().floor(63, blocks::STONE);
    let pos = use_item(&item_use(items::WATER_BUCKET, feet, 60.0), &probe, &[])
        .unwrap()
        .changes[0]
        .0;
    let slab = state("minecraft:oak_slab[type=bottom]");
    let below = at(pos.x, pos.y - 1, pos.z);
    let world = TestWorld::default()
        .floor(63, blocks::STONE)
        .with(below.x, below.y, below.z, slab);
    let used = use_item(&item_use(items::WATER_BUCKET, feet, 60.0), &world, &[]).unwrap();
    assert_eq!(
        used.changes,
        vec![(
            below,
            state("minecraft:oak_slab[type=bottom,waterlogged=true]")
        )]
    );
}

#[test]
fn a_lily_pad_goes_on_water_and_not_on_stone() {
    let world = TestWorld::default().floor(63, blocks::WATER);
    let feet = Vec3::new(0.5, 64.0, 0.5);
    let used = use_item(&item_use(items::LILY_PAD, feet, 60.0), &world, &[]).unwrap();
    assert_eq!(used.changes.len(), 1);
    assert_eq!(used.changes[0].1, blocks::LILY_PAD);
    assert_eq!(used.changes[0].0.y, 64);
    let stone = TestWorld::default().floor(63, blocks::STONE);
    assert_eq!(
        use_item(&item_use(items::LILY_PAD, feet, 60.0), &stone, &[]),
        None
    );
}

#[test]
fn a_lily_pad_is_not_placed_by_a_click() {
    let world = TestWorld::default().floor(63, blocks::WATER);
    assert_eq!(
        super::click(&click_top(at(0, 63, 0), items::LILY_PAD), &world),
        Clicked::Nothing
    );
}

/// A chest placed by a player standing north of it and looking south, on the stone
/// floor at y 63, into (0, 64, 0), with the blocks `around` set.
fn chest_placed(around: &[((i32, i32, i32), &str)], held: i32, sneaking: bool) -> BlockState {
    let mut world = TestWorld::default().floor(63, blocks::STONE);
    for ((x, y, z), text) in around {
        world = world.with(*x, *y, *z, state(text));
    }
    let mut click = click_top(at(0, 63, 0), held);
    click.sneaking = sneaking;
    let changes = click_and_place(&click, &world).expect("the chest is placed");
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].0, at(0, 64, 0));
    changes[0].1
}

#[test]
fn a_chest_placed_beside_a_chest_facing_the_same_way_pairs_with_it() {
    // Facing north, the chest to the east is on its left: it becomes the left half.
    let east = [((1, 64, 0), "minecraft:chest[facing=north]")];
    assert_eq!(
        chest_placed(&east, items::CHEST, false),
        state("minecraft:chest[facing=north,type=left]")
    );
    let west = [((-1, 64, 0), "minecraft:chest[facing=north]")];
    assert_eq!(
        chest_placed(&west, items::CHEST, false),
        state("minecraft:chest[facing=north,type=right]")
    );
    // With one on either side, the one to the east is taken.
    let both = [east[0], west[0]];
    assert_eq!(
        chest_placed(&both, items::CHEST, false),
        state("minecraft:chest[facing=north,type=left]")
    );
}

#[test]
fn a_chest_placed_sneaking_on_the_ground_stays_single() {
    let east = [((1, 64, 0), "minecraft:chest[facing=north]")];
    assert_eq!(
        chest_placed(&east, items::CHEST, true),
        state("minecraft:chest[facing=north,type=single]")
    );
}

#[test]
fn a_chest_does_not_pair_with_one_facing_another_way_or_already_double() {
    let turned = [((1, 64, 0), "minecraft:chest[facing=east]")];
    assert_eq!(
        chest_placed(&turned, items::CHEST, false),
        state("minecraft:chest[facing=north,type=single]")
    );
    let double = [((1, 64, 0), "minecraft:chest[facing=north,type=right]")];
    assert_eq!(
        chest_placed(&double, items::CHEST, false),
        state("minecraft:chest[facing=north,type=single]")
    );
}

#[test]
fn a_chest_pairs_only_with_its_own_block_and_copper_chests_with_any_copper_chest() {
    let trapped = [((1, 64, 0), "minecraft:trapped_chest[facing=north]")];
    assert_eq!(
        chest_placed(&trapped, items::CHEST, false),
        state("minecraft:chest[facing=north,type=single]")
    );
    assert_eq!(
        chest_placed(&trapped, items::TRAPPED_CHEST, false),
        state("minecraft:trapped_chest[facing=north,type=left]")
    );
    let copper = [((1, 64, 0), "minecraft:copper_chest[facing=north]")];
    assert_eq!(
        chest_placed(&copper, items::CHEST, false),
        state("minecraft:chest[facing=north,type=single]")
    );
    // The least weathered of the two, unwaxed where only one was waxed.
    let waxed_exposed = [(
        (1, 64, 0),
        "minecraft:waxed_exposed_copper_chest[facing=north]",
    )];
    assert_eq!(
        chest_placed(&waxed_exposed, items::WEATHERED_COPPER_CHEST, false),
        state("minecraft:exposed_copper_chest[facing=north,type=left]")
    );
    assert_eq!(
        chest_placed(&waxed_exposed, items::WAXED_OXIDIZED_COPPER_CHEST, false),
        state("minecraft:waxed_exposed_copper_chest[facing=north,type=left]")
    );
    assert_eq!(
        chest_placed(&copper, items::OXIDIZED_COPPER_CHEST, false),
        state("minecraft:copper_chest[facing=north,type=left]")
    );
}

#[test]
fn a_chest_placed_sneaking_against_the_side_of_a_chest_pairs_with_that_one() {
    // The chest clicked faces south; one facing north on the other side would be
    // taken were the player not sneaking.
    let world = TestWorld::default()
        .floor(63, blocks::STONE)
        .with(1, 64, 0, state("minecraft:chest[facing=south]"))
        .with(-1, 64, 0, state("minecraft:chest[facing=north]"));
    let mut click = click_top(at(1, 64, 0), items::CHEST);
    click.face = Face::West;
    click.cursor = [0.0, 0.5, 0.5];
    click.sneaking = true;
    assert_eq!(
        click_and_place(&click, &world),
        Some(vec![(
            at(0, 64, 0),
            state("minecraft:chest[facing=south,type=right]")
        )])
    );
    // Against a chest facing along the click, it faces the player and stays single.
    let along = TestWorld::default().floor(63, blocks::STONE).with(
        1,
        64,
        0,
        state("minecraft:chest[facing=east]"),
    );
    assert_eq!(
        click_and_place(&click, &along),
        Some(vec![(
            at(0, 64, 0),
            state("minecraft:chest[facing=north,type=single]")
        )])
    );
}

#[test]
fn a_rail_placed_facing_east_runs_east_and_west() {
    let world = TestWorld::default().floor(63, blocks::STONE);
    let mut click = click_top(at(0, 63, 0), items::RAIL);
    click.yaw = 270.0;
    assert_eq!(
        click_and_place(&click, &world),
        Some(vec![(
            at(0, 64, 0),
            state("minecraft:rail[shape=east_west]")
        )])
    );
    click.yaw = 0.0;
    assert_eq!(
        click_and_place(&click, &world),
        Some(vec![(
            at(0, 64, 0),
            state("minecraft:rail[shape=north_south]")
        )])
    );
}
