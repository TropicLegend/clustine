//! Tests of the eye of ender from the game's rules (`docs/groundwork/p-brief.md`,
//! section 4.3; d-javap `EnderEyeItem`, `EyeOfEnder`, `EndPortalFrameBlock`): a frame
//! filled, what stands on it lifted and the comparator behind it told; a ring that
//! opens and one that does not; an eye thrown towards the nearest of a list of
//! strongholds, its first steering worked out by hand, its 80 ticks, its end with its
//! item and without; and no eye in a world without strongholds or aimed at a frame. The
//! worlds are those of the tests of TNT (`tnt/tests.rs`): a stone floor at y = -61.

use clustine_data::{BlockState, blocks, entity_types, items, sounds};
use clustine_world::{
    BlockPos, ChunkPos, Double, EdgeId, Entity, EntityData, EntityId, EyeOfEnder, ItemStack,
    PlayerId,
};

use super::super::tnt::tests::{FLOOR, add, add_player, at, idle, record, records, state, world};
use super::{nearest_stronghold, signal_to, steer};
use crate::api::{
    Face as ClickFace, GlobalLevelEvent, PlayerInput, RegionEvent, TickInputs, TickOutput,
};
use crate::region::Region;
use crate::region::random_ticks::Random;

/// The y the tests' frames stand at, on the floor.
const Y: i32 = FLOOR + 1;

/// The south-west inner corner of the tests' rings: the inside is x 2 to 4, z 2 to 4.
const LOW: BlockPos = BlockPos::new(2, Y, 2);

fn frame(facing: &str, eye: bool) -> BlockState {
    state(&format!(
        "minecraft:end_portal_frame[eye={eye},facing={facing}]"
    ))
}

/// The twelve frames of a stronghold's ring around the inside from [`LOW`], each facing
/// inwards, with their eyes where `eye` says.
fn ring(eye: impl Fn(BlockPos) -> bool) -> Vec<(BlockPos, BlockState)> {
    let mut frames = Vec::new();
    for i in 0..3 {
        frames.push((LOW.offset(i, 0, -1), "south"));
        frames.push((LOW.offset(i, 0, 3), "north"));
        frames.push((LOW.offset(-1, 0, i), "east"));
        frames.push((LOW.offset(3, 0, i), "west"));
    }
    frames
        .into_iter()
        .map(|(pos, facing)| (pos, frame(facing, eye(pos))))
        .collect()
}

/// The inner 3 × 3 of the ring.
fn inside() -> Vec<BlockPos> {
    (0..3)
        .flat_map(|i| (0..3).map(move |j| LOW.offset(i, 0, j)))
        .collect()
}

/// A world with `blocks` and player 1 at `feet` holding an eye of ender in slot 0.
fn world_with_eye(
    blocks: impl Fn(BlockPos) -> Option<BlockState>,
    feet: [f64; 3],
) -> (Region, PlayerId, EntityId) {
    let mut region = world(5, blocks);
    let (id, entity) = add_player(&mut region, 1, feet, false);
    region
        .players
        .get_mut(&id)
        .expect("the player")
        .inventory
        .set(0, Some(ItemStack::new(items::ENDER_EYE, 1)));
    (region, id, entity)
}

/// Player 1's input `input`, numbered `number`.
fn input(id: PlayerId, entity: EntityId, number: u64, input: PlayerInput) -> TickInputs {
    TickInputs {
        inputs: vec![(EdgeId(1), id, entity, number, input)],
        ..TickInputs::default()
    }
}

/// A click with the eye on the top of `pos`.
fn use_on(pos: BlockPos) -> PlayerInput {
    PlayerInput::UseItemOn {
        position: pos,
        face: ClickFace::Top,
        cursor: [0.5, 0.8125, 0.5],
        sneaking: false,
        sequence: 1,
    }
}

/// A use of the eye in the air, looking along `yaw` and `pitch`.
fn use_in_air(yaw: f32, pitch: f32) -> PlayerInput {
    PlayerInput::UseItem {
        rotation: (yaw, pitch),
        sneaking: false,
        sequence: 1,
    }
}

fn level_events(output: &TickOutput) -> Vec<(i32, BlockPos)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::LevelEvent { kind, position, .. } => Some((*kind, *position)),
            _ => None,
        })
        .collect()
}

