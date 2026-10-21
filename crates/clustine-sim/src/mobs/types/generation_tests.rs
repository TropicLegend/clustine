//! The creatures' `finalizeSpawn` for a chunk's generation and for a structure, against
//! the brief's table of draws (`docs/groundwork/n7-brief.md`, section 10.8, with 8.7
//! and 8.12): each draw of the level's random in order with its bound, for the first
//! of a group and for a later member; the groups the members share; and what a
//! template's mob keeps of its tag.

use std::collections::VecDeque;

use clustine_data::entity_types;
use clustine_world::{BlockPos, EntityId};

use super::template::build::{self as nbt};
use super::{AgeableGroup, SpawnGroup, SpawnReason, Spawning, animal, cat, equine, fox, goat};
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::body::attributes::RANDOM_SPAWN_BONUS;
use crate::region::Random;

/// One call of the level's random, with its bound where it has one.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Call {
    Int(u32),
    Float,
    Double,
    Bool,
}

use Call::{Bool, Double, Float, Int};

/// The level's random of a test: it records every call, and answers each from its
/// script where one is given, else with a middling value (an int of 0, a float or a
/// double of a half, false).
#[derive(Default)]
struct Counting {
    calls: Vec<Call>,
    ints: VecDeque<u32>,
    floats: VecDeque<f32>,
    doubles: VecDeque<f64>,
    bools: VecDeque<bool>,
}

impl Counting {
    fn floats(floats: &[f32]) -> Self {
        Self {
            floats: floats.iter().copied().collect(),
            ..Self::default()
        }
    }
}

impl clustine_world::generation::Draws for Counting {
    fn next_int(&mut self, bound: u32) -> u32 {
        self.calls.push(Int(bound));
        self.ints
            .pop_front()
            .unwrap_or(0)
            .min(bound.saturating_sub(1))
    }

    fn next_float(&mut self) -> f32 {
        self.calls.push(Float);
        self.floats.pop_front().unwrap_or(0.5)
    }

    fn next_double(&mut self) -> f64 {
        self.calls.push(Double);
        self.doubles.pop_front().unwrap_or(0.5)
    }

    fn next_bool(&mut self) -> bool {
        self.calls.push(Bool);
        self.bools.pop_front().unwrap_or(false)
    }
}

/// `Mob.finalizeSpawn`'s draws for a mob without the follow range's bonus: the
/// triangle's two doubles, then the hand.
const MOB: [Call; 3] = [Double, Double, Float];

/// `AgeableMob`'s draw for a later member of a group that may have babies.
const AGEABLE: [Call; 1] = [Float];

fn seq(parts: &[&[Call]]) -> Vec<Call> {
    parts.concat()
}

/// A mob of `kind` made for `reason` in `biome` and finalised with `level`, as a member
/// of `group`, after what `before` does to the fresh mob (a template's tag read).
fn made_with(
    kind: i32,
    reason: SpawnReason,
    biome: Option<&'static str>,
    structure: Option<&'static str>,
    group: &mut Option<SpawnGroup>,
    level: &mut Counting,
    before: impl FnOnce(&mut Mob),
) -> Mob {
    let kind = kind as u16;
    let mut own = Random::of_generated(7, EntityId(1 << 63));
    let (_, mut mob) = crate::mobs::making::construct(kind, &mut own, 0.0);
    before(&mut mob);
    crate::mobs::making::finalize(
        kind,
        &mut Spawning {
            mob: &mut mob,
            level,
            own: &mut own,
            biome,
            at: BlockPos::new(0, 64, 0),
            day: 0,
            game_time: 0,
            reason,
            group,
            structure,
            moon_brightness: 1.0,
            difficulty: (0.0, 0.0),
        },
    );
    mob
}

/// A member of `group` made with a chunk in `biome`, and the draws it made.
fn member(
    kind: i32,
    biome: &'static str,
    group: &mut Option<SpawnGroup>,
    mut level: Counting,
) -> (Mob, Vec<Call>) {
    let mob = made_with(
        kind,
        SpawnReason::ChunkGeneration,
        Some(biome),
        None,
        group,
        &mut level,
        |_| {},
    );
    (mob, level.calls)
}

fn is_baby(kind: i32, mob: &Mob) -> bool {
    super::of(kind as u16).is_baby(mob)
}

