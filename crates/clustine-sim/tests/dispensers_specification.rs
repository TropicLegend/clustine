//! Tests of what dispensers do with what they hold in step N6, written from
//! `docs/groundwork/n6-plan.md` (section 1, decisions 3 and 7) and the porting brief
//! `docs/groundwork/n6-brief.md` (section 4) and the public API alone, by someone who did
//! not write the code: each behaviour of brief 4.4 whose target Clustine has (buckets
//! both ways, flint and steel, boats and minecarts placed, armour put on a stand and on a
//! player, armour stands and shulker boxes placed, glass bottles), and what each does
//! when it cannot.
//!
//! The dispenser stands on the stone floor at (4, -60, 4) facing east, so the block in
//! front is (5, -60, 4). Player 1 stands at (2.5, -60, 2.5) with what the dispenser is to
//! hold in hotbar slot 0 and redstone blocks in slot 1; a redstone block placed north of
//! the dispenser fires it.

mod n6_entities;
mod n6_scene;

use clustine_data::{BlockState, blocks, entity_types, items};
use clustine_sim::TickOutput;
use clustine_sim::api::{ClickKind, Face, ItemStack, MenuKind};
use clustine_world::{BlockPos, EntityData, Vec3};
use n6_entities::*;
use n6_scene::*;

const DISPENSER: BlockPos = BlockPos::new(4, -60, 4);
const FRONT: BlockPos = BlockPos::new(5, -60, 4);
const BESIDE: Vec3 = Vec3::new(2.5, -60.0, 2.5);

fn dispenser() -> BlockState {
    state("minecraft:dispenser[facing=east,triggered=false]")
}

fn hotbar_slot(n: usize) -> i16 {
    i16::try_from(MenuKind::Grid3x3.container_slots() + 27 + n).expect("fits")
}

/// A scene with the dispenser at `at` and `more`, player 1 holding `count` of `item`.
fn scene_at(at: BlockPos, more: &[(BlockPos, BlockState)], item: i32, count: i32) -> Scene {
    let mut blocks = vec![(at, dispenser())];
    blocks.extend_from_slice(more);
    let held = inventory(&[
        (0, stack(item, count)),
        (1, stack(items::REDSTONE_BLOCK, 64)),
    ]);
    Scene::alone(&World::of(&blocks), held, BESIDE)
}

fn scene_with(more: &[(BlockPos, BlockState)], item: i32, count: i32) -> Scene {
    scene_at(DISPENSER, more, item, count)
}

/// Shift-clicks hotbar slot 0 into the dispenser at `at`.
fn fill_at(scene: &mut Scene, at: BlockPos) {
    let (serial, output) = scene.open(at);
    assert_eq!(the_opened(&output, player(1)).kind, MenuKind::Grid3x3);
    scene.menu_click(serial, hotbar_slot(0), 0, ClickKind::QuickMove);
    scene.close(serial);
}

/// What the dispenser at `at` holds, as its menu shows it.
fn holds_at(scene: &mut Scene, at: BlockPos) -> Vec<Option<ItemStack>> {
    let (serial, output) = scene.open(at);
    let opened = the_opened(&output, player(1));
    scene.close(serial);
    opened.slots[..9].to_vec()
}

fn holds(scene: &mut Scene) -> Vec<Option<ItemStack>> {
    holds_at(scene, DISPENSER)
}

/// Fills the dispenser at `at`, powers it with a redstone block north of it, and ticks
/// until it has fired: the output of the tick it fired in.
fn fire_at(scene: &mut Scene, at: BlockPos) -> TickOutput {
    fill_at(scene, at);
    scene.select(1);
    let power = at.offset(0, 0, -1);
    scene.click(power.offset(0, -1, 0), Face::Top);
    assert_eq!(scene.block(power), blocks::REDSTONE_BLOCK);
    for _ in 0..10 {
        let output = scene.idle();
        if level_events(&output)
            .iter()
            .any(|(kind, position, _)| *position == at && (*kind == 1000 || *kind == 1001))
        {
            return output;
        }
    }
    panic!("the dispenser at {at:?} did not fire");
}

fn fire(scene: &mut Scene) -> TickOutput {
    fire_at(scene, DISPENSER)
}

/// How often level event `kind` played at the dispenser in `output`.
fn played(output: &TickOutput, kind: i32) -> usize {
    level_events(output)
        .iter()
        .filter(|(played, at, _)| *played == kind && *at == DISPENSER)
        .count()
}

fn only(item: i32) -> Vec<Option<ItemStack>> {
    let mut slots = vec![None; 9];
    slots[0] = Some(stack(item, 1));
    slots
}

