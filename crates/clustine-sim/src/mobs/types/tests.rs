//! The farm animals' own rules on their state alone, from the brief's section 4: the age
//! and its stamps, feeding's tenth owed back, love and the asked mark, the variants and
//! colours drawn, the colours mixed, the sizes and what clients are shown.

use clustine_data::{entity_types, items};
use clustine_world::{EntityId, TypeState};

use super::animal::{self, Animal, BABY_START_AGE, Own};
use super::chicken::ChickenState;
use super::cow::CowState;
use super::pig::PigState;
use super::rabbit::{RabbitState, random_variant, variant};
use super::sheep::{COLOURS, SheepState, dye_colour, mixed_colour, spawn_colour};
use super::{Dimensions, of};
use crate::mobs::Mob;
use crate::region::Random;
use crate::shown::{DataShown, Holder};

fn draws() -> Random {
    Random::of_entity(7, EntityId(3), 11)
}

#[test]
fn a_fresh_animal_keeps_no_state_and_its_own_part_follows_what_every_animal_shares() {
    assert_eq!(
        animal::write(&Animal::default(), &CowState::default()),
        TypeState::default()
    );
    let mut shared = Animal::default();
    shared.set_in_love(20);
    let own = CowState {
        variant: 2,
        sound: 1,
    };
    let state = animal::write(&shared, &own);
    assert_eq!(
        animal::read::<CowState>(&state),
        (shared.clone(), own.clone())
    );
    assert_eq!(animal::animal_of(&state), shared);
    // What every animal shares changed, its type's own kept.
    let mut reset = shared.clone();
    reset.reset_love();
    let changed = animal::with_animal(&state, &reset);
    assert_eq!(animal::read::<CowState>(&changed), (reset, own));
    // A type's own part as fresh is not written.
    let bare = animal::write(&shared, &SheepState::default());
    assert_eq!(animal::read::<SheepState>(&bare).1, SheepState::default());
}

#[test]
fn a_baby_grows_by_one_a_tick_to_nothing_in_24000_and_an_adult_s_cooldown_runs_down() {
    let mut calf = Animal::default();
    calf.set_age(BABY_START_AGE, 100);
    assert!(calf.is_baby() && calf.can_age_up());
    assert_eq!(calf.age(100), -24_000);
    assert_eq!(calf.age(100 + 23_999), -1);
    assert_eq!(calf.age(100 + 24_000), 0);
    assert_eq!(calf.age(100 + 30_000), 0);
    // Locked, it waits.
    calf.age_locked = true;
    assert!(!calf.can_age_up());
    assert_eq!(calf.age(100 + 5_000), -24_000);
    let mut parent = Animal::default();
    parent.bred(50);
    assert_eq!(parent.age(50), 6000);
    assert_eq!(parent.age(50 + 5999), 1);
    assert_eq!(parent.age(50 + 7000), 0);
    assert!(!parent.is_baby());
}

#[test]
fn feeding_a_baby_takes_a_tenth_of_what_is_left_and_owes_it_back_as_a_cooldown() {
    assert_eq!(animal::speed_up_seconds(24_000), 120);
    assert_eq!(animal::speed_up_seconds(2_399), 11);
    let mut calf = Animal::default();
    calf.set_age(BABY_START_AGE, 0);
    calf.age_up(animal::speed_up_seconds(24_000), true, 0);
    assert_eq!(calf.age(0), -21_600);
    assert_eq!(calf.forced_age, 2_400);
    // Fed again near its end, it grows up at once and owes all that it was given.
    calf.set_age(-100, 10);
    calf.age_up(animal::speed_up_seconds(100), true, 10);
    assert_eq!(calf.age(10), -100);
    calf.age_up(60, true, 10);
    assert!(!calf.is_baby());
    assert_eq!(calf.age(10), 2_500);
    // A sheep that grows up eating owes what feeding gave it, nothing more.
    let mut lamb = Animal::default();
    lamb.set_age(-1_000, 0);
    lamb.age_up(60, false, 0);
    assert_eq!(lamb.age(0), 0);
}

#[test]
fn love_lasts_600_ticks_and_an_asker_is_not_in_love_for_others_for_40() {
    let mut cow = Animal::default();
    cow.set_in_love(10);
    assert!(cow.in_love(609) && !cow.in_love(610));
    assert_eq!(cow.love(10), 600);
    cow.asked = Some(animal::Asked {
        partner: EntityId(9),
        number: 1,
        at: 20,
    });
    assert!(cow.in_love(30) && !cow.in_love_for_others(30));
    assert!(cow.waiting(59).is_some() && cow.waiting(60).is_none());
    assert!(cow.in_love_for_others(60));
}

