//! Tests of hoppers and hopper carts in step N6, written from `docs/groundwork/n6-plan.md`
//! (section 1) and the porting brief `docs/groundwork/n6-brief.md` (section 5) and the
//! public API alone, by someone who did not write the code: the cooldown a hopper hands
//! on along a chain, whatever the order the hoppers tick in; a hopper pulling from a
//! container cart above it; and a hopper cart on an activator rail.
//!
//! Players stand on a stone floor at y = -61 (feet at y = -60) in chunk (0, 0), whose 5x5
//! is loaded, each with 64 dirt in hotbar slot 0.

mod n6_entities;
mod n6_scene;

use clustine_data::{BlockState, entity_types, items};
use clustine_sim::TickOutput;
use clustine_sim::api::{ClickKind, MenuKind};
use clustine_world::{BlockPos, EntityData, PlayerId, Vec3};
use n6_entities::*;
use n6_scene::*;

fn hotbar_slot(kind: MenuKind, n: usize) -> i16 {
    i16::try_from(kind.container_slots() + 27 + n).expect("fits")
}

fn hopper(facing: &str) -> BlockState {
    state(&format!("minecraft:hopper[enabled=true,facing={facing}]"))
}

fn chest() -> BlockState {
    state("minecraft:chest[facing=north,type=single]")
}

/// The ticks among `outputs` at which the menu of `who` reported its slot 0, with the
/// count it then held.
fn slot_zero(outputs: &[TickOutput], who: PlayerId) -> Vec<(usize, i32)> {
    outputs
        .iter()
        .enumerate()
        .flat_map(|(tick, output)| {
            changed(output, who)
                .into_iter()
                .flat_map(|change| change.slots)
                .filter(|(slot, _)| *slot == 0)
                .map(move |(_, stack)| (tick, stack.map_or(0, |stack| stack.count)))
                .collect::<Vec<_>>()
        })
        .collect()
}

// ------------------------------------------------------------------------------------
// The cooldown along a chain (brief 5.0 item 2, 5.3, 5.4, 5.7).

/// Brief 5.0 item 2, 5.3 and 5.7 step 4: a hopper that pushes into an empty hopper gives
/// it a cooldown of 8, or 7 if that one has ticked already in this game tick, so that
/// whichever ticks first the one fed passes the item on 7 ticks after it came, and the
/// chest at the chain's end gets one every 8. A chain A -> B -> chest, built eastwards
/// and westwards so that B ticks after A in one and before it in the other; player 1
/// watches B, player 2 the chest, and player 3 fills A.
#[test]
fn a_hopper_fed_by_a_hopper_passes_the_item_on_seven_ticks_later_whatever_their_order() {
    for (step, z) in [(1, 6), (-1, 8)] {
        let a = BlockPos::new(5 - step, -60, z);
        let b = BlockPos::new(5, -60, z);
        let c = BlockPos::new(5 + step, -60, z);
        let facing = if step == 1 { "east" } else { "west" };
        let world = World::of(&[(a, hopper(facing)), (b, hopper(facing)), (c, chest())]);
        let held = inventory(&[(0, stack(items::DIRT, 64))]);
        let mut scene = Scene::alone(&world, held, Vec3::new(5.5, -60.0, 10.5));
        scene.join(2, Vec3::new(6.5, -60.0, 10.5));
        scene.join(3, Vec3::new(4.5, -60.0, 10.5));
        let (_, opening) = scene.open_by(0, b);
        assert_eq!(the_opened(&opening, player(1)).kind, MenuKind::Hopper);
        let (_, opening) = scene.open_by(1, c);
        assert_eq!(the_opened(&opening, player(2)).kind, MenuKind::Rows3);
        let (serial, _) = scene.open_by(2, a);
        let mut outputs = vec![scene.menu_click_by(
            2,
            serial,
            hotbar_slot(MenuKind::Hopper, 0),
            0,
            ClickKind::QuickMove,
        )];
        outputs.extend(scene.idle_for(40));
        let at_b = slot_zero(&outputs, player(1));
        let at_c = slot_zero(&outputs, player(2));
        let t0 = at_b.first().expect("B was fed").0;
        let expected_b: Vec<(usize, i32)> = (0..4)
            .flat_map(|n| [(t0 + 8 * n, 1), (t0 + 8 * n + 7, 0)])
            .collect();
        assert_eq!(
            at_b[..expected_b.len().min(at_b.len())],
            expected_b[..],
            "B, facing {facing}"
        );
        let expected_c: Vec<(usize, i32)> =
            (0..4).map(|n| (t0 + 7 + 8 * n, 1 + n as i32)).collect();
        assert_eq!(&at_c[..4], &expected_c[..], "the chest, facing {facing}");
    }
}