/// The draws of the first of a group and of the second, each with the default answers.
fn first_and_later(kind: i32, biome: &'static str) -> (Vec<Call>, Vec<Call>) {
    let mut group = None;
    let (_, first) = member(kind, biome, &mut group, Counting::default());
    let (_, later) = member(kind, biome, &mut group, Counting::default());
    (first, later)
}

#[test]
fn each_creature_draws_as_the_table_says_first_of_a_group_and_after() {
    const PLAINS: &str = "minecraft:plains";
    let cases: Vec<(i32, &str, Vec<Call>, Vec<Call>)> = vec![
        // A draw of 0 of a hundred is the first of the few colours: no common draw.
        (
            entity_types::SHEEP,
            PLAINS,
            seq(&[&[Int(100)], &MOB]),
            seq(&[&[Int(100)], &AGEABLE, &MOB]),
        ),
        (
            entity_types::PIG,
            PLAINS,
            seq(&[&[Int(1), Int(3)], &MOB]),
            seq(&[&[Int(1), Int(3)], &AGEABLE, &MOB]),
        ),
        (
            entity_types::COW,
            PLAINS,
            seq(&[&[Int(1), Int(2)], &MOB]),
            seq(&[&[Int(1), Int(2)], &AGEABLE, &MOB]),
        ),
        (
            entity_types::CHICKEN,
            PLAINS,
            seq(&[&[Int(1), Int(2)], &MOB]),
            seq(&[&[Int(1), Int(2)], &AGEABLE, &MOB]),
        ),
        (
            entity_types::MOOSHROOM,
            "minecraft:mushroom_fields",
            MOB.to_vec(),
            seq(&[&AGEABLE, &MOB]),
        ),
        (
            entity_types::ARMADILLO,
            "minecraft:savanna",
            MOB.to_vec(),
            seq(&[&AGEABLE, &MOB]),
        ),
        (
            entity_types::TURTLE,
            "minecraft:beach",
            MOB.to_vec(),
            seq(&[&AGEABLE, &MOB]),
        ),
        (
            entity_types::HORSE,
            PLAINS,
            seq(&[&[Int(7), Int(5), Int(8), Int(9)], &[Double; 6], &MOB]),
            seq(&[&[Int(5), Int(8), Int(9)], &[Double; 6], &AGEABLE, &MOB]),
        ),
        (
            entity_types::DONKEY,
            PLAINS,
            seq(&[&[Int(8), Int(9)], &MOB]),
            seq(&[&[Int(8), Int(9)], &AGEABLE, &MOB]),
        ),
        (
            entity_types::MULE,
            PLAINS,
            seq(&[&[Int(8), Int(9)], &MOB]),
            seq(&[&[Int(8), Int(9)], &AGEABLE, &MOB]),
        ),
        // A strength's float of a half: three, not five.
        (
            entity_types::LLAMA,
            "minecraft:savanna_plateau",
            seq(&[&[Float, Int(3), Int(4), Int(8), Int(9)], &MOB]),
            seq(&[&[Float, Int(3), Int(8), Int(9)], &AGEABLE, &MOB]),
        ),
        (
            entity_types::CAMEL,
            "minecraft:desert",
            MOB.to_vec(),
            seq(&[&AGEABLE, &MOB]),
        ),
        (
            entity_types::PANDA,
            "minecraft:bamboo_jungle",
            seq(&[&[Int(16), Int(16)], &MOB]),
            seq(&[&[Int(16), Int(16)], &AGEABLE, &MOB]),
        ),
        (
            entity_types::PARROT,
            "minecraft:jungle",
            seq(&[&[Int(5)], &MOB]),
            seq(&[&[Int(5)], &MOB]),
        ),
        (
            entity_types::RABBIT,
            "minecraft:flower_forest",
            seq(&[&[Int(100)], &MOB]),
            seq(&[&[Int(100)], &AGEABLE, &MOB]),
        ),
        // A float of a half holds nothing in its mouth.
        (
            entity_types::FOX,
            "minecraft:taiga",
            seq(&[&[Float], &MOB]),
            seq(&[&[Float], &MOB]),
        ),
        (
            entity_types::WOLF,
            "minecraft:forest",
            seq(&[&[Int(1), Int(7)], &MOB]),
            seq(&[&[Int(7)], &MOB]),
        ),
        (
            entity_types::POLAR_BEAR,
            "minecraft:snowy_plains",
            MOB.to_vec(),
            seq(&[&AGEABLE, &MOB]),
        ),
        (
            entity_types::OCELOT,
            "minecraft:jungle",
            MOB.to_vec(),
            seq(&[&AGEABLE, &MOB]),
        ),
        // A horn's float of a half keeps both.
        (
            entity_types::GOAT,
            "minecraft:snowy_slopes",
            seq(&[&[Int(601), Int(5401), Double, Float], &MOB]),
            seq(&[&[Int(601), Int(5401), Double, Float], &AGEABLE, &MOB]),
        ),
        (
            entity_types::FROG,
            "minecraft:swamp",
            seq(&[&[Int(1), Int(41)], &MOB]),
            seq(&[&[Int(1), Int(41)], &AGEABLE, &MOB]),
        ),
        // The cat's own draws come after its base's, at the full moon of eleven.
        (
            entity_types::CAT,
            PLAINS,
            seq(&[&MOB, &[Int(11), Int(2)]]),
            seq(&[&AGEABLE, &MOB, &[Int(11), Int(2)]]),
        ),
        (entity_types::ALLAY, PLAINS, MOB.to_vec(), MOB.to_vec()),
    ];
    for (kind, biome, first, later) in cases {
        assert!(super::is_built(kind as u16), "{kind}");
        let (drawn_first, drawn_later) = first_and_later(kind, biome);
        assert_eq!(drawn_first, first, "the first of a group of {kind}");
        assert_eq!(drawn_later, later, "a later member of a group of {kind}");
    }
}