// ------------------------------------------------------------------------------------
// Buckets (brief 4.5).

/// Brief 4.5: a full bucket empties into the block in front, a source, and the empty
/// bucket takes its slot; one 1000 and one 2000.
#[test]
fn a_dispenser_empties_a_water_or_lava_bucket_in_front_and_keeps_the_bucket() {
    for (bucket, fluid) in [
        (items::WATER_BUCKET, blocks::WATER),
        (items::LAVA_BUCKET, blocks::LAVA),
    ] {
        let mut scene = scene_with(&[], bucket, 1);
        let output = fire(&mut scene);
        assert_eq!(scene.block(FRONT), fluid, "a source in front");
        assert_eq!((played(&output, 1000), played(&output, 2000)), (1, 1));
        assert_eq!(holds(&mut scene), only(items::BUCKET));
        assert!(items_lying(&scene.all()).is_empty());
    }
}

/// Brief 4.5: an empty bucket fills from a source of water in front, which is taken.
#[test]
fn a_dispenser_fills_an_empty_bucket_from_water_in_front() {
    let mut scene = scene_with(&[(FRONT, blocks::WATER)], items::BUCKET, 1);
    let output = fire(&mut scene);
    assert_eq!(scene.block(FRONT), blocks::AIR, "the water taken");
    assert_eq!(played(&output, 1000), 1);
    assert_eq!(holds(&mut scene), only(items::WATER_BUCKET));
}

/// Brief 4.5: an empty bucket with nothing to fill from in front is dropped, as the
/// default behaviour drops one item.
#[test]
fn an_empty_bucket_with_nothing_to_fill_from_is_dropped() {
    let mut scene = scene_with(&[], items::BUCKET, 1);
    let output = fire(&mut scene);
    assert_eq!(played(&output, 1000), 1);
    assert_eq!(lying_count(&scene.all(), items::BUCKET), 1);
    assert_eq!(holds(&mut scene), vec![None; 9]);
}

/// Brief 4.0 item 4 and 4.5: a full bucket that cannot be emptied (a stone block in
/// front) is dropped by the inner default's whole `dispense`, and the outer plays its
/// sound and smoke again: 1000 and 2000 twice.
#[test]
fn a_full_bucket_that_cannot_be_emptied_is_dropped_with_the_sound_twice() {
    let mut scene = scene_with(&[(FRONT, blocks::STONE)], items::WATER_BUCKET, 1);
    let output = fire(&mut scene);
    assert_eq!(scene.block(FRONT), blocks::STONE);
    assert_eq!((played(&output, 1000), played(&output, 2000)), (2, 2));
    assert_eq!(lying_count(&scene.all(), items::WATER_BUCKET), 1);
}

// ------------------------------------------------------------------------------------
// Flint and steel (brief 4.11).

/// Brief 4.11: flint and steel sets fire in the air in front, and stays in its slot.
#[test]
fn a_dispenser_of_flint_and_steel_lights_a_fire_in_front() {
    let mut scene = scene_with(&[], items::FLINT_AND_STEEL, 1);
    let output = fire(&mut scene);
    assert_eq!(
        scene.block(FRONT).block_id(),
        blocks::FIRE.block_id(),
        "fire"
    );
    assert_eq!(played(&output, 1000), 1);
    assert_eq!(holds(&mut scene), only(items::FLINT_AND_STEEL));
}

/// Brief 4.11 step 4: flint and steel primes a TNT block in front, which is removed;
/// the primed TNT stands at the block's bottom centre with no owner.
#[test]
fn a_dispenser_of_flint_and_steel_primes_tnt_in_front() {
    let mut scene = scene_with(&[(FRONT, blocks::TNT)], items::FLINT_AND_STEEL, 1);
    fire(&mut scene);
    assert_eq!(scene.block(FRONT), blocks::AIR);
    let tnt = scene.of_kind(entity_types::TNT);
    assert_eq!(tnt.len(), 1);
    assert!(
        (tnt[0].position[0] - 5.5).abs() < 0.1,
        "{:?}",
        tnt[0].position
    );
    assert!(
        (tnt[0].position[2] - 4.5).abs() < 0.1,
        "{:?}",
        tnt[0].position
    );
    match &tnt[0].data {
        EntityData::PrimedTnt { owner, .. } => assert_eq!(*owner, None),
        other => panic!("{other:?}"),
    }
}

// ------------------------------------------------------------------------------------
// Boats and minecarts (brief 4.7, 4.8).

