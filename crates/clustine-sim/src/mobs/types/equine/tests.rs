//! The horse family's rules on their state alone, from the brief's sections 5.9 to 5.18:
//! the spawn's coats and stats in the game's order of draws, feeding by the table, temper,
//! the stats a foal inherits and the mule, the saddle, armour, carpet and chest put on by
//! a click, the menu's slots, what clients are shown, and what the riding reads.

use clustine_data::{attributes as ids, entity_types, items, sounds};
use clustine_world::{EntityId, ItemStack};

use super::super::animal::{self, Animal, BABY_START_AGE};
use super::super::{Breeding, Did, Interaction, Spawning, Used, horse, of, zombie_horse};
use super::{HorseState, riding};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::region::Random;
use crate::shown::DataShown;

const HORSE: u16 = entity_types::HORSE as u16;
const DONKEY: u16 = entity_types::DONKEY as u16;
const MULE: u16 = entity_types::MULE as u16;
const LLAMA: u16 = entity_types::LLAMA as u16;
const TRADER_LLAMA: u16 = entity_types::TRADER_LLAMA as u16;
const SKELETON_HORSE: u16 = entity_types::SKELETON_HORSE as u16;
const ZOMBIE_HORSE: u16 = entity_types::ZOMBIE_HORSE as u16;

const FAMILY: [u16; 7] = [
    HORSE,
    DONKEY,
    MULE,
    LLAMA,
    TRADER_LLAMA,
    SKELETON_HORSE,
    ZOMBIE_HORSE,
];

fn draws(seed: u64) -> Random {
    Random::of_entity(seed, EntityId(5), 3)
}

fn base(mob: &Mob, kind: u16, id: i32) -> f64 {
    mob.body.attributes.value(kind, attribute(id))
}

/// A mob of the type `kind` at its type's full health, as an egg's body makes it.
fn fresh(kind: u16) -> Mob {
    let mut mob = Mob::fresh();
    mob.body.vitals.health =
        crate::mobs::body::attributes::supplied(kind, attribute(ids::MAX_HEALTH)).unwrap() as f32;
    mob
}

fn with(shared: &Animal, horse: &HorseState) -> Mob {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 20.0;
    mob.state = animal::write(shared, horse);
    mob
}

fn spawned(kind: u16, seed: u64) -> (Mob, Random) {
    let mut mob = fresh(kind);
    let mut level = draws(seed);
    let replay = level.clone();
    let mut own = draws(seed.wrapping_add(1));
    of(kind).finalize_spawn(&mut Spawning {
        mob: &mut mob,
        level: &mut level,
        own: &mut own,
        biome: Some("minecraft:plains"),
        at: clustine_world::BlockPos::new(0, -60, 0),
        day: 0,
        game_time: 0,
    });
    (mob, replay)
}

#[test]
fn every_kind_of_the_family_is_built_so_that_its_egg_makes_it() {
    for kind in FAMILY {
        assert!(super::super::is_built(kind), "{kind}");
        assert!(super::is_equine(kind));
    }
    assert!(!super::is_equine(entity_types::CAMEL as u16));
}

