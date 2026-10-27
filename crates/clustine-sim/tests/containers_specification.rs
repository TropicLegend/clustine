//! Tests of the container blocks of step N6 and the redstone that waits for them,
//! written from `docs/groundwork/n6-plan.md` (section 1; decision 10; section 6 items 4
//! and 5; section 10), the porting brief `docs/groundwork/n6-brief.md` (sections 3.0 to
//! 3.9, 4 and 5, and 11.2) and the public API alone, by someone who did not write the
//! code: lids, their block events and sounds and the openers' count with its recheck;
//! a trapped chest's signal; spilling when broken; a comparator reading a chest; two
//! chests across a pinned line staying single; and a few cases each of dispensers,
//! droppers and hoppers.
//!
//! Player 1 stands on a stone floor at y = -61 (feet at y = -60) in chunk (0, 0), whose
//! 5x5 is loaded. Every player of Clustine is a creative one.

mod n6_scene;

use clustine_data::{BlockState, blocks, items, sounds};
use clustine_sim::TickOutput;
use clustine_sim::api::{ClickKind, Face, Inventory, MenuKind};
use clustine_world::{BlockPos, ChunkPos, EntityData, Vec3};
use n6_scene::*;

const BOX: BlockPos = BlockPos::new(4, -60, 4);
const BESIDE: Vec3 = Vec3::new(2.5, -60.0, 2.5);

/// The menu slot of hotbar slot `n` in a menu of `kind`.
fn hotbar_slot(kind: MenuKind, n: usize) -> i16 {
    i16::try_from(kind.container_slots() + 27 + n).expect("fits")
}

/// 64 dirt in every hotbar slot but 1, which holds 64 redstone blocks.
fn holding() -> Inventory {
    let mut slots: Vec<(usize, clustine_sim::api::ItemStack)> =
        (0..9).map(|slot| (slot, stack(items::DIRT, 64))).collect();
    slots[1] = (1, stack(items::REDSTONE_BLOCK, 64));
    inventory(&slots)
}

fn chest_state() -> BlockState {
    state("minecraft:chest[facing=north,type=single]")
}

/// Opens the container at `at`, shift-clicks the dirt of hotbar slot 0 into it, and
/// closes it again.
fn fill(scene: &mut Scene, at: BlockPos, kind: MenuKind) {
    let (serial, output) = scene.open(at);
    assert_eq!(the_opened(&output, player(1)).kind, kind);
    let output = scene.menu_click(serial, hotbar_slot(kind, 0), 0, ClickKind::QuickMove);
    // The click's answer; a hopper's own move in the same tick is reported beside it.
    assert!(
        changed(&output, player(1))
            .iter()
            .any(|change| change.slots.contains(&(0, Some(stack(items::DIRT, 64))))),
        "the dirt went into the container"
    );
    scene.close(serial);
}

/// Player 1 places a redstone block on the floor at `at` with hotbar slot 1.
fn power(scene: &mut Scene, at: BlockPos) -> TickOutput {
    scene.select(1);
    let output = scene.click(at.offset(0, -1, 0), Face::Top);
    assert_eq!(scene.block(at), blocks::REDSTONE_BLOCK, "placed at {at:?}");
    output
}

fn power_at(scene: &Scene, at: BlockPos) -> u8 {
    scene
        .block(at)
        .property("power")
        .unwrap_or_else(|| panic!("{at:?} has a power"))
        .parse()
        .expect("a number")
}

/// The tick and output of the first of `outputs` for which `found` holds.
fn first(outputs: &[TickOutput], found: impl Fn(&TickOutput) -> bool) -> Option<usize> {
    outputs.iter().position(found)
}

// ------------------------------------------------------------------------------------
// Lids, sounds and openers (brief 3.0 items 6 to 8, 3.3 to 3.5, 9.2).

/// Brief 3.3, 3.4 and 9.2: the first opener plays `block.chest.open` at the chest's
/// centre at volume 0.5, and the chest's block event 1 says one opener.
#[test]
fn a_chest_opened_plays_its_sound_at_its_centre_and_its_block_event_counts_one_opener() {
    let mut scene = Scene::alone(&World::of(&[(BOX, chest_state())]), holding(), BESIDE);
    let (_, opening) = scene.open(BOX);
    let next = scene.idle();
    let played = sounds(&opening, sounds::BLOCK_CHEST_OPEN);
    assert_eq!(played.len(), 1, "{:?}", opening.events);
    assert_eq!(played[0], (centre(BOX), 0.5));
    let events: Vec<_> = [&opening, &next]
        .iter()
        .flat_map(|output| block_events_at(output, BOX))
        .collect();
    assert_eq!(
        events.first(),
        Some(&(block_id(blocks::CHEST), 1, 1)),
        "{events:?}"
    );
}

/// Brief 3.4: while the chest is open its scheduled recheck every 5 ticks counts its
/// openers again and sends the block event each time.
#[test]
fn an_open_chest_sends_its_block_event_again_every_five_ticks() {
    let mut scene = Scene::alone(&World::of(&[(BOX, chest_state())]), holding(), BESIDE);
    let (_, opening) = scene.open(BOX);
    let mut outputs = vec![opening];
    outputs.extend(scene.idle_for(21));
    let ticks: Vec<usize> = outputs
        .iter()
        .enumerate()
        .filter(|(_, output)| {
            block_events_at(output, BOX).contains(&(block_id(blocks::CHEST), 1, 1))
        })
        .map(|(tick, _)| tick)
        .collect();
    assert!(ticks.len() >= 4, "{ticks:?}");
    for pair in ticks[1..].windows(2) {
        assert_eq!(pair[1] - pair[0], 5, "rechecks every 5 ticks: {ticks:?}");
    }
    // The open is a click, handled at the game time of the tick before its own: the
    // recheck scheduled 5 ticks from it is due in the fourth tick after.
    assert_eq!(ticks[1] - ticks[0], 4, "the first recheck: {ticks:?}");
}