/// Brief 4.7: a boat is placed onto water in front, 1.25 blocks out along the facing at
/// the dispenser's centre height plus one; no boat item is dropped.
#[test]
fn a_dispenser_places_a_boat_onto_water_in_front() {
    let mut scene = scene_with(&[(FRONT, blocks::WATER)], items::OAK_BOAT, 1);
    let output = fire(&mut scene);
    let boats = scene.of_kind(entity_types::OAK_BOAT);
    assert_eq!(boats.len(), 1);
    let spawned: Vec<_> = output
        .events
        .iter()
        .filter_map(|event| match event {
            clustine_sim::api::RegionEvent::EntitySpawned(state) => Some(state.pose.position),
            _ => None,
        })
        .collect();
    assert_eq!(spawned.len(), 1, "{:?}", output.events);
    // (4.5 + 1.25, -59.5 + 1.0, 4.5), as it was made, before it moved.
    let at = spawned[0];
    assert!(
        (at.x - 5.75).abs() < 0.2 && (at.y + 58.5).abs() < 0.2 && (at.z - 4.5).abs() < 0.2,
        "{at:?}"
    );
    assert_eq!(played(&output, 1000), 1);
    assert_eq!(lying_count(&scene.all(), items::OAK_BOAT), 0);
}

/// Brief 4.7: with neither water in front nor air over water, the boat is dropped by
/// the inner default's whole `dispense`: the sound twice.
#[test]
fn a_boat_with_no_water_in_front_is_dropped() {
    let mut scene = scene_with(&[], items::OAK_BOAT, 1);
    let output = fire(&mut scene);
    assert!(scene.of_kind(entity_types::OAK_BOAT).is_empty());
    assert_eq!(lying_count(&scene.all(), items::OAK_BOAT), 1);
    assert_eq!(played(&output, 1000), 2);
}

/// Brief 4.8: a minecart is placed onto a rail in front.
#[test]
fn a_dispenser_places_a_minecart_onto_a_rail_in_front() {
    let rail = state("minecraft:rail[shape=east_west]");
    let mut scene = scene_with(&[(FRONT, rail)], items::MINECART, 1);
    let output = fire(&mut scene);
    let carts = scene.of_kind(entity_types::MINECART);
    assert_eq!(carts.len(), 1);
    assert!(
        (carts[0].position[2] - 4.5).abs() < 0.2,
        "{:?}",
        carts[0].position
    );
    assert!(carts[0].position[0] > 5.0, "{:?}", carts[0].position);
    assert_eq!(played(&output, 1000), 1);
    assert_eq!(lying_count(&scene.all(), items::MINECART), 0);
}

/// Brief 4.8: with no rail in front or below it, the minecart is dropped.
#[test]
fn a_minecart_with_no_rail_in_front_is_dropped() {
    let mut scene = scene_with(&[], items::MINECART, 1);
    fire(&mut scene);
    assert!(scene.of_kind(entity_types::MINECART).is_empty());
    assert_eq!(lying_count(&scene.all(), items::MINECART), 1);
}

// ------------------------------------------------------------------------------------
// Armour and armour stands (brief 4.0 item 5, 4.6, 4.9).

/// Brief 4.9: armour goes on the first living entity in front that can wear it, an
/// armour stand among them; nothing is dropped.
#[test]
fn a_dispenser_puts_a_helmet_on_an_armour_stand_in_front() {
    let world =
        World::of(&[(DISPENSER, dispenser())]).with_entities(&[stand(1, [5.5, -60.0, 4.5])]);
    let held = inventory(&[
        (0, stack(items::IRON_HELMET, 1)),
        (1, stack(items::REDSTONE_BLOCK, 64)),
    ]);
    let mut scene = Scene::alone(&world, held, BESIDE);
    fire(&mut scene);
    let stands = scene.of_kind(entity_types::ARMOR_STAND);
    assert_eq!(stands.len(), 1);
    match &stands[0].data {
        EntityData::ArmorStand { equipment, .. } => assert!(
            equipment
                .iter()
                .any(|(_, worn)| *worn == stack(items::IRON_HELMET, 1)),
            "{equipment:?}"
        ),
        other => panic!("{other:?}"),
    }
    assert!(items_lying(&scene.all()).is_empty());
}

/// Brief 4.0 item 5 and 4.9: a player standing in front of a dispenser of armour has it
/// put on, into their head slot (39), when that slot is empty.
#[test]
fn a_dispenser_puts_a_helmet_on_a_player_standing_in_front() {
    let mut scene = scene_with(&[], items::IRON_HELMET, 1);
    let them = scene.join(2, Vec3::new(5.5, -60.0, 4.5));
    fire(&mut scene);
    let theirs = scene
        .region
        .player_state(scene.actors[them].id)
        .expect("player 2")
        .inventory;
    assert_eq!(theirs.0[39], Some(stack(items::IRON_HELMET, 1)));
    assert!(items_lying(&scene.all()).is_empty());
}

