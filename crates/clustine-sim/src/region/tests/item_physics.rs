//! Tests of the item's physics and life (step N4b, `docs/groundwork/n4-plan.md`, and the
//! brief's sections 1 to 5). The numbers each test expects are worked out here from
//! the game's formulas, step by step as its bytecode has them, and not by calling the
//! code under test.
//!
//! Player 1 stands at the spawn, (0.5, -60, 0.5), on a stone floor at y = -61, with the
//! chunks around chunk (0, 0) loaded so that only it ticks; the items lie at x and z of
//! 8.5 or so, out of the player's reach, unless a test wants them picked up.

use clustine_world::{Entity, EntityChange, EntityData, EntityField};

use super::*;
use crate::api::Tracked;

const ORIGIN: ChunkPos = ChunkPos::new(0, 0);

/// The game's `0.98f`, widened: the air's drag of an item.
fn drag() -> f64 {
    f64::from(0.98_f32)
}

/// An item of `stack` at `position` with `motion`, on the ground or not, that is never
/// picked up unless a test says so.
fn item(
    key: u64,
    position: [f64; 3],
    motion: [f64; 3],
    on_ground: bool,
    stack: ItemStack,
) -> Entity {
    Entity {
        key: EntityId((1 << 40) + key),
        kind: clustine_data::entity_types::ITEM as u16,
        uuid: uuid::Uuid::from_u128(u128::from(key)),
        position,
        motion,
        yaw: 0.0,
        pitch: 0.0,
        on_ground,
        hops: 0,
        age: 1,
        data: EntityData::Item {
            stack,
            pick_up_delay: 32_767,
            thrower: None,
            health: 5,
        },
        fall_distance: 0.0,
        fire_ticks: 0,
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

/// The ticks an item takes to fall from rest at `y` onto a top at `top`, and where it
/// comes to lie: each tick gravity takes 0.04 from its motion, the move is cut where
/// it would go through the top (the game's `max(d, top - y)`), and in the air the drag
/// takes a fiftieth.
fn falls(mut y: f64, top: f64) -> (u64, f64) {
    let mut motion = 0.0_f64;
    let mut ticks = 0;
    loop {
        ticks += 1;
        motion += -0.04;
        let moved = motion.max(top - y);
        y += moved;
        if moved != motion {
            return (ticks, y);
        }
        motion *= drag();
    }
}

/// An item dropped from rest at `height` above the floor's top over the block at
/// (8, -60, 8), which is `block`, comes to rest at `top` in the ticks the game takes.
fn lands_on(block: &str, x: f64, z: f64, top: f64) {
    let mut region = on_ticking_floor_with(&[on((8, -60, 8), block)]);
    let start = -57.0;
    let dropped = item(1, [x, start, z], [0.0; 3], false, DIRT);
    lay(&mut region, &dropped);
    let (ticks, y) = falls(start, top);
    for tick in 1..ticks {
        idle(&mut region);
        let falling = record(&region, dropped.key).unwrap();
        assert!(!falling.on_ground, "{block}: tick {tick}");
    }
    let output = idle(&mut region);
    let landed = record(&region, dropped.key).unwrap();
    assert_eq!(
        landed.position[1].to_bits(),
        y.to_bits(),
        "{block}: {landed:?}"
    );
    assert_eq!([landed.position[0], landed.position[2]], [x, z], "{block}");
    assert!(landed.on_ground, "{block}");
    // The landing ends the fall: the motion is cut to nothing, and the tick commits the
    // place and the ground.
    assert_eq!(landed.motion[1], 0.0, "{block}");
    assert_eq!(landed.fall_distance, 0.0, "{block}");
    let [(chunk, EntityChange::Set(key, fields))] = &output.entity_changes[..] else {
        panic!("{block}: {:?}", output.entity_changes);
    };
    assert_eq!((*chunk, *key), (ORIGIN, dropped.key));
    assert!(fields.contains(&EntityField::OnGround(true)), "{fields:?}");
    // From then on it lies still, and commits nothing.
    for tick in 0..12 {
        let output = idle(&mut region);
        assert!(output.entity_changes.is_empty(), "{block}: tick {tick}");
        let lying = record(&region, dropped.key).unwrap();
        assert_eq!(lying.position, landed.position, "{block}");
        assert!(lying.on_ground, "{block}");
    }
}

#[test]
fn an_item_falls_onto_a_slab_and_stops_at_its_top() {
    // A bottom slab's top is half a block up.
    lands_on("minecraft:oak_slab[type=bottom]", 8.5, 8.5, -59.5);
}

#[test]
fn an_item_falls_onto_stairs_and_stops_on_the_step_it_is_over() {
    // Stairs facing east have their high half on the east.
    let stairs = "minecraft:oak_stairs[facing=east,half=bottom,shape=straight]";
    lands_on(stairs, 8.25, 8.5, -59.5);
    lands_on(stairs, 8.75, 8.5, -59.0);
}

#[test]
fn an_item_falls_onto_a_fence_post_a_block_and_a_half_high() {
    // A fence alone is its post, from 6 to 10 sixteenths, and as high as a block and a
    // half, which the collider finds from the cell above it.
    lands_on("minecraft:oak_fence", 8.5, 8.5, -58.5);
}

#[test]
fn an_item_falls_into_a_cauldron_and_lies_on_its_floor() {
    // A cauldron's inside is hollow from four sixteenths up.
    lands_on("minecraft:cauldron", 8.5, 8.5, -60.0 + 0.25);
}

/// Where an item sliding along x from 8.5 at 0.2 a tick on a floor of `floor` is after
/// `ticks`, and its motion then: every tick it moves by its motion, and the ground's
/// friction (the block's times the air's, as floats) takes from it.
fn slides(floor: &str, ticks: u64) -> ([f64; 2], [f64; 2]) {
    let state = BlockState::parse(floor).unwrap();
    let row: Vec<(BlockPos, BlockState)> =
        (0..16).map(|x| (BlockPos::new(x, -61, 8), state)).collect();
    let mut region = on_ticking_floor_with(&row);
    let sliding = item(1, [3.5, -60.0, 8.5], [0.2, 0.0, 0.0], true, DIRT);
    lay(&mut region, &sliding);
    for _ in 0..ticks {
        idle(&mut region);
    }
    let after = record(&region, sliding.key).unwrap();
    assert!(after.on_ground);
    assert_eq!(after.position[1], -60.0);
    (
        [after.position[0], after.motion[0]],
        [after.position[2], after.motion[2]],
    )
}

/// The same by the game's formula.
fn slides_by_hand(friction: f32, ticks: u64) -> [f64; 2] {
    let ground = f64::from(0.98_f32 * friction);
    let (mut x, mut motion) = (3.5_f64, 0.2_f64);
    for _ in 0..ticks {
        x += motion;
        motion *= ground;
    }
    [x, motion]
}

#[test]
fn an_item_slides_on_ice_and_blue_ice_by_the_game_s_friction() {
    for (floor, friction) in [
        ("minecraft:stone", 0.6_f32),
        ("minecraft:ice", 0.98),
        ("minecraft:packed_ice", 0.98),
        ("minecraft:blue_ice", 0.989),
    ] {
        let ticks = 5;
        let (along, across) = slides(floor, ticks);
        let expected = slides_by_hand(friction, ticks);
        assert_eq!(
            along.map(f64::to_bits),
            expected.map(f64::to_bits),
            "{floor}"
        );
        assert_eq!(across, [8.5, 0.0], "{floor}");
    }
    // On blue ice it goes furthest.
    let (stone, _) = slides("minecraft:stone", 5);
    let (ice, _) = slides("minecraft:ice", 5);
    let (blue, _) = slides("minecraft:blue_ice", 5);
    assert!(stone[0] < ice[0] && ice[0] < blue[0]);
}

/// The motion an item falling from rest at -57 has as it lands on the bouncy block at
/// the top `top` with a bounce of `bounce` (the block's restitution times 0.8 for
/// what is not living, as floats), and after the tick's drag on the ground, by the
/// game's `restituteMovementAfterCollisions`.
fn bounce_by_hand(top: f64, bounce: f32) -> (u64, f64, f64) {
    let (mut y, mut motion) = (-57.0_f64, 0.0_f64);
    let mut ticks = 0;
    loop {
        ticks += 1;
        motion += -0.04;
        let moved = motion.max(top - y);
        y += moved;
        if moved != motion {
            let restitution = f64::from(bounce * 0.8);
            let ratio = moved / motion;
            let a = ratio * 0.04;
            let b = 1.0 + ratio * (f64::from(0.98_f32) - 1.0);
            let up = (a - motion) * b * restitution;
            return (ticks, y, up * drag());
        }
        motion *= drag();
    }
}

fn bounces_on(block: &str, top: f64, bounce: f32) {
    let mut region = on_ticking_floor_with(&[on((8, -60, 8), block)]);
    let dropped = item(1, [8.5, -57.0, 8.5], [0.0; 3], false, DIRT);
    lay(&mut region, &dropped);
    let (ticks, y, up) = bounce_by_hand(top, bounce);
    let mut landing = TickOutput::default();
    for _ in 0..ticks {
        landing = idle(&mut region);
    }
    let landed = record(&region, dropped.key).unwrap();
    assert_eq!(landed.position[1].to_bits(), y.to_bits(), "{block}");
    assert_eq!(
        landed.motion[1].to_bits(),
        up.to_bits(),
        "{block}: {landed:?}"
    );
    assert!(up > 0.1, "{block}: {up}");
    // The bounce makes the tick one of the edge's update ticks.
    let tracked = landing
        .events
        .iter()
        .find_map(|event| match event {
            RegionEvent::EntityTracked(tracked) if tracked.entity == dropped.key => Some(tracked),
            _ => None,
        })
        .expect("the landing is reported");
    assert_ne!(tracked.flags & Tracked::SYNC_POSITION, 0, "{block}");
    // And it goes up again, at the first tick it moves: on the ground and still
    // sideways, an item moves only where `(tickCount + getId()) % 4 == 0`, and its key's
    // game id is 1.
    loop {
        let output = idle(&mut region);
        let now = record(&region, dropped.key).unwrap();
        if (output.tick as i32 + 1) % 4 == 0 {
            assert!(now.position[1] > landed.position[1], "{block}");
            assert!(!now.on_ground, "{block}");
            break;
        }
        assert_eq!(now.position, landed.position, "{block}");
    }
}

#[test]
fn an_item_bounces_on_slime_and_on_a_bed() {
    bounces_on("minecraft:slime_block", -59.0, 1.0);
    // A bed is nine sixteenths high.
    bounces_on(
        "minecraft:red_bed[facing=north,occupied=false,part=foot]",
        -60.0 + 0.5625,
        0.75,
    );
}

#[test]
fn flowing_water_pushes_an_item_along_its_flow() {
    // A source at x = 7 and water of level 1 (seven ninths) at x = 8: the water at 8
    // flows east, away from the source, with nothing beside it to the east.
    let mut region = on_ticking_floor_with(&[
        on((7, -60, 8), "minecraft:water[level=0]"),
        on((8, -60, 8), "minecraft:water[level=1]"),
    ]);
    let floating = item(1, [8.5, -60.0, 8.5], [0.0; 3], true, DIRT);
    lay(&mut region, &floating);
    let output = idle(&mut region);
    // `baseTick` finds the item in water and pushes it 0.014 along the flow's unit
    // vector; under water it is slowed by 0.99 and rises by 5e-4 (no gravity); it
    // moves; in the air of the tick's drag, 0.98; and at the end of the tick the water
    // pushes it again.
    let pushed = 0.014 * f64::from(0.99_f32);
    let rise = f64::from(5.0e-4_f32);
    let after = record(&region, floating.key).unwrap();
    assert_eq!(after.position[0].to_bits(), (8.5 + pushed).to_bits());
    assert_eq!(after.position[1].to_bits(), (-60.0 + rise).to_bits());
    assert_eq!(after.position[2], 8.5);
    assert_eq!(
        after.motion[0].to_bits(),
        (pushed * drag() + 0.014).to_bits()
    );
    assert_eq!(after.motion[1].to_bits(), (rise * drag()).to_bits());
    assert_eq!(after.motion[2], 0.0);
    assert!(!after.on_ground);
    // In water it is sent every tick (`needsSync`).
    let tracked = output
        .events
        .iter()
        .find_map(|event| match event {
            RegionEvent::EntityTracked(tracked) => Some(tracked),
            _ => None,
        })
        .unwrap();
    assert_ne!(tracked.flags & Tracked::NEEDS_SYNC, 0);
    // It goes on east, out of the water.
    for _ in 0..30 {
        idle(&mut region);
    }
    let carried = record(&region, floating.key).unwrap();
    assert!(carried.position[0] > 9.0, "{carried:?}");
}

#[test]
fn an_item_in_still_water_rises_to_float_near_the_surface() {
    // A source alone on the floor does not flow: the item only rises.
    let mut region = on_ticking_floor_with(&[on((8, -60, 8), "minecraft:water[level=0]")]);
    let floating = item(1, [8.5, -60.0, 8.5], [0.0; 3], true, DIRT);
    lay(&mut region, &floating);
    // It rises while more than a tenth of it is under water, at 5e-4 a tick more each
    // tick less the drag, and falls by gravity once it is not: it settles where a
    // tenth of a block is under the surface at eight ninths.
    let surface = -60.0 + f64::from(8.0_f32 / 9.0);
    let mut heights = Vec::new();
    for _ in 0..300 {
        idle(&mut region);
        heights.push(record(&region, floating.key).unwrap().position[1]);
    }
    // It bobs there: above the tenth of a block gravity pulls it back by 0.04 a tick,
    // which the water's slowing takes many ticks to undo.
    let settled = &heights[200..];
    let low = settled.iter().copied().fold(f64::INFINITY, f64::min);
    let high = settled.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    assert!(high <= surface - 0.1 + 0.03, "{high} against {surface}");
    assert!(low > surface - 0.6, "{low} against {surface}");
    assert!(high - low > 0.1, "{low} to {high}");
    let still = record(&region, floating.key).unwrap();
    assert_eq!([still.position[0], still.position[2]], [8.5, 8.5]);
}

/// An item of `stack` lying in lava at (8, -60, 8) as it is ticked.
fn in_lava(stack: ItemStack) -> (Region, Entity) {
    let mut region = on_ticking_floor_with(&[on((8, -60, 8), "minecraft:lava[level=0]")]);
    let burning = item(1, [8.5, -60.0, 8.5], [0.0; 3], true, stack);
    lay(&mut region, &burning);
    (region, burning)
}

#[test]
fn lava_burns_an_item_in_two_ticks_and_netherite_not_at_all() {
    // Each tick in lava sets it alight for 300 ticks and takes 4 of its 5 health
    // (`LavaFluid.entityInside`, `Entity.lavaHurt`): it is gone in the second.
    let (mut region, burning) = in_lava(DIRT);
    let output = idle(&mut region);
    let alight = record(&region, burning.key).unwrap();
    let EntityData::Item { health, .. } = alight.data else {
        panic!();
    };
    assert_eq!((health, alight.fire_ticks), (1, 300));
    // Health and burning are committed; the hurt's motion goes to everyone.
    let tracked = output
        .events
        .iter()
        .find_map(|event| match event {
            RegionEvent::EntityTracked(tracked) => Some(tracked),
            _ => None,
        })
        .unwrap();
    assert_ne!(tracked.flags & Tracked::SYNC_VELOCITY, 0);
    let output = idle(&mut region);
    assert_eq!(record(&region, burning.key), None);
    assert!(output.events.contains(&RegionEvent::EntityGone {
        entity: burning.key,
        chunk: ORIGIN,
        hops: 0,
    }));
    assert!(
        output
            .entity_changes
            .contains(&(ORIGIN, EntityChange::Gone(burning.key)))
    );
    // As it burns up it hisses (`GENERIC_BURN`, as its health is gone).
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::Sound { sound, volume, .. }
            if *sound == clustine_data::sounds::ENTITY_GENERIC_BURN && *volume == 0.4
    )));

    // Netherite does not burn: its stack resists fire.
    let netherite = ItemStack {
        item: items::NETHERITE_INGOT,
        count: 1,
    };
    let (mut region, lying) = in_lava(netherite);
    for _ in 0..100 {
        idle(&mut region);
    }
    let unhurt = record(&region, lying.key).unwrap();
    let EntityData::Item { health, .. } = unhurt.data else {
        panic!();
    };
    assert_eq!(health, 5);
    assert!(unhurt.fire_ticks <= 0);
}

