//! The wild land animals in a region, from the brief's sections 6.1 to 6.4 and 6.11:
//! eggs make them with their draws in the game's order; a fox wanders, flees a bear,
//! sleeps sheltered by day, hunts, carries and eats, and picks berries; a panda sits to
//! eat what it is given and breeds only with bamboo near; a bear hunts a fox; a bee
//! hovers, pollinates, goes into its hive and is let out; a bat hangs and wakes; and a
//! tick of them is a function of its inputs.

use clustine_data::{blocks, entity_types, items, sounds};
use clustine_world::{Entity, EntityChange, EntityData, EntityId};

use super::*;
use crate::living::MAX_AIR;
use crate::mobs::Mob;
use crate::mobs::types::animal::{self, Animal, Own};
use crate::mobs::types::bat::{self, BatState};
use crate::mobs::types::bee::{self, BeeState};
use crate::mobs::types::fox::{self, FoxState};
use crate::mobs::types::panda::{self, PandaState};
use crate::mobs::types::polar_bear::{self, BearState};
use crate::region::hives::Hive;

const ONE: EntityId = EntityId(40);
const TWO: EntityId = EntityId(42);

/// A mob of the type `kind` at `position`, standing, at its type's full health, with
/// the type state `state`.
fn mob_of(
    key: EntityId,
    kind: i32,
    position: [f64; 3],
    state: clustine_world::TypeState,
) -> Entity {
    let mut mob = Mob::fresh();
    let kind = kind as u16;
    mob.body.vitals.health = crate::mobs::body::attributes::supplied(kind, 23).unwrap() as f32;
    mob.body.vitals.air = MAX_AIR;
    mob.state = state;
    if i32::from(kind) == entity_types::FOX || i32::from(kind) == entity_types::PANDA {
        mob.body.flags.picks_up_loot = true;
    }
    Entity {
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

fn animal_at<O: Own>(key: EntityId, kind: i32, position: [f64; 3], own: &O) -> Entity {
    mob_of(key, kind, position, animal::write(&Animal::default(), own))
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

fn all_of(region: &Region, kind: i32) -> Vec<Entity> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter(|entity| i32::from(entity.kind) == kind)
        .cloned()
        .collect()
}

fn items_lying(region: &Region) -> Vec<(EntityId, ItemStack)> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter_map(|entity| match entity.data {
            EntityData::Item { stack, .. } => Some((entity.key, stack)),
            _ => None,
        })
        .collect()
}

fn item_at(key: EntityId, stack: ItemStack, position: [f64; 3]) -> Entity {
    Entity {
        key,
        kind: entity_types::ITEM as u16,
        uuid: uuid::Uuid::from_u128(u128::from(key.0)),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: EntityData::Item {
            stack,
            pick_up_delay: 0,
            thrower: None,
            health: 5,
            age_offset: 0,
        },
    }
}

fn holding(region: &mut Region, stack: ItemStack) {
    checked(
        region,
        &moves(vec![set_slot(1, 0, Some(stack)), select(1, 0)]),
    );
}

fn one(item: i32) -> ItemStack {
    ItemStack { item, count: 1 }
}

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

fn sounds_of(output: &TickOutput) -> Vec<i32> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound { sound, .. } => Some(*sound),
            _ => None,
        })
        .collect()
}

fn idle(region: &mut Region) -> TickOutput {
    checked(region, &TickInputs::default())
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    crate::physics::distance_sqr(a, b).sqrt()
}

fn fox_hand(region: &Region, key: EntityId) -> Option<ItemStack> {
    mob(region, key).body.equipment.slots[0]
}

