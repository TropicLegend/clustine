//! Tests of dispensers and droppers from the game's rules (`docs/groundwork/n6-brief.md`,
//! section 4, read again against the class dumps): the rising edge and its four ticks,
//! quasi-connectivity, the slot drawn, the default drop, each behaviour N6 has, the
//! dropper's insertion, and a dispenser facing across a line. The worlds are those of
//! the tests of TNT (`tnt/tests.rs`): a stone floor at y = -61, the blocks at -60.

use clustine_data::{BlockState, Face, blocks, entity_types, items, sounds};
use clustine_world::{BlockPos, ChunkPos, Entity, EntityData, Float, ItemStack};

use super::super::Region;
use super::super::tnt::tests::{
    add, add_player, at, idle, primed_tnt, record, records, state, west_of_a_line, world,
};
use super::{
    DISPENSED, FAILED, SHOOT_SMOKE, dispense_position, drop_motion, drop_position, insert_item,
    random_slot, y_rot,
};
use crate::api::{RegionEvent, TickOutput};

/// Where the tests' dispensers stand.
const AT: BlockPos = BlockPos::new(2, -60, 2);

fn dispenser(facing: &str) -> BlockState {
    state(&format!(
        "minecraft:dispenser[facing={facing},triggered=false]"
    ))
}

fn dropper(facing: &str) -> BlockState {
    state(&format!(
        "minecraft:dropper[facing={facing},triggered=false]"
    ))
}

fn stack(item: i32, count: i32) -> Option<ItemStack> {
    Some(ItemStack { item, count })
}

/// A world with `block` at [`AT`] and what `more` names, the floor everywhere else.
fn with(block: BlockState, more: &[(BlockPos, BlockState)]) -> Region {
    let more = more.to_vec();
    world(7, move |pos| {
        if pos == AT {
            return Some(block);
        }
        more.iter()
            .find(|(at, _)| *at == pos)
            .map(|(_, state)| *state)
    })
}

/// The container at `pos` given `slots`.
fn fill(region: &mut Region, pos: BlockPos, slots: &[(usize, Option<ItemStack>)]) {
    let mut container = region.container_at(pos).expect("a container");
    for (slot, held) in slots {
        container.slots[*slot] = *held;
    }
    region.set_container(pos, container, &mut TickOutput::default());
}

fn slots_of(region: &Region, pos: BlockPos) -> Vec<Option<ItemStack>> {
    region.container_at(pos).expect("a container").slots
}

/// The tick of the block at [`AT`], run now.
fn fire(region: &mut Region) -> TickOutput {
    let mut output = TickOutput::default();
    let state = region.block(AT).expect("loaded");
    assert!(region.dispenser_tick(AT, state, &mut output));
    output
}

/// The level events of `output`: kind, where and data.
fn level_events(output: &TickOutput) -> Vec<(i32, BlockPos, i32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::LevelEvent {
                kind,
                position,
                data,
            } => Some((*kind, *position, *data)),
            _ => None,
        })
        .collect()
}

/// The click and smoke of one dispense at [`AT`] facing `facing` (data: the face's
/// `get3DDataValue`, east 5, up 1).
fn clicked(click: i32, facing: Face) -> Vec<(i32, BlockPos, i32)> {
    vec![(click, AT, 0), (SHOOT_SMOKE, AT, facing as i32)]
}

fn heard(output: &TickOutput) -> Vec<i32> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound { sound, .. } => Some(*sound),
            _ => None,
        })
        .collect()
}

/// The items lying in the loaded chunks: the record and its stack.
fn items_lying(region: &Region) -> Vec<(Entity, ItemStack, u16)> {
    records(region)
        .into_iter()
        .filter_map(|entity| match entity.data {
            EntityData::Item {
                stack,
                pick_up_delay,
                ..
            } => Some((entity.clone(), stack, pick_up_delay)),
            _ => None,
        })
        .collect()
}

fn of_kind(region: &Region, kind: i32) -> Vec<Entity> {
    records(region)
        .into_iter()
        .filter(|entity| i32::from(entity.kind) == kind)
        .collect()
}