#[test]
fn a_horse_draws_its_coat_its_markings_then_health_speed_and_jump_from_the_level() {
    for seed in 0..50 {
        let (mob, mut replay) = spawned(HORSE, seed);
        let coat = replay.next_int(7) as i32;
        let markings = replay.next_int(5) as i32;
        let health = 15.0_f32 + replay.next_int(8) as f32 + replay.next_int(9) as f32;
        let mut speed = 0.449_999_988_079_071_04;
        for _ in 0..3 {
            speed += replay.next_double() * 0.3;
        }
        speed *= 0.25;
        let mut jump = 0.400_000_005_960_464_5;
        for _ in 0..3 {
            jump += replay.next_double() * 0.2;
        }
        let state = super::horse_of(&mob);
        assert_eq!(state.variant, horse::type_variant(coat, markings));
        assert_eq!(horse::coat_and_markings(state.variant), (coat, markings));
        assert_eq!(base(&mob, HORSE, ids::MAX_HEALTH), f64::from(health));
        assert_eq!(
            base(&mob, HORSE, ids::MOVEMENT_SPEED).to_bits(),
            speed.to_bits()
        );
        assert_eq!(
            base(&mob, HORSE, ids::JUMP_STRENGTH).to_bits(),
            jump.to_bits()
        );
        assert!((15.0..=30.0).contains(&health));
        assert!((0.1125..=0.3375).contains(&speed));
        assert!((0.4..=1.0).contains(&jump));
        // Made at the type's 53, its health is its new most.
        assert_eq!(mob.body.vitals.health, health);
        // Wild, of no temper, an adult.
        assert!(!state.tame && state.temper == 0);
        assert!(!animal::is_baby(&mob));
    }
}

#[test]
fn a_donkey_or_mule_draws_its_health_alone_and_keeps_the_chested_speed_and_jump() {
    for kind in [DONKEY, MULE] {
        let (mob, mut replay) = spawned(kind, 9);
        let health = 15.0_f32 + replay.next_int(8) as f32 + replay.next_int(9) as f32;
        assert_eq!(base(&mob, kind, ids::MAX_HEALTH), f64::from(health));
        assert_eq!(
            base(&mob, kind, ids::MOVEMENT_SPEED),
            0.174_999_997_019_767_76
        );
        assert_eq!(base(&mob, kind, ids::JUMP_STRENGTH), 0.5);
        assert!(!super::horse_of(&mob).chest);
    }
}

#[test]
fn a_llama_draws_its_strength_its_coat_then_its_health() {
    for kind in [LLAMA, TRADER_LLAMA] {
        for seed in 0..40 {
            let (mob, mut replay) = spawned(kind, seed);
            let most = if replay.next_float() < 0.04 { 5 } else { 3 };
            let strength = 1 + replay.next_int(most) as i32;
            let coat = replay.next_int(4) as i32;
            let health = 15.0_f32 + replay.next_int(8) as f32 + replay.next_int(9) as f32;
            let state = super::horse_of(&mob);
            assert_eq!((state.strength, state.llama_variant), (strength, coat));
            assert_eq!(base(&mob, kind, ids::MAX_HEALTH), f64::from(health));
        }
    }
}

#[test]
fn the_undead_horses_draw_their_jump_and_a_zombie_horse_its_speed_by_their_own_formulas() {
    let (mob, mut replay) = spawned(SKELETON_HORSE, 4);
    let mut jump = 0.400_000_005_960_464_5;
    for _ in 0..3 {
        jump += replay.next_double() * 0.2;
    }
    assert_eq!(
        base(&mob, SKELETON_HORSE, ids::JUMP_STRENGTH).to_bits(),
        jump.to_bits()
    );
    assert_eq!(mob.body.vitals.health, 15.0);
    let (mob, mut replay) = spawned(ZOMBIE_HORSE, 4);
    let jump = zombie_horse::generate_jump_strength(&mut replay);
    let speed = zombie_horse::generate_speed(&mut replay);
    assert_eq!(base(&mob, ZOMBIE_HORSE, ids::JUMP_STRENGTH), jump);
    assert_eq!(base(&mob, ZOMBIE_HORSE, ids::MOVEMENT_SPEED), speed);
    assert!((0.5..=0.7).contains(&jump));
    assert!((9.0 / 42.16..=12.0 / 42.16).contains(&speed));
    assert_eq!(mob.body.vitals.health, 25.0);
}

