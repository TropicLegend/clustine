//! The highland animals in a region, as a player meets them (`docs/groundwork/
//! n7j-plan.md`, sections 1 and 4.3, parts B to E): an egg makes each with the draws of
//! its spawn in the game's order, a goat is milked and fed, an armadillo is brushed for a
//! scute, sheds one on its timer and rolls up when hit, a camel is fed, saddled, stands
//! up when hit and has the sizes and seats of its pose, and what clients are shown of
//! each; and their brains: a goat's long jump onto a ledge and its ram, never of a
//! creative player, each tempted by its food, bred and followed by its young, an
//! armadillo rolled up by a sprinting player and unrolled 81 ticks after, a camel sitting
//! and standing at random and panicking when hit; across a pinned line each tempted,
//! breeding by the regions' protocol, an armadillo scared, a goat's jump arriving whole;
//! a restore keeping a jump, a roll and a panic; and that a tick of them is a function
//! of its inputs.

use clustine_data::{entity_types, items, sounds};
use clustine_world::{Entity, EntityChange, EntityData, EntityId, TypeState};

use super::*;
use crate::living::MAX_AIR;
use crate::mobs::Mob;
use crate::mobs::brain::{Activity, MemoryKind, Value};
use crate::mobs::types::animal::{self, Animal, BABY_START_AGE};
use crate::mobs::types::armadillo::{self, ArmadilloState, Phase};
use crate::mobs::types::camel::{self, CamelState};
use crate::mobs::types::equine::{self, HorseState};
use crate::mobs::types::goat::{self, GoatState};
use crate::shown::{DataShown, Serialiser};

const ONE: EntityId = EntityId(40);
const TWO: EntityId = EntityId(42);

