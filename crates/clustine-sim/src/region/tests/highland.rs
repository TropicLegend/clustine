//! The highland animals in a region, as a player meets them before their brains come
//! (`docs/groundwork/n7j-plan.md`, section 4.3, parts B to E): an egg makes each with
//! the draws of its spawn in the game's order, a goat is milked and fed, an armadillo
//! is brushed for a scute, sheds one on its timer and rolls up when hit, a camel is fed,
//! saddled, stands up when hit and has the sizes and seats of its pose, and what clients
//! are shown of each.

use clustine_data::{entity_types, items, sounds};
use clustine_world::{Entity, EntityChange, EntityData, EntityId, TypeState};

use super::*;
use crate::living::MAX_AIR;
use crate::mobs::Mob;
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
            EntityData::Item { stack, .. } => Some(stack),
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
    ItemStack { item, count: 1 }
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
fn a_saddle_goes_on_a_grown_camel_with_its_own_sound_and_a_click_without_one_mounts_no_one_yet() {
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
        // An empty hand: the riding's, which mounts no one until part R.
        holding(&mut region, one(items::STICK));
        use_on(&mut region, ONE, false);
        assert!(record(&region, ONE).riding.passengers.is_empty());
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
