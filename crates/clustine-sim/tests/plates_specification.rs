//! Tests of pressure plates and tripwire with entities in step N6, written from
//! `docs/groundwork/n6-plan.md` (section 1, decisions 10 and 11, section 6 item 6,
//! section 7 item 12, section 8 item 10), the porting brief `docs/groundwork/n6-brief.md`
//! (section 6 and 11.5) and the public API alone, by someone who did not write the code:
//! wooden and weighted plates pressed by every kind of entity, stone plates by living
//! ones only, the weighted plates' counts, a marker stand pressing nothing, tripwire
//! tripped and kept by any entity, and at a pinned line an entity in a copy and a player
//! of the other region keeping a plate pressed and letting it go when they leave.
//!
//! Player 1 stands on a stone floor at y = -61 (feet at y = -60) in chunk (0, 0), whose
//! 5x5 is loaded, at (0.5, -60, 0.5), away from the plates.

mod n6_entities;
mod n6_scene;

use clustine_data::{BlockState, sounds};
use clustine_sim::TickOutput;
use clustine_world::{
    BlockPos, ChunkPos, Due, Effect, EffectId, Effected, Entity, EntityChange, FiledEffect, Vec3,
};
use n6_entities::*;
use n6_scene::*;

const PLATE: BlockPos = BlockPos::new(4, -60, 4);
const AWAY: Vec3 = Vec3::new(0.5, -60.0, 0.5);
/// An entity standing at the plate's centre.
const ON_IT: [f64; 3] = [4.5, -60.0, 4.5];

fn oak() -> BlockState {
    state("minecraft:oak_pressure_plate[powered=false]")
}

fn stone_plate() -> BlockState {
    state("minecraft:stone_pressure_plate[powered=false]")
}

/// The signal of the plate at `at`: 15 or 0 for a plate that is powered or not, the
/// power of a weighted one.
fn signal_at(scene: &Scene, at: BlockPos) -> u8 {
    let state = scene.block(at);
    if let Some(power) = state.property("power") {
        return power.parse().expect("a number");
    }
    match state.property("powered") {
        Some("true") => 15,
        Some(_) => 0,
        None => panic!("{at:?} is no plate: {state:?}"),
    }
}

fn signal(scene: &Scene) -> u8 {
    signal_at(scene, PLATE)
}

/// A scene with `plate` at [`PLATE`] and `entities` in the world.
fn plate_with(plate: BlockState, entities: &[Entity]) -> Scene {
    let world = World::of(&[(PLATE, plate)]).with_entities(entities);
    Scene::alone(&world, inventory(&[]), AWAY)
}

/// Idles until the plate's signal is not 0, for at most `ticks`: whether it was.
fn pressed_within(scene: &mut Scene, ticks: u32) -> bool {
    let mut seen = Vec::new();
    scene.idle_until(ticks, &mut seen, |scene, _| signal(scene) > 0)
}

/// Every kind of entity of brief 6.2's table that is not living, at the plate's centre;
/// falling sand from above it.
fn not_living() -> Vec<(&'static str, Entity)> {
    vec![
        ("an item", dirt(1, ON_IT)),
        ("a minecart", cart(1, ON_IT)),
        ("a boat", boat(1, ON_IT)),
        ("falling sand", falling_sand(1, [4.5, -58.0, 4.5])),
        ("primed TNT", primed(1, ON_IT, 200)),
    ]
}

// ------------------------------------------------------------------------------------
// What presses which plate (brief 6.0 items 1 and 3, 6.2).

/// Brief 6.0 item 1 and 6.2: a wooden plate (`EVERYTHING`) is pressed by an item, a
/// minecart, a boat, a falling block as it passes, primed TNT and an armour stand.
#[test]
fn a_wooden_plate_is_pressed_by_every_kind_of_entity() {
    let mut all = not_living();
    all.push(("an armour stand", stand(1, ON_IT)));
    for (what, entity) in all {
        let mut scene = plate_with(oak(), &[entity]);
        assert!(pressed_within(&mut scene, 20), "{what} presses it");
    }
}

