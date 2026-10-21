//! The water creatures' own rules on their state alone, from the brief's sections 6.5 to
//! 6.10: the variants drawn and packed, the sizes they scale, the squid's strokes
//! counted in floats, what each shows clients, and a turtle's breeding.

use clustine_data::{EntityTypeInfo, entity_types};
use clustine_world::{BlockPos, EntityId, TypeState};

use super::animal::{self, Animal, BABY_START_AGE};
use super::dolphin::{self, DolphinState};
use super::fish::{self, FishState, Species};
use super::squid::{self, SquidState};
use super::tropical_fish::{self, COMMON, PATTERNS, pack};
use super::turtle::{self, TurtleState};
use super::{Dimensions, of, pufferfish, salmon, water};
use crate::mobs::Mob;
use crate::region::Random;
use crate::shown::DataShown;

fn kind(id: i32) -> u16 {
    id as u16
}

#[test]
fn a_salmon_is_small_three_times_in_ten_medium_five_and_large_one_and_a_half() {
    let mut counts = [0; 3];
    for seed in 0..2000 {
        let mut own = Random::of_entity(seed, EntityId(5), 0);
        let mut replay = own.clone();
        let size = salmon::random_size(&mut own);
        let drawn = replay.next_int(95);
        let expected = if drawn < 30 {
            salmon::SMALL
        } else if drawn < 80 {
            salmon::MEDIUM
        } else {
            salmon::LARGE
        };
        assert_eq!(size, expected);
        assert_eq!(own, replay, "one draw");
        counts[size as usize] += 1;
    }
    // About 30, 50 and 15 in 95.
    assert!((550..750).contains(&counts[0]), "{counts:?}");
    assert!((950..1150).contains(&counts[1]), "{counts:?}");
    assert!((230..400).contains(&counts[2]), "{counts:?}");
}

#[test]
fn a_salmon_s_box_and_a_pufferfish_s_scale_with_its_size_and_its_puff() {
    let info = EntityTypeInfo::of(entity_types::SALMON);
    let mut mob = Mob::fresh();
    let module = of(kind(entity_types::SALMON));
    // Fresh, it is medium: its type's own size.
    assert_eq!(
        module.dimensions(&mob),
        Some(Dimensions {
            width: info.width,
            height: info.height,
            eye_height: info.eye_height,
        })
    );
    fish::update(kind(entity_types::SALMON), &mut mob, |fish| {
        fish.variant = salmon::LARGE
    });
    let large = module.dimensions(&mob).unwrap();
    assert_eq!(large.width, info.width * 1.5);
    assert_eq!(large.eye_height, info.eye_height * 1.5);
    // An id out of range is the nearest size.
    assert_eq!(salmon::scale(7), 1.5);
    assert_eq!(salmon::scale(-3), 0.5);
    let puffer = EntityTypeInfo::of(entity_types::PUFFERFISH);
    for (puff, scale) in [(0, 0.5), (1, 0.7), (2, 1.0)] {
        assert_eq!(pufferfish::size(puff).width, puffer.width * scale);
        assert_eq!(pufferfish::size(puff).height, puffer.height * scale);
    }
}

#[test]
fn a_tropical_fish_packs_its_pattern_and_its_two_colours_in_one_number() {
    // Kob, white on white, is nothing; a large pattern has its base in its low byte.
    assert_eq!(pack(PATTERNS[0], 0, 0), 0);
    let (stripey, orange, gray) = COMMON[0];
    assert_eq!(stripey, 0x101);
    assert_eq!(pack(stripey, orange, gray), 0x101 | 1 << 16 | 7 << 24);
    assert_eq!(COMMON.len(), 22);
    assert_eq!(PATTERNS.len(), 12);
}

#[test]
fn a_tropical_fish_is_one_of_the_common_kinds_nine_times_in_ten_and_draws_the_level_s() {
    let mut common = 0;
    for seed in 0..3000 {
        let mut level = Random::of_entity(seed, EntityId(9), 0);
        let mut replay = level.clone();
        let variant = tropical_fish::random_variant(&mut level);
        let expected = if f64::from(replay.next_float()) < 0.9 {
            common += 1;
            let (pattern, base, colour) = COMMON[replay.next_int(22) as usize];
            pack(pattern, base, colour)
        } else {
            let pattern = PATTERNS[replay.next_int(12) as usize];
            let base = replay.next_int(16) as i32;
            let colour = replay.next_int(16) as i32;
            pack(pattern, base, colour)
        };
        assert_eq!(variant, expected);
        assert_eq!(level, replay);
    }
    assert!((2600..2800).contains(&common), "{common}");
}