#[test]
fn an_egg_makes_each_wild_land_animal_with_its_draws_in_the_game_s_order() {
    for kind in [
        entity_types::FOX,
        entity_types::PANDA,
        entity_types::POLAR_BEAR,
        entity_types::BEE,
        entity_types::BAT,
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
        // Its own stream from its start: its constructor drew from it before it was in
        // its chunk.
        let mut own = Random::of_entity_aside(region.config.seed, key, 0, region.tick);
        let _uuid = (own.next_u64(), own.next_u64());
        let _yaw = own.next_float();
        let _yaw = level.next_float();
        let made = mob(&region, key);
        match kind {
            entity_types::FOX => {
                let state = fox::state_of(&made);
                assert_eq!(state.variant, fox::variant::RED);
                assert!(state.hunts);
                assert_eq!(state.sleep_countdown, own.next_int(70) as i32);
                assert!(made.body.flags.picks_up_loot);
                // `populateDefaultEquipmentSlots`'s first draw, and the item's where it
                // holds one.
                if level.next_float() < 0.2 {
                    let f = level.next_float();
                    if (0.2..0.4).contains(&f) {
                        level.next_bool();
                    }
                    assert!(made.body.equipment.slots[0].is_some());
                } else {
                    assert_eq!(made.body.equipment.slots[0], None);
                }
            }
            entity_types::PANDA => {
                let state = panda::state_of(&made);
                assert_eq!(state.main_gene, panda::random_gene(&mut level));
                assert_eq!(state.hidden_gene, panda::random_gene(&mut level));
            }
            entity_types::POLAR_BEAR => {
                assert_eq!(polar_bear::state_of(&made), BearState::default());
            }
            entity_types::BEE => {
                let state = bee::state_of(&made);
                assert_eq!(state.validate_hive_cooldown, own.next_int(21) as i32 + 20);
                assert_eq!(state.validate_flower_cooldown, own.next_int(21) as i32 + 20);
                assert_eq!(state.flower_cooldown(0), i64::from(own.next_int(41)) + 20);
                assert!(!state.no_gravity);
            }
            _ => {
                assert!(bat::state_of(&made).resting);
            }
        }
        // Then `Mob`'s: the follow range's bonus and the hand.
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
    }
}

#[test]
fn a_fox_wanders_and_looks_about() {
    let mut region = on_ticking_floor_with(&[]);
    let fox = animal_at(
        ONE,
        entity_types::FOX,
        [7.5, -60.0, 7.5],
        &FoxState::default(),
    );
    lay(&mut region, &fox);
    let mut moved = false;
    for _ in 0..600 {
        idle(&mut region);
        let now = record(&region, ONE).unwrap();
        moved |= distance(now.position, fox.position) > 1.0;
    }
    assert!(moved);
}

#[test]
fn a_fox_runs_from_a_polar_bear_near_it() {
    let mut region = on_ticking_floor_with(&[]);
    let fox = animal_at(
        ONE,
        entity_types::FOX,
        [6.5, -60.0, 6.5],
        &FoxState::default(),
    );
    // A cub, which hunts no fox: what the fox does is its own.
    let mut cub = Animal::default();
    cub.set_age(animal::BABY_START_AGE, 0);
    let bear = mob_of(
        TWO,
        entity_types::POLAR_BEAR,
        [9.5, -60.0, 6.5],
        animal::write(&cub, &BearState::default()),
    );
    lay(&mut region, &fox);
    lay(&mut region, &bear);
    let mut fled = false;
    for _ in 0..60 {
        idle(&mut region);
        let fox_at = record(&region, ONE).unwrap().position;
        let bear_at = record(&region, TWO).unwrap().position;
        fled |= distance(fox_at, bear_at) > 6.0;
    }
    assert!(fled);
}

#[test]
fn a_fox_lies_down_to_sleep_by_day_under_a_roof_with_nothing_about() {
    // Grass under it, stone over it.
    let mut placed = vec![(BlockPos::new(8, -61, 8), blocks::GRASS_BLOCK)];
    for x in 5..12 {
        for z in 5..12 {
            placed.push((BlockPos::new(x, -58, z), blocks::STONE));
        }
    }
    let mut region = on_ticking_floor_with(&placed);
    let fox = animal_at(
        ONE,
        entity_types::FOX,
        [8.5, -60.0, 8.5],
        &FoxState::default(),
    );
    lay(&mut region, &fox);
    let mut slept = None;
    for tick in 0..100 {
        let output = idle(&mut region);
        if fox::state_of(&mob(&region, ONE)).has(fox::flag_bits::SLEEPING) {
            slept = Some((tick, output));
            break;
        }
    }
    let (_, output) = slept.expect("asleep");
    // Clients see its flags.
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::EntityTracked(report) if report.entity == ONE && report.data.is_some()
    )));
    // Asleep, it does not walk.
    let before = record(&region, ONE).unwrap().position;
    for _ in 0..40 {
        idle(&mut region);
    }
    let after = record(&region, ONE).unwrap().position;
    assert!(distance(before, after) < 0.01, "{before:?} {after:?}");
    // In the open it would not sleep.
    let mut region = on_ticking_floor_with(&[(BlockPos::new(8, -61, 8), blocks::GRASS_BLOCK)]);
    lay(&mut region, &fox);
    for _ in 0..100 {
        idle(&mut region);
        assert!(!fox::state_of(&mob(&region, ONE)).has(fox::flag_bits::SLEEPING));
    }
}

