//! Tests of game events and sculk sensors in step N6, written from
//! `docs/groundwork/n6-plan.md` (section 1, decisions 9 and 10) and the porting brief
//! `docs/groundwork/n6-brief.md` (section 8, 9.6 and 11.6) and the public API alone, by
//! someone who did not write the code: which actions a sensor hears, at which distances
//! and when; the closest of a tick's events winning, then the higher frequency;
//! occlusion and dampening by wool; sneaking; a sensor stepped on; the sensor's phases,
//! its power by distance and the comparator reading its frequency; a calibrated sensor's
//! radius and filter; and a sensor beside a pinned line.
//!
//! Player 1 stands on a stone floor at y = -61 (feet at y = -60) in chunk (0, 0), whose
//! 5x5 is loaded. The sensor of most tests stands on the floor at (4, -60, 4); the
//! player acts from the south of it, along x = 4.5.
//!
//! On timing: the game handles a player's packets between its ticks, at the game time
//! of the tick before, and the round of these tests before this one found the region
//! does so too (a dispenser's 4-tick delay from a click). A vibration's candidate is held
//! with the game time of its post; the sensor's ticker takes it up once the game time is
//! past it, sets its travel to `floor(distance)` and lowers it in the same call, and
//! receives it when the travel reaches 0 (brief 8.0 item 6, 8.6). So a vibration of an
//! input of tick N, `floor(d)` at least 1, is taken up in tick N and received in tick
//! N + floor(d) - 1: `floor(d)` game ticks after its post. One posted within a tick (an
//! item landing) is taken up in the next and received `floor(d)` ticks after its tick.

mod n6_entities;
mod n6_scene;

use clustine_data::{BlockState, blocks, items, sounds};
use clustine_sim::TickOutput;
use clustine_sim::api::{Face, Keys, PlayerInput, RegionEvent};
use clustine_world::{BlockPos, ChunkPos, Vec3};
use n6_entities::*;
use n6_scene::*;

/// Where the sensor of most tests stands.
const SENSOR: BlockPos = BlockPos::new(4, -60, 4);
/// A comparator north of the sensor reading it, and the wire it feeds.
const COMPARATOR: BlockPos = BlockPos::new(4, -60, 3);
const WIRE: BlockPos = BlockPos::new(4, -60, 2);

fn sensor() -> BlockState {
    state("minecraft:sculk_sensor[sculk_sensor_phase=inactive,power=0,waterlogged=false]")
}

/// A sensor at [`SENSOR`] with a comparator reading it, and `more`.
fn with_comparator(more: &[(BlockPos, BlockState)]) -> World {
    let mut blocks = vec![
        (SENSOR, sensor()),
        (
            COMPARATOR,
            state("minecraft:comparator[facing=south,mode=compare,powered=false]"),
        ),
        (WIRE, blocks::REDSTONE_WIRE),
    ];
    blocks.extend_from_slice(more);
    World::of(&blocks)
}

/// The sensor's phase and power.
fn phase_at(scene: &Scene, at: BlockPos) -> (&'static str, u8) {
    let state = scene.block(at);
    let phase = state
        .property("sculk_sensor_phase")
        .unwrap_or_else(|| panic!("{at:?} is a sensor: {state:?}"));
    let power = state
        .property("power")
        .expect("a power")
        .parse()
        .expect("a number");
    (phase, power)
}

fn phase(scene: &Scene) -> (&'static str, u8) {
    phase_at(scene, SENSOR)
}

fn wire_power(scene: &Scene) -> u8 {
    scene
        .block(WIRE)
        .property("power")
        .expect("a wire")
        .parse()
        .expect("a number")
}

/// The power the sensor at `at` is received with at a block distance of `d`, by
/// `getRedstoneStrengthForDistance` for a radius of `radius` (brief 8.0 item 8, 8.9).
fn power_for(d: f64, radius: f64) -> u8 {
    let power = 15 - (15.0 / radius * d).floor() as i64;
    u8::try_from(power.max(1)).expect("small")
}

/// Ticks the scene until the sensor at `at` is active, for at most `ticks`, with
/// `first` the output of the tick that did what it hears: the index of the tick in
/// which it was seen active (0 for `first`'s), and every output.
fn until_active_at(
    scene: &mut Scene,
    at: BlockPos,
    first: TickOutput,
    ticks: usize,
) -> (Option<usize>, Vec<TickOutput>) {
    let mut outputs = vec![first];
    loop {
        if phase_at(scene, at).0 == "active" {
            return (Some(outputs.len() - 1), outputs);
        }
        if outputs.len() > ticks {
            return (None, outputs);
        }
        outputs.push(scene.idle());
    }
}

fn until_active(
    scene: &mut Scene,
    first: TickOutput,
    ticks: usize,
) -> (Option<usize>, Vec<TickOutput>) {
    until_active_at(scene, SENSOR, first, ticks)
}

/// Player 1 moves on the ground to `feet`.
fn walk(scene: &mut Scene, feet: Vec3) -> TickOutput {
    scene.walk_to(feet)
}

/// The feet of a player standing on the floor at the centre of column (`x`, `z`).
fn feet(x: i32, z: i32) -> Vec3 {
    Vec3::new(f64::from(x) + 0.5, -60.0, f64::from(z) + 0.5)
}

/// A scene with player 1 at `from`, who then takes one step of two blocks to `to`:
/// two blocks on the ground pass the first step's mark (`moveDist` 1.2 against
/// `nextStep` 1, brief 8.1).
fn step_from(world: &World, from: Vec3, to: Vec3) -> (Scene, TickOutput) {
    let mut scene = Scene::alone(world, inventory(&[]), from);
    let output = walk(&mut scene, to);
    (scene, output)
}

/// The frequency the comparator reads once the sensor is active: `None` if the sensor
/// does not turn active within `ticks`.
fn frequency_heard(scene: &mut Scene, first: TickOutput, ticks: usize) -> Option<u8> {
    let (active, _) = until_active(scene, first, ticks);
    active?;
    // The comparator's own delay of 2 ticks, then the wire.
    for _ in 0..6 {
        if wire_power(scene) > 0 {
            return Some(wire_power(scene));
        }
        scene.idle();
    }
    Some(wire_power(scene))
}

fn vibrations(output: &TickOutput) -> Vec<([f64; 3], BlockPos, i32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Vibration { from, to, ticks } => Some((*from, *to, *ticks)),
            _ => None,
        })
        .collect()
}

fn holding(item: i32) -> clustine_sim::api::Inventory {
    inventory(&[(0, stack(item, 64))])
}

// ------------------------------------------------------------------------------------
// When and how loud (brief 8.0 items 6 and 8, 8.6, 8.9).