#[test]
fn fire_sets_an_item_alight_and_burning_takes_its_health() {
    // In fire: alight for 160 ticks and 1 a tick in the fire; out of it, 1 every
    // twentieth tick of the burning (`baseTick`).
    let mut region = on_ticking_floor_with(&[on((8, -60, 8), "minecraft:fire")]);
    let burning = item(1, [8.5, -60.0, 8.5], [0.0; 3], true, DIRT);
    lay(&mut region, &burning);
    let mut healths = Vec::new();
    for _ in 0..5 {
        idle(&mut region);
        healths.push(
            record(&region, burning.key).map(|entity| match entity.data {
                EntityData::Item { health, .. } => (health, entity.fire_ticks),
                EntityData::FallingBlock { .. } => panic!(),
            }),
        );
    }
    // The first tick: lit at 160 and hurt by 1. It lies still and on the ground, so the
    // ticks it does not move it meets the fire again by its last moves.
    assert_eq!(healths[0], Some((4, 160)));
    assert_eq!(healths.last().unwrap(), &None, "{healths:?}");
}

#[test]
fn a_cactus_pricks_an_item_to_nothing_in_five_ticks() {
    // An item lying against a cactus's side is inside it (its inside is the whole
    // block) and loses 1 a tick.
    let mut region = on_ticking_floor_with(&[on((8, -60, 8), "minecraft:cactus")]);
    let pricked = item(1, [7.9, -60.0, 8.5], [0.0; 3], true, DIRT);
    lay(&mut region, &pricked);
    for tick in 1..5 {
        idle(&mut region);
        let EntityData::Item { health, .. } = record(&region, pricked.key).unwrap().data else {
            panic!();
        };
        assert_eq!(health, 5 - tick as i16);
    }
    idle(&mut region);
    assert_eq!(record(&region, pricked.key), None);
}

