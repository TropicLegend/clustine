//! Tests of game events and sculk sensors (step N6f, `docs/groundwork/n6-plan.md`), from
//! the game's rules as the brief's section 8 and the class dumps have them: a vibration
//! taken up a tick after it was posted and received `floor(distance)` ticks after it,
//! its power by its distance in blocks, the closest of a tick's candidates and then the
//! higher frequency, wool between, sneaking, `stepOn`, the comparator, the phases, and
//! the line between regions.
//!
//! The regions are those of the tests of R3a (`redstone.rs`): player 1 at the spawn,
//! (0.5, -60, 0.5), on a stone floor at y = -61, with the chunks around chunk (0, 0)
//! loaded so that they tick.

use super::redstone::{block_at, holding, state, ticking_with};
use super::*;
use crate::region::vibrations::{Context, Source, Vibration};

/// The sensor of most tests, three blocks south of the line the player walks along.
const SENSOR: BlockPos = BlockPos::new(8, -60, 0);

fn sensor(phase: &str, power: u8) -> BlockState {
    state(&format!(
        "minecraft:sculk_sensor[power={power},sculk_sensor_phase={phase},waterlogged=false]"
    ))
}

fn inactive() -> BlockState {
    sensor("inactive", 0)
}

fn phase(region: &Region, at: BlockPos) -> (String, u8) {
    let state = block_at(region, at).expect("a block");
    (
        state
            .property("sculk_sensor_phase")
            .expect("a sensor")
            .to_owned(),
        state
            .property("power")
            .and_then(|power| power.parse().ok())
            .expect("a sensor"),
    )
}

/// The vibration particles the tick sent.
fn particles(output: &TickOutput) -> Vec<([f64; 3], BlockPos, i32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Vibration { from, to, ticks } => Some((*from, *to, *ticks)),
            _ => None,
        })
        .collect()
}

/// Ticks `region` with nothing until the sensor at `at` is active, at most `most`
/// times; the ticks it took, or `None`.
fn ticks_until_active(region: &mut Region, at: BlockPos, most: u32) -> Option<u32> {
    for tick in 1..=most {
        checked(region, &TickInputs::default());
        if phase(region, at).0 == "active" {
            return Some(tick);
        }
    }
    None
}

fn idle_for(region: &mut Region, ticks: u32) -> Vec<TickOutput> {
    (0..ticks)
        .map(|_| checked(region, &TickInputs::default()))
        .collect()
}

/// Player 1 places what they hold on top of the floor at `at`.
fn place_on_floor(at: BlockPos) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::UseItemOn {
            position: at.offset(0, -1, 0),
            face: Face::Top,
            cursor: [0.5; 3],
            sneaking: false,
            sequence: 1,
        },
    )])
}

/// A vibration placed as a block would be, at the middle of `at`, of nobody.
fn post(region: &mut Region, event: Vibration, at: BlockPos) {
    region.vibrate_at(event, at, Context::default());
}

/// A block placed five blocks from a sensor is taken up in the tick its packet is
/// applied in, which counts from the tick before (the packet is handled between the
/// two), shows its particle on its way for five ticks, and is received five ticks after
/// that time: the sensor becomes active with the power of five blocks,
/// `15 - floor(15 / 8 * 5) = 6`, and clicks.
#[test]
fn a_sensor_hears_a_block_placed_five_blocks_away_floor_of_the_distance_ticks_later() {
    let mut region = ticking_with(&[(SENSOR, inactive())]);
    checked(&mut region, &holding(items::STONE));
    let placed = BlockPos::new(3, -60, 0);
    let output = checked(&mut region, &place_on_floor(placed));
    assert_eq!(block_at(&region, placed), Some(blocks::STONE));
    assert_eq!(
        particles(&output),
        [([3.5, -59.5, 0.5], SENSOR, 5)],
        "taken up in the tick the packet is applied in"
    );
    let applied_in = region.tick_number();
    let mut received = None;
    for _ in 0..8 {
        let output = checked(&mut region, &TickInputs::default());
        if phase(&region, SENSOR).0 == "active" {
            received = Some(region.tick_number());
            let clicks = super::redstone::sounds(&output);
            assert!(
                clicks
                    .iter()
                    .any(|(sound, ..)| *sound == clustine_data::sounds::BLOCK_SCULK_SENSOR_CLICKING),
                "{clicks:?}"
            );
            break;
        }
    }
    assert_eq!(received, Some(applied_in - 1 + 5));
    assert_eq!(phase(&region, SENSOR), ("active".to_owned(), 6));
    assert_eq!(region.sensor(SENSOR).map(|s| s.last_frequency), Some(13));
}