/// A mob of the type `kind` at `position`, standing, of its type's full health, with its
/// type state `state`.
fn mob_at(key: EntityId, kind: i32, position: [f64; 3], state: TypeState) -> Entity {
    let mut mob = Mob::fresh();
    let kind = kind as u16;
    mob.body.vitals.health = crate::mobs::body::attributes::supplied(kind, 23).unwrap() as f32;
    mob.body.vitals.air = MAX_AIR;
    mob.state = state;
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

fn goat_at(key: EntityId, x: f64, z: f64, shared: &Animal, own: &GoatState) -> Entity {
    mob_at(
        key,
        entity_types::GOAT,
        [x, -60.0, z],
        animal::write(shared, own),
    )
}

fn armadillo_at(key: EntityId, x: f64, z: f64, shared: &Animal, own: ArmadilloState) -> Entity {
    mob_at(
        key,
        entity_types::ARMADILLO,
        [x, -60.0, z],
        animal::write(shared, &own),
    )
}

fn camel_at(key: EntityId, kind: i32, x: f64, z: f64, shared: &Animal, own: CamelState) -> Entity {
    let mut entity = mob_at(
        key,
        kind,
        [x, -60.0, z],
        animal::write(shared, &HorseState::default()),
    );
    let mut mob = Mob::decode(&entity.data).unwrap();
    camel::update(&mut mob, |camel| *camel = own);
    entity.data = mob.encode();
    entity
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

fn record(region: &Region, key: EntityId) -> Entity {
    region
        .find_entity(key)
        .map(|(_, entity)| entity.clone())
        .expect("the mob is here")
}

fn mob(region: &Region, key: EntityId) -> Mob {
    Mob::decode(&record(region, key).data).unwrap()
}

fn mobs_of(region: &Region, kind: i32) -> Vec<Entity> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter(|entity| i32::from(entity.kind) == kind)
        .cloned()
        .collect()
}

fn items_lying(region: &Region) -> Vec<ItemStack> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter_map(|entity| match entity.data {
            EntityData::Item { ref stack, .. } => Some(stack.clone()),
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

fn attack(target: EntityId) -> TickInputs {
    moves(vec![numbered(1, PlayerInput::Attack { entity: target })])
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

fn heard(output: &TickOutput, sound: i32) -> usize {
    sounds_of(output)
        .iter()
        .filter(|(heard, _)| *heard == sound)
        .count()
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

fn idle_for(region: &mut Region, ticks: usize) -> Vec<TickOutput> {
    (0..ticks)
        .map(|_| checked(region, &TickInputs::default()))
        .collect()
}

fn game_time(region: &Region) -> i64 {
    i64::try_from(region.clock.game_time).unwrap()
}

fn baby() -> Animal {
    let mut shared = Animal::default();
    shared.set_age(BABY_START_AGE, 0);
    shared
}

#[test]
fn an_egg_makes_each_highland_animal_with_its_spawn_s_draws_in_the_game_s_order() {
    for kind in [
        entity_types::GOAT,
        entity_types::CAMEL,
        entity_types::CAMEL_HUSK,
        entity_types::ARMADILLO,
    ] {
        let mut region = on_ticking_floor_with(&[]);
        let player_key = region.players[&player(1)].entity_id;
        let mut level = region.entity_stream(player_key).random;
        let now = game_time(&region);
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
        match kind {
            entity_types::GOAT => {
                level.next_int(601);
                level.next_int(5401);
                let mut expected = GoatState {
                    screaming: level.next_double() < 0.02,
                    ..GoatState::default()
                };
                if f64::from(level.next_float()) < f64::from(0.1_f32) {
                    if level.next_bool() {
                        expected.left_horn = false;
                    } else {
                        expected.right_horn = false;
                    }
                }
                assert_eq!(goat::state_of(&made), expected);
            }
            entity_types::CAMEL | entity_types::CAMEL_HUSK => {
                // Standing as if long since, of no draw.
                assert_eq!(camel::camel_of(&made).pose_tick, (now - 53).max(0));
            }
            _ => {
                // Its constructor's scute timer, of its own stream.
                let own = armadillo::state_of(&made);
                assert!((6000..12_000).contains(&own.scute_at), "{own:?}");
                assert_eq!(own.phase, Phase::Idle);
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
        assert!(!animal::animal_of(&made.state).is_baby());
        // Of its type's size, standing.
        assert_eq!(crate::mobs::types::own_size(&record(&region, key)), None);
    }
    // And each from its egg used on the floor.
    for (egg, kind) in [
        (items::GOAT_SPAWN_EGG, entity_types::GOAT),
        (items::CAMEL_SPAWN_EGG, entity_types::CAMEL),
        (items::CAMEL_HUSK_SPAWN_EGG, entity_types::CAMEL_HUSK),
        (items::ARMADILLO_SPAWN_EGG, entity_types::ARMADILLO),
    ] {
        let mut region = on_ticking_floor_with(&[]);
        holding(&mut region, one(egg));
        checked(&mut region, &moves(vec![place(1, 3, -61, 3, Face::Top)]));
        assert_eq!(mobs_of(&region, kind).len(), 1, "{kind}");
    }
}

#[test]
fn a_goat_is_milked_at_every_click_its_own_client_hearing_its_own_and_a_kid_gives_nothing() {
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &goat_at(ONE, 2.5, 0.5, &Animal::default(), &GoatState::default()),
    );
    let screaming = GoatState {
        screaming: true,
        ..GoatState::default()
    };
    lay(
        &mut region,
        &goat_at(TWO, 2.5, 3.5, &Animal::default(), &screaming),
    );
    holding(&mut region, one(items::BUCKET));
    let output = use_on(&mut region, ONE, false);
    assert_eq!(
        sounds_of(&output)
            .into_iter()
            .filter(|(sound, _)| *sound == sounds::ENTITY_GOAT_MILK)
            .collect::<Vec<_>>(),
        [(sounds::ENTITY_GOAT_MILK, Some(player(1)))]
    );
    assert_eq!(held_count(&region, items::MILK_BUCKET), 1);
    // No cooldown: again at once, the player keeping the one milk bucket.
    let output = use_on(&mut region, ONE, false);
    assert_eq!(heard(&output, sounds::ENTITY_GOAT_MILK), 1);
    assert_eq!(held_count(&region, items::MILK_BUCKET), 1);
    holding(&mut region, one(items::BUCKET));
    let output = use_on(&mut region, TWO, false);
    assert_eq!(heard(&output, sounds::ENTITY_GOAT_SCREAMING_MILK), 1);
    // A kid gives none.
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &goat_at(ONE, 2.5, 0.5, &baby(), &GoatState::default()),
    );
    holding(&mut region, one(items::BUCKET));
    let output = use_on(&mut region, ONE, false);
    assert_eq!(heard(&output, sounds::ENTITY_GOAT_MILK), 0);
    assert_eq!(held_count(&region, items::MILK_BUCKET), 0);
}

#[test]
fn a_goat_fed_wheat_falls_in_love_and_sounds_its_eating_twice_as_the_game_does() {
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &goat_at(ONE, 2.5, 0.5, &Animal::default(), &GoatState::default()),
    );
    holding(&mut region, one(items::WHEAT));
    let output = use_on(&mut region, ONE, false);
    assert!(animal::animal_of(&mob(&region, ONE).state).in_love(record(&region, ONE).lived));
    assert_eq!(heard(&output, sounds::ENTITY_GOAT_EAT), 2);
    // In love already, it takes no more and makes no sound.
    let output = use_on(&mut region, ONE, false);
    assert_eq!(heard(&output, sounds::ENTITY_GOAT_EAT), 0);
}

#[test]
fn a_goat_falls_four_blocks_unhurt_where_a_cow_would_not_and_is_hurt_ten_less_from_higher() {
    for (height, hurt) in [(4.0, 0.0), (14.0, 1.0)] {
        let mut region = on_ticking_floor_with(&[]);
        let mut falling = goat_at(ONE, 4.5, 4.5, &Animal::default(), &GoatState::default());
        falling.position[1] = -60.0 + height;
        falling.motion = [0.0; 3];
        falling.on_ground = false;
        lay(&mut region, &falling);
        idle_for(&mut region, 60);
        // A fall of 14 hurts an animal by 11: a goat by 1.
        assert_eq!(
            mob(&region, ONE).body.vitals.health,
            10.0 - hurt,
            "{height}"
        );
    }
}

#[test]
fn a_brush_takes_a_scute_off_a_grown_armadillo_rolled_up_or_not_and_none_off_a_pup() {
    for phase in [Phase::Idle, Phase::Scared] {
        let mut region = on_ticking_floor_with(&[]);
        let own = ArmadilloState {
            phase,
            scute_at: 100_000,
            ..ArmadilloState::default()
        };
        lay(
            &mut region,
            &armadillo_at(ONE, 2.5, 0.5, &Animal::default(), own),
        );
        holding(&mut region, one(items::BRUSH));
        let output = use_on(&mut region, ONE, false);
        assert_eq!(
            items_lying(&region),
            [one(items::ARMADILLO_SCUTE)],
            "{phase:?}"
        );
        assert_eq!(heard(&output, sounds::ENTITY_ARMADILLO_BRUSH), 1);
        // Brushing does not wind its shedding back.
        assert_eq!(armadillo::state_of(&mob(&region, ONE)).scute_at, 100_000);
    }
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &armadillo_at(
            ONE,
            2.5,
            0.5,
            &baby(),
            ArmadilloState {
                scute_at: 100_000,
                ..ArmadilloState::default()
            },
        ),
    );
    holding(&mut region, one(items::BRUSH));
    let output = use_on(&mut region, ONE, false);
    assert!(items_lying(&region).is_empty());
    assert_eq!(heard(&output, sounds::ENTITY_ARMADILLO_BRUSH), 0);
}

#[test]
fn an_armadillo_sheds_a_scute_when_its_timer_runs_out_and_a_pup_s_waits_until_it_grows() {
    let mut region = on_ticking_floor_with(&[]);
    let due = ArmadilloState {
        scute_at: 3,
        ..ArmadilloState::default()
    };
    lay(
        &mut region,
        &armadillo_at(ONE, 2.5, 0.5, &Animal::default(), due),
    );
    lay(&mut region, &armadillo_at(TWO, 6.5, 6.5, &baby(), due));
    let outputs = idle_for(&mut region, 2);
    assert!(items_lying(&region).is_empty(), "not yet");
    assert!(
        outputs
            .iter()
            .all(|output| heard(output, sounds::ENTITY_ARMADILLO_SCUTE_DROP) == 0)
    );
    let output = checked(&mut region, &TickInputs::default());
    assert_eq!(items_lying(&region), [one(items::ARMADILLO_SCUTE)]);
    assert_eq!(heard(&output, sounds::ENTITY_ARMADILLO_SCUTE_DROP), 1);
    // Its next in 6000 to 11999 ticks from then.
    let next = armadillo::state_of(&mob(&region, ONE));
    let left = next.scute_time(record(&region, ONE).lived);
    assert!((6000..12_000).contains(&left), "{left}");
    // The pup's waits below nothing, its record as it was.
    let pup = armadillo::state_of(&mob(&region, TWO));
    assert_eq!(pup.scute_at, 3);
    assert!(pup.scute_time(record(&region, TWO).lived) <= 0);
    checked(&mut region, &TickInputs::default());
    assert!(armadillo::state_of(&mob(&region, TWO)).scute_time(record(&region, TWO).lived) < 0);
    assert_eq!(items_lying(&region).len(), 1);
}

#[test]
fn a_player_s_hit_rolls_an_armadillo_up_and_rolled_up_a_bare_hand_hurts_it_no_more() {
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &armadillo_at(
            ONE,
            1.5,
            0.5,
            &Animal::default(),
            ArmadilloState {
                scute_at: 100_000,
                ..ArmadilloState::default()
            },
        ),
    );
    let output = checked(&mut region, &attack(ONE));
    assert_eq!(
        armadillo::state_of(&mob(&region, ONE)).phase,
        Phase::Rolling
    );
    assert_eq!(heard(&output, sounds::ENTITY_ARMADILLO_ROLL), 1);
    assert_eq!(heard(&output, sounds::ENTITY_ARMADILLO_HURT_REDUCED), 1);
    assert_eq!(mob(&region, ONE).body.vitals.health, 11.0);
    // Shown rolling up.
    let shown =
        crate::shown::mob_values(entity_types::ARMADILLO as u16, &record(&region, ONE).data);
    assert!(shown.contains(&(
        18,
        DataShown::Serialised(Serialiser::ArmadilloState, 1),
        false
    )));
    idle_for(&mut region, 30);
    checked(&mut region, &attack(ONE));
    // A hit of one takes (1 − 1) / 2.
    assert_eq!(mob(&region, ONE).body.vitals.health, 11.0);
    // Fed while rolled up, it takes nothing.
    holding(&mut region, one(items::SPIDER_EYE));
    let output = use_on(&mut region, ONE, false);
    assert!(!animal::animal_of(&mob(&region, ONE).state).in_love(record(&region, ONE).lived));
    assert_eq!(heard(&output, sounds::ENTITY_ARMADILLO_EAT), 0);
}

#[test]
fn an_armadillo_fed_a_spider_eye_falls_in_love_with_its_eating_sound() {
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &armadillo_at(
            ONE,
            2.5,
            0.5,
            &Animal::default(),
            ArmadilloState {
                scute_at: 100_000,
                ..ArmadilloState::default()
            },
        ),
    );
    holding(&mut region, one(items::SPIDER_EYE));
    let output = use_on(&mut region, ONE, false);
    assert!(animal::animal_of(&mob(&region, ONE).state).in_love(record(&region, ONE).lived));
    assert_eq!(heard(&output, sounds::ENTITY_ARMADILLO_EAT), 1);
}

#[test]
fn a_camel_fed_cactus_heals_loves_or_grows_with_its_sound_and_its_husk_eats_rabbit_feet_alone() {
    let mut region = on_ticking_floor_with(&[]);
    let mut hurt = camel_at(
        ONE,
        entity_types::CAMEL,
        2.5,
        0.5,
        &Animal::default(),
        CamelState::default(),
    );
    let mut mob_one = Mob::decode(&hurt.data).unwrap();
    mob_one.body.vitals.health = 20.0;
    hurt.data = mob_one.encode();
    lay(&mut region, &hurt);
    lay(
        &mut region,
        &camel_at(
            TWO,
            entity_types::CAMEL,
            6.5,
            0.5,
            &baby(),
            CamelState::default(),
        ),
    );
    holding(&mut region, one(items::CACTUS));
    let output = use_on(&mut region, ONE, false);
    let fed = mob(&region, ONE);
    assert_eq!(fed.body.vitals.health, 22.0);
    // Grown and of no cooldown: in love too, hurt or not.
    assert!(animal::animal_of(&fed.state).in_love(record(&region, ONE).lived));
    assert_eq!(heard(&output, sounds::ENTITY_CAMEL_EAT), 1);
    let output = use_on(&mut region, TWO, false);
    let age = animal::animal_of(&mob(&region, TWO).state).age(record(&region, TWO).lived);
    assert!(age > BABY_START_AGE + 150, "{age}");
    assert_eq!(heard(&output, sounds::ENTITY_CAMEL_EAT), 1);
    // A husk: rabbit feet heal it, and it never loves; cactus is nothing to it.
    let mut region = on_ticking_floor_with(&[]);
    let mut husk = camel_at(
        ONE,
        entity_types::CAMEL_HUSK,
        2.5,
        0.5,
        &Animal::default(),
        CamelState::default(),
    );
    let mut husk_mob = Mob::decode(&husk.data).unwrap();
    husk_mob.body.vitals.health = 20.0;
    husk.data = husk_mob.encode();
    lay(&mut region, &husk);
    holding(&mut region, one(items::CACTUS));
    let output = use_on(&mut region, ONE, false);
    assert_eq!(heard(&output, sounds::ENTITY_CAMEL_HUSK_EAT), 0);
    // Any use makes it persistent.
    assert!(mob(&region, ONE).body.flags.persistent);
    holding(&mut region, one(items::RABBIT_FOOT));
    let output = use_on(&mut region, ONE, false);
    let fed = mob(&region, ONE);
    assert_eq!(fed.body.vitals.health, 22.0);
    assert!(!animal::animal_of(&fed.state).in_love(record(&region, ONE).lived));
    assert_eq!(heard(&output, sounds::ENTITY_CAMEL_HUSK_EAT), 1);
}

#[test]
fn a_saddle_goes_on_a_grown_camel_with_its_own_sound_and_a_click_without_one_takes_the_player_on() {
    for (kind, sound) in [
        (entity_types::CAMEL, sounds::ENTITY_CAMEL_SADDLE),
        (entity_types::CAMEL_HUSK, sounds::ENTITY_CAMEL_HUSK_SADDLE),
    ] {
        let mut region = on_ticking_floor_with(&[]);
        lay(
            &mut region,
            &camel_at(
                ONE,
                kind,
                2.5,
                0.5,
                &Animal::default(),
                CamelState::default(),
            ),
        );
        holding(&mut region, one(items::SADDLE));
        let output = use_on(&mut region, ONE, false);
        let saddled = mob(&region, ONE);
        assert!(equine::is_saddled(&saddled), "{kind}");
        assert_eq!(heard(&output, sound), 1);
        assert!(camel::riding::steered_by_rider(&saddled));
        // Anything else in hand: `doPlayerRide`, the player on.
        holding(&mut region, one(items::STICK));
        use_on(&mut region, ONE, false);
        assert_eq!(record(&region, ONE).riding.passengers.len(), 1);
    }
    // A calf takes no saddle.
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &camel_at(
            ONE,
            entity_types::CAMEL,
            2.5,
            0.5,
            &baby(),
            CamelState::default(),
        ),
    );
    holding(&mut region, one(items::SADDLE));
    use_on(&mut region, ONE, false);
    assert!(!equine::is_saddled(&mob(&region, ONE)));
}

