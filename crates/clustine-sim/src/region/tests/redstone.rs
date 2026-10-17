//! Tests of redstone's signals, sources, wire and the blocks power opens or lights
//! (step R3a of `docs/adr/0026-redstone.md`), from the game's numbers: a lever's
//! power along a line of wire, a button's time pressed, a lamp's delay going out,
//! doors, trapdoors and gates moved by power, blocks placed beside power, the sounds,
//! and the order of the seven positions wire updates.
//!
//! The regions have the chunks within two of chunk (0, 0) loaded, so that the nine
//! within one tick: a block whose rules read a signal is told of its neighbours only
//! there (section 1 of the record). The player stands at the spawn, (0.5, -60, 0.5),
//! on a stone floor at y = -61.

use clustine_data::Face;
use clustine_world::Ticked;

use super::*;
use crate::region::updates::relative;

/// A region of player 1 with the 25 chunks within two of chunk (0, 0) loaded on the
/// floor of [`floor`], and `blocks` set in them.
fn ticking_with(blocks: &[(BlockPos, BlockState)]) -> Region {
    let mut region = joined_in(ChunkArea::EVERYWHERE, &[1]);
    let around: Vec<ChunkPos> = (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect();
    checked(&mut region, &tickets(around.clone(), vec![]));
    let chunks = around
        .into_iter()
        .map(|position| {
            let mut chunk = floor();
            for (at, state) in blocks.iter().filter(|(at, _)| at.chunk() == position) {
                let (x, z) = at.in_chunk();
                chunk.set(x, at.y, z, *state);
            }
            (position, chunk)
        })
        .collect();
    checked(
        &mut region,
        &TickInputs {
            chunks_loaded: chunks,
            ..TickInputs::default()
        },
    );
    assert!(region.ticks_chunk(ChunkPos::new(1, 0)));
    region
}

fn block_at(region: &Region, pos: BlockPos) -> Option<BlockState> {
    let (x, z) = pos.in_chunk();
    region
        .chunk(pos.chunk())
        .and_then(|chunk| chunk.get(x, pos.y, z))
}

fn state(text: &str) -> BlockState {
    BlockState::parse(text).expect("a state")
}

fn api_face(face: Face) -> crate::api::Face {
    match face {
        Face::Up => crate::api::Face::Top,
        Face::Down => crate::api::Face::Bottom,
        Face::North => crate::api::Face::North,
        Face::South => crate::api::Face::South,
        Face::West => crate::api::Face::West,
        Face::East => crate::api::Face::East,
    }
}

/// Player 1 clicks the top of the block at `at` with what is in hand.
fn click(at: BlockPos) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::UseItemOn {
            position: at,
            face: api_face(Face::Up),
            cursor: [0.5; 3],
            sneaking: false,
            sequence: 1,
        },
    )])
}

fn holding(item: i32) -> TickInputs {
    let stack = ItemStack { item, count: 1 };
    moves(vec![set_slot(1, 0, Some(stack)), select(1, 0)])
}

fn sounds(output: &TickOutput) -> Vec<(i32, f32, f32, Option<PlayerId>)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound,
                volume,
                pitch,
                except,
                ..
            } => Some((*sound, *volume, *pitch, *except)),
            _ => None,
        })
        .collect()
}

fn lever(powered: bool) -> BlockState {
    state(&format!(
        "minecraft:lever[face=floor,facing=north,powered={powered}]"
    ))
}

fn wire_power(region: &Region, pos: BlockPos) -> u8 {
    let wire = block_at(region, pos).expect("loaded");
    assert!(
        clustine_blocks::signal::is_wire(wire),
        "{wire:?} at {pos:?}"
    );
    clustine_blocks::signal::wire_power(wire)
}

fn lit(region: &Region, pos: BlockPos) -> bool {
    block_at(region, pos).and_then(|lamp| lamp.property("lit")) == Some("true")
}

const LEVER: BlockPos = BlockPos::new(0, -60, 2);

/// A lever at [`LEVER`], wire east of it along z = 2 for `length` blocks, and a lamp
/// after the wire.
fn line(length: i32) -> (Region, BlockPos) {
    let mut blocks = vec![(LEVER, lever(false))];
    for x in 1..=length {
        blocks.push((BlockPos::new(x, -60, 2), blocks::REDSTONE_WIRE));
    }
    let lamp = BlockPos::new(length + 1, -60, 2);
    blocks.push((lamp, blocks::REDSTONE_LAMP));
    (ticking_with(&blocks), lamp)
}