/// `getRandomSlot`: each slot that holds something draws `nextInt(n)` with `n` counting
/// from 1, and the last to draw 0 is chosen (DispenserBlockEntity.txt 37–70).
#[test]
fn the_slot_drawn_is_the_last_of_the_filled_slots_to_draw_zero() {
    let mut slots = vec![None; 9];
    slots[1] = stack(items::STONE, 1);
    slots[4] = stack(items::DIRT, 1);
    slots[8] = stack(items::SAND, 1);
    // Three draws, of bounds 1, 2 and 3, and none for the empty slots.
    let mut asked = Vec::new();
    let chosen = random_slot(&slots, |bound| {
        asked.push(bound);
        if bound == 2 { 0 } else { 1 }
    });
    assert_eq!(asked, [1, 2, 3]);
    assert_eq!(chosen, Some(4));
    // The first always draws 0 from a bound of 1, so a lone stack is always chosen.
    let chosen = random_slot(&slots, |bound| if bound == 1 { 0 } else { 1 });
    assert_eq!(chosen, Some(1));
    // An empty dispenser draws nothing.
    let mut drew = false;
    assert_eq!(
        random_slot(&[None; 9], |_| {
            drew = true;
            0
        }),
        None
    );
    assert!(!drew);
}

/// `getDispensePosition` is seven tenths out of the middle; `spawnItem` lowers a
/// sideways drop by 0.15625 and a vertical one by 0.125, and its motion is a speed of
/// 0.2 to 0.3 along the facing and 0.2 up, each spread by a triangle of `0.0172275 * 6`
/// (DefaultDispenseItemBehavior.txt 57–135).
#[test]
fn the_default_drop_goes_out_of_the_front_at_the_games_speed() {
    let east = drop_position(dispense_position(AT, Face::East), Face::East);
    assert_eq!(east, [3.2, -59.65625, 2.5]);
    let up = drop_position(dispense_position(AT, Face::Up), Face::Up);
    assert_eq!(up, [2.5, -58.925, 2.5]);
    // The draws of the speed and the three triangles, in the game's order.
    let draws = [0.5, 0.75, 0.25, 0.5, 0.5, 0.0, 1.0];
    let mut next = draws.iter().copied();
    let motion = drop_motion(Face::East, &mut || next.next().expect("seven draws"));
    let spread = 0.017_227_5 * 6.0;
    assert_eq!(motion, [0.25 + spread * 0.5, 0.2, -spread]);
    assert!(next.next().is_none());
}

/// `Direction.toYRot`: up and down are east's 270 (`data2d` of -1, `& 3`).
#[test]
fn a_vertical_dispenser_turns_what_it_places_as_east() {
    assert_eq!(y_rot(Face::South), 0.0);
    assert_eq!(y_rot(Face::West), 90.0);
    assert_eq!(y_rot(Face::North), 180.0);
    assert_eq!(y_rot(Face::East), 270.0);
    assert_eq!(y_rot(Face::Up), 270.0);
    assert_eq!(y_rot(Face::Down), 270.0);
}

/// `insertItem` goes through the slots in order, each empty one or one of the same item
/// taking what it can, up to the item's own most (DispenserBlockEntity.txt 72–132).
#[test]
fn a_remainder_goes_into_the_first_slots_it_fits() {
    let mut slots = vec![None; 9];
    slots[0] = stack(items::STONE, 3);
    slots[2] = stack(items::BUCKET, 15);
    let left = insert_item(
        &mut slots,
        ItemStack {
            item: items::BUCKET,
            count: 3,
        },
    );
    assert_eq!(left, None);
    assert_eq!(slots[1], stack(items::BUCKET, 3));
    assert_eq!(slots[2], stack(items::BUCKET, 15));
    // Where the empty slots are full, the same item's slot takes up to 16.
    let mut slots = vec![stack(items::STONE, 1); 9];
    slots[5] = stack(items::BUCKET, 15);
    let left = insert_item(
        &mut slots,
        ItemStack {
            item: items::BUCKET,
            count: 3,
        },
    );
    assert_eq!(left, stack(items::BUCKET, 2));
    assert_eq!(slots[5], stack(items::BUCKET, 16));
    let mut full = vec![stack(items::STONE, 64); 9];
    let left = insert_item(
        &mut full,
        ItemStack {
            item: items::BUCKET,
            count: 1,
        },
    );
    assert_eq!(left, stack(items::BUCKET, 1));
}