/// Of two events in one tick the closer is taken, whichever came first; of two as
/// close, the one of the higher frequency.
#[test]
fn the_closest_of_two_events_in_a_tick_wins_and_the_higher_frequency_at_equal_distance() {
    // An explosion five blocks away, then a step two away: the step.
    let mut region = ticking_with(&[(SENSOR, inactive())]);
    post(&mut region, Vibration::EXPLODE, BlockPos::new(3, -60, 0));
    post(&mut region, Vibration::STEP, BlockPos::new(6, -60, 0));
    assert!(ticks_until_active(&mut region, SENSOR, 4).is_some());
    assert_eq!(region.sensor(SENSOR).map(|s| s.last_frequency), Some(1));
    assert_eq!(
        phase(&region, SENSOR).1,
        15 - (15.0_f64 / 8.0 * 2.0).floor() as u8
    );

    // A step and an explosion as far on either side: the explosion, either way round.
    for (first, second) in [
        (Vibration::STEP, Vibration::EXPLODE),
        (Vibration::EXPLODE, Vibration::STEP),
    ] {
        let mut region = ticking_with(&[(SENSOR, inactive())]);
        post(&mut region, first, BlockPos::new(5, -60, 0));
        post(&mut region, second, BlockPos::new(11, -60, 0));
        assert!(ticks_until_active(&mut region, SENSOR, 5).is_some());
        assert_eq!(region.sensor(SENSOR).map(|s| s.last_frequency), Some(15));
    }

    // A closer event of a later tick does not take the place of one already chosen.
    let mut region = ticking_with(&[(SENSOR, inactive())]);
    post(&mut region, Vibration::EXPLODE, BlockPos::new(3, -60, 0));
    let taken = checked(&mut region, &TickInputs::default());
    assert_eq!(particles(&taken).len(), 1);
    post(&mut region, Vibration::STEP, BlockPos::new(7, -60, 0));
    assert!(ticks_until_active(&mut region, SENSOR, 6).is_some());
    assert_eq!(region.sensor(SENSOR).map(|s| s.last_frequency), Some(15));
}

/// A wool block between the event and the sensor stops all six lines and the sensor
/// hears nothing; carpet does not; and a block of wool placed is not heard however
/// near.
#[test]
fn a_wool_block_between_occludes_a_vibration_and_carpet_does_not() {
    let wool = state("minecraft:white_wool");
    let mut region = ticking_with(&[(SENSOR, inactive()), (BlockPos::new(6, -60, 0), wool)]);
    post(
        &mut region,
        Vibration::BLOCK_PLACE,
        BlockPos::new(3, -60, 0),
    );
    let outputs = idle_for(&mut region, 8);
    assert!(outputs.iter().all(|output| particles(output).is_empty()));
    assert_eq!(phase(&region, SENSOR).0, "inactive");

    let carpet = state("minecraft:white_carpet");
    let mut region = ticking_with(&[(SENSOR, inactive()), (BlockPos::new(6, -60, 0), carpet)]);
    post(
        &mut region,
        Vibration::BLOCK_PLACE,
        BlockPos::new(3, -60, 0),
    );
    assert!(ticks_until_active(&mut region, SENSOR, 6).is_some());

    let mut region = ticking_with(&[(SENSOR, inactive())]);
    let context = Context::of(None, Some(wool));
    region.vibrate_at(Vibration::BLOCK_PLACE, BlockPos::new(6, -60, 0), context);
    assert!(ticks_until_active(&mut region, SENSOR, 6).is_none());
}

