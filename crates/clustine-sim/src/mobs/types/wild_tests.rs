//! The wild land animals' own rules on their state alone, from the brief's sections 6.1
//! to 6.4 and 6.11: the fox's variant and its mouth, the panda's genes and their
//! inheritance, the bear's and the bee's anger, the bee's flowers, crops and hours, a
//! hive's data, the bat's rest, and what clients are shown of each.

use clustine_data::{blocks, entity_types, items};
use clustine_world::{BlockPos, EntityId, ItemStack, TypeState};

use super::animal::{self, Animal};
use super::bat::{self, BatState};
use super::bee::{self, BeeState, attracts_bees, bees_stay_in_hive, grown};
use super::fox::{self, FoxState, can_hold_item, flag_bits, is_consumable_food};
use super::neutral::Anger;
use super::panda::{self, PandaState, gene, random_gene, variant_of};
use super::polar_bear::{self, BearState};
use super::{Breeding, Spawning, of};
use crate::mobs::Mob;
use crate::region::Random;
use crate::shown::DataShown;

fn draws(seed: u64) -> Random {
    Random::of_entity(seed, EntityId(5), 9)
}

fn one(item: i32) -> Option<ItemStack> {
    Some(ItemStack { item, count: 1 })
}

fn fox_with(own: &FoxState) -> Mob {
    let mut mob = Mob::fresh();
    mob.state = animal::write(&Animal::default(), own);
    mob
}

#[test]
fn every_wild_land_animal_is_built_and_only_the_bat_is_no_animal() {
    for kind in [
        entity_types::FOX,
        entity_types::PANDA,
        entity_types::POLAR_BEAR,
        entity_types::BEE,
        entity_types::BAT,
    ] {
        assert!(super::is_built(kind as u16), "{kind}");
    }
    let mut bat = clustine_world::Entity {
        key: EntityId(1),
        kind: entity_types::BAT as u16,
        uuid: uuid::Uuid::from_u128(1),
        position: [0.0; 3],
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: Mob::fresh().encode(),
    };
    assert!(animal::of_record(&bat).is_none());
    bat.kind = entity_types::FOX as u16;
    assert!(animal::of_record(&bat).is_some());
}

#[test]
fn a_fox_is_snowy_in_a_snowy_biome_hunts_and_holds_one_thing_in_five_by_the_level_s_draws() {
    for (biome, expected) in [
        (Some("minecraft:snowy_plains"), fox::variant::SNOW),
        (Some("minecraft:grove"), fox::variant::SNOW),
        (Some("minecraft:taiga"), fox::variant::RED),
        (None, fox::variant::RED),
    ] {
        let mut mob = Mob::fresh();
        let mut level = draws(3);
        of(entity_types::FOX as u16).finalize_spawn(&mut Spawning {
            reason: crate::mobs::types::SpawnReason::SpawnEgg,
            group: &mut None,
            structure: None,
            moon_brightness: 1.0,
            difficulty: (0.0, 0.0),
            mob: &mut mob,
            level: &mut level,
            own: &mut draws(99),
            biome,
            at: BlockPos::new(0, -60, 0),
            day: 0,
            game_time: 0,
        });
        let own = fox::state_of(&mob);
        assert_eq!(own.variant, expected, "{biome:?}");
        assert!(own.hunts);
    }
    // One draw only, where nothing is put in its mouth; else the item's.
    let mut held = 0;
    for seed in 0..400 {
        let mut mob = Mob::fresh();
        let mut level = draws(seed);
        let mut replay = level.clone();
        of(entity_types::FOX as u16).finalize_spawn(&mut Spawning {
            reason: crate::mobs::types::SpawnReason::SpawnEgg,
            group: &mut None,
            structure: None,
            moon_brightness: 1.0,
            difficulty: (0.0, 0.0),
            mob: &mut mob,
            level: &mut level,
            own: &mut draws(99),
            biome: None,
            at: BlockPos::new(0, -60, 0),
            day: 0,
            game_time: 0,
        });
        let hand = mob.body.equipment.slots[0];
        if replay.next_float() < 0.2 {
            held += 1;
            let f = replay.next_float();
            let expected = if f < 0.05 {
                items::EMERALD
            } else if f < 0.2 {
                items::EGG
            } else if f < 0.4 {
                if replay.next_bool() {
                    items::RABBIT_FOOT
                } else {
                    items::RABBIT_HIDE
                }
            } else if f < 0.6 {
                items::WHEAT
            } else if f < 0.8 {
                items::LEATHER
            } else {
                items::FEATHER
            };
            assert_eq!(hand, one(expected), "{seed}");
        } else {
            assert_eq!(hand, None);
        }
        assert_eq!(level, replay, "{seed}");
    }
    assert!((40..=120).contains(&held), "{held}");
}

