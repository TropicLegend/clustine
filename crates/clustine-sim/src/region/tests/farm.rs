//! The farm animals in a region, from the brief's section 4 and the plan's 9.4 and 10.7:
//! eggs make them with their variants, they stroll, follow food and panic, two fed adults
//! breed and their child grows up, breeding across a line, the age lock, each animal's
//! own uses, and that a tick of them is a function of its inputs.

use clustine_data::{entity_types, items, sounds};
use clustine_world::{
    Double, Due, Effect, EffectId, Effected, Entity, EntityChange, EntityData, EntityId, EntityUse,
    FiledEffect, Float, TypeState,
};

use super::*;
use crate::living::MAX_AIR;
use crate::mobs::Mob;
use crate::mobs::types::animal::{self, Animal, Asked, BABY_START_AGE, Own};
use crate::mobs::types::chicken::ChickenState;
use crate::mobs::types::cow::CowState;
use crate::mobs::types::mooshroom::MooshroomState;
use crate::mobs::types::pig::PigState;
use crate::mobs::types::rabbit::RabbitState;
use crate::mobs::types::sheep::SheepState;

const ONE: EntityId = EntityId(40);
const TWO: EntityId = EntityId(42);

/// An animal of the type `kind` at `position`, standing, of its type's full health, with
/// what every animal shares `shared` and its type's own `own`.
fn animal_of<O: Own>(
    key: EntityId,
    kind: i32,
    position: [f64; 3],
    shared: &Animal,
    own: &O,
) -> Entity {
    let mut mob = Mob::fresh();
    let kind = kind as u16;
    mob.body.vitals.health = crate::mobs::body::attributes::supplied(kind, 23).unwrap() as f32;
    mob.body.vitals.air = MAX_AIR;
    mob.state = animal::write(shared, own);
    Entity {
        portal: Default::default(),
        key,
        kind,
        uuid: uuid::Uuid::from_u128(u128::from(key.0)),
        position,
        motion: [0.0, -0.078_400_001_525_878_9, 0.0],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: mob.encode(),
    }
}

fn cow_at(key: EntityId, x: f64, z: f64, shared: &Animal) -> Entity {
    animal_of(
        key,
        entity_types::COW,
        [x, -60.0, z],
        shared,
        &CowState::default(),
    )
}

fn lay(region: &mut Region, entity: &Entity) {
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .unwrap()
        .apply_entity(&EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

fn record(region: &Region, key: EntityId) -> Option<Entity> {
    region.find_entity(key).map(|(_, entity)| entity.clone())
}

fn mob(region: &Region, key: EntityId) -> Mob {
    Mob::decode(&record(region, key).expect("the mob is here").data).unwrap()
}

fn shared(region: &Region, key: EntityId) -> Animal {
    animal::animal_of(&mob(region, key).state)
}

fn own<O: Own>(region: &Region, key: EntityId) -> O {
    animal::read::<O>(&mob(region, key).state).1
}

fn lived(region: &Region, key: EntityId) -> u32 {
    record(region, key).unwrap().lived
}

fn in_love() -> Animal {
    let mut shared = Animal::default();
    shared.set_in_love(0);
    shared
}

fn baby(age: i32) -> Animal {
    let mut shared = Animal::default();
    shared.set_age(age, 0);
    shared
}

/// Every entity of the type `kind` in the region's chunks.
fn all_of(region: &Region, kind: i32) -> Vec<Entity> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter(|entity| i32::from(entity.kind) == kind)
        .cloned()
        .collect()
}

fn items_lying(region: &Region) -> Vec<(ItemStack, [f64; 3])> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter_map(|entity| match entity.data {
            EntityData::Item { ref stack, .. } => Some((stack.clone(), entity.position)),
            _ => None,
        })
        .collect()
}

fn holding(region: &mut Region, stack: ItemStack) {
    checked(
        region,
        &moves(vec![set_slot(1, 0, Some(stack)), select(1, 0)]),
    );
}

fn one(item: i32) -> ItemStack {
    ItemStack::new(item, 1)
}

/// Player 1's use of the entity `key` with what they hold.
fn use_on(region: &mut Region, key: EntityId) -> TickOutput {
    checked(
        region,
        &moves(vec![numbered(
            1,
            PlayerInput::Interact {
                entity: key,
                location: [0.0, 0.5, 0.0],
                sneaking: false,
            },
        )]),
    )
}

fn sounds_of(output: &TickOutput) -> Vec<(i32, Option<PlayerId>)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound { sound, except, .. } => Some((*sound, *except)),
            _ => None,
        })
        .collect()
}

fn entity_events(output: &TickOutput, key: EntityId) -> Vec<u8> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntityEvent { entity, event, .. } if *entity == key => Some(*event),
            _ => None,
        })
        .collect()
}

fn held_count(region: &Region, item: i32) -> usize {
    region.players[&player(1)]
        .inventory
        .0
        .iter()
        .flatten()
        .filter(|stack| stack.item == item)
        .count()
}

fn idle_for(region: &mut Region, ticks: usize) {
    for _ in 0..ticks {
        checked(region, &TickInputs::default());
    }
}

