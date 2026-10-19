//! Tests of container blocks (step N6b, `docs/groundwork/n6-plan.md`), from the game's
//! rules (`docs/groundwork/n6-brief.md`, sections 3.1 to 3.9, read again against the
//! class dumps): the openers' count of a chest with its block events and sounds, its
//! five-tick recheck, the double chest's halves, the barrel's `open`, the shulker box's
//! room for its lid, its lid and what it pushes, the trapped chest's signal, what a
//! comparator reads, what a container broken spills, two chests across a line, and the
//! count across a restore.
//!
//! The regions are those of the tests of R3a (`redstone.rs`): player 1 at the spawn,
//! (0.5, -60, 0.5), on a stone floor at y = -61, with the chunks around chunk (0, 0)
//! loaded so that it ticks.

use clustine_blocks::data::tick_id;
use clustine_blocks::signal::{self, Query};
use clustine_data::{Face, sounds as sound};

use super::redstone::{block_at, sounds, state, ticking_with};
use super::*;
use crate::api::MenuKind;
use crate::region::containers::{ChestHalves, Container};
use crate::region::shulker_boxes::Status;
use crate::region::updates::{RegionView, UPDATE_ALL, UPDATE_LIMIT, relative};
use crate::state::{Menu, MenuOf};

const CHEST: BlockPos = BlockPos::new(3, -60, 3);

fn chest(facing: &str, kind: &str) -> BlockState {
    state(&format!(
        "minecraft:chest[facing={facing},type={kind},waterlogged=false]"
    ))
}

fn trapped(facing: &str, kind: &str) -> BlockState {
    state(&format!(
        "minecraft:trapped_chest[facing={facing},type={kind},waterlogged=false]"
    ))
}

fn barrel(facing: &str, open: bool) -> BlockState {
    state(&format!("minecraft:barrel[facing={facing},open={open}]"))
}

fn shulker_box(facing: &str) -> BlockState {
    state(&format!("minecraft:shulker_box[facing={facing}]"))
}

/// Player `number`'s menu is now one of `kind` showing `of`.
fn show(region: &mut Region, number: u128, of: MenuOf, kind: MenuKind) {
    region
        .players
        .get_mut(&player(number))
        .expect("joined")
        .menu = Some(Menu {
        serial: 1,
        of,
        kind,
        carried: None,
        drag: None,
    });
}

/// The menu of player `number` closed without the container being told.
fn forget_menu(region: &mut Region, number: u128) {
    region
        .players
        .get_mut(&player(number))
        .expect("joined")
        .menu = None;
}

fn single(at: BlockPos) -> MenuOf {
    MenuOf::Block { at, other: None }
}

/// The block events a tick sent to clients: where, of which block, and the two numbers.
fn block_events(output: &TickOutput) -> Vec<(BlockPos, u16, i32, i32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::BlockEvent {
                position,
                block,
                kind,
                data,
            } => Some((*position, *block, *kind, *data)),
            _ => None,
        })
        .collect()
}

/// The sounds of `output` without their pitch, which each tells apart only by drawing.
fn heard(output: &TickOutput) -> Vec<(i32, f32)> {
    sounds(output)
        .into_iter()
        .map(|(sound, volume, pitch, except)| {
            assert!((0.9..1.0).contains(&pitch), "{pitch}");
            assert_eq!(except, None);
            (sound, volume)
        })
        .collect()
}

/// Where the sounds of `output` play.
fn heard_at(output: &TickOutput) -> Vec<[f64; 3]> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound { position, .. } => Some([position.x, position.y, position.z]),
            _ => None,
        })
        .collect()
}

fn stack(item: i32, count: i32) -> Option<ItemStack> {
    Some(ItemStack { item, count })
}

/// The container at `at` with `slots` set, written back.
fn fill(region: &mut Region, at: BlockPos, slots: &[(usize, Option<ItemStack>)]) {
    let mut container = region.container_at(at).expect("a container");
    for (slot, held) in slots {
        container.slots[*slot] = *held;
    }
    region.set_container(at, container, &mut TickOutput::default());
}