/// A use of `mob` of the type `kind` by player 9 holding `held`.
fn use_with<'a>(
    kind: u16,
    mob: &'a mut Mob,
    held: Option<ItemStack>,
    sneaking: bool,
    stream: &'a mut Random,
) -> Used<'a> {
    Used {
        key: EntityId(5),
        kind,
        mob,
        player: EntityId(9),
        held,
        creative: true,
        sneaking,
        carries: false,
        passengers: 0,
        player_uuid: None,
        game_time: 0,
        on_ground: true,
        from: [0.0, 0.0, 0.0],
        position: [1.0, 0.0, 1.0],
        moment: 100,
        draws: stream,
        did: Vec::new(),
    }
}

fn one(item: i32) -> Option<ItemStack> {
    Some(ItemStack { item, count: 1 })
}

fn interact(
    kind: u16,
    mob: &mut Mob,
    held: Option<ItemStack>,
    sneaking: bool,
) -> (Interaction, Vec<Did>) {
    let mut stream = draws(1);
    let mut used = use_with(kind, mob, held, sneaking, &mut stream);
    let result = of(kind).mob_interact(&mut used);
    (result, used.did)
}

fn sounds_of(did: &[Did]) -> Vec<i32> {
    did.iter()
        .filter_map(|done| match done {
            Did::Sound { sound, .. } => Some(*sound),
            _ => None,
        })
        .collect()
}

#[test]
fn feeding_a_wild_horse_raises_its_temper_by_the_table_and_heals_it() {
    let mut mob = with(&Animal::default(), &HorseState::default());
    mob.body.vitals.health = 10.0;
    let (result, did) = interact(HORSE, &mut mob, one(items::WHEAT), false);
    assert_eq!(result, Interaction::SuccessServer);
    let state = super::horse_of(&mob);
    assert_eq!(state.temper, 3);
    assert_eq!(mob.body.vitals.health, 12.0);
    // It ate: the mouth opens, the eat sound, `GameEvent.EAT`.
    assert!(state.mouth && state.mouth_until == 130);
    assert_eq!(sounds_of(&did), [sounds::ENTITY_HORSE_EAT]);
    assert!(did.contains(&Did::GameEvent("minecraft:eat")));
    // A golden apple adds ten, up to the most.
    for _ in 0..12 {
        interact(HORSE, &mut mob, one(items::GOLDEN_APPLE), false);
    }
    assert_eq!(super::horse_of(&mob).temper, 100);
    // A hay block adds no temper: a tamed horse at its full health refuses it, and
    // keeps it.
    let mut full = with(&Animal::default(), &tamed());
    full.body.vitals.health = 53.0;
    let (result, did) = interact(HORSE, &mut full, one(items::HAY_BLOCK), false);
    assert_eq!(result, Interaction::Pass);
    assert!(did.is_empty());
    full.body.vitals.health = 40.0;
    let (result, _) = interact(HORSE, &mut full, one(items::HAY_BLOCK), false);
    assert_eq!(result, Interaction::SuccessServer);
    assert_eq!(full.body.vitals.health, 53.0);
    assert_eq!(super::horse_of(&full).temper, 0);
    // A llama's temper goes to 30 at most.
    let mut wild = with(&Animal::default(), &HorseState::default());
    for _ in 0..20 {
        interact(LLAMA, &mut wild, one(items::HAY_BLOCK), false);
    }
    assert_eq!(super::horse_of(&wild).temper, 30);
}

#[test]
fn a_golden_carrot_puts_a_tamed_adult_in_love_and_ages_a_foal_by_its_seconds() {
    let mut adult = with(&Animal::default(), &tamed());
    let (result, did) = interact(HORSE, &mut adult, one(items::GOLDEN_CARROT), false);
    assert_eq!(result, Interaction::SuccessServer);
    assert!(animal::animal_of(&adult.state).in_love(100));
    assert!(did.contains(&Did::Event(18)));
    // A wild horse is not put in love.
    let mut wild = with(&Animal::default(), &HorseState::default());
    interact(HORSE, &mut wild, one(items::GOLDEN_CARROT), false);
    assert!(!animal::animal_of(&wild.state).in_love(100));
    // A foal fed an apple is sixty seconds older, owing nothing back.
    let mut young = Animal::default();
    young.set_age(BABY_START_AGE, 100);
    let mut foal = with(&young, &HorseState::default());
    interact(HORSE, &mut foal, one(items::APPLE), false);
    let shared = animal::animal_of(&foal.state);
    assert_eq!(shared.age(100), BABY_START_AGE + 1200);
    assert_eq!(shared.forced_age, 0);
}