/// A rising edge sets `triggered` and schedules the tick in 4 (DispenserBlock.txt
/// 207–265); power that stays fires nothing more, and power that goes and comes again
/// fires once more.
#[test]
fn a_dispenser_fires_once_per_rising_edge_and_not_while_powered() {
    let mut region = with(dispenser("east"), &[]);
    fill(&mut region, AT, &[(0, stack(items::STONE, 64))]);
    let power = at(2, -60, 1);
    region.set_block(power, blocks::REDSTONE_BLOCK, &mut TickOutput::default());
    assert_eq!(
        region.block(AT),
        Some(state("minecraft:dispenser[facing=east,triggered=true]"))
    );
    let mut fired_at = Vec::new();
    for tick in 1..=12 {
        let output = idle(&mut region);
        if level_events(&output).contains(&(DISPENSED, AT, 0)) {
            fired_at.push(tick);
        }
    }
    assert_eq!(fired_at, [4], "fired in ticks {fired_at:?}");
    assert_eq!(slots_of(&region, AT)[0], stack(items::STONE, 63));
    region.set_block(power, blocks::AIR, &mut TickOutput::default());
    assert_eq!(region.block(AT), Some(dispenser("east")));
    region.set_block(power, blocks::REDSTONE_BLOCK, &mut TickOutput::default());
    let fired = (0..6)
        .filter(|_| level_events(&idle(&mut region)).contains(&(DISPENSED, AT, 0)))
        .count();
    assert_eq!(fired, 1);
    assert_eq!(slots_of(&region, AT)[0], stack(items::STONE, 62));
}

/// `hasNeighborSignal(pos.above())`: power beside the block above fires the dispenser,
/// but only once an update reaches it, as nothing tells it of the power.
#[test]
fn power_beside_the_block_above_fires_a_dispenser_once_it_is_updated() {
    let mut region = with(dispenser("east"), &[]);
    fill(&mut region, AT, &[(0, stack(items::STONE, 2))]);
    region.set_block(
        at(2, -58, 2),
        blocks::REDSTONE_BLOCK,
        &mut TickOutput::default(),
    );
    assert_eq!(region.block(AT), Some(dispenser("east")));
    region.set_block(at(2, -60, 3), blocks::STONE, &mut TickOutput::default());
    assert_eq!(
        region.block(AT),
        Some(state("minecraft:dispenser[facing=east,triggered=true]"))
    );
    let fired = (0..6)
        .filter(|_| level_events(&idle(&mut region)).contains(&(DISPENSED, AT, 0)))
        .count();
    assert_eq!(fired, 1);
}

/// An empty dispenser clicks its failure (1001) and no smoke; so does a dropper
/// (DispenserBlock.txt 59–89, DropperBlock.txt 59–65).
#[test]
fn an_empty_dispenser_or_dropper_clicks_its_failure_and_nothing_else() {
    let mut region = with(dispenser("east"), &[]);
    let output = fire(&mut region);
    assert_eq!(level_events(&output), [(FAILED, AT, 0)]);
    assert!(items_lying(&region).is_empty());
    let mut region = with(dropper("east"), &[]);
    let output = fire(&mut region);
    assert_eq!(level_events(&output), [(FAILED, AT, 0)]);
}