#[test]
fn an_egg_makes_each_farm_animal_with_its_variant_drawn_by_the_biome_in_the_game_s_order() {
    // The floor's biome is the registry's first, the badlands: warm.
    for kind in [
        entity_types::COW,
        entity_types::PIG,
        entity_types::CHICKEN,
        entity_types::SHEEP,
        entity_types::RABBIT,
        entity_types::MOOSHROOM,
    ] {
        let mut region = on_ticking_floor_with(&[]);
        let player_key = region.players[&player(1)].entity_id;
        let mut level = region.entity_stream(player_key).random;
        let mut output = TickOutput::default();
        let key = region
            .spawn_mob(
                kind as u16,
                player(1),
                BlockPos::new(3, -60, 3),
                true,
                true,
                &mut output,
            )
            .expect("made");
        let _yaw = level.next_float();
        let made = mob(&region, key);
        let biome = Some("minecraft:badlands");
        match kind {
            entity_types::COW => {
                let warm = animal::variants::select("minecraft:cow_variant", biome, &mut level);
                let sound = level.next_int(2) as u8;
                let state = animal::read::<CowState>(&made.state).1;
                assert_eq!(state.variant, warm);
                assert_eq!(
                    state.variant,
                    animal::variants::id("minecraft:cow_variant", "minecraft:warm")
                );
                assert_eq!(state.sound, sound);
            }
            entity_types::PIG => {
                let warm = animal::variants::select("minecraft:pig_variant", biome, &mut level);
                let sound = level.next_int(3) as u8;
                let state = animal::read::<PigState>(&made.state).1;
                assert_eq!((state.variant, state.sound), (warm, sound));
            }
            entity_types::CHICKEN => {
                let warm = animal::variants::select("minecraft:chicken_variant", biome, &mut level);
                let sound = level.next_int(2) as u8;
                let state = animal::read::<ChickenState>(&made.state).1;
                assert_eq!((state.variant, state.sound), (warm, sound));
                // Its constructor's egg timer, of its own stream.
                assert!((6000..12_000).contains(&state.egg) && !state.counting);
            }
            entity_types::SHEEP => {
                let colour = crate::mobs::types::sheep::spawn_colour(biome, &mut level);
                let state = animal::read::<SheepState>(&made.state).1;
                assert_eq!(state.colour(), colour);
                assert!(!state.sheared());
            }
            entity_types::RABBIT => {
                let variant = crate::mobs::types::rabbit::random_variant(biome, &mut level);
                let state = animal::read::<RabbitState>(&made.state).1;
                assert_eq!(state.variant, variant);
                // Its constructor wants the origin at no speed (the brief's finding 14).
                let movement = &made.body.controls.movement;
                assert_eq!(movement.wanted, [0.0; 3]);
                assert_eq!(movement.operation, crate::mobs::ai::Operation::MoveTo);
            }
            _ => {
                // A mooshroom draws nothing of its own and is red.
                assert!(!animal::read::<MooshroomState>(&made.state).1.brown);
            }
        }
        // Then `Mob`'s: the follow range's bonus, the hand.
        let bonus = 0.114_850_000_000_000_01 * (level.next_double() - level.next_double());
        let left = level.next_float() < 0.05;
        let follow = made
            .body
            .attributes
            .made(clustine_data::attributes::FOLLOW_RANGE as u16)
            .unwrap();
        assert_eq!(
            follow.modifiers[0].amount.to_bits(),
            bonus.to_bits(),
            "{kind}"
        );
        assert_eq!(made.body.flags.left_handed, left);
        // An adult, not in love, of its type's health.
        let state = animal::animal_of(&made.state);
        assert!(!state.is_baby() && !state.in_love(0));
        assert_eq!(
            made.body.vitals.health,
            crate::mobs::body::attributes::supplied(kind as u16, 23).unwrap() as f32
        );
    }
}

#[test]
fn an_animal_follows_a_player_holding_its_food_and_strolls_and_looks_about_without() {
    let mut region = on_ticking_floor_with(&[]);
    holding(&mut region, one(items::WHEAT));
    lay(&mut region, &cow_at(ONE, 7.5, 0.5, &Animal::default()));
    for _ in 0..300 {
        checked(&mut region, &TickInputs::default());
    }
    let near = record(&region, ONE).unwrap().position;
    assert!(
        (near[0] - 0.5).abs() < 3.5 && (near[2] - 0.5).abs() < 3.5,
        "tempted to the player: {near:?}"
    );

    // Holding nothing it likes, the player is only looked at, and the cow strolls. It
    // starts in the middle of the one chunk that ticks entities, so that a stroll does
    // not take it into a chunk where it stands still.
    let mut region = on_ticking_floor_with(&[]);
    holding(&mut region, one(items::STICK));
    lay(&mut region, &cow_at(ONE, 7.5, 7.5, &Animal::default()));
    let mut moved = false;
    let mut turned = false;
    for _ in 0..600 {
        checked(&mut region, &TickInputs::default());
        let now = record(&region, ONE).unwrap();
        moved |= (now.position[0] - 7.5).abs() > 1.0 || (now.position[2] - 7.5).abs() > 1.0;
        turned |= mob(&region, ONE).body.head.head_yaw != 0.0;
    }
    assert!(moved && turned, "moved {moved} turned {turned}");
}

#[test]
fn a_hurt_animal_panics_at_its_panic_speed() {
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &cow_at(ONE, 2.5, 0.5, &Animal::default()));
    checked(
        &mut region,
        &moves(vec![numbered(
            1,
            PlayerInput::Attack {
                part: None,
                entity: ONE,
            },
        )]),
    );
    // A knocked-back cow makes no path until it lands (`canUpdatePath`), so the panic's
    // speed shows only once it is down, some ticks after the hit.
    let panicked = (0..40).any(|_| {
        checked(&mut region, &TickInputs::default());
        let cow = record(&region, ONE).unwrap();
        crate::mobs::ai::panicking_record(&cow) && mob(&region, ONE).body.progress.speed == 2.0
    });
    assert!(panicked);
}

#[test]
fn two_adults_fed_breed_after_their_count_and_their_child_is_a_baby_at_the_breeding_parent() {
    let mut region = on_ticking_floor_with(&[]);
    let cold = animal::variants::id("minecraft:cow_variant", "minecraft:cold");
    let warm = animal::variants::id("minecraft:cow_variant", "minecraft:warm");
    let make = |key, x, variant| {
        animal_of(
            key,
            entity_types::COW,
            [x, -60.0, 3.5],
            &Animal::default(),
            &CowState {
                variant,
                ..CowState::default()
            },
        )
    };
    lay(&mut region, &make(ONE, 2.5, cold));
    lay(&mut region, &make(TWO, 4.0, warm));
    holding(&mut region, one(items::WHEAT));
    let fed = use_on(&mut region, ONE);
    assert_eq!(entity_events(&fed, ONE), [18]);
    assert!(shared(&region, ONE).in_love(lived(&region, ONE)));
    // A second feeding in love takes nothing.
    let again = use_on(&mut region, ONE);
    assert!(entity_events(&again, ONE).is_empty());
    use_on(&mut region, TWO);
    let mut bred = None;
    for tick in 0..300 {
        let output = checked(&mut region, &TickInputs::default());
        let cows = all_of(&region, entity_types::COW);
        if cows.len() == 3 {
            bred = Some((tick, output));
            break;
        }
    }
    let (tick, output) = bred.expect("a calf");
    // The goal counts 30 of its every other tick's runs: about 60 ticks.
    assert!((55..=70).contains(&tick), "{tick}");
    let asker = [ONE, TWO]
        .into_iter()
        .find(|key| shared(&region, *key).requests == 1)
        .expect("one asked");
    assert_eq!(entity_events(&output, asker), [18]);
    let child = all_of(&region, entity_types::COW)
        .into_iter()
        .find(|cow| cow.key != ONE && cow.key != TWO)
        .unwrap();
    assert_eq!(child.position, record(&region, asker).unwrap().position);
    assert_eq!((child.yaw, child.pitch), (0.0, 0.0));
    let calf = animal::animal_of(&Mob::decode(&child.data).unwrap().state);
    assert_eq!(calf.age(0), BABY_START_AGE);
    let variant = animal::read::<CowState>(&Mob::decode(&child.data).unwrap().state)
        .1
        .variant;
    assert!(variant == cold || variant == warm);
    for parent in [ONE, TWO] {
        let state = shared(&region, parent);
        assert_eq!(state.age, animal::BRED_AGE);
        assert!(!state.in_love(lived(&region, parent)));
        assert!(state.asked.is_none());
    }
    // The orb at the breeding parent, one to seven, added before the child.
    let orbs: Vec<Entity> = all_of(&region, entity_types::EXPERIENCE_ORB);
    let [orb] = &orbs[..] else {
        panic!("one orb: {orbs:?}");
    };
    let EntityData::ExperienceOrb { value, .. } = orb.data else {
        unreachable!();
    };
    assert!((1..=7).contains(&value));
    assert!(orb.key < child.key);
    // In their cooldown neither takes food.
    let fed = use_on(&mut region, ONE);
    assert!(entity_events(&fed, ONE).is_empty());
}