/// Brief 8.0 item 6 and 8.6: a step whose position is 4.03 blocks from the sensor's
/// centre is taken up in the tick of its move, with the vibration's particle from the
/// step's position to the sensor's block in 4 ticks, and received 4 game ticks after
/// its post: in the third tick after the move's. Its power is that of the block
/// distance, 4 (8.0 item 8): 15 - floor(15 / 8 x 4) = 8.
#[test]
fn a_step_is_received_floor_of_its_distance_after_it_was_made_with_its_particle() {
    let (mut scene, output) = step_from(&World::of(&[(SENSOR, sensor())]), feet(4, 10), feet(4, 8));
    assert_eq!(
        vibrations(&output),
        vec![([4.5, -60.0, 8.5], SENSOR, 4)],
        "the particle when it is taken up"
    );
    let (active, outputs) = until_active(&mut scene, output, 10);
    assert_eq!(active, Some(3), "received 4 game ticks after the move's");
    assert_eq!(phase(&scene), ("active", 8));
    let clicks = sounds(&outputs[3], sounds::BLOCK_SCULK_SENSOR_CLICKING);
    assert_eq!(
        clicks,
        vec![(centre(SENSOR), 1.0)],
        "clicking at its centre"
    );
}

/// Brief 8.0 item 8 and 8.9: the power by block distance d is max(1, 15 - floor(15 / 8
/// x d)), and travel is floor of the distance between the step and the sensor's centre:
/// for a step d blocks south of the sensor on the floor, d ticks.
#[test]
fn the_power_falls_with_the_block_distance_and_the_travel_grows_with_it() {
    for d in 1..=8 {
        let (mut scene, output) = step_from(
            &World::of(&[(SENSOR, sensor())]),
            feet(4, 4 + d + 2),
            feet(4, 4 + d),
        );
        let (active, _) = until_active(&mut scene, output, 12);
        assert_eq!(
            active,
            Some(usize::try_from(d - 1).expect("positive")),
            "at {d}"
        );
        assert_eq!(
            phase(&scene),
            ("active", power_for(f64::from(d), 8.0)),
            "at {d}"
        );
    }
}

/// Brief 8.2 step 3 and 8.0 item 8: a sensor hears within 8 blocks, `dx² + dy² + dz² <=
/// 64` between block positions: 4 across and 6 along (52) at power 2, received after
/// floor(7.23) = 7 game ticks; 4 across and 7 along (65) and 9 along (81) not at all.
#[test]
fn a_sensor_hears_within_eight_blocks_between_block_positions_and_no_further() {
    let (mut scene, output) =
        step_from(&World::of(&[(SENSOR, sensor())]), feet(8, 12), feet(8, 10));
    let (active, _) = until_active(&mut scene, output, 12);
    assert_eq!(active, Some(6));
    assert_eq!(phase(&scene), ("active", power_for(52f64.sqrt(), 8.0)));
    assert_eq!(power_for(52f64.sqrt(), 8.0), 2);
    for (from, to) in [(feet(8, 13), feet(8, 11)), (feet(4, 15), feet(4, 13))] {
        let (mut scene, output) = step_from(&World::of(&[(SENSOR, sensor())]), from, to);
        let (active, _) = until_active(&mut scene, output, 15);
        assert_eq!(active, None, "a step at {to:?} is out of reach");
    }
}

// ------------------------------------------------------------------------------------
// The selection (brief 8.0 item 7, 8.6).

/// Brief 8.0 item 7: of the events of one tick the closest wins, whatever its
/// frequency: a block placed 6.3 blocks away (13) and a step 2.1 away (1) in one tick,
/// in either order, give the step's frequency and its power (block distance 2: 12).
#[test]
fn the_closest_of_a_ticks_events_wins_whatever_its_frequency() {
    for step_first in [false, true] {
        let mut scene = Scene::alone(&with_comparator(&[]), holding(items::STONE), feet(4, 8));
        let sequence = scene.actors[0].sequence + 1;
        scene.actors[0].sequence = sequence;
        let place = PlayerInput::UseItemOn {
            position: BlockPos::new(6, -61, 10),
            face: Face::Top,
            cursor: [0.5, 1.0, 0.5],
            sneaking: false,
            sequence,
        };
        let step = PlayerInput::Move {
            position: Some(feet(4, 6)),
            rotation: None,
            on_ground: true,
        };
        let inputs = if step_first {
            vec![step, place]
        } else {
            vec![place, step]
        };
        let output = scene.inputs_of(0, inputs);
        assert_eq!(scene.block(BlockPos::new(6, -60, 10)), blocks::STONE);
        let heard = frequency_heard(&mut scene, output, 10);
        assert_eq!(heard, Some(1), "the step, step first: {step_first}");
        assert_eq!(
            phase(&scene).1,
            12,
            "the step's power, step first: {step_first}"
        );
    }
}

/// Brief 8.0 item 7: at the same distance the higher frequency wins, and at equal
/// frequency the first: a block broken (12) and one placed (13) at the same place in
/// one tick, in either order, are heard as the placing.
#[test]
fn at_one_distance_the_higher_frequency_wins_whichever_came_first() {
    let at = BlockPos::new(4, -60, 8);
    for dig_first in [true, false] {
        let world = if dig_first {
            with_comparator(&[(at, blocks::DIRT)])
        } else {
            with_comparator(&[])
        };
        let mut scene = Scene::alone(&world, holding(items::STONE), feet(6, 8));
        scene.actors[0].sequence += 2;
        let sequence = scene.actors[0].sequence;
        let dig = PlayerInput::Dig {
            position: at,
            sequence: sequence - 1,
        };
        let place = PlayerInput::UseItemOn {
            position: at.offset(0, -1, 0),
            face: Face::Top,
            cursor: [0.5, 1.0, 0.5],
            sneaking: false,
            sequence,
        };
        let inputs = if dig_first {
            vec![dig, place]
        } else {
            vec![place, dig]
        };
        let output = scene.inputs_of(0, inputs);
        let expected = if dig_first {
            blocks::STONE
        } else {
            blocks::AIR
        };
        assert_eq!(scene.block(at), expected, "both inputs took effect");
        let heard = frequency_heard(&mut scene, output, 10);
        assert_eq!(heard, Some(13), "the placing, dig first: {dig_first}");
        assert_eq!(phase(&scene).1, 8, "block distance 4");
    }
}

/// Brief 8.3 step 1 and 8.0 item 6: while the sensor holds a vibration in travel it
/// hears nothing else: a step 8 away, then steps nearer every tick while it travels,
/// are received as the first, at power 1, and nothing more is taken up, then or after.
#[test]
fn a_sensor_holding_a_vibration_in_travel_hears_nothing_else() {
    let mut scene = Scene::alone(&with_comparator(&[]), inventory(&[]), feet(4, 14));
    let mut outputs = vec![walk(&mut scene, feet(4, 12))];
    for z in [10, 8, 6] {
        outputs.push(walk(&mut scene, feet(4, z)));
    }
    outputs.extend(scene.idle_for(50));
    let taken: Vec<_> = outputs.iter().flat_map(vibrations).collect();
    assert_eq!(taken.len(), 1, "one taken up: {taken:?}");
    assert_eq!(taken[0].0, [4.5, -60.0, 12.5], "the first step");
    let activated = outputs
        .iter()
        .position(|output| !sounds(output, sounds::BLOCK_SCULK_SENSOR_CLICKING).is_empty())
        .expect("heard");
    assert_eq!(activated, 7, "8 game ticks after the first step");
}