/// Brief 5.0 item 2 and 5.7: a hopper pushing into a hopper that is not empty hands on
/// no cooldown: the one fed keeps its own pace of 8.
#[test]
fn a_hopper_fed_while_it_holds_something_keeps_its_own_pace() {
    let a = BlockPos::new(4, -60, 6);
    let b = BlockPos::new(5, -60, 6);
    let c = BlockPos::new(6, -60, 6);
    let world = World::of(&[(a, hopper("east")), (b, hopper("east")), (c, chest())]);
    let held = inventory(&[(0, stack(items::DIRT, 64)), (1, stack(items::STONE, 64))]);
    let mut scene = Scene::alone(&world, held, Vec3::new(5.5, -60.0, 10.5));
    scene.join(2, Vec3::new(6.5, -60.0, 10.5));
    // B starts with stone, A with dirt: B pushes every 8, whatever A gives it.
    let (serial, _) = scene.open_by(0, b);
    scene.select(1);
    scene.menu_click(
        serial,
        hotbar_slot(MenuKind::Hopper, 1),
        0,
        ClickKind::QuickMove,
    );
    let (serial_a, _) = scene.open_by(1, a);
    scene.menu_click_by(
        1,
        serial_a,
        hotbar_slot(MenuKind::Hopper, 0),
        0,
        ClickKind::QuickMove,
    );
    scene.close_by(1, serial_a);
    let (_, opening) = scene.open_by(1, c);
    let start = the_opened(&opening, player(2)).slots[0].map_or(0, |stack| stack.count);
    let outputs = scene.idle_for(50);
    let at_c = slot_zero(&outputs, player(2));
    assert!(at_c.len() >= 5, "{at_c:?}");
    for pair in at_c.windows(2) {
        assert_eq!(pair[1].0 - pair[0].0, 8, "every 8: {at_c:?}");
        assert_eq!(pair[1].1, pair[0].1 + 1);
    }
    assert!(at_c[0].1 > start);
}

/// Brief 4.0 item 10 and 4.18: a dropper putting an item into an empty hopper gives it
/// the cooldown a hopper's own push would (8: the dropper is no hopper), so the hopper
/// passes the item on 7 ticks after it came, not at once. (`region/dispensers.rs` names
/// this as a difference to be joined with the hoppers' `addItem`; neither the plan nor
/// the roadmap names it.)
#[test]
#[ignore = "finding: a dropper's insertion hands an empty hopper no cooldown; it passes the item on at once"]
fn a_hopper_fed_by_a_dropper_waits_its_cooldown_before_passing_the_item_on() {
    let dropper = BlockPos::new(4, -60, 6);
    let fed = BlockPos::new(5, -60, 6);
    let end = BlockPos::new(6, -60, 6);
    let world = World::of(&[
        (
            dropper,
            state("minecraft:dropper[facing=east,triggered=false]"),
        ),
        (fed, hopper("east")),
        (end, chest()),
    ]);
    let held = inventory(&[
        (0, stack(items::DIRT, 64)),
        (1, stack(items::REDSTONE_BLOCK, 64)),
    ]);
    let mut scene = Scene::alone(&world, held, Vec3::new(5.5, -60.0, 10.5));
    scene.join(2, Vec3::new(6.5, -60.0, 10.5));
    scene.join(3, Vec3::new(3.5, -60.0, 8.5));
    scene.open_by(0, fed);
    scene.open_by(1, end);
    let (serial, _) = scene.open_by(2, dropper);
    scene.menu_click_by(
        2,
        serial,
        hotbar_slot(MenuKind::Grid3x3, 0),
        0,
        ClickKind::QuickMove,
    );
    scene.close_by(2, serial);
    scene.input_of(2, clustine_sim::api::PlayerInput::SelectSlot { slot: 1 });
    let mut outputs =
        vec![scene.click_by(2, dropper.offset(0, -1, -1), clustine_sim::api::Face::Top)];
    assert_eq!(
        scene.block(dropper.offset(0, 0, -1)),
        clustine_data::blocks::REDSTONE_BLOCK
    );
    outputs.extend(scene.idle_for(20));
    let at_fed = slot_zero(&outputs, player(1));
    let at_end = slot_zero(&outputs, player(2));
    let t = at_fed
        .first()
        .unwrap_or_else(|| panic!("the hopper was fed: {at_fed:?}, the chest {at_end:?}"))
        .0;
    assert_eq!(&at_fed[..2], &[(t, 1), (t + 7, 0)], "{at_fed:?}");
    assert_eq!(&at_end[..1], &[(t + 7, 1)], "{at_end:?}");
}

// ------------------------------------------------------------------------------------
// Carts (brief 5.0 items 3 and 10, 5.5, 5.6, 5.9).