#[test]
fn a_temperature_variant_is_the_biome_s_with_one_draw_and_temperate_elsewhere() {
    for (biome, expected) in [
        (Some("minecraft:badlands"), "minecraft:warm"),
        (Some("minecraft:snowy_plains"), "minecraft:cold"),
        (Some("minecraft:plains"), "minecraft:temperate"),
        (None, "minecraft:temperate"),
    ] {
        for registry in [
            "minecraft:cow_variant",
            "minecraft:pig_variant",
            "minecraft:chicken_variant",
        ] {
            let mut level = draws();
            let start = level.clone();
            let id = animal::variants::select(registry, biome, &mut level);
            assert_eq!(id, animal::variants::id(registry, expected), "{biome:?}");
            // `getRandomSafe` draws `nextInt(1)` even for one.
            assert_eq!(level.draws_since(&start), 1);
        }
    }
}

#[test]
fn a_sheep_s_colour_at_spawn_is_one_of_its_biome_s_few_else_its_common_one_or_pink() {
    let few = |colour: &str| COLOURS.iter().position(|c| *c == colour).unwrap() as u8;
    for (biome, singles, common) in [
        (
            Some("minecraft:plains"),
            ["black", "gray", "light_gray", "brown"],
            "white",
        ),
        (
            Some("minecraft:badlands"),
            ["gray", "light_gray", "white", "black"],
            "brown",
        ),
        (
            Some("minecraft:snowy_plains"),
            ["light_gray", "gray", "white", "brown"],
            "black",
        ),
    ] {
        for seed in 0..200 {
            let mut level = Random::of_entity(seed, EntityId(1), 0);
            let mut replay = level.clone();
            let colour = spawn_colour(biome, &mut level);
            let drawn = replay.next_int(100) as i32;
            let weights = [5, 5, 5, 3];
            let mut left = drawn;
            let mut expected = None;
            for (name, weight) in singles.iter().zip(weights) {
                left -= weight;
                if left < 0 {
                    expected = Some(few(name));
                    break;
                }
            }
            let expected = expected.unwrap_or_else(|| {
                if replay.next_int(500) < 499 {
                    few(common)
                } else {
                    few("pink")
                }
            });
            assert_eq!(colour, expected, "{biome:?} {drawn}");
            assert_eq!(level, replay);
        }
    }
}

#[test]
fn two_dyes_a_recipe_mixes_make_its_colour_and_any_other_pair_one_of_the_two() {
    let id = |name: &str| COLOURS.iter().position(|c| *c == name).unwrap() as u8;
    let mut level = draws();
    for (a, b, made) in [
        ("black", "white", "gray"),
        ("white", "gray", "light_gray"),
        ("blue", "white", "light_blue"),
        ("green", "white", "lime"),
        ("white", "red", "pink"),
        ("blue", "green", "cyan"),
        ("red", "blue", "purple"),
        ("red", "yellow", "orange"),
        ("pink", "purple", "magenta"),
    ] {
        let start = level.clone();
        assert_eq!(mixed_colour(id(a), id(b), &mut level), id(made));
        assert_eq!(level.draws_since(&start), 0);
    }
    for (a, b) in [("white", "white"), ("red", "green"), ("black", "orange")] {
        let mut replay = level.clone();
        let mixed = mixed_colour(id(a), id(b), &mut level);
        let expected = if replay.next_bool() { id(a) } else { id(b) };
        assert_eq!(mixed, expected);
    }
}

#[test]
fn a_dye_s_colour_is_its_name_s() {
    assert_eq!(dye_colour(items::LIME_DYE), Some(5));
    assert_eq!(dye_colour(items::BLACK_DYE), Some(15));
    assert_eq!(dye_colour(items::WHEAT), None);
}

#[test]
fn a_rabbit_s_variant_is_white_in_the_snow_gold_in_the_desert_and_mixed_elsewhere() {
    for seed in 0..100 {
        let mut level = Random::of_entity(seed, EntityId(2), 0);
        let drawn = level.clone().next_int(100);
        let snowy = random_variant(Some("minecraft:snowy_plains"), &mut level.clone());
        assert_eq!(
            snowy,
            if drawn < 80 {
                variant::WHITE
            } else {
                variant::WHITE_SPLOTCHED
            }
        );
        assert_eq!(
            random_variant(Some("minecraft:desert"), &mut level.clone()),
            variant::GOLD
        );
        let plain = random_variant(Some("minecraft:plains"), &mut level);
        let expected = if drawn < 50 {
            variant::BROWN
        } else if drawn < 90 {
            variant::SALT
        } else {
            variant::BLACK
        };
        assert_eq!(plain, expected);
    }
}