#[test]
fn a_sheep_of_the_common_colour_draws_for_pink_too() {
    let mut group = None;
    let level = Counting {
        ints: VecDeque::from([50, 0]),
        ..Counting::default()
    };
    let (_, calls) = member(entity_types::SHEEP, "minecraft:plains", &mut group, level);
    assert_eq!(calls, seq(&[&[Int(100), Int(500)], &MOB]));
}

#[test]
fn a_fox_with_something_in_its_mouth_draws_what_and_a_coin_between_rabbit_and_egg() {
    let mut group = None;
    let level = Counting::floats(&[0.1, 0.3]);
    let (fox, calls) = member(entity_types::FOX, "minecraft:taiga", &mut group, level);
    assert_eq!(calls, seq(&[&[Float, Float, Bool], &MOB]));
    assert!(fox.body.equipment.slots[0].is_some());
}

#[test]
fn a_herd_of_horses_shares_the_first_one_s_coat_and_draws_its_own_markings() {
    let mut group = None;
    let first = Counting {
        ints: VecDeque::from([3, 1]),
        ..Counting::default()
    };
    let (one, _) = member(entity_types::HORSE, "minecraft:plains", &mut group, first);
    let later = Counting {
        ints: VecDeque::from([4]),
        ..Counting::default()
    };
    let (two, calls) = member(entity_types::HORSE, "minecraft:plains", &mut group, later);
    assert_eq!(equine::horse_of(&one).variant, 3 | (1 << 8));
    assert_eq!(equine::horse_of(&two).variant, 3 | (4 << 8));
    assert_eq!(calls[0], Int(5), "no coat drawn for a later horse");
    assert!(matches!(group, Some(SpawnGroup::Horse { coat: 3, .. })));
}

#[test]
fn a_herd_s_later_horse_is_a_foal_one_time_in_twenty() {
    let mut group = None;
    member(
        entity_types::HORSE,
        "minecraft:plains",
        &mut group,
        Counting::default(),
    );
    // The horse's own draws: markings, health, six doubles; then the foal's float.
    let level = Counting {
        floats: VecDeque::from([0.05]),
        ..Counting::default()
    };
    let (foal, _) = member(entity_types::HORSE, "minecraft:plains", &mut group, level);
    assert!(is_baby(entity_types::HORSE, &foal));
    let level = Counting::floats(&[0.051]);
    let (grown, _) = member(entity_types::HORSE, "minecraft:plains", &mut group, level);
    assert!(!is_baby(entity_types::HORSE, &grown));
}