/// Brief 3.4: two players at one chest make two openers; the first to close leaves one
/// and plays no sound, the last plays `block.chest.close` and the count falls to none.
#[test]
fn the_last_of_two_openers_to_close_plays_the_close_sound() {
    let mut scene = Scene::alone(&World::of(&[(BOX, chest_state())]), holding(), BESIDE);
    let second = scene.join(2, Vec3::new(5.5, -60.0, 2.5));
    let (mine, _) = scene.open(BOX);
    let (theirs, opening) = scene.open_by(second, BOX);
    assert!(
        sounds(&opening, sounds::BLOCK_CHEST_OPEN).is_empty(),
        "one open sound only"
    );
    let next = scene.idle();
    let chest = block_id(blocks::CHEST);
    assert!(
        [&opening, &next]
            .iter()
            .any(|output| block_events_at(output, BOX).contains(&(chest, 1, 2))),
        "two openers"
    );
    let closing = scene.close(mine);
    let next = scene.idle();
    assert!(sounds(&closing, sounds::BLOCK_CHEST_CLOSE).is_empty());
    assert!(sounds(&next, sounds::BLOCK_CHEST_CLOSE).is_empty());
    assert!(
        [&closing, &next]
            .iter()
            .any(|output| block_events_at(output, BOX).contains(&(chest, 1, 1))),
        "one opener left"
    );
    let closing = scene.close_by(second, theirs);
    let next = scene.idle();
    let played: Vec<_> = [&closing, &next]
        .iter()
        .flat_map(|output| sounds(output, sounds::BLOCK_CHEST_CLOSE))
        .collect();
    assert_eq!(played, vec![(centre(BOX), 0.5)]);
    assert!(
        [&closing, &next]
            .iter()
            .any(|output| block_events_at(output, BOX).contains(&(chest, 1, 0))),
        "no openers"
    );
}