/// A chest opened by one player plays the chest's opening sound at its centre, counts
/// one and says so in a block event of kind 1 sent with the next tick's events; a
/// second opener changes the count and its event only; the first to go leaves one, and
/// the last plays the closing sound and says 0.
#[test]
fn a_chest_opened_and_closed_by_one_player_and_by_two_says_its_count() {
    let mut region = ticking_with(&[(CHEST, chest("north", "single"))]);
    let block = tick_id(blocks::CHEST);
    let mut output = TickOutput::default();
    region.start_open(CHEST, &mut output);
    assert_eq!(heard(&output), [(sound::BLOCK_CHEST_OPEN, 0.5)]);
    assert_eq!(heard_at(&output), [[3.5, -59.5, 3.5]]);
    assert_eq!(region.openers(CHEST), 1);
    assert_eq!(block_events(&idle(&mut region)), [(CHEST, block, 1, 1)]);

    let mut output = TickOutput::default();
    region.start_open(CHEST, &mut output);
    assert_eq!(heard(&output), []);
    assert_eq!(region.openers(CHEST), 2);
    assert_eq!(block_events(&idle(&mut region)), [(CHEST, block, 1, 2)]);

    let mut output = TickOutput::default();
    region.stop_open(CHEST, &mut output);
    assert_eq!(heard(&output), []);
    assert_eq!(block_events(&idle(&mut region)), [(CHEST, block, 1, 1)]);

    let mut output = TickOutput::default();
    region.stop_open(CHEST, &mut output);
    assert_eq!(heard(&output), [(sound::BLOCK_CHEST_CLOSE, 0.5)]);
    assert_eq!(region.openers(CHEST), 0);
    assert_eq!(block_events(&idle(&mut region)), [(CHEST, block, 1, 0)]);

    // The count is the data's, committed and kept from the edges.
    let mut output = TickOutput::default();
    region.start_open(CHEST, &mut output);
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::BlockEntityChanged { position, seen: false, .. } if *position == CHEST
    )));
}

/// Five ticks after it opened, a chest counts the players whose menus show it within
/// nine blocks of its box: one who stands there keeps it open and is said again every
/// five ticks; one who went beyond, or whose menu shows something else, is no opener,
/// and the chest closes with its sound.
#[test]
fn the_recheck_corrects_a_count_nobody_stands_behind() {
    let mut region = ticking_with(&[(CHEST, chest("north", "single"))]);
    let block = tick_id(blocks::CHEST);
    show(&mut region, 1, single(CHEST), MenuKind::Rows3);
    region.start_open(CHEST, &mut TickOutput::default());
    let mut said = Vec::new();
    for _ in 0..10 {
        said.extend(block_events(&idle(&mut region)));
    }
    assert_eq!(
        said,
        [(CHEST, block, 1, 1); 3],
        "opened, then every five ticks"
    );
    assert_eq!(region.openers(CHEST), 1);

    // Twelve blocks off, the player is out of the chest's reach.
    region
        .players
        .get_mut(&player(1))
        .expect("joined")
        .pose
        .position = Vec3::new(15.5, -60.0, 3.5);
    let outputs: Vec<TickOutput> = (0..5).map(|_| idle(&mut region)).collect();
    let closed: Vec<&TickOutput> = outputs
        .iter()
        .filter(|output| !heard(output).is_empty())
        .collect();
    assert_eq!(closed.len(), 1);
    assert_eq!(heard(closed[0]), [(sound::BLOCK_CHEST_CLOSE, 0.5)]);
    assert_eq!(region.openers(CHEST), 0);
    // The menu is closed in the player's tick as `stillValid` fails, which says the
    // count of 0; the recheck that was due says it again, as the game's
    // `recheckOpeners` says the count whether it changed or not, and plays no sound.
    let said: Vec<_> = outputs.iter().flat_map(block_events).collect();
    assert_eq!(said, [(CHEST, block, 1, 0); 2]);
    // No recheck is left.
    let later: Vec<_> = (0..10)
        .flat_map(|_| block_events(&idle(&mut region)))
        .collect();
    assert_eq!(later, []);

    // A count with nobody's menu behind it at all goes too.
    region.start_open(CHEST, &mut TickOutput::default());
    forget_menu(&mut region, 1);
    for _ in 0..6 {
        idle(&mut region);
    }
    assert_eq!(region.openers(CHEST), 0);
}