#[test]
fn a_sitting_camel_is_low_and_seats_low_and_a_hit_stands_it_up_at_once() {
    let mut region = on_ticking_floor_with(&[]);
    region.clock.game_time = 10_000;
    let now = game_time(&region);
    let sat = CamelState {
        pose_tick: -(now - 200),
        ..CamelState::default()
    };
    lay(
        &mut region,
        &camel_at(ONE, entity_types::CAMEL, 1.5, 0.5, &Animal::default(), sat),
    );
    let entity = record(&region, ONE);
    assert_eq!(
        crate::mobs::types::own_size(&entity),
        Some(camel::ADULT_SITTING)
    );
    let shown = crate::shown::mob_values(entity_types::CAMEL as u16, &entity.data);
    assert!(shown.contains(&(6, DataShown::Pose(10), false)));
    assert!(shown.contains(&(20, DataShown::Long(sat.pose_tick), false)));
    let sitting = Mob::decode(&entity.data).unwrap();
    let height = camel::ADULT_SITTING.height;
    let seat = camel::riding::seat(&sitting, now, 0, 1, false, height, 1.0, 0.0, false);
    // Its seat sits on its sitting body: the box less the anchor's 0.375, and the low
    // part of the animation's 0.2.
    let low = 1.43_f32 - (1.43_f32 - 0.2_f32);
    assert_eq!(
        seat[1],
        f64::from((f64::from(height) - 0.375 + f64::from(low)) as f32)
    );
    // Hit, it stands up at once, as if it had stood up 53 ticks ago.
    let before = game_time(&region);
    checked(&mut region, &attack(ONE));
    let after = game_time(&region);
    let stood = mob(&region, ONE);
    assert!(!camel::is_sitting(&stood));
    let tick = camel::camel_of(&stood).pose_tick;
    assert!((before - 53..=after - 53).contains(&tick), "{tick}");
    assert_eq!(crate::mobs::types::own_size(&record(&region, ONE)), None);
}

