//! Tests of `clustine_sim::placement`, written from sections 3 to 5 of
//! `docs/groundwork/w6-plan.md` and the module's public interface alone, by someone
//! who has not read how the module does it.
//!
//! Every test builds a small world in memory: air wherever nothing is set, and no
//! block at all in a chunk it declares not loaded or outside the overworld's height.
//! A test judges what a click or the use of an item changes in the end; where the
//! plan says how a click is decided (a use, or the target of a placement), it judges
//! what `click` says as well.
//!
//! Where the plan leaves a choice open or seems to contradict itself, the test says
//! so in a comment beginning "Ambiguity" and follows the reading thought right.

use std::collections::{BTreeMap, BTreeSet};

use clustine_data::{BlockState, ITEMS, SupportType, block_id_by_name, blocks, items};
use clustine_sim::api::{Face, HOTBAR_SLOTS, ItemStack};
use clustine_sim::placement::{self, Blocks, Click, Clicked, ItemUse, UsedItem};
use clustine_world::{BlockPos, ChunkPos, Vec3};

/// The overworld's height: from y = -64 up to, and not including, y = 320.
const MIN_Y: i32 = -64;
const MAX_Y: i32 = 320;

/// The block most tests click: in the middle of chunk (0, 0), so that its neighbours
/// are in the same chunk.
const P: BlockPos = BlockPos::new(8, 64, 8);

/// Yaws in degrees as the protocol has them, by the horizontal direction they face.
const SOUTH: f32 = 0.0;
const WEST: f32 = 90.0;
const NORTH: f32 = 180.0;
const EAST: f32 = -90.0;

/// The state `text` names, without the `minecraft:` prefix.
fn state(text: &str) -> BlockState {
    BlockState::parse(&format!("minecraft:{text}"))
        .unwrap_or_else(|| panic!("{text} is not a block state of 26.3"))
}

/// A state as the game prints it, so that a failed comparison reads as blocks and not
/// as numbers.
fn describe(state: BlockState) -> String {
    let name = state.block().map_or("?", |block| block.name);
    let properties: Vec<String> = state
        .properties()
        .map(|(property, value)| format!("{property}={value}"))
        .collect();
    if properties.is_empty() {
        name.to_owned()
    } else {
        format!("{name}[{}]", properties.join(","))
    }
}

fn above(at: BlockPos) -> BlockPos {
    at.offset(0, 1, 0)
}

fn below(at: BlockPos) -> BlockPos {
    at.offset(0, -1, 0)
}

/// A world of a few blocks.
#[derive(Default)]
struct World {
    blocks: BTreeMap<BlockPos, BlockState>,
    unloaded: BTreeSet<ChunkPos>,
}

impl World {
    fn new() -> Self {
        Self::default()
    }

    fn with(mut self, at: BlockPos, text: &str) -> Self {
        self.blocks.insert(at, state(text));
        self
    }

    fn without_chunk(mut self, chunk: ChunkPos) -> Self {
        self.unloaded.insert(chunk);
        self
    }

    /// The world with an oak door of `properties` whose lower half is at `lower`.
    fn with_door(self, lower: BlockPos, properties: &str) -> Self {
        self.with(lower, &format!("oak_door[half=lower,{properties}]"))
            .with(above(lower), &format!("oak_door[half=upper,{properties}]"))
    }
}

impl Blocks for World {
    fn block(&self, at: BlockPos) -> Option<BlockState> {
        if at.y < MIN_Y || at.y >= MAX_Y || self.unloaded.contains(&at.chunk()) {
            return None;
        }
        Some(self.blocks.get(&at).copied().unwrap_or(blocks::AIR))
    }
}

/// Where a click lands on the middle of `face`.
fn centre_of(face: Face) -> [f32; 3] {
    match face {
        Face::Bottom => [0.5, 0.0, 0.5],
        Face::Top => [0.5, 1.0, 0.5],
        Face::North => [0.5, 0.5, 0.0],
        Face::South => [0.5, 0.5, 1.0],
        Face::West => [0.0, 0.5, 0.5],
        Face::East => [1.0, 0.5, 0.5],
    }
}

/// A click on the middle of `face` of `position`, not sneaking, by a player who stands
/// three blocks north of it, level with it, and looks south. Tests change what they
/// judge with the struct update syntax.
fn click_on(position: BlockPos, face: Face, held: Option<i32>) -> Click {
    Click {
        position,
        face,
        cursor: centre_of(face),
        sneaking: false,
        held,
        feet: Vec3::new(
            f64::from(position.x) + 0.5,
            f64::from(position.y),
            f64::from(position.z) - 3.5,
        ),
        yaw: SOUTH,
        pitch: 0.0,
    }
}

type Changes = BTreeMap<BlockPos, BlockState>;

fn as_changes(list: Vec<(BlockPos, BlockState)>) -> Changes {
    let count = list.len();
    let changes: Changes = list.into_iter().collect();
    assert_eq!(
        changes.len(),
        count,
        "a list of changes names a position twice"
    );
    changes
}

/// What a click changes, all told, with `bodies` the boxes of other players: a use's
/// changes, or the placement's where `click` says where to place. Empty when nothing
/// changes.
fn changes_among(click: &Click, world: &World, bodies: &[[f64; 6]]) -> Changes {
    match placement::click(click, world) {
        Clicked::Nothing => Changes::new(),
        Clicked::Used(list) => as_changes(list),
        Clicked::Place {
            target,
            replace_clicked,
            clicked,
        } => placement::place(click, target, replace_clicked, clicked, world, bodies)
            .map(as_changes)
            .unwrap_or_default(),
    }
}

fn changes(click: &Click, world: &World) -> Changes {
    changes_among(click, world, &[])
}

/// Where `click` says a block is to be placed, and whether that is the clicked block.
fn target_of(click: &Click, world: &World) -> Option<(BlockPos, bool)> {
    match placement::click(click, world) {
        Clicked::Place {
            target,
            replace_clicked,
            ..
        } => Some((target, replace_clicked)),
        _ => None,
    }
}

/// Whether a click comes to nothing. Ambiguity: the plan says a use of a block that
/// does nothing yet "consumes the click", and the interface has both `Nothing` and
/// `Used` with no changes for that; either is taken as right here.
fn comes_to_nothing(clicked: &Clicked) -> bool {
    match clicked {
        Clicked::Nothing => true,
        Clicked::Used(list) => list.is_empty(),
        Clicked::Place { .. } => false,
    }
}

#[track_caller]
fn assert_changes(actual: &Changes, expected: &[(BlockPos, &str)]) {
    let actual: BTreeMap<BlockPos, String> = actual
        .iter()
        .map(|(&at, &state)| (at, describe(state)))
        .collect();
    let expected: BTreeMap<BlockPos, String> = expected
        .iter()
        .map(|&(at, text)| (at, describe(state(text))))
        .collect();
    assert_eq!(actual, expected);
}

/// The value of `property` of the state that `changes` put at `at`.
#[track_caller]
fn property(changes: &Changes, at: BlockPos, property: &str) -> &'static str {
    let Some(&state) = changes.get(&at) else {
        panic!("nothing is placed at {at:?}; the changes are {changes:?}");
    };
    state
        .property(property)
        .unwrap_or_else(|| panic!("{} has no {property}", describe(state)))
}

/// The block that `changes` put at `at`, by name.
#[track_caller]
fn block_at(changes: &Changes, at: BlockPos) -> &'static str {
    let Some(&state) = changes.get(&at) else {
        panic!("nothing is placed at {at:?}; the changes are {changes:?}");
    };
    state.block().map_or("?", |block| block.name)
}

fn info(text: &str) -> &'static clustine_data::BlockInfo {
    state(text).block_info().expect("a block state has a block")
}

// Section 3.2: the order of a click.

#[test]
fn an_empty_hand_opens_both_halves_of_a_closed_door() {
    let world = World::new()
        .with(below(P), "stone")
        .with_door(P, "facing=north,open=false");
    let click = click_on(P, Face::North, None);
    assert!(matches!(placement::click(&click, &world), Clicked::Used(_)));
    assert_changes(
        &changes(&click, &world),
        &[
            (P, "oak_door[half=lower,facing=north,open=true]"),
            (above(P), "oak_door[half=upper,facing=north,open=true]"),
        ],
    );
}

#[test]
fn a_click_on_the_upper_half_of_an_open_door_closes_both_halves() {
    let world = World::new()
        .with(below(P), "stone")
        .with_door(P, "facing=east,hinge=right,open=true");
    let click = click_on(above(P), Face::West, None);
    assert_changes(
        &changes(&click, &world),
        &[
            (P, "oak_door[half=lower,facing=east,hinge=right,open=false]"),
            (
                above(P),
                "oak_door[half=upper,facing=east,hinge=right,open=false]",
            ),
        ],
    );
}

#[test]
fn a_block_in_hand_does_not_stop_a_door_being_used() {
    let world = World::new()
        .with(below(P), "stone")
        .with_door(P, "facing=north,open=false");
    let click = click_on(P, Face::North, Some(items::STONE));
    assert_changes(
        &changes(&click, &world),
        &[
            (P, "oak_door[half=lower,facing=north,open=true]"),
            (above(P), "oak_door[half=upper,facing=north,open=true]"),
        ],
    );
}

#[test]
fn sneaking_with_an_empty_hand_still_uses_a_door() {
    let world = World::new()
        .with(below(P), "stone")
        .with_door(P, "facing=north,open=false");
    let click = Click {
        sneaking: true,
        ..click_on(P, Face::North, None)
    };
    assert_changes(
        &changes(&click, &world),
        &[
            (P, "oak_door[half=lower,facing=north,open=true]"),
            (above(P), "oak_door[half=upper,facing=north,open=true]"),
        ],
    );
}

#[test]
fn sneaking_with_a_block_in_hand_places_against_a_door() {
    let world = World::new()
        .with(below(P), "stone")
        .with_door(P, "facing=north,open=false");
    let click = Click {
        sneaking: true,
        ..click_on(P, Face::North, Some(items::STONE))
    };
    let beside = Face::North.neighbour(P);
    assert_eq!(
        placement::click(&click, &world),
        Clicked::Place {
            target: beside,
            replace_clicked: false,
            clicked: state("oak_door[half=lower,facing=north,open=false]"),
        }
    );
    assert_changes(&changes(&click, &world), &[(beside, "stone")]);
}

#[test]
fn a_door_opens_only_the_clicked_half_when_the_other_is_not_its_match() {
    for other in [
        "stone",
        "spruce_door[half=upper,facing=north]",
        "oak_door[half=lower,facing=north]",
    ] {
        let world = World::new()
            .with(below(P), "stone")
            .with(P, "oak_door[half=lower,facing=north,open=false]")
            .with(above(P), other);
        let click = click_on(P, Face::North, None);
        assert_changes(
            &changes(&click, &world),
            &[(P, "oak_door[half=lower,facing=north,open=true]")],
        );
    }
}