/// `ChestBlock.combine`: the right half of a double chest is the first, whichever half
/// is asked; a chest whose other half faces elsewhere is single; and a conductor above
/// either half blocks both, which glass, not a conductor, does not.
#[test]
fn a_double_chests_halves_come_in_the_games_order_and_a_conductor_above_either_blocks_it() {
    // Facing north, the left half's other half is to the east.
    let left = CHEST;
    let right = relative(CHEST, Face::East);
    let mut region = ticking_with(&[
        (left, chest("north", "left")),
        (right, chest("north", "right")),
    ]);
    let pair = ChestHalves {
        first: right,
        second: Some(left),
        blocked: false,
    };
    assert_eq!(region.chest_halves(left), Some(pair));
    assert_eq!(region.chest_halves(right), Some(pair));

    for (above, blocked) in [(left, true), (right, true), (left, false)] {
        let mut output = TickOutput::default();
        let top = relative(above, Face::Up);
        let block = if blocked {
            blocks::STONE
        } else {
            blocks::GLASS
        };
        region.set_block_updated(top, block, 2, UPDATE_LIMIT, &mut output);
        let halves = region.chest_halves(left).expect("a chest");
        assert_eq!(
            (halves.first, halves.blocked),
            (right, blocked),
            "{above:?}"
        );
        region.set_block_updated(top, blocks::AIR, 2, UPDATE_LIMIT, &mut output);
    }

    // A "left" chest beside a chest that faces another way is single.
    let lone = BlockPos::new(-5, -60, -5);
    let mut region = ticking_with(&[
        (lone, chest("north", "left")),
        (relative(lone, Face::East), chest("south", "right")),
    ]);
    let halves = region.chest_halves(lone).expect("a chest");
    assert_eq!((halves.first, halves.second), (lone, None));
    assert_eq!(region.chest_halves(relative(lone, Face::Up)), None);
    // Its halves open apart: the left half's sound is the right half's to play.
    let mut output = TickOutput::default();
    region.start_open(lone, &mut output);
    assert_eq!(heard(&output), [], "a left half is silent");
}

/// A double chest's sound plays once, from its right half, at the middle of the pair.
#[test]
fn a_double_chest_sounds_from_its_right_half_at_the_middle_of_the_pair() {
    let left = CHEST;
    let right = relative(CHEST, Face::East);
    let mut region = ticking_with(&[
        (left, chest("north", "left")),
        (right, chest("north", "right")),
    ]);
    let mut output = TickOutput::default();
    region.start_open(right, &mut output);
    region.start_open(left, &mut output);
    assert_eq!(heard(&output), [(sound::BLOCK_CHEST_OPEN, 0.5)]);
    assert_eq!(heard_at(&output), [[4.0, -59.5, 3.5]]);
    let block = tick_id(blocks::CHEST);
    assert_eq!(
        block_events(&idle(&mut region)),
        [(right, block, 1, 1), (left, block, 1, 1)]
    );
}

