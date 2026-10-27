//! The villager and the wandering trader as their mobs (part K2), each number from the
//! game's rules (`docs/groundwork/n7k-brief.md` section 2, the plan's section 10 and the
//! dumps): the level's draws of each `finalizeSpawn` by a counting random, a villager's
//! type by the biome and kept from a template, the brain's construction's draws, the
//! schedule's activities, the eight slots, and what clients are shown.

use std::collections::VecDeque;

use clustine_data::villagers::{professions, types as kinds};
use clustine_data::{entity_types, items};
use clustine_world::{BlockPos, EntityId, ItemStack};

use super::template::build::{self as nbt};
use super::villager::{self, Data, SLOTS, VillagerState};
use super::{Breeding, SpawnReason, Spawning, animal, wandering_trader};
use crate::mobs::Mob;
use crate::mobs::brain::{Activity, BrainState};
use crate::region::Random;
use crate::shown::DataShown;

/// One call of the level's random.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Call {
    Int(u32),
    Float,
    Double,
    Bool,
}

use Call::{Double, Float};

/// A level's random that records every call and answers a middling value (or the
/// doubles it is given).
#[derive(Default)]
struct Counting {
    calls: Vec<Call>,
    doubles: VecDeque<f64>,
}

impl clustine_world::generation::Draws for Counting {
    fn next_int(&mut self, bound: u32) -> u32 {
        self.calls.push(Call::Int(bound));
        0
    }

    fn next_float(&mut self) -> f32 {
        self.calls.push(Float);
        0.5
    }

    fn next_double(&mut self) -> f64 {
        self.calls.push(Double);
        self.doubles.pop_front().unwrap_or(0.5)
    }

    fn next_bool(&mut self) -> bool {
        self.calls.push(Call::Bool);
        false
    }
}

/// `Mob.finalizeSpawn`'s draws without the follow range's bonus: the triangle's two
/// doubles, then the hand.
const MOB: [Call; 3] = [Double, Double, Float];

/// A mob of `kind` made for `reason` in `biome` (its tag read first where `tag` is
/// one), and the level's draws of its `finalizeSpawn`.
fn made(
    kind: i32,
    reason: SpawnReason,
    biome: Option<&'static str>,
    tag: Option<&[u8]>,
) -> (Mob, Vec<Call>) {
    let kind = kind as u16;
    let mut own = Random::of_generated(7, EntityId(1 << 63));
    let (_, mut mob) = crate::mobs::making::construct(kind, &mut own, 0.0);
    if let Some(tag) = tag {
        super::from_template(kind, &mut mob, tag);
    }
    let mut level = Counting::default();
    crate::mobs::making::finalize(
        kind,
        &mut Spawning {
            mob: &mut mob,
            level: &mut level,
            own: &mut own,
            biome,
            at: BlockPos::new(0, 64, 0),
            day: 0,
            game_time: 0,
            reason,
            group: &mut None,
            structure: None,
            moon_brightness: 1.0,
            difficulty: (0.0, 0.0),
            companions: None,
        },
    );
    (mob, level.calls)
}

fn own(mob: &Mob) -> VillagerState {
    villager::state_of(mob).1
}

#[test]
fn an_egg_s_villager_takes_the_type_of_its_biome_settled_and_draws_only_mob_s_tail() {
    for (biome, kind) in [
        ("minecraft:snowy_plains", kinds::SNOW),
        ("minecraft:mangrove_swamp", kinds::SWAMP),
        ("minecraft:wooded_badlands", kinds::DESERT),
        ("minecraft:old_growth_pine_taiga", kinds::TAIGA),
        // A biome the map does not name, and none read: plains.
        ("minecraft:forest", kinds::PLAINS),
    ] {
        let (mob, calls) = made(
            entity_types::VILLAGER,
            SpawnReason::SpawnEgg,
            Some(biome),
            None,
        );
        assert_eq!(calls, MOB, "{biome}");
        let state = own(&mob);
        assert_eq!(
            state.data,
            Data {
                kind: kind as u8,
                profession: professions::NONE as u8,
                level: 1
            },
            "{biome}"
        );
        assert!(state.finalized);
        assert!(!animal::is_baby(&mob));
    }
    let (mob, _) = made(entity_types::VILLAGER, SpawnReason::SpawnEgg, None, None);
    assert_eq!(own(&mob).data.kind, kinds::PLAINS as u8);
}