/// Two items lying side by side at rest, the first of `first` (age 100, delay 10)
/// and the second of `second` (age 50, delay 20), both with keys in that order.
fn side_by_side(first: ItemStack, second: ItemStack) -> (Region, Entity, Entity) {
    let mut region = on_ticking_floor_with(&[]);
    let mut a = item(1, [8.5, -60.0, 8.5], [0.0; 3], true, first);
    let mut b = item(2, [9.0, -60.0, 8.5], [0.0; 3], true, second);
    for (entity, age, delay) in [(&mut a, 100, 10), (&mut b, 50, 20)] {
        entity.age = age;
        if let EntityData::Item { pick_up_delay, .. } = &mut entity.data {
            *pick_up_delay = delay;
        }
    }
    lay(&mut region, &a);
    lay(&mut region, &b);
    (region, a, b)
}

#[test]
fn two_items_merge_with_the_game_s_rules_and_ages() {
    // Items at rest merge every fortieth tick of the region (`tickCount % 40`), the
    // smaller stack into the larger, the second into the first where they are as big.
    let (mut region, a, b) = side_by_side(
        ItemStack { count: 3, ..DIRT },
        ItemStack { count: 5, ..DIRT },
    );
    let start = region.tick;
    let merge_tick = (start / 40 + 1) * 40;
    while region.tick + 1 < merge_tick {
        let output = idle(&mut region);
        assert!(output.entity_changes.is_empty());
    }
    let output = idle(&mut region);
    assert_eq!(output.tick, merge_tick);
    // The first ticks first and goes into the second, which holds more; the second
    // takes the smaller age and the larger delay of the two, as they were when the
    // first ticked (its own not yet counted, the first's counted), and then counts its
    // own tick.
    let ticks = merge_tick - start;
    assert_eq!(record(&region, a.key), None);
    let merged = record(&region, b.key).unwrap();
    let EntityData::Item {
        stack,
        pick_up_delay,
        ..
    } = merged.data
    else {
        panic!();
    };
    assert_eq!(stack.count, 8);
    let a_age = 100 + ticks - 1;
    let b_age = 50 + ticks - 1;
    assert_eq!(u64::from(merged.age), a_age.min(b_age) + 1);
    let a_delay = 10_u64.saturating_sub(ticks);
    let b_delay = 20_u64.saturating_sub(ticks - 1);
    assert_eq!(
        u64::from(pick_up_delay),
        a_delay.max(b_delay).saturating_sub(1)
    );
    assert!(
        output
            .entity_changes
            .contains(&(ORIGIN, EntityChange::Gone(a.key)))
    );
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::EntityTracked(tracked) if tracked.entity == b.key && tracked.data.is_some()
    )));

    // Of different items, or more than a stack between them, nothing merges.
    for (first, second) in [
        (
            ItemStack { count: 3, ..DIRT },
            ItemStack { count: 5, ..STONE },
        ),
        (
            ItemStack { count: 60, ..DIRT },
            ItemStack { count: 5, ..DIRT },
        ),
    ] {
        let (mut region, a, b) = side_by_side(first, second);
        for _ in 0..80 {
            idle(&mut region);
        }
        assert!(record(&region, a.key).is_some() && record(&region, b.key).is_some());
    }
    // The larger takes the smaller, whichever ticks first.
    let (mut region, a, b) = side_by_side(
        ItemStack { count: 5, ..DIRT },
        ItemStack { count: 3, ..DIRT },
    );
    for _ in 0..40 {
        idle(&mut region);
    }
    assert_eq!(record(&region, b.key), None);
    let EntityData::Item { stack, .. } = record(&region, a.key).unwrap().data else {
        panic!();
    };
    assert_eq!(stack.count, 8);
}