// ------------------------------------------------------------------------------------
// The phases and the comparator (brief 8.0 item 8, 8.8).

/// Brief 8.8: active for 30 ticks, then 10 in cooldown at power 0 with the clicking's
/// end, then inactive: the cooldown is set by the tick scheduled 30 ticks after the
/// activation, and the end by the one 10 after that.
#[test]
fn a_sensor_is_active_for_thirty_ticks_then_cools_down_for_ten() {
    let (mut scene, output) = step_from(&with_comparator(&[]), feet(4, 7), feet(4, 5));
    let (active, mut outputs) = until_active(&mut scene, output, 5);
    let activated = active.expect("heard");
    let mut phases = vec![phase(&scene)];
    for _ in 0..45 {
        outputs.push(scene.idle());
        phases.push(phase(&scene));
    }
    for (tick, phase) in phases.iter().enumerate() {
        let expected = match tick {
            0..30 => ("active", 14),
            30..40 => ("cooldown", 0),
            _ => ("inactive", 0),
        };
        assert_eq!(*phase, expected, "{tick} ticks after the activation");
    }
    let stops: Vec<usize> = outputs
        .iter()
        .enumerate()
        .filter(|(_, output)| !sounds(output, sounds::BLOCK_SCULK_SENSOR_CLICKING_STOP).is_empty())
        .map(|(tick, _)| tick - activated)
        .collect();
    assert_eq!(
        stops,
        vec![40],
        "the end of the clicking, when it is inactive again"
    );
}

/// Brief 8.3 step 4: a sensor in cooldown cannot activate, and what it heard then is
/// not kept for later.
#[test]
fn a_sensor_in_cooldown_hears_nothing_and_keeps_nothing_for_later() {
    let (mut scene, output) = step_from(&with_comparator(&[]), feet(4, 7), feet(4, 5));
    until_active(&mut scene, output, 5).0.expect("heard");
    scene.idle_for(31);
    assert_eq!(phase(&scene).0, "cooldown");
    let mut outputs = vec![walk(&mut scene, feet(4, 7))];
    outputs.extend(scene.idle_for(25));
    assert_eq!(phase(&scene).0, "inactive");
    assert!(
        outputs.iter().all(|output| vibrations(output).is_empty()),
        "nothing taken up"
    );
}

/// Brief 8.0 item 8 and 8.8: a comparator reads the frequency of the last vibration
/// while the sensor is active (a step: 1), not its signal, which is by distance (8 at
/// 4 blocks), and 0 once it is no longer active.
#[test]
fn a_comparator_reads_the_frequency_while_the_sensor_is_active() {
    let (mut scene, output) = step_from(&with_comparator(&[]), feet(4, 10), feet(4, 8));
    assert_eq!(frequency_heard(&mut scene, output, 10), Some(1));
    assert_eq!(phase(&scene), ("active", 8));
    let mut seen = Vec::new();
    assert!(
        scene.idle_until(40, &mut seen, |scene, _| phase(scene).0 == "cooldown"),
        "cooled down"
    );
    let mut seen = Vec::new();
    assert!(
        scene.idle_until(6, &mut seen, |scene, _| wire_power(scene) == 0),
        "the comparator reads 0 in cooldown"
    );
}

// ------------------------------------------------------------------------------------
// What is heard (brief 8.0 item 2, 8.1, 8.4, 8.7).

/// Brief 8.1 and 8.7: what a player does is heard with its frequency, which the
/// comparator reads: a step 1, a landing 2, a lever pulled and a door opened 10 (block
/// activate and open), a chest opened 10 (container open), a block broken 12, a block
/// placed 13 and a TNT lit by flint and steel 10 (prime fuse). Each is done about 4
/// blocks south of the sensor.
#[test]
fn what_a_player_does_is_heard_with_its_frequency() {
    let at = BlockPos::new(4, -60, 8);
    type Act = fn(&mut Scene) -> TickOutput;
    // What is done, the blocks it is done with, what the player holds, where they stand,
    // the act and the frequency heard.
    type Case = (
        &'static str,
        Vec<(BlockPos, BlockState)>,
        i32,
        Vec3,
        Act,
        u8,
    );
    let place: Act = |scene| scene.click(BlockPos::new(4, -61, 8), Face::Top);
    let use_it: Act = |scene| scene.click(BlockPos::new(4, -60, 8), Face::Top);
    let dig: Act = |scene| scene.dig(BlockPos::new(4, -60, 8));
    let step: Act = |scene| scene.walk_to(Vec3::new(4.5, -60.0, 8.5));
    let land: Act = |scene| {
        scene.input(PlayerInput::Move {
            position: Some(Vec3::new(6.5, -58.0, 8.5)),
            rotation: None,
            on_ground: false,
        });
        scene.input(PlayerInput::Move {
            position: Some(Vec3::new(6.5, -60.0, 8.5)),
            rotation: None,
            on_ground: true,
        })
    };
    let cases: Vec<Case> = vec![
        ("a step", vec![], items::STONE, feet(4, 10), step, 1),
        ("a landing", vec![], items::STONE, feet(6, 8), land, 2),
        (
            "a lever pulled",
            vec![(
                at,
                state("minecraft:lever[face=floor,facing=north,powered=false]"),
            )],
            items::STONE,
            feet(6, 8),
            use_it,
            10,
        ),
        (
            "a door opened",
            vec![
                (
                    at,
                    state("minecraft:oak_door[half=lower,facing=north,open=false]"),
                ),
                (
                    at.offset(0, 1, 0),
                    state("minecraft:oak_door[half=upper,facing=north,open=false]"),
                ),
            ],
            items::STONE,
            feet(6, 8),
            use_it,
            10,
        ),
        (
            "a chest opened",
            vec![(at, state("minecraft:chest[facing=north,type=single]"))],
            items::STONE,
            feet(6, 8),
            use_it,
            10,
        ),
        (
            "a block broken",
            vec![(at, blocks::DIRT)],
            items::STONE,
            feet(6, 8),
            dig,
            12,
        ),
        (
            "a block placed",
            vec![],
            items::STONE,
            feet(6, 8),
            place,
            13,
        ),
        (
            "TNT lit",
            vec![(at, blocks::TNT)],
            items::FLINT_AND_STEEL,
            feet(6, 8),
            use_it,
            10,
        ),
    ];
    for (what, blocks, item, from, act, frequency) in cases {
        let mut scene = Scene::alone(&with_comparator(&blocks), holding(item), from);
        let output = act(&mut scene);
        assert_eq!(
            frequency_heard(&mut scene, output, 12),
            Some(frequency),
            "{what}"
        );
    }
}

