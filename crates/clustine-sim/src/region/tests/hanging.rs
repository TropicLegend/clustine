//! Tests of item frames, glow item frames and paintings (step N5c,
//! `docs/groundwork/n5-plan.md`; `docs/groundwork/n5-brief.md`, section 6 and findings
//! 26, 31 to 35): hung on each face where the game hangs them and refused where it
//! refuses, a painting's variant drawn as the game draws it from the new painting's
//! stream, the frame's item put in, turned, taken out by a first hit and the frame broken
//! by a second, the drops in the game's order, a comparator reading a frame through a
//! block, a piston popping a frame, a wall that goes, a hit from another region, and a
//! pick of an entity's item. The numbers each test expects are worked out here from the
//! game's formulas, as its bytecode has them.
//!
//! The regions are [`ticking_with`]'s: chunks within two of (0, 0) loaded on a stone
//! floor at y = -61, the nine within one ticking, and player 1 at the spawn, (0.5, -60,
//! 0.5), creative as every player is.

use clustine_data::{Face, entity_types, items, sounds as sound};
use clustine_world::{
    Attached, Double, Due, Effect, EffectId, Effected, Entity, EntityChange, EntityData, EntityUse,
    FiledEffect, Float,
};

use super::arrivals::{WEST_OF_THE_LINE, at_a_line};
use super::r7::use_on;
use super::redstone::{api_face, block_at, holding, sounds, state, ticking_with};
use super::*;
use crate::api::Tracked;
use crate::region::attached::Breaker;
use crate::region::random_ticks::Random;

const EAST: ChunkPos = ChunkPos::new(1, 0);

/// The block every frame of these tests hangs on, in the air above the floor.
const WALL: BlockPos = BlockPos::new(3, -58, 3);

fn key(n: u64) -> EntityId {
    EntityId((1 << 40) + n)
}

/// The frame or painting records of the loaded chunks.
fn hanging_records(region: &Region) -> Vec<Entity> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter(|entity| {
            matches!(
                entity.data,
                EntityData::ItemFrame { .. } | EntityData::Painting { .. }
            )
        })
        .cloned()
        .collect()
}

fn record(region: &Region, key: EntityId) -> Option<Entity> {
    region.find_entity(key).map(|(_, entity)| entity.clone())
}

/// The items of the loaded chunks, in ascending key: each one's key, place and stack.
fn items_lying(region: &Region) -> Vec<(EntityId, [f64; 3], ItemStack)> {
    let mut lying: Vec<(EntityId, [f64; 3], ItemStack)> = region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter_map(|entity| match entity.data {
            EntityData::Item { stack, .. } => Some((entity.key, entity.position, stack)),
            _ => None,
        })
        .collect();
    lying.sort_by_key(|(key, ..)| *key);
    lying
}

fn sound_ids(output: &TickOutput) -> Vec<i32> {
    sounds(output).into_iter().map(|(id, ..)| id).collect()
}

/// Lays `entity` in its loaded chunk, as a load would have it.
fn lay(region: &mut Region, entity: &Entity) {
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .unwrap()
        .apply_entity(&EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

/// A frame record on `face` of [`WALL`], holding `item` turned `rotation`, settled as
/// the region settles it.
fn frame(n: u64, face: Face, item: Option<ItemStack>, rotation: u8, fixed: bool) -> Entity {
    let block = relative(WALL, face);
    frame_at(n, block, face as u8, item, rotation, fixed)
}

fn frame_at(
    n: u64,
    block: BlockPos,
    direction: u8,
    item: Option<ItemStack>,
    rotation: u8,
    fixed: bool,
) -> Entity {
    let data = EntityData::ItemFrame {
        attached: Attached { block, direction },
        item,
        rotation,
        drop_chance: Float(1.0),
        fixed,
        invisible: false,
    };
    let mut entity = Entity {
        key: key(n),
        kind: entity_types::ITEM_FRAME as u16,
        uuid: uuid::Uuid::from_u128(u128::from(n)),
        position: [0.0; 3],
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        age: 1,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data,
    };
    let bounds = region_box(&entity);
    entity.position = bounds.center();
    entity
}

/// A frame's or painting's box, as the region works it out.
fn region_box(entity: &Entity) -> crate::physics::aabb::Aabb {
    let region = ticking_with(&[]);
    region.entity_box(entity)
}

fn relative(pos: BlockPos, face: Face) -> BlockPos {
    crate::region::updates::relative(pos, face)
}

const APPLE: ItemStack = ItemStack {
    item: items::APPLE,
    count: 5,
};

/// One apple, as a frame holds it.
const ONE_APPLE: ItemStack = ItemStack {
    item: items::APPLE,
    count: 1,
};

/// Player 1's hotbar emptied, the first slot selected.
fn empty_handed() -> TickInputs {
    let mut inputs: Vec<Input> = (0..9).map(|slot| set_slot(1, slot, None)).collect();
    inputs.push(select(1, 0));
    moves(inputs)
}

/// Where the items `output`'s tick made were born, in the order made.
fn born(output: &TickOutput) -> Vec<(i32, [f64; 3])> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntitySpawned(state) => match &state.kind {
                crate::api::EntityKind::Other {
                    data: EntityData::Item { stack, .. },
                    ..
                } => Some((
                    stack.item,
                    [
                        state.pose.position.x,
                        state.pose.position.y,
                        state.pose.position.z,
                    ],
                )),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// Player 1 uses the entity `target` with what is in hand.
fn interact(target: &Entity) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::Interact {
            entity: target.key,
            location: [0.0; 3],
            sneaking: false,
        },
    )])
}