#[test]
fn anything_but_its_food_makes_a_wild_horse_rear_angrily() {
    let mut mob = with(&Animal::default(), &HorseState::default());
    let (result, did) = interact(HORSE, &mut mob, one(items::SADDLE), false);
    assert_eq!(result, Interaction::Success);
    let state = super::horse_of(&mob);
    assert!(state.standing && state.stand_until == 120);
    assert_eq!(sounds_of(&did), [sounds::ENTITY_HORSE_ANGRY]);
    assert!(mob.body.equipment.slots[usize::from(slots::SADDLE)].is_none());
    // A llama gets angry but does not rear.
    let mut llama = with(&Animal::default(), &HorseState::default());
    let (_, did) = interact(LLAMA, &mut llama, one(items::DIRT), false);
    assert!(!super::horse_of(&llama).standing);
    assert_eq!(sounds_of(&did), [sounds::ENTITY_LLAMA_ANGRY]);
    // A wild skeleton horse passes.
    let mut skeleton = with(&Animal::default(), &HorseState::default());
    assert_eq!(
        interact(SKELETON_HORSE, &mut skeleton, one(items::DIRT), false).0,
        Interaction::Pass
    );
}

fn tamed() -> HorseState {
    HorseState {
        tame: true,
        ..HorseState::default()
    }
}

#[test]
fn a_saddle_and_armour_go_on_a_tamed_horse_by_a_click_each_kept_whatever_kills_it() {
    let mut mob = with(&Animal::default(), &tamed());
    let (result, did) = interact(HORSE, &mut mob, one(items::SADDLE), false);
    assert_eq!(result, Interaction::Success);
    assert_eq!(mob.body.equipment.slots[7], one(items::SADDLE));
    assert_eq!(mob.body.equipment.drop_chance(7), 2.0);
    assert_eq!(sounds_of(&did), [sounds::ENTITY_HORSE_SADDLE]);
    assert!(did.contains(&Did::GameEvent("minecraft:equip")));
    assert!(super::is_saddled(&mob));
    // A second saddle goes nowhere: the player gets on (the riding's).
    let (_, did) = interact(HORSE, &mut mob, one(items::SADDLE), false);
    assert_eq!(did, [Did::Ride]);
    let (result, did) = interact(HORSE, &mut mob, one(items::IRON_HORSE_ARMOR), false);
    assert_eq!(result, Interaction::Success);
    assert_eq!(mob.body.equipment.slots[6], one(items::IRON_HORSE_ARMOR));
    assert_eq!(mob.body.equipment.drop_chance(6), 2.0);
    assert!(did.contains(&Did::GameEvent("minecraft:equip")));
    // A donkey takes a saddle and no horse armour; a llama a carpet and no saddle.
    let mut donkey = with(&Animal::default(), &tamed());
    interact(DONKEY, &mut donkey, one(items::SADDLE), false);
    interact(DONKEY, &mut donkey, one(items::IRON_HORSE_ARMOR), false);
    assert_eq!(donkey.body.equipment.slots[7], one(items::SADDLE));
    assert_eq!(donkey.body.equipment.slots[6], None);
    let mut llama = with(&Animal::default(), &tamed());
    interact(LLAMA, &mut llama, one(items::SADDLE), false);
    let (_, did) = interact(LLAMA, &mut llama, one(items::RED_CARPET), false);
    assert_eq!(llama.body.equipment.slots[7], None);
    assert_eq!(llama.body.equipment.slots[6], one(items::RED_CARPET));
    assert_eq!(sounds_of(&did), [sounds::ENTITY_LLAMA_SWAG]);
    // A sneaking player opens a tamed one's inventory; an empty hand gets on.
    assert_eq!(
        interact(HORSE, &mut mob, None, true).1,
        [Did::OpenInventory]
    );
    assert_eq!(interact(HORSE, &mut mob, None, false).1, [Did::Ride]);
}