/// Brief 8.0 item 2 and 8.1 ("Posted by the N5 code: `entity_place` by `MinecartItem`,
/// `BoatItem`, `ArmorStandItem`"; "N6 has to go back and post them"): an armour stand
/// placed, and a minecart placed on a rail, are heard with frequency 14.
#[test]
fn placing_an_armour_stand_or_a_minecart_is_heard_as_an_entity_placed() {
    let at = BlockPos::new(4, -60, 8);
    for (what, blocks, item, clicked, kind) in [
        (
            "an armour stand",
            vec![],
            items::ARMOR_STAND,
            at.offset(0, -1, 0),
            clustine_data::entity_types::ARMOR_STAND,
        ),
        (
            "a minecart",
            vec![(at, state("minecraft:rail[shape=north_south]"))],
            items::MINECART,
            at,
            clustine_data::entity_types::MINECART,
        ),
    ] {
        let mut scene = Scene::alone(&with_comparator(&blocks), holding(item), feet(6, 8));
        let output = scene.click(clicked, Face::Top);
        assert_eq!(scene.of_kind(kind).len(), 1, "{what} placed");
        assert_eq!(frequency_heard(&mut scene, output, 12), Some(14), "{what}");
    }
}

/// Brief 8.1 and 8.7: an explosion is heard with frequency 15 (`explode`, posted
/// before its blocks are computed), here primed TNT 8 blocks south of the sensor,
/// beyond the reach of its rays.
#[test]
fn an_explosion_is_heard_with_frequency_fifteen() {
    let world = with_comparator(&[]).with_entities(&[primed(1, [4.5, -60.0, 12.5], 6)]);
    let mut scene = Scene::alone(&world, inventory(&[]), feet(0, 0));
    let output = scene.idle();
    assert_eq!(frequency_heard(&mut scene, output, 30), Some(15));
    assert_eq!(phase(&scene).1, 1, "block distance 8");
}

/// Brief 8.1 ("an item or a falling block landing after a fall posts `hit_ground`") and
/// 8.0 item 6: an item landing 4 blocks from the sensor is heard with frequency 2,
/// received 4 ticks after the tick it landed in, a tick late at the earliest.
#[test]
fn an_item_landing_is_heard_four_ticks_after_it_landed() {
    let mut falling = dirt(1, [4.5, -57.0, 8.5]);
    falling.on_ground = false;
    let world = with_comparator(&[]).with_entities(&[falling]);
    let mut scene = Scene::alone(&world, inventory(&[]), feet(0, 0));
    let mut landed = None;
    for tick in 0..40 {
        scene.idle();
        let item = scene.all().into_iter().find(|entity| entity.key == key(1));
        if item.is_some_and(|item| item.on_ground) {
            landed = Some(tick);
            break;
        }
        assert_eq!(phase(&scene).0, "inactive", "nothing before the landing");
    }
    landed.expect("the item landed");
    let output = TickOutput::default();
    let (active, _) = until_active(&mut scene, output, 10);
    assert_eq!(active, Some(4), "4 ticks after the landing's tick");
    let mut seen = Vec::new();
    assert!(scene.idle_until(6, &mut seen, |scene, _| wire_power(scene) == 2));
}

/// Brief 8.4 step 2: an item whose stack dampens vibrations (wool) makes no landing a
/// sensor hears.
#[test]
fn a_wool_item_landing_is_not_heard() {
    let mut falling = item(1, stack(items::WHITE_WOOL, 1), [4.5, -57.0, 8.5]);
    falling.on_ground = false;
    let world = with_comparator(&[]).with_entities(&[falling]);
    let mut scene = Scene::alone(&world, inventory(&[]), feet(0, 0));
    let outputs = scene.idle_for(40);
    assert!(
        scene
            .all()
            .iter()
            .any(|entity| entity.key == key(1) && entity.on_ground),
        "it landed"
    );
    assert_eq!(phase(&scene).0, "inactive");
    assert!(outputs.iter().all(|output| vibrations(output).is_empty()));
}

/// Brief 8.4 step 3: a step on wool, and wool placed or broken, is not heard: the
/// affected state dampens it. The same done with stone is (the control of the first).
#[test]
fn stepping_on_placing_and_breaking_wool_are_not_heard() {
    let wool_floor = BlockPos::new(4, -61, 8);
    let (mut scene, output) = step_from(
        &with_comparator(&[(wool_floor, blocks::WHITE_WOOL)]),
        feet(4, 10),
        feet(4, 8),
    );
    assert_eq!(
        until_active(&mut scene, output, 10).0,
        None,
        "a step on wool"
    );

    let at = BlockPos::new(4, -60, 8);
    let mut scene = Scene::alone(
        &with_comparator(&[]),
        holding(items::WHITE_WOOL),
        feet(6, 8),
    );
    let output = scene.click(at.offset(0, -1, 0), Face::Top);
    assert_eq!(scene.block(at), blocks::WHITE_WOOL);
    assert_eq!(until_active(&mut scene, output, 10).0, None, "wool placed");
    let output = scene.dig(at);
    assert_eq!(scene.block(at), blocks::AIR);
    assert_eq!(until_active(&mut scene, output, 10).0, None, "wool broken");
}

// ------------------------------------------------------------------------------------
// Occlusion (brief 8.5).

/// Brief 8.5: a wool block on the line between the event's block and the sensor's
/// occludes it, as every one of the six nudged lines meets it; a stone block there does
/// not (only `#occludes_vibration_signals`, the wools, do).
#[test]
fn wool_between_a_step_and_the_sensor_occludes_it_and_stone_does_not() {
    let between = BlockPos::new(4, -60, 6);
    let (mut scene, output) = step_from(
        &with_comparator(&[(between, blocks::WHITE_WOOL)]),
        feet(4, 10),
        feet(4, 8),
    );
    assert_eq!(
        until_active(&mut scene, output, 12).0,
        None,
        "occluded by wool"
    );
    let (mut scene, output) = step_from(
        &with_comparator(&[(between, blocks::STONE)]),
        feet(4, 10),
        feet(4, 8),
    );
    assert_eq!(
        until_active(&mut scene, output, 12).0,
        Some(3),
        "stone lets it through"
    );
}

/// Brief 8.5: a wool block beside the line, not on it, occludes nothing.
#[test]
fn wool_beside_the_line_occludes_nothing() {
    let (mut scene, output) = step_from(
        &with_comparator(&[(BlockPos::new(5, -60, 6), blocks::WHITE_WOOL)]),
        feet(4, 10),
        feet(4, 8),
    );
    assert_eq!(until_active(&mut scene, output, 12).0, Some(3));
}

// ------------------------------------------------------------------------------------
// Sneaking and stepping on a sensor (brief 8.0 items 4 and 5, 8.12).

fn sneak(scene: &mut Scene) -> TickOutput {
    scene.input(PlayerInput::Keys(Keys {
        sneak: true,
        ..Keys::default()
    }))
}

/// Brief 8.0 item 4 and 8.4 step 2: a sneaking player's steps are not heard.
#[test]
fn a_sneaking_players_steps_are_not_heard() {
    let mut scene = Scene::alone(&with_comparator(&[]), inventory(&[]), feet(4, 10));
    sneak(&mut scene);
    let output = walk(&mut scene, feet(4, 8));
    assert_eq!(until_active(&mut scene, output, 12).0, None);
}