#[test]
fn a_fox_cub_takes_a_parent_s_variant_by_the_breeding_parent_s_draw_and_trusts_the_egg_s_player() {
    let red = fox_with(&FoxState::default());
    let snow = fox_with(&FoxState {
        variant: fox::variant::SNOW,
        ..FoxState::default()
    });
    for seed in 0..40 {
        let mut stream = draws(seed);
        let mut replay = stream.clone();
        let child = of(entity_types::FOX as u16)
            .breed_offspring(&mut Breeding {
                this: &red,
                partner: &snow,
                partner_kind: entity_types::FOX as u16,
                child_kind: None,
                draws: &mut stream,
                biome: None,
                egg_user: None,
            })
            .unwrap();
        let expected = if replay.next_bool() {
            fox::variant::RED
        } else {
            fox::variant::SNOW
        };
        let own = fox::state_of(&child);
        assert_eq!(own.variant, expected);
        // Bred, it hunts nothing (finding F1) and trusts no one.
        assert!(!own.hunts);
        assert_eq!(own.trusted, [None, None]);
    }
    let child = of(entity_types::FOX as u16)
        .breed_offspring(&mut Breeding {
            this: &red,
            partner: &red,
            partner_kind: entity_types::FOX as u16,
            child_kind: None,
            draws: &mut draws(1),
            biome: None,
            egg_user: Some(77),
        })
        .unwrap();
    assert_eq!(fox::state_of(&child).trusted, [Some(77), None]);
    // A third trust replaces the second.
    let mut own = FoxState::default();
    own.add_trusted(1);
    own.add_trusted(2);
    own.add_trusted(3);
    assert_eq!(own.trusted, [Some(1), Some(3)]);
}

#[test]
fn a_fox_holds_anything_in_an_empty_mouth_and_swaps_only_what_is_no_food_for_food_once_it_has_eaten()
 {
    // A fox made now has eaten nothing since: in its first tick it counts one.
    let fresh = FoxState {
        eaten_at: Some(10),
        ..FoxState::default()
    };
    assert!(can_hold_item(&fresh, 10, None, one(items::STICK).unwrap()));
    assert!(!can_hold_item(
        &fresh,
        10,
        one(items::STICK),
        one(items::APPLE).unwrap()
    ));
    assert!(can_hold_item(
        &fresh,
        11,
        one(items::STICK),
        one(items::APPLE).unwrap()
    ));
    assert!(!can_hold_item(
        &fresh,
        11,
        one(items::APPLE),
        one(items::BREAD).unwrap()
    ));
    assert!(!can_hold_item(
        &fresh,
        11,
        one(items::STICK),
        one(items::EMERALD).unwrap()
    ));
    assert_eq!(FoxState::default().ticks_since_eaten(0), 1);
    assert_eq!(fresh.ticks_since_eaten(611), 601);
    // The foods that are eaten: the berries, a stew, not milk.
    for item in [
        items::SWEET_BERRIES,
        items::GLOW_BERRIES,
        items::RABBIT_STEW,
    ] {
        assert!(is_consumable_food(one(item)));
    }
    for item in [items::MILK_BUCKET, items::WHEAT, items::POTION] {
        assert!(!is_consumable_food(one(item)));
    }
}

#[test]
fn a_fox_shows_its_variant_its_flags_and_whom_it_trusts_at_18_to_21() {
    let fresh = fox_with(&FoxState::default());
    let values = of(entity_types::FOX as u16).shown(&fresh);
    assert!(values.contains(&(18, DataShown::Int(0), true)));
    assert!(values.contains(&(19, DataShown::Byte(0), true)));
    assert!(values.contains(&(20, DataShown::OptionalUuid(None), true)));
    assert!(values.contains(&(21, DataShown::OptionalUuid(None), true)));
    let mut own = FoxState {
        variant: fox::variant::SNOW,
        ..FoxState::default()
    };
    own.set(flag_bits::SLEEPING, true);
    own.set(flag_bits::CROUCHING, true);
    own.add_trusted(9);
    let values = of(entity_types::FOX as u16).shown(&fox_with(&own));
    assert!(values.contains(&(18, DataShown::Int(1), false)));
    assert!(values.contains(&(19, DataShown::Byte(32 | 4), false)));
    assert!(values.contains(&(20, DataShown::OptionalUuid(Some(9)), false)));
    // The ageable mob's own come first.
    assert_eq!(values[0].0, 16);
}