/// A hopper set into the floor at (4, -61, 4), a rail on it and `cart` on the rail.
fn hopper_under(cart: clustine_world::Entity) -> Scene {
    let world = World::of(&[
        (BlockPos::new(4, -61, 4), hopper("down")),
        (
            BlockPos::new(4, -60, 4),
            state("minecraft:rail[shape=north_south]"),
        ),
    ])
    .with_entities(&[cart]);
    Scene::alone(&world, inventory(&[]), Vec3::new(1.5, -60.0, 1.5))
}

fn cart_now(scene: &Scene) -> clustine_world::Entity {
    scene
        .all()
        .into_iter()
        .find(|entity| entity.key == key(1))
        .expect("the cart")
}

/// Brief 5.0 item 3, 5.5 and 5.6: with no block container above, a hopper pulls from a
/// container entity whose box meets the block above it, one item a move every 8 ticks:
/// a chest cart's and a hopper cart's contents end up in the hopper.
#[test]
fn a_hopper_pulls_from_a_container_cart_above_it_one_item_every_eight_ticks() {
    for kind in [entity_types::CHEST_MINECART, entity_types::HOPPER_MINECART] {
        let mut scene = hopper_under(cart_of(
            1,
            kind,
            [4.5, -60.0, 4.5],
            vec![(0, stack(items::DIRT, 4))],
        ));
        let mut taken = Vec::new();
        let mut last = held_count(&cart_now(&scene));
        for tick in 0..50 {
            scene.idle();
            let now = held_count(&cart_now(&scene));
            if now < last {
                assert_eq!(now, last - 1, "one at a time");
                taken.push(tick);
            }
            last = now;
        }
        assert_eq!(last, 0, "emptied");
        // The first may be taken in the ticks that load the scene, before these.
        assert!(taken.len() == 4 || taken.len() == 3, "{taken:?}");
        for pair in taken.windows(2) {
            assert_eq!(pair[1] - pair[0], 8, "every 8: {taken:?}");
        }
        let (serial, output) = scene.open(BlockPos::new(4, -61, 4));
        assert_eq!(
            the_opened(&output, player(1)).slots[0],
            Some(stack(items::DIRT, 4))
        );
        scene.close(serial);
    }
}

fn enabled(entity: &clustine_world::Entity) -> bool {
    match &entity.data {
        EntityData::Minecart { enabled, .. } => *enabled,
        other => panic!("not a cart: {other:?}"),
    }
}

/// A hopper cart on an activator rail, powered or not, `enabled` as given, with one
/// dirt lying on it.
fn hopper_cart_on_activator(powered: bool, was_enabled: bool) -> Scene {
    let rail = state(&format!(
        "minecraft:activator_rail[powered={powered},shape=north_south]"
    ));
    let mut cart = cart_of(
        1,
        entity_types::HOPPER_MINECART,
        [4.5, -60.0, 4.5],
        Vec::new(),
    );
    if let EntityData::Minecart { enabled, .. } = &mut cart.data {
        *enabled = was_enabled;
    }
    let world = World::of(&[(BlockPos::new(4, -60, 4), rail)])
        .with_entities(&[cart, dirt(2, [4.5, -59.25, 4.5])]);
    Scene::alone(&world, inventory(&[]), Vec3::new(1.5, -60.0, 1.5))
}

/// Brief 5.0 item 10 and 5.9: a hopper cart on a powered activator rail is not enabled,
/// which its record keeps, and takes nothing.
#[test]
fn a_hopper_cart_on_a_powered_activator_rail_takes_nothing() {
    let mut scene = hopper_cart_on_activator(true, true);
    scene.idle_for(20);
    let cart = cart_now(&scene);
    assert!(!enabled(&cart), "disabled by the rail");
    assert_eq!(held_count(&cart), 0);
    assert_eq!(lying_count(&scene.all(), items::DIRT), 1);
}

/// Brief 5.0 item 10 and 5.9: on an unpowered activator rail a hopper cart is enabled
/// again, `!powered` every tick it is on one, and takes the item lying on it.
#[test]
fn a_hopper_cart_on_an_unpowered_activator_rail_is_enabled_and_takes_the_item() {
    for was_enabled in [true, false] {
        let mut scene = hopper_cart_on_activator(false, was_enabled);
        let mut seen = Vec::new();
        assert!(
            scene.idle_until(20, &mut seen, |scene, _| lying_count(
                &scene.all(),
                items::DIRT
            ) == 0),
            "taken, enabled before: {was_enabled}"
        );
        let cart = cart_now(&scene);
        assert!(enabled(&cart));
        assert_eq!(contents(&cart), vec![(0, stack(items::DIRT, 1))]);
    }
}
