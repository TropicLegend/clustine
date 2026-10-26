//! The skeletons', the creeper's and the spiders' making and data, from the brief's
//! tables (`docs/groundwork/n7k-brief.md`, sections 11.2 to 11.4 and 11.12): each egg's
//! draws of the level's random in order, what it is made with, and what clients are sent.

use std::collections::VecDeque;

use clustine_data::{entity_types, items};
use clustine_world::{BlockPos, EntityId};

use super::{Companion, CompanionSeat, Companions, SpawnReason, Spawning};
use crate::living::slots;
use crate::mobs::Mob;
use crate::region::Random;
use crate::shown::DataShown;

/// One call of the level's random, with its bound where it has one.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Call {
    Int(u32),
    Float,
    Double,
}

use Call::{Double, Float, Int};

/// The level's random of a test: every call recorded, each answered from its script
/// where one is given, else a middling value (an int of 1 where the bound allows it, a
/// float or a double of a half).
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

/// `Monster.finalizeSpawn`, the tail: the follow range's triangle and the hand.
const TAIL: [Call; 3] = [Double, Double, Float];

/// `AbstractSkeleton.finalizeSpawn` after the tail at easy (`special` 0): the armour's
/// draw, the bow's enchantment's, the loot pick-up's.
const SKELETON: [Call; 3] = [Float, Float, Float];

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

