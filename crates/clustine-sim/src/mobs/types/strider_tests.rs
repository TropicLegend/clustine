//! The strider's making, state and uses from the game's rules (d Strider.txt; the
//! brief's section 5.9): its spawn rule, its `finalizeSpawn`'s draws and jockeys, its
//! cold and its speed, its boost, and what a player's use of it does.

use std::collections::{BTreeMap, VecDeque};

use clustine_data::{BlockState, attributes as ids, blocks, entity_types, items, sounds};
use clustine_world::{BlockPos, EntityId, ItemStack};

use super::strider::{self, StriderState};
use super::{
    AgeableGroup, Companion, CompanionSeat, Companions, Did, Interaction, SpawnGroup, SpawnReason,
    Spawning, Used, animal, of,
};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::region::Random;
use crate::shown::DataShown;

const STRIDER: u16 = entity_types::STRIDER as u16;

// ---------------------------------------------------------------------------------
// The spawn rule.

/// A column of `below` lava blocks from y 10 up, then `top`, air above that.
fn column(lava: i32, top: BlockState) -> BTreeMap<i32, BlockState> {
    let mut blocks = BTreeMap::new();
    for y in 10..10 + lava {
        blocks.insert(y, blocks::LAVA);
    }
    blocks.insert(10 + lava, top);
    blocks
}

fn rule(column: &BTreeMap<i32, BlockState>, from: i32) -> bool {
    strider::climbs_to_air(
        |at| {
            if at.y > 200 {
                return None;
            }
            Some(column.get(&at.y).copied().unwrap_or(blocks::AIR))
        },
        BlockPos::new(0, from, 0),
    )
}

#[test]
fn a_strider_may_spawn_deep_in_lava_that_has_air_over_it_and_not_under_a_roof() {
    // Lava from y 10 to 14, air at 15: anywhere in it, or at its top block.
    let open = column(5, blocks::AIR);
    for from in 9..=14 {
        assert!(rule(&open, from), "from {from}");
    }
    // Netherrack over the lava: refused wherever in the lava.
    let roofed = column(5, blocks::NETHERRACK);
    for from in 9..=14 {
        assert!(!rule(&roofed, from), "from {from}");
    }
    // The block above the position is what is first looked at: stone there refuses.
    let mut on_stone = BTreeMap::new();
    on_stone.insert(11, blocks::STONE);
    assert!(!rule(&on_stone, 10));
    // A block the world cannot read refuses.
    assert!(!strider::climbs_to_air(|_| None, BlockPos::new(0, 10, 0)));
}

// ---------------------------------------------------------------------------------
// `finalizeSpawn`.

/// One call of the level's random, with its bound where it has one.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Call {
    Int(u32),
    Float,
    Double,
}

use Call::{Double, Float, Int};

/// The level's random of a test: every call recorded, each int answered from its script
/// where one is given, else 1; a float or a double a half.
#[derive(Default)]
struct Counting {
    calls: Vec<Call>,
    ints: VecDeque<u32>,
}

impl clustine_world::generation::Draws for Counting {
    fn next_int(&mut self, bound: u32) -> u32 {
        self.calls.push(Int(bound));
        self.ints
            .pop_front()
            .unwrap_or(1)
            .min(bound.saturating_sub(1))
    }

    fn next_float(&mut self) -> f32 {
        self.calls.push(Float);
        0.5
    }

    fn next_double(&mut self) -> f64 {
        self.calls.push(Double);
        0.5
    }

    fn next_bool(&mut self) -> bool {
        panic!("no booleans are drawn");
    }
}

/// `Mob.finalizeSpawn`'s tail: the follow range's triangle and the hand.
const TAIL: [Call; 3] = [Double, Double, Float];

/// A maker of companions that keeps what it is given.
#[derive(Default)]
struct Kept {
    made: Vec<Companion>,
}

impl Companions for Kept {
    fn next(&mut self) -> Option<(EntityId, Random)> {
        let key = EntityId(500 + self.made.len() as u64);
        Some((key, Random::of_entity(3, key, 0)))
    }

    fn add(&mut self, companion: Companion) {
        self.made.push(companion);
    }

    fn nearby(&self, _: &crate::physics::aabb::Aabb) -> Vec<(EntityId, u16, bool)> {
        Vec::new()
    }

    fn ride(&mut self, _: EntityId) {}
}