/// Brief 8.0 item 4: a sneaking player's landing is not heard (`hit_ground` is in
/// `#ignore_vibrations_sneaking`); the same landing not sneaking is, as a landing (2).
#[test]
fn a_sneaking_players_landing_is_not_heard_and_one_not_sneaking_is() {
    for sneaking in [true, false] {
        let mut scene = Scene::alone(&with_comparator(&[]), inventory(&[]), feet(4, 8));
        if sneaking {
            sneak(&mut scene);
        }
        scene.input(PlayerInput::Move {
            position: Some(Vec3::new(4.5, -58.0, 8.5)),
            rotation: None,
            on_ground: false,
        });
        let output = scene.input(PlayerInput::Move {
            position: Some(feet(4, 8)),
            rotation: None,
            on_ground: true,
        });
        let heard = frequency_heard(&mut scene, output, 12);
        let expected = if sneaking { None } else { Some(2) };
        assert_eq!(heard, expected, "sneaking: {sneaking}");
    }
}

/// Brief 8.0 item 5 and 8.12: stepping on a sensor sets it off even sneaking, at
/// distance 0 (power 15) with a step's frequency (1), received at once: the entity
/// stands at the sensor's y + 0.5, within its block.
#[test]
fn stepping_on_a_sensor_sets_it_off_even_sneaking() {
    let mut scene = Scene::alone(&with_comparator(&[]), inventory(&[]), feet(4, 6));
    sneak(&mut scene);
    let output = walk(&mut scene, Vec3::new(4.5, -59.5, 4.5));
    let (active, _) = until_active(&mut scene, output, 3);
    assert!(active.is_some(), "set off");
    assert_eq!(phase(&scene), ("active", 15));
    let mut seen = Vec::new();
    assert!(scene.idle_until(6, &mut seen, |scene, _| wire_power(scene) == 1));
}

/// Brief 8.12: an entity standing on an inactive sensor sets it off again every 30 + 10
/// ticks and a tick for the selection: the step on it in the tick the cooldown ends is
/// taken up and received in the next.
#[test]
fn a_player_standing_on_a_sensor_sets_it_off_again_a_tick_after_its_cooldown() {
    let mut scene = Scene::alone(&with_comparator(&[]), inventory(&[]), feet(4, 6));
    sneak(&mut scene);
    let output = walk(&mut scene, Vec3::new(4.5, -59.5, 4.5));
    until_active(&mut scene, output, 3).0.expect("set off");
    let mut phases = Vec::new();
    for _ in 0..45 {
        scene.idle();
        phases.push(phase(&scene).0);
    }
    // phases[i] is i + 1 ticks after the activation.
    assert_eq!(phases[38], "cooldown", "{phases:?}");
    assert_eq!(phases[39], "inactive", "{phases:?}");
    assert_eq!(phases[40], "active", "{phases:?}");
}

/// Brief 8.0 item 5 and 8.14's "every entity on the ground": an item lying on a sensor
/// sets it off, at power 15.
#[test]
fn an_item_lying_on_a_sensor_sets_it_off() {
    let world = with_comparator(&[]).with_entities(&[dirt(1, [4.5, -59.5, 4.5])]);
    let mut scene = Scene::alone(&world, inventory(&[]), feet(0, 0));
    let output = scene.idle();
    let (active, _) = until_active(&mut scene, output, 10);
    assert!(active.is_some(), "set off");
    assert_eq!(phase(&scene), ("active", 15));
}

// ------------------------------------------------------------------------------------
// The calibrated sensor (brief 8.0 item 11, 8.10).

const CALIBRATED: BlockPos = BlockPos::new(4, -60, 4);

fn calibrated_world(back_powered: bool) -> World {
    let mut blocks = vec![
        (
            CALIBRATED,
            state(
                "minecraft:calibrated_sculk_sensor[facing=north,sculk_sensor_phase=inactive,power=0]",
            ),
        ),
        // West of it, reading it.
        (
            BlockPos::new(3, -60, 4),
            state("minecraft:comparator[facing=east,mode=compare,powered=false]"),
        ),
        (BlockPos::new(2, -60, 4), blocks::REDSTONE_WIRE),
    ];
    if back_powered {
        // Its back is the side opposite its facing: south.
        blocks.push((BlockPos::new(4, -60, 5), blocks::REDSTONE_BLOCK));
    }
    World::of(&blocks)
}

fn calibrated_wire(scene: &Scene) -> u8 {
    scene
        .block(BlockPos::new(2, -60, 4))
        .property("power")
        .expect("a wire")
        .parse()
        .expect("a number")
}

/// Brief 8.10: a calibrated sensor hears within 16 blocks, with the power of a radius
/// of 16 (at 12: 15 - floor(15 / 16 x 12) = 4), and is active for 10 ticks.
#[test]
fn a_calibrated_sensor_hears_within_sixteen_blocks_and_is_active_for_ten_ticks() {
    let mut scene = Scene::alone(&calibrated_world(false), inventory(&[]), feet(4, 18));
    let output = walk(&mut scene, feet(4, 16));
    let (active, _) = until_active_at(&mut scene, CALIBRATED, output, 15);
    assert_eq!(active, Some(11), "12 game ticks after the step");
    assert_eq!(
        phase_at(&scene, CALIBRATED),
        ("active", power_for(12.0, 16.0))
    );
    let mut phases = Vec::new();
    for _ in 0..21 {
        scene.idle();
        phases.push(phase_at(&scene, CALIBRATED).0);
    }
    // phases[i] is i + 1 ticks after the activation.
    assert_eq!(phases[8], "active", "{phases:?}");
    assert_eq!(phases[9], "cooldown", "{phases:?}");
    assert_eq!(phases[19], "inactive", "{phases:?}");
}

/// Brief 8.10: while the signal into its back is above 0, a calibrated sensor hears only
/// events of exactly that frequency: with a redstone block behind it (15) a step and a
/// block placed are not heard, and an explosion (15) is, which its comparator reads.
/// Without the block the step is heard (the control).
#[test]
fn a_calibrated_sensor_with_a_signal_at_its_back_hears_only_that_frequency() {
    let mut scene = Scene::alone(&calibrated_world(true), holding(items::STONE), feet(4, 10));
    let output = walk(&mut scene, feet(4, 8));
    assert_eq!(
        until_active_at(&mut scene, CALIBRATED, output, 10).0,
        None,
        "a step (1) is filtered out"
    );
    let output = scene.click(BlockPos::new(6, -61, 8), Face::Top);
    assert_eq!(scene.block(BlockPos::new(6, -60, 8)), blocks::STONE);
    assert_eq!(
        until_active_at(&mut scene, CALIBRATED, output, 10).0,
        None,
        "a block placed (13) is filtered out"
    );

    let world = calibrated_world(true).with_entities(&[primed(1, [4.5, -60.0, 14.5], 6)]);
    let mut scene = Scene::alone(&world, inventory(&[]), feet(0, 0));
    let output = scene.idle();
    let (active, _) = until_active_at(&mut scene, CALIBRATED, output, 30);
    assert!(active.is_some(), "the explosion (15) is heard");
    let mut seen = Vec::new();
    assert!(scene.idle_until(6, &mut seen, |scene, _| calibrated_wire(scene) == 15));

    let mut scene = Scene::alone(&calibrated_world(false), inventory(&[]), feet(4, 10));
    let output = walk(&mut scene, feet(4, 8));
    assert!(
        until_active_at(&mut scene, CALIBRATED, output, 10)
            .0
            .is_some(),
        "with nothing at its back the step is heard"
    );
    let mut seen = Vec::new();
    assert!(scene.idle_until(6, &mut seen, |scene, _| calibrated_wire(scene) == 1));
}

