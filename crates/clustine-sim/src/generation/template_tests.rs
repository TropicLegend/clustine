//! Tests of the structures' entities as the simulation makes them
//! (`docs/groundwork/n7l-plan.md`, sections 3.4, 3.5 and 5.4; `docs/groundwork/n7-brief.md`,
//! sections 8.10 to 8.17): a template's entities from their tags, the pieces' mobs and
//! carts, what each draws of the run's region random, and a mob made with its mount.
//!
//! Those of types other parts are building (N7k's K1 to K5) are written now and ignored
//! until their types are built.

use std::collections::VecDeque;

use clustine_data::{BIOME_SPAWNS, entity_types, items};
use clustine_noise::{Random as _, Xoroshiro};
use clustine_world::generation::{Draws, FromNoise, Makings, Placed};
use clustine_world::{Biome, BlockPos, ChunkPos, Entity, EntityData, EntityId, Generated};

use super::template::{Beside, mob_of, parse, unit};
use super::{entity, mob, template_entity};
use crate::living::slots;
use crate::mobs::body::attributes::RANDOM_SPAWN_BONUS;
use crate::mobs::making;
use crate::mobs::types::template::build as nbt;
use crate::mobs::types::{self, CompanionSeat, SpawnReason, Spawning, animal, cat};
use crate::region::Random;

/// The world of these tests.
const WORLD: u64 = 77;

/// One call of the level's random, with its bound where it has one.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Call {
    Int(u32),
    Float,
    Double,
}

use Call::{Double, Float, Int};

/// `Mob.finalizeSpawn`'s draws without the follow range's bonus: the triangle's two
/// doubles, then the hand.
const MOB: [Call; 3] = [Double, Double, Float];

/// The level's random of a test: it records every call and answers from its script, or
/// with a middling value (an int of 0, a float or a double of a half).
#[derive(Default)]
struct Counting {
    calls: Vec<Call>,
    floats: VecDeque<f32>,
}

impl Draws for Counting {
    fn next_int(&mut self, bound: u32) -> u32 {
        self.calls.push(Int(bound));
        0
    }

    fn next_float(&mut self) -> f32 {
        self.calls.push(Float);
        self.floats.pop_front().unwrap_or(0.5)
    }

    fn next_double(&mut self) -> f64 {
        self.calls.push(Double);
        0.5
    }

    fn next_bool(&mut self) -> bool {
        unreachable!("no making here draws a boolean")
    }
}

/// The run's makings of the centre (4, −2).
fn makings() -> Makings {
    Makings::new(0, ChunkPos::new(4, -2))
}

/// The key of the run's `index`th making.
fn making(index: u16) -> EntityId {
    Generated {
        dimension: 0,
        chunk: ChunkPos::new(4, -2),
        structure: true,
        index,
    }
    .key()
    .expect("within the border")
}

/// The biome called `name`.
fn biome(name: &str) -> Biome {
    let at = BIOME_SPAWNS
        .iter()
        .position(|biome| biome.name == name)
        .unwrap_or_else(|| panic!("{name} is a biome"));
    Biome(u16::try_from(at).unwrap())
}

const AT: [f64; 3] = [70.5, 65.0, -27.25];

fn placed<'a>(
    makings: &'a mut Makings,
    level: &'a mut dyn Draws,
    yaw: f32,
    structure: Option<&'static str>,
) -> Placed<'a> {
    Placed {
        position: AT,
        yaw,
        pitch: -40.0,
        structure,
        biome: biome("minecraft:plains"),
        makings,
        level,
    }
}