/// A strider made by an egg with `level` at (3, 30, 3), beside `companions` where
/// given, `prepare` done to it first; the strider and the group it left.
fn made(
    level: &mut Counting,
    companions: Option<&mut Kept>,
    prepare: impl FnOnce(&mut Mob),
) -> (Mob, Option<SpawnGroup>) {
    let mut own = Random::of_entity(7, EntityId(40), 0);
    let (_, mut mob) = crate::mobs::making::construct(STRIDER, &mut own, 0.0);
    prepare(&mut mob);
    let mut group = None;
    crate::mobs::making::finalize(
        STRIDER,
        &mut Spawning {
            mob: &mut mob,
            level,
            own: &mut own,
            biome: Some("minecraft:nether_wastes"),
            at: BlockPos::new(3, 30, 3),
            day: 0,
            game_time: 0,
            reason: SpawnReason::SpawnEgg,
            group: &mut group,
            structure: None,
            moon_brightness: 1.0,
            difficulty: (0.0, 0.0),
            companions: companions.map(|kept| kept as &mut dyn Companions),
        },
    );
    (mob, group)
}

fn saddle_of(mob: &Mob) -> Option<i32> {
    mob.body.equipment.slots[usize::from(slots::SADDLE)]
        .as_ref()
        .map(|stack| stack.item)
}

fn group_of(group: Option<SpawnGroup>) -> AgeableGroup {
    match group {
        Some(SpawnGroup::Ageable(ageable)) => ageable,
        other => panic!("an ageable group: {other:?}"),
    }
}

#[test]
fn a_strider_without_a_rider_draws_thirty_then_ten_and_leaves_a_fresh_group_of_half_babies() {
    let mut level = Counting::default();
    let mut kept = Kept::default();
    let (mob, group) = made(&mut level, Some(&mut kept), |_| {});
    assert_eq!(level.calls, [&[Int(30), Int(10)][..], &TAIL[..]].concat());
    assert!(kept.made.is_empty());
    assert_eq!(saddle_of(&mob), None);
    assert!(!animal::is_baby(&mob));
    // `AgeableMobGroupData(0.5f)`, counted once and never drawn against: it is fresh.
    let group = group_of(group);
    assert_eq!((group.made, group.babies), (1, true));
    assert_eq!(group.baby_chance, 0.5);
}

#[test]
fn a_strider_drawing_nought_of_thirty_is_saddled_for_good_under_a_zombified_piglin_with_the_stick()
{
    let mut level = Counting {
        ints: VecDeque::from([0]),
        ..Counting::default()
    };
    let mut kept = Kept::default();
    let (mob, group) = made(&mut level, Some(&mut kept), |_| {});
    assert_eq!(level.calls[0], Int(30));
    assert!(
        !level.calls.contains(&Int(10)),
        "no draw for a baby strider: {:?}",
        level.calls
    );
    // The rider's whole `finalizeSpawn` between the jockey's draw and the strider's tail.
    assert!(level.calls.len() > 1 + 2 * TAIL.len(), "{:?}", level.calls);
    assert_eq!(level.calls[level.calls.len() - 3..], TAIL);
    let [rider] = &kept.made[..] else {
        panic!("one companion: {}", kept.made.len());
    };
    assert_eq!(rider.seat, CompanionSeat::Passenger);
    assert_eq!(i32::from(rider.entity.kind), entity_types::ZOMBIFIED_PIGLIN);
    assert_eq!(
        rider.entity.position,
        [3.5, 30.0, 3.5],
        "snapped to the strider"
    );
    let piglin = Mob::decode(&rider.entity.data).unwrap();
    assert_eq!(
        piglin.body.equipment.slots[usize::from(slots::MAIN_HAND)]
            .as_ref()
            .map(|stack| (stack.item, stack.count)),
        Some((items::WARPED_FUNGUS_ON_A_STICK, 1))
    );
    assert_eq!(saddle_of(&mob), Some(items::SADDLE));
    let chances = mob
        .body
        .equipment
        .drop_chances
        .expect("its saddle's chance");
    assert_eq!(chances[usize::from(slots::SADDLE)], 2.0, "guaranteed");
    // `spawnJockey`'s `AgeableMobGroupData(0)`.
    let group = group_of(group);
    assert_eq!((group.made, group.baby_chance), (1, 0.0));
}