/// Brief 4.9: with nobody in front to wear it, the armour is dropped.
#[test]
fn armour_with_nobody_in_front_is_dropped() {
    let mut scene = scene_with(&[], items::IRON_HELMET, 1);
    fire(&mut scene);
    assert_eq!(lying_count(&scene.all(), items::IRON_HELMET), 1);
}

/// Brief 4.6: an armour stand is placed at the front block's bottom centre, turned to
/// the facing's yaw (east: 270), whatever is there.
#[test]
fn a_dispenser_places_an_armour_stand_in_front_facing_its_way() {
    let mut scene = scene_with(&[], items::ARMOR_STAND, 1);
    fire(&mut scene);
    let stands = scene.of_kind(entity_types::ARMOR_STAND);
    assert_eq!(stands.len(), 1);
    let at = stands[0].position;
    assert!(
        (at[0] - 5.5).abs() < 1e-6 && (at[1] + 60.0).abs() < 1e-6 && (at[2] - 4.5).abs() < 1e-6,
        "{at:?}"
    );
    assert_eq!(stands[0].yaw.rem_euclid(360.0), 270.0);
    assert_eq!(lying_count(&scene.all(), items::ARMOR_STAND), 0);
}

// ------------------------------------------------------------------------------------
// Shulker boxes (brief 4.15).

/// Brief 4.15: a shulker box is placed in front, facing up where the block under the
/// front is not empty.
#[test]
fn a_dispenser_places_a_shulker_box_in_front_facing_up_over_a_block() {
    let mut scene = scene_with(&[], items::SHULKER_BOX, 1);
    let output = fire(&mut scene);
    assert_eq!(
        scene.block(FRONT),
        state("minecraft:shulker_box[facing=up]")
    );
    assert_eq!(played(&output, 1000), 1);
    assert_eq!(holds(&mut scene), vec![None; 9]);
}

/// Brief 4.15: over an empty block the box faces the dispenser's way.
#[test]
fn a_shulker_box_dispensed_over_nothing_faces_the_dispensers_way() {
    let raised = DISPENSER.offset(0, 1, 0);
    let mut scene = scene_at(
        raised,
        &[
            (DISPENSER, blocks::STONE),
            (raised.offset(0, -1, -1), blocks::STONE),
        ],
        items::SHULKER_BOX,
        1,
    );
    fire_at(&mut scene, raised);
    assert_eq!(
        scene.block(raised.offset(1, 0, 0)),
        state("minecraft:shulker_box[facing=east]")
    );
}

/// Brief 4.15: a box that cannot be placed stays in the dispenser, with the failure's
/// click (1001), and is not dropped.
#[test]
fn a_shulker_box_that_cannot_be_placed_stays_with_the_failures_click() {
    let mut scene = scene_with(&[(FRONT, blocks::STONE)], items::SHULKER_BOX, 1);
    let output = fire(&mut scene);
    assert_eq!(played(&output, 1001), 1);
    assert_eq!(played(&output, 1000), 0);
    assert_eq!(holds(&mut scene), only(items::SHULKER_BOX));
    assert!(items_lying(&scene.all()).is_empty());
}

// ------------------------------------------------------------------------------------
// Bottles (brief 4.14).

/// Brief 4.14: a glass bottle facing water is filled (a water potion takes its slot),
/// and the water stays.
#[test]
fn a_dispenser_fills_a_glass_bottle_from_water_in_front() {
    let mut scene = scene_with(&[(FRONT, blocks::WATER)], items::GLASS_BOTTLE, 1);
    let output = fire(&mut scene);
    assert_eq!(scene.block(FRONT), blocks::WATER, "the water stays");
    assert_eq!(played(&output, 1000), 1);
    assert_eq!(holds(&mut scene), only(items::POTION));
}

/// Brief 4.14: with two bottles the potion goes into another slot, and one bottle
/// stays (`consumeWithRemainder`).
#[test]
fn a_second_bottle_stays_and_the_potion_goes_beside_it() {
    let mut scene = scene_with(&[(FRONT, blocks::WATER)], items::GLASS_BOTTLE, 2);
    fire(&mut scene);
    let slots = holds(&mut scene);
    assert_eq!(slots[0], Some(stack(items::GLASS_BOTTLE, 1)));
    assert_eq!(slots[1], Some(stack(items::POTION, 1)));
}