#[test]
fn a_hunting_fox_stalks_a_chicken_and_bites_it() {
    let mut region = on_ticking_floor_with(&[]);
    let fox = animal_at(
        ONE,
        entity_types::FOX,
        [3.5, -60.0, 3.5],
        &FoxState {
            hunts: true,
            ..FoxState::default()
        },
    );
    let chicken = animal_at(
        TWO,
        entity_types::CHICKEN,
        [12.5, -60.0, 12.5],
        &crate::mobs::types::chicken::ChickenState::default(),
    );
    lay(&mut region, &fox);
    lay(&mut region, &chicken);
    let mut targeted = false;
    let mut bitten = false;
    for _ in 0..1200 {
        idle(&mut region);
        targeted |= mob(&region, ONE).body.combat.target == Some(TWO);
        bitten |= record(&region, TWO)
            .is_none_or(|chicken| Mob::decode(&chicken.data).unwrap().body.vitals.health < 4.0);
        if bitten {
            break;
        }
    }
    assert!(targeted, "it took the chicken as its target");
    assert!(bitten, "it bit the chicken");
}

#[test]
fn a_fox_takes_what_it_touches_into_its_mouth_and_spits_out_what_is_no_food_for_food() {
    let mut region = on_ticking_floor_with(&[]);
    let fox = animal_at(
        ONE,
        entity_types::FOX,
        [6.5, -60.0, 6.5],
        &FoxState::default(),
    );
    lay(&mut region, &fox);
    lay(
        &mut region,
        &item_at(
            EntityId(60),
            ItemStack {
                item: items::STICK,
                count: 3,
            },
            [6.5, -60.0, 6.9],
        ),
    );
    let output = idle(&mut region);
    assert_eq!(fox_hand(&region, ONE), Some(one(items::STICK)));
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::ItemTaken { item, collector, count: 0, .. }
            if *item == EntityId(60) && *collector == ONE
    )));
    // The rest of the stack is put down at its feet.
    let lying = items_lying(&region);
    assert_eq!(lying.len(), 1, "{lying:?}");
    assert_eq!(
        lying[0].1,
        ItemStack {
            item: items::STICK,
            count: 2
        }
    );
    assert_eq!(mob(&region, ONE).body.equipment.drop_chance(0), 2.0);
    // Food comes in place of what is no food, once it has eaten since it took it.
    for _ in 0..3 {
        idle(&mut region);
    }
    let at = record(&region, ONE).unwrap().position;
    lay(&mut region, &item_at(EntityId(62), one(items::APPLE), at));
    let output = idle(&mut region);
    assert_eq!(fox_hand(&region, ONE), Some(one(items::APPLE)));
    assert!(sounds_of(&output).contains(&sounds::ENTITY_FOX_SPIT));
}

#[test]
fn a_fox_eats_the_food_in_its_mouth_half_a_minute_after_taking_it() {
    // Walled in, so that it does not stroll out of the chunk the region ticks.
    let mut walls = Vec::new();
    for (dx, dz) in [
        (-1, -1),
        (-1, 0),
        (-1, 1),
        (0, -1),
        (0, 1),
        (1, -1),
        (1, 0),
        (1, 1),
    ] {
        for y in [-60, -59] {
            walls.push((BlockPos::new(8 + dx, y, 8 + dz), blocks::STONE));
        }
    }
    let mut region = on_ticking_floor_with(&walls);
    let mut fox = animal_at(
        ONE,
        entity_types::FOX,
        [8.5, -60.0, 8.5],
        &FoxState::default(),
    );
    let mut body = Mob::decode(&fox.data).unwrap();
    body.body.equipment.slots[0] = Some(one(items::SWEET_BERRIES));
    fox.data = body.encode();
    lay(&mut region, &fox);
    let mut eaten = None;
    for tick in 0..700 {
        idle(&mut region);
        if fox_hand(&region, ONE).is_none() {
            eaten = Some(tick);
            break;
        }
    }
    // Its 601st tick since it took it.
    assert_eq!(eaten, Some(600));
}