#[test]
fn a_baby_grows_up_when_its_age_comes_to_nothing_and_clients_are_told_its_size() {
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &cow_at(ONE, 2.5, 2.5, &baby(-30)));
    let calf_box = region.entity_box(&record(&region, ONE).unwrap());
    assert!((calf_box.max[1] - calf_box.min[1] - 0.7).abs() < 1e-6);
    let mut grew = None;
    for tick in 0..40 {
        let output = checked(&mut region, &TickInputs::default());
        if !shared(&region, ONE).is_baby() {
            grew = Some((tick, output));
            break;
        }
    }
    let (tick, output) = grew.expect("grown up");
    assert_eq!(tick, 29);
    // Its record changed (the baby flag), which clients are told of.
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::EntityTracked(report) if report.entity == ONE && report.data.is_some()
    )));
    let grown = region.entity_box(&record(&region, ONE).unwrap());
    assert!((grown.max[1] - grown.min[1] - 1.4).abs() < 1e-6);
}

#[test]
fn a_fed_baby_grows_a_tenth_sooner_and_owes_it_back_as_a_cooldown() {
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &cow_at(ONE, 2.5, 0.5, &baby(BABY_START_AGE)));
    holding(&mut region, one(items::WHEAT));
    // The use comes between ticks, at the calf's `lived`.
    let used_at = lived(&region, ONE);
    let before = shared(&region, ONE).age(used_at);
    let gained = animal::speed_up_seconds(-before) * 20;
    assert_eq!(gained, 2380);
    use_on(&mut region, ONE);
    let now = lived(&region, ONE);
    let after = shared(&region, ONE);
    assert_eq!(after.age(now), before + gained + (now - used_at) as i32);
    assert_eq!(after.forced_age, gained);
    assert!(!after.in_love(now));
}

#[test]
fn a_golden_dandelion_locks_a_baby_as_a_newborn_and_another_unlocks_it_after_40_ticks() {
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &cow_at(ONE, 2.5, 0.5, &baby(-1000)));
    holding(&mut region, one(items::GOLDEN_DANDELION));
    let output = use_on(&mut region, ONE);
    assert!(
        sounds_of(&output)
            .iter()
            .any(|(sound, _)| *sound == sounds::ITEM_GOLDEN_DANDELION_USE)
    );
    let locked = shared(&region, ONE);
    assert!(locked.age_locked);
    assert_eq!(locked.age(lived(&region, ONE)), BABY_START_AGE);
    assert!(mob(&region, ONE).body.flags.persistent);
    idle_for(&mut region, 20);
    assert_eq!(
        shared(&region, ONE).age(lived(&region, ONE)),
        BABY_START_AGE
    );
    // Within the 40 ticks a second does nothing.
    use_on(&mut region, ONE);
    assert!(shared(&region, ONE).age_locked);
    idle_for(&mut region, 20);
    let now = lived(&region, ONE);
    let output = use_on(&mut region, ONE);
    assert!(
        sounds_of(&output)
            .iter()
            .any(|(sound, _)| *sound == sounds::ITEM_GOLDEN_DANDELION_UNUSE)
    );
    assert!(!shared(&region, ONE).age_locked);
    // Unlocked, it grows again from a newborn's age.
    idle_for(&mut region, 10);
    assert_eq!(
        shared(&region, ONE).age(lived(&region, ONE)),
        BABY_START_AGE + (lived(&region, ONE) - now) as i32
    );
}

#[test]
fn a_bucket_milks_a_grown_cow_for_the_player_and_a_calf_gives_nothing() {
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &cow_at(ONE, 2.5, 0.5, &Animal::default()));
    lay(&mut region, &cow_at(TWO, 2.5, 3.5, &baby(BABY_START_AGE)));
    holding(&mut region, one(items::BUCKET));
    let output = use_on(&mut region, ONE);
    assert!(sounds_of(&output).contains(&(sounds::ENTITY_COW_MILK, Some(player(1)))));
    assert_eq!(held_count(&region, items::MILK_BUCKET), 1);
    // Duplicates are kept out.
    use_on(&mut region, ONE);
    assert_eq!(held_count(&region, items::MILK_BUCKET), 1);
    holding(&mut region, one(items::BUCKET));
    let output = use_on(&mut region, TWO);
    assert!(
        !sounds_of(&output)
            .iter()
            .any(|(sound, _)| *sound == sounds::ENTITY_COW_MILK)
    );
}

#[test]
fn a_bowl_on_a_mooshroom_gives_stew_and_shears_make_it_a_cow_with_five_mushrooms() {
    let mut region = on_ticking_floor_with(&[]);
    let mooshroom = animal_of(
        ONE,
        entity_types::MOOSHROOM,
        [2.5, -60.0, 2.5],
        &Animal::default(),
        &MooshroomState::default(),
    );
    lay(&mut region, &mooshroom);
    holding(&mut region, one(items::BOWL));
    use_on(&mut region, ONE);
    use_on(&mut region, ONE);
    // Duplicates allowed: a stew every time.
    assert_eq!(held_count(&region, items::MUSHROOM_STEW), 2);
    holding(&mut region, one(items::SHEARS));
    let output = use_on(&mut region, ONE);
    assert!(
        sounds_of(&output)
            .iter()
            .any(|(sound, _)| *sound == sounds::ENTITY_MOOSHROOM_SHEAR)
    );
    assert!(record(&region, ONE).is_none());
    let cows = all_of(&region, entity_types::COW);
    let [cow] = &cows[..] else {
        panic!("one cow: {cows:?}");
    };
    assert_eq!(cow.position, mooshroom.position);
    let mushrooms: Vec<_> = items_lying(&region)
        .into_iter()
        .filter(|(stack, _)| stack.item == items::RED_MUSHROOM)
        .collect();
    assert_eq!(mushrooms.len(), 5);
    // Laid at the top of the mooshroom, one by one; the tick after has moved them.
    assert!(
        mushrooms
            .iter()
            .all(|(stack, at)| stack.count == 1 && at[1] > -60.0)
    );
}

