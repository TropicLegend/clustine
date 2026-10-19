//! Tests of hoppers, hopper carts, and plates and tripwire pressed by entities (step
//! N6e, `docs/groundwork/n6-plan.md`), from the game's rules as the brief's sections 5
//! and 6 and the class dumps have them: `HopperBlockEntity`'s cooldown of 8
//! (`MOVE_ITEM_SPEED`) and its hand-over of 8 or 7, `ejectItems` and `suckInItems`,
//! `HopperBlock.checkPoweredState`, `MinecartHopper`, `BasePressurePlateBlock`,
//! `WeightedPressurePlateBlock` and `TripWireBlock`.
//!
//! The regions are those of the tests of R3a (`redstone.rs`): player 1 at the spawn,
//! (0.5, -60, 0.5), on a stone floor at y = -61, with the chunks around chunk (0, 0)
//! loaded so that it ticks; what is tested stands at x and z of 4 and more, out of the
//! player's way.

use clustine_data::entity_types;
use clustine_world::{Double, Entity, EntityChange, EntityData, Float, Hurt};

use super::redstone::{block_at, state, ticking_with};
use super::*;
use crate::region::stands::MARKER;

const HOPPER: BlockPos = BlockPos::new(5, -60, 5);

fn hopper(facing: &str) -> BlockState {
    state(&format!("minecraft:hopper[enabled=true,facing={facing}]"))
}

fn chest() -> BlockState {
    state("minecraft:chest[facing=north,type=single,waterlogged=false]")
}

fn stack(item: i32, count: i32) -> Option<ItemStack> {
    Some(ItemStack { item, count })
}

fn key(n: u64) -> EntityId {
    EntityId((1 << 40) + n)
}

/// The container at `at` with `slots` set, written back.
fn fill(region: &mut Region, at: BlockPos, slots: &[(usize, Option<ItemStack>)]) {
    let mut container = region.container_at(at).expect("a container");
    for (slot, held) in slots {
        container.slots[*slot] = *held;
    }
    region.set_container(at, container, &mut TickOutput::default());
}

fn slots(region: &Region, at: BlockPos) -> Vec<Option<ItemStack>> {
    region.container_at(at).expect("a container").slots
}

/// How many items of every kind the container at `at` holds.
fn held(region: &Region, at: BlockPos) -> i32 {
    slots(region, at)
        .iter()
        .flatten()
        .map(|stack| stack.count)
        .sum()
}

fn cooldown(region: &Region, at: BlockPos) -> i32 {
    region.hopper_timer(at).expect("a hopper").cooldown
}

/// An item of `stack` at `position`, at rest on the ground, that is never picked up
/// and so never merges either.
fn item(n: u64, position: [f64; 3], stack: ItemStack) -> Entity {
    Entity {
        key: key(n),
        kind: entity_types::ITEM as u16,
        uuid: uuid::Uuid::from_u128(u128::from(n)),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 1,
        fresh_at: 0,
        data: EntityData::Item {
            stack,
            pick_up_delay: 32_767,
            thrower: None,
            health: 5,
            age_offset: 0,
        },
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
    }
}

fn stone(count: i32) -> ItemStack {
    ItemStack {
        item: clustine_data::items::STONE,
        count,
    }
}

fn cart_data() -> EntityData {
    EntityData::Minecart {
        hurt: Hurt::default(),
        display: None,
        display_offset: None,
        flipped: false,
        contents: Vec::new(),
        fuel: 0,
        push: [Double(0.0); 2],
        fuse: -1,
        enabled: true,
    }
}

/// A cart of the type `kind` at rest at `position`.
fn cart(n: u64, kind: i32, position: [f64; 3]) -> Entity {
    Entity {
        kind: kind as u16,
        on_ground: false,
        data: cart_data(),
        ..item(n, position, stone(1))
    }
}

/// An oak boat at rest on the ground at `position`.
fn boat(n: u64, position: [f64; 3]) -> Entity {
    Entity {
        kind: entity_types::OAK_BOAT as u16,
        data: EntityData::Boat {
            hurt: Hurt::default(),
            paddles: [false; 2],
            bubble_time: 0,
            out_of_control: Float(0.0),
            turning: Float(0.0),
            status: [3; 2],
            contents: Vec::new(),
        },
        ..item(n, position, stone(1))
    }
}