#[test]
fn a_fox_picks_a_ripe_berry_bush_and_leaves_it_at_age_one() {
    let bush = BlockPos::new(8, -60, 6);
    let ripe = clustine_blocks::data::set(blocks::SWEET_BERRY_BUSH, "age", "3");
    let mut region = on_ticking_floor_with(&[
        (BlockPos::new(8, -61, 6), blocks::GRASS_BLOCK),
        (bush, ripe),
    ]);
    let fox = animal_at(
        ONE,
        entity_types::FOX,
        [6.5, -60.0, 6.5],
        &FoxState::default(),
    );
    lay(&mut region, &fox);
    let mut picked = false;
    for _ in 0..400 {
        let output = idle(&mut region);
        if region
            .block(bush)
            .and_then(|state| clustine_blocks::data::number(state, "age"))
            == Some(1)
        {
            assert!(sounds_of(&output).contains(&sounds::BLOCK_SWEET_BERRY_BUSH_PICK_BERRIES));
            picked = true;
            break;
        }
    }
    assert!(picked);
    // One berry in its mouth, and one or two more popped out.
    assert_eq!(fox_hand(&region, ONE), Some(one(items::SWEET_BERRIES)));
    let berries: i32 = items_lying(&region)
        .iter()
        .filter(|(_, stack)| stack.item == items::SWEET_BERRIES)
        .map(|(_, stack)| stack.count)
        .sum();
    assert!((1..=2).contains(&berries), "{berries}");
}

#[test]
fn a_panda_in_love_fed_bamboo_sits_down_and_eats_it() {
    let mut region = on_ticking_floor_with(&[]);
    let panda = animal_at(
        ONE,
        entity_types::PANDA,
        [3.5, -60.0, 3.5],
        &PandaState::default(),
    );
    lay(&mut region, &panda);
    holding(&mut region, one(items::BAMBOO));
    // The first bamboo puts it in love.
    use_on(&mut region, ONE);
    let shared = animal::animal_of(&mob(&region, ONE).state);
    assert!(shared.in_love(record(&region, ONE).unwrap().lived));
    // The second it takes into its hand and sits down to eat.
    use_on(&mut region, ONE);
    let own = panda::state_of(&mob(&region, ONE));
    assert!(own.has(panda::flag_bits::SITTING));
    // It began to eat as it was fed, and its tick after counted on.
    assert!(own.eat >= 1, "{own:?}");
    assert_eq!(
        mob(&region, ONE).body.equipment.slots[0],
        Some(one(items::BAMBOO))
    );
    let mut ate = false;
    for _ in 0..800 {
        let output = idle(&mut region);
        if mob(&region, ONE).body.equipment.slots[0].is_none() {
            ate = true;
            break;
        }
        let eating = panda::state_of(&mob(&region, ONE)).eat;
        if eating > 0 && eating % 5 == 1 && eating > 1 {
            assert!(sounds_of(&output).contains(&sounds::ENTITY_PANDA_EAT));
        }
    }
    assert!(ate);
}

#[test]
fn two_pandas_in_love_breed_only_with_bamboo_near_and_are_unhappy_without() {
    let lovers = |region: &mut Region| {
        let mut love = Animal::default();
        love.set_in_love(0);
        for (key, x) in [(ONE, 3.5), (TWO, 5.0)] {
            let panda = mob_of(
                key,
                entity_types::PANDA,
                [x, -60.0, 3.5],
                animal::write(&love, &PandaState::default()),
            );
            lay(region, &panda);
        }
    };
    let mut region = on_ticking_floor_with(&[]);
    lovers(&mut region);
    let mut unhappy = false;
    for _ in 0..200 {
        let output = idle(&mut region);
        unhappy |= [ONE, TWO]
            .iter()
            .any(|key| panda::state_of(&mob(&region, *key)).unhappy > 0);
        assert_eq!(all_of(&region, entity_types::PANDA).len(), 2);
        let _ = output;
    }
    assert!(unhappy);
    // With bamboo by them, they breed.
    let mut region = on_ticking_floor_with(&[(BlockPos::new(4, -60, 6), blocks::BAMBOO)]);
    lovers(&mut region);
    let mut bred = false;
    for _ in 0..300 {
        idle(&mut region);
        if all_of(&region, entity_types::PANDA).len() == 3 {
            bred = true;
            break;
        }
    }
    assert!(bred);
}