#[test]
fn a_template_s_villager_keeps_its_type_profession_level_and_age() {
    // A taiga farmer of level 2 in a desert: its saved data settles it.
    let tag = nbt::compound(&[
        nbt::string("id", "minecraft:villager"),
        nbt::nested(
            "VillagerData",
            &[
                nbt::string("type", "minecraft:taiga"),
                nbt::string("profession", "minecraft:farmer"),
                nbt::int("level", 2),
            ],
        ),
        nbt::int("Age", -24_000),
    ]);
    let (mob, calls) = made(
        entity_types::VILLAGER,
        SpawnReason::Structure,
        Some("minecraft:desert"),
        Some(&tag),
    );
    assert_eq!(calls, MOB);
    assert_eq!(
        own(&mob).data,
        Data {
            kind: kinds::TAIGA as u8,
            profession: professions::FARMER as u8,
            level: 2
        }
    );
    assert!(own(&mob).finalized);
    assert!(animal::is_baby(&mob), "the template's baby");
    // A farmer wants bone meal, which an unemployed villager does not.
    let bone_meal = ItemStack::new(items::BONE_MEAL, 1);
    assert!(villager::wants_to_pick_up(&mob, bone_meal.clone()));
    assert!(!villager::wants_to_pick_up(&Mob::fresh(), bone_meal));

    // `VillagerDataFinalized` alone settles the default data, which the biome then
    // does not change; a tag of neither leaves it to the biome.
    let tag = nbt::compound(&[nbt::byte("VillagerDataFinalized", 1)]);
    let (mob, _) = made(
        entity_types::VILLAGER,
        SpawnReason::Structure,
        Some("minecraft:desert"),
        Some(&tag),
    );
    assert_eq!(own(&mob).data, Data::default());
    let tag = nbt::compound(&[nbt::int("Xp", 3)]);
    let (mob, _) = made(
        entity_types::VILLAGER,
        SpawnReason::Structure,
        Some("minecraft:desert"),
        Some(&tag),
    );
    assert_eq!(own(&mob).data.kind, kinds::DESERT as u8);
    assert_eq!(own(&mob).xp, 3);
}

#[test]
fn an_egg_s_wandering_trader_draws_only_mob_s_tail_and_never_leaves() {
    let (mob, calls) = made(
        entity_types::WANDERING_TRADER,
        SpawnReason::SpawnEgg,
        Some("minecraft:plains"),
        None,
    );
    assert_eq!(calls, MOB);
    assert_eq!(wandering_trader::state_of(&mob).1.despawn_delay, 0);
    // A template's delay is kept, and its age held at nothing or above.
    let tag = nbt::compound(&[nbt::int("DespawnDelay", 48_000), nbt::int("Age", -100)]);
    let (mob, _) = made(
        entity_types::WANDERING_TRADER,
        SpawnReason::Structure,
        Some("minecraft:plains"),
        Some(&tag),
    );
    assert_eq!(wandering_trader::state_of(&mob).1.despawn_delay, 48_000);
    assert!(!animal::is_baby(&mob));
}

/// The sensors' scan rates of the villager's brain, in the game's order
/// (Villager.txt 2313–2343): the nearest living things', the players', the items',
/// the bed's, the hurt's, the hostiles', the babies' (each 20), the secondary points'
/// (40) and the golems' (200).
const SCAN_RATES: [u32; 9] = [20, 20, 20, 20, 20, 20, 20, 40, 200];