/// Brief 4.14: a glass bottle with no water in front is dropped, with the failure's
/// click (1001).
#[test]
fn a_glass_bottle_with_no_water_in_front_is_dropped_with_the_failures_click() {
    let mut scene = scene_with(&[], items::GLASS_BOTTLE, 1);
    let output = fire(&mut scene);
    assert_eq!(played(&output, 1001), 1);
    assert_eq!(lying_count(&scene.all(), items::GLASS_BOTTLE), 1);
}

// ------------------------------------------------------------------------------------
// The edges of the behaviours (brief 4.3's `consumeWithRemainder`, 4.5, 4.7, 4.9).

/// Brief 4.3 and 4.5: an empty bucket of two fills one; the full bucket goes into the
/// dispenser's first slot that takes it, beside the bucket left.
#[test]
fn of_two_empty_buckets_one_is_filled_and_put_beside_the_other() {
    let mut scene = scene_with(&[(FRONT, blocks::WATER)], items::BUCKET, 2);
    fire(&mut scene);
    let slots = holds(&mut scene);
    assert_eq!(slots[0], Some(stack(items::BUCKET, 1)));
    assert_eq!(slots[1], Some(stack(items::WATER_BUCKET, 1)));
}

/// Brief 4.5: a water bucket emptied into a source of water still succeeds: the set
/// fails, the old state was a source, so the dispenser keeps an empty bucket and the
/// water stays.
#[test]
fn a_water_bucket_emptied_into_water_leaves_an_empty_bucket() {
    let mut scene = scene_with(&[(FRONT, blocks::WATER)], items::WATER_BUCKET, 1);
    let output = fire(&mut scene);
    assert_eq!(scene.block(FRONT), blocks::WATER);
    assert_eq!(played(&output, 1000), 1);
    assert_eq!(holds(&mut scene), only(items::BUCKET));
    assert!(items_lying(&scene.all()).is_empty());
}

/// Brief 4.7: over air with water below the front, the boat is placed with no lift, at
/// the dispenser's centre height.
#[test]
fn a_boat_is_placed_over_air_with_water_below_at_the_dispensers_height() {
    let raised = DISPENSER.offset(0, 1, 0);
    let mut scene = scene_at(
        raised,
        &[
            (DISPENSER, blocks::STONE),
            (raised.offset(0, -1, -1), blocks::STONE),
            (FRONT, blocks::WATER),
        ],
        items::OAK_BOAT,
        1,
    );
    let output = fire_at(&mut scene, raised);
    let spawned: Vec<_> = output
        .events
        .iter()
        .filter_map(|event| match event {
            clustine_sim::api::RegionEvent::EntitySpawned(state) => Some(state.pose.position),
            _ => None,
        })
        .collect();
    assert_eq!(spawned.len(), 1, "{:?}", output.events);
    let at = spawned[0];
    // (4.5 + 1.25, -58.5, 4.5).
    assert!(
        (at.x - 5.75).abs() < 0.2 && (at.y + 58.5).abs() < 0.2 && (at.z - 4.5).abs() < 0.2,
        "{at:?}"
    );
    assert_eq!(lying_count(&scene.all(), items::OAK_BOAT), 0);
}

/// Brief 4.9: armour goes only into an empty slot: a player in front already wearing a
/// helmet is passed over, and with nobody else there the helmet is dropped.
#[test]
fn a_player_in_front_already_wearing_a_helmet_is_passed_over() {
    let mut scene = scene_with(&[], items::IRON_HELMET, 1);
    let them = scene.join(2, Vec3::new(5.5, -60.0, 4.5));
    scene.input_of(
        them,
        clustine_sim::api::PlayerInput::SetInventorySlot {
            slot: 39,
            stack: Some(stack(items::GOLDEN_HELMET, 1)),
        },
    );
    let helmets = |scene: &Scene| {
        scene
            .region
            .player_state(scene.actors[them].id)
            .expect("player 2")
            .inventory
            .0[..36]
            .iter()
            .flatten()
            .filter(|held| held.item == items::IRON_HELMET)
            .count()
    };
    // They came in with the scene's inventory, a helmet in hand.
    let before = helmets(&scene);
    fire(&mut scene);
    let theirs = scene
        .region
        .player_state(scene.actors[them].id)
        .expect("player 2")
        .inventory;
    assert_eq!(theirs.0[39], Some(stack(items::GOLDEN_HELMET, 1)));
    // Dropped with no pick-up delay (brief 4.3) at the player's feet, it may be theirs
    // at once, picked up as any item: not worn.
    let picked = helmets(&scene) - before;
    let lying = usize::try_from(lying_count(&scene.all(), items::IRON_HELMET)).expect("few");
    assert_eq!(picked + lying, 1, "dropped, not worn");
}