#[test]
fn a_panda_gene_is_drawn_one_in_sixteen_and_a_recessive_one_shows_only_doubled() {
    // `nextInt(16)`: 0 lazy, 1 worried, 2 playful, 4 aggressive, 3 and 5 to 8 weak, 9 and
    // 10 brown, 11 to 15 normal.
    let mut counts = [0; 7];
    for seed in 0..4000 {
        let mut random = draws(seed);
        let mut replay = random.clone();
        let drawn = random_gene(&mut random);
        let expected = match replay.next_int(16) {
            0 => gene::LAZY,
            1 => gene::WORRIED,
            2 => gene::PLAYFUL,
            4 => gene::AGGRESSIVE,
            3 | 5..=8 => gene::WEAK,
            9 | 10 => gene::BROWN,
            _ => gene::NORMAL,
        };
        assert_eq!(drawn, expected);
        counts[usize::from(drawn)] += 1;
    }
    assert!(counts[usize::from(gene::WEAK)] > counts[usize::from(gene::LAZY)] * 3);
    assert_eq!(variant_of(gene::BROWN, gene::BROWN), gene::BROWN);
    assert_eq!(variant_of(gene::BROWN, gene::WEAK), gene::NORMAL);
    assert_eq!(variant_of(gene::WEAK, gene::NORMAL), gene::NORMAL);
    assert_eq!(variant_of(gene::LAZY, gene::BROWN), gene::LAZY);
}

#[test]
fn an_egg_s_panda_draws_its_two_genes_and_a_weak_one_has_ten_health_at_most_and_a_lazy_one_is_slow()
{
    let mut weak = false;
    let mut lazy = false;
    for seed in 0..400 {
        let mut mob = Mob::fresh();
        let mut level = draws(seed);
        let mut replay = level.clone();
        of(entity_types::PANDA as u16).finalize_spawn(&mut Spawning {
            reason: crate::mobs::types::SpawnReason::SpawnEgg,
            group: &mut None,
            structure: None,
            moon_brightness: 1.0,
            difficulty: (0.0, 0.0),
            mob: &mut mob,
            level: &mut level,
            own: &mut draws(99),
            biome: None,
            at: BlockPos::new(0, -60, 0),
            day: 0,
            game_time: 0,
        });
        let own = panda::state_of(&mob);
        assert_eq!(own.main_gene, random_gene(&mut replay));
        assert_eq!(own.hidden_gene, random_gene(&mut replay));
        assert_eq!(level, replay);
        let kind = entity_types::PANDA as u16;
        let health = mob
            .body
            .attributes
            .value(kind, clustine_data::attributes::MAX_HEALTH as u16);
        let speed = mob
            .body
            .attributes
            .value(kind, clustine_data::attributes::MOVEMENT_SPEED as u16);
        match own.variant() {
            gene::WEAK => {
                weak = true;
                assert_eq!(health, 10.0);
            }
            gene::LAZY => {
                lazy = true;
                assert_eq!(speed, f64::from(0.07_f32));
            }
            _ => {
                assert_eq!(health, 20.0);
                assert_eq!(speed, f64::from(0.15_f32));
            }
        }
    }
    assert!(weak && lazy);
}