/// Player 1 hits the entity `target`.
fn attack(target: &Entity) -> TickInputs {
    moves(vec![numbered(
        1,
        PlayerInput::Attack { entity: target.key },
    )])
}

/// A hit on `target` from another region's player, creative or not, as an effect at
/// its block.
fn hit_from_afar(target: &Entity, sequence: u64, creative: bool) -> TickInputs {
    TickInputs {
        effects: vec![FiledEffect {
            id: EffectId { sequence, index: 0 },
            effect: Effect {
                at: crate::physics::containing(target.position),
                what: Effected::OnEntity {
                    key: target.key,
                    by: EntityUse::Attack {
                        attacker: EntityId(1),
                        creative,
                        sprinting: false,
                        damage: Float(1.0),
                        from: [Double(0.5), Double(-60.0), Double(0.5)],
                    },
                },
            },
            due: Due::FromTaking,
        }],
        ..TickInputs::default()
    }
}

/// A use of `target` from another region's player holding `held`, as an effect.
fn used_from_afar(target: &Entity, sequence: u64, held: Option<ItemStack>) -> TickInputs {
    TickInputs {
        effects: vec![FiledEffect {
            id: EffectId { sequence, index: 0 },
            effect: Effect {
                at: crate::physics::containing(target.position),
                what: Effected::OnEntity {
                    key: target.key,
                    by: EntityUse::Interact {
                        player: EntityId(1),
                        creative: true,
                        held,
                        location: [Double(0.0); 3],
                        sneaking: false,
                        from: [Double(0.5), Double(-60.0), Double(0.5)],
                    },
                },
            },
            due: Due::FromTaking,
        }],
        ..TickInputs::default()
    }
}

fn frame_item_of(entity: &Entity) -> (Option<ItemStack>, u8) {
    match entity.data {
        EntityData::ItemFrame { item, rotation, .. } => (item, rotation),
        _ => panic!("not a frame: {entity:?}"),
    }
}

// Placing.

#[test]
fn a_frame_is_hung_on_each_of_the_six_faces_facing_out_of_it_and_the_item_is_kept() {
    let mut region = ticking_with(&[(WALL, blocks::STONE)]);
    checked(&mut region, &holding(items::ITEM_FRAME));
    for face in Face::ALL {
        let output = checked(&mut region, &use_on(WALL, api_face(face)));
        assert_eq!(
            sound_ids(&output),
            [sound::ENTITY_ITEM_FRAME_PLACE],
            "{face:?}"
        );
    }
    let mut frames = hanging_records(&region);
    frames.sort_by_key(|entity| entity.key);
    assert_eq!(frames.len(), 6);
    for (frame, face) in frames.iter().zip(Face::ALL) {
        let EntityData::ItemFrame {
            attached,
            item,
            rotation,
            fixed,
            ..
        } = frame.data
        else {
            panic!("{frame:?}");
        };
        assert_eq!(attached.block, relative(WALL, face));
        assert_eq!(attached.direction, face as u8);
        assert_eq!((item, rotation, fixed), (None, 0, false));
        assert_eq!(i32::from(frame.kind), entity_types::ITEM_FRAME);
        // `setDirection`: on a wall a quarter turn per 2D value, on a floor or a
        // ceiling a pitch of -90 or 90.
        let expected = match face {
            Face::Up => (0.0, -90.0),
            Face::Down => (0.0, 90.0),
            Face::South => (0.0, 0.0),
            Face::West => (90.0, 0.0),
            Face::North => (180.0, 0.0),
            Face::East => (270.0, 0.0),
        };
        assert_eq!((frame.yaw, frame.pitch), expected, "{face:?}");
    }
    // The north one: 0.46875 from its block's centre towards the wall, 0.75 by 0.75.
    let north = &frames[2];
    assert_eq!(north.position, [3.5, -57.5, 2.968_75]);
    // A creative player keeps the item.
    let player = &region.players[&player(1)];
    assert_eq!(
        player.held(),
        Some(ItemStack {
            item: items::ITEM_FRAME,
            count: 1
        })
    );
    // A second frame on a face that has one is refused: it would overlap one facing the
    // same way.
    let output = checked(&mut region, &use_on(WALL, api_face(Face::North)));
    assert!(sound_ids(&output).is_empty());
    assert_eq!(hanging_records(&region).len(), 6);
}