fn heard(output: &TickOutput, wanted: i32) -> Vec<(clustine_world::Vec3, f32, i32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound,
                position,
                pitch,
                source,
                ..
            } if *sound == wanted => Some((*position, *pitch, *source)),
            _ => None,
        })
        .collect()
}

fn global_events(output: &TickOutput) -> Vec<GlobalLevelEvent> {
    output
        .village
        .as_ref()
        .map(|report| report.global_events.clone())
        .unwrap_or_default()
}

fn eyes(region: &Region) -> Vec<Entity> {
    records(region)
        .into_iter()
        .filter(|entity| matches!(entity.data, EntityData::EyeOfEnder(_)))
        .collect()
}

fn items_of(region: &Region, item: i32) -> Vec<Entity> {
    records(region)
        .into_iter()
        .filter(
            |entity| matches!(&entity.data, EntityData::Item { stack, .. } if stack.item == item),
        )
        .collect()
}

fn slot_0(region: &Region, id: PlayerId) -> Option<ItemStack> {
    region.players[&id].inventory.0[0].clone()
}

/// `useOn` of a frame without its eye: the eye set (seen, with level event 1503 there),
/// a creative player's eye kept, an item lying on the frame and the player standing on
/// it lifted onto the eye's top, and the comparator behind the frame told, so that it
/// lights within its delay. A second click on the frame, which has its eye, does
/// nothing.
#[test]
fn an_eye_fills_a_frame_lifts_what_stands_on_it_and_tells_the_comparator() {
    let pos = at(2, Y, 2);
    let comparator = at(2, Y, 3);
    let blocks = move |at: BlockPos| {
        if at == pos {
            Some(frame("north", false))
        } else if at == comparator {
            // It reads the block towards its facing, the frame.
            Some(state("minecraft:comparator[facing=north]"))
        } else {
            None
        }
    };
    let top = f64::from(Y) + 0.8125;
    let (mut region, id, entity) = world_with_eye(blocks, [2.5, top, 2.5]);
    let item = record(
        &mut region,
        entity_types::ITEM,
        [2.6, top, 2.4],
        EntityData::Item {
            stack: ItemStack::new(items::STICK, 1),
            // Not to be picked up by the player who stands beside it.
            pick_up_delay: 40,
            thrower: None,
            health: 5,
            age_offset: 0,
        },
    );
    let item = add(&mut region, item);
    let output = region.tick(&input(id, entity, 1, use_on(pos)));
    assert_eq!(region.block(pos), Some(frame("north", true)));
    assert!(output.events.iter().any(|event| matches!(event,
        RegionEvent::BlockChanged { position, state, seen: true }
            if *position == pos && *state == frame("north", true))));
    assert_eq!(level_events(&output), [(1503, pos)]);
    assert!(global_events(&output).is_empty());
    assert_eq!(
        slot_0(&region, id),
        Some(ItemStack::new(items::ENDER_EYE, 1))
    );
    // Lifted to stand on the eye, a block above the frame's feet.
    let player = &region.players[&id];
    assert_eq!(player.pose.position.y, f64::from(Y) + 1.0);
    assert!(output.player_events.iter().any(|(who, event)| *who == id
        && matches!(event, crate::api::PlayerEvent::Teleported { pose }
            if pose.position.y == f64::from(Y) + 1.0)));
    let lifted = region.find_entity(item).expect("the item").1.position;
    assert_eq!(lifted[1], f64::from(Y) + 1.0, "{lifted:?}");
    // The comparator lights within its delay of two ticks.
    let mut lit = false;
    for _ in 0..4 {
        if region.block(comparator)
            == Some(state("minecraft:comparator[facing=north,powered=true]"))
        {
            lit = true;
            break;
        }
        idle(&mut region);
    }
    assert!(lit, "{:?}", region.block(comparator));
    // Again on the frame that has its eye: nothing.
    let output = region.tick(&input(id, entity, 2, use_on(pos)));
    assert!(level_events(&output).is_empty());
    assert!(eyes(&region).is_empty());
}