#[test]
fn a_panda_cub_takes_one_gene_of_each_parent_each_by_its_own_draw_and_now_and_then_a_new_one() {
    let parent = |main, hidden| {
        let mut mob = Mob::fresh();
        mob.state = animal::write(
            &Animal::default(),
            &PandaState {
                main_gene: main,
                hidden_gene: hidden,
                ..PandaState::default()
            },
        );
        mob
    };
    let mother = parent(gene::BROWN, gene::LAZY);
    let father = parent(gene::PLAYFUL, gene::WORRIED);
    for seed in 0..200 {
        let mut stream = draws(seed);
        let mut replay = stream.clone();
        let child = of(entity_types::PANDA as u16)
            .breed_offspring(&mut Breeding {
                this: &mother,
                partner: &father,
                partner_kind: entity_types::PANDA as u16,
                child_kind: None,
                draws: &mut stream,
                biome: None,
                egg_user: None,
            })
            .unwrap();
        let one_of = |main, hidden, replay: &mut Random| {
            if replay.next_bool() { main } else { hidden }
        };
        let (mut main, mut hidden) = if replay.next_bool() {
            let main = one_of(gene::BROWN, gene::LAZY, &mut replay);
            (main, one_of(gene::PLAYFUL, gene::WORRIED, &mut replay))
        } else {
            let main = one_of(gene::PLAYFUL, gene::WORRIED, &mut replay);
            (main, one_of(gene::BROWN, gene::LAZY, &mut replay))
        };
        if replay.next_int(32) == 0 {
            main = random_gene(&mut replay);
        }
        if replay.next_int(32) == 0 {
            hidden = random_gene(&mut replay);
        }
        let own = panda::state_of(&child);
        assert_eq!((own.main_gene, own.hidden_gene), (main, hidden), "{seed}");
        assert_eq!(stream, replay);
    }
}

#[test]
fn a_panda_shows_its_counters_genes_and_flags_at_18_to_23() {
    let mut own = PandaState {
        main_gene: gene::AGGRESSIVE,
        hidden_gene: gene::BROWN,
        eat: 4,
        ..PandaState::default()
    };
    own.set(panda::flag_bits::SITTING, true);
    let mut mob = Mob::fresh();
    mob.state = animal::write(&Animal::default(), &own);
    let values = of(entity_types::PANDA as u16).shown(&mob);
    for expected in [
        (18, DataShown::Int(0), true),
        (19, DataShown::Int(0), true),
        (20, DataShown::Int(4), false),
        (21, DataShown::Byte(gene::AGGRESSIVE), false),
        (22, DataShown::Byte(gene::BROWN), false),
        (23, DataShown::Byte(8), false),
    ] {
        assert!(values.contains(&expected), "{expected:?} in {values:?}");
    }
    assert!(!of(entity_types::PANDA as u16).can_be_leashed(entity_types::PANDA as u16, &mob));
}

#[test]
fn anger_lasts_400_to_780_ticks_by_one_draw_and_ends_at_its_end() {
    let mut anger = Anger::default();
    assert!(!anger.is_angry(0));
    for seed in 0..200 {
        let mut random = draws(seed);
        let mut replay = random.clone();
        anger.end = 1000 + polar_bear::anger_time(&mut random);
        assert_eq!(anger.end, 1000 + 400 + i64::from(replay.next_int(381)));
        assert!((1400..=1780).contains(&anger.end));
        assert!(anger.is_angry(anger.end - 1) && !anger.is_angry(anger.end));
        assert_eq!(random, replay);
    }
}

#[test]
fn a_polar_bear_shows_whether_it_stands_and_breeds_a_cub_of_nothing_of_its_own() {
    let mut mob = Mob::fresh();
    let values = of(entity_types::POLAR_BEAR as u16).shown(&mob);
    assert!(values.contains(&(18, DataShown::Boolean(false), true)));
    mob.state = animal::write(
        &Animal::default(),
        &BearState {
            standing: true,
            ..BearState::default()
        },
    );
    let values = of(entity_types::POLAR_BEAR as u16).shown(&mob);
    assert!(values.contains(&(18, DataShown::Boolean(true), false)));
    assert!(polar_bear::state_of(&mob).standing);
    let child = of(entity_types::POLAR_BEAR as u16).breed_offspring(&mut Breeding {
        this: &mob,
        partner: &mob,
        partner_kind: entity_types::POLAR_BEAR as u16,
        child_kind: None,
        draws: &mut draws(1),
        biome: None,
        egg_user: None,
    });
    assert_eq!(child.map(|child| child.state), Some(TypeState::default()));
    // It swims as well as it walks.
    assert_eq!(of(entity_types::POLAR_BEAR as u16).water_slow_down(), 0.98);
}