#[test]
fn an_iron_door_is_not_used_and_a_block_in_hand_is_placed_against_it() {
    assert!(!info("iron_door").used_by_click);
    let world = World::new()
        .with(below(P), "stone")
        .with(P, "iron_door[half=lower,facing=north]")
        .with(above(P), "iron_door[half=upper,facing=north]");
    let empty_hand = click_on(P, Face::North, None);
    assert!(comes_to_nothing(&placement::click(&empty_hand, &world)));
    assert!(changes(&empty_hand, &world).is_empty());

    let with_stone = click_on(P, Face::North, Some(items::STONE));
    assert_changes(
        &changes(&with_stone, &world),
        &[(Face::North.neighbour(P), "stone")],
    );
}

#[test]
fn a_trapdoor_opens_and_closes() {
    for (before, after) in [("false", "true"), ("true", "false")] {
        let world = World::new().with(
            P,
            &format!("oak_trapdoor[facing=south,half=top,open={before}]"),
        );
        let click = click_on(P, Face::Top, None);
        assert_changes(
            &changes(&click, &world),
            &[(
                P,
                &format!("oak_trapdoor[facing=south,half=top,open={after}]"),
            )],
        );
    }
}

#[test]
fn an_iron_trapdoor_is_not_used_and_a_block_in_hand_is_placed_against_it() {
    assert!(!info("iron_trapdoor").used_by_click);
    let world = World::new().with(P, "iron_trapdoor[facing=south,half=bottom]");
    assert!(changes(&click_on(P, Face::Top, None), &world).is_empty());
    assert_changes(
        &changes(&click_on(P, Face::Top, Some(items::STONE)), &world),
        &[(above(P), "stone")],
    );
}

#[test]
fn a_fence_gate_opened_from_behind_turns_to_face_the_player() {
    // A gate facing north, opened by a player facing south (the opposite), turns south.
    let world = World::new().with(P, "oak_fence_gate[facing=north,open=false]");
    let click = Click {
        yaw: SOUTH,
        ..click_on(P, Face::North, None)
    };
    assert_changes(
        &changes(&click, &world),
        &[(P, "oak_fence_gate[facing=south,open=true]")],
    );

    let world = World::new().with(P, "oak_fence_gate[facing=east,open=false]");
    let click = Click {
        yaw: WEST,
        ..click_on(P, Face::East, Some(items::STONE))
    };
    assert_changes(
        &changes(&click, &world),
        &[(P, "oak_fence_gate[facing=west,open=true]")],
    );
}

#[test]
fn a_fence_gate_opened_from_the_front_or_the_side_keeps_its_facing() {
    for yaw in [NORTH, EAST, WEST] {
        let world = World::new().with(P, "oak_fence_gate[facing=north,open=false]");
        let click = Click {
            yaw,
            ..click_on(P, Face::South, None)
        };
        assert_changes(
            &changes(&click, &world),
            &[(P, "oak_fence_gate[facing=north,open=true]")],
        );
    }
}

#[test]
fn an_open_fence_gate_closes_whichever_way_the_player_faces() {
    for yaw in [SOUTH, NORTH, EAST, WEST] {
        let world = World::new().with(P, "oak_fence_gate[facing=north,open=true]");
        let click = Click {
            yaw,
            ..click_on(P, Face::North, None)
        };
        assert_changes(
            &changes(&click, &world),
            &[(P, "oak_fence_gate[facing=north,open=false]")],
        );
    }
}

#[test]
fn an_empty_flower_pot_takes_a_plant_that_a_pot_holds() {
    for (item, potted) in [
        (items::POPPY, "potted_poppy"),
        (items::AZALEA, "potted_azalea_bush"),
        (items::BAMBOO, "potted_bamboo"),
    ] {
        let world = World::new().with(below(P), "stone").with(P, "flower_pot");
        let click = click_on(P, Face::Top, Some(item));
        assert!(matches!(placement::click(&click, &world), Clicked::Used(_)));
        assert_changes(&changes(&click, &world), &[(P, potted)]);
    }
}

#[test]
fn a_flower_pot_takes_nothing_else_and_nothing_is_placed_on_it() {
    for (pot, held) in [
        ("flower_pot", Some(items::STONE)),
        ("flower_pot", None),
        ("potted_poppy", Some(items::POPPY)),
        ("potted_poppy", Some(items::STONE)),
        ("potted_poppy", None),
    ] {
        let world = World::new().with(below(P), "stone").with(P, pot);
        let click = click_on(P, Face::Top, held);
        assert!(
            comes_to_nothing(&placement::click(&click, &world)),
            "{pot} clicked with {held:?}"
        );
    }
}

#[test]
fn a_chest_is_used_and_nothing_happens() {
    let world = World::new().with(P, "chest[facing=north]");
    let click = click_on(P, Face::Top, Some(items::STONE));
    assert!(comes_to_nothing(&placement::click(&click, &world)));
    assert!(changes(&click, &world).is_empty());
}

#[test]
fn sneaking_with_a_block_in_hand_places_on_a_chest() {
    let world = World::new().with(P, "chest[facing=north]");
    let click = Click {
        sneaking: true,
        ..click_on(P, Face::Top, Some(items::STONE))
    };
    assert_changes(&changes(&click, &world), &[(above(P), "stone")]);
}

#[test]
fn a_block_in_hand_is_placed_on_a_fence() {
    // Ambiguity, and what seems a contradiction: the jar's data has a fence
    // `used_by_click` (its class handles a click, for leads), so section 3.2 read to
    // the letter would place nothing on a fence. Vanilla places a block on a fence
    // when no leashed animal is tied to it, which is every time in Clustine, and a
    // player places torches and blocks on fences within minutes. The plan names the
    // blocks "that vanilla uses only in some states and places against in others",
    // and a fence is one of them by what vanilla does; this test takes vanilla's
    // behaviour as right.
    let world = World::new().with(P, "oak_fence");
    let click = click_on(P, Face::Top, Some(items::STONE));
    assert_changes(&changes(&click, &world), &[(above(P), "stone")]);
}

#[test]
fn an_empty_hand_never_places() {
    for (at, text) in [(P, "stone"), (P, "short_grass"), (P, "air"), (P, "water")] {
        let world = World::new().with(below(P), "grass_block").with(at, text);
        for face in [Face::Top, Face::North, Face::East] {
            let click = click_on(P, face, None);
            assert!(
                comes_to_nothing(&placement::click(&click, &world)),
                "{text} clicked on {face:?}"
            );
        }
    }
}

#[test]
fn an_item_that_places_no_block_on_a_click_places_nothing() {
    for held in [
        items::STICK,
        items::WATER_BUCKET,
        items::BUCKET,
        items::LILY_PAD,
        items::COMMAND_BLOCK,
    ] {
        for (text, face) in [("stone", Face::Top), ("water", Face::Top)] {
            let world = World::new().with(below(P), "stone").with(P, text);
            let click = click_on(P, face, Some(held));
            assert!(
                changes(&click, &world).is_empty(),
                "{} clicked on {text}",
                ITEMS[held as usize].name
            );
        }
    }
}

// Section 4.1: the context.

#[test]
fn a_click_on_short_grass_places_in_its_place() {
    let world = World::new()
        .with(below(P), "grass_block")
        .with(P, "short_grass");
    for face in [Face::Top, Face::North, Face::East] {
        let click = click_on(P, face, Some(items::STONE));
        assert_eq!(target_of(&click, &world), Some((P, true)), "{face:?}");
        assert_changes(&changes(&click, &world), &[(P, "stone")]);
    }
}

#[test]
fn a_click_on_air_places_there() {
    let world = World::new().with(below(P), "stone");
    for face in [Face::Top, Face::South, Face::West] {
        let click = click_on(P, face, Some(items::STONE));
        assert_eq!(target_of(&click, &world), Some((P, true)), "{face:?}");
        assert_changes(&changes(&click, &world), &[(P, "stone")]);
    }
}

#[test]
fn a_click_on_water_or_lava_places_there() {
    for fluid in ["water", "water[level=3]", "lava"] {
        let world = World::new().with(below(P), "stone").with(P, fluid);
        let click = click_on(P, Face::Top, Some(items::STONE));
        assert_eq!(target_of(&click, &world), Some((P, true)), "{fluid}");
        assert_changes(&changes(&click, &world), &[(P, "stone")]);
    }
}

#[test]
fn a_click_on_a_solid_block_places_beside_it_across_the_face() {
    let faces = [
        Face::Bottom,
        Face::Top,
        Face::North,
        Face::South,
        Face::West,
        Face::East,
    ];
    for face in faces {
        let world = World::new().with(P, "stone");
        let click = click_on(P, face, Some(items::OAK_PLANKS));
        let beside = face.neighbour(P);
        assert_eq!(target_of(&click, &world), Some((beside, false)), "{face:?}");
        assert_changes(&changes(&click, &world), &[(beside, "oak_planks")]);
    }
}

#[test]
fn nothing_is_placed_where_the_block_beside_cannot_be_replaced() {
    let world = World::new().with(P, "stone").with(above(P), "dirt");
    let click = click_on(P, Face::Top, Some(items::STONE));
    assert!(changes(&click, &world).is_empty());
}

#[test]
fn the_horizontal_direction_follows_the_yaw_alone() {
    // A furnace faces the opposite of the horizontal direction (section 4.4).
    for (yaw, pitch, facing) in [
        (0.0, 0.0, "north"),
        (44.0, 0.0, "north"),
        (-44.0, 0.0, "north"),
        (46.0, 0.0, "east"),
        (90.0, 0.0, "east"),
        (180.0, 0.0, "south"),
        (-180.0, 0.0, "south"),
        (-90.0, 0.0, "west"),
        (270.0, 0.0, "west"),
        (450.0, 0.0, "east"),
        (0.0, 80.0, "north"),
        (0.0, -80.0, "north"),
        (90.0, 89.0, "east"),
    ] {
        let world = World::new().with(P, "stone");
        let click = Click {
            yaw,
            pitch,
            ..click_on(P, Face::Top, Some(items::FURNACE))
        };
        let placed = changes(&click, &world);
        assert_eq!(
            property(&placed, above(P), "facing"),
            facing,
            "yaw {yaw}, pitch {pitch}"
        );
    }
}

#[test]
fn the_cursors_height_is_measured_from_the_target() {
    // Ambiguity: the cursor's height above `T` differs from the cursor's y only when
    // `T` is above or below `P`, that is when `F` is up or down, and every rule that
    // reads the height decides those faces by `F` first. So the difference can be seen
    // only where an implementation took the cursor's y for the height: a click on the
    // top of a block lands at y 1 and one on the bottom at y 0.
    let world = World::new().with(P, "stone");
    let on_top = changes(&click_on(P, Face::Top, Some(items::OAK_SLAB)), &world);
    assert_changes(&on_top, &[(above(P), "oak_slab[type=bottom]")]);
    let underneath = changes(&click_on(P, Face::Bottom, Some(items::OAK_SLAB)), &world);
    assert_changes(&underneath, &[(below(P), "oak_slab[type=top]")]);

    let on_top = changes(&click_on(P, Face::Top, Some(items::OAK_STAIRS)), &world);
    assert_eq!(property(&on_top, above(P), "half"), "bottom");
    let underneath = changes(&click_on(P, Face::Bottom, Some(items::OAK_STAIRS)), &world);
    assert_eq!(property(&underneath, below(P), "half"), "top");
}