#[test]
fn a_fish_shows_whether_it_came_from_a_bucket_and_its_own_value_at_17() {
    let cod = of(kind(entity_types::COD));
    let fresh = Mob::fresh();
    assert_eq!(cod.shown(&fresh), [(16, DataShown::Boolean(false), true)]);
    let salmon = of(kind(entity_types::SALMON));
    assert_eq!(
        salmon.shown(&fresh),
        [
            (16, DataShown::Boolean(false), true),
            (17, DataShown::Int(1), true),
        ]
    );
    let mut poured = Mob::fresh();
    fish::set_from_bucket(kind(entity_types::PUFFERFISH), &mut poured);
    fish::update(kind(entity_types::PUFFERFISH), &mut poured, |fish| {
        fish.puff = 2
    });
    assert_eq!(
        of(kind(entity_types::PUFFERFISH)).shown(&poured),
        [
            (16, DataShown::Boolean(true), false),
            (17, DataShown::Int(2), false),
        ]
    );
    let mut tropical = Mob::fresh();
    let packed = pack(COMMON[5].0, COMMON[5].1, COMMON[5].2);
    fish::update(kind(entity_types::TROPICAL_FISH), &mut tropical, |fish| {
        fish.variant = packed;
    });
    assert_eq!(
        of(kind(entity_types::TROPICAL_FISH)).shown(&tropical)[1],
        (17, DataShown::Int(packed), false)
    );
}

#[test]
fn a_fish_from_a_bucket_never_despawns_and_a_fish_is_on_no_lead() {
    let module = of(kind(entity_types::COD));
    let mut mob = Mob::fresh();
    assert!(module.remove_when_far_away(kind(entity_types::COD), &mob, 0, 100.0));
    fish::set_from_bucket(kind(entity_types::COD), &mut mob);
    assert!(!module.remove_when_far_away(kind(entity_types::COD), &mob, 0, 100.0));
    assert!(!module.can_be_leashed(kind(entity_types::COD), &mob));
    assert!(of(kind(entity_types::SQUID)).can_be_leashed(kind(entity_types::SQUID), &mob));
    assert!(of(kind(entity_types::DOLPHIN)).can_be_leashed(kind(entity_types::DOLPHIN), &mob));
    assert!(!of(kind(entity_types::TURTLE)).can_be_leashed(kind(entity_types::TURTLE), &mob));
}

#[test]
fn a_fish_state_is_its_own_encoding_which_no_animal_reading_takes_for_an_animal_s() {
    let cod = kind(entity_types::COD);
    let mut state = FishState::fresh(Species::Cod);
    state.leader = Some(EntityId(77));
    state.school_size = 4;
    let written = fish::write(cod, &state);
    assert_eq!(fish::read(cod, &written), state);
    assert_eq!(animal::animal_of(&written), Animal::default());
    assert_eq!(
        fish::read(cod, &TypeState::default()),
        FishState::fresh(Species::Cod)
    );
}

#[test]
fn a_squid_s_tentacles_count_up_by_its_speed_in_floats_from_their_stroke_s_start() {
    let own = SquidState {
        phase: 0.5,
        phase_from: 10,
        tentacle_speed: 0.15,
        ..SquidState::default()
    };
    // The tick that begins at 10 finds the phase as the stroke began.
    assert_eq!(own.tentacle_movement(10), 0.5);
    let mut summed = 0.5_f32;
    for _ in 0..7 {
        summed += 0.15;
    }
    assert_eq!(own.tentacle_movement(17).to_bits(), summed.to_bits());
    // A crash's restore before the stroke's start finds it as it began.
    assert_eq!(own.tentacle_movement(3), 0.5);
}

#[test]
fn a_squid_s_constructor_draws_its_first_stroke_s_speed_between_a_tenth_and_a_fifth() {
    for seed in 0..200 {
        let mut own = Random::of_entity(seed, EntityId(1), 0);
        let mut mob = Mob::fresh();
        squid::construct(&mut mob, &mut own);
        let (_, state) = squid::read(&mob);
        assert!(
            (0.1..=0.2).contains(&state.tentacle_speed),
            "{}",
            state.tentacle_speed
        );
    }
}

#[test]
fn a_squid_shows_the_ageable_mob_s_values_and_a_glow_squid_its_dark_ticks_at_18() {
    let mut mob = Mob::fresh();
    let shown = of(kind(entity_types::SQUID)).shown(&mob);
    assert_eq!(
        shown,
        [
            (16, DataShown::Boolean(false), true),
            (17, DataShown::Boolean(false), true),
        ]
    );
    squid::update(&mut mob, |own| own.dark_ticks = 42);
    let shown = of(kind(entity_types::GLOW_SQUID)).shown(&mob);
    assert_eq!(shown[2], (18, DataShown::Int(42), false));
}

#[test]
fn babies_of_the_water_creatures_have_sizes_of_their_own() {
    let mut baby = Mob::fresh();
    animal::made_baby(&mut baby);
    let squid = of(kind(entity_types::SQUID)).dimensions(&baby).unwrap();
    assert_eq!(
        (squid.width, squid.height, squid.eye_height),
        (0.5, 0.5, 0.37)
    );
    let dolphin = of(kind(entity_types::DOLPHIN)).dimensions(&baby).unwrap();
    assert_eq!(dolphin.width, 0.9_f32 * 0.65);
    assert_eq!(dolphin.eye_height, 0.093_75);
    let turtle = of(kind(entity_types::TURTLE)).dimensions(&baby).unwrap();
    assert_eq!(turtle.width, 1.2_f32 * 0.3);
    assert_eq!(turtle.height, 0.4_f32 * 0.3);
    assert!(
        of(kind(entity_types::TURTLE))
            .dimensions(&Mob::fresh())
            .is_none()
    );
}

