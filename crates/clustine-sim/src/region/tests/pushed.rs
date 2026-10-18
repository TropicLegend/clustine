//! Tests of entities pushed by pistons (step N4f, `docs/groundwork/n4-plan.md`), from
//! the game's `PistonMovingBlockEntity.moveCollidedEntities` and `moveStuckEntities`
//! and `Entity.move(MoverType.PISTON, …)`. The numbers each test expects are worked
//! out here from the game's formulas, as its bytecode has them, and not by calling the
//! code under test.
//!
//! A move takes three ticks of the block entities' phase, which come after the
//! entities' own: in the first and second the moving block goes half a block on and
//! pushes what is in the slab it sweeps, as far as that reaches into the slab and a
//! hundredth more (`PUSH_OFFSET`), so that it stays a hundredth ahead of the block; in
//! the third the block takes its place. The power is a redstone block set beside the
//! piston between two ticks, so that the piston's block event runs in the next tick.
//!
//! Player 1 stands at the spawn, (0.5, -60, 0.5), on the stone floor whose top is y =
//! -60; only chunk (0, 0) ticks. The items are never picked up.

use clustine_data::BlockClass;
use clustine_world::{Entity, EntityChange, EntityData, EntityField};

use super::redstone::block_at;
use super::*;

const ORIGIN: ChunkPos = ChunkPos::new(0, 0);

/// The game's `0.98f`, widened: the drag of the air.
fn drag() -> f64 {
    f64::from(0.98_f32)
}

/// Half the width of an item, as a float widened.
fn item_half() -> f64 {
    f64::from(0.25_f32 / 2.0)
}

/// Half the width of a falling block, as a float widened.
fn falling_half() -> f64 {
    f64::from(0.98_f32 / 2.0)
}