/// A barrel's first opener plays its sound at the middle of the face it faces and sets
/// it open with every update; its last closes it; it sends no block event.
#[test]
fn a_barrel_is_open_while_anyone_has_it_open() {
    let mut region = ticking_with(&[(CHEST, barrel("up", false))]);
    let mut output = TickOutput::default();
    region.start_open(CHEST, &mut output);
    assert_eq!(block_at(&region, CHEST), Some(barrel("up", true)));
    assert_eq!(heard(&output), [(sound::BLOCK_BARREL_OPEN, 0.5)]);
    assert_eq!(heard_at(&output), [[3.5, -59.0, 3.5]]);
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::BlockChanged { position, seen: true, .. } if *position == CHEST
    )));
    region.start_open(CHEST, &mut output);
    let mut output = TickOutput::default();
    region.stop_open(CHEST, &mut output);
    assert_eq!(block_at(&region, CHEST), Some(barrel("up", true)));
    region.stop_open(CHEST, &mut output);
    assert_eq!(block_at(&region, CHEST), Some(barrel("up", false)));
    assert_eq!(heard(&output), [(sound::BLOCK_BARREL_CLOSE, 0.5)]);
    assert_eq!(block_events(&idle(&mut region)), []);
    // Its slots stay with it as it opens.
    fill(&mut region, CHEST, &[(4, stack(items::STONE, 9))]);
    region.start_open(CHEST, &mut TickOutput::default());
    let container = region.container_at(CHEST).expect("a barrel");
    assert_eq!(container.slots[4], stack(items::STONE, 9));
}

/// A shut shulker box opens only with room for its lid: a block in the half block
/// beyond the face it faces keeps it shut; a block on another side does not.
#[test]
fn a_shulker_box_without_room_for_its_lid_does_not_open() {
    let mut region = ticking_with(&[(CHEST, shulker_box("east"))]);
    assert!(region.shulker_box_may_open(CHEST));
    let mut output = TickOutput::default();
    region.set_block_updated(
        relative(CHEST, Face::Up),
        blocks::STONE,
        2,
        UPDATE_LIMIT,
        &mut output,
    );
    assert!(
        region.shulker_box_may_open(CHEST),
        "the top is not its lid's way"
    );
    region.set_block_updated(
        relative(CHEST, Face::East),
        blocks::STONE,
        2,
        UPDATE_LIMIT,
        &mut output,
    );
    assert!(!region.shulker_box_may_open(CHEST));
    // A slab on the lower half leaves no room either; one beyond it does not reach.
    let slab = state("minecraft:stone_slab[type=bottom,waterlogged=false]");
    region.set_block_updated(
        relative(CHEST, Face::East),
        slab,
        2,
        UPDATE_LIMIT,
        &mut output,
    );
    assert!(!region.shulker_box_may_open(CHEST));
    assert!(
        !region.shulker_box_may_open(relative(CHEST, Face::West)),
        "no box"
    );
}

/// The lid starts opening at the count's block event and goes a tenth a tick, open on
/// the tenth tick, pushing an item lying on the box up out of its way by the slab it
/// swept and a hundredth more; the last opener gone, it shuts in ten ticks; and a box
/// moving its lid opens whatever stands in the way.
#[test]
fn a_shulker_boxs_lid_opens_in_ten_ticks_and_pushes_what_lies_on_it() {
    let mut region = ticking_with(&[(CHEST, shulker_box("up"))]);
    let mut output = TickOutput::default();
    assert!(region.spawn_item_as(
        [3.5, -59.0, 3.5],
        ItemStack {
            item: items::DIAMOND,
            count: 1
        },
        0,
        Some([0.0; 3]),
        &mut output,
    ));
    for _ in 0..5 {
        idle(&mut region);
    }
    let lying = |region: &Region| items_lying(region)[0].0;
    let rested = lying(&region);
    assert!((rested[1] + 59.0).abs() < 1e-6, "{rested:?}");

    let mut output = TickOutput::default();
    region.start_open(CHEST, &mut output);
    assert_eq!(heard(&output), [(sound::BLOCK_SHULKER_BOX_OPEN, 0.5)]);
    assert_eq!(
        region.shulker_lid(CHEST).expect("a box").status,
        Status::Closed
    );
    let block = tick_id(blocks::SHULKER_BOX);
    let first = idle(&mut region);
    assert_eq!(block_events(&first), [(CHEST, block, 1, 1)]);
    let lid = region.shulker_lid(CHEST).expect("a box");
    assert_eq!((lid.status, lid.progress), (Status::Opening, 0.1));
    // The swept slab, a tenth deep, and a hundredth more.
    let pushed = lying(&region);
    assert!((pushed[1] - (rested[1] + 0.11)).abs() < 1e-6, "{pushed:?}");
    for tick in 2..=10 {
        idle(&mut region);
        let lid = region.shulker_lid(CHEST).expect("a box");
        let expected = if tick < 10 {
            Status::Opening
        } else {
            Status::Opened
        };
        assert_eq!(lid.status, expected, "tick {tick}");
    }
    assert_eq!(region.shulker_lid(CHEST).expect("a box").progress, 1.0);

    // Something in the way does not keep a box from opening while its lid moves.
    let mut output = TickOutput::default();
    region.set_block_updated(
        relative(CHEST, Face::Up),
        blocks::STONE,
        2,
        UPDATE_LIMIT,
        &mut output,
    );
    assert!(region.shulker_box_may_open(CHEST));

    let mut output = TickOutput::default();
    region.stop_open(CHEST, &mut output);
    assert_eq!(heard(&output), [(sound::BLOCK_SHULKER_BOX_CLOSE, 0.5)]);
    let mut statuses = Vec::new();
    for _ in 0..10 {
        idle(&mut region);
        statuses.push(region.shulker_lid(CHEST).expect("a box").status);
    }
    assert_eq!(statuses[..9], [Status::Closing; 9]);
    assert_eq!(statuses[9], Status::Closed);
}