#[test]
fn a_chest_goes_on_a_tamed_donkey_and_its_menu_shows_the_saddle_the_body_and_fifteen_slots() {
    let mut donkey = with(&Animal::default(), &tamed());
    let (result, did) = interact(DONKEY, &mut donkey, one(items::CHEST), false);
    assert_eq!(result, Interaction::Success);
    assert!(super::horse_of(&donkey).chest);
    assert_eq!(sounds_of(&did), [sounds::ENTITY_DONKEY_CHEST]);
    assert_eq!(super::inventory_columns(DONKEY, &donkey), 5);
    assert_eq!(super::inventory_size(DONKEY, &donkey), 15);
    // A second chest goes nowhere: the player gets on.
    assert_eq!(
        interact(DONKEY, &mut donkey, one(items::CHEST), false).1,
        [Did::Ride]
    );
    let mut slots = super::menu_container(DONKEY, &donkey);
    assert_eq!(slots.len(), 17);
    slots[0] = one(items::SADDLE);
    slots[4] = Some(ItemStack {
        item: items::DIRT,
        count: 32,
    });
    let changed = super::write_menu_container(&mut donkey, &slots);
    assert_eq!(changed, [(slots::SADDLE, None, one(items::SADDLE))]);
    assert_eq!(super::menu_container(DONKEY, &donkey), slots);
    // Put on in the menu, it drops whatever kills it, and the donkey is kept.
    assert_eq!(donkey.body.equipment.drop_chance(7), 2.0);
    assert!(donkey.body.flags.persistent);
    // As it dies: its inventory, then its chest, which it no longer has.
    let drops = of(DONKEY).drop_equipment(&mut donkey);
    assert_eq!(
        drops,
        [
            ItemStack {
                item: items::DIRT,
                count: 32
            },
            ItemStack {
                item: items::CHEST,
                count: 1
            }
        ]
    );
    assert!(!super::horse_of(&donkey).chest);
    // A wild donkey offered a chest gets angry; a horse takes none.
    let mut wild = with(&Animal::default(), &HorseState::default());
    interact(DONKEY, &mut wild, one(items::CHEST), false);
    assert!(!super::horse_of(&wild).chest && super::horse_of(&wild).standing);
    let mut horse = with(&Animal::default(), &tamed());
    interact(HORSE, &mut horse, one(items::CHEST), false);
    assert!(!super::horse_of(&horse).chest);
    // A llama's chest has its strength's columns.
    let mut llama = with(
        &Animal::default(),
        &HorseState {
            strength: 3,
            ..tamed()
        },
    );
    interact(LLAMA, &mut llama, one(items::CHEST), false);
    assert_eq!(super::inventory_columns(LLAMA, &llama), 3);
}

#[test]
fn a_foal_s_stat_is_about_its_parents_mean_by_three_draws_and_reflected_at_the_bounds() {
    let mut r = draws(3);
    let mut replay = r.clone();
    let value = super::offspring_attribute(20.0, 24.0, 15.0, 30.0, &mut r);
    let spread = 4.0 + 0.15 * 15.0 * 2.0;
    let roll = (replay.next_double() + replay.next_double() + replay.next_double()) / 3.0 - 0.5;
    assert_eq!(value, 22.0 + spread * roll);
    // Beyond a bound it comes back by as much.
    for seed in 0..200 {
        let mut r = draws(seed);
        let value = super::offspring_attribute(30.0, 30.0, 15.0, 30.0, &mut r);
        assert!((15.0..=30.0).contains(&value), "{value}");
        let mut r = draws(seed);
        let value = super::offspring_attribute(0.1, 0.0, 0.112_5, 0.337_5, &mut r);
        assert!((0.112_5..=0.337_5).contains(&value), "{value}");
    }
}