#[test]
fn a_goat_shows_its_scream_and_horns_and_a_camel_its_dash_to_clients() {
    let mut region = on_ticking_floor_with(&[]);
    let one_horned = GoatState {
        screaming: true,
        left_horn: false,
        right_horn: true,
        ..GoatState::default()
    };
    lay(
        &mut region,
        &goat_at(ONE, 2.5, 0.5, &Animal::default(), &one_horned),
    );
    let shown = crate::shown::mob_values(entity_types::GOAT as u16, &record(&region, ONE).data);
    let own: Vec<_> = shown
        .iter()
        .filter(|(index, _, _)| *index >= 16)
        .cloned()
        .collect();
    assert_eq!(
        own,
        [
            (16, DataShown::Boolean(false), true),
            (17, DataShown::Boolean(false), true),
            (18, DataShown::Boolean(true), false),
            (19, DataShown::Boolean(false), false),
            (20, DataShown::Boolean(true), true),
        ]
    );
    lay(
        &mut region,
        &camel_at(
            TWO,
            entity_types::CAMEL,
            6.5,
            0.5,
            &Animal::default(),
            CamelState {
                dashing: true,
                ..CamelState::default()
            },
        ),
    );
    let shown = crate::shown::mob_values(entity_types::CAMEL as u16, &record(&region, TWO).data);
    // Sorted by index: the pose before the age's.
    let indices: Vec<u8> = shown.iter().map(|(index, _, _)| *index).collect();
    assert_eq!(indices, [1, 6, 7, 9, 15, 16, 17, 18, 19, 20]);
    assert!(shown.contains(&(19, DataShown::Boolean(true), false)));
}

// ------------------------------------------------------------------------------------
// Their brains, as a player meets them (`docs/groundwork/n7j-plan.md`, sections 1 and
// 4.3): each waits for a state, with a bound far beyond what the game's times give.

/// The mob `entity`'s brain with the int memories `set`, as a spawn's `initMemories`
/// leaves them.
fn remembering(entity: &mut Entity, set: &[(MemoryKind, i32)]) {
    let provider = crate::mobs::types::of(entity.kind)
        .brain()
        .expect("a brain");
    let mut mob = Mob::decode(&entity.data).unwrap();
    let mut memories = crate::mobs::brain::memories_of(provider, &mob, entity.lived);
    for (kind, value) in set {
        memories.set(*kind, Value::Int(*value));
    }
    crate::mobs::brain::write_memories(provider, &mut mob, memories);
    entity.data = mob.encode();
}

/// The memories of the region's mob `key` as its record has them now (its sensed ones
/// not among them).
fn memories(region: &Region, key: EntityId) -> crate::mobs::brain::Memories {
    let entity = record(region, key);
    let provider = crate::mobs::types::of(entity.kind)
        .brain()
        .expect("a brain");
    let mob = Mob::decode(&entity.data).unwrap();
    crate::mobs::brain::memories_of(provider, &mob, entity.lived)
}

/// The activity beside the core that the region's mob `key` has active.
fn active(region: &Region, key: EntityId) -> Option<Activity> {
    let entity = record(region, key);
    let provider = crate::mobs::types::of(entity.kind)
        .brain()
        .expect("a brain");
    let mob = Mob::decode(&entity.data).unwrap();
    crate::mobs::brain::Brain::read(
        provider,
        &mob.mind.brain,
        Some(&Default::default()),
        entity.lived,
    )
    .state
    .active
}

/// The volumes `sound` was heard at in `output`.
fn loud(output: &TickOutput, sound: i32) -> Vec<f32> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound: heard,
                volume,
                ..
            } if *heard == sound => Some(*volume),
            _ => None,
        })
        .collect()
}

/// The entity events `key` was shown with in `output`.
fn events_of(output: &TickOutput, key: EntityId) -> Vec<u8> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntityEvent { entity, event, .. } if *entity == key => Some(*event),
            _ => None,
        })
        .collect()
}

/// Ticks of the region with `inputs` until `done` holds of it, at most `bound`: the
/// outputs on the way, the last the one in which it came to hold; none where it never
/// did.
fn until_with(
    region: &mut Region,
    inputs: &TickInputs,
    bound: usize,
    mut done: impl FnMut(&Region) -> bool,
) -> Option<Vec<TickOutput>> {
    let mut outputs = Vec::new();
    for _ in 0..bound {
        outputs.push(checked(region, inputs));
        if done(region) {
            return Some(outputs);
        }
    }
    None
}

/// The same with no inputs.
fn until(
    region: &mut Region,
    bound: usize,
    done: impl FnMut(&Region) -> bool,
) -> Option<Vec<TickOutput>> {
    until_with(region, &TickInputs::default(), bound, done)
}

/// A ledge of stone two blocks above the floor, east of where the goats stand, which no
/// path of 8 reaches: what a goat long-jumps onto rather than walks to.
fn ledge() -> Vec<(BlockPos, clustine_data::BlockState)> {
    (11..=13)
        .flat_map(|x| {
            (6..=10).flat_map(move |z| {
                [-60, -59].map(|y| (BlockPos::new(x, y, z), clustine_data::blocks::STONE))
            })
        })
        .collect()
}

#[test]
fn a_goat_whose_jump_has_cooled_down_leaps_onto_a_ledge_in_its_pose_and_lands_loud() {
    let mut region = on_ticking_floor_with(&ledge());
    let mut goat = goat_at(ONE, 8.5, 8.5, &Animal::default(), &GoatState::default());
    // Its ram's cooldown far off, its jump's out: it is in its long jump at once.
    remembering(&mut goat, &[(MemoryKind::RamCooldownTicks, 100_000)]);
    lay(&mut region, &goat);
    checked(&mut region, &TickInputs::default());
    assert_eq!(active(&region, ONE), Some(Activity::LongJump));
    // It takes off with its jump's sound, and is in the jump's pose from the next tick,
    // smaller by three tenths and shown so.
    let outputs = until(&mut region, 400, |region| {
        goat::state_of(&mob(region, ONE)).long_jumping
    })
    .expect("it jumps");
    let took_off = outputs.len() - 2;
    assert_eq!(heard(&outputs[took_off], sounds::ENTITY_GOAT_LONG_JUMP), 1);
    let flying = record(&region, ONE);
    assert!(!flying.on_ground);
    assert_eq!(
        crate::mobs::types::own_size(&flying).map(|size| size.height),
        Some(1.3 * 0.7)
    );
    let shown = crate::shown::mob_values(entity_types::GOAT as u16, &flying.data);
    assert!(shown.contains(&(6, DataShown::Pose(6), false)));
    // It lands on the ledge, the step's sound at twice the volume, standing again, with
    // a cooldown of 600 to 1200 drawn.
    let outputs = until(&mut region, 200, |region| {
        !goat::state_of(&mob(region, ONE)).long_jumping
    })
    .expect("it lands");
    let landed = outputs.last().unwrap();
    assert_eq!(loud(landed, sounds::ENTITY_GOAT_STEP), [2.0]);
    let on = record(&region, ONE);
    assert_eq!(on.position[1], -58.0, "{:?}", on.position);
    assert!(on.position[0] > 10.0, "{:?}", on.position);
    let cooldown = memories(&region, ONE)
        .int(MemoryKind::LongJumpCoolingDown)
        .expect("cooling down");
    assert!((590..=1200).contains(&cooldown), "{cooldown}");
    assert_eq!(crate::mobs::types::own_size(&on), None);
}