#[test]
fn a_brown_mooshroom_fed_a_flower_gives_a_suspicious_stew_once() {
    let mut region = on_ticking_floor_with(&[]);
    let brown = MooshroomState {
        brown: true,
        stew: None,
    };
    lay(
        &mut region,
        &animal_of(
            ONE,
            entity_types::MOOSHROOM,
            [2.5, -60.0, 2.5],
            &Animal::default(),
            &brown,
        ),
    );
    holding(&mut region, one(items::POPPY));
    let output = use_on(&mut region, ONE);
    assert!(
        sounds_of(&output)
            .iter()
            .any(|(sound, _)| *sound == sounds::ENTITY_MOOSHROOM_EAT)
    );
    assert_eq!(own::<MooshroomState>(&region, ONE).stew, Some(items::POPPY));
    holding(&mut region, one(items::BOWL));
    let output = use_on(&mut region, ONE);
    assert!(
        sounds_of(&output)
            .iter()
            .any(|(sound, _)| *sound == sounds::ENTITY_MOOSHROOM_SUSPICIOUS_MILK)
    );
    assert_eq!(held_count(&region, items::SUSPICIOUS_STEW), 1);
    use_on(&mut region, ONE);
    assert_eq!(held_count(&region, items::MUSHROOM_STEW), 1);
}

#[test]
fn shears_shear_a_sheep_for_its_wool_once_and_a_dye_colours_only_one_with_its_wool() {
    let mut region = on_ticking_floor_with(&[]);
    let mut lime = SheepState::default();
    lime.set_colour(5);
    lay(
        &mut region,
        &animal_of(
            ONE,
            entity_types::SHEEP,
            [2.5, -60.0, 2.5],
            &Animal::default(),
            &lime,
        ),
    );
    holding(&mut region, one(items::RED_DYE));
    let output = use_on(&mut region, ONE);
    assert!(sounds_of(&output).contains(&(sounds::ITEM_DYE_USE, Some(player(1)))));
    assert_eq!(own::<SheepState>(&region, ONE).colour(), 14);
    holding(&mut region, one(items::SHEARS));
    let output = use_on(&mut region, ONE);
    assert!(
        sounds_of(&output)
            .iter()
            .any(|(sound, _)| *sound == sounds::ENTITY_SHEEP_SHEAR)
    );
    assert!(own::<SheepState>(&region, ONE).sheared());
    let wool: Vec<_> = items_lying(&region)
        .into_iter()
        .filter(|(stack, _)| stack.item == items::RED_WOOL)
        .collect();
    assert!((1..=3).contains(&wool.len()), "{wool:?}");
    assert!(
        wool.iter()
            .all(|(stack, at)| stack.count == 1 && at[1] > -60.0)
    );
    // A sheared sheep only takes the click, and no dye.
    use_on(&mut region, ONE);
    assert_eq!(
        items_lying(&region)
            .iter()
            .filter(|(stack, _)| stack.item == items::RED_WOOL)
            .count(),
        wool.len()
    );
    holding(&mut region, one(items::BLUE_DYE));
    use_on(&mut region, ONE);
    assert_eq!(own::<SheepState>(&region, ONE).colour(), 14);
}

#[test]
fn a_saddle_goes_on_a_grown_pig_kept_whatever_kills_it_and_shears_take_it_off() {
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &animal_of(
            ONE,
            entity_types::PIG,
            [2.5, -60.0, 2.5],
            &Animal::default(),
            &PigState::default(),
        ),
    );
    holding(&mut region, one(items::SADDLE));
    let output = use_on(&mut region, ONE);
    assert!(
        sounds_of(&output)
            .iter()
            .any(|(sound, _)| *sound == sounds::ENTITY_PIG_SADDLE)
    );
    let equipment = mob(&region, ONE).body.equipment;
    assert_eq!(equipment.slots[7], Some(one(items::SADDLE)));
    assert_eq!(equipment.drop_chance(7), 2.0);
    assert!(output.events.iter().any(
        |event| matches!(event, RegionEvent::EquipmentChanged { entity, .. } if *entity == ONE)
    ));
    // Its food in hand, a saddled pig is fed, not mounted.
    holding(&mut region, one(items::CARROT));
    let fed = use_on(&mut region, ONE);
    assert_eq!(entity_events(&fed, ONE), [18]);
    holding(&mut region, one(items::SHEARS));
    let output = use_on(&mut region, ONE);
    assert!(
        sounds_of(&output)
            .iter()
            .any(|(sound, _)| *sound == sounds::ITEM_SADDLE_UNEQUIP)
    );
    assert_eq!(mob(&region, ONE).body.equipment.slots[7], None);
    assert!(
        items_lying(&region)
            .iter()
            .any(|(stack, _)| stack.item == items::SADDLE)
    );
}

#[test]
fn a_chicken_lays_an_egg_of_its_variant_when_its_timer_runs_out_and_a_chick_lays_none() {
    let mut region = on_ticking_floor_with(&[]);
    let laying = ChickenState {
        egg: 3,
        variant: animal::variants::id("minecraft:chicken_variant", "minecraft:cold"),
        ..ChickenState::default()
    };
    lay(
        &mut region,
        &animal_of(
            ONE,
            entity_types::CHICKEN,
            [2.5, -60.0, 2.5],
            &Animal::default(),
            &laying,
        ),
    );
    lay(
        &mut region,
        &animal_of(
            TWO,
            entity_types::CHICKEN,
            [5.5, -60.0, 5.5],
            &baby(BABY_START_AGE),
            &laying,
        ),
    );
    let mut heard = Vec::new();
    for _ in 0..3 {
        let output = checked(&mut region, &TickInputs::default());
        heard.extend(sounds_of(&output));
    }
    let eggs: Vec<_> = items_lying(&region)
        .into_iter()
        .filter(|(stack, _)| stack.item == items::BLUE_EGG)
        .collect();
    assert_eq!(eggs.len(), 1, "{:?}", items_lying(&region));
    assert!(
        heard
            .iter()
            .any(|(sound, _)| *sound == sounds::ENTITY_CHICKEN_EGG)
    );
    let after = own::<ChickenState>(&region, ONE);
    assert!(after.counting);
    let left = after.egg - (lived(&region, ONE) - after.egg_at) as i32;
    assert!((5990..12_000).contains(&left), "{left}");
    // The chick's timer waits.
    assert_eq!(own::<ChickenState>(&region, TWO).egg, 3);
    assert!(!own::<ChickenState>(&region, TWO).counting);
}