#[test]
fn an_item_despawns_at_the_age_of_6000() {
    let mut region = on_ticking_floor_with(&[]);
    let mut old = item(1, [8.5, -60.0, 8.5], [0.0; 3], true, DIRT);
    old.age = 5998;
    lay(&mut region, &old);
    idle(&mut region);
    assert_eq!(record(&region, old.key).unwrap().age, 5999);
    let output = idle(&mut region);
    assert_eq!(record(&region, old.key), None);
    assert!(output.events.contains(&RegionEvent::EntityGone {
        entity: old.key,
        chunk: ORIGIN,
        hops: 0,
    }));
}

/// An item of `stack` at the player's feet, picked up after `delay` ticks.
fn at_the_feet(stack: ItemStack, delay: u16) -> (Region, Entity) {
    let mut region = on_ticking_floor_with(&[]);
    let mut lying = item(1, [0.5, -60.0, 1.5], [0.0; 3], true, stack);
    if let EntityData::Item { pick_up_delay, .. } = &mut lying.data {
        *pick_up_delay = delay;
    }
    lay(&mut region, &lying);
    (region, lying)
}

fn slot(region: &Region, slot: usize) -> Option<ItemStack> {
    region.state().players[&player(1)].inventory.0[slot]
}

#[test]
fn a_player_picks_an_item_up_after_its_delay_into_the_slot_the_game_chooses() {
    // Dirt goes into the dirt of slot 1: the selected slot (stone) and the off hand
    // (empty) do not take it, and the first of the main slots with dirt and room does.
    let five = ItemStack { count: 5, ..DIRT };
    let (mut region, lying) = at_the_feet(five, 3);
    for _ in 0..2 {
        let output = idle(&mut region);
        assert!(
            !output
                .events
                .iter()
                .any(|event| matches!(event, RegionEvent::ItemTaken { .. }))
        );
        assert!(record(&region, lying.key).is_some());
    }
    // The delay runs out in the item's third tick, and the player, after the entities,
    // takes it.
    let output = idle(&mut region);
    let taken = RegionEvent::ItemTaken {
        item: lying.key,
        collector: EntityId(1),
        count: 5,
        chunk: ORIGIN,
    };
    let at = output
        .events
        .iter()
        .position(|event| *event == taken)
        .unwrap();
    let gone = RegionEvent::EntityGone {
        entity: lying.key,
        chunk: ORIGIN,
        hops: 0,
    };
    let gone_at = output
        .events
        .iter()
        .position(|event| *event == gone)
        .unwrap();
    assert!(at < gone_at);
    assert_eq!(record(&region, lying.key), None);
    assert_eq!(slot(&region, 1), Some(ItemStack { count: 6, ..DIRT }));
    assert!(output.player_events.contains(&(
        player(1),
        PlayerEvent::InventorySlot {
            slot: 1,
            stack: Some(ItemStack { count: 6, ..DIRT }),
        }
    )));
    assert!(output.delta.players.iter().any(|(id, _)| *id == player(1)));

    // An item the player has none of goes into the first empty main slot, 3.
    let sand = ItemStack {
        item: items::SAND,
        count: 2,
    };
    let (mut region, _) = at_the_feet(sand, 0);
    idle(&mut region);
    assert_eq!(slot(&region, 3), Some(sand));
}