/// A pen of stone two blocks high about the blocks from x and z 3 to 13, inside the one
/// chunk that ticks its entities, so that a stroll takes no mob where it would stand
/// still; the player stands outside it.
fn pen() -> Vec<(BlockPos, clustine_data::BlockState)> {
    let mut walls = Vec::new();
    for a in 2..=14 {
        for y in [-60, -59] {
            for (x, z) in [(a, 2), (a, 14), (2, a), (14, a)] {
                walls.push((BlockPos::new(x, y, z), clustine_data::blocks::STONE));
            }
        }
    }
    walls
}

/// A cow of no AI at `position`, standing where it is laid.
fn still_cow(key: EntityId, position: [f64; 3]) -> Entity {
    let mut cow = mob_at(key, entity_types::COW, position, TypeState::default());
    let mut mob = Mob::decode(&cow.data).unwrap();
    mob.body.flags.no_ai = true;
    cow.data = mob.encode();
    cow
}

#[test]
fn a_goat_whose_ram_has_cooled_down_lowers_its_head_charges_a_mob_and_knocks_it_back() {
    let mut region = on_ticking_floor_with(&[]);
    let mut goat = goat_at(ONE, 4.5, 8.5, &Animal::default(), &GoatState::default());
    remembering(&mut goat, &[(MemoryKind::LongJumpCoolingDown, 100_000)]);
    lay(&mut region, &goat);
    lay(&mut region, &still_cow(TWO, [9.5, -60.0, 8.5]));
    let full = mob(&region, TWO).body.vitals.health;
    let mut events = Vec::new();
    let outputs = until(&mut region, 600, |region| {
        mob(region, TWO).body.vitals.health < full
    })
    .expect("it rams the cow");
    for output in &outputs {
        events.extend(events_of(output, ONE));
    }
    // Its head lowered at its start, raised as the ram ends; the impact heard.
    assert!(events.contains(&58), "{events:?}");
    assert_eq!(events.last(), Some(&59), "{events:?}");
    let last = outputs.last().unwrap();
    assert_eq!(heard(last, sounds::ENTITY_GOAT_RAM_IMPACT), 1);
    // An adult's damage, 2, and a push away from the goat.
    assert_eq!(mob(&region, TWO).body.vitals.health, full - 2.0);
    assert!(record(&region, TWO).motion[0] > 0.0);
    // Its next ram 600 to 6000 ticks off.
    let cooldown = memories(&region, ONE)
        .int(MemoryKind::RamCooldownTicks)
        .expect("cooling down");
    assert!((600..=6000).contains(&cooldown), "{cooldown}");
}

#[test]
fn a_goat_never_rams_a_creative_player_and_waits_the_least_of_its_cooldown_instead() {
    for (screaming, least) in [(false, 600), (true, 100)] {
        let mut region = on_ticking_floor_with(&[]);
        let own = GoatState {
            screaming,
            ..GoatState::default()
        };
        let mut goat = goat_at(ONE, 5.5, 0.5, &Animal::default(), &own);
        remembering(&mut goat, &[(MemoryKind::LongJumpCoolingDown, 100_000)]);
        lay(&mut region, &goat);
        let outputs = until(&mut region, 40, |region| {
            memories(region, ONE)
                .int(MemoryKind::RamCooldownTicks)
                .is_some()
        })
        .expect("it gives up");
        assert_eq!(
            memories(&region, ONE).int(MemoryKind::RamCooldownTicks),
            Some(least)
        );
        // Its head was never lowered.
        assert!(
            outputs
                .iter()
                .all(|output| !events_of(output, ONE).contains(&58))
        );
    }
}

#[test]
fn a_goat_follows_a_player_holding_wheat_and_a_camel_cactus_and_an_armadillo_spider_eyes() {
    for (kind, food) in [
        (entity_types::GOAT, items::WHEAT),
        (entity_types::CAMEL, items::CACTUS),
        (entity_types::CAMEL_HUSK, items::RABBIT_FOOT),
        (entity_types::ARMADILLO, items::SPIDER_EYE),
    ] {
        let mut region = on_ticking_floor_with(&[]);
        holding(&mut region, one(food));
        let mut animal = match kind {
            entity_types::GOAT => goat_at(ONE, 9.5, 0.5, &Animal::default(), &GoatState::default()),
            entity_types::ARMADILLO => armadillo_at(
                ONE,
                9.5,
                0.5,
                &Animal::default(),
                ArmadilloState {
                    scute_at: 100_000,
                    ..ArmadilloState::default()
                },
            ),
            _ => camel_at(
                ONE,
                kind,
                9.5,
                0.5,
                &Animal::default(),
                CamelState::default(),
            ),
        };
        if kind == entity_types::GOAT {
            remembering(
                &mut animal,
                &[
                    (MemoryKind::LongJumpCoolingDown, 100_000),
                    (MemoryKind::RamCooldownTicks, 100_000),
                ],
            );
        }
        lay(&mut region, &animal);
        until(&mut region, 600, |region| {
            let at = record(region, ONE).position;
            (at[0] - 0.5).abs() < 4.0 && (at[2] - 0.5).abs() < 4.0
        })
        .unwrap_or_else(|| panic!("{kind}: tempted to the player"));
    }
}