// ------------------------------------------------------------------------------------
// At a pinned line (plan decision 10; brief 8.0 item 9, 11.6).

/// A sensor in the west's chunk (0, 0) beside the line at x = 16.
const BY_THE_LINE: BlockPos = BlockPos::new(15, -60, 4);

/// Brief 8.0 item 9 and 11.6: a sensor beside a line, whose 3x3 of chunks is partly
/// copies, still receives what it hears from its own side ("read as the sensor's chunk
/// ticking", or it would hold its vibration for ever).
#[test]
fn a_sensor_beside_a_pinned_line_hears_its_own_side() {
    let world = World::of(&[(BY_THE_LINE, sensor())]);
    let mut scene = Scene::new(Layout::side(true), &world, inventory(&[]), feet(15, 10));
    let output = walk(&mut scene, feet(15, 8));
    let (active, _) = until_active_at(&mut scene, BY_THE_LINE, output, 10);
    assert_eq!(active, Some(3));
    assert_eq!(phase_at(&scene, BY_THE_LINE), ("active", 8));
}

/// Plan decision 10 names a vibration across a line as an effect, and the commit of
/// N6f "an event is heard only by the region's own sensors": a block placed 2 blocks
/// across the line from a sensor is not heard by it. Nothing worse comes of it: the
/// region beyond reports no vibration for the sensor it sees in its copy, what crosses
/// to the sensor's region changes nothing of the sensor, and the sensor hears its own
/// side afterwards.
#[test]
fn a_sensor_does_not_hear_across_a_pinned_line_and_goes_on_hearing_its_own_side() {
    let world = World::of(&[(BY_THE_LINE, sensor())]);
    let mut west = Scene::new(Layout::side(true), &world, inventory(&[]), feet(15, 12));
    let mut east = Scene::new(
        Layout::side(false),
        &world,
        holding(items::STONE),
        feet(19, 4),
    );
    let placed = BlockPos::new(17, -60, 4);
    let mut outputs = vec![east.click(placed.offset(0, -1, 0), Face::Top)];
    assert_eq!(east.block(placed), blocks::STONE);
    outputs.extend(east.idle_for(10));
    for output in &outputs {
        assert!(
            vibrations(output)
                .iter()
                .all(|(_, to, _)| *to != BY_THE_LINE),
            "the east takes up nothing for the west's sensor"
        );
    }
    let crossing: Vec<_> = outputs
        .iter()
        .flat_map(|output| output.effects.iter().cloned())
        .filter(|effect| effect.at.chunk().x <= 0)
        .collect();
    let mut seen = filed(&mut west.region, 40, crossing);
    seen.extend(west.idle_for(10));
    assert_eq!(phase_at(&west, BY_THE_LINE).0, "inactive", "not heard");
    assert!(seen.iter().all(|output| vibrations(output).is_empty()));
    assert!(
        west.region.chunk(ChunkPos::new(0, 0)).is_some(),
        "the sensor's chunk is still the west's"
    );
    let output = walk(&mut west, feet(15, 10));
    let (active, _) = until_active_at(&mut west, BY_THE_LINE, output, 10);
    assert!(active.is_some(), "the west's own step is heard after");
}

// ------------------------------------------------------------------------------------
// Orders and lives (brief 8.0 item 7, 8.2 step 5, 8.3 step 4, 8.6, 8.9, 8.11).

/// Brief 8.0 item 7 with this file's note on timing: an item landing in tick T is
/// posted at T's game time, and so is a step whose input comes in tick T + 1, handled
/// between the two ticks: the two are candidates of one game tick, and the closer step
/// wins over the landing 8 blocks away. A step a tick later comes after the landing was
/// taken up, and is not heard.
#[test]
fn an_input_after_a_landing_is_of_the_landings_game_tick_and_the_closer_wins() {
    for late in [false, true] {
        let mut falling = dirt(1, [4.5, -57.0, 12.5]);
        falling.on_ground = false;
        let world = with_comparator(&[]).with_entities(&[falling]);
        let mut scene = Scene::alone(&world, inventory(&[]), feet(6, 8));
        let mut landed = false;
        for _ in 0..40 {
            scene.idle();
            let item = scene.all().into_iter().find(|entity| entity.key == key(1));
            if item.is_some_and(|item| item.on_ground) {
                landed = true;
                break;
            }
        }
        assert!(landed, "the item landed");
        if late {
            scene.idle();
        }
        // Two blocks west, to 4 along from the sensor: closer than the landing's 8.
        let output = walk(&mut scene, feet(4, 8));
        let heard = frequency_heard(&mut scene, output, 12);
        if late {
            assert_eq!(heard, Some(2), "the landing, taken up before the step");
            assert_eq!(phase(&scene).1, 1);
        } else {
            assert_eq!(heard, Some(1), "the step, of the same game tick and closer");
            assert_eq!(phase(&scene).1, 8, "block distance 4");
        }
    }
}

/// Brief 8.11: a sensor that activates beside an amethyst block makes it resonate with
/// the vibration's frequency (`resonate_13` for a block placed), which another sensor
/// within 8 of the amethyst hears with that frequency, though the block placed is beyond
/// its own reach.
#[test]
fn an_amethyst_block_beside_a_sensor_relays_its_frequency_to_another() {
    let relay = BlockPos::new(11, -60, 4);
    let world = World::of(&[
        (SENSOR, sensor()),
        (BlockPos::new(5, -60, 4), blocks::AMETHYST_BLOCK),
        (relay, sensor()),
        (
            relay.offset(0, 0, -1),
            state("minecraft:comparator[facing=south,mode=compare,powered=false]"),
        ),
        (relay.offset(0, 0, -2), blocks::REDSTONE_WIRE),
    ]);
    let mut scene = Scene::alone(&world, holding(items::STONE), feet(6, 8));
    let placed = BlockPos::new(4, -60, 8);
    let output = scene.click(placed.offset(0, -1, 0), Face::Top);
    assert_eq!(scene.block(placed), blocks::STONE);
    assert!(
        (placed.x - relay.x).pow(2) + (placed.z - relay.z).pow(2) > 64,
        "the placing is beyond the relay's reach"
    );
    let (active, _) = until_active_at(&mut scene, relay, output, 20);
    assert!(active.is_some(), "the relay heard the resonance");
    let wire = relay.offset(0, 0, -2);
    let mut seen = Vec::new();
    assert!(
        scene.idle_until(6, &mut seen, |scene, _| {
            scene.block(wire).property("power") == Some("13")
        }),
        "with the frequency of the block placed"
    );
}