/// A mob of `kind` made by an egg with `level`, beside `companions` where given.
fn made(kind: i32, level: &mut Counting, companions: Option<&mut Kept>) -> Mob {
    let kind = kind as u16;
    let mut own = Random::of_entity(7, EntityId(40), 0);
    let (_, mut mob) = crate::mobs::making::construct(kind, &mut own, 0.0);
    let mut group = None;
    crate::mobs::making::finalize(
        kind,
        &mut Spawning {
            mob: &mut mob,
            level,
            own: &mut own,
            biome: Some("minecraft:plains"),
            at: BlockPos::new(3, -60, 3),
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
    mob
}

fn follow_bonus(kind: i32, mob: &Mob) -> bool {
    let _ = kind;
    mob.body
        .attributes
        .made(clustine_data::attributes::FOLLOW_RANGE as u16)
        .is_some_and(|instance| !instance.modifiers.is_empty())
}

#[test]
fn a_skeleton_s_egg_draws_the_tail_then_armour_the_bow_s_enchantment_and_the_loot_pick_up() {
    for kind in [
        entity_types::SKELETON,
        entity_types::STRAY,
        entity_types::BOGGED,
        entity_types::PARCHED,
    ] {
        let mut level = Counting::default();
        let mob = made(kind, &mut level, None);
        assert_eq!(level.calls, [TAIL, SKELETON].concat(), "{kind}");
        let worn = &mob.body.equipment.slots;
        assert_eq!(
            worn[usize::from(slots::MAIN_HAND)].map(|stack| (stack.item, stack.count)),
            Some((items::BOW, 1)),
            "the bow, of no draw"
        );
        for slot in [slots::HEAD, slots::CHEST, slots::LEGS, slots::FEET] {
            assert_eq!(worn[usize::from(slot)], None, "no armour at easy");
        }
        assert!(!mob.body.flags.picks_up_loot, "0.55 × 0 is never passed");
        assert!(follow_bonus(kind, &mob));
    }
}

#[test]
fn a_creeper_s_egg_draws_the_tail_alone() {
    let mut level = Counting::default();
    let mob = made(entity_types::CREEPER, &mut level, None);
    assert_eq!(level.calls, TAIL);
    assert!(mob.body.equipment.slots.iter().all(Option::is_none));
}

#[test]
fn a_spider_s_egg_draws_the_tail_and_the_jockey_s_hundred_and_nothing_for_an_effect_at_easy() {
    let mut level = Counting::default();
    let mut kept = Kept::default();
    made(entity_types::SPIDER, &mut level, Some(&mut kept));
    assert_eq!(level.calls, [&TAIL[..], &[Int(100)]].concat());
    assert!(kept.made.is_empty(), "no skeleton but at a draw of 0");
}

#[test]
fn a_spider_drawing_nought_of_a_hundred_is_ridden_by_a_skeleton_of_its_own_draws() {
    let mut level = Counting {
        ints: VecDeque::from([0]),
        ..Counting::default()
    };
    let mut kept = Kept::default();
    made(entity_types::SPIDER, &mut level, Some(&mut kept));
    assert_eq!(
        level.calls,
        [&TAIL[..], &[Int(100)], &TAIL[..], &SKELETON[..]].concat(),
        "the skeleton's whole `finalizeSpawn` between the jockey's draw and the end"
    );
    let [skeleton] = &kept.made[..] else {
        panic!("one companion: {}", kept.made.len());
    };
    assert_eq!(skeleton.seat, CompanionSeat::Passenger);
    assert_eq!(i32::from(skeleton.entity.kind), entity_types::SKELETON);
    assert_eq!(skeleton.entity.position, [3.5, -60.0, 3.5]);
    let rider = Mob::decode(&skeleton.entity.data).unwrap();
    assert_eq!(
        rider.body.equipment.slots[usize::from(slots::MAIN_HAND)].map(|stack| stack.item),
        Some(items::BOW)
    );
}

#[test]
fn a_spider_made_where_no_companion_can_be_draws_the_jockey_s_hundred_all_the_same() {
    let mut level = Counting {
        ints: VecDeque::from([0]),
        ..Counting::default()
    };
    made(entity_types::SPIDER, &mut level, None);
    assert_eq!(level.calls, [&TAIL[..], &[Int(100)]].concat());
}

#[test]
fn a_cave_spider_s_egg_draws_nothing_and_leaves_it_as_constructed() {
    let mut level = Counting::default();
    let mob = made(entity_types::CAVE_SPIDER, &mut level, None);
    assert_eq!(level.calls, []);
    assert!(!follow_bonus(entity_types::CAVE_SPIDER, &mob));
    assert!(!mob.body.flags.left_handed);
}

fn shown(kind: i32, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
    super::of(kind as u16).shown(mob)
}

#[test]
fn each_type_shows_its_own_synced_values_at_their_defaults_when_fresh() {
    let fresh = Mob::fresh();
    assert_eq!(
        shown(entity_types::SKELETON, &fresh),
        [(16, DataShown::Boolean(false), true)],
        "`DATA_STRAY_CONVERSION_ID`"
    );
    assert_eq!(
        shown(entity_types::BOGGED, &fresh),
        [(16, DataShown::Boolean(false), true)],
        "`DATA_SHEARED`"
    );
    assert_eq!(shown(entity_types::STRAY, &fresh), []);
    assert_eq!(shown(entity_types::PARCHED, &fresh), []);
    assert_eq!(
        shown(entity_types::CREEPER, &fresh),
        [
            (16, DataShown::Int(-1), true),
            (17, DataShown::Boolean(false), true),
            (18, DataShown::Boolean(false), true),
        ]
    );
    for spider in [entity_types::SPIDER, entity_types::CAVE_SPIDER] {
        assert_eq!(
            shown(spider, &fresh),
            [(16, DataShown::Byte(0), true)],
            "`DATA_FLAGS_ID`"
        );
    }
}

#[test]
fn a_converting_skeleton_a_sheared_bogged_an_ignited_creeper_and_a_climbing_spider_show_it() {
    let mut skeleton = Mob::fresh();
    super::skeletons::change(&mut skeleton, |own| own.conversion = Some(12));
    assert_eq!(
        shown(entity_types::SKELETON, &skeleton),
        [(16, DataShown::Boolean(true), false)]
    );
    let mut bogged = Mob::fresh();
    super::skeletons::change(&mut bogged, |own| own.sheared = true);
    assert_eq!(
        shown(entity_types::BOGGED, &bogged),
        [(16, DataShown::Boolean(true), false)]
    );
    let mut creeper = Mob::fresh();
    let mut own = super::creeper::state_of(&creeper);
    own.ignited = true;
    own.swell_dir = 1;
    super::creeper::put_state(&mut creeper, &own);
    assert_eq!(
        shown(entity_types::CREEPER, &creeper),
        [
            (16, DataShown::Int(1), false),
            (17, DataShown::Boolean(false), true),
            (18, DataShown::Boolean(true), false),
        ]
    );
    let mut spider = Mob::fresh();
    super::spider::set_path_to(&mut spider, Some(BlockPos::new(1, 2, 3)));
    assert_eq!(
        shown(entity_types::SPIDER, &spider),
        [(16, DataShown::Byte(0), true)],
        "where it is sent is no synced value"
    );
}

#[test]
fn only_the_skeleton_is_spared_freezing_and_only_the_spiders_climb_by_their_flag_and_pass_webs() {
    let fresh = Mob::fresh();
    for kind in [
        entity_types::SKELETON,
        entity_types::STRAY,
        entity_types::BOGGED,
        entity_types::PARCHED,
        entity_types::CREEPER,
        entity_types::SPIDER,
        entity_types::CAVE_SPIDER,
    ] {
        let module = super::of(kind as u16);
        assert_eq!(
            module.can_freeze(),
            kind != entity_types::SKELETON,
            "{kind}: the stray is spared by its tag, not by its type"
        );
        let spider = matches!(kind, entity_types::SPIDER | entity_types::CAVE_SPIDER);
        assert_eq!(module.on_climbable(&fresh), spider.then_some(false));
        assert_eq!(module.stuck_in_webs(), !spider);
        assert_eq!(
            module.mob_finalize_spawn(),
            kind != entity_types::CAVE_SPIDER
        );
    }
}

#[test]
fn a_template_s_tag_gives_a_bogged_its_shearing_and_a_skeleton_its_freezing() {
    use super::template::build as nbt;
    let tag = nbt::compound(&[nbt::byte("sheared", 1)]);
    let mut bogged = Mob::fresh();
    super::of(entity_types::BOGGED as u16).from_template(&mut bogged, &tag);
    assert!(super::skeletons::state_of(&bogged).sheared);
    let tag = nbt::compound(&[
        nbt::int("FreezingTime", 150),
        nbt::int("StrayConversionTime", 77),
    ]);
    let mut skeleton = Mob::fresh();
    super::of(entity_types::SKELETON as u16).from_template(&mut skeleton, &tag);
    let own = super::skeletons::state_of(&skeleton);
    assert_eq!((own.affliction, own.conversion), (150, Some(77)));
    let tag = nbt::compound(&[nbt::int("StrayConversionTime", -1)]);
    let mut skeleton = Mob::fresh();
    super::of(entity_types::SKELETON as u16).from_template(&mut skeleton, &tag);
    let own = super::skeletons::state_of(&skeleton);
    assert_eq!((own.affliction, own.conversion), (0, None));
}