/// Brief 6.1 and 6.2: an item falling onto a wooden plate presses it as it comes in,
/// with the wooden plate's click at the plate, once.
#[test]
fn an_item_falling_onto_a_wooden_plate_presses_it_with_its_click() {
    let mut falling = dirt(1, [4.5, -57.0, 4.5]);
    falling.on_ground = false;
    let mut scene = plate_with(oak(), &[falling]);
    assert_eq!(signal(&scene), 0, "not yet");
    let mut seen = Vec::new();
    assert!(scene.idle_until(30, &mut seen, |scene, _| signal(scene) > 0));
    let clicks: Vec<_> = seen
        .iter()
        .flat_map(|output| sounds(output, sounds::BLOCK_WOODEN_PRESSURE_PLATE_CLICK_ON))
        .collect();
    assert_eq!(clicks.len(), 1, "{clicks:?}");
    assert_eq!(clicks[0].1, 1.0);
}

/// Brief 6.0 items 1 and 3, 6.2: a stone plate (`MOBS`) counts living entities only: an
/// armour stand presses it, and an item, a minecart, a boat, falling sand and primed TNT
/// do not.
#[test]
fn a_stone_plate_is_pressed_by_an_armour_stand_and_by_nothing_that_is_not_living() {
    let mut scene = plate_with(stone_plate(), &[stand(1, ON_IT)]);
    assert!(pressed_within(&mut scene, 20), "an armour stand presses it");
    for (what, entity) in not_living() {
        let mut scene = plate_with(stone_plate(), &[entity]);
        assert!(!pressed_within(&mut scene, 30), "{what} does not press it");
    }
}

/// Brief 6.0 item 1: a marker armour stand ignores block triggers and presses neither a
/// wooden nor a stone plate.
#[test]
fn a_marker_armour_stand_presses_no_plate() {
    for plate in [oak(), stone_plate()] {
        let mut scene = plate_with(plate, &[stand_with(1, ON_IT, MARKER)]);
        assert!(!pressed_within(&mut scene, 30), "{plate:?}");
    }
}

/// Brief 6.0 item 2 and 6.3: a weighted plate counts entities, each item one whatever
/// its stack: the light plate's power is the count up to 15, the heavy plate's
/// ceil(count / 150 x 15).
#[test]
fn a_weighted_plate_counts_the_entities_on_it() {
    let light = state("minecraft:light_weighted_pressure_plate[power=0]");
    let heavy = state("minecraft:heavy_weighted_pressure_plate[power=0]");
    for (plate, count, power) in [
        (light, 1, 1),
        (light, 3, 3),
        (light, 15, 15),
        (light, 17, 15),
        (heavy, 1, 1),
        (heavy, 10, 1),
        (heavy, 11, 2),
    ] {
        // Spread over the plate on a 3 x 3 grid, never merging (n6_entities::item).
        let items: Vec<Entity> = (0..count)
            .map(|n| {
                let x = 4.25 + 0.25 * f64::from(n % 3);
                let z = 4.25 + 0.25 * f64::from((n / 3) % 3);
                dirt(n + 1, [x, -60.0, z])
            })
            .collect();
        let mut scene = plate_with(plate, &items);
        let mut seen = Vec::new();
        assert!(
            scene.idle_until(30, &mut seen, |scene, _| signal(scene) == power),
            "{count} on {plate:?}: power {} against {power}",
            signal(&scene)
        );
        // Recounted by its 10-tick look, it stays.
        scene.idle_for(25);
        assert_eq!(signal(&scene), power, "{count} on {plate:?}, later");
    }
}

/// Brief 6.1 and 6.2: an item keeps a wooden plate pressed through the plate's
/// 20-tick looks while it lies there; once it has despawned the next look lets the plate
/// go, with the wooden plate's click off.
#[test]
fn an_item_keeps_a_wooden_plate_pressed_until_it_is_gone() {
    let mut scene = plate_with(oak(), &[ageing(dirt(1, ON_IT), 50)]);
    assert!(pressed_within(&mut scene, 10));
    for _ in 0..30 {
        scene.idle();
        assert_eq!(signal(&scene), 15, "kept while it lies there");
    }
    let mut seen = Vec::new();
    assert!(
        scene.idle_until(40, &mut seen, |scene, _| scene.all().is_empty()),
        "the item despawned"
    );
    assert!(
        scene.idle_until(22, &mut seen, |scene, _| signal(scene) == 0),
        "let go at the next look"
    );
    assert_eq!(
        seen.iter()
            .map(|output| sounds(output, sounds::BLOCK_WOODEN_PRESSURE_PLATE_CLICK_OFF).len())
            .sum::<usize>(),
        1
    );
}