/// A lever pulled powers wire beside it to 15, and each wire along a line one less than
/// the one before; the fifteenth has 1, which lights a lamp at its end. A line one
/// longer ends at 0, and its lamp stays dark.
#[test]
fn a_lever_lights_a_lamp_through_fifteen_wires_and_not_through_sixteen() {
    let (mut region, lamp) = line(15);
    assert!(!lit(&region, lamp));
    region.tick(&click(LEVER));
    assert_eq!(block_at(&region, LEVER), Some(lever(true)));
    let powers: Vec<u8> = (1..=15)
        .map(|x| wire_power(&region, BlockPos::new(x, -60, 2)))
        .collect();
    assert_eq!(powers, (1..=15).rev().collect::<Vec<u8>>());
    assert!(lit(&region, lamp));

    let (mut region, lamp) = line(16);
    region.tick(&click(LEVER));
    assert_eq!(wire_power(&region, BlockPos::new(15, -60, 2)), 1);
    assert_eq!(wire_power(&region, BlockPos::new(16, -60, 2)), 0);
    assert!(!lit(&region, lamp));
}

/// Wire loses its power in the tick its source goes: the default evaluator works the
/// whole line out again within the update.
#[test]
fn wire_goes_dark_in_the_tick_its_lever_is_pulled_back() {
    let (mut region, lamp) = line(15);
    region.tick(&click(LEVER));
    region.tick(&click(LEVER));
    assert_eq!(block_at(&region, LEVER), Some(lever(false)));
    for x in 1..=15 {
        assert_eq!(wire_power(&region, BlockPos::new(x, -60, 2)), 0, "at x {x}");
    }
    // The lamp goes out later, by its own tick.
    assert!(lit(&region, lamp));
}

/// A lever's click is heard by everyone, the player who pulled it too, at 0.3 and a
/// pitch of 0.6 as it goes on and 0.5 as it goes off (`LeverBlock.playSound`).
#[test]
fn a_levers_click_is_heard_by_everyone_higher_as_it_goes_on() {
    let mut region = ticking_with(&[(LEVER, lever(false))]);
    let lever_click = clustine_data::sounds::BLOCK_LEVER_CLICK;
    let on = region.tick(&click(LEVER));
    assert_eq!(sounds(&on), [(lever_click, 0.3, 0.6, None)]);
    let off = region.tick(&click(LEVER));
    assert_eq!(sounds(&off), [(lever_click, 0.3, 0.5, None)]);
    let position = on.events.iter().find_map(|event| match event {
        RegionEvent::Sound { position, .. } => Some(*position),
        _ => None,
    });
    assert_eq!(position, Some(Vec3::new(0.5, -59.5, 2.5)));
}

/// A button pressed by hand stays pressed for its block set's time and lets go in its
/// own tick: 20 ticks for stone, 30 for wood. Its click as it is pressed is played by
/// the player's own client; the one as it lets go is heard by all.
#[test]
fn a_button_lets_go_twenty_ticks_on_for_stone_and_thirty_for_wood() {
    use clustine_data::sounds as s;
    for (name, ticks, on_sound, off_sound) in [
        (
            "stone_button",
            20,
            s::BLOCK_STONE_BUTTON_CLICK_ON,
            s::BLOCK_STONE_BUTTON_CLICK_OFF,
        ),
        (
            "oak_button",
            30,
            s::BLOCK_WOODEN_BUTTON_CLICK_ON,
            s::BLOCK_WOODEN_BUTTON_CLICK_OFF,
        ),
    ] {
        let button = |powered: bool| {
            state(&format!(
                "minecraft:{name}[face=floor,facing=north,powered={powered}]"
            ))
        };
        let lamp = relative(LEVER, Face::East);
        let mut region = ticking_with(&[(LEVER, button(false)), (lamp, blocks::REDSTONE_LAMP)]);
        let pressed = region.tick(&click(LEVER));
        assert_eq!(block_at(&region, LEVER), Some(button(true)), "{name}");
        assert!(lit(&region, lamp), "{name}");
        assert_eq!(sounds(&pressed), [(on_sound, 1.0, 1.0, Some(player(1)))]);
        // A click on a pressed button does nothing more.
        assert_eq!(sounds(&region.tick(&click(LEVER))), []);
        for tick in 2..ticks {
            idle(&mut region);
            assert_eq!(
                block_at(&region, LEVER),
                Some(button(true)),
                "{name} {tick}"
            );
        }
        let released = idle(&mut region);
        assert_eq!(block_at(&region, LEVER), Some(button(false)), "{name}");
        assert_eq!(sounds(&released), [(off_sound, 1.0, 1.0, None)]);
    }
}