#[test]
fn a_sprinting_player_rolls_an_armadillo_up_and_it_unrolls_81_ticks_after_it_last_saw_them() {
    let mut region = on_ticking_floor_with(&[]);
    let own = ArmadilloState {
        scute_at: 100_000,
        ..ArmadilloState::default()
    };
    lay(
        &mut region,
        &armadillo_at(ONE, 4.5, 0.5, &Animal::default(), own),
    );
    let keys = |sprint| {
        moves(vec![numbered(
            1,
            PlayerInput::Keys(crate::api::Keys {
                sprint,
                ..Default::default()
            }),
        )])
    };
    // A walking player scares it not.
    idle_for(&mut region, 40);
    assert_eq!(armadillo::state_of(&mob(&region, ONE)).phase, Phase::Idle);
    // Sprinting: danger is remembered at a scan, it panics, and rolls up with its sound.
    let outputs = until_with(&mut region, &keys(true), 40, |region| {
        armadillo::state_of(&mob(region, ONE)).phase == Phase::Rolling
    })
    .expect("rolled up");
    assert_eq!(
        heard(outputs.last().unwrap(), sounds::ENTITY_ARMADILLO_ROLL),
        1
    );
    assert_eq!(active(&region, ONE), Some(Activity::Panic));
    let rolled = record(&region, ONE).lived;
    // Eleven ticks rolling, then scared with the landing's sound.
    let outputs = until(&mut region, 40, |region| {
        armadillo::state_of(&mob(region, ONE)).phase == Phase::Scared
    })
    .expect("scared");
    assert_eq!(record(&region, ONE).lived - rolled, 11);
    assert_eq!(
        heard(outputs.last().unwrap(), sounds::ENTITY_ARMADILLO_LAND),
        1
    );
    // The player walks on: the last scan that saw them sprint set the memory's end.
    checked(&mut region, &keys(false));
    checked(&mut region, &TickInputs::default());
    let crate::mobs::brain::Slot::Kept {
        expires: Some(end), ..
    } = memories(&region, ONE).slots[&MemoryKind::DangerDetectedRecently].clone()
    else {
        panic!("danger remembered with an end");
    };
    // It begins to unroll when less than 30 ticks are left, with its sound.
    let outputs = until(&mut region, 100, |region| {
        armadillo::state_of(&mob(region, ONE)).phase == Phase::Unrolling
    })
    .expect("unrolling");
    assert_eq!(record(&region, ONE).lived - 1, end - 29);
    assert_eq!(
        heard(
            outputs.last().unwrap(),
            sounds::ENTITY_ARMADILLO_UNROLL_START
        ),
        1
    );
    // And is idle in the tick after the memory's last, 81 after the scan.
    let outputs = until(&mut region, 100, |region| {
        armadillo::state_of(&mob(region, ONE)).phase == Phase::Idle
    })
    .expect("unrolled");
    assert_eq!(record(&region, ONE).lived - 1, end + 1);
    assert_eq!(
        heard(
            outputs.last().unwrap(),
            sounds::ENTITY_ARMADILLO_UNROLL_FINISH
        ),
        1
    );
    assert!(
        memories(&region, ONE)
            .get(MemoryKind::DangerDetectedRecently)
            .is_none()
    );
    checked(&mut region, &TickInputs::default());
    assert_eq!(active(&region, ONE), Some(Activity::Idle));
}

#[test]
fn a_camel_sits_down_after_400_ticks_standing_and_stands_up_400_ticks_after() {
    let mut region = on_ticking_floor_with(&pen());
    region.clock.game_time = 10_000;
    lay(
        &mut region,
        &camel_at(
            ONE,
            entity_types::CAMEL,
            8.5,
            8.5,
            &Animal::default(),
            CamelState::default(),
        ),
    );
    let outputs = until(&mut region, 4000, |region| {
        camel::is_sitting(&mob(region, ONE))
    })
    .expect("it sits");
    assert_eq!(heard(outputs.last().unwrap(), sounds::ENTITY_CAMEL_SIT), 1);
    // Stamped with the game time of its tick, negated.
    let sat = -camel::camel_of(&mob(&region, ONE)).pose_tick;
    assert!((0..=1).contains(&(game_time(&region) - sat)), "{sat}");
    let entity = record(&region, ONE);
    let shown = crate::shown::mob_values(entity_types::CAMEL as u16, &entity.data);
    assert!(shown.contains(&(6, DataShown::Pose(10), false)));
    // Sitting, it strolls nowhere.
    let still = record(&region, ONE).position;
    let outputs = until(&mut region, 4000, |region| {
        !camel::is_sitting(&mob(region, ONE))
    })
    .expect("it stands up");
    assert_eq!(
        heard(outputs.last().unwrap(), sounds::ENTITY_CAMEL_STAND),
        1
    );
    let stood = camel::camel_of(&mob(&region, ONE)).pose_tick;
    assert!(stood - sat >= 400, "{}", stood - sat);
    let moved = record(&region, ONE).position;
    assert_eq!((moved[0], moved[2]), (still[0], still[2]));
}

#[test]
fn a_sitting_camel_hit_stands_up_at_once_and_panics_at_four_times_its_speed() {
    let mut region = on_ticking_floor_with(&pen());
    region.clock.game_time = 10_000;
    let now = game_time(&region);
    let sat = CamelState {
        pose_tick: -(now - 100),
        ..CamelState::default()
    };
    lay(
        &mut region,
        &camel_at(ONE, entity_types::CAMEL, 4.5, 4.5, &Animal::default(), sat),
    );
    checked(&mut region, &attack(ONE));
    assert!(!camel::is_sitting(&mob(&region, ONE)));
    until(&mut region, 60, |region| {
        memories(region, ONE).get(MemoryKind::IsPanicking).is_some()
            && mob(region, ONE).body.progress.speed == 4.0
    })
    .expect("it panics");
}

// ------------------------------------------------------------------------------------
// Across a pinned line (`docs/groundwork/n7j-plan.md`, sections 1, 3 and 4.3).

/// A region of the land west of chunk x = 2, its chunks x of -1 to 1 loaded with a
/// floor, and the chunks of x = 2 another's, copied, with `beyond` in the one at z = 0.
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
    let chunks = around.iter().map(|position| (*position, floor())).collect();
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

/// Player 9 of the region beyond (entity 900), standing at `x` on the line's far side,
/// holding `held`, sprinting or not, as their face comes.
fn face_beyond(x: f64, held: Option<i32>, sprinting: bool) -> TickInputs {
    let mut face = crate::api::PlayerFace::of(
        &Pose::at(Vec3::new(x, -60.0, 0.5)),
        false,
        false,
        &Default::default(),
        0,
        None,
    );
    face.main_hand = held.map(one);
    face.sprinting = sprinting;
    TickInputs {
        faces: vec![crate::api::FaceChange::Face {
            player: player(9),
            entity: EntityId(900),
            hops: 0,
            seq: crate::api::FaceSeq(1, 0),
            face,
        }],
        ..TickInputs::default()
    }
}

/// An adult in love.
fn in_love() -> Animal {
    let mut shared = Animal::default();
    shared.set_in_love(0);
    shared
}

/// A goat idling: both its cooldowns far off.
fn idle_goat(key: EntityId, x: f64, z: f64, shared: &Animal) -> Entity {
    let mut goat = goat_at(key, x, z, shared, &GoatState::default());
    remembering(
        &mut goat,
        &[
            (MemoryKind::LongJumpCoolingDown, 100_000),
            (MemoryKind::RamCooldownTicks, 100_000),
        ],
    );
    goat
}

/// A highland animal of the type `kind` with `shared` as its animal's state, its brain
/// idling: a goat's two cooldowns far off, an armadillo's scute.
fn idling(kind: i32, key: EntityId, x: f64, z: f64, shared: &Animal) -> Entity {
    match kind {
        entity_types::GOAT => idle_goat(key, x, z, shared),
        entity_types::ARMADILLO => armadillo_at(
            key,
            x,
            z,
            shared,
            ArmadilloState {
                scute_at: 100_000,
                ..ArmadilloState::default()
            },
        ),
        _ => camel_at(key, kind, x, z, shared, CamelState::default()),
    }
}