#[test]
fn a_frame_is_refused_where_a_block_fills_its_place_and_a_glow_frame_is_its_own_kind() {
    // A block in front of the wall's north face: the frame would be inside it.
    let mut region = ticking_with(&[
        (WALL, blocks::STONE),
        (relative(WALL, Face::North), blocks::STONE),
    ]);
    checked(&mut region, &holding(items::GLOW_ITEM_FRAME));
    let output = checked(&mut region, &use_on(WALL, api_face(Face::North)));
    assert!(sound_ids(&output).is_empty());
    assert!(hanging_records(&region).is_empty());
    // On the floor's top in front of the player, lying flat on it: it holds.
    let floor = BlockPos::new(2, -61, 2);
    let output = checked(&mut region, &use_on(floor, api_face(Face::Up)));
    assert_eq!(sound_ids(&output), [sound::ENTITY_GLOW_ITEM_FRAME_PLACE]);
    let frames = hanging_records(&region);
    assert_eq!(i32::from(frames[0].kind), entity_types::GLOW_ITEM_FRAME);
    assert_eq!(frames[0].position, [2.5, -59.968_75, 2.5]);
}

#[test]
fn a_painting_goes_only_on_the_four_sides() {
    let mut region = ticking_with(&[(WALL, blocks::STONE)]);
    checked(&mut region, &holding(items::PAINTING));
    for face in [Face::Up, Face::Down] {
        let output = checked(&mut region, &use_on(WALL, api_face(face)));
        assert!(sound_ids(&output).is_empty(), "{face:?}");
    }
    assert!(hanging_records(&region).is_empty());
    let output = checked(&mut region, &use_on(WALL, api_face(Face::North)));
    assert_eq!(sound_ids(&output), [sound::ENTITY_PAINTING_PLACE]);
    assert_eq!(hanging_records(&region).len(), 1);
    // The item is kept.
    assert_eq!(
        region.players[&player(1)].held().map(|stack| stack.item),
        Some(items::PAINTING)
    );
}

/// The variant one painting placed by `output`'s tick draws, from the stream of its key
/// after the two draws of its UUID, over `candidates` (the tag's order).
fn drawn(painting: &Entity, tick: u64, candidates: &[u16]) -> u16 {
    let mut own = Random::of_entity(0, painting.key, tick);
    let (most, least) = (own.next_u64(), own.next_u64());
    assert_eq!(
        painting.uuid,
        crate::region::entities::insecure_uuid(most, least)
    );
    let count = u32::try_from(candidates.len()).unwrap();
    candidates[own.next_int(count) as usize]
}

#[test]
fn a_painting_on_a_single_block_draws_one_of_the_eight_placeable_one_by_ones() {
    let mut region = ticking_with(&[(WALL, blocks::STONE)]);
    checked(&mut region, &holding(items::PAINTING));
    let output = checked(&mut region, &use_on(WALL, api_face(Face::North)));
    let [painting] = &hanging_records(&region)[..] else {
        panic!("one painting");
    };
    let EntityData::Painting { attached, variant } = painting.data else {
        panic!("{painting:?}");
    };
    assert_eq!(
        attached,
        Attached {
            block: BlockPos::new(3, -58, 2),
            direction: 2
        }
    );
    // kebab, aztec, alban, aztec2, bomb, plant, wasteland and meditative, in the tag's
    // order: nothing larger is held by one block.
    let one_by_one = [24, 1, 0, 2, 5, 32, 47, 27];
    assert_eq!(variant, drawn(painting, output.tick, &one_by_one));
    // Centred on its block, against the wall.
    assert_eq!(painting.position, [3.5, -57.5, 2.968_75]);
    assert_eq!((painting.yaw, painting.pitch), (180.0, 0.0));
}

