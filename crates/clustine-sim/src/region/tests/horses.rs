//! The horse family in a region, from the brief's sections 5.9 to 5.18: eggs make each
//! kind with its stats, a wild horse with a player on it is tamed by its temper or throws
//! them, a use feeds, saddles and arms a tamed one, two tamed horses in love breed and a
//! horse and a donkey make a mule, a sneaking owner opens a donkey's menu and what goes
//! in its saddle's slot goes on, a llama joins a caravan and spits at a wolf, a trader
//! llama leaves when its time runs out, and a tick of them is a function of its inputs.

use clustine_data::{entity_types, items, sounds};
use clustine_world::{Double, Entity, EntityChange, EntityId, Leash, LeashHolder, Seat};

use super::*;
use crate::api::{ClickKind, MenuKind};
use crate::living::MAX_AIR;
use crate::mobs::Mob;
use crate::mobs::types::animal::{self, Animal};
use crate::mobs::types::equine::{self, HorseState};

const ONE: EntityId = EntityId(40);
const TWO: EntityId = EntityId(42);

const HORSE: i32 = entity_types::HORSE;
const DONKEY: i32 = entity_types::DONKEY;
const LLAMA: i32 = entity_types::LLAMA;

/// A horse of the family of the type `kind` at `position`, standing, at its type's full
/// health, with what every animal shares `shared` and its own `horse`.
fn horse_at(
    key: EntityId,
    kind: i32,
    position: [f64; 3],
    shared: &Animal,
    horse: &HorseState,
) -> Entity {
    let mut mob = Mob::fresh();
    let kind = kind as u16;
    mob.body.vitals.health = crate::mobs::body::attributes::supplied(kind, 23).unwrap() as f32;
    mob.body.vitals.air = MAX_AIR;
    mob.state = animal::write(shared, horse);
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

fn horse_of(region: &Region, key: EntityId) -> HorseState {
    equine::horse_of(&mob(region, key))
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

fn one(item: i32) -> ItemStack {
    ItemStack::new(item, 1)
}

fn holding(region: &mut Region, stack: Option<ItemStack>) {
    checked(region, &moves(vec![set_slot(1, 0, stack), select(1, 0)]));
}

/// Player 1's use of the entity `key` with what they hold.
fn use_on(region: &mut Region, key: EntityId, sneaking: bool) -> TickOutput {
    checked(
        region,
        &moves(vec![numbered(
            1,
            PlayerInput::Interact {
                entity: key,
                location: [0.0, 0.5, 0.0],
                sneaking,
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

fn tamed() -> HorseState {
    HorseState {
        tame: true,
        ..HorseState::default()
    }
}

fn in_love() -> Animal {
    let mut shared = Animal::default();
    shared.set_in_love(0);
    shared
}

#[test]
fn an_egg_makes_each_of_the_family_its_horse_s_coat_and_stats_drawn_after_its_yaw() {
    let family = [
        entity_types::HORSE,
        entity_types::DONKEY,
        entity_types::MULE,
        entity_types::LLAMA,
        entity_types::TRADER_LLAMA,
        entity_types::SKELETON_HORSE,
        entity_types::ZOMBIE_HORSE,
    ];
    for kind in family {
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
            .unwrap_or_else(|| panic!("{kind} made"));
        let made = mob(&region, key);
        let max = made
            .body
            .attributes
            .value(kind as u16, crate::mobs::body::attribute(23)) as f32;
        assert_eq!(made.body.vitals.health, max, "{kind}");
        if kind == entity_types::HORSE {
            let _yaw = level.next_float();
            let coat = level.next_int(7) as i32;
            let markings = level.next_int(5) as i32;
            let health = 15.0_f32 + level.next_int(8) as f32 + level.next_int(9) as f32;
            for _ in 0..6 {
                level.next_double();
            }
            let bonus = 0.114_850_000_000_000_01 * (level.next_double() - level.next_double());
            let left = level.next_float() < 0.05;
            let state = equine::horse_of(&made);
            assert_eq!(
                state.variant,
                crate::mobs::types::horse::type_variant(coat, markings)
            );
            assert_eq!(made.body.vitals.health, health);
            let follow = made
                .body
                .attributes
                .made(clustine_data::attributes::FOLLOW_RANGE as u16)
                .unwrap();
            assert_eq!(follow.modifiers[0].amount.to_bits(), bonus.to_bits());
            assert_eq!(made.body.flags.left_handed, left);
        }
        // Wild and grown.
        assert!(!equine::is_tamed(&made) && !animal::is_baby(&made));
    }
}

/// A wild horse at `x` with player 1 in its seat, of the temper `temper`.
fn ridden_wild_horse(temper: i32) -> Region {
    let mut region = on_ticking_floor_with(&[]);
    let mut horse = horse_at(
        ONE,
        HORSE,
        [3.5, -60.0, 3.5],
        &Animal::default(),
        &HorseState {
            temper,
            ..HorseState::default()
        },
    );
    horse.riding.passengers.push(Seat {
        key: EntityId(1),
        player: true,
        pending: None,
    });
    lay(&mut region, &horse);
    region
}

#[test]
fn a_wild_horse_of_full_temper_is_tamed_by_the_player_on_it_with_hearts() {
    let mut region = ridden_wild_horse(100);
    let mut events = Vec::new();
    for _ in 0..400 {
        let output = checked(&mut region, &TickInputs::default());
        events.extend(entity_events(&output, ONE));
        if horse_of(&region, ONE).tame {
            break;
        }
    }
    let state = horse_of(&region, ONE);
    assert!(state.tame, "tamed");
    assert_eq!(state.owner, Some(player(1).0.as_u128()));
    assert!(events.contains(&7));
    assert!(!events.contains(&6));
}

#[test]
fn a_wild_horse_of_no_temper_throws_the_player_on_it_rearing_and_grows_less_wild() {
    let mut region = ridden_wild_horse(0);
    let mut events = Vec::new();
    let mut heard = Vec::new();
    for _ in 0..400 {
        let output = checked(&mut region, &TickInputs::default());
        events.extend(entity_events(&output, ONE));
        heard.extend(sounds_of(&output));
        if events.contains(&6) {
            break;
        }
    }
    assert!(events.contains(&6), "thrown with smoke: {events:?}");
    let state = horse_of(&region, ONE);
    assert!(!state.tame);
    assert_eq!(state.temper, 5);
    assert!(state.standing);
    assert!(heard.contains(&sounds::ENTITY_HORSE_ANGRY));
    assert!(record(&region, ONE).unwrap().riding.passengers.is_empty());
}

#[test]
fn feeding_saddling_and_arming_a_tamed_horse_show_what_it_eats_and_wears() {
    let mut region = on_ticking_floor_with(&[]);
    let mut horse = horse_at(ONE, HORSE, [2.5, -60.0, 2.5], &Animal::default(), &tamed());
    horse.data = {
        let mut mob = Mob::decode(&horse.data).unwrap();
        mob.body.vitals.health = 30.0;
        mob.encode()
    };
    lay(&mut region, &horse);
    holding(&mut region, Some(one(items::APPLE)));
    let output = use_on(&mut region, ONE, false);
    assert!(sounds_of(&output).contains(&sounds::ENTITY_HORSE_EAT));
    assert_eq!(mob(&region, ONE).body.vitals.health, 33.0);
    holding(&mut region, Some(one(items::SADDLE)));
    let output = use_on(&mut region, ONE, false);
    assert!(sounds_of(&output).contains(&sounds::ENTITY_HORSE_SADDLE));
    assert!(output.events.iter().any(
        |event| matches!(event, RegionEvent::EquipmentChanged { entity, .. } if *entity == ONE)
    ));
    holding(&mut region, Some(one(items::DIAMOND_HORSE_ARMOR)));
    use_on(&mut region, ONE, false);
    let worn = mob(&region, ONE).body.equipment.slots;
    assert_eq!(worn[7], Some(one(items::SADDLE)));
    assert_eq!(worn[6], Some(one(items::DIAMOND_HORSE_ARMOR)));
    // What clients are shown of it: tamed, its mouth open from the apple.
    let shown = crate::shown::mob_values(HORSE as u16, &record(&region, ONE).unwrap().data);
    assert!(shown.contains(&(18, crate::shown::DataShown::Byte(2 | 64), false)));
}

#[test]
fn two_tamed_horses_in_love_breed_a_foal_and_a_horse_and_a_donkey_a_mule() {
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &horse_at(ONE, HORSE, [2.5, -60.0, 3.5], &in_love(), &tamed()),
    );
    lay(
        &mut region,
        &horse_at(TWO, HORSE, [5.0, -60.0, 3.5], &in_love(), &tamed()),
    );
    let mut foal = None;
    for _ in 0..400 {
        checked(&mut region, &TickInputs::default());
        foal = all_of(&region, HORSE)
            .into_iter()
            .find(|horse| horse.key != ONE && horse.key != TWO);
        if foal.is_some() {
            break;
        }
    }
    let foal = foal.expect("a foal");
    let foal = Mob::decode(&foal.data).unwrap();
    assert!(animal::is_baby(&foal) && !equine::is_tamed(&foal));
    // Its health is its own, inherited, and it is at it.
    let max = foal
        .body
        .attributes
        .value(HORSE as u16, crate::mobs::body::attribute(23)) as f32;
    assert!((15.0..=30.0).contains(&max));
    assert_eq!(foal.body.vitals.health, max);

    // A horse and a donkey: a mule, at the breeding parent.
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &horse_at(ONE, HORSE, [2.5, -60.0, 3.5], &in_love(), &tamed()),
    );
    lay(
        &mut region,
        &horse_at(TWO, DONKEY, [5.0, -60.0, 3.5], &in_love(), &tamed()),
    );
    let mut output = TickOutput::default();
    assert!(region.breed_with(TWO, ONE, &mut output));
    assert_eq!(all_of(&region, entity_types::MULE).len(), 1);
    // A wild one does not breed, nor do two mules.
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &horse_at(ONE, HORSE, [2.5, -60.0, 3.5], &in_love(), &tamed()),
    );
    lay(
        &mut region,
        &horse_at(
            TWO,
            HORSE,
            [5.0, -60.0, 3.5],
            &in_love(),
            &HorseState::default(),
        ),
    );
    assert!(!region.breed_with(TWO, ONE, &mut output));
    let mut region = on_ticking_floor_with(&[]);
    let mule = entity_types::MULE;
    lay(
        &mut region,
        &horse_at(ONE, mule, [2.5, -60.0, 3.5], &in_love(), &tamed()),
    );
    lay(
        &mut region,
        &horse_at(TWO, mule, [5.0, -60.0, 3.5], &in_love(), &tamed()),
    );
    assert!(!region.breed_with(TWO, ONE, &mut output));
}

#[test]
fn a_sneaking_owner_opens_a_donkey_s_menu_and_a_saddle_put_in_its_slot_goes_on() {
    let mut region = on_ticking_floor_with(&[]);
    let donkey = HorseState {
        chest: true,
        ..tamed()
    };
    lay(
        &mut region,
        &horse_at(ONE, DONKEY, [1.5, -60.0, 1.5], &Animal::default(), &donkey),
    );
    holding(&mut region, Some(one(items::SADDLE)));
    let output = use_on(&mut region, ONE, true);
    let opened = output
        .player_events
        .iter()
        .find_map(|(id, event)| match event {
            PlayerEvent::MenuOpened {
                serial,
                kind,
                slots,
                ..
            } if *id == player(1) => Some((*serial, *kind, slots.clone())),
            _ => None,
        });
    let (serial, kind, slots) = opened.expect("the donkey's menu opens");
    assert_eq!(
        kind,
        MenuKind::Mount {
            columns: 5,
            entity: ONE
        }
    );
    assert_eq!(slots.len(), 17 + 36);
    // The saddle from the hotbar's first slot into the saddle's slot.
    let click = |slot: i16| {
        moves(vec![numbered(
            1,
            PlayerInput::ContainerClick {
                serial,
                slot,
                button: 0,
                kind: ClickKind::Pickup,
            },
        )])
    };
    checked(&mut region, &click(17 + 27));
    let output = checked(&mut region, &click(0));
    assert_eq!(
        mob(&region, ONE).body.equipment.slots[7],
        Some(one(items::SADDLE))
    );
    // `ArmorSlot.setByPlayer` and `setItemSlot` each play it.
    let saddled: Vec<i32> = sounds_of(&output)
        .into_iter()
        .filter(|sound| *sound == sounds::ENTITY_HORSE_SADDLE)
        .collect();
    assert_eq!(saddled.len(), 2);
    // Dirt does not go in the body's slot; it does in the chest.
    holding(&mut region, Some(ItemStack::new(items::DIRT, 5)));
    checked(&mut region, &click(17 + 27));
    checked(&mut region, &click(1));
    assert_eq!(mob(&region, ONE).body.equipment.slots[6], None);
    checked(&mut region, &click(2));
    assert_eq!(
        horse_of(&region, ONE).items,
        [(0, ItemStack::new(items::DIRT, 5))]
    );
    // A wild donkey's menu does not open.
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &horse_at(
            ONE,
            DONKEY,
            [1.5, -60.0, 1.5],
            &Animal::default(),
            &HorseState::default(),
        ),
    );
    let output = use_on(&mut region, ONE, true);
    assert!(
        !output
            .player_events
            .iter()
            .any(|(_, event)| matches!(event, PlayerEvent::MenuOpened { .. }))
    );
}

fn leashed_to_player(mut entity: Entity) -> Entity {
    entity.riding.leash = Some(Box::new(Leash {
        holder: LeashHolder::Entity(EntityId(1)),
        angular_momentum: Double(0.0),
    }));
    entity
}

#[test]
fn a_llama_joins_a_caravan_behind_a_leashed_llama() {
    let mut region = on_ticking_floor_with(&[]);
    let head = leashed_to_player(horse_at(
        ONE,
        LLAMA,
        [3.5, -60.0, 0.5],
        &Animal::default(),
        &HorseState::default(),
    ));
    lay(&mut region, &head);
    lay(
        &mut region,
        &horse_at(
            TWO,
            LLAMA,
            [8.5, -60.0, 0.5],
            &Animal::default(),
            &HorseState::default(),
        ),
    );
    for _ in 0..10 {
        checked(&mut region, &TickInputs::default());
    }
    assert_eq!(horse_of(&region, TWO).caravan_head, Some(ONE));
    // The head itself joins nothing: it is leashed.
    assert_eq!(horse_of(&region, ONE).caravan_head, None);
}

#[test]
fn a_llama_spits_at_a_wolf_near_it() {
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &horse_at(
            ONE,
            LLAMA,
            [3.5, -60.0, 3.5],
            &Animal::default(),
            &HorseState::default(),
        ),
    );
    let mut wolf = Mob::fresh();
    wolf.body.vitals.health = 8.0;
    wolf.body.vitals.air = MAX_AIR;
    // Held still: a wolf built runs from a llama, which is not what this tries.
    wolf.body.flags.no_ai = true;
    lay(
        &mut region,
        &Entity {
            key: TWO,
            kind: entity_types::WOLF as u16,
            data: wolf.encode(),
            ..horse_at(
                TWO,
                LLAMA,
                [5.5, -60.0, 3.5],
                &Animal::default(),
                &HorseState::default(),
            )
        },
    );
    let mut spat = false;
    for _ in 0..400 {
        let output = checked(&mut region, &TickInputs::default());
        if sounds_of(&output).contains(&sounds::ENTITY_LLAMA_SPIT) {
            spat = true;
            break;
        }
    }
    assert!(spat, "it spat");
    assert_eq!(mob(&region, ONE).body.combat.target, Some(TWO));
}

#[test]
fn a_trader_llama_leaves_when_its_time_runs_out_and_a_tamed_one_stays() {
    let trader = entity_types::TRADER_LLAMA;
    let almost = HorseState {
        despawn_spent: crate::mobs::types::trader_llama::DESPAWN_DELAY - 3,
        ..HorseState::default()
    };
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &horse_at(ONE, trader, [3.5, -60.0, 3.5], &Animal::default(), &almost),
    );
    for _ in 0..3 {
        checked(&mut region, &TickInputs::default());
    }
    assert!(record(&region, ONE).is_none(), "gone");
    let mut region = on_ticking_floor_with(&[]);
    let kept = HorseState {
        tame: true,
        ..almost
    };
    lay(
        &mut region,
        &horse_at(ONE, trader, [3.5, -60.0, 3.5], &Animal::default(), &kept),
    );
    for _ in 0..10 {
        checked(&mut region, &TickInputs::default());
    }
    assert!(record(&region, ONE).is_some());
}

#[test]
fn a_tick_of_the_family_is_a_function_of_its_inputs() {
    let run = || {
        let grass: Vec<(BlockPos, clustine_data::BlockState)> = (0..16)
            .flat_map(|x| (0..16).map(move |z| (BlockPos::new(x, -61, z), blocks::GRASS_BLOCK)))
            .collect();
        let mut region = on_ticking_floor_with(&grass);
        lay(
            &mut region,
            &horse_at(ONE, HORSE, [2.5, -60.0, 3.5], &in_love(), &tamed()),
        );
        lay(
            &mut region,
            &horse_at(TWO, DONKEY, [4.0, -60.0, 3.5], &in_love(), &tamed()),
        );
        lay(
            &mut region,
            &leashed_to_player(horse_at(
                EntityId(44),
                LLAMA,
                [6.5, -60.0, 8.5],
                &Animal::default(),
                &HorseState::default(),
            )),
        );
        lay(
            &mut region,
            &horse_at(
                EntityId(46),
                LLAMA,
                [10.5, -60.0, 8.5],
                &Animal::default(),
                &HorseState::default(),
            ),
        );
        lay(
            &mut region,
            &horse_at(
                EntityId(48),
                entity_types::ZOMBIE_HORSE,
                [12.5, -60.0, 12.5],
                &Animal::default(),
                &HorseState::default(),
            ),
        );
        let mut changes = Vec::new();
        for _ in 0..300 {
            let output = checked(&mut region, &TickInputs::default());
            changes.push((output.entity_changes.clone(), output.effects.clone()));
        }
        changes
    };
    assert_eq!(run(), run());
}

#[test]
fn a_horse_grazes_on_grass_for_51_ticks_standing_still_and_the_grass_stays() {
    let grass: Vec<(BlockPos, clustine_data::BlockState)> = (0..16)
        .flat_map(|x| (0..16).map(move |z| (BlockPos::new(x, -61, z), blocks::GRASS_BLOCK)))
        .collect();
    let mut region = on_ticking_floor_with(&grass);
    lay(
        &mut region,
        &horse_at(
            ONE,
            HORSE,
            [8.5, -60.0, 8.5],
            &Animal::default(),
            &HorseState::default(),
        ),
    );
    let mut grazed = false;
    for _ in 0..3000 {
        checked(&mut region, &TickInputs::default());
        if horse_of(&region, ONE).eating {
            grazed = true;
            break;
        }
    }
    assert!(grazed, "it grazed");
    // Immobile while it eats (it slides to a stop, and goes nowhere of its own), which
    // ends by itself in the 51st tick of its count.
    let mut at = record(&region, ONE).unwrap().position;
    for tick in 0..49 {
        checked(&mut region, &TickInputs::default());
        assert!(horse_of(&region, ONE).eating);
        if tick == 19 {
            at = record(&region, ONE).unwrap().position;
        }
    }
    let still = record(&region, ONE).unwrap().position;
    assert!((still[0] - at[0]).abs() < 1e-3 && (still[2] - at[2]).abs() < 1e-3);
    checked(&mut region, &TickInputs::default());
    let state = horse_of(&region, ONE);
    assert!(!state.eating);
    assert_eq!(state.eaten, 0);
    assert_eq!(
        region.block(BlockPos::new(8, -61, 8)),
        Some(blocks::GRASS_BLOCK)
    );
}