#[test]
fn each_highland_animal_is_tempted_by_a_player_beyond_a_pinned_line_and_walks_to_them() {
    for (kind, food) in [
        (entity_types::GOAT, items::WHEAT),
        (entity_types::CAMEL, items::CACTUS),
        (entity_types::CAMEL_HUSK, items::RABBIT_FOOT),
        (entity_types::ARMADILLO, items::SPIDER_EYE),
    ] {
        let mut region = at_a_line(&[idling(kind, ONE, 26.5, 0.5, &Animal::default())], &[]);
        let walks_to_them = |region: &Region| {
            matches!(
                memories(region, ONE).get(MemoryKind::WalkTarget),
                Some(Value::Walk {
                    to: crate::mobs::brain::Tracker::Entity { key, .. },
                    ..
                }) if key == EntityId(900)
            )
        };
        checked(&mut region, &face_beyond(35.5, Some(food), false));
        until(&mut region, 60, walks_to_them)
            .unwrap_or_else(|| panic!("{kind}: tempted across the line"));
        // It comes up to the line, which it does not pass where nothing is loaded
        // beyond.
        until(&mut region, 600, |region| {
            record(region, ONE).position[0] > 28.0
        })
        .unwrap_or_else(|| panic!("{kind}: it walks to the line"));
    }
}