#[test]
fn a_creative_player_takes_a_whole_stack_and_what_does_not_fit_is_destroyed() {
    let (mut region, lying) = at_the_feet(ItemStack { count: 64, ..DIRT }, 0);
    // Every main slot full of stone but slot 1's dirt, at 60 of 64.
    if let Some(player) = region.players.get_mut(&player(1)) {
        for slot in 0..36 {
            player.inventory.0[slot] = Some(ItemStack { count: 64, ..STONE });
        }
        player.inventory.0[1] = Some(ItemStack { count: 60, ..DIRT });
    }
    let output = idle(&mut region);
    assert!(output.events.contains(&RegionEvent::ItemTaken {
        item: lying.key,
        collector: EntityId(1),
        count: 64,
        chunk: ORIGIN,
    }));
    assert_eq!(slot(&region, 1), Some(ItemStack { count: 64, ..DIRT }));
    assert_eq!(slot(&region, clustine_world::OFF_HAND), None);
    assert_eq!(record(&region, lying.key), None);
}

#[test]
fn a_region_and_its_restore_move_the_same_items_tick_for_tick() {
    let mut original = on_ticking_floor_with(&[
        on((8, -60, 8), "minecraft:slime_block"),
        on((10, -60, 8), "minecraft:water[level=0]"),
        on((12, -61, 8), "minecraft:ice"),
    ]);
    let items = [
        item(1, [8.5, -56.0, 8.5], [0.0; 3], false, DIRT),
        item(2, [10.5, -59.0, 8.5], [0.02, 0.0, 0.0], false, DIRT),
        item(3, [12.5, -60.0, 8.5], [0.3, 0.0, 0.1], true, STONE),
        item(4, [5.5, -58.0, 5.5], [0.1, 0.2, -0.1], false, STONE),
    ];
    for entity in &items {
        lay(&mut original, entity);
    }
    for _ in 0..7 {
        idle(&mut original);
    }
    let mut copy = super::scheduled_ticks::restored(&original);
    for tick in 0..60 {
        let (a, b) = (idle(&mut original), idle(&mut copy));
        assert_eq!(a.entity_changes, b.entity_changes, "tick {tick}");
        assert_eq!(a.entities_ageing, b.entities_ageing, "tick {tick}");
        for entity in &items {
            assert_eq!(
                record(&original, entity.key),
                record(&copy, entity.key),
                "tick {tick}"
            );
        }
    }
}