/// A mob of `kind` whose shared state is `shared` and its own `own`.
fn mob_with<O: Own>(shared: &Animal, own: &O) -> Mob {
    let mut mob = Mob::fresh();
    mob.state = animal::write(shared, own);
    mob
}

#[test]
fn babies_have_sizes_of_their_own_and_adults_their_type_s() {
    let mut baby = Animal::default();
    baby.set_age(BABY_START_AGE, 0);
    for (kind, size) in [
        (entity_types::COW, (0.45, 0.7, 0.69)),
        (entity_types::MOOSHROOM, (0.45, 0.7, 0.69)),
        (entity_types::PIG, (0.45, 0.45, 0.406_25)),
        (entity_types::SHEEP, (0.45, 0.65, 0.656_25)),
        (entity_types::CHICKEN, (0.3, 0.4, 0.281_25)),
        (entity_types::RABBIT, (0.24, 0.4, 0.39)),
    ] {
        let module = of(kind as u16);
        let calf = mob_with(&baby, &CowState::default());
        assert_eq!(
            module.dimensions(&calf),
            Some(Dimensions {
                width: size.0,
                height: size.1,
                eye_height: size.2,
            }),
            "{kind}"
        );
        assert!(module.is_baby(&calf));
        let adult = Mob::fresh();
        assert_eq!(module.dimensions(&adult), None);
        assert!(!module.is_baby(&adult));
    }
}

#[test]
fn each_animal_shows_its_baby_its_age_lock_and_its_own_values_from_index_18() {
    let shared = Animal::default();
    let cow = of(entity_types::COW as u16).shown(&mob_with(&shared, &CowState::default()));
    let temperate = animal::variants::id("minecraft:cow_variant", "minecraft:temperate");
    let classic = animal::variants::id("minecraft:cow_sound_variant", "minecraft:classic");
    assert_eq!(
        cow,
        [
            (16, DataShown::Boolean(false), true),
            (17, DataShown::Boolean(false), true),
            (
                18,
                DataShown::Holder(Holder::CowVariant, i32::from(temperate)),
                true
            ),
            (
                19,
                DataShown::Holder(Holder::CowSoundVariant, i32::from(classic)),
                true
            ),
        ]
    );
    let mut baby = Animal::default();
    baby.set_age(BABY_START_AGE, 0);
    baby.age_locked = true;
    let pig = PigState {
        variant: animal::variants::id("minecraft:pig_variant", "minecraft:cold"),
        ..PigState::default()
    };
    let shown = of(entity_types::PIG as u16).shown(&mob_with(&baby, &pig));
    assert_eq!(shown[0], (16, DataShown::Boolean(true), false));
    assert_eq!(shown[1], (17, DataShown::Boolean(true), false));
    assert_eq!(shown[2], (18, DataShown::Int(0), true));
    assert_eq!(
        shown[3],
        (
            19,
            DataShown::Holder(Holder::PigVariant, i32::from(pig.variant)),
            false
        )
    );
    assert_eq!(shown.len(), 5);
    let mut wool = SheepState::default();
    wool.set_colour(14);
    wool.set_sheared(true);
    let sheep = of(entity_types::SHEEP as u16).shown(&mob_with(&shared, &wool));
    assert_eq!(sheep[2], (18, DataShown::Byte(0x1E), false));
    let rabbit = RabbitState {
        variant: variant::GOLD,
        ..RabbitState::default()
    };
    let shown = of(entity_types::RABBIT as u16).shown(&mob_with(&shared, &rabbit));
    assert_eq!(shown[2], (18, DataShown::Int(variant::GOLD), false));
    let chicken =
        of(entity_types::CHICKEN as u16).shown(&mob_with(&shared, &ChickenState::default()));
    assert_eq!(chicken.len(), 4);
    assert!(chicken.iter().all(|(_, _, default)| *default));
    let mooshroom = of(entity_types::MOOSHROOM as u16).shown(&mob_with(
        &shared,
        &super::mooshroom::MooshroomState {
            brown: true,
            stew: None,
        },
    ));
    assert_eq!(mooshroom[2], (18, DataShown::Int(1), false));
}