/// The requests to breed the region sent in `output`.
fn asks(output: &TickOutput) -> Vec<(EntityId, clustine_world::EntityUse)> {
    output
        .effects
        .iter()
        .filter_map(|effect| match effect.what {
            clustine_world::Effected::OnEntity { key, ref by } => Some((key, by.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn highland_animals_in_love_across_a_pinned_line_breed_by_the_protocol_the_higher_key_asking() {
    for kind in [
        entity_types::GOAT,
        entity_types::CAMEL,
        entity_types::ARMADILLO,
    ] {
        // The partner beyond has the lower key: the region's animal asks.
        let partner = idling(kind, EntityId(5), 32.5, 0.5, &in_love());
        let mut region = at_a_line(&[idling(kind, ONE, 30.5, 0.5, &in_love())], &[partner]);
        let mut asked = None;
        for _ in 0..200 {
            let output = checked(&mut region, &TickInputs::default());
            if let Some(found) = asks(&output).first().cloned() {
                asked = Some(found);
                break;
            }
        }
        let (to, by) = asked.unwrap_or_else(|| panic!("{kind}: asked"));
        assert_eq!(to, EntityId(5));
        assert_eq!(
            by,
            clustine_world::EntityUse::BreedAsk {
                asker: ONE,
                number: 1
            }
        );
        // The partner beyond has the higher key: the region's animal waits to be asked.
        let partner = idling(kind, EntityId(1 << 34), 32.5, 0.5, &in_love());
        let mut region = at_a_line(&[idling(kind, ONE, 30.5, 0.5, &in_love())], &[partner]);
        for _ in 0..200 {
            let output = checked(&mut region, &TickInputs::default());
            assert!(asks(&output).is_empty(), "{kind}");
        }
    }
    // Camel husks never: neither loves, nor is a camel's partner.
    let partner = idling(entity_types::CAMEL_HUSK, EntityId(5), 33.5, 0.5, &in_love());
    let husk = idling(entity_types::CAMEL_HUSK, ONE, 30.5, 0.5, &in_love());
    let mut region = at_a_line(&[husk], &[partner]);
    for _ in 0..200 {
        let output = checked(&mut region, &TickInputs::default());
        assert!(asks(&output).is_empty());
    }
}

#[test]
fn highland_animals_in_love_breed_a_baby_of_their_kind_and_a_baby_follows_an_adult() {
    for kind in [
        entity_types::GOAT,
        entity_types::CAMEL,
        entity_types::ARMADILLO,
    ] {
        let mut region = on_ticking_floor_with(&pen());
        region.clock.game_time = 10_000;
        lay(&mut region, &idling(kind, ONE, 6.5, 8.5, &in_love()));
        lay(&mut region, &idling(kind, TWO, 9.5, 8.5, &in_love()));
        until(&mut region, 400, |region| mobs_of(region, kind).len() == 3)
            .unwrap_or_else(|| panic!("{kind}: a baby bred"));
        let baby = mobs_of(&region, kind)
            .into_iter()
            .find(|entity| entity.key != ONE && entity.key != TWO)
            .unwrap();
        let made = Mob::decode(&baby.data).unwrap();
        assert!(animal::animal_of(&made.state).is_baby(), "{kind}");
        if kind == entity_types::GOAT {
            // A bred kid has its two cooldowns.
            let memories = memories(&region, baby.key);
            assert!(memories.int(MemoryKind::RamCooldownTicks).is_some());
            assert!(memories.int(MemoryKind::LongJumpCoolingDown).is_some());
        }
    }
    // A baby of each 5 to 16 blocks from an adult walks to it.
    for kind in [
        entity_types::GOAT,
        entity_types::CAMEL,
        entity_types::ARMADILLO,
    ] {
        let mut region = on_ticking_floor_with(&pen());
        region.clock.game_time = 10_000;
        lay(&mut region, &idling(kind, ONE, 4.5, 4.5, &baby()));
        let mut adult = idling(kind, TWO, 12.5, 12.5, &Animal::default());
        let mut still = Mob::decode(&adult.data).unwrap();
        still.body.flags.no_ai = true;
        adult.data = still.encode();
        lay(&mut region, &adult);
        until(&mut region, 600, |region| {
            matches!(
                memories(region, ONE).get(MemoryKind::WalkTarget),
                Some(Value::Walk {
                    to: crate::mobs::brain::Tracker::Entity { key, .. },
                    ..
                }) if key == TWO
            )
        })
        .unwrap_or_else(|| panic!("{kind}: follows the adult"));
    }
}

#[test]
fn a_player_sprinting_beyond_a_pinned_line_rolls_an_armadillo_up() {
    let own = ArmadilloState {
        scute_at: 100_000,
        ..ArmadilloState::default()
    };
    let mut region = at_a_line(
        &[armadillo_at(ONE, 29.5, 0.5, &Animal::default(), own)],
        &[],
    );
    // Walking there, they scare it not.
    checked(&mut region, &face_beyond(33.5, None, false));
    idle_for(&mut region, 30);
    assert_eq!(armadillo::state_of(&mob(&region, ONE)).phase, Phase::Idle);
    let mut sprinting = face_beyond(33.5, None, true);
    if let crate::api::FaceChange::Face { seq, .. } = &mut sprinting.faces[0] {
        *seq = crate::api::FaceSeq(2, 0);
    }
    checked(&mut region, &sprinting);
    until(&mut region, 40, |region| {
        armadillo::state_of(&mob(region, ONE)).phase == Phase::Rolling
    })
    .expect("rolled up by the sprint beyond");
}

/// A goat in the air in the middle of a long jump at `position`, moving at `motion`,
/// in its pose with its friction discarded, its brain remembering the jump.
fn goat_mid_jump(key: EntityId, position: [f64; 3], motion: [f64; 3]) -> Entity {
    let own = GoatState {
        long_jumping: true,
        discards_friction: true,
        ..GoatState::default()
    };
    let mut goat = goat_at(key, position[0], position[2], &Animal::default(), &own);
    goat.position = position;
    goat.motion = motion;
    goat.on_ground = false;
    remembering(&mut goat, &[(MemoryKind::RamCooldownTicks, 100_000)]);
    remembered(&mut goat, |memories| {
        memories.set(MemoryKind::LongJumpMidJump, Value::Bool(true));
    });
    goat
}

/// The mob `entity`'s brain with what `write` does to its memories.
fn remembered(entity: &mut Entity, write: impl FnOnce(&mut crate::mobs::brain::Memories)) {
    let provider = crate::mobs::types::of(entity.kind)
        .brain()
        .expect("a brain");
    let mut mob = Mob::decode(&entity.data).unwrap();
    let mut memories = crate::mobs::brain::memories_of(provider, &mob, entity.lived);
    write(&mut memories);
    crate::mobs::brain::write_memories(provider, &mut mob, memories);
    entity.data = mob.encode();
}

#[test]
fn a_goat_that_long_jumps_over_a_pinned_line_arrives_in_mid_air_with_its_brain_and_pose() {
    let mut region = at_a_line(
        &[goat_mid_jump(ONE, [29.5, -57.0, 0.5], [0.4, 0.3, 0.0])],
        &[],
    );
    let mut sent = None;
    for _ in 0..20 {
        let output = checked(&mut region, &TickInputs::default());
        if let Some((_, entity)) = output.arrivals.iter().find(|(_, entity)| entity.key == ONE) {
            sent = Some(entity.clone());
            break;
        }
    }
    let sent = sent.expect("sent over the line");
    assert!(!sent.on_ground);
    assert!(sent.position[0] >= 32.0);
    // Unslowed by the air's drag while it flew.
    assert_eq!(sent.motion[0], 0.4);
    let flying = Mob::decode(&sent.data).unwrap();
    assert!(goat::state_of(&flying).long_jumping);
    let provider = crate::mobs::types::of(sent.kind).brain().unwrap();
    let brain = crate::mobs::brain::Brain::read(
        provider,
        &flying.mind.brain,
        Some(&Default::default()),
        sent.lived,
    );
    assert_eq!(brain.state.active, Some(Activity::LongJump));
    assert!(brain.memories.check(
        MemoryKind::LongJumpMidJump,
        crate::mobs::brain::Status::Present
    ));
    // The mid-jump behaviour runs in the record it carries.
    assert!(
        flying
            .mind
            .brain
            .iter()
            .any(|entry| entry.id >= crate::mobs::brain::ids::FIRST_BEHAVIOUR)
    );
    // Taken in another region, it flies on and lands as it would have here.
    let mut beyond = on_ticking_floor_with(&[]);
    let mut arriving = sent.clone();
    arriving.position[0] -= 24.0;
    let output = checked(
        &mut beyond,
        &TickInputs {
            arrivals: vec![clustine_world::Arrival {
                id: clustine_world::EffectId {
                    sequence: 7,
                    index: 0,
                },
                chunk: arriving.chunk(),
                entity: arriving,
                due: clustine_world::Due::FromTaking,
            }],
            ..TickInputs::default()
        },
    );
    assert_eq!(output.arrivals_taken.len(), 1);
    let outputs = until(&mut beyond, 100, |region| {
        !goat::state_of(&mob(region, ONE)).long_jumping
    })
    .expect("it lands");
    assert_eq!(
        loud(outputs.last().unwrap(), sounds::ENTITY_GOAT_STEP),
        [2.0]
    );
    let cooldown = memories(&beyond, ONE)
        .int(MemoryKind::LongJumpCoolingDown)
        .expect("cooling down");
    assert!((600..=1200).contains(&cooldown), "{cooldown}");
}

#[test]
fn a_restore_keeps_a_long_jump_a_rolled_up_armadillo_and_a_camels_panic_tick_for_tick() {
    let mut original = on_ticking_floor_with(&pen());
    original.clock.game_time = 10_000;
    lay(
        &mut original,
        &goat_mid_jump(ONE, [5.5, -57.0, 5.5], [0.2, 0.3, 0.1]),
    );
    // An armadillo just rolled up by a danger it remembers for 80 ticks, a camel that
    // panics.
    let mut armadillo = armadillo_at(
        TWO,
        10.5,
        10.5,
        &Animal::default(),
        ArmadilloState {
            phase: Phase::Rolling,
            scute_at: 100_000,
            ..ArmadilloState::default()
        },
    );
    remembered(&mut armadillo, |memories| {
        memories.set_with_expiry(MemoryKind::DangerDetectedRecently, Value::Bool(true), 80);
    });
    lay(&mut original, &armadillo);
    let camel_key = EntityId(44);
    let mut camel = camel_at(
        camel_key,
        entity_types::CAMEL,
        10.5,
        5.5,
        &Animal::default(),
        CamelState::default(),
    );
    remembered(&mut camel, |memories| {
        memories.set(MemoryKind::IsPanicking, Value::Bool(true));
    });
    lay(&mut original, &camel);
    for restore_after in [1, 5, 12, 30] {
        let mut region = original.clone();
        for _ in 0..restore_after {
            checked(&mut region, &TickInputs::default());
        }
        let mut copy = super::scheduled_ticks::restored(&region);
        for tick in restore_after..120 {
            let (a, b) = (
                checked(&mut region, &TickInputs::default()),
                checked(&mut copy, &TickInputs::default()),
            );
            let case = format!("restored after {restore_after}, tick {tick}");
            assert_eq!(a.entity_changes, b.entity_changes, "{case}");
            for key in [ONE, TWO, camel_key] {
                assert_eq!(record(&region, key), record(&copy, key), "{case}");
            }
        }
        // What was kept went on: the goat landed, the armadillo unrolled once its danger
        // was gone, the camel ran.
        assert!(!goat::state_of(&mob(&copy, ONE)).long_jumping);
        assert_eq!(armadillo::state_of(&mob(&copy, TWO)).phase, Phase::Idle);
        let ran = record(&copy, camel_key).position;
        assert!(
            (ran[0] - 10.5).abs() > 1.0 || (ran[2] - 5.5).abs() > 1.0,
            "{ran:?}"
        );
    }
}

#[test]
fn a_tick_of_the_highland_animals_with_their_brains_is_a_function_of_its_inputs() {
    let run = || {
        let mut region = on_ticking_floor_with(&pen());
        region.clock.game_time = 10_000;
        holding(&mut region, one(items::WHEAT));
        lay(
            &mut region,
            &goat_at(ONE, 5.5, 5.5, &in_love(), &GoatState::default()),
        );
        lay(&mut region, &idle_goat(TWO, 7.5, 5.5, &in_love()));
        lay(
            &mut region,
            &idling(
                entity_types::CAMEL,
                EntityId(44),
                10.5,
                10.5,
                &Animal::default(),
            ),
        );
        lay(
            &mut region,
            &idling(
                entity_types::CAMEL_HUSK,
                EntityId(46),
                5.5,
                11.5,
                &Animal::default(),
            ),
        );
        lay(
            &mut region,
            &idling(entity_types::ARMADILLO, EntityId(48), 11.5, 4.5, &baby()),
        );
        let mut seen = Vec::new();
        for tick in 0..300 {
            let inputs = if tick == 100 {
                moves(vec![numbered(
                    1,
                    PlayerInput::Keys(crate::api::Keys {
                        sprint: true,
                        ..Default::default()
                    }),
                )])
            } else {
                TickInputs::default()
            };
            let output = checked(&mut region, &inputs);
            seen.push((output.entity_changes, output.events));
        }
        seen
    };
    assert_eq!(run(), run());
}