/// A trapped chest's signal is its count of openers: weakly to every side, so the wire
/// beside it takes it as its power, and strongly down into the block below; the wire
/// follows the count as it changes.
#[test]
fn a_trapped_chests_signal_is_its_count_of_openers() {
    let wire_at = relative(CHEST, Face::East);
    let wire = state("minecraft:redstone_wire[east=none,north=none,power=0,south=none,west=side]");
    let mut region = ticking_with(&[(CHEST, trapped("north", "single")), (wire_at, wire)]);
    let power = |region: &Region| {
        block_at(region, wire_at)
            .and_then(|state| clustine_blocks::data::number(state, "power"))
            .expect("wire")
    };
    assert_eq!(power(&region), 0);
    let mut output = TickOutput::default();
    region.start_open(CHEST, &mut output);
    assert_eq!(power(&region), 1);
    assert_eq!(
        heard(&output),
        [(sound::BLOCK_CHEST_OPEN, 0.5)],
        "a chest's"
    );
    region.start_open(CHEST, &mut output);
    assert_eq!(power(&region), 2);
    let mut view = RegionView(&mut region);
    assert_eq!(
        signal::direct_signal(&mut view, CHEST, Face::Up, Query::ALL),
        2
    );
    assert_eq!(
        signal::direct_signal(&mut view, CHEST, Face::North, Query::ALL),
        0
    );
    assert_eq!(signal::signal(&mut view, CHEST, Face::West, Query::ALL), 2);
    region.stop_open(CHEST, &mut output);
    region.stop_open(CHEST, &mut output);
    assert_eq!(power(&region), 0);
}