#[test]
fn a_painting_on_a_wall_of_four_by_four_draws_among_the_largest_that_fit() {
    // A wall four wide and four high at z = 3, from x = 0 to 3 and y = -59 to -56; the
    // painting clicked onto the north face of (2, -58, 3) is centred at x = 2, on the
    // line between its block and the block to its west.
    let mut wall = Vec::new();
    for x in 0..=3 {
        for y in -59..=-56 {
            wall.push((BlockPos::new(x, y, 3), blocks::STONE));
        }
    }
    let mut region = ticking_with(&wall);
    checked(&mut region, &holding(items::PAINTING));
    let output = checked(
        &mut region,
        &use_on(BlockPos::new(2, -58, 3), api_face(Face::North)),
    );
    let [painting] = &hanging_records(&region)[..] else {
        panic!("one painting");
    };
    let EntityData::Painting { variant, .. } = painting.data else {
        panic!("{painting:?}");
    };
    // pointer, pigscene, burning_skull, unpacked and orb: the 4 by 4s, in the tag's
    // order.
    assert_eq!(variant, drawn(painting, output.tick, &[33, 31, 7, 44, 28]));
    assert_eq!(painting.position, [2.0, -57.0, 2.968_75]);
    // A second one anywhere on it is refused: it would overlap the first.
    let output = checked(
        &mut region,
        &use_on(BlockPos::new(0, -56, 3), api_face(Face::North)),
    );
    assert!(sound_ids(&output).is_empty());
    assert_eq!(hanging_records(&region).len(), 1);
}

#[test]
fn a_painting_with_nowhere_to_fit_is_not_placed() {
    // A carpet in the block in front: its shape is in the box of any painting there.
    let front = BlockPos::new(3, -58, 2);
    let mut region = ticking_with(&[
        (WALL, blocks::STONE),
        (front, state("minecraft:white_carpet")),
    ]);
    checked(&mut region, &holding(items::PAINTING));
    let output = checked(&mut region, &use_on(WALL, api_face(Face::North)));
    assert!(sound_ids(&output).is_empty());
    assert!(hanging_records(&region).is_empty());
}

#[test]
fn a_two_wide_painting_on_a_chunk_line_is_filed_in_its_positions_chunk() {
    // A wall of two at z = 6, x = 15 and 16, either side of the line between chunks 0
    // and 1; clicked on the south face of (15, -58, 6), the 2 by 1 reaches east, and
    // its centre is on x = 16.
    let mut region = ticking_with(&[
        (BlockPos::new(15, -58, 6), blocks::STONE),
        (BlockPos::new(16, -58, 6), blocks::STONE),
    ]);
    let walked = moves(vec![numbered(
        1,
        PlayerInput::Move {
            position: Some(Vec3::new(13.5, -60.0, 8.5)),
            rotation: None,
            on_ground: true,
        },
    )]);
    checked(&mut region, &walked);
    checked(&mut region, &holding(items::PAINTING));
    let output = checked(
        &mut region,
        &use_on(BlockPos::new(15, -58, 6), api_face(Face::South)),
    );
    let [painting] = &hanging_records(&region)[..] else {
        panic!("one painting");
    };
    let EntityData::Painting { attached, variant } = painting.data else {
        panic!("{painting:?}");
    };
    assert_eq!(attached.block, BlockPos::new(15, -58, 7));
    assert_eq!(attached.block.chunk(), ChunkPos::new(0, 0));
    // pool, courbet, sea, sunset and creebet: the 2 by 1s.
    assert_eq!(variant, drawn(painting, output.tick, &[35, 12, 37, 42, 13]));
    assert_eq!(painting.position, [16.0, -57.5, 7.031_25]);
    assert_eq!(region.find_entity(painting.key).unwrap().0, EAST);
}

#[test]
fn a_frame_hung_into_another_regions_chunk_is_sent_there_as_it_is_made() {
    // The region holds the land west of x = 16; a frame on the east face of a block at
    // x = 15 is in its block at x = 16, another region's.
    let at = BlockPos::new(15, -58, 0);
    let mut region = at_a_line(WEST_OF_THE_LINE, &[(at, blocks::STONE)]);
    let walked = moves(vec![numbered(
        1,
        PlayerInput::Move {
            position: Some(Vec3::new(12.5, -60.0, 0.5)),
            rotation: None,
            on_ground: true,
        },
    )]);
    checked(&mut region, &walked);
    checked(&mut region, &holding(items::ITEM_FRAME));
    let output = region.tick(&use_on(at, api_face(Face::East)));
    let [(chunk, sent)] = &output.arrivals[..] else {
        panic!("{:?}", output.arrivals);
    };
    assert_eq!(*chunk, EAST);
    assert!(matches!(
        sent.data,
        EntityData::ItemFrame {
            attached: Attached { direction: 5, .. },
            ..
        }
    ));
    assert_eq!(sent.position, [16.031_25, -57.5, 0.5]);
    assert!(hanging_records(&region).is_empty());
}