/// The last eye of a ring whose frames all face inwards opens it: each block of the
/// inside destroyed with its drops (the dirt dropped), then `end_portal`; and the
/// opening is reported once, at the inside's centre, as a global level event of the
/// overworld.
#[test]
fn the_last_eye_of_a_ring_makes_the_portal_and_reports_its_opening() {
    let last = LOW.offset(1, 0, 3);
    let frames = ring(move |pos| pos != last);
    let dirt = LOW.offset(2, 0, 0);
    let blocks = move |at: BlockPos| {
        if at == dirt {
            return Some(blocks::DIRT);
        }
        frames
            .iter()
            .find(|(pos, _)| *pos == at)
            .map(|(_, state)| *state)
    };
    let (mut region, id, entity) = world_with_eye(blocks, [3.5, f64::from(Y), 7.5]);
    let output = region.tick(&input(id, entity, 1, use_on(last)));
    assert_eq!(region.block(last), Some(frame("north", true)));
    for cell in inside() {
        assert_eq!(region.block(cell), Some(blocks::END_PORTAL), "{cell:?}");
    }
    assert_eq!(items_of(&region, items::DIRT).len(), 1, "the dirt's drop");
    assert_eq!(
        global_events(&output),
        [GlobalLevelEvent {
            event: 1038,
            dimension: clustine_world::Dimension::Overworld,
            position: LOW.offset(1, 0, 1),
            data: 0,
        }]
    );
    assert_eq!(level_events(&output)[0], (1503, last));
}

/// A ring with one frame turned outwards stays shut when its last eye goes in: no
/// portal, no global event.
#[test]
fn a_ring_with_a_frame_turned_the_wrong_way_does_not_open() {
    let last = LOW.offset(1, 0, 3);
    let turned = LOW.offset(-1, 0, 1);
    let frames = ring(move |pos| pos != last);
    let blocks = move |at: BlockPos| {
        if at == turned {
            return Some(frame("west", true));
        }
        frames
            .iter()
            .find(|(pos, _)| *pos == at)
            .map(|(_, state)| *state)
    };
    let (mut region, id, entity) = world_with_eye(blocks, [3.5, f64::from(Y), 7.5]);
    let output = region.tick(&input(id, entity, 1, use_on(last)));
    assert_eq!(region.block(last), Some(frame("north", true)));
    for cell in inside() {
        assert_eq!(region.block(cell), Some(blocks::AIR), "{cell:?}");
    }
    assert!(global_events(&output).is_empty());
}

/// `findNearestMapStructure` among rings: by the square of the distance from the
/// player's block to each chunk's (8, 32, 8), the first of equals; the chunk's lowest
/// corner at y 0; none of none.
#[test]
fn the_nearest_stronghold_is_the_first_of_the_nearest_ring_chunks() {
    let from = BlockPos::new(0, -60, 0);
    let list = [
        ChunkPos::new(100, 0),
        ChunkPos::new(2, 2),
        ChunkPos::new(-3, 2),
        ChunkPos::new(2, -4),
    ];
    // (40, 32, 40) and (-40, 32, 40) are as far; (40, 32, -56) further.
    assert_eq!(
        nearest_stronghold(&list, from),
        Some(BlockPos::new(32, 0, 32))
    );
    assert_eq!(
        nearest_stronghold(&list[2..], from),
        Some(BlockPos::new(-48, 0, 32))
    );
    // Height counts: from far below, the y of 32 matters as much as across.
    assert_eq!(
        nearest_stronghold(
            &[ChunkPos::new(0, 3), ChunkPos::new(1, 0)],
            BlockPos::new(8, 32, 8)
        ),
        Some(BlockPos::new(16, 0, 0))
    );
    assert_eq!(nearest_stronghold(&[], from), None);
}