/// What `Brain.<init>` draws of the mob's own stream, a sensor at a time: the
/// `lived` of each sensor's first scan for a brain whose first tick is at 0.
fn delays(own: &mut Random) -> Vec<u32> {
    SCAN_RATES
        .iter()
        .map(|rate| own.next_int(*rate).max(1) - 1)
        .collect()
}

#[test]
fn a_villager_s_construction_draws_its_nine_sensors_delays_and_a_baby_s_again() {
    let kind = entity_types::VILLAGER as u16;
    let seeded = || Random::of_generated(11, EntityId(5));
    let mut own = seeded();
    let (_, adult) = crate::mobs::making::construct(kind, &mut own, 0.0);
    let mut expected = seeded();
    expected.next_u64();
    expected.next_u64();
    expected.next_float();
    let scans = delays(&mut expected);
    let state: BrainState = crate::mobs::part::decode(&adult.mind.brain).unwrap();
    assert_eq!(state.next_scans, scans);
    assert_eq!(own.next_u64(), expected.next_u64(), "nothing more drawn");
    assert!(adult.body.flags.picks_up_loot);

    // A child made a baby before it is added (an egg's on a villager) has its brain made
    // again (`setBaby` → `refreshBrain`): nine more.
    let mut child = Mob::fresh();
    animal::made_baby(&mut child);
    let mut own = seeded();
    crate::mobs::brain::construct(super::of(kind), &mut child, &mut own);
    let mut expected = seeded();
    delays(&mut expected);
    let again = delays(&mut expected);
    let state: BrainState = crate::mobs::part::decode(&child.mind.brain).unwrap();
    assert_eq!(state.next_scans, again);
}

#[test]
fn the_schedule_gives_each_time_of_day_its_activity_and_work_meeting_and_play_are_idle() {
    use Activity::{Idle, Meet, Play, Rest, Work};
    let adult = [
        (0, Rest),
        (9, Rest),
        (10, Idle),
        (1999, Idle),
        (2000, Work),
        (8999, Work),
        (9000, Meet),
        (11_000, Idle),
        (12_000, Rest),
        (23_999, Rest),
        (24_010, Idle),
    ];
    for (day, activity) in adult {
        assert_eq!(villager::scheduled(day, false), activity, "{day}");
    }
    let baby = [
        (5, Rest),
        (10, Idle),
        (3000, Play),
        (6000, Idle),
        (10_000, Play),
        (12_000, Rest),
    ];
    for (day, activity) in baby {
        assert_eq!(villager::scheduled(day, true), activity, "{day}");
    }
}

#[test]
fn its_eight_slots_fill_as_a_simple_container_s_and_refuse_once_full() {
    let mut inventory = [const { None }; SLOTS];
    let bread = |count| ItemStack::new(items::BREAD, count);
    // Into the slot of the same item up to its most (64), the rest into the first
    // empty slot.
    assert_eq!(villager::add_item(&mut inventory, bread(60)), 60);
    assert_eq!(villager::add_item(&mut inventory, bread(10)), 10);
    assert_eq!(inventory[0], Some(bread(64)));
    assert_eq!(inventory[1], Some(bread(6)));
    // Full: eight full stacks of carrots take no bread and no more carrots.
    let carrots = ItemStack::new(items::CARROT, 64);
    let mut full: [Option<ItemStack>; SLOTS] = std::array::from_fn(|_| Some(carrots.clone()));
    assert!(!villager::can_add_item(&full, bread(1)));
    assert!(!villager::can_add_item(&full, carrots.clone()));
    full[3] = Some(carrots.with_count(63));
    assert!(villager::can_add_item(&full, carrots));
    assert!(!villager::can_add_item(&full, bread(1)));
    // What it wants: its food and the seeds and wheat of `#villager_picks_up`, not
    // what else lies about.
    let fresh = Mob::fresh();
    for item in [
        items::BREAD,
        items::CARROT,
        items::POTATO,
        items::BEETROOT,
        items::WHEAT,
        items::WHEAT_SEEDS,
        items::BEETROOT_SEEDS,
    ] {
        assert!(
            villager::wants_to_pick_up(&fresh, ItemStack::new(item, 1)),
            "{item}"
        );
    }
    assert!(!villager::wants_to_pick_up(
        &fresh,
        ItemStack::new(items::BONE_MEAL, 1)
    ));
    assert!(!villager::wants_to_pick_up(
        &fresh,
        ItemStack::new(items::STONE, 1)
    ));
}