/// A comparator reads a chest's slots, each by its stack's most, scaled to 15 and one
/// for anything; a double chest's 54 slots as one; a blocked chest as 0. Filling the
/// chest tells the comparator behind it, which comes on.
#[test]
fn a_comparator_reads_a_chest_a_double_chest_and_a_blocked_chest() {
    let left = CHEST;
    let right = relative(CHEST, Face::East);
    let lone = BlockPos::new(-5, -60, -5);
    let reader = relative(lone, Face::South);
    let comparator = state("minecraft:comparator[facing=north,mode=compare,powered=false]");
    let mut region = ticking_with(&[
        (left, chest("north", "left")),
        (right, chest("north", "right")),
        (lone, chest("north", "single")),
        (reader, comparator),
    ]);
    let read = |region: &Region, at: BlockPos| {
        let state = block_at(region, at).expect("loaded");
        region.analog_output_at(at, state)
    };
    assert_eq!(read(&region, lone), 0);
    // One stack of stone in 27 slots: a little, which is 1.
    fill(&mut region, lone, &[(0, stack(items::STONE, 64))]);
    assert_eq!(read(&region, lone), 1);
    // Sixteen ender pearls fill a slot as 64 stone do.
    fill(&mut region, lone, &[(1, stack(items::ENDER_PEARL, 16))]);
    assert_eq!(read(&region, lone), 2);
    let full: Vec<(usize, Option<ItemStack>)> = (0..27)
        .map(|slot| (slot, stack(items::STONE, 64)))
        .collect();
    fill(&mut region, lone, &full);
    assert_eq!(read(&region, lone), 15);
    // The comparator behind it was told and is on.
    for _ in 0..4 {
        idle(&mut region);
    }
    assert_eq!(region.comparator_output(reader), 15);
    assert_eq!(
        block_at(&region, reader),
        Some(state(
            "minecraft:comparator[facing=north,mode=compare,powered=true]"
        ))
    );

    // The right half full is half the double chest, from either half.
    fill(&mut region, right, &full);
    assert_eq!(read(&region, right), 8);
    assert_eq!(read(&region, left), 8);
    let mut output = TickOutput::default();
    region.set_block_updated(
        relative(left, Face::Up),
        blocks::STONE,
        2,
        UPDATE_LIMIT,
        &mut output,
    );
    assert_eq!(read(&region, right), 0, "blocked");
    assert_eq!(read(&region, left), 0, "blocked");
}

/// A chest broken by a creative player spills what it held, in slot order, in pieces of
/// 10 to 30 within its block; a shulker box spills as a chest does.
#[test]
fn a_chest_broken_spills_its_slots_and_a_shulker_box_spills_too() {
    let boxed = BlockPos::new(-5, -60, -5);
    let mut region = ticking_with(&[
        (CHEST, chest("north", "single")),
        (boxed, shulker_box("up")),
    ]);
    fill(
        &mut region,
        CHEST,
        &[
            (0, stack(items::STONE, 64)),
            (13, stack(items::DIAMOND, 1)),
            (26, stack(items::DIRT, 40)),
        ],
    );
    fill(&mut region, boxed, &[(5, stack(items::OAK_LOG, 12))]);
    let mut output = TickOutput::default();
    region.break_by_player(CHEST, Some(player(1)), &mut output);
    region.break_by_player(boxed, Some(player(1)), &mut output);
    assert_eq!(block_at(&region, CHEST), Some(blocks::AIR));
    let lying = items_lying(&region);
    let total = |item: i32, inside: BlockPos| -> i32 {
        lying
            .iter()
            .filter(|(at, stack)| stack.item == item && crate::physics::containing(*at) == inside)
            .map(|(_, stack)| stack.count)
            .sum()
    };
    assert_eq!(total(items::STONE, CHEST), 64);
    assert_eq!(total(items::DIAMOND, CHEST), 1);
    assert_eq!(total(items::DIRT, CHEST), 40);
    assert_eq!(total(items::OAK_LOG, boxed), 12);
    for (at, _) in &lying {
        let inside = crate::physics::containing(*at);
        assert!(inside == CHEST || inside == boxed, "{at:?}");
    }
    // Stone came out in more than one piece, none above 30.
    let stone: Vec<i32> = lying
        .iter()
        .filter(|(_, stack)| stack.item == items::STONE)
        .map(|(_, stack)| stack.count)
        .collect();
    assert!(
        stone.len() >= 3 && stone.iter().all(|count| *count <= 30),
        "{stone:?}"
    );
}