#[test]
fn a_strider_drawing_nought_of_ten_carries_a_baby_strider_finalised_as_a_baby() {
    let mut level = Counting {
        ints: VecDeque::from([1, 0]),
        ..Counting::default()
    };
    let mut kept = Kept::default();
    let (mob, group) = made(&mut level, Some(&mut kept), |_| {});
    // The baby's `finalizeSpawn` is the animal's alone: no jockey of its own, its tail.
    assert_eq!(
        level.calls,
        [&[Int(30), Int(10)][..], &TAIL[..], &TAIL[..]].concat()
    );
    let [rider] = &kept.made[..] else {
        panic!("one companion: {}", kept.made.len());
    };
    assert_eq!(rider.seat, CompanionSeat::Passenger);
    assert_eq!(rider.entity.kind, STRIDER);
    let baby = Mob::decode(&rider.entity.data).unwrap();
    assert!(animal::is_baby(&baby));
    assert_eq!(saddle_of(&mob), None, "no saddle for a strider's jockey");
    assert!(!animal::is_baby(&mob));
    assert_eq!(group_of(group).baby_chance, 0.0);
}

#[test]
fn a_strider_made_where_no_rider_can_be_made_is_neither_saddled_nor_given_the_jockey_s_group() {
    let mut level = Counting {
        ints: VecDeque::from([0]),
        ..Counting::default()
    };
    let (mob, group) = made(&mut level, None, |_| {});
    assert_eq!(level.calls, [&[Int(30)][..], &TAIL[..]].concat());
    assert_eq!(saddle_of(&mob), None);
    // The game's `create` null: the group handed in, none, so `AgeableMob`'s own (true).
    let group = group_of(group);
    assert_eq!((group.baby_chance, group.made), (0.05, 1));
}

#[test]
fn a_baby_strider_s_making_draws_no_jockey() {
    let mut level = Counting::default();
    let mut kept = Kept::default();
    let (mob, _) = made(&mut level, Some(&mut kept), |mob| {
        animal::update(mob, |shared| shared.set_age(animal::BABY_START_AGE, 0));
    });
    assert_eq!(level.calls, TAIL);
    assert!(kept.made.is_empty());
    assert!(animal::is_baby(&mob));
    let size = of(STRIDER).dimensions(&mob).expect("a baby's own size");
    assert_eq!(
        (size.width, size.height, size.eye_height),
        (0.45, 0.85, 0.4375)
    );
}

// ---------------------------------------------------------------------------------
// Its state.

fn speed(mob: &Mob) -> f64 {
    mob.body
        .attributes
        .value(STRIDER, attribute(ids::MOVEMENT_SPEED))
}

fn shown(mob: &Mob, index: u8) -> Option<(DataShown, bool)> {
    of(STRIDER)
        .shown(mob)
        .into_iter()
        .find(|(at, _, _)| *at == index)
        .map(|(_, value, default)| (value, default))
}

#[test]
fn a_strider_s_type_stands_on_lava_and_is_steered_and_boosted_by_the_warped_fungus_on_a_stick() {
    let strider = of(STRIDER);
    assert!(strider.stands_on_lava());
    assert_eq!(
        strider.steered_by_stick(),
        Some(items::WARPED_FUNGUS_ON_A_STICK)
    );
    let stick = Some(ItemStack::new(items::WARPED_FUNGUS_ON_A_STICK, 1));
    let carrot = Some(ItemStack::new(items::CARROT_ON_A_STICK, 1));
    let mut mob = Mob::fresh();
    assert!(
        !strider.steered_by(&mob, [stick.clone(), None]),
        "no saddle"
    );
    mob.body.equipment.slots[usize::from(slots::SADDLE)] = Some(ItemStack::new(items::SADDLE, 1));
    assert!(strider.steered_by(&mob, [stick.clone(), None]));
    assert!(strider.steered_by(&mob, [None, stick]), "in the off hand");
    assert!(!strider.steered_by(&mob, [carrot, None]));
}

#[test]
fn a_cold_strider_shows_it_and_is_a_third_slower_until_it_is_warm_again() {
    let mut mob = Mob::fresh();
    let base = speed(&mob);
    assert_eq!(base, 0.174_999_997_019_767_76);
    assert_eq!(shown(&mob, 19), Some((DataShown::Boolean(false), true)));
    // What `Strider.tick`'s head sets of a strider out of warmth: the flag and the
    // modifier of −0.34 of the base.
    let cold = StriderState {
        suffocating: true,
        ..StriderState::default()
    };
    mob.state = animal::write(&animal::Animal::default(), &cold);
    let instance = mob
        .body
        .attributes
        .instance(STRIDER, attribute(ids::MOVEMENT_SPEED))
        .unwrap();
    instance.add_modifier(crate::mobs::body::Modifier {
        id: "minecraft:suffocating".to_owned(),
        amount: -0.340_000_003_576_278_7,
        operation: crate::mobs::body::attributes::ADD_MULTIPLIED_BASE,
    });
    assert_eq!(speed(&mob), base + base * -0.340_000_003_576_278_7);
    assert_eq!(shown(&mob, 19), Some((DataShown::Boolean(true), false)));
}