/// An item of dirt at `position`, at rest, on the ground or not, never picked up.
fn item(key: u64, position: [f64; 3], on_ground: bool) -> Entity {
    Entity {
        key: EntityId((1 << 40) + key),
        kind: clustine_data::entity_types::ITEM as u16,
        uuid: uuid::Uuid::from_u128(u128::from(key)),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground,
        hops: 0,
        age: 1,
        data: EntityData::Item {
            stack: DIRT,
            pick_up_delay: 32_767,
            thrower: None,
            health: 5,
        },
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
    }
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

/// The record of the entity `key`, wherever the region has it.
fn record(region: &Region, key: EntityId) -> Option<Entity> {
    let chunk = region.entity_chunks.get(&key)?;
    region
        .chunk(*chunk)?
        .entities()
        .iter()
        .find(|entity| entity.key == key)
        .cloned()
}

fn state(name: &str) -> BlockState {
    BlockState::parse(name).unwrap()
}

/// Sets a redstone block at `at` between two ticks, or takes it away.
fn power(region: &mut Region, at: BlockPos, on: bool) {
    let mut output = TickOutput::default();
    let block = if on {
        blocks::REDSTONE_BLOCK
    } else {
        blocks::AIR
    };
    region.set_block(at, block, &mut output);
}

/// Whether `output` commits a new place for the entity `key`.
fn commits_place(output: &TickOutput, key: EntityId, place: [f64; 3]) -> bool {
    output.entity_changes.iter().any(|(chunk, change)| {
        *chunk == ORIGIN
            && matches!(change, EntityChange::Set(changed, fields)
                if *changed == key && fields.contains(&EntityField::Position(place)))
    })
}

fn bits(values: [f64; 3]) -> [u64; 3] {
    values.map(f64::to_bits)
}

/// A piston at (1, -60, 2) facing east, with no block before it, and an item lying on
/// the floor where its head goes, at x = 2.5. The head's plate is the far sixteenth to
/// a quarter of its block (x from 0.75 to 1 of it), and the head comes out from where
/// the piston is, drawn back a whole block at first and half a block after a tick.
#[test]
fn a_piston_extending_pushes_an_item_in_front_of_it_by_the_game_s_amounts_each_tick() {
    let piston = BlockPos::new(1, -60, 2);
    let mut region = on_ticking_floor_with(&[(piston, state("minecraft:piston[facing=east]"))]);
    let lying = item(1, [2.5, -60.0, 2.5], true);
    lay(&mut region, &lying);
    idle(&mut region);
    power(&mut region, BlockPos::new(1, -60, 3), true);

    // First tick: the plate's face is at 1 + (2 + (0 - 1)) = 2, and it sweeps to 2.5;
    // the item's box begins at 2.5 - 0.125.
    let first = idle(&mut region);
    let reached = (2.0 + 0.5) - (2.5 - item_half());
    let x1 = 2.5 + (reached.min(0.5) + 0.01);
    let after = record(&region, lying.key).unwrap();
    assert_eq!(bits(after.position), bits([x1, -60.0, 2.5]), "{after:?}");
    // A move by a piston along x hits nothing below: the item is off the ground.
    assert!(!after.on_ground);
    assert!(
        commits_place(&first, lying.key, after.position),
        "{:?}",
        first.entity_changes
    );

    // Second tick: the item falls back onto the floor at once in its own tick; then
    // the plate's face is at 1 + (2 + (0.5 - 1)) = 2.5, sweeping to 3.
    let second = idle(&mut region);
    let reached = (2.5 + 0.5) - (x1 - item_half());
    let x2 = x1 + (reached.min(0.5) + 0.01);
    let after = record(&region, lying.key).unwrap();
    assert_eq!(bits(after.position), bits([x2, -60.0, 2.5]), "{after:?}");
    assert!(commits_place(&second, lying.key, after.position));

    // Third tick: the head is in place, its plate's face at 3, a hundredth short of the
    // item, which lands and lies there.
    idle(&mut region);
    assert!(
        block_at(&region, BlockPos::new(2, -60, 2))
            .is_some_and(|head| clustine_blocks::data::is_class(head, BlockClass::PistonHeadBlock))
    );
    for _ in 0..8 {
        idle(&mut region);
        let lies = record(&region, lying.key).unwrap();
        assert_eq!(bits(lies.position), bits([x2, -60.0, 2.5]));
        assert!(lies.on_ground);
    }
    assert!(x2 - item_half() > 3.0);
}

/// The slime launch's setting: a piston at (1, -60, 2) facing up under a slime block,
/// with an item lying on the slime at y = -58, and the power to come at (0, -60, 2).
fn slime_launch() -> (Region, Entity) {
    let mut region = on_ticking_floor_with(&[
        (
            BlockPos::new(1, -60, 2),
            state("minecraft:piston[facing=up]"),
        ),
        (BlockPos::new(1, -59, 2), blocks::SLIME_BLOCK),
    ]);
    let lying = item(1, [1.5, -58.0, 2.5], true);
    lay(&mut region, &lying);
    (region, lying)
}

/// A slime block pushed up pushes the item on it up with it and sets its motion along
/// y to a block a tick; the next tick the item is beyond the block's reach and flies
/// on under gravity and drag alone.
#[test]
fn a_slime_block_pushed_up_launches_an_item() {
    let (mut region, lying) = slime_launch();
    idle(&mut region);
    power(&mut region, BlockPos::new(0, -60, 2), true);
    // The slime is drawn back a block at first: its top is at -58, and it sweeps to
    // -57.5, half a block into which the item reaches. The pistons may move an entity
    // 0.51 along an axis in a tick.
    let first = idle(&mut region);
    let reached: f64 = (-58.0 + 0.5) - -58.0;
    let mut y = -58.0 + (reached.min(0.5) + 0.01).min(0.51);
    let after = record(&region, lying.key).unwrap();
    assert_eq!(bits(after.position), bits([1.5, y, 2.5]), "{after:?}");
    assert_eq!(bits(after.motion), bits([0.0, 1.0, 0.0]));
    assert!(!after.on_ground);
    assert!(commits_place(&first, lying.key, after.position));
    let mut motion = 1.0_f64;
    for tick in 2..14 {
        idle(&mut region);
        let moved = motion - 0.04;
        y += moved;
        motion = moved * drag();
        let flying = record(&region, lying.key).unwrap();
        assert_eq!(bits(flying.position), bits([1.5, y, 2.5]), "tick {tick}");
        assert_eq!(flying.motion[1].to_bits(), motion.to_bits(), "tick {tick}");
        assert!(!flying.on_ground, "tick {tick}");
    }
    assert!(y > -50.0, "{y}");
    assert_eq!(
        block_at(&region, BlockPos::new(1, -58, 2)),
        Some(blocks::SLIME_BLOCK)
    );
}

/// A honey block's top, fifteen sixteenths of its block.
const HONEY_TOP: f64 = 0.9375;

/// A piston at (1, -58, 2) facing east, raised above the floor so that the honey
/// block before it takes no floor with it, and an item lying on the honey block at
/// x = 2.5.
#[test]
fn a_honey_block_carries_an_item_stuck_to_it() {
    let honey = BlockPos::new(2, -58, 2);
    let mut region = on_ticking_floor_with(&[
        (
            BlockPos::new(1, -58, 2),
            state("minecraft:piston[facing=east]"),
        ),
        (honey, blocks::HONEY_BLOCK),
    ]);
    let top = -58.0 + HONEY_TOP;
    let lying = item(1, [2.5, top, 2.5], true);
    lay(&mut region, &lying);
    idle(&mut region);
    power(&mut region, BlockPos::new(1, -58, 3), true);
    // The item does not reach into the slab the honey sweeps beside it; it stands on
    // the honey, over it, and is carried half a block each tick, no further.
    let first = idle(&mut region);
    let after = record(&region, lying.key).unwrap();
    assert_eq!(bits(after.position), bits([3.0, top, 2.5]), "{after:?}");
    assert!(commits_place(&first, lying.key, after.position));
    idle(&mut region);
    let after = record(&region, lying.key).unwrap();
    assert_eq!(bits(after.position), bits([3.5, top, 2.5]), "{after:?}");
    for _ in 0..6 {
        idle(&mut region);
        let lies = record(&region, lying.key).unwrap();
        assert_eq!(bits(lies.position), bits([3.5, top, 2.5]));
        assert!(lies.on_ground);
    }
    assert_eq!(
        block_at(&region, BlockPos::new(3, -58, 2)),
        Some(blocks::HONEY_BLOCK)
    );
}

/// A sticky piston at (1, -58, 2) facing east and extended, raised above the floor, its
/// head at (2, -58, 2) and `block` before the head, with an item lying on it at x =
/// 3.5; the power beside it at (1, -58, 3) is taken away.
fn pulled(block: BlockState, top: f64) -> (Region, Entity) {
    let at = BlockPos::new(1, -58, 3);
    let mut region = on_ticking_floor_with(&[
        (
            BlockPos::new(1, -58, 2),
            state("minecraft:sticky_piston[facing=east,extended=true]"),
        ),
        (
            BlockPos::new(2, -58, 2),
            state("minecraft:piston_head[facing=east,type=sticky]"),
        ),
        (BlockPos::new(3, -58, 2), block),
        (at, blocks::REDSTONE_BLOCK),
    ]);
    let lying = item(1, [3.5, -58.0 + top, 2.5], true);
    lay(&mut region, &lying);
    idle(&mut region);
    power(&mut region, at, false);
    (region, lying)
}

/// The sticky piston pulls a honey block back with the item on it, half a block each
/// tick; a block of stone it pulls leaves the item where it was, to fall when the block
/// is gone from under it.
#[test]
fn a_retracting_sticky_piston_pulls_an_item_on_top_of_its_block() {
    let (mut region, lying) = pulled(blocks::HONEY_BLOCK, HONEY_TOP);
    let top = -58.0 + HONEY_TOP;
    idle(&mut region);
    let after = record(&region, lying.key).unwrap();
    assert_eq!(bits(after.position), bits([3.0, top, 2.5]), "{after:?}");
    idle(&mut region);
    let after = record(&region, lying.key).unwrap();
    assert_eq!(bits(after.position), bits([2.5, top, 2.5]), "{after:?}");
    for _ in 0..6 {
        idle(&mut region);
        let lies = record(&region, lying.key).unwrap();
        assert_eq!(bits(lies.position), bits([2.5, top, 2.5]));
        assert!(lies.on_ground);
    }
    assert_eq!(
        block_at(&region, BlockPos::new(2, -58, 2)),
        Some(blocks::HONEY_BLOCK)
    );

    let (mut region, lying) = pulled(blocks::STONE, 1.0);
    for _ in 0..3 {
        idle(&mut region);
        let stays = record(&region, lying.key).unwrap();
        assert_eq!(stays.position[0], 3.5);
    }
    assert_eq!(
        block_at(&region, BlockPos::new(2, -58, 2)),
        Some(blocks::STONE)
    );
    for _ in 0..40 {
        idle(&mut region);
    }
    let fallen = record(&region, lying.key).unwrap();
    assert_eq!(bits(fallen.position), bits([3.5, -60.0, 2.5]));
    assert!(fallen.on_ground);
}

/// A falling block of sand at x = 2.5 falls past a piston at (1, -60, 2) facing east as
/// its head comes out: its own tick moves it down, then the head pushes it east by as
/// much as its box (0.98 wide) reaches into the slab the plate sweeps, so that it
/// comes down a block east of where it would have.
#[test]
fn a_falling_block_in_the_path_of_a_head_is_pushed_and_lands_beyond_it() {
    let piston = BlockPos::new(1, -60, 2);
    let mut region = on_ticking_floor_with(&[(piston, state("minecraft:piston[facing=east]"))]);
    power(&mut region, BlockPos::new(1, -60, 3), true);
    let sand = Entity {
        key: EntityId((1 << 40) + 7),
        kind: clustine_data::entity_types::FALLING_BLOCK as u16,
        uuid: uuid::Uuid::from_u128(7),
        position: [2.5, -59.5, 2.5],
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        age: 0,
        data: EntityData::FallingBlock {
            state: blocks::SAND.0,
            time: 1,
            drop_item: true,
            cancel_drop: false,
        },
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
    };
    lay(&mut region, &sand);
    let first = idle(&mut region);
    // Its own tick: gravity, the move, then the drag.
    let mut moved = -0.04_f64;
    let mut y = -59.5 + moved;
    let motion = moved * drag();
    let reached = (2.0 + 0.5) - (2.5 - falling_half());
    let x1 = 2.5 + (reached.min(0.5) + 0.01);
    let after = record(&region, sand.key).unwrap();
    assert_eq!(bits(after.position), bits([x1, y, 2.5]), "{after:?}");
    assert_eq!(after.motion[1].to_bits(), motion.to_bits());
    assert!(commits_place(&first, sand.key, after.position));

    idle(&mut region);
    moved = motion - 0.04;
    y += moved;
    let reached = (2.5 + 0.5) - (x1 - falling_half());
    let x2 = x1 + (reached.min(0.5) + 0.01);
    let after = record(&region, sand.key).unwrap();
    assert_eq!(bits(after.position), bits([x2, y, 2.5]), "{after:?}");

    for _ in 0..8 {
        idle(&mut region);
    }
    assert_eq!(record(&region, sand.key), None, "it has landed");
    assert_eq!(
        block_at(&region, BlockPos::new(3, -60, 2)),
        Some(blocks::SAND)
    );
    assert!(
        block_at(&region, BlockPos::new(2, -60, 2))
            .is_some_and(|head| clustine_blocks::data::is_class(head, BlockClass::PistonHeadBlock))
    );
}

/// Chunk (0, 0) ticks at a line, with nothing loaded east of it. A piston at
/// (13, -60, 2) facing east pushes stone from x = 14 to 15, and an item lying at
/// x = 15.5 before it: the first half block takes it to 15.635, the second over the
/// line at 16, into a chunk the region has not loaded, and it is sent there from the
/// block entities' phase: gone here and in the tick's arrivals whole. (Step N3d, in
/// place of N4's `an_item_a_piston_would_push_into_a_chunk_not_ticking_is_held_at_the_line`,
/// whose guard held it at the line.)
#[test]
fn an_item_a_piston_pushes_across_a_line_is_sent_from_the_block_entities_phase() {
    let mut region = super::arrivals::at_a_line(
        ChunkArea::EVERYWHERE,
        &[
            (
                BlockPos::new(13, -60, 2),
                state("minecraft:piston[facing=east]"),
            ),
            (BlockPos::new(14, -60, 2), blocks::STONE),
        ],
    );
    let lying = item(1, [15.5, -60.0, 2.5], true);
    lay(&mut region, &lying);
    idle(&mut region);
    power(&mut region, BlockPos::new(13, -60, 3), true);
    idle(&mut region);
    let reached = (15.0 + 0.5) - (15.5 - item_half());
    let x1 = 15.5 + (reached.min(0.5) + 0.01);
    let after = record(&region, lying.key).unwrap();
    assert_eq!(after.position[0].to_bits(), x1.to_bits());
    let pushed = idle(&mut region);
    // The second half block reaches as far as the block goes, and a hundredth more.
    let reached = (15.0 + 1.0) - (x1 - item_half());
    let x2 = x1 + (reached.min(0.5) + 0.01);
    assert!(x2 > 16.0);
    let [(to, sent)] = &pushed.arrivals[..] else {
        panic!("{:?}", pushed.arrivals);
    };
    assert_eq!(*to, ChunkPos::new(1, 0));
    assert!((sent.position[0] - x2).abs() < 1e-9, "{sent:?}");
    assert_eq!(sent.hops, 0);
    assert!(
        pushed
            .entity_changes
            .contains(&(ORIGIN, EntityChange::Gone(lying.key)))
    );
    assert!(pushed.events.iter().any(|event| matches!(
        event,
        RegionEvent::HandedOn { entity, chunk: ORIGIN, hops: 0, .. } if *entity == lying.key
    )));
    assert_eq!(record(&region, lying.key), None);
    assert!(!region.entity_chunks.contains_key(&lying.key));
    assert!(!region.entity_memory.contains_key(&lying.key));
    for _ in 0..5 {
        assert!(idle(&mut region).arrivals.is_empty());
    }
}

/// A region and its restore push and launch the same item tick for tick: one restored
/// before the piston is powered, and one restored while the item flies.
#[test]
fn a_region_and_its_restore_push_alike_tick_for_tick() {
    let (mut original, lying) = slime_launch();
    for _ in 0..5 {
        idle(&mut original);
    }
    let mut copy = super::scheduled_ticks::restored(&original);
    for region in [&mut original, &mut copy] {
        power(region, BlockPos::new(0, -60, 2), true);
    }
    let mut late: Option<Region> = None;
    for tick in 0..30 {
        let (a, b) = (idle(&mut original), idle(&mut copy));
        assert_eq!(a.entity_changes, b.entity_changes, "tick {tick}");
        assert_eq!(a.entities_ageing, b.entities_ageing, "tick {tick}");
        assert_eq!(
            record(&original, lying.key),
            record(&copy, lying.key),
            "tick {tick}"
        );
        if let Some(late) = late.as_mut() {
            let c = idle(late);
            assert_eq!(a.entity_changes, c.entity_changes, "tick {tick}");
            assert_eq!(
                record(&original, lying.key),
                record(late, lying.key),
                "tick {tick}"
            );
        }
        if tick == 3 {
            late = Some(super::scheduled_ticks::restored(&original));
        }
    }
    assert!(record(&original, lying.key).unwrap().position[1] > -58.0);
}