// Frames.

#[test]
fn a_click_puts_one_of_the_held_item_in_and_later_clicks_turn_it_round_eight_eighths() {
    let mut region = ticking_with(&[(WALL, blocks::STONE)]);
    let target = frame(1, Face::North, None, 0, false);
    lay(&mut region, &target);
    checked(&mut region, &empty_handed());
    // An empty hand does nothing to an empty frame.
    let output = checked(&mut region, &interact(&target));
    assert!(sound_ids(&output).is_empty());
    let stack = moves(vec![set_slot(1, 0, Some(APPLE)), select(1, 0)]);
    checked(&mut region, &stack);
    let output = checked(&mut region, &interact(&target));
    assert_eq!(sound_ids(&output), [sound::ENTITY_ITEM_FRAME_ADD_ITEM]);
    let after = record(&region, target.key).unwrap();
    assert_eq!(
        frame_item_of(&after),
        (
            Some(ItemStack {
                item: items::APPLE,
                count: 1
            }),
            0
        )
    );
    // Reported with its data, for the edge to send.
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::EntityTracked(Tracked { entity, data: Some(_), .. }) if *entity == target.key
    )));
    // The creative player keeps all five.
    assert_eq!(region.players[&player(1)].held(), Some(APPLE));
    for turn in 1..=8 {
        let output = checked(&mut region, &interact(&target));
        assert_eq!(sound_ids(&output), [sound::ENTITY_ITEM_FRAME_ROTATE_ITEM]);
        let after = record(&region, target.key).unwrap();
        assert_eq!(frame_item_of(&after).1, turn % 8, "turn {turn}");
    }
    // The item stays an apple whatever is held.
    checked(&mut region, &moves(vec![set_slot(1, 0, None)]));
    checked(&mut region, &interact(&target));
    let after = record(&region, target.key).unwrap();
    assert_eq!(frame_item_of(&after).0.unwrap().item, items::APPLE);
    assert_eq!(frame_item_of(&after).1, 1);
}

#[test]
fn a_fixed_frame_takes_nothing_turns_nothing_and_stands_a_survival_hit_and_a_push() {
    let mut region = ticking_with(&[(WALL, blocks::STONE)]);
    let target = frame(1, Face::North, Some(ONE_APPLE), 2, true);
    lay(&mut region, &target);
    checked(&mut region, &interact(&target));
    assert_eq!(record(&region, target.key).unwrap(), target);
    checked(&mut region, &hit_from_afar(&target, 1, false));
    assert_eq!(record(&region, target.key).unwrap(), target);
    let mut output = TickOutput::default();
    assert!(!region.moved_attached(target.key, [0.1, 0.0, 0.0], &mut output));
    assert_eq!(record(&region, target.key).unwrap(), target);
    // A creative player's hit breaks it, and it drops nothing.
    checked(&mut region, &attack(&target));
    assert!(record(&region, target.key).is_none());
    assert!(items_lying(&region).is_empty());
}

#[test]
fn a_creative_players_first_hit_deletes_the_item_and_the_second_breaks_the_frame() {
    let mut region = ticking_with(&[(WALL, blocks::STONE)]);
    let target = frame(1, Face::North, Some(ONE_APPLE), 3, false);
    lay(&mut region, &target);
    let output = checked(&mut region, &attack(&target));
    assert_eq!(sound_ids(&output), [sound::ENTITY_ITEM_FRAME_REMOVE_ITEM]);
    let after = record(&region, target.key).expect("the frame stays");
    // Its rotation is kept for the next item.
    assert_eq!(frame_item_of(&after), (None, 3));
    assert!(items_lying(&region).is_empty());
    let output = checked(&mut region, &attack(&target));
    assert_eq!(sound_ids(&output), [sound::ENTITY_ITEM_FRAME_BREAK]);
    assert!(record(&region, target.key).is_none());
    assert!(items_lying(&region).is_empty());
}