#[test]
fn a_grown_polar_bear_hunts_a_fox_stands_up_to_bite_and_is_angry_at_it() {
    let mut region = on_ticking_floor_with(&[]);
    let bear = animal_at(
        TWO,
        entity_types::POLAR_BEAR,
        [3.5, -60.0, 3.5],
        &BearState::default(),
    );
    let fox = animal_at(
        ONE,
        entity_types::FOX,
        [8.5, -60.0, 3.5],
        &FoxState::default(),
    );
    lay(&mut region, &bear);
    lay(&mut region, &fox);
    let mut stood = false;
    let mut angry = false;
    let mut hurt = false;
    for _ in 0..600 {
        idle(&mut region);
        let own = polar_bear::state_of(&mob(&region, TWO));
        stood |= own.standing;
        angry |= own.anger.target == Some(ONE) && own.anger.end > region.clock.game_time as i64;
        hurt |= record(&region, ONE)
            .is_none_or(|fox| Mob::decode(&fox.data).unwrap().body.vitals.health < 10.0);
        if hurt && stood && angry {
            break;
        }
    }
    assert!(angry, "angry at its prey");
    assert!(stood, "stood up before a bite");
    assert!(hurt, "bit the fox");
}

#[test]
fn an_egg_s_bee_hovers_once_it_moves_and_wanders_in_the_air() {
    let mut region = on_ticking_floor_with(&[]);
    let mut output = TickOutput::default();
    let key = region
        .spawn_mob(
            entity_types::BEE as u16,
            player(1),
            BlockPos::new(6, -60, 6),
            true,
            true,
            &mut output,
        )
        .unwrap();
    let start = record(&region, key).unwrap().position;
    let mut hovered = false;
    let mut moved = false;
    let mut aloft = false;
    for _ in 0..400 {
        idle(&mut region);
        let now = record(&region, key).unwrap();
        hovered |= bee::state_of(&Mob::decode(&now.data).unwrap()).no_gravity;
        moved |= distance(now.position, start) > 1.5;
        aloft |= now.position[1] > -59.0;
    }
    assert!(hovered && moved && aloft, "{hovered} {moved} {aloft}");
    // Clients are told it has no gravity.
    let values = crate::shown::mob_values(
        entity_types::BEE as u16,
        &record(&region, key).unwrap().data,
    );
    assert!(values.contains(&(5, crate::shown::DataShown::Boolean(true), false)));
}

#[test]
fn a_bee_pollinates_a_flower_near_it_and_comes_away_with_nectar() {
    let flower = BlockPos::new(8, -60, 8);
    let mut region = on_ticking_floor_with(&[
        (BlockPos::new(8, -61, 8), blocks::GRASS_BLOCK),
        (flower, blocks::POPPY),
    ]);
    let bee_state = BeeState::default();
    lay(
        &mut region,
        &animal_at(ONE, entity_types::BEE, [6.5, -60.0, 8.5], &bee_state),
    );
    let mut remembered = false;
    let mut nectar = false;
    for _ in 0..1500 {
        idle(&mut region);
        let own = bee::state_of(&mob(&region, ONE));
        remembered |= own.flower == Some(flower);
        if own.has(bee::flag_bits::HAS_NECTAR) {
            nectar = true;
            break;
        }
    }
    assert!(remembered && nectar, "{remembered} {nectar}");
}

/// A beehive facing south at `pos`, with its data `hive`.
fn with_hive(region: &mut Region, pos: BlockPos, hive: &Hive) {
    let state = clustine_blocks::data::set(blocks::BEEHIVE, "facing", "south");
    let chunk = region.chunks.get_mut(&pos.chunk()).unwrap();
    let (x, z) = pos.in_chunk();
    chunk.set(x, pos.y, z, state);
    chunk
        .set_block_entity_data(InChunk::new(x, pos.y, z), hive.write())
        .unwrap();
}