#[test]
fn rabbits_after_the_first_are_all_babies_of_the_group_s_variant() {
    let mut group = None;
    let biome = "minecraft:flower_forest";
    // 0 of a hundred is brown, 95 black.
    let first = Counting {
        ints: VecDeque::from([0]),
        ..Counting::default()
    };
    let (one, _) = member(entity_types::RABBIT, biome, &mut group, first);
    assert!(!is_baby(entity_types::RABBIT, &one));
    for _ in 0..3 {
        let level = Counting {
            ints: VecDeque::from([95]),
            floats: VecDeque::from([0.999_999]),
            ..Counting::default()
        };
        let (later, calls) = member(entity_types::RABBIT, biome, &mut group, level);
        assert_eq!(calls[0], Int(100), "drawn even where the group's is taken");
        assert!(is_baby(entity_types::RABBIT, &later));
        let (_, own) = animal::read::<super::rabbit::RabbitState>(&later.state);
        assert_eq!(
            own.variant,
            super::rabbit::random_variant(Some(biome), &mut Counting::default())
        );
    }
}

#[test]
fn a_fox_is_a_cub_from_the_group_s_third_on_without_a_draw_for_it() {
    let mut group = None;
    let mut babies = Vec::new();
    for _ in 0..4 {
        let (fox, calls) = member(
            entity_types::FOX,
            "minecraft:snowy_taiga",
            &mut group,
            Counting::default(),
        );
        assert_eq!(calls, seq(&[&[Float], &MOB]));
        assert_eq!(fox::state_of(&fox).variant, fox::variant::SNOW);
        babies.push(is_baby(entity_types::FOX, &fox));
    }
    assert_eq!(babies, [false, false, true, true]);
}

#[test]
fn a_goat_that_becomes_a_kid_may_have_lost_a_horn_first() {
    let mut group = None;
    member(
        entity_types::GOAT,
        "minecraft:snowy_slopes",
        &mut group,
        Counting::default(),
    );
    // A horn off (below 0.1, the left by the coin), then the group's kid (at most 0.05).
    let level = Counting {
        floats: VecDeque::from([0.05, 0.01]),
        bools: VecDeque::from([true]),
        ..Counting::default()
    };
    let (kid, calls) = member(
        entity_types::GOAT,
        "minecraft:snowy_slopes",
        &mut group,
        level,
    );
    assert_eq!(
        calls,
        seq(&[&[Int(601), Int(5401), Double, Float, Bool], &AGEABLE, &MOB])
    );
    assert!(is_baby(entity_types::GOAT, &kid));
    let own = goat::state_of(&kid);
    assert!(!own.left_horn);
    assert!(own.right_horn);
}

#[test]
fn polar_bears_and_ocelots_after_the_first_are_all_young() {
    for kind in [entity_types::POLAR_BEAR, entity_types::OCELOT] {
        let mut group = None;
        let (first, _) = member(
            kind,
            "minecraft:snowy_plains",
            &mut group,
            Counting::default(),
        );
        let level = Counting::floats(&[1.0]);
        let (later, _) = member(kind, "minecraft:snowy_plains", &mut group, level);
        assert!(!is_baby(kind, &first));
        assert!(is_baby(kind, &later), "{kind}");
    }
}

#[test]
fn a_frog_draws_for_a_baby_of_its_group_but_is_never_one() {
    let mut group = None;
    member(
        entity_types::FROG,
        "minecraft:swamp",
        &mut group,
        Counting::default(),
    );
    let (frog, _) = member(
        entity_types::FROG,
        "minecraft:swamp",
        &mut group,
        Counting::floats(&[0.0]),
    );
    assert!(!is_baby(entity_types::FROG, &frog));
    assert_eq!(group.map(|mut group| group.ageable().made), Some(2));
}

#[test]
fn a_wolf_pack_shares_its_variant_and_has_no_pups() {
    let mut group = None;
    let (one, _) = member(
        entity_types::WOLF,
        "minecraft:taiga",
        &mut group,
        Counting::default(),
    );
    let (two, calls) = member(
        entity_types::WOLF,
        "minecraft:taiga",
        &mut group,
        Counting::floats(&[0.0]),
    );
    assert_eq!(calls, seq(&[&[Int(7)], &MOB]));
    assert_eq!(
        super::wolf::state_of(&one).1.variant,
        super::wolf::state_of(&two).1.variant
    );
    assert!(!is_baby(entity_types::WOLF, &two));
}