#[test]
fn a_bee_s_constructor_draws_its_validations_twenty_to_forty_and_its_flower_twenty_to_sixty() {
    for seed in 0..100 {
        let mut mob = Mob::fresh();
        let mut own = draws(seed);
        let mut replay = own.clone();
        of(entity_types::BEE as u16).construct(&mut mob, &mut own);
        let state = bee::state_of(&mob);
        assert_eq!(
            state.validate_hive_cooldown,
            replay.next_int(21) as i32 + 20
        );
        assert_eq!(
            state.validate_flower_cooldown,
            replay.next_int(21) as i32 + 20
        );
        assert_eq!(
            state.flower_cooldown(0),
            i64::from(replay.next_int(41)) + 20
        );
        assert_eq!(own, replay);
        // It falls until its first move.
        assert!(!of(entity_types::BEE as u16).no_gravity(&mob));
    }
}

#[test]
fn bees_stay_in_from_tick_12542_to_23460_of_the_day() {
    assert!(!bees_stay_in_hive(12_541));
    assert!(bees_stay_in_hive(12_542));
    assert!(bees_stay_in_hive(23_459));
    assert!(!bees_stay_in_hive(23_460));
    assert!(bees_stay_in_hive(3 * 24_000 + 18_000));
    assert!(!bees_stay_in_hive(3 * 24_000 + 1_000));
}

#[test]
fn a_bee_is_drawn_to_flowers_but_a_sunflower_only_by_its_top_and_nothing_under_water() {
    for state in [
        blocks::POPPY,
        blocks::DANDELION,
        blocks::CORNFLOWER,
        blocks::FLOWERING_AZALEA,
    ] {
        assert!(attracts_bees(state), "{state:?}");
    }
    for state in [blocks::GRASS_BLOCK, blocks::OAK_LEAVES, blocks::AIR] {
        assert!(!attracts_bees(state));
    }
    let top = clustine_blocks::data::set(blocks::SUNFLOWER, "half", "upper");
    let bottom = clustine_blocks::data::set(blocks::SUNFLOWER, "half", "lower");
    assert!(attracts_bees(top) && !attracts_bees(bottom));
    let wet = clustine_blocks::data::set_flag(blocks::MANGROVE_PROPAGULE, "waterlogged", true);
    assert!(!attracts_bees(wet));
}

#[test]
fn a_bee_s_nectar_grows_crops_stems_bushes_and_vines_by_one_and_ripe_ones_not_at_all() {
    let set = clustine_blocks::data::set;
    let age = |state, age: u8| set(state, "age", &age.to_string());
    assert_eq!(grown(age(blocks::WHEAT, 3)), Some(age(blocks::WHEAT, 4)));
    assert_eq!(grown(age(blocks::WHEAT, 7)), None);
    assert_eq!(
        grown(age(blocks::BEETROOTS, 2)),
        Some(age(blocks::BEETROOTS, 3))
    );
    assert_eq!(grown(age(blocks::BEETROOTS, 3)), None);
    assert_eq!(
        grown(age(blocks::MELON_STEM, 6)),
        Some(age(blocks::MELON_STEM, 7))
    );
    assert_eq!(grown(age(blocks::MELON_STEM, 7)), None);
    assert_eq!(
        grown(age(blocks::SWEET_BERRY_BUSH, 2)),
        Some(age(blocks::SWEET_BERRY_BUSH, 3))
    );
    assert_eq!(grown(age(blocks::SWEET_BERRY_BUSH, 3)), None);
    let bare = clustine_blocks::data::set_flag(blocks::CAVE_VINES, "berries", false);
    assert_eq!(
        grown(bare),
        Some(clustine_blocks::data::set_flag(bare, "berries", true))
    );
    assert_eq!(
        grown(age(blocks::TORCHFLOWER_CROP, 1)),
        Some(blocks::TORCHFLOWER)
    );
    // The pitcher crop is of the tag but no crop block's: the bee leaves it.
    assert_eq!(grown(blocks::PITCHER_CROP), None);
    assert_eq!(grown(blocks::GRASS_BLOCK), None);
}