/// A lamp is lit in the tick power comes, and goes out four ticks after it goes
/// (`RedstoneLampBlock`'s scheduled tick).
#[test]
fn a_lamp_goes_out_four_ticks_after_its_power() {
    let lamp = relative(LEVER, Face::East);
    let mut region = ticking_with(&[(LEVER, lever(false)), (lamp, blocks::REDSTONE_LAMP)]);
    region.tick(&click(LEVER));
    assert!(lit(&region, lamp));
    region.tick(&click(LEVER));
    let block = clustine_blocks::data::tick_id(blocks::REDSTONE_LAMP);
    assert!(region.has_scheduled_tick(lamp, Ticked::Block(block)));
    for _ in 1..4 {
        idle(&mut region);
        assert!(lit(&region, lamp));
    }
    idle(&mut region);
    assert!(!lit(&region, lamp));
}

/// A door beside a lever opens with it, both halves, and shuts again, heard by
/// everyone each time; the click of the lever is heard as well.
#[test]
fn a_door_opens_and_shuts_with_the_lever_beside_it() {
    let lower = relative(LEVER, Face::East);
    let upper = relative(lower, Face::Up);
    let door = |half: &str, open: bool| {
        state(&format!(
            "minecraft:oak_door[half={half},facing=north,open={open},powered={open}]"
        ))
    };
    let mut region = ticking_with(&[
        (LEVER, lever(false)),
        (lower, door("lower", false)),
        (upper, door("upper", false)),
    ]);
    let opened = region.tick(&click(LEVER));
    assert_eq!(block_at(&region, lower), Some(door("lower", true)));
    assert_eq!(block_at(&region, upper), Some(door("upper", true)));
    let heard: Vec<(i32, Option<PlayerId>)> = sounds(&opened)
        .into_iter()
        .map(|(sound, _, _, except)| (sound, except))
        .collect();
    use clustine_data::sounds as s;
    assert_eq!(
        heard,
        [
            (s::BLOCK_WOODEN_DOOR_OPEN, None),
            (s::BLOCK_LEVER_CLICK, None)
        ]
    );
    let shut = region.tick(&click(LEVER));
    assert_eq!(block_at(&region, lower), Some(door("lower", false)));
    assert_eq!(block_at(&region, upper), Some(door("upper", false)));
    assert_eq!(sounds(&shut)[0].0, s::BLOCK_WOODEN_DOOR_CLOSE);
}

/// A trapdoor and a fence gate beside a lever open with it and shut again.
#[test]
fn a_trapdoor_and_a_gate_open_and_shut_with_the_lever_beside_them() {
    use clustine_data::sounds as s;
    for (name, open_sound, close_sound) in [
        (
            "oak_trapdoor[facing=north,half=bottom",
            s::BLOCK_WOODEN_TRAPDOOR_OPEN,
            s::BLOCK_WOODEN_TRAPDOOR_CLOSE,
        ),
        (
            "oak_fence_gate[facing=north",
            s::BLOCK_FENCE_GATE_OPEN,
            s::BLOCK_FENCE_GATE_CLOSE,
        ),
    ] {
        let block = relative(LEVER, Face::East);
        let moved = |open: bool| state(&format!("minecraft:{name},open={open},powered={open}]"));
        let mut region = ticking_with(&[(LEVER, lever(false)), (block, moved(false))]);
        let opened = region.tick(&click(LEVER));
        assert_eq!(block_at(&region, block), Some(moved(true)), "{name}");
        assert_eq!(sounds(&opened)[0].0, open_sound, "{name}");
        assert_eq!(sounds(&opened)[0].3, None, "{name}");
        let shut = region.tick(&click(LEVER));
        assert_eq!(block_at(&region, block), Some(moved(false)), "{name}");
        assert_eq!(sounds(&shut)[0].0, close_sound, "{name}");
    }
}