/// Player 1 on the ground at `x`, `z`.
fn walk_to(x: f64, z: f64) -> TickInputs {
    moves(vec![walk(1, x, z)])
}

fn keys(sneak: bool) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::Keys(crate::api::Keys {
            sneak,
            ..Default::default()
        }),
    )])
}

/// A player walking two blocks makes a step, which a sensor hears; one walking with the
/// shift key down makes none it hears.
#[test]
fn a_sneaking_players_steps_are_not_heard_and_a_walking_players_are() {
    let sensor_at = BlockPos::new(4, -60, 4);
    for sneaking in [false, true] {
        let mut region = ticking_with(&[(sensor_at, inactive())]);
        checked(&mut region, &keys(sneaking));
        // On the ground first, then two blocks east.
        checked(&mut region, &walk_to(0.6, 0.5));
        checked(&mut region, &walk_to(1.6, 0.5));
        checked(&mut region, &walk_to(2.6, 0.5));
        let heard = ticks_until_active(&mut region, sensor_at, 8).is_some();
        assert_eq!(heard, !sneaking, "sneaking {sneaking}");
        if heard {
            assert_eq!(region.sensor(sensor_at).map(|s| s.last_frequency), Some(1));
        }
    }
}

/// A player who falls three blocks and lands is heard (`hit_ground`), unless they land
/// with the shift key down.
#[test]
fn a_players_landing_is_heard_unless_they_sneak() {
    let sensor_at = BlockPos::new(4, -60, 4);
    for sneaking in [false, true] {
        let mut region = ticking_with(&[(sensor_at, inactive())]);
        checked(&mut region, &keys(sneaking));
        let fall = |y: f64, on_ground: bool| {
            moves(vec![numbered(
                1,
                PlayerInput::Move {
                    position: Some(Vec3::new(0.5, y, 0.5)),
                    rotation: None,
                    on_ground,
                },
            )])
        };
        checked(&mut region, &fall(-57.0, false));
        checked(&mut region, &fall(-58.5, false));
        checked(&mut region, &fall(-60.0, true));
        let heard = ticks_until_active(&mut region, sensor_at, 8).is_some();
        assert_eq!(heard, !sneaking, "sneaking {sneaking}");
        if heard {
            assert_eq!(region.sensor(sensor_at).map(|s| s.last_frequency), Some(2));
        }
    }
}

/// A player standing on a sensor sets it off with the shift key down too, at full
/// power and the frequency of a step, a tick after they stand on it.
#[test]
fn stepping_on_a_sensor_activates_it_even_sneaking() {
    let sensor_at = BlockPos::new(2, -60, 0);
    let mut region = ticking_with(&[(sensor_at, inactive())]);
    checked(&mut region, &keys(true));
    let on_top = moves(vec![numbered(
        1,
        PlayerInput::Move {
            position: Some(Vec3::new(2.5, -59.5, 0.5)),
            rotation: None,
            on_ground: true,
        },
    )]);
    checked(&mut region, &on_top);
    assert_eq!(
        phase(&region, sensor_at).0,
        "inactive",
        "heard in the next tick"
    );
    let output = checked(&mut region, &TickInputs::default());
    assert_eq!(particles(&output), [([2.5, -59.5, 0.5], sensor_at, 0)]);
    assert_eq!(phase(&region, sensor_at), ("active".to_owned(), 15));
    assert_eq!(region.sensor(sensor_at).map(|s| s.last_frequency), Some(1));
}