/// The default behaviour drops one of the stack where `spawnItem` puts it, with no
/// pick-up delay, then clicks (1000) and smokes out of its face (2000, data 5 for east).
#[test]
fn a_dispenser_drops_one_item_out_of_its_front_with_its_click_and_smoke() {
    let mut region = with(dispenser("east"), &[]);
    fill(&mut region, AT, &[(3, stack(items::COBBLESTONE, 10))]);
    let output = fire(&mut region);
    assert_eq!(level_events(&output), clicked(DISPENSED, Face::East));
    let [(item, dropped, delay)] = &items_lying(&region)[..] else {
        panic!("one item: {:?}", items_lying(&region));
    };
    assert_eq!(
        *dropped,
        ItemStack {
            item: items::COBBLESTONE,
            count: 1
        }
    );
    assert_eq!(*delay, 0);
    assert_eq!(item.position, [3.2, -59.65625, 2.5]);
    let spread = 0.017_227_5 * 6.0;
    let [x, y, z] = item.motion;
    assert!((0.2 - spread..=0.3 + spread).contains(&x), "{x}");
    assert!((0.2 - spread..=0.2 + spread).contains(&y), "{y}");
    assert!((-spread..=spread).contains(&z), "{z}");
    assert_eq!(slots_of(&region, AT)[3], stack(items::COBBLESTONE, 9));
}

/// Projectiles are dropped as items until N9 (decision 7).
#[test]
fn an_arrow_is_dropped_as_an_item_until_projectiles_come() {
    let mut region = with(dispenser("east"), &[]);
    fill(&mut region, AT, &[(0, stack(items::ARROW, 1))]);
    let output = fire(&mut region);
    assert_eq!(level_events(&output), clicked(DISPENSED, Face::East));
    assert_eq!(items_lying(&region).len(), 1);
    assert_eq!(slots_of(&region, AT)[0], None);
}

/// `$3`: a water bucket empties into the air in front, the empty bucket takes its slot,
/// and the bucket's sound plays there (BucketItem.txt 212–452).
#[test]
fn a_water_bucket_is_emptied_in_front_and_leaves_the_empty_bucket() {
    let front = at(3, -60, 2);
    let mut region = with(dispenser("east"), &[]);
    fill(&mut region, AT, &[(0, stack(items::WATER_BUCKET, 1))]);
    let output = fire(&mut region);
    assert_eq!(region.block(front), Some(blocks::WATER));
    assert_eq!(slots_of(&region, AT)[0], stack(items::BUCKET, 1));
    assert_eq!(heard(&output), [sounds::ITEM_BUCKET_EMPTY]);
    assert_eq!(level_events(&output), clicked(DISPENSED, Face::East));
}

/// A bucket that cannot be emptied (stone in front) is dropped by the whole inner
/// dispense, so it clicks and smokes twice.
#[test]
fn a_bucket_that_cannot_be_emptied_is_dropped_with_two_clicks() {
    let mut region = with(dispenser("east"), &[(at(3, -60, 2), blocks::STONE)]);
    fill(&mut region, AT, &[(0, stack(items::LAVA_BUCKET, 1))]);
    let output = fire(&mut region);
    let mut twice = clicked(DISPENSED, Face::East);
    twice.extend(clicked(DISPENSED, Face::East));
    assert_eq!(level_events(&output), twice);
    assert_eq!(items_lying(&region).len(), 1);
    assert_eq!(slots_of(&region, AT)[0], None);
}

/// `$4`: an empty bucket takes the source in front; the filled bucket takes the slot
/// where the bucket was the last, or goes into another slot where it was not.
#[test]
fn an_empty_bucket_is_filled_from_a_source_in_front() {
    let front = at(3, -60, 2);
    let mut region = with(dispenser("east"), &[(front, blocks::WATER)]);
    fill(&mut region, AT, &[(0, stack(items::BUCKET, 2))]);
    let output = fire(&mut region);
    assert_eq!(region.block(front), Some(blocks::AIR));
    let slots = slots_of(&region, AT);
    assert_eq!(slots[0], stack(items::BUCKET, 1));
    assert_eq!(slots[1], stack(items::WATER_BUCKET, 1));
    assert_eq!(level_events(&output), clicked(DISPENSED, Face::East));
    // No sound of filling: `$4` plays none.
    assert!(heard(&output).is_empty());
}