/// An eye used in the air flies at the nearest stronghold of the list: made at the
/// player's x and z and half their height, its target 12 towards it and 8 above, its
/// survival the fifth draw of its stream after its UUID's two, the launch heard at the
/// player's feet with a pitch from 0.33 to 0.5, the creative player's eye kept. In its
/// first tick it does not move (its motion was none) and takes its first steer, worked
/// out by hand: 0.0025 of the 12 across, along the line to the target, and 0.015 up.
#[test]
fn an_eye_is_thrown_towards_the_nearest_stronghold_and_steers_by_the_game_s_rule() {
    let (mut region, id, entity) = world_with_eye(|_| None, [0.5, f64::from(Y), 0.5]);
    region.config.strongholds = vec![
        ChunkPos::new(100, 0),
        ChunkPos::new(2, 2),
        ChunkPos::new(-3, 2),
    ];
    let tick = region.tick_number() + 1;
    let output = region.tick(&input(id, entity, 1, use_in_air(0.0, 0.0)));
    let [eye] = &eyes(&region)[..] else {
        panic!("one eye: {:?}", records(&region));
    };
    let EntityData::EyeOfEnder(data) = &eye.data else {
        unreachable!()
    };
    // `getY(0.5)`: half the box's height, a float.
    let at = [0.5, f64::from(Y) + f64::from(1.8_f32) * 0.5, 0.5];
    assert_eq!(eye.position, at);
    assert_eq!(eye.kind, entity_types::EYE_OF_ENDER as u16);
    assert_eq!(data.item, ItemStack::new(items::ENDER_EYE, 1));
    // 31.5 each way across to (32, 0, 32): 12 along the diagonal, 8 up.
    let along = 12.0 / 2.0_f64.sqrt();
    let target = [0.5 + along, at[1] + 8.0, 0.5 + along];
    for (got, wanted) in data.target.iter().zip(target) {
        assert!((got.0 - wanted).abs() < 1e-9, "{:?}", data.target);
    }
    let mut stream = Random::of_entity_aside(region.seed(), eye.key, 0, tick);
    stream.next_u64();
    stream.next_u64();
    assert_eq!(data.survives, stream.next_int(5) > 0);
    // Its first tick: not moved, and steered.
    assert_eq!(eye.lived, 1);
    let speed = 0.0025 * 12.0;
    let wanted = [speed / 2.0_f64.sqrt(), 0.015, speed / 2.0_f64.sqrt()];
    for (got, wanted) in eye.motion.iter().zip(wanted) {
        assert!((got - wanted).abs() < 1e-9, "{:?}", eye.motion);
    }
    let launch = heard(&output, sounds::ENTITY_ENDER_EYE_LAUNCH);
    let [(position, pitch, source)] = launch[..] else {
        panic!("{launch:?}");
    };
    assert_eq!(position, clustine_world::Vec3::new(0.5, f64::from(Y), 0.5));
    assert!((0.33..0.5).contains(&pitch), "{pitch}");
    assert_eq!(source, crate::physics::item::SOUND_SOURCE_NEUTRAL);
    assert_eq!(
        slot_0(&region, id),
        Some(ItemStack::new(items::ENDER_EYE, 1))
    );
    // The swing of `SUCCESS_SERVER`, the thrower's own client told too.
    assert!(output.events.iter().any(|event| matches!(event,
        RegionEvent::ArmSwung { entity: who, own: true, off_hand: false, .. } if *who == entity)));
}

/// The rule of `updateDeltaMovement` beside the tick: within a block across the speed
/// and the rise are slowed to 0.8 first, and below the target it falls.
#[test]
fn the_steer_slows_within_a_block_and_falls_from_above_its_target() {
    let motion = steer([0.1, 0.2, 0.0], [5.0, 70.0, 5.0], [5.5, 60.0, 5.0]);
    let wanted_speed = (0.1 + 0.0025 * (0.5 - 0.1)) * 0.8;
    let rise = 0.2 * 0.8;
    assert!((motion[0] - wanted_speed).abs() < 1e-12, "{motion:?}");
    assert!((motion[1] - (rise + (-1.0 - rise) * 0.015)).abs() < 1e-12);
    assert_eq!(motion[2], 0.0);
    // A point within 12 across is the target itself.
    assert_eq!(
        signal_to([0.0, 0.0, 0.0], [3.0, -5.0, 4.0]),
        [3.0, -5.0, 4.0]
    );
}

/// The eye flies 80 ticks: in the tick of its making it ticks its first, and it is gone
/// in the tick its 81st would be, not before.
#[test]
fn a_thrown_eye_lives_eighty_ticks() {
    let (mut region, id, entity) = world_with_eye(|_| None, [0.5, f64::from(Y), 0.5]);
    region.config.strongholds = vec![ChunkPos::new(-3, 2)];
    region.tick(&input(id, entity, 1, use_in_air(0.0, 0.0)));
    let key = eyes(&region)[0].key;
    let mut ticks = 1;
    while region.find_entity(key).is_some() {
        assert!(ticks <= 81, "the eye outlived its life");
        let output = idle(&mut region);
        ticks += 1;
        if region.find_entity(key).is_some() {
            assert!(heard(&output, sounds::ENTITY_ENDER_EYE_DEATH).is_empty());
        } else {
            assert_eq!(heard(&output, sounds::ENTITY_ENDER_EYE_DEATH).len(), 1);
        }
    }
    assert_eq!(ticks, 81);
}

