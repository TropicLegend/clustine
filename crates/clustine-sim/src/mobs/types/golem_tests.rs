//! The iron and snow golems' own rules, apart from a region
//! (`docs/groundwork/n7k-brief.md`, sections 1.2, 1.4 and 11.6): the cracks' levels,
//! what clients are shown, what a template's tag gives, and where a snow golem's trail
//! falls and where it melts.

use clustine_world::BlockPos;

use super::MobType;
use super::iron_golem::{self, IronGolem, IronGolemState};
use super::snow_golem::{self, SnowGolem};
use super::template::build::{byte, compound};
use crate::mobs::Mob;
use crate::shown::DataShown;

#[test]
fn an_iron_golem_cracks_below_three_quarters_a_half_and_a_quarter_strictly() {
    assert_eq!(iron_golem::crackiness(1.0), 0);
    assert_eq!(iron_golem::crackiness(0.75), 0);
    assert_eq!(iron_golem::crackiness(0.749_999_9), 1);
    assert_eq!(iron_golem::crackiness(0.5), 1);
    assert_eq!(iron_golem::crackiness(0.499_999_97), 2);
    assert_eq!(iron_golem::crackiness(0.25), 2);
    assert_eq!(iron_golem::crackiness(0.249_999_99), 3);
    assert_eq!(iron_golem::crackiness(0.0), 3);
}

#[test]
fn an_iron_golem_shows_its_player_made_flag_as_bit_one_of_index_sixteen() {
    let mut mob = Mob::fresh();
    assert_eq!(IronGolem.shown(&mob), [(16, DataShown::Byte(0), true)]);
    iron_golem::player_created(&mut mob);
    assert_eq!(IronGolem.shown(&mob), [(16, DataShown::Byte(1), false)]);
}

#[test]
fn a_template_s_iron_golem_is_a_player_s_only_where_its_tag_says_so() {
    let mut mob = Mob::fresh();
    IronGolem.from_template(&mut mob, &compound(&[]));
    assert!(!iron_golem::state_of(&mob).player_created);
    IronGolem.from_template(&mut mob, &compound(&[byte("PlayerCreated", 1)]));
    assert!(iron_golem::state_of(&mob).player_created);
}

#[test]
fn an_iron_golem_s_counters_are_moments_that_a_restore_carries_on_from() {
    let own = IronGolemState {
        attack_animation_end: 110,
        offer_flower_end: 500,
        ..IronGolemState::default()
    };
    let restored = iron_golem::read(&iron_golem::write(&own));
    assert_eq!(restored, own);
    assert_eq!(iron_golem::attack_animation(&restored, 105), 5);
    assert_eq!(iron_golem::attack_animation(&restored, 200), 0);
    assert_eq!(iron_golem::offer_flower_tick(&restored, 100), 400);
    // A fresh golem's state is nothing at all in its record.
    assert!(
        iron_golem::write(&IronGolemState::default())
            .bytes
            .is_empty()
    );
}

#[test]
fn a_snow_golem_shows_its_pumpkin_and_a_template_s_has_one_unless_it_says_not() {
    let mut mob = Mob::fresh();
    assert_eq!(SnowGolem.shown(&mob), [(16, DataShown::Byte(0x10), true)]);
    SnowGolem.from_template(&mut mob, &compound(&[byte("Pumpkin", 0)]));
    assert_eq!(SnowGolem.shown(&mob), [(16, DataShown::Byte(0), false)]);
    SnowGolem.from_template(&mut mob, &compound(&[]));
    assert!(snow_golem::has_pumpkin(&mob));
}

#[test]
fn a_snow_golem_s_trail_falls_a_quarter_either_way_of_its_feet_x_first() {
    assert_eq!(
        snow_golem::trail_cells([10.5, 64.2, -3.5]),
        [BlockPos::new(10, 64, -4); 4]
    );
    assert_eq!(
        snow_golem::trail_cells([10.1, 64.0, -3.9]),
        [
            BlockPos::new(9, 64, -5),
            BlockPos::new(10, 64, -5),
            BlockPos::new(9, 64, -4),
            BlockPos::new(10, 64, -4),
        ]
    );
}

#[test]
fn a_snow_golem_melts_in_the_seven_warm_biomes_only() {
    for biome in [
        "minecraft:badlands",
        "minecraft:eroded_badlands",
        "minecraft:wooded_badlands",
        "minecraft:desert",
        "minecraft:savanna",
        "minecraft:savanna_plateau",
        "minecraft:windswept_savanna",
    ] {
        assert!(snow_golem::melts_in(Some(biome)), "{biome}");
    }
    for biome in [
        "minecraft:plains",
        "minecraft:jungle",
        "minecraft:nether_wastes",
    ] {
        assert!(!snow_golem::melts_in(Some(biome)), "{biome}");
    }
    assert!(!snow_golem::melts_in(None));
    assert!(snow_golem::MELTS_IN.is_sorted());
}