/// An item lying on a sensor sets it off, and one of wool too: `stepOn` asks nothing
/// of the entity.
#[test]
fn an_item_on_a_sensor_sets_it_off_even_of_wool() {
    for item in [items::STONE, items::WHITE_WOOL] {
        let sensor_at = BlockPos::new(6, -60, 6);
        let mut region = ticking_with(&[(sensor_at, inactive())]);
        let entity = clustine_world::Entity {
            key: EntityId((1 << 40) + 1),
            kind: clustine_data::entity_types::ITEM as u16,
            uuid: uuid::Uuid::from_u128(1),
            position: [6.5, -59.5, 6.5],
            motion: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: true,
            hops: 0,
            age: 1,
            data: clustine_world::EntityData::Item {
                stack: ItemStack { item, count: 1 },
                pick_up_delay: 32_767,
                thrower: None,
                health: 5,
            },
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
        };
        let chunk = entity.chunk();
        region
            .chunks
            .get_mut(&chunk)
            .expect("loaded")
            .apply_entity(&clustine_world::EntityChange::Whole(entity.clone()));
        region.entity_chunks.insert(entity.key, chunk);
        assert!(
            ticks_until_active(&mut region, sensor_at, 8).is_some(),
            "item {item}"
        );
    }
}

/// A comparator behind an active sensor reads the frequency it last took (a block
/// placed: 13), and nothing once its cooldown begins.
#[test]
fn a_comparator_reads_the_frequency_of_an_active_sensor() {
    let comparator_at = SENSOR.offset(1, 0, 0);
    let comparator = state("minecraft:comparator[facing=west,mode=compare,powered=false]");
    let mut region = ticking_with(&[(SENSOR, inactive()), (comparator_at, comparator)]);
    post(
        &mut region,
        Vibration::BLOCK_PLACE,
        BlockPos::new(5, -60, 0),
    );
    assert!(ticks_until_active(&mut region, SENSOR, 5).is_some());
    idle_for(&mut region, 3);
    assert_eq!(region.comparator_output(comparator_at), 13);
    assert_eq!(
        region.sensor_output(SENSOR, block_at(&region, SENSOR).unwrap()),
        Some(13)
    );
    // 30 ticks active in all, then the cooldown, and the comparator's own delay.
    idle_for(&mut region, 30);
    assert_eq!(phase(&region, SENSOR).0, "cooldown");
    assert_eq!(region.comparator_output(comparator_at), 0);
}

/// A sensor is active for 30 ticks with its power, then cools down for 10 without
/// power, then is inactive again with its stopping click; neither active nor cooling
/// down does it hear, and inactive it hears again.
#[test]
fn a_sensor_is_active_for_thirty_ticks_cools_down_for_ten_and_hears_nothing_meanwhile() {
    let mut region = ticking_with(&[(SENSOR, inactive())]);
    post(&mut region, Vibration::STEP, BlockPos::new(8, -60, 1));
    assert_eq!(ticks_until_active(&mut region, SENSOR, 3), Some(1));
    let mut phases = Vec::new();
    for tick in 0..41 {
        if tick == 5 || tick == 35 {
            // Heard by nothing: it is active, then cooling down.
            post(&mut region, Vibration::EXPLODE, BlockPos::new(6, -60, 0));
        }
        let output = checked(&mut region, &TickInputs::default());
        assert!(particles(&output).is_empty(), "tick {tick}");
        phases.push(phase(&region, SENSOR));
    }
    let active = phases.iter().filter(|(phase, _)| phase == "active").count();
    let cooling = phases
        .iter()
        .filter(|(phase, power)| phase == "cooldown" && *power == 0)
        .count();
    assert_eq!(
        (active, cooling),
        (29, 10),
        "one active tick was the receipt's"
    );
    assert_eq!(phases.last(), Some(&("inactive".to_owned(), 0)));
    post(&mut region, Vibration::EXPLODE, BlockPos::new(6, -60, 0));
    assert!(ticks_until_active(&mut region, SENSOR, 4).is_some());
}