// Section 4.2: can be replaced.

#[test]
fn a_block_is_not_replaced_by_its_own_item() {
    let world = World::new()
        .with(below(P), "grass_block")
        .with(below(P).offset(1, 0, 0), "grass_block")
        .with(P, "short_grass");
    let click = click_on(P, Face::East, Some(items::SHORT_GRASS));
    let beside = Face::East.neighbour(P);
    assert_eq!(target_of(&click, &world), Some((beside, false)));
    assert_changes(&changes(&click, &world), &[(beside, "short_grass")]);
}

#[test]
fn snow_in_hand_adds_a_layer_to_snow_clicked_on_top() {
    for (before, after) in [(1, 2), (3, 4), (7, 8)] {
        let world = World::new()
            .with(below(P), "stone")
            .with(P, &format!("snow[layers={before}]"));
        let click = click_on(P, Face::Top, Some(items::SNOW));
        assert_eq!(target_of(&click, &world), Some((P, true)));
        assert_changes(
            &changes(&click, &world),
            &[(P, &format!("snow[layers={after}]"))],
        );
    }
}

#[test]
fn snow_in_hand_on_the_side_of_snow_goes_beside_and_joins_snow_there() {
    // Ambiguity: section 4.2's snow rule depends on `replaceClicked`, which is itself
    // what 4.2 decides for `P`. Read as vanilla's `BlockPlaceContext` does: judging
    // `P`, `replaceClicked` is taken as true; judging the block beside, as false.
    let beside = Face::East.neighbour(P);
    let world = World::new()
        .with(below(P), "stone")
        .with(below(beside), "stone")
        .with(P, "snow[layers=3]");
    let click = click_on(P, Face::East, Some(items::SNOW));
    assert_eq!(target_of(&click, &world), Some((beside, false)));
    assert_changes(&changes(&click, &world), &[(beside, "snow[layers=1]")]);

    let world = World::new()
        .with(below(P), "stone")
        .with(below(beside), "stone")
        .with(P, "snow[layers=3]")
        .with(beside, "snow[layers=2]");
    assert_changes(&changes(&click, &world), &[(beside, "snow[layers=3]")]);

    let world = World::new()
        .with(below(P), "stone")
        .with(below(beside), "stone")
        .with(P, "snow[layers=3]")
        .with(beside, "snow[layers=8]");
    assert!(changes(&click, &world).is_empty());
}

#[test]
fn eight_layers_of_snow_take_no_more_and_snow_goes_on_top() {
    let world = World::new()
        .with(below(P), "stone")
        .with(P, "snow[layers=8]");
    let click = click_on(P, Face::Top, Some(items::SNOW));
    assert_eq!(target_of(&click, &world), Some((above(P), false)));
    assert_changes(&changes(&click, &world), &[(above(P), "snow[layers=1]")]);
}

#[test]
fn one_layer_of_snow_is_replaced_by_another_block_and_more_are_not() {
    let world = World::new()
        .with(below(P), "stone")
        .with(P, "snow[layers=1]");
    let click = click_on(P, Face::Top, Some(items::STONE));
    assert_changes(&changes(&click, &world), &[(P, "stone")]);

    let world = World::new()
        .with(below(P), "stone")
        .with(P, "snow[layers=2]");
    assert_changes(&changes(&click, &world), &[(above(P), "stone")]);
}

#[test]
fn a_slab_joins_a_slab_of_its_block_clicked_on_its_open_side() {
    let world = World::new().with(P, "stone_slab[type=bottom]");
    let click = click_on(P, Face::Top, Some(items::STONE_SLAB));
    assert_eq!(target_of(&click, &world), Some((P, true)));
    assert_changes(&changes(&click, &world), &[(P, "stone_slab[type=double]")]);

    let world = World::new().with(P, "stone_slab[type=top]");
    let click = click_on(P, Face::Bottom, Some(items::STONE_SLAB));
    assert_eq!(target_of(&click, &world), Some((P, true)));
    assert_changes(&changes(&click, &world), &[(P, "stone_slab[type=double]")]);
}

#[test]
fn a_slab_clicked_on_its_side_joins_by_the_cursors_height() {
    let east = Face::East.neighbour(P);
    let side = |height: f32| Click {
        cursor: [1.0, height, 0.5],
        yaw: WEST,
        ..click_on(P, Face::East, Some(items::STONE_SLAB))
    };

    let bottom = World::new().with(P, "stone_slab[type=bottom]");
    assert_changes(
        &changes(&side(0.7), &bottom),
        &[(P, "stone_slab[type=double]")],
    );
    assert_changes(
        &changes(&side(0.3), &bottom),
        &[(east, "stone_slab[type=bottom]")],
    );

    let top = World::new().with(P, "stone_slab[type=top]");
    assert_changes(
        &changes(&side(0.3), &top),
        &[(P, "stone_slab[type=double]")],
    );
    assert_changes(
        &changes(&side(0.7), &top),
        &[(east, "stone_slab[type=top]")],
    );
}

#[test]
fn a_slab_clicked_on_its_full_side_goes_beside() {
    let world = World::new().with(P, "stone_slab[type=bottom]");
    let click = click_on(P, Face::Bottom, Some(items::STONE_SLAB));
    assert_eq!(target_of(&click, &world), Some((below(P), false)));
    assert_changes(
        &changes(&click, &world),
        &[(below(P), "stone_slab[type=top]")],
    );

    let world = World::new().with(P, "stone_slab[type=top]");
    let click = click_on(P, Face::Top, Some(items::STONE_SLAB));
    assert_eq!(target_of(&click, &world), Some((above(P), false)));
    assert_changes(
        &changes(&click, &world),
        &[(above(P), "stone_slab[type=bottom]")],
    );
}

#[test]
fn a_slab_of_another_block_does_not_join() {
    let world = World::new().with(P, "stone_slab[type=bottom]");
    let click = click_on(P, Face::Top, Some(items::OAK_SLAB));
    assert_changes(
        &changes(&click, &world),
        &[(above(P), "oak_slab[type=bottom]")],
    );
}

#[test]
fn a_slab_beside_the_clicked_block_is_joined_whatever_the_cursor() {
    let east = Face::East.neighbour(P);
    for (there, height) in [("bottom", 0.3), ("bottom", 0.7), ("top", 0.3), ("top", 0.7)] {
        let world = World::new()
            .with(P, "stone")
            .with(east, &format!("stone_slab[type={there}]"));
        let click = Click {
            cursor: [1.0, height, 0.5],
            yaw: WEST,
            ..click_on(P, Face::East, Some(items::STONE_SLAB))
        };
        assert_eq!(target_of(&click, &world), Some((east, false)));
        assert_changes(
            &changes(&click, &world),
            &[(east, "stone_slab[type=double]")],
        );
    }
}

#[test]
fn a_double_slab_is_not_waterlogged() {
    let world = World::new().with(P, "stone_slab[type=bottom,waterlogged=true]");
    let click = click_on(P, Face::Top, Some(items::STONE_SLAB));
    assert_changes(
        &changes(&click, &world),
        &[(P, "stone_slab[type=double,waterlogged=false]")],
    );
}

#[test]
fn candles_and_the_like_in_hand_add_one_more_while_not_sneaking() {
    for (item, before, count, most) in [
        (items::CANDLE, "candle", "candles", 4),
        (items::SEA_PICKLE, "sea_pickle", "pickles", 4),
        (items::TURTLE_EGG, "turtle_egg", "eggs", 4),
        (items::PINK_PETALS, "pink_petals", "flower_amount", 4),
        (items::LEAF_LITTER, "leaf_litter", "segment_amount", 4),
    ] {
        for held in 1..most {
            let world = World::new()
                .with(below(P), "grass_block")
                .with(P, &format!("{before}[{count}={held}]"));
            let click = click_on(P, Face::Top, Some(item));
            assert_eq!(target_of(&click, &world), Some((P, true)), "{before}");
            let placed = changes(&click, &world);
            assert_eq!(block_at(&placed, P), format!("minecraft:{before}"));
            assert_eq!(
                property(&placed, P, count),
                (held + 1).to_string(),
                "{before} with {held}"
            );
        }
    }
}

#[test]
fn candles_and_the_like_in_hand_go_beside_while_sneaking_or_when_full() {
    for (item, before, count) in [
        (items::CANDLE, "candle", "candles"),
        (items::SEA_PICKLE, "sea_pickle", "pickles"),
        (items::TURTLE_EGG, "turtle_egg", "eggs"),
        (items::PINK_PETALS, "pink_petals", "flower_amount"),
        (items::LEAF_LITTER, "leaf_litter", "segment_amount"),
    ] {
        let world = World::new()
            .with(below(P), "grass_block")
            .with(P, &format!("{before}[{count}=1]"));
        let sneaking = Click {
            sneaking: true,
            ..click_on(P, Face::Top, Some(item))
        };
        assert_eq!(
            target_of(&sneaking, &world),
            Some((above(P), false)),
            "{before} while sneaking"
        );

        let world = World::new()
            .with(below(P), "grass_block")
            .with(P, &format!("{before}[{count}=4]"));
        let click = click_on(P, Face::Top, Some(item));
        assert_eq!(
            target_of(&click, &world),
            Some((above(P), false)),
            "{before} full"
        );
    }
}

#[test]
fn a_candle_is_not_added_to_a_candle_of_another_colour() {
    // Ambiguity: "a block of the same kind in hand" could mean any candle; read as the
    // same block, as vanilla's `CandleBlock.canBeReplaced` asks of the item.
    let world = World::new()
        .with(below(P), "stone")
        .with(P, "candle[candles=1]");
    let click = click_on(P, Face::Top, Some(items::WHITE_CANDLE));
    assert_eq!(target_of(&click, &world), Some((above(P), false)));
}

#[test]
fn glow_lichen_is_replaced_by_another_block() {
    // Ambiguity: "replaceable unless the item in hand is its own, or while a face is
    // still free" can be read two ways when lichen is in hand; only the part both
    // readings share is tested: with another block in hand, lichen is replaced.
    let world = World::new()
        .with(below(P), "stone")
        .with(P, "glow_lichen[down=true]");
    let click = click_on(P, Face::Top, Some(items::STONE));
    assert_eq!(target_of(&click, &world), Some((P, true)));
    assert_changes(&changes(&click, &world), &[(P, "stone")]);
}

// Section 4.3: survival, obstruction and all or nothing.