// ------------------------------------------------------------------------------------
// Tripwire (brief 6.0 item 4, 6.4).

const HOOK_WEST: BlockPos = BlockPos::new(2, -60, 4);
const HOOK_EAST: BlockPos = BlockPos::new(6, -60, 4);
const WIRE_MIDDLE: BlockPos = BlockPos::new(4, -60, 4);

/// Two hooks on stone walls at x = 1 and x = 7, with tripwire from x = 3 to 5 between
/// them, at z = 4, and `entities`.
fn tripwire_with(entities: &[Entity]) -> Scene {
    let mut blocks = vec![
        (BlockPos::new(1, -60, 4), clustine_data::blocks::STONE),
        (BlockPos::new(7, -60, 4), clustine_data::blocks::STONE),
        (
            HOOK_WEST,
            state("minecraft:tripwire_hook[attached=true,facing=east,powered=false]"),
        ),
        (
            HOOK_EAST,
            state("minecraft:tripwire_hook[attached=true,facing=west,powered=false]"),
        ),
    ];
    for x in 3..=5 {
        blocks.push((
            BlockPos::new(x, -60, 4),
            state("minecraft:tripwire[attached=true,east=true,west=true,powered=false]"),
        ));
    }
    let world = World::of(&blocks).with_entities(entities);
    Scene::alone(&world, inventory(&[]), AWAY)
}

fn powered(scene: &Scene, at: BlockPos) -> bool {
    scene.block(at).property("powered") == Some("true")
}

/// Brief 6.0 item 4 and 6.4: tripwire is tripped by any entity that walks in (an item, a
/// minecart), which powers both hooks; its 10-tick look keeps it so while the entity
/// stays, and lets it go once the entity is gone.
#[test]
fn tripwire_is_tripped_by_any_entity_and_kept_while_it_stays() {
    for (what, entity) in [
        ("an item", ageing(dirt(1, ON_IT), 60)),
        ("a minecart", cart(1, ON_IT)),
    ] {
        let mut scene = tripwire_with(&[entity]);
        let mut seen = Vec::new();
        assert!(
            scene.idle_until(20, &mut seen, |scene, _| powered(scene, WIRE_MIDDLE)),
            "{what} trips it"
        );
        scene.idle();
        assert!(powered(&scene, HOOK_WEST), "{what}: the west hook");
        assert!(powered(&scene, HOOK_EAST), "{what}: the east hook");
        for _ in 0..25 {
            scene.idle();
            assert!(powered(&scene, WIRE_MIDDLE), "{what}: kept");
        }
    }
    // The item despawns, and the wire and the hooks are let go.
    let mut scene = tripwire_with(&[ageing(dirt(1, ON_IT), 40)]);
    let mut seen = Vec::new();
    assert!(scene.idle_until(20, &mut seen, |scene, _| powered(scene, WIRE_MIDDLE)));
    assert!(scene.idle_until(40, &mut seen, |scene, _| scene.all().is_empty()));
    assert!(
        scene.idle_until(15, &mut seen, |scene, _| !powered(scene, WIRE_MIDDLE)
            && !powered(scene, HOOK_WEST)
            && !powered(scene, HOOK_EAST)),
        "let go"
    );
}

/// Brief 6.0 item 1 and 6.4: a marker armour stand does not trip tripwire.
#[test]
fn a_marker_armour_stand_does_not_trip_tripwire() {
    let mut scene = tripwire_with(&[stand_with(1, ON_IT, MARKER)]);
    scene.idle_for(30);
    assert!(!powered(&scene, WIRE_MIDDLE));
}

// ------------------------------------------------------------------------------------
// At a pinned line (plan section 6 item 6, section 7 item 12, section 8 item 10;
// brief 11.5).