#[test]
fn an_item_stops_at_the_edge_of_the_chunks_the_region_ticks() {
    // Only chunk (0, 0) ticks: an item sliding east at x = 15.95 would cross into
    // chunk (1, 0); it keeps its x and loses its motion along x, and goes on along z.
    let mut region = on_ticking_floor_with(&[]);
    let sliding = item(1, [15.95, -60.0, 8.5], [0.2, 0.0, 0.1], true, DIRT);
    lay(&mut region, &sliding);
    let output = idle(&mut region);
    let stopped = record(&region, sliding.key).unwrap();
    assert_eq!(stopped.position[0], 15.95);
    assert_eq!(stopped.motion[0], 0.0);
    // Along z it moved by its motion, and the ground's friction took from it.
    let ground = f64::from(0.98_f32 * 0.6);
    assert_eq!(stopped.position[2].to_bits(), (8.5 + 0.1_f64).to_bits());
    assert_eq!(stopped.motion[2].to_bits(), (0.1 * ground).to_bits());
    assert_eq!(region.entity_chunks[&sliding.key], ORIGIN);
    assert!(
        output
            .entity_changes
            .iter()
            .all(|(chunk, _)| *chunk == ORIGIN)
    );
    // Thrown at the line, it stays there for good.
    for _ in 0..20 {
        idle(&mut region);
    }
    assert_eq!(record(&region, sliding.key).unwrap().position[0], 15.95);
}