/// A chest placed beside a chest in a chunk the region does not hold stays single, and
/// so does a single chest whose shape is asked again from across the line by the half
/// of a double chest there.
#[test]
fn two_chests_across_a_line_stay_single() {
    // Chunk (1, 0) is no chunk of the region's: only its copy is read.
    let mut region = ticking_with(&[]);
    let beyond = ChunkPos::new(1, 0);
    let mut copy = floor();
    let across = BlockPos::new(16, -60, 8);
    let (x, z) = across.in_chunk();
    copy.set(x, across.y, z, chest("north", "single"));
    // The region lets go of its own chunk (1, 0) to read the copy.
    region.chunks.remove(&beyond);
    region.let_go_block_entities(beyond);
    region.tick(&TickInputs {
        copy_bases: vec![(beyond, copy)],
        ..TickInputs::default()
    });
    assert_eq!(region.block(across), Some(chest("north", "single")));

    // Facing north, a chest pairs with the single chest to its east first.
    let here = relative(across, Face::West);
    region
        .players
        .get_mut(&player(1))
        .expect("joined")
        .pose
        .position = Vec3::new(15.5, -60.0, 5.5);
    region.tick(&super::redstone::holding(items::CHEST));
    region.tick(&super::redstone::click(relative(here, Face::Down)));
    assert_eq!(block_at(&region, here), Some(chest("north", "single")));

    // The half of a double chest across the line, whose other half would be here.
    let mut output = TickOutput::default();
    region.shape_changed_at(
        here,
        Face::East,
        chest("north", "right"),
        UPDATE_ALL,
        UPDATE_LIMIT,
        &mut output,
    );
    assert_eq!(block_at(&region, here), Some(chest("north", "single")));

    // Within the region the same placement pairs.
    let mut region = ticking_with(&[(BlockPos::new(4, -60, 8), chest("north", "single"))]);
    region
        .players
        .get_mut(&player(1))
        .expect("joined")
        .pose
        .position = Vec3::new(3.5, -60.0, 5.5);
    region.tick(&super::redstone::holding(items::CHEST));
    region.tick(&super::redstone::click(BlockPos::new(3, -61, 8)));
    assert_eq!(
        block_at(&region, BlockPos::new(3, -60, 8)),
        Some(chest("north", "left"))
    );
}