/// A plate in the west's chunk (0, 0), against the line at x = 16.
const BY_THE_LINE: BlockPos = BlockPos::new(15, -60, 4);
/// Where a minecart east of the line stands with its box over the plate's touch box
/// (x from 15.0625 to 15.9375): its centre in chunk (1, 0), its box from 15.61.
const CART_ACROSS: [f64; 3] = [16.1, -60.0, 4.5];

/// Plan section 6 item 6: a plate's recount counts its copies' entities, so a minecart
/// across the line keeps a plate pressed that an item on this side pressed, after the
/// item is gone and through the 20-tick looks; when the cart is gone from the copy, the
/// next look lets the plate go. Without the cart the plate is let go once the item is
/// gone (the control).
#[test]
fn a_cart_across_the_line_keeps_a_plate_pressed_until_it_leaves_the_copy() {
    for with_cart in [true, false] {
        let mut entities = vec![ageing(dirt(1, [15.5, -60.0, 4.5]), 30)];
        if with_cart {
            entities.push(cart(2, CART_ACROSS));
        }
        let world = World::of(&[(BY_THE_LINE, oak())]).with_entities(&entities);
        let mut scene = Scene::new(Layout::side(true), &world, inventory(&[]), AWAY);
        assert_eq!(
            scene
                .region
                .copy(ChunkPos::new(1, 0))
                .map(|copy| copy.entities().len()),
            Some(usize::from(with_cart)),
            "the copy holds the cart"
        );
        let mut seen = Vec::new();
        assert!(
            scene.idle_until(10, &mut seen, |scene, _| signal_at(scene, BY_THE_LINE) > 0),
            "the item presses it"
        );
        assert!(
            scene.idle_until(40, &mut seen, |scene, _| scene.all().is_empty()),
            "the item despawned"
        );
        if !with_cart {
            assert!(
                scene.idle_until(22, &mut seen, |scene, _| signal_at(scene, BY_THE_LINE) == 0),
                "let go without the cart"
            );
            continue;
        }
        for _ in 0..50 {
            scene.idle();
            assert_eq!(signal_at(&scene, BY_THE_LINE), 15, "kept by the cart");
        }
        scene.region.tick(&clustine_sim::TickInputs {
            copy_entity_updates: vec![(ChunkPos::new(1, 0), vec![EntityChange::Gone(key(2))])],
            ..clustine_sim::TickInputs::default()
        });
        assert!(
            scene.idle_until(22, &mut seen, |scene, _| signal_at(scene, BY_THE_LINE) == 0),
            "let go once the cart has left"
        );
    }
}

/// Plan section 6 item 6: a weighted plate at the line counts the copy's entities with
/// its own: an item on this side and a cart across make 2 on the light plate.
#[test]
fn a_weighted_plate_at_the_line_counts_the_copys_entities_with_its_own() {
    let light = state("minecraft:light_weighted_pressure_plate[power=0]");
    let world = World::of(&[(BY_THE_LINE, light)])
        .with_entities(&[dirt(1, [15.5, -60.0, 4.5]), cart(2, CART_ACROSS)]);
    let mut scene = Scene::new(Layout::side(true), &world, inventory(&[]), AWAY);
    let mut seen = Vec::new();
    assert!(
        scene.idle_until(30, &mut seen, |scene, _| signal_at(scene, BY_THE_LINE) == 2),
        "power {}",
        signal_at(&scene, BY_THE_LINE)
    );
}

/// Where player 1 of the east stands: their box (0.6 wide) reaches the west's plate.
const PLAYER_ACROSS: Vec3 = Vec3::new(16.2, -60.0, 4.5);

/// The [`Effected::PlayerInside`] effects of a tick at `at`.
fn player_inside(output: &TickOutput, at: BlockPos) -> Vec<Effect> {
    output
        .effects
        .iter()
        .filter(|effect| effect.at == at && matches!(effect.what, Effected::PlayerInside { .. }))
        .copied()
        .collect()
}