#[test]
fn a_plant_is_refused_on_stone_and_placed_on_grass() {
    for item in [items::POPPY, items::SHORT_GRASS] {
        let stone = World::new().with(P, "stone");
        assert!(changes(&click_on(P, Face::Top, Some(item)), &stone).is_empty());

        let grass = World::new().with(P, "grass_block");
        let placed = changes(&click_on(P, Face::Top, Some(item)), &grass);
        assert_eq!(placed.len(), 1);
        assert_eq!(
            placed[&above(P)].block_id(),
            ITEMS[item as usize].places.map(usize::from),
        );
    }
}

#[test]
fn seeds_are_placed_on_farmland_and_not_on_grass() {
    let farmland = World::new().with(P, "farmland");
    assert_changes(
        &changes(&click_on(P, Face::Top, Some(items::WHEAT_SEEDS)), &farmland),
        &[(above(P), "wheat")],
    );
    let grass = World::new().with(P, "grass_block");
    assert!(changes(&click_on(P, Face::Top, Some(items::WHEAT_SEEDS)), &grass).is_empty());
}

#[test]
fn a_torch_goes_on_the_side_of_a_block_and_on_its_top() {
    let world = World::new().with(P, "stone");
    let side = Click {
        yaw: WEST,
        ..click_on(P, Face::East, Some(items::TORCH))
    };
    assert_changes(
        &changes(&side, &world),
        &[(Face::East.neighbour(P), "wall_torch[facing=east]")],
    );
    let top = click_on(P, Face::Top, Some(items::TORCH));
    assert_changes(&changes(&top, &world), &[(above(P), "torch")]);
}

#[test]
fn a_torch_on_nothing_is_not_placed() {
    let world = World::new();
    let click = click_on(P, Face::Top, Some(items::TORCH));
    assert_eq!(target_of(&click, &world), Some((P, true)));
    assert!(changes(&click, &world).is_empty());
}

#[test]
fn a_torch_clicked_under_a_block_stands_on_what_is_below_it() {
    // The attachment direction of a torch is down, so up is skipped and the first
    // looking direction that can hold a torch is down, onto the stone below.
    let world = World::new().with(P, "stone").with(below(below(P)), "stone");
    let click = Click {
        yaw: NORTH,
        ..click_on(P, Face::Bottom, Some(items::TORCH))
    };
    assert_changes(&changes(&click, &world), &[(below(P), "torch")]);
}

#[test]
fn a_door_needs_a_sturdy_top_below() {
    for (ground, placed) in [
        ("stone", true),
        ("stone_slab[type=top]", true),
        ("stone_slab[type=bottom]", false),
        ("air", false),
    ] {
        let world = World::new().with(P, ground);
        let click = click_on(P, Face::Top, Some(items::OAK_DOOR));
        let changed = changes(&click, &world);
        assert_eq!(!changed.is_empty(), placed, "a door on {ground}");
    }
}

#[test]
fn a_bed_needs_no_support() {
    let at = BlockPos::new(8, 100, 8);
    let world = World::new();
    let click = Click {
        yaw: SOUTH,
        ..click_on(at, Face::Top, Some(items::RED_BED))
    };
    assert_changes(
        &changes(&click, &world),
        &[
            (at, "red_bed[facing=south,part=foot,occupied=false]"),
            (
                at.offset(0, 0, 1),
                "red_bed[facing=south,part=head,occupied=false]",
            ),
        ],
    );
}

#[test]
fn a_block_is_not_placed_into_another_players_box() {
    let world = World::new().with(P, "stone");
    let click = click_on(P, Face::Top, Some(items::STONE));
    let t = above(P);
    let standing_in = placement::body(Vec3::new(8.5, f64::from(t.y) - 0.5, 8.5), false);
    assert!(changes_among(&click, &world, &[standing_in]).is_empty());
    let at_a_corner = placement::body(Vec3::new(8.1, f64::from(t.y) + 0.9, 8.1), false);
    assert!(changes_among(&click, &world, &[at_a_corner]).is_empty());
}

#[test]
fn a_player_who_only_touches_the_block_does_not_stop_it() {
    let world = World::new().with(P, "stone");
    let click = click_on(P, Face::Top, Some(items::STONE));
    let t = above(P);
    // Boxes written out, so that they touch the block's faces exactly.
    let top = f64::from(t.y) + 1.0;
    let on_top = [8.2, top, 8.2, 8.8, top + 1.8, 8.8];
    let beside = [9.0, f64::from(t.y), 8.2, 9.6, top + 0.8, 8.8];
    assert_changes(
        &changes_among(&click, &world, &[on_top, beside]),
        &[(t, "stone")],
    );
}

#[test]
fn the_placers_own_box_is_made_from_the_click_and_is_lower_while_sneaking() {
    // The placer floats with their head reaching into the target: 1.8 high standing,
    // it reaches 0.1 into the block; 1.5 high sneaking, it stays 0.2 below it.
    let wall = BlockPos::new(9, 64, 8);
    let t = BlockPos::new(8, 64, 8);
    let world = World::new().with(wall, "stone");
    let standing = Click {
        feet: Vec3::new(8.5, 62.3, 8.5),
        yaw: EAST,
        ..click_on(wall, Face::West, Some(items::STONE))
    };
    assert_eq!(target_of(&standing, &world), Some((t, false)));
    assert!(changes(&standing, &world).is_empty());

    let sneaking = Click {
        sneaking: true,
        ..standing.clone()
    };
    assert_changes(&changes(&sneaking, &world), &[(t, "stone")]);
}

#[test]
fn the_body_of_a_player_is_as_the_plan_says() {
    let feet = Vec3::new(1.0, 2.0, 3.0);
    let close = |a: [f64; 6], b: [f64; 6]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-9);
    assert!(close(
        placement::body(feet, false),
        [0.7, 2.0, 2.7, 1.3, 3.8, 3.3]
    ));
    assert!(close(
        placement::body(feet, true),
        [0.7, 2.0, 2.7, 1.3, 3.5, 3.3]
    ));
}

#[test]
fn a_block_without_collision_is_placed_where_a_player_stands() {
    let world = World::new().with(P, "stone");
    let t = above(P);
    let standing_in = placement::body(Vec3::new(8.5, f64::from(t.y), 8.5), false);
    let click = click_on(P, Face::Top, Some(items::TORCH));
    assert_changes(
        &changes_among(&click, &world, &[standing_in]),
        &[(t, "torch")],
    );
    // The placer standing there does not stop it either.
    let own = Click {
        feet: Vec3::new(8.5, f64::from(t.y), 8.5),
        pitch: 90.0,
        ..click
    };
    assert_changes(&changes(&own, &world), &[(t, "torch")]);
}

#[test]
fn only_the_target_is_checked_for_players() {
    // A player whose box is only in the upper half of a door or in a bed's head does
    // not stop either.
    let world = World::new().with(P, "stone");
    let t = above(P);
    let upper_only = placement::body(Vec3::new(8.5, f64::from(t.y) + 1.0, 8.5), false);
    let door = click_on(P, Face::Top, Some(items::OAK_DOOR));
    let placed = changes_among(&door, &world, &[upper_only]);
    assert_eq!(property(&placed, t, "half"), "lower");
    assert_eq!(property(&placed, above(t), "half"), "upper");

    let head = t.offset(0, 0, 1);
    let in_head = placement::body(Vec3::new(8.5, f64::from(t.y), 9.5), false);
    let bed = Click {
        yaw: SOUTH,
        ..click_on(P, Face::Top, Some(items::RED_BED))
    };
    let placed = changes_among(&bed, &world, &[in_head]);
    assert_eq!(property(&placed, t, "part"), "foot");
    assert_eq!(property(&placed, head, "part"), "head");
}

#[test]
fn nothing_is_placed_where_a_block_that_would_change_is_not_loaded() {
    // A bed whose head is in a chunk not loaded.
    let t = BlockPos::new(8, 64, 15);
    let world = World::new()
        .with(below(t), "stone")
        .without_chunk(ChunkPos::new(0, 1));
    let bed = Click {
        yaw: SOUTH,
        ..click_on(below(t), Face::Top, Some(items::RED_BED))
    };
    assert!(changes(&bed, &world).is_empty());

    // The same bed facing north fits.
    let north = Click { yaw: NORTH, ..bed };
    assert_eq!(changes(&north, &world).len(), 2);
}

#[test]
fn a_target_not_loaded_is_never_read_as_air() {
    let t = BlockPos::new(16, 64, 8);
    let world = World::new()
        .with(BlockPos::new(15, 64, 8), "stone")
        .without_chunk(ChunkPos::new(1, 0));
    let click = Click {
        yaw: EAST,
        ..click_on(BlockPos::new(15, 64, 8), Face::East, Some(items::STONE))
    };
    assert!(
        placement::place(&click, t, false, state("stone"), &world, &[]).is_none(),
        "a target in a chunk not loaded"
    );
}

#[test]
fn a_door_whose_upper_half_would_be_outside_the_world_is_not_placed() {
    let ground = BlockPos::new(8, MAX_Y - 2, 8);
    let world = World::new().with(ground, "stone");
    let click = click_on(ground, Face::Top, Some(items::OAK_DOOR));
    assert!(changes(&click, &world).is_empty());

    // One lower, it fits.
    let ground = BlockPos::new(8, MAX_Y - 3, 8);
    let world = World::new().with(ground, "stone");
    let click = click_on(ground, Face::Top, Some(items::OAK_DOOR));
    assert_eq!(changes(&click, &world).len(), 2);
}

#[test]
fn nothing_is_placed_above_the_top_of_the_world() {
    let top = BlockPos::new(8, MAX_Y - 1, 8);
    let world = World::new().with(top, "stone");
    let click = click_on(top, Face::Top, Some(items::STONE));
    assert!(changes(&click, &world).is_empty());
}

// Section 4.4: the rules, by the class of the block placed.

#[test]
fn a_pillar_takes_the_axis_of_the_face() {
    for item in [items::OAK_LOG, items::IRON_CHAIN] {
        for (face, axis) in [
            (Face::Top, "y"),
            (Face::Bottom, "y"),
            (Face::North, "z"),
            (Face::South, "z"),
            (Face::West, "x"),
            (Face::East, "x"),
        ] {
            let world = World::new().with(P, "stone");
            let placed = changes(&click_on(P, face, Some(item)), &world);
            assert_eq!(property(&placed, face.neighbour(P), "axis"), axis);
        }
    }
}

#[test]
fn blocks_that_face_the_player_face_the_opposite_of_the_horizontal_direction() {
    for item in [
        items::FURNACE,
        items::SMOKER,
        items::BLAST_FURNACE,
        items::CARVED_PUMPKIN,
        items::BEEHIVE,
        items::LOOM,
        items::STONECUTTER,
        items::LECTERN,
        items::WHITE_GLAZED_TERRACOTTA,
        items::ENDER_CHEST,
        items::CHISELED_BOOKSHELF,
        items::END_PORTAL_FRAME,
        items::REPEATER,
        items::COMPARATOR,
        items::CHEST,
    ] {
        for (yaw, facing) in [
            (SOUTH, "north"),
            (WEST, "east"),
            (NORTH, "south"),
            (EAST, "west"),
        ] {
            let world = World::new().with(P, "stone");
            let click = Click {
                yaw,
                pitch: 45.0,
                ..click_on(P, Face::Top, Some(item))
            };
            let placed = changes(&click, &world);
            assert_eq!(
                property(&placed, above(P), "facing"),
                facing,
                "{} at yaw {yaw}",
                ITEMS[item as usize].name
            );
        }
    }
}