/// `$6`: TNT is primed in the block in front, at the middle of its floor, with no
/// owner, its sound, and the click of a success.
#[test]
fn a_dispenser_primes_tnt_in_front() {
    let mut region = with(dispenser("east"), &[]);
    fill(&mut region, AT, &[(0, stack(items::TNT, 2))]);
    let output = fire(&mut region);
    let [(tnt, fuse, owner)] = &primed_tnt(&region)[..] else {
        panic!("one primed TNT");
    };
    assert_eq!(tnt.position, [3.5, -60.0, 2.5]);
    assert_eq!((*fuse, *owner), (80, None));
    assert_eq!(heard(&output), [sounds::ENTITY_TNT_PRIMED]);
    assert_eq!(level_events(&output), clicked(DISPENSED, Face::East));
    assert_eq!(slots_of(&region, AT)[0], stack(items::TNT, 1));
}

/// Flint and steel sets fire in the empty block in front; and TNT in front it primes
/// and takes away (FlintAndSteelDispenseItemBehavior.txt 9–109). It stays in its slot.
#[test]
fn flint_and_steel_lights_fire_and_primes_tnt_in_front() {
    let front = at(3, -60, 2);
    let mut region = with(dispenser("east"), &[]);
    fill(&mut region, AT, &[(0, stack(items::FLINT_AND_STEEL, 1))]);
    let output = fire(&mut region);
    assert_eq!(
        region.block(front).map(|s| s.block_id()),
        Some(blocks::FIRE.block_id())
    );
    assert_eq!(level_events(&output), clicked(DISPENSED, Face::East));
    assert_eq!(slots_of(&region, AT)[0], stack(items::FLINT_AND_STEEL, 1));

    let mut region = with(dispenser("east"), &[(front, blocks::TNT)]);
    fill(&mut region, AT, &[(0, stack(items::FLINT_AND_STEEL, 1))]);
    let output = fire(&mut region);
    assert_eq!(region.block(front), Some(blocks::AIR));
    assert_eq!(primed_tnt(&region).len(), 1);
    assert_eq!(level_events(&output), clicked(DISPENSED, Face::East));
}

/// A boat goes a block and a quarter out, a block above the dispenser's middle where
/// the block in front holds water, turned the way the dispenser faces
/// (BoatDispenseItemBehavior.txt 21–141).
#[test]
fn a_boat_is_placed_on_water_in_front() {
    let mut region = with(dispenser("east"), &[(at(3, -60, 2), blocks::WATER)]);
    fill(&mut region, AT, &[(0, stack(items::OAK_BOAT, 1))]);
    let output = fire(&mut region);
    let [boat] = &of_kind(&region, entity_types::OAK_BOAT)[..] else {
        panic!("one boat");
    };
    // 0.5625 and half of 1.375, and `c.y + 1`.
    assert_eq!(boat.position, [2.5 + 1.25, -58.5, 2.5]);
    assert_eq!(boat.yaw, 270.0);
    assert_eq!(level_events(&output), clicked(DISPENSED, Face::East));
    assert_eq!(slots_of(&region, AT)[0], None);
}

/// A cart goes an eighth past the block in front and a tenth up on a flat rail there
/// (MinecartDispenseItemBehavior.txt 21–155); with nothing for it, it is dropped.
#[test]
fn a_minecart_is_placed_on_a_rail_in_front() {
    let rail = state("minecraft:rail[shape=east_west,waterlogged=false]");
    let mut region = with(dispenser("east"), &[(at(3, -60, 2), rail)]);
    fill(&mut region, AT, &[(0, stack(items::MINECART, 1))]);
    let output = fire(&mut region);
    let [cart] = &of_kind(&region, entity_types::MINECART)[..] else {
        panic!("one cart");
    };
    assert_eq!(cart.position, [2.5 + 1.125, -60.0 + 0.1, 2.5]);
    assert_eq!(level_events(&output), clicked(DISPENSED, Face::East));

    let mut region = with(dispenser("east"), &[]);
    fill(&mut region, AT, &[(0, stack(items::MINECART, 1))]);
    let output = fire(&mut region);
    assert!(of_kind(&region, entity_types::MINECART).is_empty());
    assert_eq!(items_lying(&region).len(), 1);
    assert_eq!(level_events(&output).len(), 4);
}

