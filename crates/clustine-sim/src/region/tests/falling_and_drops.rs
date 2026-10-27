//! Tests of falling blocks and of what blocks broken by a rule drop (step N4c,
//! `docs/groundwork/n4-plan.md`; the brief's sections 6 to 8). The numbers each test
//! expects are worked out here from the game's formulas, as its bytecode has them, and
//! not by calling the code under test.
//!
//! Player 1 stands at the spawn, (0.5, -60, 0.5), on a stone floor at y = -61 whose top
//! is y = -60, with the chunks around chunk (0, 0) loaded so that it ticks. The falling
//! blocks a test lays lie at x and z of 8.5, out of the player's reach.

use clustine_data::items;
use clustine_world::{Entity, EntityChange, EntityData, EntityField};

use super::*;
use crate::api::Tracked;

/// The game's `0.98f`, widened: a falling block's drag.
fn drag() -> f64 {
    f64::from(0.98_f32)
}

/// A falling block of `state` at `position` with `motion` that has fallen `time` ticks.
fn falling(key: u64, position: [f64; 3], motion: [f64; 3], state: BlockState, time: i32) -> Entity {
    Entity {
        key: EntityId((1 << 40) + key),
        kind: clustine_data::entity_types::FALLING_BLOCK as u16,
        uuid: uuid::Uuid::from_u128(u128::from(key)),
        position,
        motion,
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        data: EntityData::FallingBlock {
            state: state.0,
            time,
            drop_item: true,
            cancel_drop: false,
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

fn on(at: (i32, i32, i32), name: &str) -> (BlockPos, BlockState) {
    (
        BlockPos::new(at.0, at.1, at.2),
        BlockState::parse(name).unwrap(),
    )
}

/// The heights a block falling from rest at `y` passes in each tick until it meets a
/// top at `top`, and the tick it meets it in: each tick gravity takes 0.04 from its
/// motion, the move is cut where it would go through the top (the game's `max(d,
/// top - y)`), and in the air the drag takes a fiftieth.
fn fall_from(mut y: f64, top: f64) -> (Vec<f64>, u64) {
    let mut motion = 0.0_f64;
    let mut heights = Vec::new();
    loop {
        motion += -0.04;
        let moved = motion.max(top - y);
        y += moved;
        heights.push(y);
        if moved != motion {
            return (heights.clone(), heights.len() as u64);
        }
        motion *= drag();
    }
}

/// The items spawned among `events`, with where and how each set off.
fn spawned_items(events: &[RegionEvent]) -> Vec<(EntityId, Vec3, [f64; 3], ItemStack, u16)> {
    events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntitySpawned(state) => match &state.kind {
                EntityKind::Other {
                    data:
                        EntityData::Item {
                            stack,
                            pick_up_delay,
                            ..
                        },
                    motion,
                    ..
                } => Some((
                    state.entity,
                    state.pose.position,
                    *motion,
                    stack.clone(),
                    *pick_up_delay,
                )),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

const SAND: ItemStack = ItemStack::new(items::SAND, 1);

/// Sand let fall from rest five blocks above the floor falls as the game's falling
/// block does: by hand, it passes y -55.04, -55.1192, -55.2368… and meets the floor in
/// its 17th tick, when it lands there as a block, is shown landing, and is gone.
#[test]
fn sand_falls_from_rest_and_lands_on_the_floor_in_the_game_s_tick() {
    let mut region = on_ticking_floor_with(&[]);
    let sand = falling(1, [8.5, -55.0, 8.5], [0.0; 3], blocks::SAND, 0);
    lay(&mut region, &sand);
    let (heights, ticks) = fall_from(-55.0, -60.0);
    assert_eq!(ticks, 17);
    assert_eq!(heights[0], -55.04);
    for (tick, height) in heights.iter().enumerate().take(16) {
        let output = idle(&mut region);
        let now = record(&region, sand.key).expect("still falling");
        assert_eq!(now.position[1].to_bits(), height.to_bits(), "tick {tick}");
        assert_eq!([now.position[0], now.position[2]], [8.5, 8.5]);
        assert!(!now.on_ground);
        let EntityData::FallingBlock { time, .. } = now.data else {
            panic!("{now:?}");
        };
        assert_eq!(time, tick as i32 + 1);
        // Each tick it moves, it commits its place and is reported to the edges.
        assert!(
            output.entity_changes.iter().any(
                |(_, change)| matches!(change, EntityChange::Set(key, fields)
                    if *key == sand.key && fields.contains(&EntityField::Position(now.position)))
            ),
            "tick {tick}"
        );
        assert!(output.events.iter().any(
            |event| matches!(event, RegionEvent::EntityTracked(tracked) if tracked.entity == sand.key)
        ));
    }
    let landed = BlockPos::new(8, -60, 8);
    assert_eq!(region.block(landed), Some(blocks::AIR));
    let output = idle(&mut region);
    assert_eq!(region.block(landed), Some(blocks::SAND));
    assert!(record(&region, sand.key).is_none());
    let shown: Vec<&RegionEvent> = output
        .events
        .iter()
        .filter(|event| {
            matches!(
                event,
                RegionEvent::BlockChanged { .. }
                    | RegionEvent::TransientBlock { .. }
                    | RegionEvent::EntityGone { .. }
            )
        })
        .collect();
    assert_eq!(
        shown,
        [
            &RegionEvent::BlockChanged {
                position: landed,
                state: blocks::SAND,
                seen: true,
            },
            &RegionEvent::TransientBlock {
                position: landed,
                state: blocks::SAND.0,
            },
            &RegionEvent::EntityGone {
                entity: sand.key,
                chunk: ChunkPos::new(0, 0),
                hops: 0,
            },
        ]
    );
}

/// Sand that meets a bottom slab stops on its top, y -59.5, in the block of the slab,
/// which cannot be replaced: it drops one item of sand where it stopped, which waits
/// ten ticks to be picked up and sets off a little sideways and 0.2 up (the random
/// constructor of `ItemEntity`). Made in the entities' phase, the item does not tick
/// until the next.
#[test]
fn sand_that_meets_a_bottom_slab_drops_one_item_where_it_stopped() {
    let mut region = on_ticking_floor_with(&[on((8, -60, 8), "minecraft:stone_slab[type=bottom]")]);
    let sand = falling(1, [8.5, -56.0, 8.5], [0.0; 3], blocks::SAND, 0);
    lay(&mut region, &sand);
    let (_, ticks) = fall_from(-56.0, -59.5);
    for _ in 1..ticks {
        idle(&mut region);
        assert!(record(&region, sand.key).is_some());
    }
    let output = idle(&mut region);
    assert!(record(&region, sand.key).is_none());
    assert_eq!(
        region.block(BlockPos::new(8, -60, 8)),
        Some(BlockState::parse("minecraft:stone_slab[type=bottom]").unwrap())
    );
    let [(key, position, motion, ref stack, delay)] = spawned_items(&output.events)[..] else {
        panic!("{:?}", output.events);
    };
    assert_eq!(*stack, SAND);
    assert_eq!(delay, 10);
    assert_eq!([position.x, position.y, position.z], [8.5, -59.5, 8.5]);
    assert_eq!(motion[1], 0.2);
    assert!(
        motion[0].abs() <= 0.1 && motion[2].abs() <= 0.1,
        "{motion:?}"
    );
    let item = record(&region, key).expect("the item lies in its chunk");
    assert_eq!(item.position, [8.5, -59.5, 8.5], "it has not ticked yet");
    assert!(
        output
            .entity_changes
            .contains(&(ChunkPos::new(0, 0), EntityChange::Whole(item.clone())))
    );
}

/// White concrete powder falling into a pool one block deep is in water once its
/// block is the water's: it lands there in that tick, shown as powder, and the water
/// it replaced hardens it (`ConcretePowderBlock.onLand`).
#[test]
fn concrete_powder_falling_into_water_lands_in_it_and_hardens() {
    let mut region = on_ticking_floor_with(&[on((8, -60, 8), "minecraft:water[level=0]")]);
    let powder = falling(
        1,
        [8.5, -57.0, 8.5],
        [0.0; 3],
        blocks::WHITE_CONCRETE_POWDER,
        0,
    );
    lay(&mut region, &powder);
    // By hand, from y -57: -57.04, -57.1192, -57.2368, -57.3921, -57.5842, -57.8126,
    // -58.0763, -58.3748, -58.7073, -59.0731: in its tenth tick its feet are below
    // -59, in the water's block.
    let (heights, _) = fall_from(-57.0, -60.0);
    let into = heights.iter().position(|y| *y < -59.0).unwrap() + 1;
    assert_eq!(into, 10);
    for _ in 1..into {
        idle(&mut region);
        assert!(record(&region, powder.key).is_some());
    }
    let output = idle(&mut region);
    assert!(record(&region, powder.key).is_none());
    let pool = BlockPos::new(8, -60, 8);
    assert_eq!(region.block(pool), Some(blocks::WHITE_CONCRETE));
    assert!(output.events.contains(&RegionEvent::TransientBlock {
        position: pool,
        state: blocks::WHITE_CONCRETE_POWDER.0,
    }));
}

/// A block that has fallen 600 ticks without landing drops as its item where it is
/// after its move, and is gone (the `time > 600` rule).
#[test]
fn a_block_that_has_fallen_six_hundred_ticks_drops_as_an_item() {
    let mut region = on_ticking_floor_with(&[]);
    let sand = falling(1, [8.5, -50.0, 8.5], [0.0; 3], blocks::SAND, 600);
    lay(&mut region, &sand);
    let output = idle(&mut region);
    assert!(record(&region, sand.key).is_none());
    let [(_, position, _, ref stack, _)] = spawned_items(&output.events)[..] else {
        panic!("{:?}", output.events);
    };
    assert_eq!(*stack, SAND);
    assert_eq!(
        [position.x, position.y, position.z],
        [8.5, -50.0 + -0.04, 8.5]
    );
}

/// A falling block that meets a wall sideways asks to be sent precisely
/// (`setRequiresPrecisePosition(horizontalCollision || verticalCollision)`).
#[test]
fn a_falling_block_that_meets_a_wall_asks_for_its_precise_position() {
    let mut region = on_ticking_floor_with(&[
        on((9, -57, 8), "minecraft:stone"),
        on((9, -56, 8), "minecraft:stone"),
    ]);
    let sand = falling(1, [8.5, -56.5, 8.5], [0.3, 0.0, 0.0], blocks::SAND, 0);
    lay(&mut region, &sand);
    let output = idle(&mut region);
    let tracked = output
        .events
        .iter()
        .find_map(|event| match event {
            RegionEvent::EntityTracked(tracked) if tracked.entity == sand.key => Some(tracked),
            _ => None,
        })
        .expect("reported");
    assert_ne!(tracked.flags & Tracked::HORIZONTAL_COLLISION, 0);
    assert_ne!(tracked.flags & Tracked::PRECISE, 0);
    // The box, 0.98 wide (a float), stops at the wall: its east side at x 9.
    assert_eq!(tracked.position[0], 9.0 - f64::from(0.98_f32) / 2.0);
}

/// A cobweb holds a falling block on its next move: a twentieth of its fall, and its
/// motion gone (`makeStuckInBlock`, then `Entity.move`).
#[test]
fn a_cobweb_holds_a_falling_block_on_its_next_move() {
    let mut region = on_ticking_floor_with(&[on((8, -58, 8), "minecraft:cobweb")]);
    let sand = falling(1, [8.5, -57.5, 8.5], [0.0; 3], blocks::SAND, 0);
    lay(&mut region, &sand);
    idle(&mut region);
    let first = record(&region, sand.key).unwrap();
    assert_eq!(first.position[1], -57.54);
    idle(&mut region);
    let second = record(&region, sand.key).unwrap();
    let asked = -0.04 * drag() + -0.04;
    let held = asked * f64::from(0.05_f32);
    assert_eq!(second.position[1].to_bits(), (-57.54 + held).to_bits());
    assert_eq!(second.motion, [0.0, 0.0, 0.0]);
}

/// A region and its restore move the same falling blocks tick for tick: one that
/// lands, one that drops on a slab, one that falls far.
#[test]
fn a_region_and_its_restore_fall_alike_tick_for_tick() {
    let mut original =
        on_ticking_floor_with(&[on((10, -60, 8), "minecraft:stone_slab[type=bottom]")]);
    let blocks_falling = [
        falling(1, [8.5, -52.0, 8.5], [0.0; 3], blocks::SAND, 0),
        falling(2, [10.5, -55.0, 8.5], [0.0; 3], blocks::GRAVEL, 0),
        falling(3, [12.5, -40.0, 8.5], [0.0; 3], blocks::ANVIL, 0),
    ];
    for entity in &blocks_falling {
        lay(&mut original, entity);
    }
    for _ in 0..5 {
        idle(&mut original);
    }
    let mut copy = super::scheduled_ticks::restored(&original);
    for tick in 0..40 {
        let (a, b) = (idle(&mut original), idle(&mut copy));
        assert_eq!(a.entity_changes, b.entity_changes, "tick {tick}");
        assert_eq!(a.events, b.events, "tick {tick}");
        for entity in &blocks_falling {
            assert_eq!(
                record(&original, entity.key),
                record(&copy, entity.key),
                "tick {tick}"
            );
        }
    }
    assert!(
        items_lying(&original)
            .iter()
            .any(|(_, stack)| stack.item == items::GRAVEL)
    );
    assert_eq!(items_lying(&original), items_lying(&copy));
}

/// A creative player breaks the floor under a torch: the floor drops nothing, and the
/// torch, broken by the update, drops a torch where `popResource` puts it: within a
/// quarter of a block of its block's centre each way, its feet an eighth lower. Laid
/// before the entities' phase, the item ticks in that tick: 0.2 up less gravity's
/// 0.04. Its record is committed whole, and a restore has it.
#[test]
fn a_torch_whose_support_is_broken_drops_a_torch_within_its_draws_bounds() {
    let torch = BlockPos::new(2, -60, 3);
    let mut region = on_ticking_floor_with(&[(torch, blocks::TORCH)]);
    let output = region.tick(&moves(vec![dig(1, 2, -61, 3, 1)]));
    assert_eq!(region.block(torch), Some(blocks::AIR));
    let [(key, position, motion, ref stack, delay)] = spawned_items(&output.events)[..] else {
        panic!("one drop, of the torch: {:?}", output.events);
    };
    assert_eq!(*stack, ItemStack::new(items::TORCH, 1));
    assert_eq!(delay, 10);
    assert!((2.25..2.75).contains(&position.x), "{position:?}");
    assert!((-59.875..-59.375).contains(&position.y), "{position:?}");
    assert!((3.25..3.75).contains(&position.z), "{position:?}");
    assert_eq!(motion[1], 0.2);
    let item = record(&region, key).expect("lying in its chunk");
    assert_eq!(
        item.position[1].to_bits(),
        (position.y + (0.2 + -0.04)).to_bits()
    );
    let EntityData::Item { pick_up_delay, .. } = item.data else {
        panic!("{item:?}");
    };
    assert_eq!(pick_up_delay, 9, "it counted down in its tick");
    let made = output
        .entity_changes
        .iter()
        .find_map(|(chunk, change)| match change {
            EntityChange::Whole(entity) if entity.key == key => Some((*chunk, entity.clone())),
            _ => None,
        })
        .expect("committed whole");
    assert_eq!(made.0, ChunkPos::new(0, 0));
    assert_eq!(
        [made.1.position[0], made.1.position[1], made.1.position[2]],
        [position.x, position.y, position.z]
    );
    let copy = super::scheduled_ticks::restored(&region);
    assert_eq!(record(&copy, key), Some(item));
}

/// A creative player's own break drops nothing (`ServerPlayerGameMode.destroyBlock`).
#[test]
fn a_creative_player_s_break_drops_nothing() {
    let mut region = on_ticking_floor_with(&[on((2, -60, 3), "minecraft:oak_log")]);
    let output = region.tick(&moves(vec![dig(1, 2, -60, 3, 1)]));
    assert_eq!(region.block(BlockPos::new(2, -60, 3)), Some(blocks::AIR));
    assert_eq!(spawned_items(&output.events), []);
    assert!(items_lying(&region).is_empty());
}

/// Scaffolding whose support a player breaks takes its distance at its tick of 1,
/// which counts from the tick before the dig's as the game handles a packet between
/// ticks, so in the dig's tick: seven, from nothing below or beside, so it breaks and
/// drops itself; the player's break drops nothing.
#[test]
fn scaffolding_whose_support_is_broken_breaks_and_drops_itself() {
    let low = "minecraft:scaffolding[bottom=false,distance=0,waterlogged=false]";
    let mut region = on_ticking_floor_with(&[on((2, -60, 3), low), on((2, -59, 3), low)]);
    let output = region.tick(&moves(vec![dig(1, 2, -60, 3, 1)]));
    assert_eq!(region.block(BlockPos::new(2, -60, 3)), Some(blocks::AIR));
    assert_eq!(region.block(BlockPos::new(2, -59, 3)), Some(blocks::AIR));
    let drops = spawned_items(&output.events);
    assert_eq!(drops.len(), 1, "{drops:?}");
    assert_eq!(drops[0].3, ItemStack::new(items::SCAFFOLDING, 1));
    // Dropped where the upper one was.
    assert!((-59.0..-58.0).contains(&drops[0].1.y), "{drops:?}");
}

/// A stalactite whose ceiling is broken falls whole, two ticks on, from its base down
/// to its tip; neither part can stay on the floor, with nothing above to hang from,
/// so each drops as pointed dripstone with the sound of a stalactite landing (level
/// event 1045).
#[test]
fn a_stalactite_whose_ceiling_is_broken_falls_and_drops_as_dripstone() {
    let (ceiling, base, tip) = (
        BlockPos::new(2, -57, 3),
        BlockPos::new(2, -58, 3),
        BlockPos::new(2, -59, 3),
    );
    let mut region = on_ticking_floor_with(&[
        (ceiling, blocks::STONE),
        on(
            (2, -58, 3),
            "minecraft:pointed_dripstone[thickness=base,vertical_direction=down,waterlogged=false]",
        ),
        on(
            (2, -59, 3),
            "minecraft:pointed_dripstone[thickness=tip,vertical_direction=down,waterlogged=false]",
        ),
    ]);
    region.tick(&moves(vec![dig(1, 2, -57, 3, 1)]));
    assert!(
        region
            .block(base)
            .is_some_and(|state| !state.props().is_air())
    );
    let started = idle(&mut region);
    assert_eq!(region.block(base), Some(blocks::AIR));
    assert_eq!(region.block(tip), Some(blocks::AIR));
    let falling_blocks = started
        .events
        .iter()
        .filter(|event| {
            matches!(event, RegionEvent::EntitySpawned(state)
                if matches!(state.kind, EntityKind::Other { data: EntityData::FallingBlock { .. }, .. }))
        })
        .count();
    assert_eq!(falling_blocks, 2);
    let outputs = until_fallen(&mut region);
    let events: Vec<RegionEvent> = outputs
        .into_iter()
        .flat_map(|output| output.events)
        .collect();
    let landings = events
        .iter()
        .filter(|event| matches!(event, RegionEvent::LevelEvent { kind: 1045, .. }))
        .count();
    assert_eq!(landings, 2);
    let lying: Vec<ItemStack> = items_lying(&region)
        .into_iter()
        .map(|(_, stack)| stack)
        .collect();
    let dripstone = ItemStack::new(items::POINTED_DRIPSTONE, 1);
    assert_eq!(lying, [dripstone.clone(), dripstone]);
    assert_eq!(region.block(BlockPos::new(2, -60, 3)), Some(blocks::AIR));
}

/// Water that flows onto a torch washes it away with its drop
/// (`WaterFluid.beforeDestroyingBlock`).
#[test]
fn water_that_washes_a_torch_away_drops_the_torch() {
    let mut region = on_ticking_floor_with(&[
        on((4, -60, 3), "minecraft:water[level=0]"),
        on((5, -60, 3), "minecraft:torch"),
    ]);
    region.tick(&moves(vec![]));
    let water_tick = clustine_data::FLUIDS
        .iter()
        .position(|name| *name == "minecraft:water")
        .unwrap() as u8;
    region.schedule_tick(
        BlockPos::new(4, -60, 3),
        clustine_world::Ticked::Fluid(water_tick),
        1,
        0,
    );
    let mut events = Vec::new();
    for _ in 0..12 {
        events.extend(idle(&mut region).events);
        if region.block(BlockPos::new(5, -60, 3)) != Some(blocks::TORCH) {
            break;
        }
    }
    assert_ne!(region.block(BlockPos::new(5, -60, 3)), Some(blocks::TORCH));
    let drops = spawned_items(&events);
    assert_eq!(drops.len(), 1, "{drops:?}");
    assert_eq!(drops[0].3.item, items::TORCH);
}