/// Plan section 7 item 12 and section 8 item 10: a player of the other region whose box
/// reaches a plate the region reads only in a copy is sent to the plate's holder as an
/// effect every tick they touch it, with their box; the holder presses the plate for it
/// and keeps it pressed while the effects come, through the plate's looks; when the
/// player walks away the effects stop and the plate is let go.
#[test]
fn a_player_of_the_other_region_on_a_plate_presses_it_and_lets_it_go_when_they_leave() {
    let world = World::of(&[(BY_THE_LINE, oak())]);
    let mut east = Scene::new(Layout::side(false), &world, inventory(&[]), PLAYER_ACROSS);
    let mut west = Scene::new(Layout::side(true), &world, inventory(&[]), AWAY);
    let mut sequence = 100;
    let mut feed = |west: &mut Scene, effects: Vec<Effect>| {
        sequence += 1;
        filed_now(west, sequence, effects)
    };
    let output = east.idle();
    let first = player_inside(&output, BY_THE_LINE);
    assert_eq!(
        first.len(),
        1,
        "the player's box goes to the plate's holder"
    );
    let Effected::PlayerInside { min, max } = first[0].what else {
        unreachable!()
    };
    let (min, max) = (min.map(|d| d.0), max.map(|d| d.0));
    for (got, expected) in min
        .iter()
        .chain(&max)
        .zip([15.9, -60.0, 4.2, 16.5, -58.2, 4.8])
    {
        // The box as the walk takes it, deflated by 1e-5.
        assert!((got - expected).abs() < 2e-5, "{min:?} {max:?}");
    }
    feed(&mut west, first);
    let mut pressed_at = None;
    for tick in 0..45 {
        let output = east.idle();
        let effects = player_inside(&output, BY_THE_LINE);
        assert_eq!(effects.len(), 1, "every tick they touch it");
        feed(&mut west, effects);
        if signal_at(&west, BY_THE_LINE) > 0 && pressed_at.is_none() {
            pressed_at = Some(tick);
        }
        if let Some(pressed) = pressed_at {
            assert_eq!(
                signal_at(&west, BY_THE_LINE),
                15,
                "kept, {} ticks on",
                tick - pressed
            );
        }
    }
    assert!(
        pressed_at.is_some_and(|tick| tick <= 1),
        "pressed a tick late at most"
    );
    let output = east.walk_to(Vec3::new(18.5, -60.0, 4.5));
    assert!(
        player_inside(&output, BY_THE_LINE).is_empty(),
        "no effect once they leave"
    );
    let mut let_go = None;
    for tick in 0..25 {
        let output = east.idle();
        assert!(player_inside(&output, BY_THE_LINE).is_empty());
        feed(&mut west, Vec::new());
        if signal_at(&west, BY_THE_LINE) == 0 {
            let_go = Some(tick);
            break;
        }
    }
    assert!(let_go.is_some(), "let go at its next look");
}

/// Plan section 7 item 12 ("on a plate or a tripwire"): a player of the other region
/// standing on tripwire the region reads only in a copy trips it through its holder,
/// whose hooks are powered, and when they walk away the wire's 10-tick look lets it go.
#[test]
fn a_player_of_the_other_region_on_tripwire_trips_it_through_its_holder() {
    let mut blocks = vec![
        (BlockPos::new(15, -60, 1), clustine_data::blocks::STONE),
        (BlockPos::new(15, -60, 7), clustine_data::blocks::STONE),
        (
            BlockPos::new(15, -60, 2),
            state("minecraft:tripwire_hook[attached=true,facing=south,powered=false]"),
        ),
        (
            BlockPos::new(15, -60, 6),
            state("minecraft:tripwire_hook[attached=true,facing=north,powered=false]"),
        ),
    ];
    for z in 3..=5 {
        blocks.push((
            BlockPos::new(15, -60, z),
            state("minecraft:tripwire[attached=true,north=true,south=true,powered=false]"),
        ));
    }
    let world = World::of(&blocks);
    let wire = BlockPos::new(15, -60, 4);
    let mut east = Scene::new(Layout::side(false), &world, inventory(&[]), PLAYER_ACROSS);
    let mut west = Scene::new(Layout::side(true), &world, inventory(&[]), AWAY);
    let mut sequence = 200;
    let mut tripped = None;
    for tick in 0..30 {
        let output = east.idle();
        let effects = player_inside(&output, wire);
        assert_eq!(effects.len(), 1, "the player's box, every tick");
        sequence += 1;
        filed_now(&mut west, sequence, effects);
        if powered(&west, wire) && tripped.is_none() {
            tripped = Some(tick);
        }
    }
    assert!(
        tripped.is_some_and(|tick| tick <= 1),
        "tripped a tick late at most"
    );
    assert!(powered(&west, wire), "kept while they stand there");
    assert!(powered(&west, BlockPos::new(15, -60, 2)), "the hooks");
    assert!(powered(&west, BlockPos::new(15, -60, 6)), "the hooks");
    east.walk_to(Vec3::new(18.5, -60.0, 4.5));
    let mut let_go = false;
    for _ in 0..15 {
        let output = east.idle();
        assert!(player_inside(&output, wire).is_empty());
        west.idle();
        if !powered(&west, wire) {
            let_go = true;
            break;
        }
    }
    assert!(let_go, "let go once they have left");
}