/// Equipment goes on the first living entity in the block in front that can wear it, in
/// its own slot where that is empty (EquipmentDispenseItemBehavior.txt 25–80): an
/// armour stand takes a helmet on its head.
#[test]
fn armour_is_put_on_an_armour_stand_in_front() {
    let mut region = with(dispenser("east"), &[]);
    let data = EntityData::ArmorStand {
        flags: 0,
        disabled_slots: 0,
        pose: Box::new(super::super::stands::DEFAULT_POSE.map(|part| part.map(Float))),
        equipment: Vec::new(),
        health: Float(20.0),
        last_hit: None,
        air: 300,
        frozen: 0,
    };
    let stand = record(
        &mut region,
        entity_types::ARMOR_STAND,
        [3.5, -60.0, 2.5],
        data,
    );
    let key = add(&mut region, stand);
    fill(&mut region, AT, &[(0, stack(items::DIAMOND_HELMET, 2))]);
    let output = fire(&mut region);
    let (_, record) = region.find_entity(key).expect("the stand");
    let EntityData::ArmorStand { equipment, .. } = &record.data else {
        panic!("a stand");
    };
    assert_eq!(
        equipment,
        &vec![(
            5,
            ItemStack {
                item: items::DIAMOND_HELMET,
                count: 1
            }
        )]
    );
    assert_eq!(level_events(&output), clicked(DISPENSED, Face::East));
    assert_eq!(slots_of(&region, AT)[0], stack(items::DIAMOND_HELMET, 1));
    // Its head now full, the next helmet is dropped.
    fire(&mut region);
    assert_eq!(items_lying(&region).len(), 1);
}

/// And a player standing in front has it put on, into their armour slot.
#[test]
fn armour_is_put_on_a_player_in_front() {
    let mut region = with(dispenser("east"), &[]);
    let (id, _) = add_player(&mut region, 1, [3.5, -60.0, 2.5], false);
    fill(&mut region, AT, &[(0, stack(items::IRON_BOOTS, 1))]);
    fire(&mut region);
    let boots = region.players[&id].inventory.0[clustine_world::ARMOUR_START];
    assert_eq!(boots, stack(items::IRON_BOOTS, 1));
    assert_eq!(slots_of(&region, AT)[0], None);
}

/// `$1`: an armour stand in front at the middle of its floor, turned the way the
/// dispenser faces.
#[test]
fn an_armour_stand_is_placed_in_front() {
    let mut region = with(dispenser("north"), &[]);
    fill(&mut region, AT, &[(0, stack(items::ARMOR_STAND, 1))]);
    fire(&mut region);
    let [stand] = &of_kind(&region, entity_types::ARMOR_STAND)[..] else {
        panic!("one stand");
    };
    assert_eq!(stand.position, [2.5, -60.0, 1.5]);
    assert_eq!(stand.yaw, 180.0);
}

/// A shulker box is placed in front, facing up where the block below it is not empty
/// (ShulkerBoxDispenseBehavior.txt 11–68), and where it cannot be placed it stays with
/// the click of a failure.
#[test]
fn a_shulker_box_is_placed_in_front_or_stays() {
    let front = at(3, -60, 2);
    let mut region = with(dispenser("east"), &[]);
    fill(&mut region, AT, &[(0, stack(items::RED_SHULKER_BOX, 1))]);
    let output = fire(&mut region);
    assert_eq!(
        region.block(front),
        Some(state("minecraft:red_shulker_box[facing=up]"))
    );
    assert_eq!(level_events(&output), clicked(DISPENSED, Face::East));
    assert_eq!(slots_of(&region, AT)[0], None);

    let mut region = with(dispenser("east"), &[(front, blocks::STONE)]);
    fill(&mut region, AT, &[(0, stack(items::SHULKER_BOX, 1))]);
    let output = fire(&mut region);
    assert_eq!(level_events(&output), clicked(FAILED, Face::East));
    assert_eq!(slots_of(&region, AT)[0], stack(items::SHULKER_BOX, 1));
}