fn parent(kind: u16, health: f64, jump: f64, speed: f64, horse: HorseState) -> Mob {
    let mut mob = with(&Animal::default(), &horse);
    super::set_base(&mut mob, kind, ids::MAX_HEALTH, health);
    super::set_base(&mut mob, kind, ids::JUMP_STRENGTH, jump);
    super::set_base(&mut mob, kind, ids::MOVEMENT_SPEED, speed);
    mob
}

fn breed(this: (&Mob, u16), partner: (&Mob, u16), r: &mut Random) -> (Option<Mob>, Option<u16>) {
    let mut breeding = Breeding {
        this: this.0,
        partner: partner.0,
        partner_kind: partner.1,
        child_kind: None,
        draws: r,
        biome: None,
        egg_user: None,
    };
    let child = of(this.1).breed_offspring(&mut breeding);
    (child, breeding.child_kind)
}

/// The three stats the breeding parent's random gives a foal of `kind`, in the game's
/// order: health, jump, speed.
fn expected_stats(
    this: &Mob,
    this_kind: u16,
    partner: &Mob,
    partner_kind: u16,
    r: &mut Random,
) -> [f64; 3] {
    let bounds = [
        (ids::MAX_HEALTH, 15.0, 30.0),
        (ids::JUMP_STRENGTH, f64::from(0.4_f32), 1.0),
        (
            ids::MOVEMENT_SPEED,
            f64::from(0.1125_f32),
            f64::from(0.3375_f32),
        ),
    ];
    bounds.map(|(id, min, max)| {
        super::offspring_attribute(
            base(this, this_kind, id),
            base(partner, partner_kind, id),
            min,
            max,
            r,
        )
    })
}

#[test]
fn two_horses_make_a_foal_of_their_coats_and_markings_then_their_stats() {
    let mare = parent(
        HORSE,
        20.0,
        0.6,
        0.2,
        HorseState {
            variant: horse::type_variant(2, 1),
            ..tamed()
        },
    );
    let stallion = parent(
        HORSE,
        28.0,
        0.9,
        0.3,
        HorseState {
            variant: horse::type_variant(5, 3),
            ..tamed()
        },
    );
    for seed in 0..60 {
        let mut r = draws(seed);
        let mut replay = r.clone();
        let (child, kind) = breed((&mare, HORSE), (&stallion, HORSE), &mut r);
        let child = child.unwrap();
        assert_eq!(kind, None);
        let pick = replay.next_int(9);
        let coat = if pick < 4 {
            2
        } else if pick < 8 {
            5
        } else {
            replay.next_int(7) as i32
        };
        let pick = replay.next_int(5);
        let markings = if pick < 2 {
            1
        } else if pick < 4 {
            3
        } else {
            replay.next_int(5) as i32
        };
        assert_eq!(
            super::horse_of(&child).variant,
            horse::type_variant(coat, markings)
        );
        let [health, jump, speed] = expected_stats(&mare, HORSE, &stallion, HORSE, &mut replay);
        assert_eq!(base(&child, HORSE, ids::MAX_HEALTH), health);
        assert_eq!(base(&child, HORSE, ids::JUMP_STRENGTH), jump);
        assert_eq!(base(&child, HORSE, ids::MOVEMENT_SPEED), speed);
        // A foal is not tamed.
        assert!(!super::horse_of(&child).tame);
        assert_eq!(r, replay);
    }
}