#[test]
fn an_egg_on_a_villager_draws_one_double_of_its_own_for_the_child_s_type() {
    let kind = entity_types::VILLAGER as u16;
    let mut parent = Mob::fresh();
    villager::update(&mut parent, |own| {
        own.data = Data {
            kind: kinds::JUNGLE as u8,
            profession: professions::MASON as u8,
            level: 4,
        };
        own.finalized = true;
    });
    for seed in 0..12 {
        let mut draws = Random::of_generated(seed, EntityId(9));
        let mut replay = draws.clone();
        let child = super::of(kind)
            .breed_offspring(&mut Breeding {
                this: &parent,
                partner: &parent,
                partner_kind: kind,
                child_kind: None,
                draws: &mut draws,
                biome: Some("minecraft:savanna"),
                egg_user: None,
            })
            .expect("a child");
        let r = replay.next_double();
        let expected = if r < 0.5 {
            kinds::SAVANNA
        } else {
            kinds::JUNGLE
        };
        assert_eq!(
            own(&child).data,
            Data {
                kind: expected as u8,
                ..Data::default()
            }
        );
        assert!(own(&child).finalized);
        assert_eq!(draws.next_u64(), replay.next_u64(), "one draw");
    }
    // A wandering trader makes none.
    let trader = entity_types::WANDERING_TRADER as u16;
    let mut draws = Random::of_generated(1, EntityId(9));
    assert!(
        super::of(trader)
            .breed_offspring(&mut Breeding {
                this: &parent,
                partner: &parent,
                partner_kind: trader,
                child_kind: None,
                draws: &mut draws,
                biome: None,
                egg_user: None,
            })
            .is_none()
    );
}

#[test]
fn clients_are_shown_a_villager_s_unhappiness_data_and_settling_and_a_baby_s_size() {
    let kind = entity_types::VILLAGER as u16;
    let fresh = Mob::fresh();
    let values = super::of(kind).shown(&fresh);
    assert_eq!(
        values,
        [
            (16, DataShown::Boolean(false), true),
            (17, DataShown::Boolean(false), true),
            (villager::UNHAPPY_INDEX, DataShown::Int(0), true),
            (
                villager::DATA_INDEX,
                DataShown::VillagerData {
                    kind: kinds::PLAINS,
                    profession: professions::NONE,
                    level: 1
                },
                true
            ),
            (villager::FINALIZED_INDEX, DataShown::Boolean(false), true),
        ]
    );
    let mut baby = Mob::fresh();
    animal::made_baby(&mut baby);
    villager::update(&mut baby, |own| {
        own.unhappy = 12;
        own.data.kind = kinds::SWAMP as u8;
    });
    let values = super::of(kind).shown(&baby);
    assert!(values.contains(&(16, DataShown::Boolean(true), false)));
    assert!(values.contains(&(villager::UNHAPPY_INDEX, DataShown::Int(12), false)));
    assert!(values.contains(&(
        villager::DATA_INDEX,
        DataShown::VillagerData {
            kind: kinds::SWAMP,
            profession: professions::NONE,
            level: 1
        },
        false
    )));
    assert_eq!(super::of(kind).dimensions(&baby), Some(villager::BABY));
    assert_eq!(super::of(kind).dimensions(&fresh), None);
}