#[test]
fn an_egg_s_mob_draws_as_the_first_of_a_group_made_with_a_chunk() {
    for kind in [
        entity_types::SHEEP,
        entity_types::HORSE,
        entity_types::LLAMA,
        entity_types::RABBIT,
        entity_types::FOX,
        entity_types::WOLF,
        entity_types::GOAT,
        entity_types::CAT,
        entity_types::PANDA,
    ] {
        let biome = Some("minecraft:plains");
        let mut egg = Counting::default();
        made_with(
            kind,
            SpawnReason::SpawnEgg,
            biome,
            None,
            &mut None,
            &mut egg,
            |_| {},
        );
        let mut chunk = Counting::default();
        made_with(
            kind,
            SpawnReason::ChunkGeneration,
            biome,
            None,
            &mut None,
            &mut chunk,
            |_| {},
        );
        assert_eq!(egg.calls, chunk.calls, "{kind}");
    }
}

#[test]
fn a_swamp_hut_s_cat_is_all_black_of_one() {
    let mut level = Counting {
        ints: VecDeque::from([0]),
        ..Counting::default()
    };
    let cat = made_with(
        entity_types::CAT,
        SpawnReason::Structure,
        Some("minecraft:swamp"),
        Some("minecraft:swamp_hut"),
        &mut None,
        &mut level,
        |_| {},
    );
    assert_eq!(level.calls, seq(&[&MOB, &[Int(1), Int(2)]]));
    let all_black = animal::variants::id(cat::VARIANTS, "minecraft:all_black");
    assert_eq!(cat::state_of(&cat).1.variant, all_black);
}

/// A template mob's tag: its id, `PersistenceRequired` 1, and its follow range, with
/// the bonus or without.
fn template_tag(id: &str, bonus: bool, more: Vec<Vec<u8>>) -> Vec<u8> {
    let modifiers: &[(&str, f64, &str)] = if bonus {
        &[(RANDOM_SPAWN_BONUS, 0.15, "add_multiplied_base")]
    } else {
        &[]
    };
    let mut entries = vec![
        nbt::string("id", id),
        nbt::byte("PersistenceRequired", 1),
        nbt::compounds(
            "attributes",
            &[nbt::attribute("minecraft:follow_range", 16.0, modifiers)],
        ),
    ];
    entries.extend(more);
    nbt::compound(&entries)
}

/// A template's mob of `kind` from `tag`, finalised for `STRUCTURE`, and its draws.
fn from_template(kind: i32, tag: &[u8]) -> (Mob, Vec<Call>) {
    let mut level = Counting::default();
    let mob = made_with(
        kind,
        SpawnReason::Structure,
        Some("minecraft:plains"),
        Some("minecraft:village_plains"),
        &mut None,
        &mut level,
        |mob| super::from_template(kind as u16, mob, tag),
    );
    (mob, level.calls)
}

#[test]
fn a_template_s_cat_keeps_its_collar_and_draws_its_variant_again() {
    let more = || {
        vec![
            nbt::string("variant", "minecraft:calico"),
            nbt::byte("CollarColor", 5),
        ]
    };
    let calico = animal::variants::id(cat::VARIANTS, "minecraft:calico");
    let mut read = Mob::fresh();
    let tag = template_tag("minecraft:cat", false, more());
    super::from_template(entity_types::CAT as u16, &mut read, &tag);
    assert_eq!(cat::state_of(&read).1.variant, calico);

    let (cat_mob, calls) = from_template(entity_types::CAT, &tag);
    assert_eq!(calls, seq(&[&MOB, &[Int(11), Int(2)]]));
    let (_, own) = cat::state_of(&cat_mob);
    assert_eq!(own.collar, 5);
    // The draw of 0 of eleven is all black, the registry's first.
    assert_eq!(
        own.variant,
        animal::variants::id(cat::VARIANTS, "minecraft:all_black")
    );
    assert!(cat_mob.body.flags.persistent);

    // With the bonus, no triangle.
    let tag = template_tag("minecraft:cat", true, more());
    let (_, calls) = from_template(entity_types::CAT, &tag);
    assert_eq!(calls, vec![Float, Int(11), Int(2)]);
}