/// A sensor beside the line in another region's land does not hear an event in this
/// region's, which it would in one region (named: carrying the event over the line is a
/// later step's); and nothing is sent across for it.
#[test]
fn an_event_from_another_regions_land_is_not_heard_named() {
    // The line between x = 15 and x = 16: the western region holds chunk x = 0 and
    // copies chunk x = 1, where the sensor is.
    let across = BlockPos::new(18, -60, 0);
    let event = BlockPos::new(13, -60, 0);
    let mut land: BTreeMap<ChunkPos, Chunk> = (-1..=2)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .map(|position| (position, floor()))
        .collect();
    let (x, z) = across.in_chunk();
    land.get_mut(&across.chunk())
        .expect("in the land")
        .set(x, across.y, z, inactive());
    let west = ChunkArea {
        min_x: None,
        max_x: Some(1),
    };
    let region_of = |area: ChunkArea, copied: &[ChunkPos]| {
        let chunks: Vec<(ChunkPos, Chunk)> = land
            .iter()
            .filter(|(position, _)| area.contains(**position))
            .map(|(position, chunk)| (*position, chunk.clone()))
            .collect();
        let tickets: Vec<(ChunkPos, Ticket)> = chunks
            .iter()
            .map(|(position, _)| (*position, Ticket::Viewer))
            .collect();
        let holdings = Holdings {
            held: chunks.iter().map(|(position, _)| *position).collect(),
            pinned: vec![area],
            lent: Vec::new(),
            lent_away: Vec::new(),
        };
        let copies = copied
            .iter()
            .map(|position| (*position, land[position].clone()))
            .collect();
        let state = RegionState::new(EntityIds::block(0).expect("a block of ids"));
        Region::restore_loaded(config(), state, holdings, &tickets, chunks, copies)
    };
    let copied: Vec<ChunkPos> = (-1..=1).map(|z| ChunkPos::new(1, z)).collect();

    // In one region it is heard.
    let mut whole = region_of(ChunkArea::EVERYWHERE, &[]);
    checked(&mut whole, &TickInputs::default());
    post(&mut whole, Vibration::BLOCK_PLACE, event);
    assert!(ticks_until_active(&mut whole, across, 8).is_some());

    // Divided, the region of the event does not hear it, and sends nothing for it.
    let mut divided = region_of(west, &copied);
    checked(&mut divided, &TickInputs::default());
    post(&mut divided, Vibration::BLOCK_PLACE, event);
    for _ in 0..8 {
        let output = checked(&mut divided, &TickInputs::default());
        assert!(particles(&output).is_empty());
        assert!(output.effects.is_empty(), "{:?}", output.effects);
    }
}

/// Two regions that tick the same inputs, with sensors hearing, stepped on and cooling
/// down, put out the same, tick by tick; and what the sensors keep is in their chunks'
/// data, so a sensor read back from it holds what it held.
#[test]
fn a_tick_with_sensors_is_a_function_of_the_region_and_its_inputs() {
    let other = BlockPos::new(4, -60, 4);
    let build = || {
        let mut region = ticking_with(&[(SENSOR, inactive()), (other, inactive())]);
        checked(&mut region, &holding(items::STONE));
        region
    };
    let (mut one, mut two) = (build(), build());
    let script: Vec<TickInputs> = vec![
        place_on_floor(BlockPos::new(3, -60, 0)),
        walk_to(1.5, 0.5),
        walk_to(2.5, 1.5),
        TickInputs::default(),
        walk_to(4.5, 4.5),
        keys(true),
        walk_to(4.6, 4.5),
    ];
    for inputs in script
        .iter()
        .chain(std::iter::repeat_n(&TickInputs::default(), 50))
    {
        let (a, b) = (checked(&mut one, inputs), checked(&mut two, inputs));
        assert_eq!(a, b);
    }
    assert_eq!(one, two);
    // The data written at the end of each tick reads back as what the sensor holds.
    for at in [SENSOR, other] {
        let (x, z) = at.in_chunk();
        let data = one
            .chunk(at.chunk())
            .and_then(|chunk| {
                chunk
                    .block_entities()
                    .get(&clustine_world::InChunk::new(x, at.y, z))
            })
            .map(|entity| entity.data.clone())
            .unwrap_or_default();
        let read = crate::region::sculk::Sensor::loaded(&data);
        assert_eq!(read.saved(), one.sensor(at).expect("a sensor").saved());
    }
    let _ = Source::of(EntityId(0));
}