#[test]
fn a_chicken_falls_at_six_tenths_of_the_speed() {
    let mut region = on_ticking_floor_with(&[]);
    let mut falling = animal_of(
        ONE,
        entity_types::CHICKEN,
        [2.5, -50.0, 2.5],
        &Animal::default(),
        &ChickenState::default(),
    );
    falling.on_ground = false;
    falling.motion = [0.0; 3];
    lay(&mut region, &falling);
    let mut cow = cow_at(TWO, 6.5, 6.5, &Animal::default());
    cow.position[1] = -50.0;
    cow.on_ground = false;
    cow.motion = [0.0; 3];
    lay(&mut region, &cow);
    idle_for(&mut region, 5);
    let chicken = record(&region, ONE).unwrap();
    let cow = record(&region, TWO).unwrap();
    assert!(chicken.position[1] > cow.position[1]);
    assert!(chicken.motion[1] > cow.motion[1]);
}

#[test]
fn a_lamb_eats_the_grass_under_it_and_grows_its_wool_back_and_a_minute_older() {
    let grass: Vec<(BlockPos, clustine_data::BlockState)> = (0..16)
        .flat_map(|x| (0..16).map(move |z| (BlockPos::new(x, -61, z), blocks::GRASS_BLOCK)))
        .collect();
    let mut region = on_ticking_floor_with(&grass);
    let mut sheared = SheepState::default();
    sheared.set_sheared(true);
    lay(
        &mut region,
        &animal_of(
            ONE,
            entity_types::SHEEP,
            [8.5, -60.0, 8.5],
            &baby(BABY_START_AGE),
            &sheared,
        ),
    );
    let mut ate = None;
    for _ in 0..1500 {
        let output = checked(&mut region, &TickInputs::default());
        if output.events.iter().any(|event| {
            matches!(event, RegionEvent::BlockChanged { state, .. } if *state == blocks::DIRT)
        }) {
            ate = Some(output);
            break;
        }
    }
    let output = ate.expect("it ate");
    assert!(
        output
            .events
            .iter()
            .any(|event| matches!(event, RegionEvent::LevelEvent { kind: 2001, .. }))
    );
    assert!(!own::<SheepState>(&region, ONE).sheared());
    let now = lived(&region, ONE);
    // Grown by its ticks and a minute more.
    assert_eq!(
        shared(&region, ONE).age(now),
        BABY_START_AGE + now as i32 + 1200
    );
}

#[test]
fn a_rabbit_on_a_ripe_carrot_eats_a_stage_of_it() {
    let farmland = BlockPos::new(2, -61, 2);
    let carrots = BlockPos::new(2, -60, 2);
    let ripe = clustine_blocks::data::set(blocks::CARROTS, "age", "7");
    let mut region = on_ticking_floor_with(&[(farmland, blocks::FARMLAND), (carrots, ripe)]);
    let mut rabbit = animal_of(
        ONE,
        entity_types::RABBIT,
        [2.5, -60.0625, 2.5],
        &Animal::default(),
        &RabbitState::default(),
    );
    rabbit.motion = [0.0; 3];
    lay(&mut region, &rabbit);
    let mut eaten = false;
    for _ in 0..6 {
        checked(&mut region, &TickInputs::default());
        if region
            .block(carrots)
            .and_then(|state| clustine_blocks::data::number(state, "age"))
            == Some(6)
        {
            eaten = true;
            break;
        }
    }
    assert!(eaten);
    assert!(own::<RabbitState>(&region, ONE).more_carrot_ticks > 30);
}

#[test]
fn a_rabbit_hops_towards_a_player_holding_a_carrot() {
    let mut region = on_ticking_floor_with(&[]);
    holding(&mut region, one(items::CARROT));
    lay(
        &mut region,
        &animal_of(
            ONE,
            entity_types::RABBIT,
            [7.5, -60.0, 0.5],
            &Animal::default(),
            &RabbitState::default(),
        ),
    );
    let mut hops = 0;
    let mut jumps = 0;
    for _ in 0..200 {
        let output = checked(&mut region, &TickInputs::default());
        hops += entity_events(&output, ONE)
            .iter()
            .filter(|event| **event == 1)
            .count();
        jumps += sounds_of(&output)
            .iter()
            .filter(|(sound, _)| *sound == sounds::ENTITY_RABBIT_JUMP)
            .count();
    }
    assert!(hops > 2 && jumps > 2, "{hops} {jumps}");
    let near = record(&region, ONE).unwrap().position;
    assert!((near[0] - 0.5).abs() < 4.0, "{near:?}");
}

#[test]
fn an_egg_used_on_a_farm_animal_makes_a_baby_of_it_at_its_feet() {
    let mut region = on_ticking_floor_with(&[]);
    let warm = animal::variants::id("minecraft:pig_variant", "minecraft:warm");
    lay(
        &mut region,
        &animal_of(
            ONE,
            entity_types::PIG,
            [2.5, -60.0, 2.5],
            &Animal::default(),
            &PigState {
                variant: warm,
                ..PigState::default()
            },
        ),
    );
    holding(&mut region, one(items::PIG_SPAWN_EGG));
    use_on(&mut region, ONE);
    let pigs = all_of(&region, entity_types::PIG);
    let piglet = pigs.iter().find(|pig| pig.key != ONE).expect("a piglet");
    let state = Mob::decode(&piglet.data).unwrap().state;
    assert!(animal::animal_of(&state).is_baby());
    assert_eq!(animal::read::<PigState>(&state).1.variant, warm);
    assert_eq!(piglet.position, [2.5, -60.0, 2.5]);
}

/// A region of the land west of chunk x = 2, its chunks x of -1 to 1 loaded with a floor
/// and a wall at x = 31 that no cow passes, and the chunks of x = 2 another's, copied,
/// with `beyond` in the one at z = 0.
fn at_a_line(own: &[Entity], beyond: &[Entity]) -> Region {
    let west = ChunkArea {
        min_x: None,
        max_x: Some(2),
    };
    let mut region = joined_in(west, &[1]);
    let around: Vec<ChunkPos> = (-1..=1)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect();
    region.tick(&tickets(around.clone(), vec![]));
    let chunks = around
        .iter()
        .map(|position| {
            let mut chunk = floor();
            if position.x == 1 {
                for z in 0..16 {
                    for y in [-60, -59] {
                        chunk.set(15, y, z, blocks::STONE);
                    }
                }
            }
            (*position, chunk)
        })
        .collect();
    region.tick(&TickInputs {
        chunks_loaded: chunks,
        ..TickInputs::default()
    });
    for entity in own {
        lay(&mut region, entity);
    }
    let copies: Vec<ChunkPos> = (-1..=1).map(|z| ChunkPos::new(2, z)).collect();
    let bases = copies
        .iter()
        .map(|position| {
            let mut base = floor();
            if position.z == 0 {
                base.set_entities(beyond.to_vec());
            }
            (*position, base)
        })
        .collect();
    region.tick(&TickInputs {
        foreign: copies
            .iter()
            .map(|position| (*position, RegionId(1)))
            .collect(),
        copy_bases: bases,
        ..TickInputs::default()
    });
    region
}