#[test]
fn a_survival_hit_from_another_region_drops_the_item_and_a_second_drops_the_frame() {
    let mut region = ticking_with(&[(WALL, blocks::STONE)]);
    let target = frame(1, Face::North, Some(ONE_APPLE), 0, false);
    lay(&mut region, &target);
    let output = checked(&mut region, &hit_from_afar(&target, 1, false));
    assert_eq!(frame_item_of(&record(&region, target.key).unwrap()).0, None);
    let dropped = items_lying(&region);
    let [(_, _, stack)] = &dropped[..] else {
        panic!("{dropped:?}");
    };
    assert_eq!(*stack, ONE_APPLE);
    // Born 0.15 (a float) out from the wall, north, at the frame's centre.
    let out = f64::from(-0.15_f32);
    assert_eq!(
        born(&output),
        [(items::APPLE, [3.5, -57.5, 2.968_75 + out])]
    );
    checked(&mut region, &hit_from_afar(&target, 2, false));
    assert!(record(&region, target.key).is_none());
    let dropped = items_lying(&region);
    assert_eq!(dropped.len(), 2);
    assert_eq!(dropped[1].2.item, items::ITEM_FRAME);
}

#[test]
fn a_broken_frame_drops_itself_before_its_item_and_draws_once_for_the_item() {
    let mut region = ticking_with(&[(WALL, blocks::STONE)]);
    let target = frame(1, Face::North, Some(ONE_APPLE), 0, false);
    lay(&mut region, &target);
    let mut output = TickOutput::default();
    assert!(region.hurt_attached(target.key, Breaker::Nothing, &mut output));
    let dropped = items_lying(&region);
    let items: Vec<i32> = dropped.iter().map(|(_, _, stack)| stack.item).collect();
    assert_eq!(items, [items::ITEM_FRAME, items::APPLE]);
    // The one float against the drop chance, drawn from the frame's own stream.
    assert_eq!(region.entity_draws[&target.key], 1);
    assert_eq!(sound_ids(&output), [sound::ENTITY_ITEM_FRAME_BREAK]);
}

#[test]
fn a_frame_whose_wall_went_drops_at_its_hundred_and_first_tick() {
    let mut region = ticking_with(&[(WALL, blocks::STONE)]);
    let target = frame(1, Face::North, Some(ONE_APPLE), 0, false);
    lay(&mut region, &target);
    idle(&mut region);
    let mut output = TickOutput::default();
    region.set_block(WALL, blocks::AIR, &mut output);
    for tick in 2..=100 {
        idle(&mut region);
        assert!(record(&region, target.key).is_some(), "tick {tick}");
    }
    idle(&mut region);
    assert!(record(&region, target.key).is_none());
    let items: Vec<i32> = items_lying(&region)
        .iter()
        .map(|(_, _, stack)| stack.item)
        .collect();
    assert_eq!(items, [items::ITEM_FRAME, items::APPLE]);
}

#[test]
fn a_frame_on_its_wall_holds_at_every_check_and_a_diode_holds_a_wall_frame() {
    let repeater = BlockPos::new(6, -60, 6);
    let mut region = ticking_with(&[
        (WALL, blocks::STONE),
        (repeater, state("minecraft:repeater[facing=north]")),
    ]);
    let target = frame(1, Face::North, None, 0, false);
    lay(&mut region, &target);
    let on_diode = frame_at(2, BlockPos::new(6, -60, 5), 2, None, 0, false);
    lay(&mut region, &on_diode);
    for _ in 0..303 {
        idle(&mut region);
    }
    assert_eq!(record(&region, target.key).unwrap(), target);
    assert_eq!(record(&region, on_diode.key).unwrap(), on_diode);
}

#[test]
fn a_pistons_push_pops_a_frame_and_it_drops_its_item() {
    let piston = BlockPos::new(1, -60, 2);
    let mut region = ticking_with(&[(
        piston,
        BlockState::parse("minecraft:piston[facing=east]").unwrap(),
    )]);
    // A frame on the floor in front of the piston, facing up.
    let target = frame_at(1, BlockPos::new(2, -60, 2), 1, Some(ONE_APPLE), 0, false);
    lay(&mut region, &target);
    idle(&mut region);
    let mut output = TickOutput::default();
    region.set_block(
        BlockPos::new(1, -60, 3),
        blocks::REDSTONE_BLOCK,
        &mut output,
    );
    idle(&mut region);
    assert!(record(&region, target.key).is_none());
    let items: Vec<i32> = items_lying(&region)
        .iter()
        .map(|(_, _, stack)| stack.item)
        .collect();
    assert_eq!(items, [items::ITEM_FRAME, items::APPLE]);
    let _ = block_at(&region, piston);
}

#[test]
fn a_use_from_another_region_puts_an_item_in_and_turns_it() {
    let mut region = ticking_with(&[(WALL, blocks::STONE)]);
    let target = frame(1, Face::North, None, 0, false);
    lay(&mut region, &target);
    checked(&mut region, &used_from_afar(&target, 1, Some(APPLE)));
    assert_eq!(
        frame_item_of(&record(&region, target.key).unwrap()),
        (
            Some(ItemStack {
                item: items::APPLE,
                count: 1
            }),
            0
        )
    );
    checked(&mut region, &used_from_afar(&target, 2, None));
    assert_eq!(frame_item_of(&record(&region, target.key).unwrap()).1, 1);
}