/// A dropper puts one item into the chest in front, silently (DropperBlock.txt 28–121),
/// into the first slot that is empty or holds the same item below its most; with no
/// container in front it drops it.
#[test]
fn a_dropper_puts_one_item_into_a_chest_in_front_or_drops_it() {
    let chest = at(3, -60, 2);
    let chest_state = state("minecraft:chest[facing=north,type=single,waterlogged=false]");
    let mut region = with(dropper("east"), &[(chest, chest_state)]);
    fill(&mut region, AT, &[(0, stack(items::WATER_BUCKET, 1))]);
    fill(
        &mut region,
        chest,
        &[
            (0, stack(items::DIRT, 64)),
            (2, stack(items::WATER_BUCKET, 1)),
        ],
    );
    let output = fire(&mut region);
    assert!(level_events(&output).is_empty());
    let chest_slots = slots_of(&region, chest);
    // The empty second slot comes before the third's bucket.
    assert_eq!(chest_slots[1], stack(items::WATER_BUCKET, 1));
    assert_eq!(chest_slots[2], stack(items::WATER_BUCKET, 1));
    assert_eq!(slots_of(&region, AT)[0], None);
    // A dropper never empties a bucket: with nothing in front it drops it.
    let mut region = with(dropper("east"), &[]);
    fill(&mut region, AT, &[(0, stack(items::WATER_BUCKET, 1))]);
    let output = fire(&mut region);
    assert_eq!(level_events(&output), clicked(DISPENSED, Face::East));
    assert_eq!(region.block(at(3, -60, 2)), Some(blocks::AIR));
    assert_eq!(items_lying(&region).len(), 1);
}

/// A dropper facing a full container keeps its item and makes no sound.
#[test]
fn a_dropper_facing_a_full_container_keeps_its_item_silently() {
    let barrel = at(3, -60, 2);
    let mut region = with(
        dropper("east"),
        &[(barrel, state("minecraft:barrel[facing=up,open=false]"))],
    );
    fill(&mut region, AT, &[(0, stack(items::STONE, 5))]);
    let full: Vec<(usize, Option<ItemStack>)> =
        (0..27).map(|slot| (slot, stack(items::DIRT, 64))).collect();
    fill(&mut region, barrel, &full);
    let output = fire(&mut region);
    assert!(level_events(&output).is_empty());
    assert_eq!(slots_of(&region, AT)[0], stack(items::STONE, 5));
    assert!(items_lying(&region).is_empty());
}

/// A dispenser at a line whose front is another region's: a behaviour that reads or
/// changes that block fails as an empty dispense does, its item kept; a drop is made
/// here and sent there as an arrival (the plan, section 10 items 1 and 7).
#[test]
fn a_dispenser_facing_across_a_line_fails_or_sends_its_drop_across() {
    let dispenser_at = at(15, -60, 2);
    let mut region = west_of_a_line(7, move |pos| {
        (pos == dispenser_at).then(|| dispenser("east"))
    });
    fill(
        &mut region,
        dispenser_at,
        &[(0, stack(items::WATER_BUCKET, 1))],
    );
    let mut output = TickOutput::default();
    let state = region.block(dispenser_at).expect("loaded");
    assert!(region.dispenser_tick(dispenser_at, state, &mut output));
    assert_eq!(level_events(&output), [(FAILED, dispenser_at, 0)]);
    assert_eq!(
        slots_of(&region, dispenser_at)[0],
        stack(items::WATER_BUCKET, 1)
    );
    assert_eq!(region.block(at(16, -60, 2)), Some(blocks::AIR));
    assert!(output.arrivals.is_empty());

    fill(&mut region, dispenser_at, &[(0, stack(items::STONE, 1))]);
    let mut output = TickOutput::default();
    assert!(region.dispenser_tick(dispenser_at, state, &mut output));
    assert_eq!(
        level_events(&output),
        [(DISPENSED, dispenser_at, 0), (SHOOT_SMOKE, dispenser_at, 5)]
    );
    let [(to, item)] = &output.arrivals[..] else {
        panic!("one arrival: {:?}", output.arrivals);
    };
    assert_eq!(*to, ChunkPos::new(1, 0));
    assert_eq!(item.position, [16.2, -59.65625, 2.5]);
    assert_eq!(slots_of(&region, dispenser_at)[0], None);
}