#[test]
fn a_chest_is_placed_as_half_of_a_double_chest_beside_a_chest_facing_its_way() {
    // Facing north, the chest to its east is on its left (phase B, step B7).
    let east = P.offset(1, 1, 0);
    let world = World::new()
        .with(P, "stone")
        .with(east, "chest[facing=north,type=single]");
    let placed = changes(&click_on(P, Face::Top, Some(items::CHEST)), &world);
    assert_changes(
        &placed,
        &[(above(P), "chest[facing=north,type=left,waterlogged=false]")],
    );
}

#[test]
fn a_campfire_faces_the_horizontal_direction_and_is_lit() {
    let world = World::new().with(P, "stone");
    let click = Click {
        yaw: WEST,
        ..click_on(P, Face::Top, Some(items::CAMPFIRE))
    };
    assert_changes(
        &changes(&click, &world),
        &[(
            above(P),
            "campfire[facing=west,lit=true,signal_fire=false,waterlogged=false]",
        )],
    );
}

#[test]
fn a_campfire_on_hay_is_a_signal_fire() {
    let world = World::new().with(P, "hay_block");
    let placed = changes(&click_on(P, Face::Top, Some(items::CAMPFIRE)), &world);
    assert_eq!(property(&placed, above(P), "signal_fire"), "true");
}

#[test]
fn a_campfire_in_water_is_waterlogged_and_not_lit() {
    let world = World::new().with(below(P), "stone").with(P, "water");
    let placed = changes(&click_on(P, Face::Top, Some(items::CAMPFIRE)), &world);
    assert_eq!(property(&placed, P, "waterlogged"), "true");
    assert_eq!(property(&placed, P, "lit"), "false");
}

#[test]
fn an_anvil_faces_the_horizontal_direction_turned_clockwise() {
    for (yaw, facing) in [
        (SOUTH, "west"),
        (WEST, "north"),
        (NORTH, "east"),
        (EAST, "south"),
    ] {
        let world = World::new().with(P, "stone");
        let click = Click {
            yaw,
            ..click_on(P, Face::Top, Some(items::ANVIL))
        };
        let placed = changes(&click, &world);
        assert_eq!(property(&placed, above(P), "facing"), facing, "yaw {yaw}");
    }
}

#[test]
fn barrels_dispensers_droppers_and_pistons_face_away_from_where_the_player_looks() {
    for item in [
        items::BARREL,
        items::DISPENSER,
        items::DROPPER,
        items::PISTON,
    ] {
        for (yaw, pitch, facing) in [
            (SOUTH, 90.0, "up"),
            (SOUTH, -90.0, "down"),
            (SOUTH, 0.0, "north"),
            (WEST, 10.0, "east"),
        ] {
            let world = World::new().with(P, "stone");
            let click = Click {
                yaw,
                pitch,
                ..click_on(P, Face::Top, Some(item))
            };
            let placed = changes(&click, &world);
            assert_eq!(
                property(&placed, above(P), "facing"),
                facing,
                "{} at yaw {yaw} and pitch {pitch}",
                ITEMS[item as usize].name
            );
        }
    }
}

#[test]
fn an_observer_faces_where_the_player_looks() {
    for (yaw, pitch, facing) in [
        (SOUTH, 90.0, "down"),
        (SOUTH, -90.0, "up"),
        (SOUTH, 0.0, "south"),
        (EAST, -10.0, "east"),
    ] {
        let world = World::new().with(P, "stone");
        let click = Click {
            yaw,
            pitch,
            ..click_on(P, Face::Top, Some(items::OBSERVER))
        };
        let placed = changes(&click, &world);
        assert_eq!(property(&placed, above(P), "facing"), facing);
    }
}

#[test]
fn an_end_rod_faces_the_face_and_turns_against_an_end_rod_facing_it() {
    let world = World::new().with(P, "stone");
    let placed = changes(&click_on(P, Face::Top, Some(items::END_ROD)), &world);
    assert_eq!(property(&placed, above(P), "facing"), "up");
    let placed = changes(&click_on(P, Face::East, Some(items::END_ROD)), &world);
    assert_eq!(property(&placed, Face::East.neighbour(P), "facing"), "east");

    let world = World::new().with(P, "end_rod[facing=up]");
    let placed = changes(&click_on(P, Face::Top, Some(items::END_ROD)), &world);
    assert_eq!(property(&placed, above(P), "facing"), "down");
    let placed = changes(&click_on(P, Face::East, Some(items::END_ROD)), &world);
    assert_eq!(property(&placed, Face::East.neighbour(P), "facing"), "east");
}

#[test]
fn a_lightning_rod_faces_the_face_even_on_a_lightning_rod() {
    let world = World::new().with(P, "lightning_rod[facing=up]");
    let placed = changes(&click_on(P, Face::Top, Some(items::LIGHTNING_ROD)), &world);
    assert_eq!(property(&placed, above(P), "facing"), "up");
}

#[test]
fn stairs_face_the_horizontal_direction_and_take_their_half_from_the_click() {
    let world = World::new().with(P, "stone");
    let east = Face::East.neighbour(P);
    let side = |height: f32| Click {
        cursor: [1.0, height, 0.5],
        yaw: WEST,
        ..click_on(P, Face::East, Some(items::OAK_STAIRS))
    };
    assert_changes(
        &changes(&side(0.7), &world),
        &[(
            east,
            "oak_stairs[facing=west,half=top,shape=straight,waterlogged=false]",
        )],
    );
    assert_changes(
        &changes(&side(0.3), &world),
        &[(
            east,
            "oak_stairs[facing=west,half=bottom,shape=straight,waterlogged=false]",
        )],
    );
    let on_top = Click {
        yaw: NORTH,
        ..click_on(P, Face::Top, Some(items::OAK_STAIRS))
    };
    assert_changes(
        &changes(&on_top, &world),
        &[(
            above(P),
            "oak_stairs[facing=north,half=bottom,shape=straight,waterlogged=false]",
        )],
    );
    let underneath = Click {
        yaw: EAST,
        ..click_on(P, Face::Bottom, Some(items::OAK_STAIRS))
    };
    assert_changes(
        &changes(&underneath, &world),
        &[(
            below(P),
            "oak_stairs[facing=east,half=top,shape=straight,waterlogged=false]",
        )],
    );
}

#[test]
fn a_slab_takes_its_type_from_the_click() {
    let world = World::new().with(P, "stone");
    let east = Face::East.neighbour(P);
    for (height, kind) in [(0.7, "top"), (0.3, "bottom")] {
        let click = Click {
            cursor: [1.0, height, 0.5],
            ..click_on(P, Face::East, Some(items::OAK_SLAB))
        };
        assert_changes(
            &changes(&click, &world),
            &[(east, &format!("oak_slab[type={kind},waterlogged=false]"))],
        );
    }
}

#[test]
fn a_door_faces_the_horizontal_direction_with_two_matching_halves() {
    for (yaw, facing) in [
        (SOUTH, "south"),
        (WEST, "west"),
        (NORTH, "north"),
        (EAST, "east"),
    ] {
        let world = World::new().with(P, "stone");
        let click = Click {
            yaw,
            ..click_on(P, Face::Top, Some(items::OAK_DOOR))
        };
        let placed = changes(&click, &world);
        let (lower, upper) = (above(P), above(above(P)));
        assert_eq!(placed.len(), 2);
        assert_eq!(property(&placed, lower, "half"), "lower");
        assert_eq!(property(&placed, upper, "half"), "upper");
        for at in [lower, upper] {
            assert_eq!(block_at(&placed, at), "minecraft:oak_door");
            assert_eq!(property(&placed, at, "facing"), facing);
            assert_eq!(property(&placed, at, "open"), "false");
            assert_eq!(property(&placed, at, "powered"), "false");
        }
        assert_eq!(
            property(&placed, lower, "hinge"),
            property(&placed, upper, "hinge")
        );
    }
}

/// The hinge of an oak door placed on the stone at `P`, by a player facing south.
fn hinge(world: World, cursor_x: f32) -> &'static str {
    let world = world.with(P, "stone");
    let click = Click {
        cursor: [cursor_x, 1.0, 0.5],
        yaw: SOUTH,
        ..click_on(P, Face::Top, Some(items::OAK_DOOR))
    };
    let placed = changes(&click, &world);
    property(&placed, above(P), "hinge")
}

#[test]
fn a_doors_hinge_turns_on_a_full_block_to_one_side() {
    // Ambiguity: the plan gives the inputs of vanilla's `getHinge` and not which side
    // each gives, so the test asks only that the side the full block is on matters.
    let t = above(P);
    let east = World::new().with(t.offset(1, 0, 0), "stone");
    let west = World::new().with(t.offset(-1, 0, 0), "stone");
    assert_ne!(hinge(east, 0.5), hinge(west, 0.5));

    let east_above = World::new().with(t.offset(1, 1, 0), "stone");
    let west_above = World::new().with(t.offset(-1, 1, 0), "stone");
    assert_ne!(hinge(east_above, 0.5), hinge(west_above, 0.5));
}

#[test]
fn a_doors_hinge_turns_on_a_door_beside_it() {
    let t = above(P);
    let door_east = World::new().with_door(t.offset(1, 0, 0), "facing=south");
    let door_west = World::new().with_door(t.offset(-1, 0, 0), "facing=south");
    assert_ne!(hinge(door_east, 0.5), hinge(door_west, 0.5));
}

#[test]
fn a_doors_hinge_turns_on_the_click_when_its_sides_are_alike() {
    assert_ne!(hinge(World::new(), 0.2), hinge(World::new(), 0.8));
    let t = above(P);
    let both = || {
        World::new()
            .with(t.offset(1, 0, 0), "stone")
            .with(t.offset(-1, 0, 0), "stone")
    };
    assert_ne!(hinge(both(), 0.2), hinge(both(), 0.8));
}

#[test]
fn a_trapdoor_on_the_side_of_a_block_faces_out_and_takes_its_half_from_the_click() {
    let world = World::new().with(P, "stone");
    let east = Face::East.neighbour(P);
    for (height, half) in [(0.7, "top"), (0.3, "bottom")] {
        let click = Click {
            cursor: [1.0, height, 0.5],
            yaw: WEST,
            ..click_on(P, Face::East, Some(items::OAK_TRAPDOOR))
        };
        assert_changes(
            &changes(&click, &world),
            &[(
                east,
                &format!(
                    "oak_trapdoor[facing=east,half={half},open=false,powered=false,waterlogged=false]"
                ),
            )],
        );
    }
}