/// The requests to breed the region sent in `output`.
fn asks(output: &TickOutput) -> Vec<(EntityId, EntityUse)> {
    output
        .effects
        .iter()
        .filter_map(|effect| match effect.what {
            Effected::OnEntity { key, ref by } => Some((key, by.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn across_a_line_only_the_higher_key_asks_and_waits_out_of_love_for_others_until_answered() {
    // The partner beyond has the higher key: the region's cow only counts.
    let partner = cow_at(EntityId(1 << 34), 32.5, 0.5, &in_love());
    let mut region = at_a_line(&[cow_at(ONE, 30.5, 0.5, &in_love())], &[partner]);
    for _ in 0..150 {
        let output = checked(&mut region, &TickInputs::default());
        assert!(asks(&output).is_empty(), "{:?}", asks(&output));
    }
    assert!(shared(&region, ONE).asked.is_none());

    // The region's cow has the higher key: it asks, as an effect at the partner's block.
    let partner = cow_at(EntityId(5), 32.5, 0.5, &in_love());
    let mut region = at_a_line(&[cow_at(ONE, 30.5, 0.5, &in_love())], &[partner]);
    let mut asked = None;
    for _ in 0..150 {
        let output = checked(&mut region, &TickInputs::default());
        if let Some(found) = asks(&output).first().cloned() {
            asked = Some(found);
            break;
        }
    }
    let (to, by) = asked.expect("asked");
    assert_eq!(to, EntityId(5));
    assert_eq!(
        by,
        EntityUse::BreedAsk {
            asker: ONE,
            number: 1
        }
    );
    let waiting = shared(&region, ONE);
    let now = lived(&region, ONE);
    assert!(waiting.in_love(now) && !waiting.in_love_for_others(now));
    // It asks no more while it waits.
    for _ in 0..20 {
        let output = checked(&mut region, &TickInputs::default());
        assert!(asks(&output).is_empty());
    }
    // The answer, accepted: its part of the breeding.
    let answer = TickInputs {
        effects: vec![FiledEffect {
            id: EffectId {
                sequence: 3,
                index: 0,
            },
            effect: Effect {
                at: BlockPos::new(30, -60, 0),
                what: Effected::OnEntity {
                    key: ONE,
                    by: EntityUse::BreedAnswer {
                        partner: EntityId(5),
                        number: 1,
                        accepted: true,
                    },
                },
            },
            due: Due::FromTaking,
        }],
        ..TickInputs::default()
    };
    region.tick(&answer);
    let bred = shared(&region, ONE);
    assert!(bred.asked.is_none());
    assert_eq!(bred.age, animal::BRED_AGE);
    assert!(!bred.in_love(lived(&region, ONE)));
    // No child here: the partner's region made it.
    assert_eq!(all_of(&region, entity_types::COW).len(), 1);
}

#[test]
fn an_asker_not_answered_within_40_ticks_may_ask_again() {
    let partner = cow_at(EntityId(5), 32.5, 0.5, &in_love());
    let mut region = at_a_line(&[cow_at(ONE, 30.5, 0.5, &in_love())], &[partner]);
    let mut numbers = Vec::new();
    for _ in 0..400 {
        let output = checked(&mut region, &TickInputs::default());
        for (_, by) in asks(&output) {
            if let EntityUse::BreedAsk { number, .. } = by {
                numbers.push((number, lived(&region, ONE)));
            }
        }
        if numbers.len() == 2 {
            break;
        }
    }
    assert_eq!(numbers.len(), 2, "{numbers:?}");
    assert_eq!((numbers[0].0, numbers[1].0), (1, 2));
    assert!(numbers[1].1 - numbers[0].1 >= 40);
}

fn request(to: EntityId, asker: EntityId, number: u32) -> TickInputs {
    TickInputs {
        effects: vec![FiledEffect {
            id: EffectId {
                sequence: 9,
                index: 0,
            },
            effect: Effect {
                at: BlockPos::new(30, -60, 0),
                what: Effected::OnEntity {
                    key: to,
                    by: EntityUse::BreedAsk { asker, number },
                },
            },
            due: Due::FromTaking,
        }],
        ..TickInputs::default()
    }
}

fn answered(output: &TickOutput, to: EntityId) -> Option<bool> {
    asks(output).into_iter().find_map(|(key, by)| match by {
        EntityUse::BreedAnswer { accepted, .. } if key == to => Some(accepted),
        _ => None,
    })
}

#[test]
fn the_partner_s_region_breeds_with_an_asker_seen_in_a_copy_and_answers_it() {
    let asker = EntityId(1 << 34);
    let beyond = cow_at(asker, 33.5, 0.5, &in_love());
    let mut region = at_a_line(&[cow_at(ONE, 30.5, 0.5, &in_love())], &[beyond]);
    let output = region.tick(&request(ONE, asker, 7));
    assert_eq!(answered(&output, asker), Some(true));
    // The child, the orb and the hearts are at the partner, as the game's `this`.
    let cows = all_of(&region, entity_types::COW);
    let child = cows.iter().find(|cow| cow.key != ONE).expect("a calf");
    assert_eq!(child.position, record(&region, ONE).unwrap().position);
    assert_eq!(entity_events(&output, ONE), [18]);
    assert_eq!(all_of(&region, entity_types::EXPERIENCE_ORB).len(), 1);
    assert_eq!(shared(&region, ONE).age, animal::BRED_AGE);
    // No twins: a second request finds it out of love.
    let output = region.tick(&request(ONE, asker, 8));
    assert_eq!(answered(&output, asker), Some(false));
    assert_eq!(all_of(&region, entity_types::COW).len(), 2);
}

#[test]
fn an_animal_waiting_on_its_own_answer_refuses_every_request() {
    let asker = EntityId(1 << 34);
    let beyond = cow_at(asker, 33.5, 0.5, &in_love());
    let mut waiting = in_love();
    waiting.asked = Some(Asked {
        partner: EntityId(77),
        number: 1,
        at: 0,
    });
    let mut region = at_a_line(&[cow_at(ONE, 30.5, 0.5, &waiting)], &[beyond]);
    // Its mark is as of its `lived`, which the region's ticks so far have not moved far.
    assert!(shared(&region, ONE).waiting(lived(&region, ONE)).is_some());
    let output = region.tick(&request(ONE, asker, 1));
    assert_eq!(answered(&output, asker), Some(false));
    assert_eq!(all_of(&region, entity_types::COW).len(), 1);
}

#[test]
fn a_tick_of_farm_animals_is_a_function_of_its_inputs() {
    let run = || {
        let grass: Vec<(BlockPos, clustine_data::BlockState)> = (0..16)
            .flat_map(|x| (0..16).map(move |z| (BlockPos::new(x, -61, z), blocks::GRASS_BLOCK)))
            .collect();
        let mut region = on_ticking_floor_with(&grass);
        lay(&mut region, &cow_at(ONE, 2.5, 3.5, &in_love()));
        lay(&mut region, &cow_at(TWO, 4.0, 3.5, &in_love()));
        let mut lamb = SheepState::default();
        lamb.set_sheared(true);
        lay(
            &mut region,
            &animal_of(
                EntityId(44),
                entity_types::SHEEP,
                [8.5, -60.0, 8.5],
                &baby(-5000),
                &lamb,
            ),
        );
        lay(
            &mut region,
            &animal_of(
                EntityId(46),
                entity_types::RABBIT,
                [10.5, -60.0, 2.5],
                &Animal::default(),
                &RabbitState::default(),
            ),
        );
        lay(
            &mut region,
            &animal_of(
                EntityId(48),
                entity_types::CHICKEN,
                [12.5, -60.0, 12.5],
                &Animal::default(),
                &ChickenState {
                    egg: 30,
                    ..ChickenState::default()
                },
            ),
        );
        let mut changes = Vec::new();
        for _ in 0..200 {
            let output = checked(&mut region, &TickInputs::default());
            changes.push((output.entity_changes.clone(), output.effects.clone()));
        }
        changes
    };
    assert_eq!(run(), run());
}

/// A breeding parent of `kind` and its partner, each in love, with their own states.
fn pair<O: Own>(kind: i32, this: &O, partner: &O) -> (Region, Entity) {
    let mut region = on_ticking_floor_with(&[]);
    let first = animal_of(ONE, kind, [2.5, -60.0, 2.5], &in_love(), this);
    lay(&mut region, &first);
    lay(
        &mut region,
        &animal_of(TWO, kind, [4.5, -60.0, 2.5], &in_love(), partner),
    );
    (region, first)
}

/// What `breed_with` makes of the pair: the child's type state and the orb's worth;
/// and the breeding parent's stream as it was before.
fn breed(region: &mut Region, kind: i32) -> (Random, TypeState, i32, Entity) {
    let replay = region.entity_stream(ONE).random;
    let mut output = TickOutput::default();
    assert!(region.breed_with(TWO, ONE, &mut output));
    let child = all_of(region, kind)
        .into_iter()
        .find(|mob| mob.key != ONE && mob.key != TWO)
        .expect("a child");
    let state = Mob::decode(&child.data).unwrap().state;
    let orbs = all_of(region, entity_types::EXPERIENCE_ORB);
    let [orb] = &orbs[..] else {
        panic!("one orb");
    };
    let EntityData::ExperienceOrb { value, .. } = orb.data else {
        unreachable!();
    };
    assert_eq!(shared(region, ONE).age, animal::BRED_AGE);
    assert_eq!(shared(region, TWO).age, animal::BRED_AGE);
    (replay, state, value, child)
}

/// The child's draws are the breeding parent's (`this`), in the game's order: a cow's
/// variant by one `nextBoolean`, a rabbit's by the level's `nextInt(100)` and then
/// `nextInt(20)` and `nextBoolean`, a sheep's mixed colour or the level's
/// `nextBoolean`; then the orb's `nextInt(7) + 1`.
#[test]
fn a_breeding_draws_the_child_and_its_orb_from_the_breeding_parent_s_stream() {
    let cold = animal::variants::id("minecraft:cow_variant", "minecraft:cold");
    let warm = animal::variants::id("minecraft:cow_variant", "minecraft:warm");
    let cow = |variant| CowState {
        variant,
        ..CowState::default()
    };
    let (mut region, first) = pair(entity_types::COW, &cow(cold), &cow(warm));
    let (mut replay, state, value, child) = breed(&mut region, entity_types::COW);
    let expected = if replay.next_bool() { cold } else { warm };
    assert_eq!(animal::read::<CowState>(&state).1.variant, expected);
    assert_eq!(value, replay.next_int(7) as i32 + 1);
    assert_eq!(child.position, first.position);
    assert!(animal::animal_of(&state).is_baby());

    let rabbit = |variant| RabbitState {
        variant,
        ..RabbitState::default()
    };
    let (mut region, _) = pair(entity_types::RABBIT, &rabbit(1), &rabbit(2));
    let (mut replay, state, value, _) = breed(&mut region, entity_types::RABBIT);
    let biome = Some("minecraft:badlands");
    let mut variant = crate::mobs::types::rabbit::random_variant(biome, &mut replay);
    if replay.next_int(20) != 0 {
        variant = if replay.next_bool() { 2 } else { 1 };
    }
    assert_eq!(animal::read::<RabbitState>(&state).1.variant, variant);
    assert_eq!(value, replay.next_int(7) as i32 + 1);

    let wool = |colour| {
        let mut state = SheepState::default();
        state.set_colour(colour);
        state
    };
    // Red and green mix by no recipe: one of the two, by the level's draw.
    let (mut region, _) = pair(entity_types::SHEEP, &wool(14), &wool(13));
    let (mut replay, state, value, _) = breed(&mut region, entity_types::SHEEP);
    let expected = if replay.next_bool() { 14 } else { 13 };
    assert_eq!(animal::read::<SheepState>(&state).1.colour(), expected);
    assert_eq!(value, replay.next_int(7) as i32 + 1);
    // White and red make a pink lamb, with no draw for it.
    let (mut region, _) = pair(entity_types::SHEEP, &wool(0), &wool(14));
    let (mut replay, state, value, _) = breed(&mut region, entity_types::SHEEP);
    assert_eq!(animal::read::<SheepState>(&state).1.colour(), 6);
    assert_eq!(value, replay.next_int(7) as i32 + 1);
}

/// No state of a type's own: an animal as its type makes it fresh.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct Fresh;

impl Own for Fresh {}

/// Each farm animal, its food and the speed of its panic.
const FARM: [(i32, i32, f64); 6] = [
    (entity_types::COW, items::WHEAT, 2.0),
    (entity_types::MOOSHROOM, items::WHEAT, 2.0),
    (entity_types::PIG, items::CARROT, 1.25),
    (entity_types::SHEEP, items::WHEAT, 1.25),
    (entity_types::CHICKEN, items::WHEAT_SEEDS, 1.4),
    (entity_types::RABBIT, items::CARROT, 2.2),
];

#[test]
fn every_farm_animal_follows_its_food_panics_when_hurt_and_breeds_when_fed_in_pairs() {
    for (kind, food, panic) in FARM {
        // Tempted: it comes to the player holding its food.
        let mut region = on_ticking_floor_with(&[]);
        holding(&mut region, one(food));
        lay(
            &mut region,
            &animal_of(ONE, kind, [7.5, -60.0, 0.5], &Animal::default(), &Fresh),
        );
        idle_for(&mut region, 300);
        let near = record(&region, ONE).unwrap().position;
        assert!(
            (near[0] - 0.5).abs() < 4.0 && (near[2] - 0.5).abs() < 4.0,
            "{kind} tempted to the player: {near:?}"
        );

        // Hurt, it panics at its speed.
        let mut region = on_ticking_floor_with(&[]);
        lay(
            &mut region,
            &animal_of(ONE, kind, [2.5, -60.0, 0.5], &Animal::default(), &Fresh),
        );
        checked(
            &mut region,
            &moves(vec![numbered(
                1,
                PlayerInput::Attack {
                    part: None,
                    entity: ONE,
                },
            )]),
        );
        // It makes no path until it lands from the knock-back (`canUpdatePath`).
        let panicked = (0..40).any(|_| {
            checked(&mut region, &TickInputs::default());
            let hurt = record(&region, ONE).unwrap();
            let speed = mob(&region, ONE).body.progress.speed;
            // A rabbit's speed is nothing between its hops.
            crate::mobs::ai::panicking_record(&hurt)
                && (speed == panic || kind == entity_types::RABBIT)
        });
        assert!(panicked, "{kind} panics");

        // Fed in pairs, they breed.
        let mut region = on_ticking_floor_with(&[]);
        lay(
            &mut region,
            &animal_of(ONE, kind, [2.5, -60.0, 3.5], &Animal::default(), &Fresh),
        );
        lay(
            &mut region,
            &animal_of(TWO, kind, [4.0, -60.0, 3.5], &Animal::default(), &Fresh),
        );
        holding(&mut region, one(food));
        assert_eq!(
            entity_events(&use_on(&mut region, ONE), ONE),
            [18],
            "{kind}"
        );
        assert_eq!(
            entity_events(&use_on(&mut region, TWO), TWO),
            [18],
            "{kind}"
        );
        let mut child = None;
        for _ in 0..400 {
            checked(&mut region, &TickInputs::default());
            child = all_of(&region, kind)
                .into_iter()
                .find(|mob| mob.key != ONE && mob.key != TWO);
            if child.is_some() {
                break;
            }
        }
        let child = child.unwrap_or_else(|| panic!("{kind} bred"));
        let state = Mob::decode(&child.data).unwrap().state;
        assert!(animal::animal_of(&state).is_baby(), "{kind}");
        for parent in [ONE, TWO] {
            assert_eq!(shared(&region, parent).age, animal::BRED_AGE, "{kind}");
        }
    }
}

/// A hit on `target` from a goat at (30.5, 0.5), with a knockback and a sound.
fn hit_on(target: EntityId) -> crate::mobs::tick::Local {
    crate::mobs::tick::Local::Hurt {
        key: target,
        kind: entity_types::GOAT as u16,
        from: [30.5, -60.0, 0.5],
        amount: 3.0,
        sound: Some(sounds::ENTITY_PUFFER_FISH_STING),
        knockback: Some([1.25, -1.0, 0.0]),
        fling: None,
    }
}

/// A mob's hit on a mob only a copy shows goes to its holder whole, as an effect at its
/// block (`docs/groundwork/n7j-plan.md`, section 3 item 5).
#[test]
fn a_mobs_hit_on_a_mob_in_a_copy_goes_to_its_holder() {
    let beyond = EntityId(5);
    let mut region = at_a_line(&[], &[cow_at(beyond, 32.5, 0.5, &Animal::default())]);
    let mut output = TickOutput::default();
    region.apply_locals(ONE, vec![hit_on(beyond)], &mut output);
    // Effects made between ticks go with the next tick's.
    let output = region.tick(&TickInputs::default());
    assert_eq!(
        asks(&output),
        [(
            beyond,
            EntityUse::MobHit {
                attacker: ONE,
                kind: entity_types::GOAT as u16,
                from: [Double(30.5), Double(-60.0), Double(0.5)],
                damage: Float(3.0),
                knockback: Some([Double(1.25), Double(-1.0), Double(0.0)]),
                fling: None,
                sound: Some(sounds::ENTITY_PUFFER_FISH_STING),
                thorns: false,
            }
        )]
    );
    // A mob beyond the copies is not there: nothing is sent.
    let mut output = TickOutput::default();
    region.apply_locals(ONE, vec![hit_on(EntityId(77))], &mut output);
    assert!(asks(&region.tick(&TickInputs::default())).is_empty());
}

/// The holder hits its mob as a hit within the region would: the same health and motion
/// after, and the sound where it took.
#[test]
fn a_holder_hits_its_mob_as_a_hit_within_its_region_would() {
    let cow = cow_at(ONE, 20.5, 0.5, &Animal::default());
    let mut within = at_a_line(std::slice::from_ref(&cow), &[]);
    let mut output = TickOutput::default();
    within.apply_locals(EntityId(5), vec![hit_on(ONE)], &mut output);

    let mut held = at_a_line(&[cow], &[]);
    let hit = TickInputs {
        effects: vec![FiledEffect {
            id: EffectId {
                sequence: 4,
                index: 0,
            },
            effect: Effect {
                at: BlockPos::new(20, -60, 0),
                what: Effected::OnEntity {
                    key: ONE,
                    by: EntityUse::MobHit {
                        attacker: EntityId(5),
                        kind: entity_types::GOAT as u16,
                        from: [Double(30.5), Double(-60.0), Double(0.5)],
                        damage: Float(3.0),
                        knockback: Some([Double(1.25), Double(-1.0), Double(0.0)]),
                        fling: None,
                        sound: Some(sounds::ENTITY_PUFFER_FISH_STING),
                        thorns: false,
                    },
                },
            },
            due: Due::FromTaking,
        }],
        ..TickInputs::default()
    };
    let effected = held.tick(&hit);
    let health = |region: &Region| {
        Mob::decode(&record(region, ONE).unwrap().data)
            .unwrap()
            .body
            .vitals
            .health
    };
    let full = health(&at_a_line(
        &[cow_at(ONE, 20.5, 0.5, &Animal::default())],
        &[],
    ));
    assert!(health(&held) < full, "hurt");
    assert_eq!(health(&held), health(&within));
    // Its push is the same, but for the tick the holder ran after the hit.
    let motion = record(&held, ONE).unwrap().motion;
    assert!(motion[0] > 0.5, "{motion:?}");
    assert!(effected.events.iter().any(|event| matches!(
        event,
        RegionEvent::Sound { sound, .. } if *sound == sounds::ENTITY_PUFFER_FISH_STING
    )));
}