#[test]
fn a_bee_with_nectar_at_its_hive_goes_in_and_leaves_the_world() {
    let hive_at = BlockPos::new(8, -60, 8);
    let mut region = on_ticking_floor_with(&[]);
    with_hive(
        &mut region,
        hive_at,
        &Hive {
            bees: Vec::new(),
            flower: None,
            other: Vec::new(),
        },
    );
    let mut own = BeeState {
        hive: Some(hive_at),
        no_gravity: true,
        ..BeeState::default()
    };
    own.set_has_nectar(true, 0);
    lay(
        &mut region,
        &animal_at(ONE, entity_types::BEE, [8.5, -60.0, 9.6], &own),
    );
    let mut entered = None;
    for tick in 0..40 {
        let output = idle(&mut region);
        if record(&region, ONE).is_none() {
            entered = Some(output);
            let _ = tick;
            break;
        }
    }
    let output = entered.expect("it went in");
    assert!(sounds_of(&output).contains(&sounds::BLOCK_BEEHIVE_ENTER));
    let hive = region.hive(hive_at).unwrap();
    assert_eq!(hive.bees.len(), 1);
    assert_eq!(hive.bees[0].min_ticks, 2400);
    // The hive's data is committed but not shown.
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::BlockEntityChanged { position, seen: false, .. } if *position == hive_at
    )));
}

#[test]
fn a_hive_lets_out_a_bee_that_stayed_long_enough_by_day_and_its_nectar_raises_the_honey() {
    use crate::region::hives::{Hive, StoredBee};
    let hive_at = BlockPos::new(8, -60, 8);
    let stored = StoredBee {
        age: 0,
        love: 0,
        age_locked: false,
        flags: bee::flag_bits::HAS_NECTAR,
        anger: Default::default(),
        flower: Some(BlockPos::new(2, -60, 2)),
        health: 7.0,
    };
    let make = |region: &mut Region| {
        // A bee come in with nectar a moment ago, as the hive keeps it.
        let mut hive = Hive::read(&[]).unwrap();
        let now = region.clock.game_time as i64;
        hive.bees.push(crate::region::hives::occupant_for_tests(
            &stored,
            now - 2401,
        ));
        with_hive(region, hive_at, &hive);
    };
    // By night it stays in.
    let mut region = on_ticking_floor_with(&[]);
    region.clock.day = 18_000;
    make(&mut region);
    for _ in 0..5 {
        idle(&mut region);
    }
    assert!(all_of(&region, entity_types::BEE).is_empty());
    assert_eq!(region.hive(hive_at).unwrap().bees.len(), 1);
    // By day it comes out before the hive's front, without its nectar, and the hive's
    // honey is one higher.
    let mut region = on_ticking_floor_with(&[]);
    make(&mut region);
    let output = idle(&mut region);
    let bees = all_of(&region, entity_types::BEE);
    let [released] = &bees[..] else {
        panic!("one bee: {bees:?}");
    };
    assert!(sounds_of(&output).contains(&sounds::BLOCK_BEEHIVE_EXIT));
    assert!((released.position[2] - (8.5 + 0.55 + f64::from(0.55_f32 / 2.0))).abs() < 0.2);
    let released = Mob::decode(&released.data).unwrap();
    let own = bee::state_of(&released);
    assert!(!own.has(bee::flag_bits::HAS_NECTAR));
    assert_eq!(own.flower, stored.flower);
    assert_eq!(released.body.vitals.health, 7.0);
    assert_eq!(region.hive(hive_at).unwrap().bees.len(), 0);
    let honey = region
        .block(hive_at)
        .and_then(|state| clustine_blocks::data::number(state, "honey_level"));
    assert!(matches!(honey, Some(1 | 2)), "{honey:?}");
}