#[test]
fn a_trapdoor_on_top_or_underneath_faces_the_player() {
    let world = World::new().with(P, "stone");
    let on_top = click_on(P, Face::Top, Some(items::OAK_TRAPDOOR));
    assert_changes(
        &changes(&on_top, &world),
        &[(
            above(P),
            "oak_trapdoor[facing=north,half=bottom,open=false,powered=false,waterlogged=false]",
        )],
    );
    let underneath = Click {
        yaw: WEST,
        ..click_on(P, Face::Bottom, Some(items::OAK_TRAPDOOR))
    };
    assert_changes(
        &changes(&underneath, &world),
        &[(
            below(P),
            "oak_trapdoor[facing=east,half=top,open=false,powered=false,waterlogged=false]",
        )],
    );
}

#[test]
fn a_trapdoor_replacing_the_clicked_block_on_its_side_faces_the_player() {
    // The first branch of the rule needs `replaceClicked` false; short grass clicked
    // on its side is replaced, so the second branch holds: half top since `F` is not
    // up, whatever the cursor.
    let world = World::new()
        .with(below(P), "grass_block")
        .with(P, "short_grass");
    let click = Click {
        cursor: [1.0, 0.3, 0.5],
        ..click_on(P, Face::East, Some(items::OAK_TRAPDOOR))
    };
    assert_changes(
        &changes(&click, &world),
        &[(
            P,
            "oak_trapdoor[facing=north,half=top,open=false,powered=false,waterlogged=false]",
        )],
    );
}

#[test]
fn a_trapdoor_in_water_is_waterlogged() {
    let world = World::new().with(below(P), "stone").with(P, "water");
    let placed = changes(&click_on(P, Face::Top, Some(items::OAK_TRAPDOOR)), &world);
    assert_eq!(property(&placed, P, "waterlogged"), "true");
}

#[test]
fn a_fence_gate_faces_the_horizontal_direction_and_sits_in_a_wall_across_its_axis() {
    let t = above(P);
    let placed = |world: World, yaw: f32| {
        let click = Click {
            yaw,
            ..click_on(P, Face::Top, Some(items::OAK_FENCE_GATE))
        };
        changes(&click, &world.with(P, "stone"))
    };
    let alone = placed(World::new(), SOUTH);
    assert_changes(
        &alone,
        &[(
            t,
            "oak_fence_gate[facing=south,in_wall=false,open=false,powered=false]",
        )],
    );

    // Facing south, its axis is z: walls east or west put it in a wall.
    for beside in [t.offset(1, 0, 0), t.offset(-1, 0, 0)] {
        let world = World::new().with(beside, "cobblestone_wall");
        assert_eq!(property(&placed(world, SOUTH), t, "in_wall"), "true");
    }
    for beside in [t.offset(0, 0, 1), t.offset(0, 0, -1)] {
        let world = World::new().with(beside, "cobblestone_wall");
        assert_eq!(property(&placed(world, SOUTH), t, "in_wall"), "false");
        let world = World::new().with(beside, "cobblestone_wall");
        assert_eq!(property(&placed(world, EAST), t, "in_wall"), "true");
    }
}

/// What an item placed on top of the stone at `P` becomes, with `around` beside it.
fn placed_among(item: i32, around: &[(BlockPos, &str)]) -> Changes {
    let mut world = World::new().with(P, "stone");
    for &(at, text) in around {
        world = world.with(at, text);
    }
    changes(&click_on(P, Face::Top, Some(item)), &world)
}

#[test]
fn a_fence_joins_fences_of_its_kind_and_sturdy_faces() {
    let t = above(P);
    let east = t.offset(1, 0, 0);
    for (neighbour, joins) in [
        ("oak_fence", true),
        ("spruce_fence", true),
        ("nether_brick_fence", false),
        ("stone", true),
        ("oak_leaves", false),
        ("pumpkin", false),
        ("carved_pumpkin", false),
        ("melon", false),
        ("barrier", false),
        ("shulker_box", false),
        ("oak_fence_gate[facing=north]", true),
        ("oak_fence_gate[facing=east]", false),
        ("air", false),
    ] {
        let placed = placed_among(items::OAK_FENCE, &[(east, neighbour)]);
        assert_eq!(
            property(&placed, t, "east"),
            if joins { "true" } else { "false" },
            "an oak fence beside {neighbour}"
        );
        assert_eq!(placed.len(), 1, "the neighbour does not change");
        for side in ["north", "south", "west"] {
            assert_eq!(property(&placed, t, side), "false");
        }
    }

    let placed = placed_among(items::NETHER_BRICK_FENCE, &[(east, "oak_fence")]);
    assert_eq!(property(&placed, t, "east"), "false");
    let placed = placed_among(items::NETHER_BRICK_FENCE, &[(east, "nether_brick_fence")]);
    assert_eq!(property(&placed, t, "east"), "true");
}

#[test]
fn a_pane_between_two_blocks_is_a_window_at_once() {
    let t = above(P);
    let placed = placed_among(
        items::GLASS_PANE,
        &[(t.offset(1, 0, 0), "stone"), (t.offset(-1, 0, 0), "stone")],
    );
    assert_changes(
        &placed,
        &[(
            t,
            "glass_pane[east=true,west=true,north=false,south=false,waterlogged=false]",
        )],
    );
}

#[test]
fn a_pane_joins_panes_bars_and_walls_and_not_leaves() {
    let t = above(P);
    let east = t.offset(1, 0, 0);
    for (neighbour, joins) in [
        ("glass_pane", true),
        ("iron_bars", true),
        ("cobblestone_wall", true),
        ("oak_leaves", false),
        ("air", false),
    ] {
        for item in [items::GLASS_PANE, items::IRON_BARS] {
            let placed = placed_among(item, &[(east, neighbour)]);
            assert_eq!(
                property(&placed, t, "east"),
                if joins { "true" } else { "false" },
                "{} beside {neighbour}",
                ITEMS[item as usize].name
            );
        }
    }
}

#[test]
fn a_wall_joins_a_wall_beside_it_by_the_block_above() {
    let t = above(P);
    let east = t.offset(1, 0, 0);
    let alone = placed_among(items::COBBLESTONE_WALL, &[]);
    assert_eq!(property(&alone, t, "up"), "true");
    for side in ["north", "south", "east", "west"] {
        assert_eq!(property(&alone, t, side), "none");
    }
    let low = placed_among(items::COBBLESTONE_WALL, &[(east, "cobblestone_wall")]);
    assert_eq!(property(&low, t, "east"), "low");
    assert_eq!(property(&low, t, "west"), "none");
    let tall = placed_among(
        items::COBBLESTONE_WALL,
        &[(east, "cobblestone_wall"), (above(t), "stone")],
    );
    assert_eq!(property(&tall, t, "east"), "tall");
    assert_eq!(tall.len(), 1, "the neighbour does not change");
}

#[test]
fn redstone_alone_is_a_cross() {
    let t = above(P);
    let placed = placed_among(items::REDSTONE, &[]);
    assert_changes(
        &placed,
        &[(
            t,
            "redstone_wire[north=side,south=side,east=side,west=side,power=0]",
        )],
    );
}

#[test]
fn a_rail_runs_along_the_horizontal_direction() {
    for item in [items::RAIL, items::POWERED_RAIL] {
        for (yaw, shape) in [
            (SOUTH, "north_south"),
            (NORTH, "north_south"),
            (WEST, "east_west"),
            (EAST, "east_west"),
        ] {
            let world = World::new().with(P, "stone");
            let click = Click {
                yaw,
                ..click_on(P, Face::Top, Some(item))
            };
            let placed = changes(&click, &world);
            assert_eq!(property(&placed, above(P), "shape"), shape);
        }
    }
}

#[test]
fn a_beds_foot_is_at_the_target_and_its_head_the_way_the_player_faces() {
    for (yaw, facing, step) in [
        (SOUTH, "south", (0, 1)),
        (NORTH, "north", (0, -1)),
        (WEST, "west", (-1, 0)),
        (EAST, "east", (1, 0)),
    ] {
        let world = World::new().with(P, "stone");
        let click = Click {
            yaw,
            ..click_on(P, Face::Top, Some(items::RED_BED))
        };
        let t = above(P);
        let head = t.offset(step.0, 0, step.1);
        assert_changes(
            &changes(&click, &world),
            &[
                (t, &format!("red_bed[facing={facing},part=foot]")),
                (head, &format!("red_bed[facing={facing},part=head]")),
            ],
        );
    }
}

#[test]
fn a_bed_is_not_placed_where_its_head_cannot_go() {
    let t = above(P);
    let world = World::new()
        .with(P, "stone")
        .with(t.offset(0, 0, 1), "stone");
    let click = Click {
        yaw: SOUTH,
        ..click_on(P, Face::Top, Some(items::RED_BED))
    };
    assert!(changes(&click, &world).is_empty());

    // Short grass where the head goes can be replaced.
    let world = World::new()
        .with(P, "stone")
        .with(t.offset(0, 0, 1), "short_grass");
    assert_eq!(changes(&click, &world).len(), 2);
}

#[test]
fn a_double_plant_takes_two_blocks() {
    for (item, block) in [
        (items::TALL_GRASS, "tall_grass"),
        (items::SUNFLOWER, "sunflower"),
    ] {
        let world = World::new().with(P, "grass_block");
        let placed = changes(&click_on(P, Face::Top, Some(item)), &world);
        assert_changes(
            &placed,
            &[
                (above(P), &format!("{block}[half=lower]")),
                (above(above(P)), &format!("{block}[half=upper]")),
            ],
        );

        let covered = World::new()
            .with(P, "grass_block")
            .with(above(above(P)), "stone");
        assert!(changes(&click_on(P, Face::Top, Some(item)), &covered).is_empty());

        let stone = World::new().with(P, "stone");
        assert!(changes(&click_on(P, Face::Top, Some(item)), &stone).is_empty());
    }
}

#[test]
fn snow_is_placed_as_one_layer_on_stone_and_not_on_ice() {
    let world = World::new().with(P, "stone");
    assert_changes(
        &changes(&click_on(P, Face::Top, Some(items::SNOW)), &world),
        &[(above(P), "snow[layers=1]")],
    );
    let ice = World::new().with(P, "ice");
    assert!(changes(&click_on(P, Face::Top, Some(items::SNOW)), &ice).is_empty());
}

#[test]
fn a_block_placed_in_a_water_source_is_waterlogged_and_in_flowing_water_not() {
    for (item, block) in [
        (items::OAK_SLAB, "oak_slab"),
        (items::OAK_STAIRS, "oak_stairs"),
        (items::OAK_FENCE, "oak_fence"),
        (items::CHEST, "chest"),
        (items::LANTERN, "lantern"),
    ] {
        // Clicked on the water itself, and placed into water beside the clicked block.
        let in_source = World::new().with(below(P), "stone").with(P, "water");
        let placed = changes(&click_on(below(P), Face::Top, Some(item)), &in_source);
        assert_eq!(property(&placed, P, "waterlogged"), "true", "{block}");
        let placed = changes(&click_on(P, Face::Top, Some(item)), &in_source);
        assert_eq!(property(&placed, P, "waterlogged"), "true", "{block}");

        let flowing = World::new()
            .with(below(P), "stone")
            .with(P, "water[level=2]");
        let placed = changes(&click_on(P, Face::Top, Some(item)), &flowing);
        assert_eq!(property(&placed, P, "waterlogged"), "false", "{block}");
    }
}