#[test]
fn a_horse_and_a_donkey_make_a_mule_of_their_stats_alone_whichever_breeds() {
    let mare = parent(HORSE, 20.0, 0.6, 0.2, tamed());
    let jack = parent(DONKEY, 25.0, 0.5, 0.175, tamed());
    for (this, partner) in [
        ((&mare, HORSE), (&jack, DONKEY)),
        ((&jack, DONKEY), (&mare, HORSE)),
    ] {
        let mut r = draws(8);
        let mut replay = r.clone();
        let (child, kind) = breed(this, partner, &mut r);
        let child = child.unwrap();
        assert_eq!(kind, Some(MULE));
        let [health, jump, speed] =
            expected_stats(this.0, this.1, partner.0, partner.1, &mut replay);
        assert_eq!(base(&child, MULE, ids::MAX_HEALTH), health);
        assert_eq!(base(&child, MULE, ids::JUMP_STRENGTH), jump);
        assert_eq!(base(&child, MULE, ids::MOVEMENT_SPEED), speed);
        assert_eq!(super::horse_of(&child).variant, 0);
        assert_eq!(r, replay);
    }
    // Two donkeys make a donkey.
    let mut r = draws(1);
    assert_eq!(
        breed((&jack, DONKEY), (&jack, DONKEY), &mut r).1,
        Some(DONKEY)
    );
}

#[test]
fn a_llama_foal_takes_its_stats_then_a_strength_up_to_the_stronger_s_then_a_coat() {
    let a = parent(
        LLAMA,
        20.0,
        0.5,
        0.175,
        HorseState {
            strength: 2,
            llama_variant: 1,
            ..tamed()
        },
    );
    let b = parent(
        LLAMA,
        22.0,
        0.5,
        0.175,
        HorseState {
            strength: 4,
            llama_variant: 3,
            ..tamed()
        },
    );
    for seed in 0..40 {
        let mut r = draws(seed);
        let mut replay = r.clone();
        let child = breed((&a, LLAMA), (&b, LLAMA), &mut r).0.unwrap();
        let [health, ..] = expected_stats(&a, LLAMA, &b, LLAMA, &mut replay);
        let mut strength = replay.next_int(4) as i32 + 1;
        if replay.next_float() < 0.03 {
            strength += 1;
        }
        let coat = if replay.next_bool() { 1 } else { 3 };
        let state = super::horse_of(&child);
        assert_eq!(
            (state.strength, state.llama_variant),
            (strength.min(5), coat)
        );
        assert_eq!(base(&child, LLAMA, ids::MAX_HEALTH), health);
    }
    // A trader llama's foal is a trader llama, kept.
    let mut r = draws(2);
    let child = breed((&a, TRADER_LLAMA), (&b, LLAMA), &mut r).0.unwrap();
    assert!(child.body.flags.persistent);
}

#[test]
fn each_of_the_family_shows_its_flags_at_18_and_its_own_values_after() {
    let horse = HorseState {
        tame: true,
        standing: true,
        variant: horse::type_variant(3, 2),
        ..HorseState::default()
    };
    let shown = of(HORSE).shown(&with(&Animal::default(), &horse));
    assert_eq!(
        shown,
        [
            (16, DataShown::Boolean(false), true),
            (17, DataShown::Boolean(false), true),
            (18, DataShown::Byte(2 | 32), false),
            (19, DataShown::Int(3 | (2 << 8)), false),
        ]
    );
    let donkey = HorseState {
        chest: true,
        eating: true,
        ..HorseState::default()
    };
    let shown = of(DONKEY).shown(&with(&Animal::default(), &donkey));
    assert_eq!(shown[2], (18, DataShown::Byte(16), false));
    assert_eq!(shown[3], (19, DataShown::Boolean(true), false));
    let llama = HorseState {
        strength: 4,
        llama_variant: 2,
        mouth: true,
        ..HorseState::default()
    };
    let shown = of(LLAMA).shown(&with(&Animal::default(), &llama));
    assert_eq!(shown.len(), 6);
    assert_eq!(shown[2], (18, DataShown::Byte(64), false));
    assert_eq!(shown[3], (19, DataShown::Boolean(false), true));
    assert_eq!(shown[4], (20, DataShown::Int(4), false));
    assert_eq!(shown[5], (21, DataShown::Int(2), false));
    for kind in [SKELETON_HORSE, ZOMBIE_HORSE] {
        let shown = of(kind).shown(&with(&Animal::default(), &HorseState::default()));
        assert_eq!(shown.len(), 3);
        assert!(shown.iter().all(|(_, _, default)| *default));
    }
}