#[test]
fn a_resting_bat_hangs_under_a_solid_block_and_wakes_at_a_player_within_four_blocks() {
    let at = region_player_feet();
    let near = [at[0] + 2.0, -59.95, at[2]];
    let far = [at[0] + 12.0, -59.95, at[2] + 6.0];
    let ceiling = |feet: [f64; 3]| {
        let pos = crate::physics::containing(feet);
        (BlockPos::new(pos.x, pos.y + 1, pos.z), blocks::STONE)
    };
    let mut region = on_ticking_floor_with(&[ceiling(near), ceiling(far)]);
    let resting = BatState {
        resting: true,
        target: None,
    };
    for (key, feet) in [(ONE, near), (TWO, far)] {
        let mut mob = Mob::fresh();
        bat::put_state(&mut mob, &resting);
        let mut entity = mob_of(key, entity_types::BAT, feet, mob.state.clone());
        entity.on_ground = false;
        entity.motion = [0.0; 3];
        lay(&mut region, &entity);
    }
    let output = idle(&mut region);
    assert!(!bat::state_of(&mob(&region, ONE)).resting);
    assert!(
        output
            .events
            .iter()
            .any(|event| matches!(event, RegionEvent::LevelEvent { kind: 1025, .. }))
    );
    // The far one hangs on, its feet a tenth of a block over the block it is in.
    for _ in 0..20 {
        idle(&mut region);
    }
    let far_now = record(&region, TWO).unwrap();
    assert!(bat::state_of(&Mob::decode(&far_now.data).unwrap()).resting);
    let expected = (-60.0_f64) + 1.0 - f64::from(0.9_f32);
    assert!(
        (far_now.position[1] - expected).abs() < 1e-9,
        "{:?}",
        far_now.position
    );
    assert_eq!(far_now.motion, [0.0; 3]);
}

/// Where player 1 of a fresh floor stands.
fn region_player_feet() -> [f64; 3] {
    let region = on_ticking_floor_with(&[]);
    let at = region.players[&player(1)].pose.position;
    [at.x, at.y, at.z]
}

#[test]
fn a_flying_bat_steers_its_own_motion_towards_where_it_wants_to_go() {
    let mut region = on_ticking_floor_with(&[]);
    let mut mob = Mob::fresh();
    bat::put_state(
        &mut mob,
        &BatState {
            resting: false,
            target: None,
        },
    );
    let start = [10.5, -58.0, 10.5];
    let mut entity = mob_of(ONE, entity_types::BAT, start, mob.state.clone());
    entity.on_ground = false;
    entity.motion = [0.0; 3];
    lay(&mut region, &entity);
    for _ in 0..60 {
        idle(&mut region);
    }
    let now = record(&region, ONE).unwrap();
    let own = bat::state_of(&Mob::decode(&now.data).unwrap());
    assert!(!own.resting && own.target.is_some());
    assert!(distance(now.position, start) > 0.5);
}

#[test]
fn a_tick_of_wild_land_animals_is_a_function_of_its_inputs() {
    let run = || {
        let mut placed = vec![
            (
                BlockPos::new(8, -60, 6),
                clustine_blocks::data::set(blocks::SWEET_BERRY_BUSH, "age", "3"),
            ),
            (BlockPos::new(12, -60, 12), blocks::POPPY),
            (BlockPos::new(4, -60, 12), blocks::BAMBOO),
        ];
        placed.extend((0..16).map(|x| (BlockPos::new(x, -61, 3), blocks::GRASS_BLOCK)));
        let mut region = on_ticking_floor_with(&placed);
        lay(
            &mut region,
            &animal_at(
                ONE,
                entity_types::FOX,
                [3.5, -60.0, 3.5],
                &FoxState {
                    hunts: true,
                    ..FoxState::default()
                },
            ),
        );
        lay(
            &mut region,
            &animal_at(
                TWO,
                entity_types::PANDA,
                [5.5, -60.0, 10.5],
                &PandaState::default(),
            ),
        );
        lay(
            &mut region,
            &animal_at(
                EntityId(44),
                entity_types::POLAR_BEAR,
                [12.5, -60.0, 3.5],
                &BearState::default(),
            ),
        );
        lay(
            &mut region,
            &animal_at(
                EntityId(46),
                entity_types::BEE,
                [11.5, -60.0, 11.5],
                &BeeState::default(),
            ),
        );
        let mut bat_mob = Mob::fresh();
        bat::put_state(
            &mut bat_mob,
            &BatState {
                resting: false,
                target: None,
            },
        );
        let mut flying = mob_of(
            EntityId(48),
            entity_types::BAT,
            [8.5, -57.0, 8.5],
            bat_mob.state.clone(),
        );
        flying.on_ground = false;
        lay(&mut region, &flying);
        lay(
            &mut region,
            &animal_at(
                EntityId(50),
                entity_types::CHICKEN,
                [10.5, -60.0, 1.5],
                &crate::mobs::types::chicken::ChickenState::default(),
            ),
        );
        let mut changes = Vec::new();
        for _ in 0..200 {
            let output = idle(&mut region);
            changes.push((output.entity_changes.clone(), output.effects.clone()));
        }
        changes
    };
    assert_eq!(run(), run());
}