/// Brief 3.0 item 4 and 3.3: a double chest's halves each send their block event, and
/// one sound plays, the right half's, at the pair's centre.
#[test]
fn a_double_chest_sends_a_block_event_for_each_half_and_one_sound_at_its_centre() {
    let west = BOX;
    let east = BOX.offset(1, 0, 0);
    let world = World::of(&[
        (west, state("minecraft:chest[facing=north,type=left]")),
        (east, state("minecraft:chest[facing=north,type=right]")),
    ]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    let (_, opening) = scene.open(west);
    let next = scene.idle();
    let played = sounds(&opening, sounds::BLOCK_CHEST_OPEN);
    assert_eq!(played.len(), 1, "{:?}", opening.events);
    assert_eq!(
        played[0].0,
        Vec3::new(5.0, -59.5, 4.5),
        "between the halves"
    );
    let chest = block_id(blocks::CHEST);
    for half in [west, east] {
        assert!(
            [&opening, &next]
                .iter()
                .any(|output| block_events_at(output, half).contains(&(chest, 1, 1))),
            "{half:?} sends its count"
        );
    }
}

/// Brief 3.4: a barrel shows its lid by its `open` property, set on the first open
/// and the last close, with its sounds at the centre of its facing face; it sends no
/// block event.
#[test]
fn a_barrel_opens_by_its_open_property_with_its_sound_at_its_facing_face() {
    let world = World::of(&[(BOX, state("minecraft:barrel[facing=up,open=false]"))]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    let (serial, opening) = scene.open(BOX);
    assert_eq!(
        scene.block(BOX),
        state("minecraft:barrel[facing=up,open=true]")
    );
    let played = sounds(&opening, sounds::BLOCK_BARREL_OPEN);
    assert_eq!(played, vec![(Vec3::new(4.5, -59.0, 4.5), 0.5)]);
    let mut outputs = vec![opening];
    outputs.extend(scene.idle_for(6));
    assert!(
        outputs
            .iter()
            .all(|output| block_events_at(output, BOX).is_empty()),
        "no block events"
    );
    let closing = scene.close(serial);
    assert_eq!(
        scene.block(BOX),
        state("minecraft:barrel[facing=up,open=false]")
    );
    assert_eq!(
        sounds(&closing, sounds::BLOCK_BARREL_CLOSE),
        vec![(Vec3::new(4.5, -59.0, 4.5), 0.5)]
    );
}

/// Brief 3.5: a shulker box's block event 1 carries its openers on every open and
/// close, with its sounds at its centre.
#[test]
fn a_shulker_box_sends_its_openers_on_open_and_close_with_its_sounds_at_its_centre() {
    let world = World::of(&[(BOX, state("minecraft:shulker_box[facing=up]"))]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    let shulker = block_id(blocks::SHULKER_BOX);
    let (serial, opening) = scene.open(BOX);
    let next = scene.idle();
    assert_eq!(
        sounds(&opening, sounds::BLOCK_SHULKER_BOX_OPEN),
        vec![(centre(BOX), 0.5)]
    );
    assert!(
        [&opening, &next]
            .iter()
            .any(|output| block_events_at(output, BOX).contains(&(shulker, 1, 1)))
    );
    let closing = scene.close(serial);
    let next = scene.idle();
    assert_eq!(
        sounds(&closing, sounds::BLOCK_SHULKER_BOX_CLOSE),
        vec![(centre(BOX), 0.5)]
    );
    assert!(
        [&closing, &next]
            .iter()
            .any(|output| block_events_at(output, BOX).contains(&(shulker, 1, 0)))
    );
}

/// Brief 3.2: dispensers, droppers and hoppers count nobody and play nothing.
#[test]
fn dispensers_droppers_and_hoppers_open_without_a_sound_or_a_block_event() {
    for name in [
        "minecraft:dispenser[facing=north,triggered=false]",
        "minecraft:dropper[facing=north,triggered=false]",
        "minecraft:hopper[facing=down,enabled=true]",
    ] {
        let mut scene = Scene::alone(&World::of(&[(BOX, state(name))]), holding(), BESIDE);
        let (_, opening) = scene.open(BOX);
        assert_eq!(opened(&opening, player(1)).len(), 1, "{name} opens");
        let mut outputs = vec![opening];
        outputs.extend(scene.idle_for(6));
        for output in &outputs {
            assert!(
                !output.events.iter().any(|event| matches!(
                    event,
                    clustine_sim::api::RegionEvent::Sound { .. }
                        | clustine_sim::api::RegionEvent::BlockEvent { .. }
                )),
                "{name}: {:?}",
                output.events
            );
        }
    }
}

/// Brief 3.3: a trapped chest gives a signal of its openers' count to the wire beside it.
#[test]
fn a_trapped_chest_powers_the_wire_beside_it_while_it_is_open() {
    let wire = BOX.offset(1, 0, 0);
    let world = World::of(&[
        (
            BOX,
            state("minecraft:trapped_chest[facing=north,type=single]"),
        ),
        (wire, blocks::REDSTONE_WIRE),
    ]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    assert_eq!(power_at(&scene, wire), 0);
    let (serial, _) = scene.open(BOX);
    let mut seen = Vec::new();
    assert!(
        scene.idle_until(4, &mut seen, |scene, _| power_at(scene, wire) == 1),
        "one opener powers the wire to 1"
    );
    scene.close(serial);
    assert!(
        scene.idle_until(4, &mut seen, |scene, _| power_at(scene, wire) == 0),
        "the closed chest gives no signal"
    );
}

// ------------------------------------------------------------------------------------
// Spilling (brief 3.0 item 10; plan section 6 item 5).

/// Brief 3.0 item 10 and 3.6: breaking a container spills its contents, a creative
/// player's break too, in stacks of 10 to 30 at a time; its own drop is not made.
#[test]
fn a_broken_container_spills_what_it_holds_even_for_a_creative_player() {
    let cases = [
        (chest_state(), MenuKind::Rows3),
        (
            state("minecraft:barrel[facing=up,open=false]"),
            MenuKind::Rows3,
        ),
        (
            state("minecraft:dispenser[facing=north,triggered=false]"),
            MenuKind::Grid3x3,
        ),
        (
            state("minecraft:dropper[facing=north,triggered=false]"),
            MenuKind::Grid3x3,
        ),
        (
            state("minecraft:hopper[facing=down,enabled=true]"),
            MenuKind::Hopper,
        ),
        // Section 6 item 5: a shulker box keeps its contents until stacks have
        // components, so one broken spills them as a chest does.
        (
            state("minecraft:shulker_box[facing=up]"),
            MenuKind::ShulkerBox,
        ),
    ];
    for (block, kind) in cases {
        let mut scene = Scene::alone(&World::of(&[(BOX, block)]), holding(), BESIDE);
        fill(&mut scene, BOX, kind);
        scene.dig(BOX);
        assert_eq!(scene.block(BOX), blocks::AIR, "{block:?} broken");
        let lying = items_lying(&scene.all());
        assert_eq!(
            lying_count(&scene.all(), items::DIRT),
            64,
            "{block:?}: {lying:?}"
        );
        assert!(
            lying
                .iter()
                .all(|(stack, _)| stack.item == items::DIRT && stack.count <= 30),
            "{block:?}: only the dirt, at most 30 to a stack: {lying:?}"
        );
        for (_, at) in &lying {
            assert!(
                (at[0] - 4.5).abs() < 1.0 && (at[2] - 4.5).abs() < 1.0,
                "{block:?}: spilled where it stood: {at:?}"
            );
        }
    }
}

// ------------------------------------------------------------------------------------
// The comparator (brief 3.8).

/// Brief 3.8: a comparator behind a chest reads `floor(f * 14) + 1` where `f` is the
/// chest's fullness: nine full stacks of 27 slots give 5.
#[test]
fn a_comparator_reads_how_full_a_chest_is() {
    let comparator = BOX.offset(0, 0, 1);
    let wire = BOX.offset(0, 0, 2);
    let world = World::of(&[
        (BOX, chest_state()),
        // Facing north, it reads the block to its north and powers the one south of it.
        (
            comparator,
            state("minecraft:comparator[facing=north,mode=compare,powered=false]"),
        ),
        (wire, blocks::REDSTONE_WIRE),
    ]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    let (serial, _) = scene.open(BOX);
    scene.menu_click(
        serial,
        hotbar_slot(MenuKind::Rows3, 0),
        0,
        ClickKind::QuickMove,
    );
    let mut seen = Vec::new();
    assert!(
        scene.idle_until(6, &mut seen, |scene, _| power_at(scene, wire) == 1),
        "one stack of 64 in 27 slots reads 1, not {}",
        power_at(&scene, wire)
    );
    for slot in 2..9 {
        scene.menu_click(
            serial,
            hotbar_slot(MenuKind::Rows3, slot),
            0,
            ClickKind::QuickMove,
        );
    }
    scene.menu_click(
        serial,
        hotbar_slot(MenuKind::Rows3, 0),
        0,
        ClickKind::Pickup,
    );
    // Eight stacks in: 8/27 reads floor(4.15) + 1 = 5.
    assert!(
        scene.idle_until(6, &mut seen, |scene, _| power_at(scene, wire) == 5),
        "eight full stacks read 5, not {}",
        power_at(&scene, wire)
    );
}

// ------------------------------------------------------------------------------------
// Two chests across a pinned line (section 10 item 2).

const BY_THE_LINE: Vec3 = Vec3::new(14.5, -60.0, 2.5);
const ACROSS: BlockPos = BlockPos::new(16, -60, 4);
const THIS_SIDE: BlockPos = BlockPos::new(15, -60, 4);

/// The control: a chest placed beside a chest of the same facing in one region joins
/// it. Looking south, the chest is placed facing north; the western of two halves facing
/// north is the left one.
#[test]
fn a_chest_placed_beside_a_chest_on_the_same_side_joins_it() {
    let world = World::of(&[(BOX.offset(1, 0, 0), chest_state())]);
    let mut scene = Scene::alone(&world, inventory(&[(0, stack(items::CHEST, 64))]), BESIDE);
    scene.click(BOX.offset(0, -1, 0), Face::Top);
    assert_eq!(
        scene.block(BOX),
        state("minecraft:chest[facing=north,type=left]")
    );
    assert_eq!(
        scene.block(BOX.offset(1, 0, 0)),
        state("minecraft:chest[facing=north,type=right]")
    );
}

/// Section 10 item 2: a chest does not join a chest in a chunk its region does not
/// hold, at its placement or at the shape update that crosses as an effect: both stay
/// single.
#[test]
fn two_chests_side_by_side_across_a_pinned_line_stay_single() {
    let world = World::of(&[(ACROSS, chest_state())]);
    let chests = inventory(&[(0, stack(items::CHEST, 64))]);
    let mut west = Scene::new(Layout::side(true), &world, chests.clone(), BY_THE_LINE);
    let mut east = region_of(&Layout::side(false), &world, chests);
    let placing = west.click(THIS_SIDE.offset(0, -1, 0), Face::Top);
    assert_eq!(west.block(THIS_SIDE), chest_state(), "placed single");
    let crossing: Vec<_> = placing
        .effects
        .iter()
        .filter(|effect| effect.at.chunk() == ChunkPos::new(1, 0))
        .cloned()
        .collect();
    filed(&mut east, 40, crossing);
    assert_eq!(
        block_at(&east, ACROSS),
        chest_state(),
        "the far one stays single"
    );
}

/// The lends a scene's region asks for in `outputs`.
fn lends_asked(outputs: &[TickOutput]) -> Vec<ChunkPos> {
    let mut asked: Vec<ChunkPos> = outputs
        .iter()
        .flat_map(|output| output.lends.iter().copied())
        .collect();
    asked.sort();
    asked.dedup();
    asked
}

/// Player 1 by the line, looking with `yaw`, places the item of hotbar slot 0 on the
/// floor at [`THIS_SIDE`]; the outputs of that tick and two more.
fn place_by_the_line(item: i32, yaw: f32) -> (Scene, Vec<TickOutput>) {
    let mut scene = Scene::new(
        Layout::side(true),
        &World::default(),
        inventory(&[(0, stack(item, 64))]),
        BY_THE_LINE,
    );
    scene.input(clustine_sim::api::PlayerInput::Move {
        position: None,
        rotation: Some((yaw, 0.0)),
        on_ground: true,
    });
    let mut outputs = vec![scene.click(THIS_SIDE.offset(0, -1, 0), Face::Top)];
    outputs.extend(scene.idle_for(2));
    (scene, outputs)
}

/// Section 6 item 4 and section 7 item 14: a dispenser is a circuit's part whose reach
/// is the block in front of it: one placed by the line facing across it asks for the
/// chunk beyond, as a piston, a wire or a repeater placed there does (section 10 item 7
/// gives this to N6e, "beside their ticking").
#[test]
fn a_dispenser_placed_facing_across_a_pinned_line_asks_for_the_chunk_it_faces() {
    // Looking west, the dispenser is placed facing east, at the player.
    let (scene, outputs) = place_by_the_line(items::DISPENSER, 90.0);
    assert_eq!(
        scene.block(THIS_SIDE),
        state("minecraft:dispenser[facing=east,triggered=false]")
    );
    assert_eq!(lends_asked(&outputs), vec![ChunkPos::new(1, 0)]);
}

/// Section 7 item 14: the new parts have their own reach, not the two blocks every part
/// has by default: a dispenser by the line facing away from it asks for nothing.
#[test]
fn a_dispenser_placed_by_a_pinned_line_facing_away_from_it_asks_for_no_lend() {
    let (scene, outputs) = place_by_the_line(items::DISPENSER, -90.0);
    assert_eq!(
        scene.block(THIS_SIDE),
        state("minecraft:dispenser[facing=west,triggered=false]")
    );
    assert!(
        lends_asked(&outputs).is_empty(),
        "{:?}",
        lends_asked(&outputs)
    );
}

/// Section 7 item 14: a hopper's reach is the block it faces and the one above it it
/// pulls from: one facing down by the line asks for nothing.
#[test]
fn a_hopper_placed_by_a_pinned_line_facing_down_asks_for_no_lend() {
    let (scene, outputs) = place_by_the_line(items::HOPPER, 0.0);
    assert_eq!(
        scene.block(THIS_SIDE),
        state("minecraft:hopper[facing=down,enabled=true]")
    );
    assert!(
        lends_asked(&outputs).is_empty(),
        "{:?}",
        lends_asked(&outputs)
    );
}

/// Section 10 item 1: containers are not circuit's parts: a chest placed by the line
/// asks for no land.
#[test]
fn a_chest_placed_by_a_pinned_line_asks_for_no_lend() {
    let (scene, outputs) = place_by_the_line(items::CHEST, 0.0);
    assert_eq!(scene.block(THIS_SIDE), chest_state());
    assert!(
        lends_asked(&outputs).is_empty(),
        "{:?}",
        lends_asked(&outputs)
    );
}

// ------------------------------------------------------------------------------------
// Dispensers and droppers (brief 4.0 to 4.3, 4.18).

const FACING_EAST: &str = "[facing=east,triggered=false]";

/// Brief 4.0 item 1, 4.2 and 4.3: a dispenser fires 4 ticks after a rising edge,
/// dropping one item from the front at its centre plus 0.7 along its facing, lowered by
/// 0.15625, with level events 1000 and 2000 (the facing's 3D value, east 5); while the
/// power stays, it does not fire again.
#[test]
fn a_dispenser_drops_one_item_four_ticks_after_a_rising_edge_and_only_once() {
    let world = World::of(&[(BOX, state(&format!("minecraft:dispenser{FACING_EAST}")))]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    fill(&mut scene, BOX, MenuKind::Grid3x3);
    let mut outputs = vec![power(&mut scene, BOX.offset(0, 0, -1))];
    outputs.extend(scene.idle_for(30));
    let fired = first(&outputs, |output| {
        level_events(output).contains(&(1000, BOX, 0))
    })
    .expect("the dispenser fired");
    // A click is handled between the game's ticks, at the game time of the tick before:
    // `scheduleTick(pos, block, 4)` from there is due in the third tick after the one
    // whose inputs placed the block.
    assert_eq!(fired, 3, "4 game ticks after the update that powered it");
    assert!(level_events(&outputs[fired]).contains(&(2000, BOX, 5)));
    let firings = outputs
        .iter()
        .filter(|output| level_events(output).contains(&(1000, BOX, 0)))
        .count();
    assert_eq!(firings, 1, "only on the rising edge");
    let lying = items_lying(&scene.all());
    assert_eq!(lying.len(), 1, "{lying:?}");
    assert_eq!(lying[0].0, stack(items::DIRT, 1));
    let (serial, output) = scene.open(BOX);
    assert_eq!(
        the_opened(&output, player(1)).slots[0],
        Some(stack(items::DIRT, 63))
    );
    scene.close(serial);
}

/// Brief 4.3: the dropped item starts at the dispense position, lowered.
#[test]
fn a_dispensed_item_starts_in_front_of_the_dispenser() {
    let world = World::of(&[(BOX, state(&format!("minecraft:dispenser{FACING_EAST}")))]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    fill(&mut scene, BOX, MenuKind::Grid3x3);
    power(&mut scene, BOX.offset(0, 0, -1));
    let mut seen = Vec::new();
    assert!(scene.idle_until(10, &mut seen, |scene, _| {
        !items_lying(&scene.all()).is_empty()
    }));
    let (_, at) = items_lying(&scene.all())[0];
    // Within a tick's move of (5.2, -59.65625, 4.5).
    assert!((at[0] - 5.2).abs() < 0.4, "{at:?}");
    assert!((at[1] + 59.656_25).abs() < 0.4, "{at:?}");
    assert!((at[2] - 4.5).abs() < 0.4, "{at:?}");
}

/// Brief 4.0 item 3: an empty dispenser fails with level event 1001.
#[test]
fn an_empty_dispenser_clicks_when_powered() {
    let world = World::of(&[(BOX, state(&format!("minecraft:dispenser{FACING_EAST}")))]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    let mut outputs = vec![power(&mut scene, BOX.offset(0, 0, -1))];
    outputs.extend(scene.idle_for(8));
    assert!(
        outputs
            .iter()
            .any(|output| level_events(output).contains(&(1001, BOX, 0))),
        "the failure click"
    );
    assert!(items_lying(&scene.all()).is_empty());
}

/// Brief 4.0 item 1: there is no `onPlace`: a dispenser placed beside a block that powers
/// it waits for a neighbour's update, and a block set beside it later fires it.
#[test]
fn a_dispenser_beside_a_power_it_was_placed_by_fires_only_at_a_neighbours_update() {
    let world = World::of(&[
        (BOX, state(&format!("minecraft:dispenser{FACING_EAST}"))),
        (BOX.offset(0, 0, -1), blocks::REDSTONE_BLOCK),
    ]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    fill(&mut scene, BOX, MenuKind::Grid3x3);
    let outputs = scene.idle_for(10);
    assert!(
        outputs
            .iter()
            .all(|output| !level_events(output).contains(&(1000, BOX, 0))),
        "nothing fires it"
    );
    let mut outputs = vec![power(&mut scene, BOX.offset(0, 0, 1))];
    outputs.extend(scene.idle_for(8));
    // Powered already, it is `triggered` only by an update: the one the block placed
    // beside it makes.
    assert_eq!(
        outputs
            .iter()
            .filter(|output| level_events(output).contains(&(1000, BOX, 0)))
            .count(),
        1
    );
}

/// Brief 4.10 and 4.0 item 8: a dispensed TNT is primed in the block in front, at its
/// bottom centre, with no owner.
#[test]
fn a_dispenser_of_tnt_primes_it_in_the_block_in_front() {
    let world = World::of(&[(BOX, state(&format!("minecraft:dispenser{FACING_EAST}")))]);
    let mut slots: Vec<(usize, clustine_sim::api::ItemStack)> = vec![
        (0, stack(items::TNT, 64)),
        (1, stack(items::REDSTONE_BLOCK, 64)),
    ];
    slots.truncate(2);
    let mut scene = Scene::alone(&world, inventory(&slots), BESIDE);
    let (serial, _) = scene.open(BOX);
    scene.menu_click(
        serial,
        hotbar_slot(MenuKind::Grid3x3, 0),
        0,
        ClickKind::QuickMove,
    );
    scene.close(serial);
    power(&mut scene, BOX.offset(0, 0, -1));
    let mut seen = Vec::new();
    assert!(scene.idle_until(10, &mut seen, |scene, _| {
        !scene.of_kind(clustine_data::entity_types::TNT).is_empty()
    }));
    let tnt = scene.of_kind(clustine_data::entity_types::TNT);
    assert_eq!(tnt.len(), 1);
    match &tnt[0].data {
        EntityData::PrimedTnt { owner, power, .. } => {
            assert_eq!(*owner, None);
            assert_eq!(power.0, 4.0);
        }
        other => panic!("not primed TNT: {other:?}"),
    }
    let at = tnt[0].position;
    assert!(
        (at[0] - 5.5).abs() < 0.1 && (at[2] - 4.5).abs() < 0.1,
        "{at:?}"
    );
    assert!(
        items_lying(&scene.all()).is_empty(),
        "no TNT item is dropped"
    );
}

/// Brief 4.0 item 9 and 4.18: a dropper facing a chest puts one item into it, silently,
/// and drops nothing.
#[test]
fn a_dropper_facing_a_chest_puts_one_item_into_it_silently() {
    let chest = BOX.offset(1, 0, 0);
    let world = World::of(&[
        (BOX, state(&format!("minecraft:dropper{FACING_EAST}"))),
        (chest, chest_state()),
    ]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    fill(&mut scene, BOX, MenuKind::Grid3x3);
    let mut outputs = vec![power(&mut scene, BOX.offset(0, 0, -1))];
    outputs.extend(scene.idle_for(10));
    for output in &outputs {
        let events = level_events(output);
        assert!(
            !events
                .iter()
                .any(|(kind, at, _)| *at == BOX && (*kind == 1000 || *kind == 2000)),
            "silent: {events:?}"
        );
    }
    assert!(items_lying(&scene.all()).is_empty(), "nothing dropped");
    let (serial, output) = scene.open(chest);
    assert_eq!(
        the_opened(&output, player(1)).slots[0],
        Some(stack(items::DIRT, 1))
    );
    scene.close(serial);
    let (_, output) = scene.open(BOX);
    assert_eq!(
        the_opened(&output, player(1)).slots[0],
        Some(stack(items::DIRT, 63))
    );
}

/// Brief 4.18: a dropper with no container in front drops the item as a dispenser does.
#[test]
fn a_dropper_with_nothing_in_front_drops_the_item() {
    let world = World::of(&[(BOX, state(&format!("minecraft:dropper{FACING_EAST}")))]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    fill(&mut scene, BOX, MenuKind::Grid3x3);
    let mut outputs = vec![power(&mut scene, BOX.offset(0, 0, -1))];
    outputs.extend(scene.idle_for(10));
    assert!(
        outputs
            .iter()
            .any(|output| level_events(output).contains(&(1000, BOX, 0)))
    );
    assert_eq!(lying_count(&scene.all(), items::DIRT), 1);
}

// ------------------------------------------------------------------------------------
// Hoppers (brief 5.0 to 5.8).

/// The ticks, counted from the first output, at which the open menu's slot 0 of
/// `who` changed, with its counts.
fn slot_zero_changes(outputs: &[TickOutput]) -> Vec<(usize, i32)> {
    outputs
        .iter()
        .enumerate()
        .flat_map(|(tick, output)| {
            changed(output, player(1))
                .into_iter()
                .flat_map(|change| change.slots)
                .filter(|(slot, _)| *slot == 0)
                .map(move |(_, stack)| (tick, stack.map_or(0, |stack| stack.count)))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Brief 5.0 item 2 and 5.3 to 5.7: a hopper pushes one item into the chest it faces
/// every 8 ticks, which the chest's open menu shows as it happens.
#[test]
fn a_hopper_moves_one_item_into_the_chest_it_faces_every_eight_ticks() {
    let chest = BOX.offset(1, 0, 0);
    let world = World::of(&[
        (BOX, state("minecraft:hopper[facing=east,enabled=true]")),
        (chest, chest_state()),
    ]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    fill(&mut scene, BOX, MenuKind::Hopper);
    let (_, opening) = scene.open(chest);
    let start = the_opened(&opening, player(1)).slots[0]
        .as_ref()
        .map_or(0, |stack| stack.count);
    let outputs = scene.idle_for(50);
    let changes = slot_zero_changes(&outputs);
    assert!(changes.len() >= 5, "{changes:?}");
    let mut count = start;
    for (_, now) in &changes {
        assert_eq!(*now, count + 1, "one at a time: {changes:?}");
        count = *now;
    }
    for pair in changes.windows(2) {
        assert_eq!(pair[1].0 - pair[0].0, 8, "every 8 ticks: {changes:?}");
    }
}

/// Brief 5.5: a hopper pulls from the container above it, one item a move.
#[test]
fn a_hopper_pulls_from_the_chest_above_it() {
    let above = BOX.offset(0, 1, 0);
    let world = World::of(&[
        (BOX, state("minecraft:hopper[facing=down,enabled=true]")),
        (above, chest_state()),
    ]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    fill(&mut scene, above, MenuKind::Rows3);
    let (_, opening) = scene.open(BOX);
    let start = the_opened(&opening, player(1)).slots[0]
        .as_ref()
        .map_or(0, |stack| stack.count);
    let outputs = scene.idle_for(40);
    let changes = slot_zero_changes(&outputs);
    assert!(changes.len() >= 4, "{changes:?}");
    assert!(changes.last().expect("some").1 > start);
    for pair in changes.windows(2) {
        assert_eq!(pair[1].1, pair[0].1 + 1);
        assert_eq!(pair[1].0 - pair[0].0, 8, "every 8 ticks: {changes:?}");
    }
}

/// Brief 5.5 and 5.8: a hopper takes an item lying in it, whatever its pick-up delay.
#[test]
fn a_hopper_takes_an_item_lying_in_its_bowl() {
    let item = clustine_world::Entity {
        key: key(1),
        kind: u16::try_from(clustine_data::entity_types::ITEM).expect("fits"),
        uuid: uuid::Uuid::from_u128(7_001),
        position: [4.5, -59.375, 4.5],
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 1,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: EntityData::Item {
            stack: stack(items::COBBLESTONE, 3),
            pick_up_delay: 32_767,
            thrower: None,
            health: 5,
            age_offset: 0,
        },
    };
    let world = World::of(&[(BOX, state("minecraft:hopper[facing=down,enabled=true]"))])
        .with_entities(&[item]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    let mut seen = Vec::new();
    assert!(
        scene.idle_until(20, &mut seen, |scene, _| items_lying(&scene.all())
            .is_empty()),
        "the item was taken"
    );
    let (_, output) = scene.open(BOX);
    assert_eq!(
        the_opened(&output, player(1)).slots[0],
        Some(stack(items::COBBLESTONE, 3))
    );
}

/// Brief 5.1: a hopper powered by redstone is not enabled and moves nothing.
#[test]
fn a_hopper_powered_by_redstone_moves_nothing() {
    let chest = BOX.offset(1, 0, 0);
    let world = World::of(&[
        (BOX, state("minecraft:hopper[facing=east,enabled=true]")),
        (chest, chest_state()),
    ]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    power(&mut scene, BOX.offset(0, 0, -1));
    assert_eq!(
        scene.block(BOX),
        state("minecraft:hopper[facing=east,enabled=false]")
    );
    fill(&mut scene, BOX, MenuKind::Hopper);
    let (_, opening) = scene.open(chest);
    assert!(the_opened(&opening, player(1)).slots[0].is_none());
    let outputs = scene.idle_for(30);
    assert!(slot_zero_changes(&outputs).is_empty(), "locked");
}

/// Brief 3.0 item 5, 3.8 and 5.6: a hopper ignores what blocks a chest and feeds one
/// under a stone block, while a comparator reads that chest as 0.
#[test]
fn a_hopper_feeds_a_chest_under_a_block_which_a_comparator_reads_as_empty() {
    let hopper = BOX.offset(-1, 0, 0);
    let comparator = BOX.offset(0, 0, 1);
    let wire = BOX.offset(0, 0, 2);
    let world = World::of(&[
        (BOX, chest_state()),
        (BOX.offset(0, 1, 0), blocks::STONE),
        (hopper, state("minecraft:hopper[facing=east,enabled=true]")),
        (
            comparator,
            state("minecraft:comparator[facing=north,mode=compare,powered=false]"),
        ),
        (wire, blocks::REDSTONE_WIRE),
    ]);
    let mut scene = Scene::alone(&world, holding(), Vec3::new(1.5, -60.0, 2.5));
    fill(&mut scene, hopper, MenuKind::Hopper);
    for _ in 0..40 {
        scene.idle();
        assert_eq!(power_at(&scene, wire), 0, "a blocked chest reads 0");
    }
    let (_, output) = scene.open(hopper);
    let left = the_opened(&output, player(1)).slots[0]
        .as_ref()
        .map_or(0, |stack| stack.count);
    assert!(
        left < 60,
        "the hopper moved its dirt into the chest: {left} left"
    );
}

/// Brief 3.1 and 3.4: an ender chest counts its openers as a chest does, with its
/// block event; its sounds are its own. (The ender chest's block entity was not read,
/// brief 3.1; its sound is taken to be `block.ender_chest.open` at its centre.)
#[test]
fn an_ender_chest_sends_its_openers_as_a_chest_does() {
    let world = World::of(&[(BOX, state("minecraft:ender_chest[facing=north]"))]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    let (serial, opening) = scene.open(BOX);
    let next = scene.idle();
    let ender = block_id(blocks::ENDER_CHEST);
    assert!(
        [&opening, &next]
            .iter()
            .any(|output| block_events_at(output, BOX).contains(&(ender, 1, 1))),
        "{:?}",
        opening.events
    );
    assert_eq!(sounds(&opening, sounds::BLOCK_ENDER_CHEST_OPEN).len(), 1);
    let closing = scene.close(serial);
    let next = scene.idle();
    assert!(
        [&closing, &next]
            .iter()
            .any(|output| block_events_at(output, BOX).contains(&(ender, 1, 0)))
    );
}

/// Brief 3.8: a double chest's comparator reads its 54 slots together: one stack in
/// either half reads `floor(1/54 * 14) + 1` = 1, and twenty stacks `floor(20/54 * 14)
/// + 1` = 6. The comparator stands behind the left half and the stacks go into the
/// right half's slots (0 to 26): brief 3.6 has a `CompoundContainer`'s `setChanged`
/// call both halves', and each half's `setChanged` tells the comparators beside it
/// (`updateNeighbourForOutputSignal`), so this one re-reads.
#[test]
fn a_comparator_reads_a_double_chest_as_fifty_four_slots() {
    let west = BOX;
    let east = BOX.offset(1, 0, 0);
    let comparator = BOX.offset(0, 0, 1);
    let wire = BOX.offset(0, 0, 2);
    let world = World::of(&[
        (west, state("minecraft:chest[facing=north,type=left]")),
        (east, state("minecraft:chest[facing=north,type=right]")),
        (
            comparator,
            state("minecraft:comparator[facing=north,mode=compare,powered=false]"),
        ),
        (wire, blocks::REDSTONE_WIRE),
    ]);
    let mut slots: Vec<(usize, clustine_sim::api::ItemStack)> =
        (0..36).map(|slot| (slot, stack(items::DIRT, 64))).collect();
    slots[1] = (1, stack(items::REDSTONE_BLOCK, 64));
    let mut scene = Scene::alone(&world, inventory(&slots), BESIDE);
    let (serial, _) = scene.open(west);
    // Hotbar slot 0 is menu slot 54 + 27 = 81.
    scene.menu_click(serial, 81, 0, ClickKind::QuickMove);
    let mut seen = Vec::new();
    assert!(
        scene.idle_until(6, &mut seen, |scene, _| power_at(scene, wire) == 1),
        "one stack reads 1, not {}",
        power_at(&scene, wire)
    );
    // Nineteen more from the main inventory, menu slots 54 to 72.
    for slot in 54..73 {
        scene.menu_click(serial, slot, 0, ClickKind::QuickMove);
    }
    assert!(
        scene.idle_until(6, &mut seen, |scene, _| power_at(scene, wire) == 6),
        "twenty stacks read 6, not {}",
        power_at(&scene, wire)
    );
}

/// Brief 3.8 and 5.7: a hopper's push into a double chest tells the comparators of
/// both halves (`target.setChanged()` on the `CompoundContainer`, brief 3.6): one behind
/// the left half reads the dirt the hopper puts into the right half's first slot.
#[test]
fn a_hopper_filling_a_double_chests_right_half_is_read_behind_the_left_half() {
    let west = BOX;
    let east = BOX.offset(1, 0, 0);
    let hopper = BOX.offset(2, 0, 0);
    let comparator = BOX.offset(0, 0, 1);
    let wire = BOX.offset(0, 0, 2);
    let world = World::of(&[
        (west, state("minecraft:chest[facing=north,type=left]")),
        (east, state("minecraft:chest[facing=north,type=right]")),
        (hopper, state("minecraft:hopper[facing=west,enabled=true]")),
        (
            comparator,
            state("minecraft:comparator[facing=north,mode=compare,powered=false]"),
        ),
        (wire, blocks::REDSTONE_WIRE),
    ]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    fill(&mut scene, hopper, MenuKind::Hopper);
    let mut seen = Vec::new();
    assert!(
        scene.idle_until(20, &mut seen, |scene, _| power_at(scene, wire) == 1),
        "the double chest's comparator reads 1, not {}",
        power_at(&scene, wire)
    );
}

/// The same the other way round: a comparator behind the right half reads what goes
/// into the left half's slots through the menu (brief 3.6, `CompoundContainer
/// .setChanged`).
#[test]
fn a_comparator_behind_a_double_chests_right_half_reads_the_left_halfs_slots() {
    let west = BOX;
    let east = BOX.offset(1, 0, 0);
    let comparator = east.offset(0, 0, 1);
    let wire = east.offset(0, 0, 2);
    let world = World::of(&[
        (west, state("minecraft:chest[facing=north,type=left]")),
        (east, state("minecraft:chest[facing=north,type=right]")),
        (
            comparator,
            state("minecraft:comparator[facing=north,mode=compare,powered=false]"),
        ),
        (wire, blocks::REDSTONE_WIRE),
    ]);
    let mut scene = Scene::alone(&world, holding(), BESIDE);
    let (serial, _) = scene.open(west);
    scene.menu_click(serial, 81, 0, ClickKind::Pickup);
    scene.menu_click(serial, 30, 0, ClickKind::Pickup);
    let mut seen = Vec::new();
    assert!(
        scene.idle_until(6, &mut seen, |scene, _| power_at(scene, wire) == 1),
        "reads 1, not {}",
        power_at(&scene, wire)
    );
}

/// N9 plan, section 4.9: a dispenser of arrows shoots one as an arrow, and drops none.
#[test]
fn a_dispenser_of_arrows_shoots_one_as_an_arrow() {
    let world = World::of(&[(BOX, state(&format!("minecraft:dispenser{FACING_EAST}")))]);
    let mut scene = Scene::alone(
        &world,
        inventory(&[
            (0, stack(items::ARROW, 64)),
            (1, stack(items::REDSTONE_BLOCK, 64)),
        ]),
        BESIDE,
    );
    let (serial, _) = scene.open(BOX);
    scene.menu_click(
        serial,
        hotbar_slot(MenuKind::Grid3x3, 0),
        0,
        ClickKind::QuickMove,
    );
    scene.close(serial);
    power(&mut scene, BOX.offset(0, 0, -1));
    let mut seen = Vec::new();
    assert!(scene.idle_until(10, &mut seen, |scene, _| {
        !scene.of_kind(clustine_data::entity_types::ARROW).is_empty()
    }));
    assert_eq!(scene.of_kind(clustine_data::entity_types::ARROW).len(), 1);
    assert!(items_lying(&scene.all()).is_empty(), "no item dropped");
}

/// Brief 4.18 and 5.7: a dropper facing a full chest keeps the item and makes no sound.
#[test]
fn a_dropper_facing_a_full_chest_keeps_its_item() {
    let chest = BOX.offset(1, 0, 0);
    let world = World::of(&[
        (BOX, state(&format!("minecraft:dropper{FACING_EAST}"))),
        (chest, chest_state()),
    ]);
    let mut slots: Vec<(usize, clustine_sim::api::ItemStack)> = (0..36)
        .map(|slot| (slot, stack(items::STONE, 64)))
        .collect();
    slots[0] = (0, stack(items::DIRT, 64));
    slots[1] = (1, stack(items::REDSTONE_BLOCK, 64));
    let mut scene = Scene::alone(&world, inventory(&slots), BESIDE);
    // Fill the chest's 27 slots with the stone of the main inventory, menu slots 27 to 53.
    let (serial, _) = scene.open(chest);
    for slot in 27..54 {
        scene.menu_click(serial, slot, 0, ClickKind::QuickMove);
    }
    scene.close(serial);
    fill(&mut scene, BOX, MenuKind::Grid3x3);
    let mut outputs = vec![power(&mut scene, BOX.offset(0, 0, -1))];
    outputs.extend(scene.idle_for(10));
    assert!(items_lying(&scene.all()).is_empty(), "nothing dropped");
    for output in &outputs {
        assert!(
            level_events(output).iter().all(|(_, at, _)| *at != BOX),
            "silent"
        );
    }
    let (_, output) = scene.open(BOX);
    assert_eq!(
        the_opened(&output, player(1)).slots[0],
        Some(stack(items::DIRT, 64))
    );
}