#[test]
fn a_standing_sign_turns_to_face_the_player() {
    for (yaw, rotation) in [(0.0, "8"), (90.0, "12"), (-90.0, "4"), (180.0, "0")] {
        let world = World::new().with(P, "stone");
        let click = Click {
            yaw,
            ..click_on(P, Face::Top, Some(items::OAK_SIGN))
        };
        assert_changes(
            &changes(&click, &world),
            &[(
                above(P),
                &format!("oak_sign[rotation={rotation},waterlogged=false]"),
            )],
        );
    }
}

#[test]
fn a_sign_on_the_side_of_a_block_is_a_wall_sign() {
    let world = World::new().with(P, "stone");
    let click = Click {
        yaw: WEST,
        ..click_on(P, Face::East, Some(items::OAK_SIGN))
    };
    assert_changes(
        &changes(&click, &world),
        &[(
            Face::East.neighbour(P),
            "oak_wall_sign[facing=east,waterlogged=false]",
        )],
    );
}

#[test]
fn a_hanging_sign_hangs_under_a_block_and_is_attached_under_a_narrow_one() {
    // A fence post holds it at its centre but has no full bottom face.
    let post = state("oak_fence").props();
    assert!(post.is_face_sturdy(clustine_data::Face::Down, SupportType::Center));
    assert!(!post.is_face_sturdy(clustine_data::Face::Down, SupportType::Full));
    let hold = |above_it: &str| {
        let world = World::new().with(P, above_it);
        let click = click_on(P, Face::Bottom, Some(items::OAK_HANGING_SIGN));
        changes(&click, &world)
    };
    let under_stone = hold("stone");
    assert_eq!(
        block_at(&under_stone, below(P)),
        "minecraft:oak_hanging_sign"
    );
    assert_eq!(property(&under_stone, below(P), "attached"), "false");

    let under_post = hold("oak_fence");
    assert_eq!(
        block_at(&under_post, below(P)),
        "minecraft:oak_hanging_sign"
    );
    assert_eq!(property(&under_post, below(P), "attached"), "true");

    let on_the_side = {
        let world = World::new().with(P, "stone");
        let click = Click {
            yaw: WEST,
            ..click_on(P, Face::East, Some(items::OAK_HANGING_SIGN))
        };
        changes(&click, &world)
    };
    assert_eq!(
        block_at(&on_the_side, Face::East.neighbour(P)),
        "minecraft:oak_wall_hanging_sign"
    );
}

#[test]
fn buttons_and_levers_go_on_the_floor_the_ceiling_and_walls() {
    for (item, block) in [
        (items::STONE_BUTTON, "stone_button"),
        (items::LEVER, "lever"),
    ] {
        let world = World::new().with(P, "stone");
        let on_top = Click {
            yaw: WEST,
            ..click_on(P, Face::Top, Some(item))
        };
        assert_changes(
            &changes(&on_top, &world),
            &[(above(P), &format!("{block}[face=floor,facing=west]"))],
        );
        let underneath = Click {
            yaw: NORTH,
            ..click_on(P, Face::Bottom, Some(item))
        };
        assert_changes(
            &changes(&underneath, &world),
            &[(below(P), &format!("{block}[face=ceiling,facing=north]"))],
        );
        let side = Click {
            yaw: WEST,
            ..click_on(P, Face::East, Some(item))
        };
        assert_changes(
            &changes(&side, &world),
            &[(
                Face::East.neighbour(P),
                &format!("{block}[face=wall,facing=east]"),
            )],
        );
    }
}

#[test]
fn a_ladder_goes_on_a_wall_and_not_on_a_ladder_facing_the_face() {
    let world = World::new().with(P, "stone");
    let side = Click {
        yaw: WEST,
        ..click_on(P, Face::East, Some(items::LADDER))
    };
    assert_changes(
        &changes(&side, &world),
        &[(
            Face::East.neighbour(P),
            "ladder[facing=east,waterlogged=false]",
        )],
    );

    // A ladder on a floor with nothing beside it to hang on is not placed.
    let on_top = click_on(P, Face::Top, Some(items::LADDER));
    assert!(changes(&on_top, &world).is_empty());

    // The face of a ladder facing it takes no ladder.
    let ladder = P.offset(1, 0, 0);
    let world = World::new()
        .with(P, "stone")
        .with(ladder, "ladder[facing=east]");
    let on_ladder = Click {
        yaw: WEST,
        ..click_on(ladder, Face::East, Some(items::LADDER))
    };
    assert!(changes(&on_ladder, &world).is_empty());

    // On its top, the ladder goes on above it against the same wall.
    let world = World::new()
        .with(P, "stone")
        .with(above(P), "stone")
        .with(ladder, "ladder[facing=east]");
    let on_top_of_ladder = Click {
        yaw: WEST,
        ..click_on(ladder, Face::Top, Some(items::LADDER))
    };
    assert_changes(
        &changes(&on_top_of_ladder, &world),
        &[(above(ladder), "ladder[facing=east,waterlogged=false]")],
    );
}

#[test]
fn a_lantern_stands_on_a_block_and_hangs_under_one() {
    let world = World::new().with(P, "stone");
    assert_changes(
        &changes(&click_on(P, Face::Top, Some(items::LANTERN)), &world),
        &[(above(P), "lantern[hanging=false,waterlogged=false]")],
    );
    assert_changes(
        &changes(&click_on(P, Face::Bottom, Some(items::LANTERN)), &world),
        &[(below(P), "lantern[hanging=true,waterlogged=false]")],
    );
    // On the side of a block with nothing above or below it: nowhere to go.
    assert!(changes(&click_on(P, Face::East, Some(items::LANTERN)), &world).is_empty());
}

// Section 4.5: blocks a rule reads, across regions.

/// The block west of chunk (1, 0)'s first column, and that column's block beside it.
const WEST_OF_BORDER: BlockPos = BlockPos::new(15, 64, 8);
const EAST_OF_BORDER: BlockPos = BlockPos::new(16, 64, 8);

#[test]
fn a_click_whose_target_is_in_a_chunk_not_loaded_says_where_with_the_clicked_state() {
    let world = World::new()
        .with(WEST_OF_BORDER, "stone")
        .without_chunk(ChunkPos::new(1, 0));
    let click = Click {
        yaw: EAST,
        ..click_on(WEST_OF_BORDER, Face::East, Some(items::TORCH))
    };
    assert_eq!(
        placement::click(&click, &world),
        Clicked::Place {
            target: EAST_OF_BORDER,
            replace_clicked: false,
            clicked: state("stone"),
        }
    );
}

#[test]
fn a_torch_goes_on_the_side_of_a_clicked_block_that_is_not_loaded() {
    let world = World::new().without_chunk(ChunkPos::new(1, 0));
    let click = Click {
        yaw: EAST,
        ..click_on(EAST_OF_BORDER, Face::West, Some(items::TORCH))
    };
    assert_changes(
        &placement::place(&click, WEST_OF_BORDER, false, state("stone"), &world, &[])
            .map(as_changes)
            .unwrap_or_default(),
        &[(WEST_OF_BORDER, "wall_torch[facing=west]")],
    );

    // Against what the step says was air, it has nothing to hang on.
    assert!(placement::place(&click, WEST_OF_BORDER, false, blocks::AIR, &world, &[]).is_none());
}

#[test]
fn a_loaded_clicked_block_wins_over_the_state_carried() {
    let click = Click {
        yaw: EAST,
        ..click_on(EAST_OF_BORDER, Face::West, Some(items::TORCH))
    };
    let air_there = World::new();
    assert!(
        placement::place(
            &click,
            WEST_OF_BORDER,
            false,
            state("stone"),
            &air_there,
            &[]
        )
        .is_none()
    );
    let stone_there = World::new().with(EAST_OF_BORDER, "stone");
    assert_changes(
        &placement::place(
            &click,
            WEST_OF_BORDER,
            false,
            blocks::AIR,
            &stone_there,
            &[],
        )
        .map(as_changes)
        .unwrap_or_default(),
        &[(WEST_OF_BORDER, "wall_torch[facing=west]")],
    );
}

#[test]
fn a_block_a_rule_only_looks_at_in_a_chunk_not_loaded_is_taken_as_air() {
    let world = World::new()
        .with(below(WEST_OF_BORDER), "stone")
        .without_chunk(ChunkPos::new(1, 0));
    let click = click_on(below(WEST_OF_BORDER), Face::Top, Some(items::OAK_FENCE));
    let placed = changes(&click, &world);
    assert_eq!(property(&placed, WEST_OF_BORDER, "east"), "false");
}

// Section 5: buckets and lily pads.

fn hotbar_holding(item: i32) -> [Option<ItemStack>; HOTBAR_SLOTS] {
    let mut hotbar = [None; HOTBAR_SLOTS];
    hotbar[0] = Some(ItemStack { item, count: 1 });
    hotbar
}

/// The use of `held`, the first slot of an otherwise empty hotbar, by a player at
/// `feet` looking straight down.
fn use_down(held: i32, feet: Vec3) -> ItemUse {
    ItemUse {
        held: Some(held),
        feet,
        yaw: SOUTH,
        pitch: 90.0,
        sneaking: false,
        hotbar: hotbar_holding(held),
    }
}

/// Where a player stands to look down on `P` from two blocks above it.
fn over_p() -> Vec3 {
    Vec3::new(8.5, 66.0, 8.5)
}

/// A hotbar slot that a use changes, and what it holds then.
type SlotChange = Option<(u8, Option<ItemStack>)>;

/// What a use changes: blocks, and a hotbar slot.
fn used(item_use: &ItemUse, world: &World) -> Option<(Changes, SlotChange)> {
    used_among(item_use, world, &[])
}

fn used_among(
    item_use: &ItemUse,
    world: &World,
    bodies: &[[f64; 6]],
) -> Option<(Changes, SlotChange)> {
    placement::use_item(item_use, world, bodies)
        .map(|UsedItem { changes, hotbar }| (as_changes(changes), hotbar))
}

#[track_caller]
fn assert_used(
    item_use: &ItemUse,
    world: &World,
    expected: &[(BlockPos, &str)],
    hotbar: SlotChange,
) {
    let Some((changes, slot)) = used(item_use, world) else {
        panic!("the use changed nothing");
    };
    assert_changes(&changes, expected);
    assert_eq!(slot, hotbar);
}

fn one(item: i32) -> Option<ItemStack> {
    Some(ItemStack { item, count: 1 })
}