/// The openers' count is saved with the chest: a region restored from its state and
/// its chunks has it, and its recheck, kept with the chunk's ticks, keeps it while the
/// player's menu shows the chest and heals it once nothing does.
#[test]
fn the_openers_count_survives_a_restore_and_the_recheck_heals_it() {
    let mut region = ticking_with(&[(CHEST, chest("north", "single"))]);
    show(&mut region, 1, single(CHEST), MenuKind::Rows3);
    region.start_open(CHEST, &mut TickOutput::default());
    idle(&mut region);
    let around: Vec<ChunkPos> = (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect();
    let chunks: Vec<(ChunkPos, Chunk)> = around
        .iter()
        .map(|position| (*position, region.saved_chunk(*position).expect("loaded")))
        .collect();
    let ticketed: Vec<(ChunkPos, Ticket)> = around
        .iter()
        .map(|position| (*position, Ticket::Viewer))
        .collect();
    let mut restored = Region::restore_loaded(
        region.config.clone(),
        region.state(),
        Holdings {
            pinned: vec![ChunkArea::EVERYWHERE],
            held: around.clone(),
            lent: Vec::new(),
            lent_away: Vec::new(),
        },
        &ticketed,
        chunks,
        Vec::new(),
    );
    assert_eq!(restored.openers(CHEST), 1);
    let block = tick_id(blocks::CHEST);
    let said: Vec<_> = (0..5)
        .flat_map(|_| block_events(&idle(&mut restored)))
        .collect();
    assert_eq!(said, [(CHEST, block, 1, 1)], "the recheck found the opener");
    assert_eq!(restored.openers(CHEST), 1);

    // The menu gone without the chest being told, the next recheck closes it.
    forget_menu(&mut restored, 1);
    let outputs: Vec<TickOutput> = (0..5).map(|_| idle(&mut restored)).collect();
    let said: Vec<_> = outputs.iter().flat_map(block_events).collect();
    assert_eq!(said, [(CHEST, block, 1, 0)]);
    let closing: Vec<_> = outputs.iter().flat_map(heard).collect();
    assert_eq!(closing, [(sound::BLOCK_CHEST_CLOSE, 0.5)]);
    assert_eq!(restored.openers(CHEST), 0);
}

/// A shulker box loaded with openers that no player's menu shows is corrected as it
/// loads, and shuts; one loaded with the menu that shows it is loaded open.
#[test]
fn a_shulker_box_loaded_with_openers_nobody_has_is_corrected() {
    let mut region = ticking_with(&[(CHEST, shulker_box("up"))]);
    show(&mut region, 1, single(CHEST), MenuKind::ShulkerBox);
    region.start_open(CHEST, &mut TickOutput::default());
    for _ in 0..12 {
        idle(&mut region);
    }
    assert_eq!(
        region.shulker_lid(CHEST).expect("a box").status,
        Status::Opened
    );
    let saved = region.saved_chunk(CHEST.chunk()).expect("loaded");

    // Let go of and loaded again while the menu still shows it: open, nothing said.
    let reload = |region: &mut Region| {
        region.chunks.remove(&CHEST.chunk());
        region.let_go_block_entities(CHEST.chunk());
        region.requested.insert(CHEST.chunk());
        region.tick(&TickInputs {
            chunks_loaded: vec![(CHEST.chunk(), saved.clone())],
            ..TickInputs::default()
        })
    };
    let output = reload(&mut region);
    assert_eq!(heard(&output), []);
    assert_eq!(region.openers(CHEST), 1);
    assert_eq!(
        region.shulker_lid(CHEST).expect("a box").status,
        Status::Opened
    );

    forget_menu(&mut region, 1);
    let output = reload(&mut region);
    assert_eq!(heard(&output), [(sound::BLOCK_SHULKER_BOX_CLOSE, 0.5)]);
    assert_eq!(region.openers(CHEST), 0);
    // Loaded as the tick begins, its event runs in the tick's block events.
    let block = tick_id(blocks::SHULKER_BOX);
    assert_eq!(block_events(&output), [(CHEST, block, 1, 0)]);
    assert_eq!(
        region.shulker_lid(CHEST).expect("a box").status,
        Status::Closing
    );
}

/// An ender chest counts its openers as a chest does, with its own sounds and block
/// event, and the menu of the player's ender chest opened at it keeps it open.
#[test]
fn an_ender_chest_counts_its_openers_as_a_chest_does() {
    let ender = state("minecraft:ender_chest[facing=north,waterlogged=false]");
    let mut region = ticking_with(&[(CHEST, ender)]);
    show(&mut region, 1, MenuOf::Ender { at: CHEST }, MenuKind::Rows3);
    let mut output = TickOutput::default();
    region.start_open(CHEST, &mut output);
    assert_eq!(heard(&output), [(sound::BLOCK_ENDER_CHEST_OPEN, 0.5)]);
    assert_eq!(
        region.container_at(CHEST),
        None,
        "its slots are the player's"
    );
    let block = tick_id(blocks::ENDER_CHEST);
    let said: Vec<_> = (0..6)
        .flat_map(|_| block_events(&idle(&mut region)))
        .collect();
    assert_eq!(said, [(CHEST, block, 1, 1); 2]);
    assert_eq!(region.openers(CHEST), 1);
    let mut output = TickOutput::default();
    region.stop_open(CHEST, &mut output);
    assert_eq!(heard(&output), [(sound::BLOCK_ENDER_CHEST_CLOSE, 0.5)]);
}

/// A container's slots read back as they were written, and a dispenser's count of
/// openers stays 0: it counts nobody.
#[test]
fn a_dispenser_counts_nobody_and_keeps_its_slots() {
    let dispenser = state("minecraft:dispenser[facing=north,triggered=false]");
    let mut region = ticking_with(&[(CHEST, dispenser)]);
    fill(&mut region, CHEST, &[(8, stack(items::ARROW, 5))]);
    let mut output = TickOutput::default();
    region.start_open(CHEST, &mut output);
    assert_eq!(region.openers(CHEST), 0);
    assert_eq!(heard(&output), []);
    let container: Container = region.container_at(CHEST).expect("a dispenser");
    assert_eq!(container.slots.len(), 9);
    assert_eq!(container.slots[8], stack(items::ARROW, 5));
}