#[test]
fn a_foal_is_of_its_kind_s_baby_size() {
    let mut young = Animal::default();
    young.set_age(BABY_START_AGE, 0);
    let foal = with(&young, &HorseState::default());
    for (kind, scale) in [
        (HORSE, 0.7),
        (DONKEY, 0.5),
        (LLAMA, 0.5),
        (ZOMBIE_HORSE, 0.7),
    ] {
        let info = clustine_data::EntityTypeInfo::of(i32::from(kind));
        let size = of(kind).dimensions(&foal).unwrap();
        assert_eq!(size.width, info.width * scale);
        assert_eq!(size.height, info.height * scale);
        assert_eq!(size.eye_height, info.eye_height * scale);
        assert_eq!(of(kind).dimensions(&Mob::fresh()), None);
    }
}

#[test]
fn rearing_ends_after_20_ticks_eating_after_51_and_the_mouth_closes_after_30() {
    let mut state = HorseState::default();
    state.set_eating(true, 10);
    assert_eq!(state.eating_counter(10), 0);
    assert_eq!(state.eating_counter(30), 20);
    // Rearing stops the eating, its count kept.
    state.set_standing(30);
    assert!(!state.eating && state.standing);
    assert_eq!(state.eaten, 20);
    assert_eq!(state.stand_until, 50);
    assert_eq!(state.flags(), 32);
    state.clear_standing();
    assert_eq!(state.flags(), 0);
    state.tame_with_name(77);
    assert_eq!(state.owner, Some(77));
    assert_eq!(state.flags(), 2);
    assert_eq!(state.modify_temper(-5, 100), 0);
    assert_eq!(state.modify_temper(500, 30), 30);
}

#[test]
fn the_riding_reads_whether_it_is_saddled_its_stats_its_seat_and_where_a_rider_gets_off() {
    let mut mob = with(&Animal::default(), &tamed());
    assert!(!riding::steered_by_rider(&mob) && !riding::can_jump(&mob));
    mob.body.equipment.slots[7] = one(items::SADDLE);
    assert!(riding::steered_by_rider(&mob) && riding::can_jump(&mob));
    assert!(super::is_tamed(&mob));
    super::set_base(&mut mob, HORSE, ids::JUMP_STRENGTH, 0.8);
    super::set_base(&mut mob, HORSE, ids::MOVEMENT_SPEED, 0.3);
    assert_eq!(riding::jump_strength(HORSE, &mob), 0.8);
    assert_eq!(riding::speed(HORSE, &mob), 0.3);
    // At rest the seat is the type's; rearing moves it up and back, turned by the yaw.
    assert_eq!(riding::passenger_offset(HORSE, &mob, 1.0, 0.0), [0.0; 3]);
    riding::handle_start_jump(HORSE, &mut mob, 40);
    let state = super::horse_of(&mob);
    assert!(state.standing && state.allow_stand_sliding);
    super::update(&mut mob, |horse| horse.stand_anim_o = 1.0);
    let offset = riding::passenger_offset(HORSE, &mob, 1.0, 0.0);
    assert!((offset[1] - 0.15).abs() < 1e-9 && (offset[2] + 0.7).abs() < 1e-6);
    assert_eq!(riding::passenger_offset(LLAMA, &mob, 1.0, 0.0), [0.0; 3]);
    // A right-handed rider gets off on the horse's right first (`yRot + 90`).
    let [first, second] = riding::dismount_directions(1.396_484_4, 0.6, 0.0, true);
    assert!(first[0] < 0.0 && second[0] > 0.0);
    assert!(first[2].abs() < 1e-6);
}