#[test]
fn a_players_use_of_a_frame_in_another_regions_copy_is_an_effect_on_it() {
    let mut region = at_a_line(WEST_OF_THE_LINE, &[]);
    let target = frame_at(9, BlockPos::new(17, -60, 0), 1, None, 0, false);
    region
        .copies
        .get_mut(&EAST)
        .unwrap()
        .apply_entity(&EntityChange::Whole(target.clone()));
    let walked = moves(vec![numbered(
        1,
        PlayerInput::Move {
            position: Some(Vec3::new(12.5, -60.0, 0.5)),
            rotation: None,
            on_ground: true,
        },
    )]);
    checked(&mut region, &walked);
    let output = region.tick(&interact(&target));
    assert_eq!(output.effects.len(), 1, "{:?}", output.effects);
    assert!(matches!(
        output.effects[0].what,
        Effected::OnEntity {
            key,
            by: EntityUse::Interact { .. },
        } if key == target.key
    ));
}

// Comparators.

/// A comparator facing north at (5, -60, 5), stone behind it at (5, -60, 4), and a frame
/// on the stone's north side, in (5, -60, 3), facing north as the comparator does.
fn comparator_through_a_block(item: Option<ItemStack>) -> (Region, BlockPos, Entity) {
    let at = BlockPos::new(5, -60, 5);
    let mut region = ticking_with(&[
        (
            at,
            state("minecraft:comparator[facing=north,mode=compare,powered=false]"),
        ),
        (BlockPos::new(5, -60, 4), blocks::STONE),
    ]);
    let target = frame_at(1, BlockPos::new(5, -60, 3), 2, item, 0, false);
    lay(&mut region, &target);
    (region, at, target)
}

#[test]
fn a_comparator_reads_a_frame_through_a_block_and_follows_its_rotation() {
    let (mut region, at, target) = comparator_through_a_block(None);
    idle(&mut region);
    assert_eq!(region.comparator_output(at), 0);
    // An item put in tells the comparator, which reads 1 two ticks later.
    let stack = moves(vec![set_slot(1, 0, Some(ONE_APPLE)), select(1, 0)]);
    checked(&mut region, &stack);
    checked(&mut region, &interact(&target));
    idle(&mut region);
    idle(&mut region);
    assert_eq!(region.comparator_output(at), 1);
    // Turned twice: 3.
    checked(&mut region, &interact(&target));
    checked(&mut region, &interact(&target));
    idle(&mut region);
    idle(&mut region);
    assert_eq!(region.comparator_output(at), 3);
    // Taken out by a hit: 0 again.
    checked(&mut region, &attack(&target));
    idle(&mut region);
    idle(&mut region);
    assert_eq!(region.comparator_output(at), 0);
}

#[test]
fn a_comparator_reads_nothing_of_two_frames_or_of_one_facing_another_way() {
    let (mut region, at, target) = comparator_through_a_block(Some(ONE_APPLE));
    let rotated = Entity {
        data: EntityData::ItemFrame {
            attached: Attached {
                block: BlockPos::new(5, -60, 3),
                direction: 2,
            },
            item: Some(ONE_APPLE),
            rotation: 4,
            drop_chance: Float(1.0),
            fixed: false,
            invisible: false,
        },
        ..target.clone()
    };
    assert_eq!(
        region.frame_signal(BlockPos::new(5, -60, 3), Face::North),
        Some(1)
    );
    assert_eq!(
        region.frame_signal(BlockPos::new(5, -60, 3), Face::South),
        None
    );
    // A second frame of the same facing in the block (laid by hand, as nothing places
    // one there): neither is read.
    let second = Entity {
        key: key(2),
        ..rotated
    };
    lay(&mut region, &second);
    assert_eq!(
        region.frame_signal(BlockPos::new(5, -60, 3), Face::North),
        None
    );
    let _ = at;
}