/// Hands `effects` to `west` as the store files them, for the tick it takes them in.
fn filed_now(west: &mut Scene, sequence: u64, effects: Vec<Effect>) -> TickOutput {
    let filed = effects
        .into_iter()
        .enumerate()
        .map(|(index, effect)| FiledEffect {
            id: EffectId {
                sequence,
                index: u32::try_from(index).expect("few"),
            },
            effect,
            due: Due::FromTaking,
        })
        .collect();
    west.region.tick(&clustine_sim::TickInputs {
        effects: filed,
        ..clustine_sim::TickInputs::default()
    })
}

/// Brief 8.14, "for section 6": redstone ore is lit by any entity on the ground that
/// does not step carefully, an item among them.
#[test]
fn an_item_lying_on_redstone_ore_lights_it() {
    let ore = BlockPos::new(4, -61, 4);
    let world = World::of(&[(ore, state("minecraft:redstone_ore[lit=false]"))])
        .with_entities(&[dirt(1, ON_IT)]);
    let mut scene = Scene::alone(&world, inventory(&[]), AWAY);
    let mut seen = Vec::new();
    assert!(
        scene.idle_until(10, &mut seen, |scene, _| scene.block(ore).property("lit")
            == Some("true")),
        "lit"
    );
}

/// Plan section 8 item 10: the boxes are keyed by the effect's due tick, not "the tick
/// before", so jitter in the coupling does not flicker a plate. The east's player stands
/// on the west's plate; each tick's effect is due in the west tick that takes it, but
/// once one comes a tick late, beside the next one. Wherever in the plate's 20-tick look
/// that gap falls, the plate stays pressed.
#[test]
fn an_effect_a_tick_late_does_not_let_a_plate_go() {
    for gap in 0..21 {
        let world = World::of(&[(BY_THE_LINE, oak())]);
        let mut east = Scene::new(Layout::side(false), &world, inventory(&[]), PLAYER_ACROSS);
        let mut west = Scene::new(Layout::side(true), &world, inventory(&[]), AWAY);
        let mut held_back: Option<FiledEffect> = None;
        let mut pressed = false;
        for tick in 0..60 {
            let output = east.idle();
            let due = west.region.tick_number() + 1;
            let mut effects: Vec<FiledEffect> = held_back.take().into_iter().collect();
            let sequence = 300 + tick;
            for (index, effect) in player_inside(&output, BY_THE_LINE).into_iter().enumerate() {
                effects.push(FiledEffect {
                    id: EffectId {
                        sequence,
                        index: u32::try_from(index).expect("few"),
                    },
                    effect,
                    due: Due::At { tick: due },
                });
            }
            if pressed && tick == 10 + gap {
                assert_eq!(effects.len(), 1);
                held_back = effects.pop();
            }
            west.region.tick(&clustine_sim::TickInputs {
                effects,
                ..clustine_sim::TickInputs::default()
            });
            if pressed {
                assert_eq!(
                    signal_at(&west, BY_THE_LINE),
                    15,
                    "the gap {gap} ticks into the look, at {tick}"
                );
            }
            pressed |= signal_at(&west, BY_THE_LINE) > 0;
        }
        assert!(pressed);
    }
}