#[test]
fn an_empty_bucket_takes_a_source_and_fills_the_first_empty_hotbar_slot() {
    for (source, filled) in [
        ("water", items::WATER_BUCKET),
        ("lava", items::LAVA_BUCKET),
        ("powder_snow", items::POWDER_SNOW_BUCKET),
    ] {
        let world = World::new().with(below(P), "stone").with(P, source);
        let item_use = use_down(items::BUCKET, over_p());
        assert_used(&item_use, &world, &[(P, "air")], Some((1, one(filled))));
    }
}

#[test]
fn an_empty_bucket_unwaterlogs_a_waterlogged_block() {
    let world = World::new()
        .with(below(P), "stone")
        .with(P, "oak_slab[type=bottom,waterlogged=true]");
    let item_use = use_down(items::BUCKET, over_p());
    assert_used(
        &item_use,
        &world,
        &[(P, "oak_slab[type=bottom,waterlogged=false]")],
        Some((1, one(items::WATER_BUCKET))),
    );
}

#[test]
fn the_filled_bucket_goes_into_the_first_empty_slot_unless_one_is_there() {
    let world = World::new().with(below(P), "stone").with(P, "water");

    let mut hotbar = hotbar_holding(items::BUCKET);
    hotbar[1] = one(items::STONE);
    hotbar[3] = one(items::LAVA_BUCKET);
    let item_use = ItemUse {
        hotbar,
        ..use_down(items::BUCKET, over_p())
    };
    assert_used(
        &item_use,
        &world,
        &[(P, "air")],
        Some((2, one(items::WATER_BUCKET))),
    );

    let mut hotbar = hotbar_holding(items::BUCKET);
    hotbar[5] = one(items::WATER_BUCKET);
    let item_use = ItemUse {
        hotbar,
        ..use_down(items::BUCKET, over_p())
    };
    assert_used(&item_use, &world, &[(P, "air")], None);
}

#[test]
fn a_full_hotbar_takes_no_filled_bucket_and_the_source_is_taken_all_the_same() {
    // Ambiguity: the plan does not say what happens with no empty slot; read as
    // vanilla's creative inventory, where the source is taken and the bucket that
    // does not fit is lost.
    let world = World::new().with(below(P), "stone").with(P, "water");
    let mut hotbar = [one(items::STONE); HOTBAR_SLOTS];
    hotbar[0] = one(items::BUCKET);
    let item_use = ItemUse {
        hotbar,
        ..use_down(items::BUCKET, over_p())
    };
    assert_used(&item_use, &world, &[(P, "air")], None);
}

#[test]
fn an_empty_bucket_looks_through_flowing_water_and_takes_nothing_from_stone() {
    let world = World::new()
        .with(below(P), "stone")
        .with(P, "water[level=1]");
    assert!(used(&use_down(items::BUCKET, over_p()), &world).is_none());
}

#[test]
fn an_empty_bucket_does_not_reach_a_source_beyond_five_blocks() {
    // The eyes are at 67.62; the water's surface six blocks down is out of reach.
    let deep = BlockPos::new(8, 60, 8);
    let world = World::new().with(deep, "water").with(below(deep), "stone");
    assert!(used(&use_down(items::BUCKET, over_p()), &world).is_none());
    // Two blocks higher it is in reach.
    let near = BlockPos::new(8, 62, 8);
    let world = World::new().with(near, "water").with(below(near), "stone");
    assert_used(
        &use_down(items::BUCKET, over_p()),
        &world,
        &[(near, "air")],
        Some((1, one(items::WATER_BUCKET))),
    );
}

#[test]
fn a_water_bucket_places_a_source_on_the_face_it_looks_at() {
    let world = World::new().with(P, "stone");
    let item_use = use_down(items::WATER_BUCKET, over_p());
    assert_used(&item_use, &world, &[(above(P), "water")], None);

    let world = World::new().with(P, "stone");
    let item_use = use_down(items::LAVA_BUCKET, over_p());
    assert_used(&item_use, &world, &[(above(P), "lava")], None);

    // Looking south at the north face of a block.
    let wall = BlockPos::new(8, 65, 10);
    let world = World::new().with(wall, "stone");
    let item_use = ItemUse {
        pitch: 0.0,
        ..use_down(items::WATER_BUCKET, Vec3::new(8.5, 64.0, 8.5))
    };
    assert_used(
        &item_use,
        &world,
        &[(Face::North.neighbour(wall), "water")],
        None,
    );
}

#[test]
fn a_water_bucket_waterlogs_a_block_that_can_hold_water_and_lava_does_not() {
    let world = World::new().with(P, "oak_slab[type=bottom]");
    assert_used(
        &use_down(items::WATER_BUCKET, over_p()),
        &world,
        &[(P, "oak_slab[type=bottom,waterlogged=true]")],
        None,
    );
    assert_used(
        &use_down(items::LAVA_BUCKET, over_p()),
        &world,
        &[(above(P), "lava")],
        None,
    );
}

#[test]
fn a_water_bucket_while_sneaking_places_beside_a_block_that_can_hold_water() {
    let world = World::new().with(P, "oak_slab[type=bottom]");
    let sneaking = ItemUse {
        sneaking: true,
        ..use_down(items::WATER_BUCKET, over_p())
    };
    assert_used(&sneaking, &world, &[(above(P), "water")], None);
}

#[test]
fn a_water_bucket_waterlogs_a_light_block_its_ray_goes_through() {
    // A light block has no outline, so the ray goes through it to the stone below;
    // it can hold water, so the water bucket waterlogs it rather than replacing it.
    //
    // Ambiguity: sneaking, the plan says the first target must be air and the
    // fallback "comes to nothing". In vanilla's `BucketItem.emptyContents` (as this
    // author remembers it), the retry passes no hit result, and without one the
    // sneaking condition no longer applies, so the retry would waterlog the light
    // block after all. The sneaking case is left to the comparisons of W6f.
    assert!(
        state("light")
            .props()
            .outline_shape()
            .boxes()
            .next()
            .is_none()
    );
    let world = World::new().with(P, "stone").with(above(P), "light");
    let standing = use_down(items::WATER_BUCKET, over_p());
    assert_used(
        &standing,
        &world,
        &[(above(P), "light[waterlogged=true]")],
        None,
    );
}

#[test]
fn water_washes_away_a_torch() {
    // Looking down beside the torch's thin outline, onto the stone it stands on.
    let world = World::new().with(P, "stone").with(above(P), "torch");
    let item_use = use_down(items::WATER_BUCKET, Vec3::new(8.2, 66.0, 8.2));
    assert_used(&item_use, &world, &[(above(P), "water")], None);
}

#[test]
fn the_eyes_are_lower_while_sneaking() {
    // Standing, the eyes are at 65.12 and see the upper block; sneaking, at 64.77,
    // the lower one.
    let upper = BlockPos::new(8, 65, 10);
    let lower = BlockPos::new(8, 64, 10);
    let world = World::new().with(upper, "stone").with(lower, "stone");
    let standing = ItemUse {
        pitch: 0.0,
        ..use_down(items::WATER_BUCKET, Vec3::new(8.5, 63.5, 8.5))
    };
    assert_used(
        &standing,
        &world,
        &[(Face::North.neighbour(upper), "water")],
        None,
    );
    let sneaking = ItemUse {
        sneaking: true,
        ..standing
    };
    assert_used(
        &sneaking,
        &world,
        &[(Face::North.neighbour(lower), "water")],
        None,
    );
}

#[test]
fn a_lily_pad_goes_on_water_and_on_ice() {
    for ground in ["water", "ice"] {
        let world = World::new().with(below(P), "stone").with(P, ground);
        assert_used(
            &use_down(items::LILY_PAD, over_p()),
            &world,
            &[(above(P), "lily_pad")],
            None,
        );
    }
}

#[test]
fn a_lily_pad_is_not_placed_on_land_on_flowing_water_or_on_a_lily_pad() {
    let stone = World::new().with(P, "stone");
    assert!(used(&use_down(items::LILY_PAD, over_p()), &stone).is_none());

    let flowing = World::new()
        .with(below(P), "stone")
        .with(P, "water[level=1]");
    assert!(used(&use_down(items::LILY_PAD, over_p()), &flowing).is_none());

    let pad = BlockPos::new(8, 64, 8);
    let world = World::new().with(below(pad), "water").with(pad, "lily_pad");
    assert!(used(&use_down(items::LILY_PAD, over_p()), &world).is_none());
}

#[test]
fn a_lily_pad_is_not_placed_into_a_player() {
    let world = World::new().with(below(P), "stone").with(P, "water");
    let in_the_way = placement::body(Vec3::new(8.5, 65.0, 8.5), false);
    assert!(used_among(&use_down(items::LILY_PAD, over_p()), &world, &[in_the_way]).is_none());
}

#[test]
fn nothing_but_buckets_and_lily_pads_is_used_without_a_block() {
    let world = World::new().with(below(P), "stone").with(P, "water");
    for held in [items::STONE, items::STICK, items::OAK_DOOR] {
        assert!(used(&use_down(held, over_p()), &world).is_none());
    }
    let empty_hand = ItemUse {
        held: None,
        hotbar: [None; HOTBAR_SLOTS],
        ..use_down(items::BUCKET, over_p())
    };
    assert!(used(&empty_hand, &world).is_none());
}

#[test]
fn the_ray_stops_at_a_block_not_loaded() {
    let far_wall = BlockPos::new(17, 65, 8);
    let looking_east = ItemUse {
        yaw: EAST,
        pitch: 0.0,
        ..use_down(items::WATER_BUCKET, Vec3::new(14.5, 63.5, 8.5))
    };
    let loaded = World::new().with(far_wall, "stone");
    assert_used(
        &looking_east,
        &loaded,
        &[(Face::West.neighbour(far_wall), "water")],
        None,
    );
    let not_loaded = World::new()
        .with(far_wall, "stone")
        .without_chunk(ChunkPos::new(1, 0));
    assert!(used(&looking_east, &not_loaded).is_none());
}

#[test]
fn the_items_of_these_tests_place_what_the_plan_says() {
    // The fixture's assumptions about the data, so that a test fails here and not
    // as a puzzle elsewhere.
    let places = |item: i32| ITEMS[item as usize].places.map(usize::from);
    // Ambiguity: the plan says the bamboo item places `bamboo_sapling`; the jar's data
    // says it places `bamboo`, which is also what `potted_bamboo` holds, so potting
    // bamboo works by the plan's rule either way.
    assert_eq!(
        places(items::BAMBOO),
        info("potted_bamboo").potted.map(usize::from)
    );
    assert_eq!(
        places(items::WHEAT_SEEDS),
        block_id_by_name("minecraft:wheat")
    );
    assert_eq!(places(items::SNOW), block_id_by_name("minecraft:snow"));
    assert!(info("oak_door").used_by_click);
    assert!(info("chest").used_by_click);
    assert!(!info("candle").used_by_click);
    assert!(!info("snow").used_by_click);
}