/// A lamp placed beside a redstone block comes out lit, and a door beside one comes out
/// open and powered, both halves (`getStateForPlacement`).
#[test]
fn a_lamp_and_a_door_placed_beside_power_come_out_lit_and_open() {
    let target = BlockPos::new(2, -60, 2);
    let power = relative(target, Face::East);
    let floor_below = relative(target, Face::Down);

    let mut region = ticking_with(&[(power, blocks::REDSTONE_BLOCK)]);
    region.tick(&holding(items::REDSTONE_LAMP));
    region.tick(&click(floor_below));
    assert!(lit(&region, target), "{:?}", block_at(&region, target));

    let mut region = ticking_with(&[(power, blocks::REDSTONE_BLOCK)]);
    region.tick(&holding(items::OAK_DOOR));
    region.tick(&click(floor_below));
    for (pos, half) in [(target, "lower"), (relative(target, Face::Up), "upper")] {
        let door = block_at(&region, pos).expect("loaded");
        assert_eq!(door.property("half"), Some(half));
        assert_eq!(door.property("open"), Some("true"), "{door:?}");
        assert_eq!(door.property("powered"), Some("true"), "{door:?}");
    }

    // Without power, as before.
    let mut region = ticking_with(&[]);
    region.tick(&holding(items::REDSTONE_LAMP));
    region.tick(&click(floor_below));
    assert_eq!(block_at(&region, target), Some(blocks::REDSTONE_LAMP));
}

/// A placement whose state is worked out from a signal is refused where its chunk
/// does not tick, as a click whose use reads one is (the record, section 1): a lamp
/// is not placed in a region whose one chunk does not tick, and stone is.
#[test]
fn a_placement_that_reads_a_signal_is_refused_where_the_chunk_does_not_tick() {
    let target = BlockPos::new(2, -60, 2);
    let floor_below = relative(target, Face::Down);
    let mut region = on_floor_with(&[]);
    assert!(!region.ticks_chunk(ChunkPos::new(0, 0)));
    region.tick(&holding(items::REDSTONE_LAMP));
    region.tick(&click(floor_below));
    assert_eq!(block_at(&region, target), Some(blocks::AIR));
    region.tick(&holding(items::STONE));
    region.tick(&click(floor_below));
    assert_eq!(block_at(&region, target), Some(blocks::STONE));
}

/// Wire whose power changes updates the neighbours of seven positions, itself and its
/// six neighbours, in the order of Java's `HashSet` of them (the record, section 2).
/// Water two blocks from the wire on five sides is told by one of the seven each, and
/// schedules its tick as it is told, so the order of the water's ticks is the order
/// of the seven: for wire at (8, -59, 8), up, north, west, the wire, down, south and
/// east, where the order they are put in would be up, north, south, west and east.
#[test]
fn wire_updates_the_neighbours_of_its_seven_positions_in_the_order_of_javas_hash_set() {
    let wire = BlockPos::new(8, -59, 8);
    let support = relative(wire, Face::Down);
    // On the support's west side, hanging on it.
    let lever_at = relative(support, Face::West);
    let wall_lever = |powered: bool| {
        state(&format!(
            "minecraft:lever[face=wall,facing=west,powered={powered}]"
        ))
    };
    let two = |face: Face| relative(relative(wire, face), face);
    let sides = [Face::Up, Face::North, Face::West, Face::South, Face::East];
    let mut blocks = vec![
        (support, blocks::STONE),
        (wire, blocks::REDSTONE_WIRE),
        (lever_at, wall_lever(false)),
    ];
    blocks.extend(sides.iter().map(|face| (two(*face), blocks::WATER)));
    let mut region = ticking_with(&blocks);
    let water_ticks = |region: &Region| -> Vec<BlockPos> {
        region
            .scheduled_ticks(wire.chunk())
            .into_iter()
            .filter(|tick| matches!(tick.of, Ticked::Fluid(_)))
            .map(|tick| {
                let chunk = wire.chunk();
                BlockPos::new(
                    chunk.x * 16 + i32::from(tick.at.x),
                    i32::from(tick.at.y),
                    chunk.z * 16 + i32::from(tick.at.z),
                )
            })
            .collect()
    };
    assert_eq!(water_ticks(&region), []);
    region.tick(&moves(vec![walk(1, 5.5, 8.5)]));
    let output = region.tick(&moves(vec![numbered(
        1,
        PlayerInput::UseItemOn {
            position: lever_at,
            face: api_face(Face::West),
            cursor: [0.5; 3],
            sneaking: false,
            sequence: 1,
        },
    )]));
    assert_eq!(
        block_at(&region, lever_at),
        Some(wall_lever(true)),
        "{output:?}"
    );
    assert_eq!(wire_power(&region, wire), 15);
    assert_eq!(water_ticks(&region), sides.map(two));
}