#[test]
fn a_dolphin_is_defined_with_4800_air_and_full_moistness_and_shows_its_fish_and_moistness() {
    let module = of(kind(entity_types::DOLPHIN));
    assert_eq!(module.max_air(), 4800);
    assert!(module.refills_air_at_once());
    let mut mob = Mob::fresh();
    mob.body.vitals.air = 4800;
    let values = crate::shown::mob_values(kind(entity_types::DOLPHIN), &mob.encode());
    // Its air is the default it is defined with, not shown to a client being paired.
    assert!(values.contains(&(1, DataShown::Int(4800), true)));
    assert!(values.contains(&(18, DataShown::Boolean(false), true)));
    assert!(values.contains(&(19, DataShown::Int(2400), true)));
    dolphin::update(&mut mob, |own| {
        own.got_fish = true;
        own.moistness = 1200;
    });
    let values = module.shown(&mob);
    assert_eq!(values[2], (18, DataShown::Boolean(true), false));
    assert_eq!(values[3], (19, DataShown::Int(1200), false));
    // A fresh dolphin's state is written as nothing.
    assert_eq!(
        animal::write(&Animal::default(), &DolphinState::default()),
        TypeState::default()
    );
}

#[test]
fn a_dolphin_s_direction_is_its_yaw_s_quarter_south_west_north_east() {
    assert_eq!(dolphin::direction_steps(0.0), (0, 1));
    assert_eq!(dolphin::direction_steps(44.0), (0, 1));
    assert_eq!(dolphin::direction_steps(46.0), (-1, 0));
    assert_eq!(dolphin::direction_steps(180.0), (0, -1));
    assert_eq!(dolphin::direction_steps(-90.0), (1, 0));
    assert_eq!(dolphin::direction_steps(270.0), (1, 0));
}

#[test]
fn a_turtle_shows_its_egg_and_its_digging_and_its_breeding_gives_it_an_egg() {
    let module = of(kind(entity_types::TURTLE));
    let mob = Mob::fresh();
    let shown = module.shown(&mob);
    assert_eq!(shown[2], (18, DataShown::Boolean(false), true));
    assert_eq!(shown[3], (19, DataShown::Boolean(false), true));
    let bred = module
        .bred_without_child(&mob)
        .expect("an egg, not a child");
    let mut with_egg = Mob::fresh();
    with_egg.state = bred;
    assert!(turtle::read(&with_egg).has_egg);
    assert_eq!(
        module.shown(&with_egg)[2],
        (18, DataShown::Boolean(true), false)
    );
    // No other water creature breeds so.
    assert!(
        of(kind(entity_types::DOLPHIN))
            .bred_without_child(&mob)
            .is_none()
    );
}

#[test]
fn a_hatchling_is_a_newborn_whose_home_is_its_nest() {
    let nest = BlockPos::new(3, -60, 7);
    let baby = turtle::hatchling(nest);
    let shared = animal::animal_of(&baby.state);
    assert_eq!(shared.age(0), BABY_START_AGE);
    assert_eq!(turtle::read(&baby).home, nest);
    assert_eq!(
        TurtleState::default().home,
        BlockPos::new(0, 0, 0),
        "the constructor's home is the origin"
    );
}

#[test]
fn a_block_s_middle_is_nearer_than_a_distance_by_its_square() {
    let pos = BlockPos::new(0, 0, 0);
    assert!(turtle::closer_to_centre_than(pos, [0.5, 0.5, 16.4], 16.0));
    assert!(!turtle::closer_to_centre_than(pos, [0.5, 0.5, 16.5], 16.0));
}

#[test]
fn a_bucket_takes_the_four_fish_and_pours_what_it_took() {
    for id in [
        entity_types::COD,
        entity_types::SALMON,
        entity_types::PUFFERFISH,
        entity_types::TROPICAL_FISH,
    ] {
        let bucket = water::bucket_of(kind(id)).expect("a bucket");
        assert_eq!(water::poured_by(bucket), Some(kind(id)));
    }
    assert_eq!(water::bucket_of(kind(entity_types::SQUID)), None);
    assert_eq!(water::poured_by(clustine_data::items::WATER_BUCKET), None);
}

#[test]
fn every_water_creature_is_built_so_that_its_egg_makes_it() {
    for id in [
        entity_types::COD,
        entity_types::SALMON,
        entity_types::PUFFERFISH,
        entity_types::TROPICAL_FISH,
        entity_types::SQUID,
        entity_types::GLOW_SQUID,
        entity_types::DOLPHIN,
        entity_types::TURTLE,
    ] {
        assert!(super::is_built(kind(id)), "{id}");
    }
}