#[test]
fn an_item_that_moves_into_another_chunk_of_the_region_is_gone_from_one_and_whole_in_the_other() {
    // A region whose chunks from x = -1 to 2 are loaded, so that (0, 0) and (1, 0) tick.
    let mut region = joined_in(ChunkArea::EVERYWHERE, &[1]);
    let around: Vec<ChunkPos> = (-1..=2)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect();
    region.tick(&tickets(around.clone(), vec![]));
    region.tick(&TickInputs {
        chunks_loaded: around.iter().map(|position| (*position, floor())).collect(),
        ..TickInputs::default()
    });
    let sliding = item(1, [15.95, -60.0, 8.5], [0.2, 0.0, 0.0], true, DIRT);
    lay(&mut region, &sliding);
    let output = idle(&mut region);
    let east = ChunkPos::new(1, 0);
    let [
        (from, EntityChange::Gone(gone)),
        (to, EntityChange::Whole(whole)),
    ] = &output.entity_changes[..]
    else {
        panic!("{:?}", output.entity_changes);
    };
    assert_eq!((*from, *gone, *to), (ORIGIN, sliding.key, east));
    assert_eq!(whole.position[0].to_bits(), (15.95 + 0.2_f64).to_bits());
    assert_eq!(region.entity_chunks[&sliding.key], east);
    assert!(region.chunk(ORIGIN).unwrap().entities().is_empty());
    assert_eq!(record(&region, sliding.key).as_ref(), Some(whole));
}

#[test]
fn a_resting_item_commits_nothing_and_names_its_chunk_among_those_that_age() {
    let mut region = on_ticking_floor_with(&[]);
    let lying = item(1, [8.5, -60.0, 8.5], [0.0; 3], true, DIRT);
    lay(&mut region, &lying);
    for tick in 0..50 {
        let output = idle(&mut region);
        assert!(output.entity_changes.is_empty(), "tick {tick}");
        assert_eq!(output.entities_ageing, [ORIGIN], "tick {tick}");
        // The edges hear of it only where its motion changed by more than 0.1 in the
        // tick (`needsSync`): at each fourth tick, whose move cuts back the 0.12 of
        // gravity that three ticks gave it.
        let reports: Vec<&Tracked> = output
            .events
            .iter()
            .filter_map(|event| match event {
                RegionEvent::EntityTracked(tracked) => Some(tracked),
                _ => None,
            })
            .collect();
        let moves = (output.tick as i32 + 1) % 4 == 0;
        if moves && tick >= 4 {
            let [report] = reports[..] else {
                panic!("tick {tick}: {reports:?}");
            };
            assert_eq!(
                report.flags & !Tracked::VERTICAL_COLLISION,
                Tracked::NEEDS_SYNC
            );
            assert_eq!(report.position, lying.position);
        } else if !moves {
            assert!(reports.is_empty(), "tick {tick}: {reports:?}");
        }
        let now = record(&region, lying.key).unwrap();
        assert_eq!(now.position, lying.position);
        // Its motion cycles: gravity a tick at a time, cut back by each fourth tick's
        // move (`(tickCount + getId()) % 4 == 0`), and the drag after it.
        assert!(
            now.motion[1] <= 0.0 && now.motion[1] > -0.17,
            "{:?}",
            now.motion
        );
    }
}

/// A region holding the chunks from x = -1 to 0 around the origin, with copies of the
/// chunks at x = 1 (another region's, past a pinned line at x = 16): the origin ticks,
/// its copied neighbours do not. `copy` is the copy of chunk (1, 0).
fn at_a_pinned_line(copy: Chunk) -> Region {
    let mut region = joined_in(ChunkArea::EVERYWHERE, &[1]);
    let ours: Vec<ChunkPos> = (-1..=0)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect();
    region.tick(&tickets(ours.clone(), vec![]));
    let copies: Vec<(ChunkPos, Chunk)> = (-1..=1)
        .map(|z| {
            let position = ChunkPos::new(1, z);
            (position, if z == 0 { copy.clone() } else { floor() })
        })
        .collect();
    region.tick(&TickInputs {
        chunks_loaded: ours.iter().map(|position| (*position, floor())).collect(),
        copy_bases: copies,
        ..TickInputs::default()
    });
    assert!(region.ticks_chunk(ORIGIN));
    assert!(!region.ticks_chunk(ChunkPos::new(1, 0)));
    region
}