#[test]
fn a_template_s_cat_of_the_old_number_alone_is_read_by_it() {
    let tag = nbt::compound(&[nbt::int("CatType", 9)]);
    let mut mob = Mob::fresh();
    super::from_template(entity_types::CAT as u16, &mut mob, &tag);
    let jellie = animal::variants::id(cat::VARIANTS, "minecraft:jellie");
    assert_eq!(cat::state_of(&mob).1.variant, jellie);
}

#[test]
fn a_template_s_horse_draws_its_coat_and_health_again_and_skips_the_triangle() {
    let entries = vec![
        nbt::string("id", "minecraft:horse"),
        nbt::byte("PersistenceRequired", 1),
        nbt::int("Variant", 773),
        nbt::float("Health", 28.0),
        nbt::compounds(
            "attributes",
            &[
                nbt::attribute("minecraft:max_health", 28.0, &[]),
                nbt::attribute(
                    "minecraft:follow_range",
                    16.0,
                    &[(RANDOM_SPAWN_BONUS, 0.159, "add_multiplied_base")],
                ),
            ],
        ),
    ];
    let tag = nbt::compound(&entries);
    let kind = entity_types::HORSE as u16;

    let mut read = Mob::fresh();
    super::from_template(kind, &mut read, &tag);
    assert_eq!(equine::horse_of(&read).variant, 773);
    assert_eq!(read.body.vitals.health, 28.0);

    let (horse, calls) = from_template(entity_types::HORSE, &tag);
    assert_eq!(
        calls,
        seq(&[&[Int(7), Int(5), Int(8), Int(9)], &[Double; 6], &[Float]])
    );
    // A coat and markings of 0, and a health of 15 + 0 + 0, to which the template's 28
    // is held.
    assert_eq!(equine::horse_of(&horse).variant, 0);
    let max_health = attribute(clustine_data::attributes::MAX_HEALTH);
    assert_eq!(horse.body.attributes.value(kind, max_health), 15.0);
    assert_eq!(horse.body.vitals.health, 15.0);
    let follow = attribute(clustine_data::attributes::FOLLOW_RANGE);
    let modifiers = &horse.body.attributes.made(follow).expect("read").modifiers;
    assert_eq!(modifiers.len(), 1);
    assert_eq!(modifiers[0].amount, 0.159);
    assert!(horse.body.flags.persistent);
}

#[test]
fn a_template_s_sheep_reads_its_colour_which_its_finalize_spawn_draws_again() {
    let tag = template_tag("minecraft:sheep", true, vec![nbt::byte("Color", 15)]);
    let mut read = Mob::fresh();
    super::from_template(entity_types::SHEEP as u16, &mut read, &tag);
    let (_, own) = animal::read::<super::sheep::SheepState>(&read.state);
    assert_eq!(own.colour(), 15);

    let (sheep, calls) = from_template(entity_types::SHEEP, &tag);
    assert_eq!(calls, vec![Int(100), Float]);
    let (_, own) = animal::read::<super::sheep::SheepState>(&sheep.state);
    assert_eq!(
        own.colour(),
        super::sheep::spawn_colour(None, &mut Counting::default())
    );
}

#[test]
fn a_template_s_allay_with_its_bonus_draws_only_its_hand() {
    let tag = template_tag(
        "minecraft:allay",
        true,
        vec![nbt::nested("Brain", &[nbt::nested("memories", &[])])],
    );
    let (allay, calls) = from_template(entity_types::ALLAY, &tag);
    assert_eq!(calls, vec![Float]);
    assert!(allay.body.flags.persistent);
}

#[test]
fn every_group_of_a_creature_is_an_ageable_group() {
    let ageable = AgeableGroup::with_chance(0.2);
    for mut group in [
        SpawnGroup::Ageable(ageable),
        SpawnGroup::Horse { ageable, coat: 1 },
        SpawnGroup::Llama { ageable, coat: 1 },
        SpawnGroup::Rabbit {
            ageable,
            variant: 1,
        },
        SpawnGroup::Fox {
            ageable,
            variant: 1,
        },
        SpawnGroup::WolfPack {
            ageable,
            variant: 1,
        },
        SpawnGroup::Axolotl {
            ageable,
            variants: [0, 1],
        },
    ] {
        assert_eq!(*group.ageable(), ageable);
    }
}