/// A template mob's tag as a template keeps it once placed: its id, its rotation, its
/// persistence, and its follow range with the bonus or without; then `more`.
fn template_tag(id: &str, bonus: bool, more: Vec<Vec<u8>>) -> Vec<u8> {
    let modifiers: &[(&str, f64, &str)] = if bonus {
        &[(RANDOM_SPAWN_BONUS, 0.0412, "add_multiplied_base")]
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

/// What a template's entity of `tag` makes, finalised, and what it drew.
fn finalised(tag: &[u8]) -> (Vec<Entity>, Vec<Call>) {
    let mut makings = makings();
    let mut level = Counting::default();
    let made = template_entity(
        WORLD,
        tag,
        true,
        placed(&mut makings, &mut level, 30.0, None),
    );
    (made, level.calls)
}

#[test]
fn a_template_s_cow_is_made_where_it_was_placed_with_its_tag_read_drawing_what_the_brief_says() {
    let (made, calls) = finalised(&template_tag("minecraft:cow", true, Vec::new()));
    assert_eq!(made.len(), 1);
    let cow = &made[0];
    assert_eq!(cow.kind, entity_types::COW as u16);
    assert_eq!(cow.key, making(0), "named by its making");
    assert_eq!(cow.position, AT);
    assert_eq!(cow.yaw, 30.0);
    assert_eq!(cow.pitch, -40.0);
    assert_eq!(cow.motion, [0.0; 3]);
    assert!(!cow.on_ground);
    assert_eq!((cow.lived, cow.hops, cow.fresh_at), (0, 0, 0));
    assert!(cow.riding.is_free());
    let mob = mob_of(cow);
    assert!(
        mob.body.flags.persistent,
        "its template's PersistenceRequired"
    );
    assert_eq!(mob.body.head.head_yaw, 30.0);
    assert_eq!(mob.body.head.body_yaw, 30.0);
    // B 8.17: the variant of one, the sound of two, and with its bonus no triangle.
    assert_eq!(calls, [Int(1), Int(2), Float]);
}

#[test]
fn the_villages_animals_and_the_desert_camel_draw_from_the_region_as_the_brief_says() {
    // B 8.17, each with the default answers of the test's random: the village's cows,
    // pigs and sheep carry their bonus; a horse of `horses_1`; the calico cat has no
    // bonus, the British one has; the desert's camel is a tag of its id alone.
    let cases: [(&str, bool, Vec<Call>); 7] = [
        ("minecraft:cow", true, vec![Int(1), Int(2), Float]),
        ("minecraft:pig", true, vec![Int(1), Int(3), Float]),
        ("minecraft:sheep", true, vec![Int(100), Float]),
        (
            "minecraft:horse",
            true,
            vec![
                Int(7),
                Int(5),
                Int(8),
                Int(9),
                Double,
                Double,
                Double,
                Double,
                Double,
                Double,
                Float,
            ],
        ),
        (
            "minecraft:cat",
            false,
            [&MOB[..], &[Int(11), Int(2)]].concat(),
        ),
        ("minecraft:cat", true, vec![Float, Int(11), Int(2)]),
        ("minecraft:allay", true, vec![Float]),
    ];
    for (id, bonus, expected) in cases {
        let (made, calls) = finalised(&template_tag(id, bonus, Vec::new()));
        assert_eq!(made.len(), 1, "{id}");
        assert_eq!(calls, expected, "{id} with the bonus {bonus}");
    }
    let camel = nbt::compound(&[nbt::string("id", "minecraft:camel")]);
    let (made, calls) = finalised(&camel);
    assert_eq!(made.len(), 1);
    assert_eq!(made[0].kind, entity_types::CAMEL as u16);
    assert_eq!(calls, MOB);
}

#[test]
fn the_outpost_s_two_caged_allays_leave_the_region_random_past_two_floats() {
    let seed = 0x5eed;
    let mut region = Xoroshiro::from_seed(seed);
    let mut makings = makings();
    let allay = template_tag("minecraft:allay", true, Vec::new());
    for _ in 0..2 {
        let made = template_entity(
            WORLD,
            &allay,
            true,
            Placed {
                position: AT,
                yaw: 270.0,
                pitch: 40.0,
                structure: None,
                biome: biome("minecraft:plains"),
                makings: &mut makings,
                level: &mut FromNoise(&mut region),
            },
        );
        assert_eq!(made.len(), 1);
        assert_eq!(made[0].kind, entity_types::ALLAY as u16);
    }
    let mut expected = Xoroshiro::from_seed(seed);
    expected.next_f32();
    expected.next_f32();
    assert_eq!(region.next_i64(), expected.next_i64());
    assert_eq!(makings.taken(), 2);
}

#[test]
fn an_unfinalised_template_mob_draws_nothing_and_keeps_its_tag() {
    let tag = template_tag(
        "minecraft:cat",
        false,
        vec![
            nbt::string("variant", "minecraft:calico"),
            nbt::byte("CollarColor", 5),
        ],
    );
    let mut makings = makings();
    let mut level = Counting::default();
    let made = template_entity(
        WORLD,
        &tag,
        false,
        placed(&mut makings, &mut level, 0.0, None),
    );
    assert!(level.calls.is_empty());
    let (_, own) = cat::state_of(&mob_of(&made[0]));
    assert_eq!(
        own.variant,
        animal::variants::id(cat::VARIANTS, "minecraft:calico")
    );
    assert_eq!(own.collar, 5);
}

#[test]
fn a_camp_s_cushion_is_attached_to_its_moved_block_of_its_colour_and_draws_nothing() {
    let tag = nbt::compound(&[
        nbt::string("id", "minecraft:cushion"),
        nbt::string("color", "red"),
        nbt::entry(11, "block_pos", &{
            let mut payload = 3i32.to_be_bytes().to_vec();
            for value in [28, 64, -18] {
                payload.extend_from_slice(&i32::to_be_bytes(value));
            }
            payload
        }),
        nbt::entry(2, "Air", &300i16.to_be_bytes()),
    ]);
    let mut makings = makings();
    let mut level = Counting::default();
    let made = template_entity(
        WORLD,
        &tag,
        true,
        placed(&mut makings, &mut level, -360.0, None),
    );
    assert!(level.calls.is_empty(), "a cushion is no mob");
    assert_eq!(made.len(), 1);
    let cushion = &made[0];
    assert_eq!(cushion.kind, entity_types::CUSHION as u16);
    assert_eq!(cushion.yaw, -360.0);
    assert_eq!(cushion.key, making(0));
    assert_eq!(
        cushion.data,
        EntityData::Cushion {
            attached: clustine_world::Attached {
                block: BlockPos::new(28, 64, -18),
                direction: 0,
            },
            colour: 14,
        }
    );
}

#[test]
fn the_taiga_armourer_s_stand_wears_its_helmet_in_its_pose_and_draws_nothing() {
    let pose = |name: &str, angles: [f32; 3]| {
        let mut payload = vec![5u8];
        payload.extend_from_slice(&3i32.to_be_bytes());
        for angle in angles {
            payload.extend_from_slice(&angle.to_be_bytes());
        }
        nbt::entry(9, name, &payload)
    };
    let tag = nbt::compound(&[
        nbt::string("id", "minecraft:armor_stand"),
        nbt::nested(
            "Pose",
            &[
                pose("Head", [3.978_817, 1.545_424_5, 0.0]),
                pose("Body", [0.0, 1.466_972_4, 0.0]),
            ],
        ),
        nbt::nested(
            "equipment",
            &[nbt::nested(
                "head",
                &[
                    nbt::int("count", 1),
                    nbt::string("id", "minecraft:iron_helmet"),
                ],
            )],
        ),
        nbt::float("Health", 20.0),
        nbt::byte("ShowArms", 0),
        nbt::byte("Small", 0),
    ]);
    let mut makings = makings();
    let mut level = Counting::default();
    let made = template_entity(
        WORLD,
        &tag,
        true,
        placed(&mut makings, &mut level, 135.0, None),
    );
    assert!(level.calls.is_empty(), "a stand is no mob");
    let EntityData::ArmorStand {
        flags,
        pose,
        equipment,
        health,
        air,
        ..
    } = &made[0].data
    else {
        panic!("a stand is made: {:?}", made[0].data);
    };
    assert_eq!(*flags, 0);
    assert_eq!(
        equipment,
        &vec![(
            slots::HEAD,
            clustine_world::ItemStack::new(items::IRON_HELMET, 1)
        )]
    );
    assert_eq!(pose[0].map(|angle| angle.0), [3.978_817, 1.545_424_5, 0.0]);
    assert_eq!(pose[1].map(|angle| angle.0), [0.0, 1.466_972_4, 0.0]);
    // The arms and legs it does not name keep the stand's default pose.
    assert_eq!(pose[2].map(|angle| angle.0), crate::region::STAND_POSE[2]);
    assert_eq!(health.0, 20.0);
    assert_eq!(*air, 300);
    assert_eq!(made[0].yaw, 135.0);
}

#[test]
fn a_mineshaft_s_chest_minecart_is_an_empty_cart_and_nothing_else_is_made_so() {
    let mut makings = makings();
    let mut level = Counting::default();
    let cart = entity(
        WORLD,
        entity_types::CHEST_MINECART as u16,
        placed(&mut makings, &mut level, 0.0, None),
    )
    .expect("a cart is made");
    assert_eq!(cart.kind, entity_types::CHEST_MINECART as u16);
    assert_eq!(cart.position, AT);
    assert_eq!(cart.key, making(0));
    let EntityData::Minecart { contents, fuse, .. } = &cart.data else {
        panic!("a cart is made: {:?}", cart.data);
    };
    assert!(contents.is_empty());
    assert_eq!(*fuse, -1);
    assert!(level.calls.is_empty());
    let cow = entity(
        WORLD,
        entity_types::COW as u16,
        placed(&mut makings, &mut level, 0.0, None),
    );
    assert_eq!(cow, None);
}

#[test]
fn the_swamp_hut_s_cat_is_black_kept_and_drawn_for_as_the_hut_draws() {
    let mut makings = makings();
    let mut level = Counting::default();
    let made = mob(
        WORLD,
        entity_types::CAT as u16,
        true,
        placed(&mut makings, &mut level, 0.0, Some("minecraft:swamp_hut")),
    );
    assert_eq!(made.len(), 1);
    let cat_mob = mob_of(&made[0]);
    assert!(cat_mob.body.flags.persistent);
    assert_eq!(
        cat::state_of(&cat_mob).1.variant,
        animal::variants::id(cat::VARIANTS, "minecraft:all_black")
    );
    // The tails, then a variant of one and a sound of two (`swamp_hut.rs`).
    assert_eq!(level.calls, [&MOB[..], &[Int(1), Int(2)]].concat());
}

#[test]
fn a_type_not_built_makes_nothing_draws_nothing_and_takes_no_making() {
    // The shulker is the End's, which no part before the End's milestone builds.
    let mut makings = makings();
    let mut level = Counting::default();
    let made = mob(
        WORLD,
        entity_types::SHULKER as u16,
        true,
        placed(&mut makings, &mut level, 0.0, None),
    );
    assert!(made.is_empty());
    let tag = template_tag("minecraft:shulker", true, Vec::new());
    let made = template_entity(
        WORLD,
        &tag,
        true,
        placed(&mut makings, &mut level, 0.0, None),
    );
    assert!(made.is_empty());
    assert!(level.calls.is_empty());
    assert_eq!(makings.taken(), 0);
    // Nor does a tag that is not one, or names no type.
    let made = template_entity(
        WORLD,
        &[10, 0],
        true,
        placed(&mut makings, &mut level, 0.0, None),
    );
    assert!(made.is_empty());
    let made = template_entity(
        WORLD,
        &[1, 2],
        true,
        placed(&mut makings, &mut level, 0.0, None),
    );
    assert!(made.is_empty());
}

#[test]
fn the_same_structure_made_twice_makes_the_same_entities_and_another_making_another_uuid() {
    let tag = template_tag("minecraft:sheep", true, Vec::new());
    let make = |skip: usize| {
        let mut makings = makings();
        for _ in 0..skip {
            makings.next();
        }
        let mut region = Xoroshiro::from_seed(5);
        template_entity(
            WORLD,
            &tag,
            true,
            Placed {
                position: AT,
                yaw: 10.0,
                pitch: 0.0,
                structure: None,
                biome: biome("minecraft:plains"),
                makings: &mut makings,
                level: &mut FromNoise(&mut region),
            },
        )
    };
    let first = make(0);
    assert_eq!(make(0), first);
    let other = make(1);
    assert_ne!(other[0].uuid, first[0].uuid);
    assert_eq!(other[0].key, making(1));
}

#[test]
fn a_tag_is_read_whole_with_its_nesting_and_arrays() {
    let tag = nbt::compound(&[
        nbt::string("id", "minecraft:cow"),
        nbt::nested("inner", &[nbt::double("x", 1.5)]),
        nbt::entry(11, "ints", &{
            let mut payload = 2i32.to_be_bytes().to_vec();
            payload.extend_from_slice(&7i32.to_be_bytes());
            payload.extend_from_slice(&(-3i32).to_be_bytes());
            payload
        }),
    ]);
    let parsed = parse(&tag).expect("a whole tag");
    assert_eq!(
        parsed.get("inner").and_then(|inner| inner.get("x")),
        Some(&super::template::Tag::Number(1.5))
    );
    assert_eq!(
        parsed.get("ints"),
        Some(&super::template::Tag::List(vec![
            super::template::Tag::Number(7.0),
            super::template::Tag::Number(-3.0)
        ]))
    );
    // Cut short anywhere, it is no tag.
    for end in 1..tag.len() {
        assert_eq!(parse(&tag[..end]), None, "cut at {end}");
    }
}

/// A mob made in a run, with a chicken made beside it in its `finalizeSpawn` as a
/// zombie's is (`Zombie.finalizeSpawn`): the chicken, finalised for `JOCKEY` from the
/// same random, then the mob's draws after it, which change the mob.
#[test]
fn a_mount_made_in_a_making_comes_first_carrying_the_mob_as_it_was_when_the_mount_was_added() {
    let chicken = entity_types::CHICKEN as u16;
    let rider = entity_types::COW as u16;
    let mut makings = makings();
    let making_of_rider = makings.next().unwrap();
    let mut own = super::making_stream(WORLD, making_of_rider).unwrap();
    let (uuid, mut mob) = making::construct(rider, &mut own, 0.0);
    let mut level = Counting::default();
    let mut beside = Beside {
        world: WORLD,
        makings: &mut makings,
        made: Vec::new(),
    };
    let mut group = None;
    let mut spawning = Spawning {
        mob: &mut mob,
        level: &mut level,
        own: &mut own,
        biome: Some("minecraft:plains"),
        at: BlockPos::new(70, 65, -28),
        day: 0,
        game_time: 0,
        reason: SpawnReason::Structure,
        group: &mut group,
        structure: None,
        moon_brightness: 1.0,
        difficulty: (0.0, 0.0),
        companions: Some(&mut beside),
    };
    assert!(making::companion(
        &mut spawning,
        chicken,
        AT,
        0.0,
        (SpawnReason::Jockey, CompanionSeat::Vehicle),
        |_| {},
    ));
    let then = spawning.mob.clone();
    // What the mob's making does after its mount was added is drawn, and kept nowhere.
    spawning.mob.body.flags.left_handed = !then.body.flags.left_handed;
    let record = making::fresh_record(making(0), rider, uuid, AT, 0.0, &mob);
    // The chicken drew its own `finalizeSpawn`: a variant of one, a sound of two, the
    // ageable's nothing and the mob's tail.
    assert_eq!(level.calls, [&[Int(1), Int(2)][..], &MOB[..]].concat());
    let unit = unit(record, beside.made);
    assert_eq!(unit.len(), 2);
    let (mount, made) = (&unit[0], &unit[1]);
    assert_eq!(mount.kind, chicken);
    assert_eq!(mount.key, making(1), "the run's next making");
    assert_eq!(mount.position, AT, "where its rider stands, in its chunk");
    assert_eq!(mount.riding.passengers.len(), 1);
    assert_eq!(mount.riding.passengers[0].key, made.key);
    assert!(!mount.riding.passengers[0].player);
    assert_eq!(made.riding.vehicle, Some(mount.key));
    assert_eq!(mob_of(made), then, "as it was when its mount was added");
}

#[test]
fn a_run_s_side_of_a_companion_gives_the_next_making_and_finds_nothing_nearby() {
    let mut makings = makings();
    makings.next();
    let mut beside = Beside {
        world: WORLD,
        makings: &mut makings,
        made: Vec::new(),
    };
    let (key, stream) = types::Companions::next(&mut beside).unwrap();
    assert_eq!(key, making(1));
    assert_eq!(stream, Random::of_generated(WORLD, making(1)));
    let anywhere = crate::physics::aabb::Aabb::of_entity([0.0, 0.0, 0.0], 100.0, 100.0);
    assert!(types::Companions::nearby(&beside, &anywhere).is_empty());
}

// Types other parts are building: written now, run once each is registered.

#[test]

fn a_village_s_villager_leaves_the_region_random_past_the_mob_s_tail() {
    let villager = template_tag(
        "minecraft:villager",
        false,
        vec![nbt::nested(
            "VillagerData",
            &[
                nbt::string("type", "minecraft:plains"),
                nbt::string("profession", "minecraft:none"),
                nbt::int("level", 1),
            ],
        )],
    );
    let seed = 0xc0ffee;
    let mut region = Xoroshiro::from_seed(seed);
    let mut makings = makings();
    let made = template_entity(
        WORLD,
        &villager,
        true,
        Placed {
            position: AT,
            yaw: 48.821_632,
            pitch: 0.0,
            structure: None,
            biome: biome("minecraft:plains"),
            makings: &mut makings,
            level: &mut FromNoise(&mut region),
        },
    );
    assert_eq!(made.len(), 1);
    assert_eq!(made[0].kind, entity_types::VILLAGER as u16);
    // B 8.17: the triangle (no bonus in the village templates) and the hand.
    let mut expected = Xoroshiro::from_seed(seed);
    expected.next_f64();
    expected.next_f64();
    expected.next_f32();
    assert_eq!(region.next_i64(), expected.next_i64());
}

#[test]

fn the_igloo_s_cleric_and_its_zombie_draw_nothing_and_keep_their_tags() {
    let cleric = |id: &str, health: f32| {
        template_tag(
            id,
            true,
            vec![
                nbt::nested(
                    "VillagerData",
                    &[
                        nbt::string("type", "minecraft:plains"),
                        nbt::string("profession", "minecraft:cleric"),
                        nbt::int("level", 1),
                    ],
                ),
                nbt::float("Health", health),
            ],
        )
    };
    for (id, kind, health) in [
        ("minecraft:villager", entity_types::VILLAGER, 20.0),
        (
            "minecraft:zombie_villager",
            entity_types::ZOMBIE_VILLAGER,
            19.05,
        ),
    ] {
        let mut makings = makings();
        let mut level = Counting::default();
        let made = template_entity(
            WORLD,
            &cleric(id, health),
            false,
            placed(&mut makings, &mut level, 0.0, None),
        );
        assert_eq!(made.len(), 1, "{id}");
        assert_eq!(made[0].kind, kind as u16);
        assert!(level.calls.is_empty(), "{id} drew {:?}", level.calls);
        let mob = mob_of(&made[0]);
        assert!(mob.body.flags.persistent);
        assert_eq!(mob.body.vitals.health, health);
    }
}

#[test]

fn the_swamp_hut_s_witch_is_kept_and_draws_the_mob_s_tail() {
    let mut makings = makings();
    let mut level = Counting::default();
    let made = mob(
        WORLD,
        entity_types::WITCH as u16,
        true,
        placed(&mut makings, &mut level, 0.0, Some("minecraft:swamp_hut")),
    );
    assert_eq!(made.len(), 1);
    assert!(mob_of(&made[0]).body.flags.persistent);
    assert_eq!(level.calls, MOB);
}

#[test]

fn the_monument_s_elder_guardians_draw_the_mob_s_tail() {
    let mut makings = makings();
    let mut level = Counting::default();
    let made = mob(
        WORLD,
        entity_types::ELDER_GUARDIAN as u16,
        false,
        placed(
            &mut makings,
            &mut level,
            0.0,
            Some("minecraft:ocean_monument"),
        ),
    );
    assert_eq!(made.len(), 1);
    // Its constructor keeps it.
    assert!(mob_of(&made[0]).body.flags.persistent);
    assert_eq!(level.calls, MOB);
}

#[test]

fn the_mansion_s_illagers_draw_as_the_mansion_draws() {
    for (kind, expected) in [
        (entity_types::EVOKER, MOB.to_vec()),
        // The axe's enchantment, a float.
        (entity_types::VINDICATOR, [&MOB[..], &[Float]].concat()),
    ] {
        let mut makings = makings();
        let mut level = Counting::default();
        let made = mob(
            WORLD,
            kind as u16,
            true,
            placed(
                &mut makings,
                &mut level,
                0.0,
                Some("minecraft:woodland_mansion"),
            ),
        );
        assert_eq!(made.len(), 1);
        assert_eq!(level.calls, expected, "{kind}");
    }
}

#[test]

fn the_ocean_ruins_drowned_draws_as_the_ruin_draws() {
    let mut makings = makings();
    let mut level = Counting::default();
    let made = mob(
        WORLD,
        entity_types::DROWNED as u16,
        true,
        placed(&mut makings, &mut level, 0.0, None),
    );
    assert_eq!(made.len(), 1);
    // `ocean_ruin.rs`'s `drowned_draws` with every float a half: the tail, the loot, the
    // baby, the doors, the hand's item, the nautilus shell.
    assert_eq!(
        level.calls,
        [&MOB[..], &[Float, Float, Float, Float, Float]].concat()
    );
}

#[test]

fn the_outpost_s_caged_golem_draws_one_float_and_may_despawn() {
    // The cage's golem is saved with `PersistenceRequired` 0, and its bonus.
    let tag = nbt::compound(&[
        nbt::string("id", "minecraft:iron_golem"),
        nbt::byte("PersistenceRequired", 0),
        nbt::compounds(
            "attributes",
            &[nbt::attribute(
                "minecraft:follow_range",
                16.0,
                &[(RANDOM_SPAWN_BONUS, 0.0412, "add_multiplied_base")],
            )],
        ),
    ]);
    let (made, calls) = finalised(&tag);
    assert_eq!(made.len(), 1);
    assert!(!mob_of(&made[0]).body.flags.persistent);
    assert_eq!(calls, [Float]);
}

#[test]

fn a_baby_zombie_villager_and_the_chicken_made_for_it_are_one_unit_in_the_chicken_s_chunk() {
    let tag = template_tag("minecraft:zombie_villager", false, Vec::new());
    let mut makings = makings();
    // The hand, the loot, a baby (below 0.05), no chicken nearby to look for, and then
    // one made (below 0.05); every other float a half.
    let mut level = Counting {
        floats: VecDeque::from([0.5, 0.5, 0.0, 0.5, 0.0]),
        ..Counting::default()
    };
    let made = template_entity(
        WORLD,
        &tag,
        true,
        placed(&mut makings, &mut level, 0.0, None),
    );
    assert_eq!(made.len(), 2);
    let (chicken, zombie) = (&made[0], &made[1]);
    assert_eq!(chicken.kind, entity_types::CHICKEN as u16);
    assert_eq!(zombie.kind, entity_types::ZOMBIE_VILLAGER as u16);
    assert_eq!(chicken.riding.passengers[0].key, zombie.key);
    assert_eq!(zombie.riding.vehicle, Some(chicken.key));
    assert_eq!(chicken.chunk(), zombie.chunk());
    assert_eq!(makings.taken(), 2);
}