fn calibrated(facing: &str) -> BlockState {
    state(&format!(
        "minecraft:calibrated_sculk_sensor[facing={facing},power=0,sculk_sensor_phase=inactive,waterlogged=false]"
    ))
}

/// A calibrated sensor hears twice as far, is active for 10 ticks, and while a signal
/// comes into its back hears only that frequency; it gives no signal out of its back.
#[test]
fn a_calibrated_sensor_hears_sixteen_blocks_and_only_the_frequency_behind_it() {
    use clustine_blocks::signal::{Query, signal};
    use clustine_data::Face as Side;

    // Twelve blocks away: the calibrated sensor hears it.
    let at = BlockPos::new(14, -60, 0);
    let mut region = ticking_with(&[(at, calibrated("north"))]);
    post(
        &mut region,
        Vibration::BLOCK_PLACE,
        BlockPos::new(2, -60, 0),
    );
    assert!(ticks_until_active(&mut region, at, 14).is_some());
    let power = phase(&region, at).1;
    assert_eq!(power, 15 - (15.0_f64 / 16.0 * 12.0).floor() as u8);
    let mut view = crate::region::updates::RegionView(&mut region);
    // Asked northwards, by the block south of it: its back.
    assert_eq!(signal(&mut view, at, Side::North, Query::ALL), 0);
    assert_eq!(signal(&mut view, at, Side::South, Query::ALL), power);
    idle_for(&mut region, 10);
    assert_eq!(phase(&region, at).0, "cooldown", "active for 10 ticks only");

    // The same twelve blocks from a plain sensor: not heard.
    let mut region = ticking_with(&[(at, inactive())]);
    post(
        &mut region,
        Vibration::BLOCK_PLACE,
        BlockPos::new(2, -60, 0),
    );
    assert!(ticks_until_active(&mut region, at, 14).is_none());

    // A redstone block behind it (south): only frequency 15 is heard.
    let behind = at.offset(0, 0, 1);
    for (event, heard) in [(Vibration::STEP, false), (Vibration::EXPLODE, true)] {
        let mut region = ticking_with(&[
            (at, calibrated("north")),
            (behind, state("minecraft:redstone_block")),
        ]);
        post(&mut region, event, BlockPos::new(12, -60, 0));
        let active = ticks_until_active(&mut region, at, 6).is_some();
        assert_eq!(active, heard, "{event:?}");
    }
}

/// A sensor a player places from its item is one that hears; a calibrated one faces
/// the way the player looks.
#[test]
fn a_sensor_placed_by_a_player_hears_and_a_calibrated_one_faces_the_players_way() {
    let mut region = ticking_with(&[]);
    checked(&mut region, &holding(items::SCULK_SENSOR));
    let placed = BlockPos::new(2, -60, 0);
    checked(&mut region, &place_on_floor(placed));
    assert_eq!(block_at(&region, placed), Some(inactive()));
    assert!(region.sensor(placed).is_some());
    post(&mut region, Vibration::EXPLODE, BlockPos::new(2, -60, 3));
    assert!(ticks_until_active(&mut region, placed, 5).is_some());

    let mut region = ticking_with(&[]);
    checked(&mut region, &holding(items::CALIBRATED_SCULK_SENSOR));
    checked(&mut region, &place_on_floor(placed));
    let state = block_at(&region, placed).expect("placed");
    assert!(clustine_blocks::data::is_class(
        state,
        clustine_data::BlockClass::CalibratedSculkSensorBlock
    ));
    // The player of these tests looks south (yaw 0).
    assert_eq!(state.property("facing"), Some("south"));
}