/// A click on a lone dot of wire makes it a cross and a click on the cross a dot
/// again, keeping its power (`RedstoneWireBlock.useWithoutItem`).
#[test]
fn a_click_on_lone_wire_turns_a_dot_into_a_cross_and_back() {
    let at = BlockPos::new(2, -60, 2);
    let mut region = ticking_with(&[(at, blocks::REDSTONE_WIRE)]);
    region.tick(&holding(items::STICK));
    region.tick(&click(at));
    let cross = block_at(&region, at).expect("loaded");
    assert!(clustine_blocks::connect::wire_is_cross(cross), "{cross:?}");
    region.tick(&click(at));
    assert_eq!(block_at(&region, at), Some(blocks::REDSTONE_WIRE));
}

/// A redstone block placed beside wire powers it as any block placed tells its
/// neighbours, and the wire goes dark when the block is broken.
#[test]
fn a_redstone_block_placed_beside_wire_powers_it_and_broken_leaves_it_dark() {
    let target = BlockPos::new(2, -60, 2);
    let wire = relative(target, Face::East);
    let mut region = ticking_with(&[(wire, blocks::REDSTONE_WIRE)]);
    region.tick(&holding(items::REDSTONE_BLOCK));
    region.tick(&click(relative(target, Face::Down)));
    assert_eq!(block_at(&region, target), Some(blocks::REDSTONE_BLOCK));
    assert_eq!(wire_power(&region, wire), 15);
    region.tick(&moves(vec![dig(1, target.x, target.y, target.z, 2)]));
    assert_eq!(block_at(&region, target), Some(blocks::AIR));
    assert_eq!(wire_power(&region, wire), 0);
}

/// A powered lever broken tells its neighbours as it goes
/// (`LeverBlock.affectNeighborsAfterRemoval`), and the door it held open shuts.
#[test]
fn a_powered_lever_broken_lets_the_door_beside_it_shut() {
    let lower = relative(LEVER, Face::East);
    let upper = relative(lower, Face::Up);
    let door = |half: &str, open: bool| {
        state(&format!(
            "minecraft:oak_door[half={half},facing=north,open={open},powered={open}]"
        ))
    };
    let mut region = ticking_with(&[
        (LEVER, lever(true)),
        (lower, door("lower", true)),
        (upper, door("upper", true)),
    ]);
    region.tick(&moves(vec![dig(1, LEVER.x, LEVER.y, LEVER.z, 1)]));
    assert_eq!(block_at(&region, LEVER), Some(blocks::AIR));
    assert_eq!(block_at(&region, lower), Some(door("lower", false)));
    assert_eq!(block_at(&region, upper), Some(door("upper", false)));
}

/// Wire whose block below is broken goes, and the wire it powered goes dark.
#[test]
fn wire_whose_ground_is_broken_goes_and_the_line_beyond_goes_dark() {
    let (mut region, _) = line(3);
    region.tick(&click(LEVER));
    assert_eq!(wire_power(&region, BlockPos::new(3, -60, 2)), 13);
    region.tick(&moves(vec![dig(1, 2, -61, 2, 2)]));
    assert_eq!(
        block_at(&region, BlockPos::new(2, -60, 2)),
        Some(blocks::AIR)
    );
    assert_eq!(wire_power(&region, BlockPos::new(3, -60, 2)), 0);
    assert_eq!(wire_power(&region, BlockPos::new(1, -60, 2)), 15);
}