/// An armour stand at rest at `position`, with the flags `flags`.
fn armour_stand(n: u64, position: [f64; 3], flags: u8) -> Entity {
    Entity {
        kind: entity_types::ARMOR_STAND as u16,
        data: EntityData::ArmorStand {
            flags,
            disabled_slots: 0,
            pose: Box::new([[Float(0.0); 3]; 6]),
            equipment: Vec::new(),
            health: Float(20.0),
            last_hit: None,
            air: 300,
            frozen: 0,
        },
        ..item(n, position, stone(1))
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

/// Takes `entity` away, as if it had gone.
fn take_away(region: &mut Region, entity: &Entity) {
    region.remove_entity(
        entity.key,
        entity.hops,
        entity.chunk(),
        &mut TickOutput::default(),
    );
}

fn record(region: &Region, key: EntityId) -> Option<Entity> {
    region.find_entity(key).map(|(_, entity)| entity.clone())
}

/// A chest above a hopper that faces a chest east of it: the hopper takes one item
/// from above in its first tick off its cooldown, and from then on every eighth tick
/// (`MOVE_ITEM_SPEED`) it pushes one into the chest it faces and takes the next. The
/// cooldown it waits on is saved in its data as it counts down.
#[test]
fn a_hopper_moves_one_item_every_eight_ticks_from_a_chest_above_into_the_chest_it_faces() {
    let above = BlockPos::new(5, -59, 5);
    let front = BlockPos::new(6, -60, 5);
    let mut region = ticking_with(&[(HOPPER, hopper("east")), (above, chest()), (front, chest())]);
    fill(
        &mut region,
        above,
        &[(0, stack(clustine_data::items::STONE, 64))],
    );
    idle(&mut region);
    assert_eq!(
        slots(&region, HOPPER)[0],
        stack(clustine_data::items::STONE, 1)
    );
    assert_eq!(held(&region, above), 63);
    assert_eq!(held(&region, front), 0);
    assert_eq!(cooldown(&region, HOPPER), 8);
    for tick in 2..=8 {
        idle(&mut region);
        assert_eq!(held(&region, front), 0, "tick {tick}");
        assert_eq!(held(&region, HOPPER), 1, "tick {tick}");
        // Saved as it counts down: what a restore would read.
        let data = region
            .chunk(HOPPER.chunk())
            .and_then(|chunk| {
                let (x, z) = HOPPER.in_chunk();
                chunk
                    .block_entities()
                    .get(&clustine_world::InChunk::new(x, HOPPER.y, z))
                    .map(|entity| entity.data.clone())
            })
            .expect("the hopper's data");
        let saved = crate::region::hoppers::Timer::loaded(&data).cooldown;
        assert_eq!(saved, 9 - tick, "tick {tick}");
        assert_eq!(cooldown(&region, HOPPER), 9 - tick);
    }
    // The ninth: one pushed on and the next taken, in the same move.
    idle(&mut region);
    assert_eq!(held(&region, front), 1);
    assert_eq!(held(&region, HOPPER), 1);
    assert_eq!(held(&region, above), 62);
    for _ in 10..=16 {
        idle(&mut region);
        assert_eq!(held(&region, front), 1);
    }
    idle(&mut region);
    assert_eq!(held(&region, front), 2);
}

/// Power beside a hopper sets its `enabled` false (`checkPoweredState`), and a hopper
/// that is not enabled moves nothing however long it waits; with the power gone it
/// moves again.
#[test]
fn a_hopper_locked_by_redstone_moves_nothing() {
    let above = BlockPos::new(5, -59, 5);
    let front = BlockPos::new(6, -60, 5);
    let beside = BlockPos::new(5, -60, 4);
    let mut region = ticking_with(&[(HOPPER, hopper("east")), (above, chest()), (front, chest())]);
    fill(
        &mut region,
        above,
        &[(0, stack(clustine_data::items::STONE, 64))],
    );
    fill(
        &mut region,
        HOPPER,
        &[(0, stack(clustine_data::items::DIRT, 1))],
    );
    region.set_block(beside, blocks::REDSTONE_BLOCK, &mut TickOutput::default());
    let locked = state("minecraft:hopper[enabled=false,facing=east]");
    assert_eq!(block_at(&region, HOPPER), Some(locked));
    for _ in 0..30 {
        idle(&mut region);
    }
    assert_eq!(held(&region, above), 64);
    assert_eq!(held(&region, front), 0);
    assert_eq!(
        slots(&region, HOPPER)[0],
        stack(clustine_data::items::DIRT, 1)
    );
    region.set_block(beside, blocks::AIR, &mut TickOutput::default());
    assert_eq!(block_at(&region, HOPPER), Some(hopper("east")));
    idle(&mut region);
    assert_eq!(held(&region, front), 1, "the dirt pushed on");
    assert_eq!(held(&region, above), 63, "and a stone taken");
}

/// An item falling into a hopper is taken in its own tick, in the entities' phase
/// (`HopperBlock.entityInside`): the hopper goes on its cooldown of 8 there, and its
/// ticker, later in the same tick, counts it down to 7, where a take by the ticker
/// would have left 8.
#[test]
fn an_item_dropped_on_a_hopper_is_taken_in_in_its_own_tick() {
    let mut region = ticking_with(&[(HOPPER, hopper("east"))]);
    // Above the hopper and falling fast: its tick takes it into the hopper's bowl.
    let mut falling = item(1, [5.5, -58.2, 5.5], stone(3));
    falling.on_ground = false;
    falling.motion = [0.0, -1.0, 0.0];
    lay(&mut region, &falling);
    idle(&mut region);
    assert_eq!(record(&region, falling.key), None, "taken in whole");
    assert_eq!(slots(&region, HOPPER)[0], Some(stone(3)));
    assert_eq!(cooldown(&region, HOPPER), 7);
}

/// A hopper above the right half of a double chest, facing down, fills it as one
/// container of 54 slots: with the right half full, its item goes into the left
/// half's first slot (`ChestBlock.getContainer`, a `CompoundContainer`).
#[test]
fn a_hopper_fills_a_double_chest_through_its_other_half() {
    let left = BlockPos::new(5, -60, 5);
    let right = BlockPos::new(6, -60, 5);
    let above = BlockPos::new(6, -59, 5);
    let half = |kind: &str| {
        state(&format!(
            "minecraft:chest[facing=north,type={kind},waterlogged=false]"
        ))
    };
    let mut region = ticking_with(&[
        (left, half("left")),
        (right, half("right")),
        (above, hopper("down")),
    ]);
    let halves = region.chest_halves(right).expect("a double chest");
    assert_eq!((halves.first, halves.second), (right, Some(left)));
    let full: Vec<(usize, Option<ItemStack>)> = (0..27)
        .map(|slot| (slot, stack(clustine_data::items::STONE, 64)))
        .collect();
    fill(&mut region, right, &full);
    fill(
        &mut region,
        above,
        &[(0, stack(clustine_data::items::DIRT, 1))],
    );
    idle(&mut region);
    assert_eq!(held(&region, above), 0);
    assert_eq!(
        slots(&region, left)[0],
        stack(clustine_data::items::DIRT, 1)
    );
    assert_eq!(held(&region, right), 27 * 64);
}

/// A shulker box takes no shulker box through any face
/// (`ShulkerBoxBlockEntity.canPlaceItemThroughFace`): a hopper holding one before a
/// stone pushes the stone, and keeps the box however long it waits.
#[test]
fn a_hopper_puts_no_shulker_box_into_a_shulker_box() {
    let front = BlockPos::new(6, -60, 5);
    let mut region = ticking_with(&[
        (HOPPER, hopper("east")),
        (front, state("minecraft:shulker_box[facing=up]")),
    ]);
    fill(
        &mut region,
        HOPPER,
        &[
            (0, stack(clustine_data::items::RED_SHULKER_BOX, 1)),
            (1, stack(clustine_data::items::STONE, 1)),
        ],
    );
    idle(&mut region);
    assert_eq!(
        slots(&region, front)[0],
        stack(clustine_data::items::STONE, 1)
    );
    for _ in 0..20 {
        idle(&mut region);
    }
    assert_eq!(held(&region, front), 1);
    assert_eq!(
        slots(&region, HOPPER)[0],
        stack(clustine_data::items::RED_SHULKER_BOX, 1)
    );
}

/// A hopper that feeds an empty hopper sets its cooldown (`tryMoveInItem`): to 7 where
/// the fed one ticked already in this tick, and to 8 where it ticks after, which its
/// own tick counts down to 7. So whichever ticks first, the fed hopper holds the item
/// seven ticks and moves it on in the eighth tick after the feed. Hoppers tick in the
/// order of their places in the chunk, x last, so the one further west ticks first.
#[test]
fn a_hopper_fed_while_empty_moves_the_item_on_seven_ticks_later_whichever_ticked_first() {
    // The fed hopper ticks first (west of its feeder), then it ticks after.
    for (feeder, fed, facing, chest_at) in [(6, 5, "west", 4), (5, 6, "east", 7)] {
        let feeder = BlockPos::new(feeder, -60, 5);
        let fed = BlockPos::new(fed, -60, 5);
        let front = BlockPos::new(chest_at, -60, 5);
        let mut region = ticking_with(&[
            (feeder, hopper(facing)),
            (fed, hopper(facing)),
            (front, chest()),
        ]);
        fill(
            &mut region,
            feeder,
            &[(0, stack(clustine_data::items::STONE, 1))],
        );
        idle(&mut region);
        assert_eq!(held(&region, fed), 1, "fed, facing {facing}");
        assert_eq!(cooldown(&region, fed), 7, "facing {facing}");
        for tick in 2..=7 {
            idle(&mut region);
            assert_eq!(held(&region, fed), 1, "tick {tick}, facing {facing}");
        }
        idle(&mut region);
        assert_eq!(held(&region, front), 1, "facing {facing}");
        assert_eq!(held(&region, fed), 0, "facing {facing}");
    }
}

/// A comparator reading a hopper is told when its slots change: one item in five
/// slots of stone is a signal of 1 (`getRedstoneSignalFromContainer`), from the
/// comparator's tick two ticks after the hopper took it.
#[test]
fn a_comparator_reads_a_hopper_and_is_told_of_its_change() {
    let above = BlockPos::new(5, -59, 5);
    let comparator = BlockPos::new(4, -60, 5);
    let mut region = ticking_with(&[
        (HOPPER, hopper("east")),
        (above, chest()),
        (
            comparator,
            state("minecraft:comparator[facing=east,mode=compare,powered=false]"),
        ),
    ]);
    assert_eq!(region.comparator_output(comparator), 0);
    fill(
        &mut region,
        above,
        &[(0, stack(clustine_data::items::STONE, 64))],
    );
    idle(&mut region);
    assert_eq!(held(&region, HOPPER), 1);
    // The comparator's delay of two ticks.
    idle(&mut region);
    assert_eq!(region.comparator_output(comparator), 0);
    idle(&mut region);
    assert_eq!(region.comparator_output(comparator), 1);
}

/// A hopper cart at rest on a rail takes in an item lying about it after its tick
/// (`MinecartHopper.suckInItems`, its box grown by a quarter sideways); on a powered
/// activator rail it is switched off (`activateMinecart`) and takes nothing, and its
/// `enabled` is committed.
#[test]
fn a_hopper_cart_takes_items_about_it_and_a_powered_activator_rail_stops_it() {
    let rail = state("minecraft:rail[shape=north_south,waterlogged=false]");
    let activator =
        state("minecraft:activator_rail[powered=true,shape=north_south,waterlogged=false]");
    let mut region = ticking_with(&[
        (BlockPos::new(5, -60, 8), rail),
        (BlockPos::new(5, -60, 12), blocks::REDSTONE_BLOCK),
        (BlockPos::new(5, -59, 12), activator),
    ]);
    let rolling = cart(1, entity_types::HOPPER_MINECART, [5.5, -60.0 + 0.0625, 8.5]);
    let lying = item(2, [5.5, -60.0, 8.5], stone(5));
    let stopped = cart(
        3,
        entity_types::HOPPER_MINECART,
        [5.5, -59.0 + 0.0625, 12.5],
    );
    let left = item(4, [5.5, -59.0, 12.5], stone(5));
    for entity in [&rolling, &lying, &stopped, &left] {
        lay(&mut region, entity);
    }
    let output = idle(&mut region);
    assert_eq!(record(&region, lying.key), None, "taken in");
    let contents = |region: &Region, key: EntityId| match record(region, key).map(|e| e.data) {
        Some(EntityData::Minecart {
            contents, enabled, ..
        }) => (contents, enabled),
        other => panic!("{other:?}"),
    };
    assert_eq!(contents(&region, rolling.key), (vec![(0, stone(5))], true));
    for _ in 0..10 {
        idle(&mut region);
    }
    assert!(record(&region, left.key).is_some(), "left lying");
    assert_eq!(contents(&region, stopped.key), (vec![], false));
    // Its switching off was committed in the first tick.
    let committed = output.entity_changes.iter().any(|(_, change)| {
        matches!(change, EntityChange::Set(key, fields) if *key == stopped.key
            && fields.iter().any(|field| matches!(field,
                clustine_world::EntityField::Data(EntityData::Minecart { enabled: false, .. }))))
    });
    assert!(committed);
}

fn powered(region: &Region, at: BlockPos) -> bool {
    block_at(region, at).and_then(|plate| plate.property("powered")) == Some("true")
}

fn power(region: &Region, at: BlockPos) -> Option<String> {
    block_at(region, at)
        .and_then(|plate| plate.property("power"))
        .map(str::to_string)
}

/// A wooden plate's block set is `EVERYTHING`: an item lying on it presses it in the
/// item's tick, and so does a boat.
#[test]
fn a_wooden_plate_is_pressed_by_an_item_and_by_a_boat() {
    let by_item = BlockPos::new(5, -60, 5);
    let by_boat = BlockPos::new(9, -60, 5);
    let plate = state("minecraft:oak_pressure_plate[powered=false]");
    let mut region = ticking_with(&[(by_item, plate), (by_boat, plate)]);
    lay(&mut region, &item(1, [5.5, -60.0, 5.5], stone(1)));
    lay(&mut region, &boat(2, [9.5, -60.0, 5.5]));
    idle(&mut region);
    assert!(powered(&region, by_item));
    assert!(powered(&region, by_boat));
}

/// A stone plate's block set is `MOBS`: it counts living entities only, so an item
/// lying on it does not press it and an armour stand does.
#[test]
fn a_stone_plate_is_pressed_by_an_armour_stand_and_not_by_an_item() {
    let by_item = BlockPos::new(5, -60, 5);
    let by_stand = BlockPos::new(9, -60, 5);
    let plate = state("minecraft:stone_pressure_plate[powered=false]");
    let mut region = ticking_with(&[(by_item, plate), (by_stand, plate)]);
    lay(&mut region, &item(1, [5.5, -60.0, 5.5], stone(1)));
    lay(&mut region, &armour_stand(2, [9.5, -60.0, 5.5], 0));
    for _ in 0..3 {
        idle(&mut region);
    }
    assert!(!powered(&region, by_item));
    assert!(powered(&region, by_stand));
}

/// A weighted plate counts entities, each item one whatever its stack:
/// `ceil(min(n, most) / most * 15)`, the light plate's most 15 and the heavy one's 150
/// (`Blocks`, `bipush 15` and `sipush 150`). For 1, 15 and 16 items: 1, 15 and 15 on
/// the light plate, and 1, 2 (15/150 of 15 is 1.5) and 2 on the heavy one.
#[test]
fn a_weighted_plate_gives_the_share_of_the_entities_on_it() {
    for (plate, counts) in [
        ("light", [(1, "1"), (15, "15"), (16, "15")]),
        ("heavy", [(1, "1"), (15, "2"), (16, "2")]),
    ] {
        for (count, expected) in counts {
            let at = BlockPos::new(5, -60, 5);
            let block = state(&format!(
                "minecraft:{plate}_weighted_pressure_plate[power=0]"
            ));
            let mut region = ticking_with(&[(at, block)]);
            for n in 0..count {
                lay(&mut region, &item(n + 1, [5.5, -60.0, 5.5], stone(10)));
            }
            idle(&mut region);
            assert_eq!(
                power(&region, at).as_deref(),
                Some(expected),
                "{count} on the {plate} plate"
            );
        }
    }
}

fn tripwire(powered: bool) -> BlockState {
    state(&format!(
        "minecraft:tripwire[attached=false,disarmed=false,east=false,north=false,powered={powered},south=false,west=false]"
    ))
}

/// Tripwire is pressed by the entity that walked in (`entityInside`), an item lying on
/// it; its look ten ticks on counts every entity on it (`checkPressed(level, pos)`),
/// so a cart there keeps it down after the item has gone, and with the cart gone too
/// it comes up at the next look.
#[test]
fn tripwire_is_tripped_by_an_item_and_kept_down_by_a_cart() {
    let at = BlockPos::new(5, -60, 5);
    let mut region = ticking_with(&[(at, tripwire(false))]);
    let lying = item(1, [5.5, -60.0, 5.5], stone(1));
    lay(&mut region, &lying);
    idle(&mut region);
    assert_eq!(block_at(&region, at), Some(tripwire(true)));
    let resting = cart(2, entity_types::MINECART, [5.5, -60.0, 5.5]);
    lay(&mut region, &resting);
    take_away(&mut region, &lying);
    for tick in 0..25 {
        idle(&mut region);
        assert_eq!(block_at(&region, at), Some(tripwire(true)), "tick {tick}");
    }
    let resting = record(&region, resting.key).expect("the cart");
    take_away(&mut region, &resting);
    for _ in 0..11 {
        idle(&mut region);
    }
    assert_eq!(block_at(&region, at), Some(tripwire(false)));
}

/// A marker armour stand ignores triggers (`isIgnoringBlockTriggers`): it presses no
/// plate, and no plate's count has it.
#[test]
fn a_marker_armour_stand_presses_nothing() {
    let wooden = BlockPos::new(5, -60, 5);
    let weighted = BlockPos::new(9, -60, 5);
    let mut region = ticking_with(&[
        (wooden, state("minecraft:oak_pressure_plate[powered=false]")),
        (
            weighted,
            state("minecraft:light_weighted_pressure_plate[power=0]"),
        ),
    ]);
    lay(&mut region, &armour_stand(1, [5.5, -60.0, 5.5], MARKER));
    lay(&mut region, &armour_stand(2, [9.5, -60.0, 5.5], MARKER));
    lay(&mut region, &item(3, [9.5, -60.0, 5.5], stone(1)));
    for _ in 0..3 {
        idle(&mut region);
    }
    assert!(!powered(&region, wooden));
    // The item alone: one of fifteen.
    assert_eq!(power(&region, weighted).as_deref(), Some("1"));
}

/// A hopper under a hopper cart pulls from it as from a container above
/// (`getEntityContainer`: a container entity whose box meets the block above, chosen
/// by a draw among those there), one item a move.
#[test]
fn a_hopper_pulls_from_a_cart_resting_on_it() {
    let mut region = ticking_with(&[(HOPPER, hopper("east"))]);
    let mut loaded = cart(1, entity_types::HOPPER_MINECART, [5.5, -59.0, 5.5]);
    if let EntityData::Minecart { contents, .. } = &mut loaded.data {
        *contents = vec![(0, stone(3))];
    }
    lay(&mut region, &loaded);
    idle(&mut region);
    assert_eq!(slots(&region, HOPPER)[0], Some(stone(1)));
    let left = match record(&region, loaded.key).map(|cart| cart.data) {
        Some(EntityData::Minecart { contents, .. }) => contents,
        other => panic!("{other:?}"),
    };
    assert_eq!(left, vec![(0, stone(2))]);
}