/// A dropper facing a container across a line fails.
#[test]
fn a_dropper_facing_a_container_across_a_line_fails() {
    let dropper_at = at(15, -60, 2);
    let chest = state("minecraft:chest[facing=north,type=single,waterlogged=false]");
    let mut region = west_of_a_line(7, move |pos| {
        if pos == dropper_at {
            Some(dropper("east"))
        } else if pos == at(16, -60, 2) {
            Some(chest)
        } else {
            None
        }
    });
    fill(&mut region, dropper_at, &[(0, stack(items::STONE, 1))]);
    let mut output = TickOutput::default();
    let state = region.block(dropper_at).expect("loaded");
    assert!(region.dispenser_tick(dropper_at, state, &mut output));
    assert_eq!(level_events(&output), [(FAILED, dropper_at, 0)]);
    assert_eq!(slots_of(&region, dropper_at)[0], stack(items::STONE, 1));
}

/// Honeycomb waxes copper in front, with its sparks (3003); a glass bottle at water is
/// filled and goes into the dispenser where it is not the last.
#[test]
fn honeycomb_waxes_copper_and_a_bottle_is_filled_at_water() {
    let front = at(3, -60, 2);
    let mut region = with(dispenser("east"), &[(front, blocks::COPPER_BLOCK)]);
    fill(&mut region, AT, &[(0, stack(items::HONEYCOMB, 1))]);
    let output = fire(&mut region);
    assert_eq!(region.block(front), Some(blocks::WAXED_COPPER_BLOCK));
    assert!(level_events(&output).contains(&(super::WAX_ON, front, 0)));

    let mut region = with(dispenser("east"), &[(front, blocks::WATER)]);
    fill(&mut region, AT, &[(0, stack(items::GLASS_BOTTLE, 2))]);
    let output = fire(&mut region);
    let slots = slots_of(&region, AT);
    assert_eq!(slots[0], stack(items::GLASS_BOTTLE, 1));
    assert_eq!(slots[1], stack(items::POTION, 1));
    assert_eq!(region.block(front), Some(blocks::WATER));
    assert_eq!(level_events(&output), clicked(DISPENSED, Face::East));
}

/// Glowstone charges an anchor in front by one, and fails at a full one.
#[test]
fn glowstone_charges_a_respawn_anchor_until_it_is_full() {
    let front = at(3, -60, 2);
    let anchor = |charges: u8| state(&format!("minecraft:respawn_anchor[charges={charges}]"));
    let mut region = with(dispenser("east"), &[(front, anchor(3))]);
    fill(&mut region, AT, &[(0, stack(items::GLOWSTONE, 3))]);
    let output = fire(&mut region);
    assert_eq!(region.block(front), Some(anchor(4)));
    assert_eq!(heard(&output), [sounds::BLOCK_RESPAWN_ANCHOR_CHARGE]);
    let output = fire(&mut region);
    assert_eq!(level_events(&output), clicked(FAILED, Face::East));
    assert_eq!(slots_of(&region, AT)[0], stack(items::GLOWSTONE, 2));
}

/// Bone meal grows nothing in N6, and stays with the click of a failure, as the game's
/// does where nothing can grow.
#[test]
fn bone_meal_stays_with_a_failure_until_growth_comes() {
    let mut region = with(dispenser("east"), &[]);
    fill(&mut region, AT, &[(0, stack(items::BONE_MEAL, 4))]);
    let output = fire(&mut region);
    assert_eq!(level_events(&output), clicked(FAILED, Face::East));
    assert_eq!(slots_of(&region, AT)[0], stack(items::BONE_MEAL, 4));
}