/// Brief 8.2 step 5 and 8.3 step 4: a sensor placed is a listener from then on, and
/// does not hear its own placing; the sensor beside hears it as a block placed (13).
#[test]
fn a_sensor_placed_hears_from_then_on_but_not_its_own_placing() {
    let placed = BlockPos::new(4, -60, 8);
    let mut scene = Scene::alone(
        &with_comparator(&[]),
        holding(items::SCULK_SENSOR),
        feet(6, 8),
    );
    let output = scene.click(placed.offset(0, -1, 0), Face::Top);
    assert_eq!(phase_at(&scene, placed), ("inactive", 0), "placed");
    assert_eq!(
        frequency_heard(&mut scene, output, 10),
        Some(13),
        "the old one hears it"
    );
    assert_eq!(
        phase_at(&scene, placed).0,
        "inactive",
        "not its own placing"
    );
    scene.idle_for(45);
    // A step 2 blocks south of the new sensor and 6 from the old, which is idle again.
    walk(&mut scene, feet(6, 10));
    let output = walk(&mut scene, feet(4, 10));
    let (active, _) = until_active_at(&mut scene, placed, output, 10);
    assert!(active.is_some(), "the new sensor hears");
}

/// Brief 8.6 step 4 and 8.9: a sensor holding a vibration in travel is saved with it
/// and its travel left; restored, it sends the particle again from the point it had
/// reached with the ticks left, and receives it when they run out.
#[test]
fn a_vibration_in_travel_survives_a_restore_and_its_particle_is_sent_again() {
    let (mut scene, output) = step_from(&with_comparator(&[]), feet(4, 14), feet(4, 12));
    assert_eq!(vibrations(&output), vec![([4.5, -60.0, 12.5], SENSOR, 8)]);
    // Taken up with 8 and lowered to 7 in that tick; 6 and 5 after two more.
    scene.idle_for(2);
    assert_eq!(phase(&scene).0, "inactive");
    let saved: Vec<(ChunkPos, clustine_world::Chunk)> = scene
        .region
        .loaded_chunks()
        .into_iter()
        .filter_map(|chunk| scene.region.saved_chunk(chunk).map(|saved| (chunk, saved)))
        .collect();
    let mut restored = clustine_sim::Region::restore(
        config(inventory(&[])),
        scene.region.state(),
        scene.layout.holdings(),
    );
    restored.tick(&clustine_sim::TickInputs {
        tickets_added: scene
            .layout
            .loaded
            .iter()
            .map(|chunk| (*chunk, clustine_sim::Ticket::Viewer))
            .collect(),
        ..clustine_sim::TickInputs::default()
    });
    let loading = restored.tick(&clustine_sim::TickInputs {
        chunks_loaded: saved,
        ..clustine_sim::TickInputs::default()
    });
    scene.region = restored;
    let again = vibrations(&loading);
    assert_eq!(again.len(), 1, "the particle sent again: {again:?}");
    let (from, to, ticks) = again[0];
    assert_eq!((to, ticks), (SENSOR, 5), "the ticks left");
    // lerp(1 - 5 / 8, the step, the sensor's centre).
    let reached = [4.5, -60.0 + 0.375 * 0.5, 12.5 - 0.375 * 8.0];
    for axis in 0..3 {
        assert!(
            (from[axis] - reached[axis]).abs() < 1e-6,
            "{from:?} against {reached:?}"
        );
    }
    let (active, _) = until_active(&mut scene, loading, 10);
    assert_eq!(active, Some(4), "received when the 5 ticks left run out");
    assert_eq!(phase(&scene), ("active", 1));
}

// ------------------------------------------------------------------------------------
// More of the player and the block (brief 8.0 item 4, 8.8, 8.10).

/// Brief 8.0 item 4: a creative player flying makes no steps at all.
#[test]
fn a_flying_players_moves_make_no_steps() {
    let mut scene = Scene::alone(&with_comparator(&[]), inventory(&[]), feet(4, 10));
    scene.input(PlayerInput::SetFlying { flying: true });
    let output = scene.input(PlayerInput::Move {
        position: Some(feet(4, 8)),
        rotation: None,
        on_ground: false,
    });
    assert_eq!(until_active(&mut scene, output, 12).0, None);
}

/// Brief 8.8: a waterlogged sensor activates as any, without the clicking.
#[test]
fn a_waterlogged_sensor_activates_without_its_clicking() {
    let world = World::of(&[(
        SENSOR,
        state("minecraft:sculk_sensor[sculk_sensor_phase=inactive,power=0,waterlogged=true]"),
    )]);
    let (mut scene, output) = step_from(&world, feet(4, 10), feet(4, 8));
    let (active, outputs) = until_active(&mut scene, output, 10);
    assert_eq!(active, Some(3));
    assert!(
        outputs
            .iter()
            .all(|output| sounds(output, sounds::BLOCK_SCULK_SENSOR_CLICKING).is_empty()),
        "no clicking under water"
    );
}

/// Brief 8.8: an active sensor's signal is its power on every side, and it strongly
/// powers the block below it, whose neighbours it updates too: dust beside the sensor
/// and dust beside the block under it read the power.
#[test]
fn an_active_sensor_powers_dust_beside_it_and_through_the_block_below() {
    let raised = BlockPos::new(4, -58, 4);
    let beside = BlockPos::new(3, -58, 4);
    let below_beside = BlockPos::new(5, -59, 4);
    let world = World::of(&[
        (BlockPos::new(4, -59, 4), blocks::STONE),
        (BlockPos::new(3, -59, 4), blocks::STONE),
        (BlockPos::new(5, -60, 4), blocks::STONE),
        (raised, sensor()),
        (beside, blocks::REDSTONE_WIRE),
        (below_beside, blocks::REDSTONE_WIRE),
    ]);
    // A step whose block is 4 south of the sensor's, 2 down: sqrt(20), power 15 - 8 = 7.
    let (mut scene, output) = step_from(&world, feet(4, 10), feet(4, 8));
    let (active, _) = until_active_at(&mut scene, raised, output, 10);
    assert!(active.is_some());
    let power = phase_at(&scene, raised).1;
    assert_eq!(power, power_for(20f64.sqrt(), 8.0));
    let read = |scene: &Scene, at: BlockPos| -> u8 {
        scene
            .block(at)
            .property("power")
            .expect("dust")
            .parse()
            .expect("a number")
    };
    assert_eq!(read(&scene, beside), power, "beside it");
    assert_eq!(
        read(&scene, below_beside),
        power,
        "beside the block under it"
    );
}