#[test]
fn a_bee_shows_its_gravity_its_flags_and_its_anger_s_end_in_game_time() {
    let mut mob = Mob::fresh();
    let values = of(entity_types::BEE as u16).shown(&mob);
    assert!(values.contains(&(5, DataShown::Boolean(false), true)));
    assert!(values.contains(&(18, DataShown::Byte(0), true)));
    assert!(values.contains(&(19, DataShown::Long(-1), true)));
    let mut own = BeeState {
        no_gravity: true,
        anger: Anger {
            end: 900,
            target: None,
        },
        ..BeeState::default()
    };
    own.set_has_nectar(true, 0);
    mob.state = animal::write(&Animal::default(), &own);
    let values = of(entity_types::BEE as u16).shown(&mob);
    assert!(values.contains(&(5, DataShown::Boolean(true), false)));
    assert!(values.contains(&(18, DataShown::Byte(8), false)));
    assert!(values.contains(&(19, DataShown::Long(900), false)));
    assert!(of(entity_types::BEE as u16).omnidirectional_air());
}

#[test]
fn a_hive_s_data_counts_its_bees_and_reads_back_what_it_wrote() {
    use crate::region::hives::{Hive, occupant_count};
    // As generation writes a nest of two (`clustine_features`' `beehive`).
    let mut data = vec![10, 9, 0, 4];
    data.extend_from_slice(b"bees");
    data.push(10);
    data.extend_from_slice(&2_i32.to_be_bytes());
    for ticks in [5_i32, 598] {
        data.push(3);
        data.extend_from_slice(&13_u16.to_be_bytes());
        data.extend_from_slice(b"ticks_in_hive");
        data.extend_from_slice(&ticks.to_be_bytes());
        data.push(10);
        data.extend_from_slice(&11_u16.to_be_bytes());
        data.extend_from_slice(b"entity_data");
        data.push(8);
        data.extend_from_slice(&2_u16.to_be_bytes());
        data.extend_from_slice(b"id");
        data.extend_from_slice(&13_u16.to_be_bytes());
        data.extend_from_slice(b"minecraft:bee");
        data.push(0);
        data.push(3);
        data.extend_from_slice(&17_u16.to_be_bytes());
        data.extend_from_slice(b"min_ticks_in_hive");
        data.extend_from_slice(&600_i32.to_be_bytes());
        data.push(0);
    }
    data.push(10);
    data.extend_from_slice(&10_u16.to_be_bytes());
    data.extend_from_slice(b"components");
    data.push(0);
    data.push(0);
    assert_eq!(occupant_count(&data), 2);
    let hive = Hive::read(&data).expect("a nest's data");
    assert_eq!(hive.bees.len(), 2);
    assert_eq!(hive.bees[1].ticks_in_hive, 598);
    assert_eq!(hive.bees[1].min_ticks, 600);
    assert_eq!(hive.flower, None);
    // Written back in the game's order of a compound's entries (by their names'
    // buckets: `components` before `bees`), and read as it was.
    let written = hive.write();
    assert_eq!(written.len(), data.len());
    assert_eq!(Hive::read(&written), Some(hive.clone()));
    let mut flowered = hive.clone();
    flowered.flower = Some(BlockPos::new(3, -60, -2));
    flowered.bees.truncate(1);
    let again = Hive::read(&flowered.write()).unwrap();
    assert_eq!(again, flowered);
    assert_eq!(occupant_count(&flowered.write()), 1);
    // A hive placed and never saved is an empty one.
    assert_eq!(Hive::read(&[]).map(|hive| hive.bees.len()), Some(0));
    assert_eq!(occupant_count(&[]), 0);
}

#[test]
fn a_new_bat_rests_shows_its_rest_at_16_and_is_pushed_by_nothing() {
    let mut mob = Mob::fresh();
    of(entity_types::BAT as u16).construct(&mut mob, &mut draws(1));
    assert_eq!(
        bat::state_of(&mob),
        BatState {
            resting: true,
            target: None
        }
    );
    let values = of(entity_types::BAT as u16).shown(&mob);
    assert_eq!(values, [(16, DataShown::Byte(1), false)]);
    let mut flying = bat::state_of(&mob);
    flying.resting = false;
    flying.target = Some(BlockPos::new(1, 2, 3));
    bat::put_state(&mut mob, &flying);
    assert_eq!(bat::state_of(&mob), flying);
    assert!(!of(entity_types::BAT as u16).pushable());
    assert_eq!(of(entity_types::BAT as u16).sound_volume(), 0.1);
    assert_eq!(of(entity_types::BAT as u16).voice_pitch_factor(), 0.95);
}