#[test]
fn a_use_of_the_stick_boosts_a_strider_once_for_its_own_draw_and_shows_the_length() {
    let mut mob = Mob::fresh();
    let mut draws = Random::of_entity(9, EntityId(40), 0);
    let mut expected = draws.clone();
    assert!(of(STRIDER).stick_boost(&mut mob, &mut draws));
    let total = expected.next_int(841) as i32 + 140;
    let own = strider::state_of(&mob);
    assert!(own.boost.boosting);
    assert_eq!((own.boost.time, own.boost.total), (0, total));
    assert_eq!(shown(&mob, 18), Some((DataShown::Int(total), false)));
    let before = draws.clone();
    assert!(!of(STRIDER).stick_boost(&mut mob, &mut draws), "boosting");
    assert_eq!(
        draws.draws_since(&before),
        0,
        "a refused boost draws nothing"
    );
}

// ---------------------------------------------------------------------------------
// A player's use.

/// A grown strider of its full health.
fn alive() -> Mob {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 20.0;
    mob
}

/// A use of `mob` by a player holding `held`, sneaking where `sneaking`.
fn used(mob: &mut Mob, held: Option<ItemStack>, sneaking: bool) -> (Interaction, Vec<Did>) {
    let mut draws = Random::of_entity(9, EntityId(40), 0);
    let mut use_ = Used {
        key: EntityId(40),
        kind: STRIDER,
        mob,
        player: EntityId(1),
        player_uuid: None,
        game_time: 0,
        held,
        creative: true,
        sneaking,
        carries: false,
        passengers: 0,
        on_ground: true,
        from: [2.0, 30.0, 0.5],
        position: [0.5, 30.0, 0.5],
        moment: 0,
        draws: &mut draws,
        did: Vec::new(),
    };
    let result = of(STRIDER).mob_interact(&mut use_);
    (result, use_.did)
}

fn sounds_of(did: &[Did]) -> Vec<i32> {
    did.iter()
        .filter_map(|did| match did {
            Did::Sound { sound, .. } => Some(*sound),
            _ => None,
        })
        .collect()
}

#[test]
fn a_saddle_goes_on_a_grown_strider_with_its_own_sound_and_then_a_click_takes_the_player_on() {
    let saddle = Some(ItemStack::new(items::SADDLE, 1));
    let mut baby = alive();
    animal::update(&mut baby, |shared| {
        shared.set_age(animal::BABY_START_AGE, 0)
    });
    assert_eq!(used(&mut baby, saddle.clone(), false).0, Interaction::Pass);

    let mut mob = alive();
    let (result, did) = used(&mut mob, saddle.clone(), false);
    assert_eq!(result, Interaction::Success);
    assert_eq!(sounds_of(&did), [sounds::ENTITY_STRIDER_SADDLE]);
    assert!(did.contains(&Did::GameEvent("minecraft:equip")));
    assert_eq!(saddle_of(&mob), Some(items::SADDLE));
    // Saddled, a click with anything but its food takes the player on, a saddle too;
    // one who sneaks is not.
    let (result, did) = used(&mut mob, saddle, false);
    assert_eq!((result, did), (Interaction::Success, vec![Did::Mount]));
    let (result, did) = used(&mut mob, None, false);
    assert_eq!((result, did), (Interaction::Success, vec![Did::Mount]));
    let (_, did) = used(&mut mob, None, true);
    assert!(!did.contains(&Did::Mount));
}

#[test]
fn warped_fungus_puts_a_grown_strider_in_love_with_its_eating_sound_and_is_not_a_ride() {
    let mut mob = Mob::fresh();
    mob.body.equipment.slots[usize::from(slots::SADDLE)] = Some(ItemStack::new(items::SADDLE, 1));
    let (result, did) = used(
        &mut mob,
        Some(ItemStack::new(items::WARPED_FUNGUS, 1)),
        false,
    );
    assert_eq!(result, Interaction::SuccessServer);
    assert!(!did.contains(&Did::Mount));
    assert!(did.contains(&Did::Event(18)), "the hearts");
    assert_eq!(sounds_of(&did), [sounds::ENTITY_STRIDER_EAT]);
    assert!(animal::animal_of(&mob.state).in_love(0));
}