#[test]
fn at_a_pinned_line_an_item_meets_the_copy_s_blocks_and_stops_at_the_line() {
    // A wall across the line, in the copy at x = 16: an item sliding east meets it as
    // it would a block of its own region, its box's east side stopping at 16.
    let mut walled = floor();
    walled.set(0, -60, 8, blocks::STONE);
    let mut region = at_a_pinned_line(walled);
    let sliding = item(1, [15.7, -60.0, 8.5], [0.2, 0.0, 0.0], true, DIRT);
    lay(&mut region, &sliding);
    let output = idle(&mut region);
    let stopped = record(&region, sliding.key).unwrap();
    // The game's `coord - maxX`, the box's east side at x + 0.125.
    let east = 15.7_f64 + 0.125;
    assert_eq!(
        stopped.position[0].to_bits(),
        (15.7 + (16.0 - east)).to_bits()
    );
    // The collision costs it its motion along x (`-0.2 * 0`).
    assert_eq!(stopped.motion[0], 0.0);
    let tracked = output
        .events
        .iter()
        .find_map(|event| match event {
            RegionEvent::EntityTracked(tracked) => Some(tracked),
            _ => None,
        })
        .unwrap();
    assert_ne!(tracked.flags & Tracked::HORIZONTAL_COLLISION, 0);

    // With nothing in the way, the line stops it: it keeps its x.
    let mut region = at_a_pinned_line(floor());
    let sliding = item(1, [15.95, -60.0, 8.5], [0.2, 0.0, 0.0], true, DIRT);
    lay(&mut region, &sliding);
    for _ in 0..10 {
        idle(&mut region);
    }
    let held = record(&region, sliding.key).unwrap();
    assert_eq!(held.position[0], 15.95);
    assert_eq!(region.entity_chunks[&sliding.key], ORIGIN);
}

#[test]
fn an_item_of_a_copy_is_neither_merged_with_nor_picked_up() {
    // Dirt lying across the line, in the copy, beside dirt of the region's own: the
    // region's never merges with it, and the copy's record is left as it was.
    let mut copied = floor();
    let theirs = item(9, [16.2, -60.0, 8.5], [0.0; 3], true, DIRT);
    copied.set_entities(vec![theirs.clone()]);
    let mut region = at_a_pinned_line(copied);
    let ours = item(1, [15.9, -60.0, 8.5], [0.0; 3], true, DIRT);
    lay(&mut region, &ours);
    for _ in 0..80 {
        let output = idle(&mut region);
        assert!(output.entity_changes.is_empty());
    }
    let EntityData::Item { stack, .. } = record(&region, ours.key).unwrap().data else {
        panic!();
    };
    assert_eq!(stack.count, 1);
    let copy = region.copy(ChunkPos::new(1, 0)).unwrap();
    assert_eq!(copy.entities(), [theirs]);
}

#[test]
fn copied_water_holds_up_an_item_a_block_from_the_line() {
    // Water across the line, in the copy: an item at x = 15.9, whose box reaches over
    // the line, is in it as in water of the region's own: it rises and is pushed.
    let mut wet = floor();
    wet.set(
        0,
        -60,
        8,
        BlockState::parse("minecraft:water[level=0]").unwrap(),
    );
    let mut region = at_a_pinned_line(wet);
    let floating = item(1, [15.9, -60.0, 8.5], [0.0; 3], true, DIRT);
    lay(&mut region, &floating);
    idle(&mut region);
    let after = record(&region, floating.key).unwrap();
    // Under water it rises by 5e-4 a tick and is not pulled down by gravity (lying on
    // the ground and still, it moves at the fourth tick).
    let rise = f64::from(5.0e-4_f32);
    assert_eq!(after.motion[1].to_bits(), rise.to_bits());
    for _ in 0..4 {
        idle(&mut region);
    }
    let risen = record(&region, floating.key).unwrap();
    assert!(risen.position[1] > -60.0 && !risen.on_ground, "{risen:?}");
}

#[test]
fn a_cobweb_holds_an_item_back_on_its_next_move() {
    let mut region = on_ticking_floor_with(&[on((8, -60, 8), "minecraft:cobweb")]);
    let caught = item(1, [8.5, -59.5, 8.5], [0.2, -0.1, 0.0], false, DIRT);
    lay(&mut region, &caught);
    // The first tick moves it into the web, which holds it (`makeStuckInBlock`, by
    // 0.25 sideways and 0.05 up and down) for its next move: that move is cut so and
    // its motion is lost.
    idle(&mut region);
    let fallen: [f64; 3] = [0.2, -0.1 + -0.04, 0.0];
    let first = record(&region, caught.key).unwrap();
    assert_eq!(first.position[0].to_bits(), (8.5 + fallen[0]).to_bits());
    let motion = [fallen[0] * drag(), fallen[1] * drag()];
    idle(&mut region);
    let held = record(&region, caught.key).unwrap();
    let asked = [motion[0], motion[1] + -0.04];
    assert_eq!(
        held.position[0].to_bits(),
        (first.position[0] + asked[0] * 0.25).to_bits()
    );
    assert_eq!(
        held.position[1].to_bits(),
        (first.position[1] + asked[1] * f64::from(0.05_f32)).to_bits()
    );
    assert_eq!(held.motion, [0.0; 3]);
}