/// An eye in its last tick, surviving or not, at `position` moving by `motion`.
fn last_tick_eye(region: &mut Region, survives: bool) -> (EntityId, [f64; 3]) {
    let position = [4.25, f64::from(Y) + 3.5, 4.75];
    let motion = [0.25, 0.125, -0.5];
    let mut eye = record(
        region,
        entity_types::EYE_OF_ENDER,
        position,
        EntityData::EyeOfEnder(Box::new(EyeOfEnder {
            item: ItemStack::new(items::ENDER_EYE, 1),
            target: [Double(20.0), Double(0.0), Double(20.0)],
            survives,
        })),
    );
    eye.motion = motion;
    eye.lived = 80;
    let moved = [
        position[0] + motion[0],
        position[1] + motion[1],
        position[2] + motion[2],
    ];
    (add(region, eye), moved)
}

/// At its end the eye plays `ender_eye_death` where its last move took it and is gone;
/// one that survives drops its eye there, which may be picked up at once.
#[test]
fn a_surviving_eye_drops_its_item_where_it_ends() {
    let mut region = world(5, |_| None);
    let (key, moved) = last_tick_eye(&mut region, true);
    let output = idle(&mut region);
    assert!(region.find_entity(key).is_none());
    let death = heard(&output, sounds::ENTITY_ENDER_EYE_DEATH);
    assert_eq!(death.len(), 1);
    assert_eq!(
        death[0].0,
        clustine_world::Vec3::new(moved[0], moved[1], moved[2])
    );
    let dropped = items_of(&region, items::ENDER_EYE);
    let [item] = &dropped[..] else {
        panic!("{dropped:?}");
    };
    // Made where the eye ended, in the entities' phase, so not yet ticked.
    assert_eq!(item.position, moved);
    assert!(matches!(
        item.data,
        EntityData::Item {
            pick_up_delay: 0,
            ..
        }
    ));
    assert!(level_events(&output).is_empty());
}

/// One that does not survive shatters: level event 2003 at the block it ended in, and
/// nothing dropped.
#[test]
fn an_eye_that_does_not_survive_shatters_with_level_event_2003() {
    let mut region = world(5, |_| None);
    let (key, moved) = last_tick_eye(&mut region, false);
    let output = idle(&mut region);
    assert!(region.find_entity(key).is_none());
    assert_eq!(heard(&output, sounds::ENTITY_ENDER_EYE_DEATH).len(), 1);
    assert_eq!(
        level_events(&output),
        [(
            2003,
            BlockPos::new(
                moved[0].floor() as i32,
                moved[1].floor() as i32,
                moved[2].floor() as i32
            )
        )]
    );
    assert!(items_of(&region, items::ENDER_EYE).is_empty());
}

/// The flat world has no strongholds: an eye used in the air there makes nothing and
/// plays nothing, and is kept.
#[test]
fn no_eye_is_thrown_in_a_world_without_strongholds() {
    let (mut region, id, entity) = world_with_eye(|_| None, [0.5, f64::from(Y), 0.5]);
    assert!(region.config.strongholds.is_empty());
    let output = region.tick(&input(id, entity, 1, use_in_air(0.0, 0.0)));
    assert!(eyes(&region).is_empty());
    assert!(heard(&output, sounds::ENTITY_ENDER_EYE_LAUNCH).is_empty());
    assert_eq!(
        slot_0(&region, id),
        Some(ItemStack::new(items::ENDER_EYE, 1))
    );
}

/// Aimed at a frame (here one that has its eye, which the client's click on passes to
/// the use in the air), the use passes: no eye flies, though there are strongholds.
#[test]
fn an_eye_aimed_at_a_frame_is_not_thrown() {
    let pos = at(2, Y, 2);
    let blocks = move |at: BlockPos| (at == pos).then(|| frame("north", true));
    let (mut region, id, entity) = world_with_eye(blocks, [2.5, f64::from(Y) + 1.0, 2.5]);
    region.config.strongholds = vec![ChunkPos::new(-3, 2)];
    let output = region.tick(&input(id, entity, 1, use_in_air(0.0, 90.0)));
    assert!(eyes(&region).is_empty());
    assert!(heard(&output, sounds::ENTITY_ENDER_EYE_LAUNCH).is_empty());
    // Looking up instead, it flies.
    region.tick(&input(id, entity, 2, use_in_air(0.0, -90.0)));
    assert_eq!(eyes(&region).len(), 1);
}