#[test]
fn a_frames_turn_tells_a_comparator_across_a_line_by_an_effect() {
    // The region holds the land west of x = 16. A frame in (15, -60, 0) facing west
    // hangs on stone in the copy at x = 16, behind which a comparator at x = 17 faces
    // west, reading it.
    let mut region = at_a_line(WEST_OF_THE_LINE, &[]);
    let copy = region.copies.get_mut(&EAST).unwrap();
    copy.set(0, -60, 0, blocks::STONE);
    copy.set(
        1,
        -60,
        0,
        state("minecraft:comparator[facing=west,mode=compare,powered=false]"),
    );
    let target = frame_at(1, BlockPos::new(15, -60, 0), 4, Some(ONE_APPLE), 0, false);
    lay(&mut region, &target);
    let walked = moves(vec![numbered(
        1,
        PlayerInput::Move {
            position: Some(Vec3::new(12.5, -60.0, 0.5)),
            rotation: None,
            on_ground: true,
        },
    )]);
    checked(&mut region, &walked);
    assert_eq!(
        region.frame_signal(BlockPos::new(15, -60, 0), Face::West),
        Some(1)
    );
    let output = region.tick(&interact(&target));
    assert_eq!(frame_item_of(&record(&region, target.key).unwrap()).1, 1);
    let told: Vec<&Effect> = output
        .effects
        .iter()
        .filter(|effect| effect.at == BlockPos::new(17, -60, 0))
        .collect();
    assert!(
        told.iter()
            .any(|effect| matches!(effect.what, Effected::Update { .. })),
        "{:?}",
        output.effects
    );
}

// Picking.

#[test]
fn a_pick_of_a_frame_gives_its_item_and_of_an_empty_one_the_frame() {
    let mut region = ticking_with(&[(WALL, blocks::STONE)]);
    let holding_apple = frame(1, Face::North, Some(ONE_APPLE), 0, false);
    let empty = Entity {
        kind: entity_types::GLOW_ITEM_FRAME as u16,
        ..frame(2, Face::West, None, 0, false)
    };
    lay(&mut region, &holding_apple);
    lay(&mut region, &empty);
    checked(&mut region, &empty_handed());
    let pick = |target: &Entity| {
        moves(vec![numbered(
            1,
            PlayerInput::PickItemFromEntity { entity: target.key },
        )])
    };
    let output = checked(&mut region, &pick(&holding_apple));
    let events: Vec<&PlayerEvent> = output
        .player_events
        .iter()
        .map(|(_, event)| event)
        .collect();
    let one_apple = ItemStack {
        item: items::APPLE,
        count: 1,
    };
    assert_eq!(
        events,
        [
            &PlayerEvent::HeldSlot { slot: 0 },
            &PlayerEvent::InventorySlot {
                slot: 0,
                stack: Some(one_apple)
            },
        ]
    );
    // Picked again: it is in the hotbar, selected, and nothing changes.
    checked(&mut region, &moves(vec![select(1, 4)]));
    let output = checked(&mut region, &pick(&holding_apple));
    let events: Vec<&PlayerEvent> = output
        .player_events
        .iter()
        .map(|(_, event)| event)
        .collect();
    assert_eq!(events, [&PlayerEvent::HeldSlot { slot: 0 }]);
    // The empty glow frame gives a glow frame, into the next empty slot from the
    // selected one.
    let output = checked(&mut region, &pick(&empty));
    let events: Vec<&PlayerEvent> = output
        .player_events
        .iter()
        .map(|(_, event)| event)
        .collect();
    assert_eq!(
        events,
        [
            &PlayerEvent::HeldSlot { slot: 1 },
            &PlayerEvent::InventorySlot {
                slot: 1,
                stack: Some(ItemStack {
                    item: items::GLOW_ITEM_FRAME,
                    count: 1
                })
            },
        ]
    );
    assert_eq!(region.players[&player(1)].selected_slot, 1);
}

// Paintings.

#[test]
fn a_painting_breaks_at_one_hit_and_drops_a_plain_painting_but_to_a_creative_player() {
    let mut region = ticking_with(&[(WALL, blocks::STONE)]);
    checked(&mut region, &holding(items::PAINTING));
    checked(&mut region, &use_on(WALL, api_face(Face::North)));
    let painting = hanging_records(&region).remove(0);
    let output = checked(&mut region, &hit_from_afar(&painting, 1, false));
    assert_eq!(sound_ids(&output), [sound::ENTITY_PAINTING_BREAK]);
    assert!(record(&region, painting.key).is_none());
    let out = f64::from(-0.15_f32);
    assert_eq!(
        born(&output),
        [(items::PAINTING, [3.5, -57.5, 2.968_75 + out])]
    );
    // A creative player's hit: nothing.
    checked(&mut region, &use_on(WALL, api_face(Face::North)));
    let painting = hanging_records(&region).remove(0);
    checked(&mut region, &attack(&painting));
    assert!(record(&region, painting.key).is_none());
    assert_eq!(items_lying(&region).len(), 1);
}