/// Brief 8.10: a calibrated sensor gives no signal to the side `getSignal` names by its
/// facing (`dir == facing`, the direction from the block asking to the sensor, so the
/// block at its back, opposite its facing; the brief's words "out of the side named by
/// `facing`" read otherwise, and this test follows the formula): dust at its back reads
/// nothing, dust at its front its power.
#[test]
fn a_calibrated_sensor_gives_no_signal_to_its_back() {
    let front = BlockPos::new(4, -60, 3);
    let back = BlockPos::new(4, -60, 5);
    let world = World::of(&[
        (
            CALIBRATED,
            state(
                "minecraft:calibrated_sculk_sensor[facing=north,sculk_sensor_phase=inactive,power=0]",
            ),
        ),
        (front, blocks::REDSTONE_WIRE),
        (back, blocks::REDSTONE_WIRE),
    ]);
    let mut scene = Scene::alone(&world, inventory(&[]), feet(9, 4));
    let output = walk(&mut scene, feet(7, 4));
    let (active, _) = until_active_at(&mut scene, CALIBRATED, output, 10);
    assert!(active.is_some(), "heard");
    let power = phase_at(&scene, CALIBRATED).1;
    assert!(power > 0);
    let read = |scene: &Scene, at: BlockPos| -> u8 {
        scene
            .block(at)
            .property("power")
            .expect("dust")
            .parse()
            .expect("a number")
    };
    assert_eq!(read(&scene, front), power, "its front");
    assert_eq!(read(&scene, back), 0, "its back");
}

/// Brief 4.0 item 3 and 8.1: an empty dispenser that fires posts `block_activate` (10)
/// with its failure's click; an empty dropper clicks only. Each stands 8 blocks south of
/// the sensor, and the redstone block placed beyond it that fires it is out of the
/// sensor's reach.
#[test]
fn an_empty_dispenser_firing_is_heard_and_an_empty_dropper_is_not() {
    let at = BlockPos::new(4, -60, 12);
    let power = BlockPos::new(4, -60, 13);
    for (block, heard) in [
        (
            "minecraft:dispenser[facing=north,triggered=false]",
            Some(10),
        ),
        ("minecraft:dropper[facing=north,triggered=false]", None),
    ] {
        let world = with_comparator(&[(at, state(block))]);
        let mut scene = Scene::alone(&world, holding(items::REDSTONE_BLOCK), feet(6, 14));
        let output = scene.click(power.offset(0, -1, 0), Face::Top);
        assert_eq!(scene.block(power), blocks::REDSTONE_BLOCK);
        assert_eq!(frequency_heard(&mut scene, output, 12), heard, "{block}");
    }
}

/// Brief 6.0 item 9 and 8.1: a plate pressed posts `block_activate` (10) at its
/// centre; an item falling onto a wooden plate 4 blocks from the sensor is heard as the
/// plate, nearer to the sensor than the item's landing.
#[test]
fn a_plate_pressed_by_an_item_is_heard_as_the_plate() {
    let plate = BlockPos::new(4, -60, 8);
    let mut falling = dirt(1, [4.5, -57.0, 8.5]);
    falling.on_ground = false;
    let world = with_comparator(&[(plate, state("minecraft:oak_pressure_plate[powered=false]"))])
        .with_entities(&[falling]);
    let mut scene = Scene::alone(&world, inventory(&[]), feet(0, 0));
    let output = scene.idle();
    assert_eq!(frequency_heard(&mut scene, output, 40), Some(10));
    assert_eq!(scene.block(plate).property("powered"), Some("true"));
}

/// Brief 8.1 ("Posted by N6's classes"), 4.5, 4.6, 4.10, 4.11, 4.14 and 4.15: what a
/// dispenser does is heard with its game event's frequency: a water bucket emptied
/// (`fluid_place`, 13), an empty bucket filled (`fluid_pickup`, 12), fire lit
/// (`block_place`, 13), TNT primed and an armour stand placed (`entity_place`, 14), a
/// shulker box placed (`block_place`, 13) and a glass bottle filled (`fluid_pickup` at
/// the dispenser, 12). The dispenser stands 8 blocks south of the sensor facing it, and
/// is fired by a redstone block placed beyond the sensor's reach.
#[test]
fn what_a_dispenser_does_is_heard_with_its_frequency() {
    let at = BlockPos::new(4, -60, 12);
    let front = BlockPos::new(4, -60, 11);
    let power = BlockPos::new(4, -60, 13);
    let mut unheard = Vec::new();
    for (what, item, more, frequency) in [
        ("a water bucket emptied", items::WATER_BUCKET, vec![], 13),
        (
            "a bucket filled",
            items::BUCKET,
            vec![(front, blocks::WATER)],
            12,
        ),
        ("fire lit", items::FLINT_AND_STEEL, vec![], 13),
        ("TNT primed", items::TNT, vec![], 14),
        ("an armour stand placed", items::ARMOR_STAND, vec![], 14),
        ("a shulker box placed", items::SHULKER_BOX, vec![], 13),
        (
            "a bottle filled",
            items::GLASS_BOTTLE,
            vec![(front, blocks::WATER)],
            12,
        ),
    ] {
        let mut blocks = vec![(
            at,
            state("minecraft:dispenser[facing=north,triggered=false]"),
        )];
        blocks.extend(more);
        let held = inventory(&[(0, stack(item, 1)), (1, stack(items::REDSTONE_BLOCK, 64))]);
        let mut scene = Scene::alone(&with_comparator(&blocks), held, feet(6, 14));
        let (serial, _) = scene.open(at);
        scene.menu_click(
            serial,
            i16::try_from(9 + 27).expect("fits"),
            0,
            clustine_sim::api::ClickKind::QuickMove,
        );
        scene.close(serial);
        scene.select(1);
        let output = scene.click(power.offset(0, -1, 0), Face::Top);
        assert_eq!(scene.block(power), blocks::REDSTONE_BLOCK);
        let mut outputs = vec![output];
        outputs.extend(scene.idle_for(4));
        assert!(
            outputs
                .iter()
                .any(|output| level_events(output).contains(&(1000, at, 0))),
            "{what}: it fired"
        );
        let output = outputs.pop().expect("some");
        let heard = frequency_heard(&mut scene, output, 15);
        if heard != Some(frequency) {
            unheard.push((what, heard));
        }
    }
    assert!(unheard.is_empty(), "{unheard:?}");
}

/// Brief 8.1 (`fluid_place` by `BucketItem.playEmptySound`, `fluid_pickup` by
/// `BucketItem.use`): a player's water bucket emptied on the floor 4 blocks from the
/// sensor is heard as 13, and the water taken up again with an empty bucket as 12.
#[test]
fn a_players_bucket_emptied_and_filled_is_heard() {
    let at = BlockPos::new(4, -60, 8);
    for (item, more, expected, frequency) in [
        (items::WATER_BUCKET, vec![], blocks::WATER, 13),
        (items::BUCKET, vec![(at, blocks::WATER)], blocks::AIR, 12),
    ] {
        let mut scene = Scene::alone(&with_comparator(&more), holding(item), feet(4, 10));
        scene.actors[0].sequence += 1;
        let sequence = scene.actors[0].sequence;
        // Looking north and down, at the floor's top about 1.9 blocks ahead.
        let output = scene.input(PlayerInput::UseItem {
            rotation: (180.0, 40.0),
            sneaking: false,
            sequence,
        });
        assert_eq!(scene.block(at), expected, "the bucket of {item} was used");
        assert_eq!(
            frequency_heard(&mut scene, output, 10),
            Some(frequency),
            "{item}"
        );
    }
}
