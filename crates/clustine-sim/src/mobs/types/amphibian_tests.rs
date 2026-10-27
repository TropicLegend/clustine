//! The axolotl's, the frog's and the tadpole's own rules on their state alone, from the
//! brief's sections 7.4, 7.5, 9.7.2 and 10.7.2: their type states and what each shows
//! clients at which index, the draws of their `finalizeSpawn` in order, the play-dead
//! test's draws, the variants of a breeding, every interaction a player makes of them,
//! a tadpole's growing, and their sounds.

use clustine_blocks::data::item_in_tag;
use clustine_data::{EntityTypeInfo, attributes as ids, entity_types, items, sounds};
use clustine_world::{BlockPos, EntityId, ItemStack, TypeState};

use super::animal::{self, Animal, BABY_START_AGE};
use super::axolotl::{self, AxolotlState, Hurt};
use super::frog::{self, FrogState, Variant};
use super::tadpole::{self, TadpoleState};
use super::{Breeding, Did, Dimensions, Interaction, Spawning, Used, of};
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::body::attributes::supplied;
use crate::mobs::body::hurt::{DamageSource, DamageType};
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::region::Random;
use crate::shown::{DataShown, Holder};

const AXOLOTL: u16 = entity_types::AXOLOTL as u16;
const FROG: u16 = entity_types::FROG as u16;
const TADPOLE: u16 = entity_types::TADPOLE as u16;

fn draws(seed: u64) -> Random {
    Random::of_entity(seed, EntityId(21), 0)
}

fn one(item: i32) -> Option<ItemStack> {
    Some(ItemStack::new(item, 1))
}

/// A player's use of `mob`, of the type `kind`, holding `held`, at the mob's moment
/// `moment`, drawing from `stream`: what it answered and did.
fn use_with(
    kind: u16,
    mob: &mut Mob,
    held: Option<ItemStack>,
    moment: u32,
    stream: &mut Random,
) -> (Interaction, Vec<Did>) {
    let mut used = Used {
        key: EntityId(5),
        kind,
        mob,
        player: EntityId(9),
        player_uuid: Some(9),
        game_time: 0,
        held,
        creative: true,
        sneaking: false,
        carries: false,
        passengers: 0,
        on_ground: true,
        from: [0.0, 0.0, 2.0],
        position: [0.5, 0.0, 0.5],
        moment,
        draws: stream,
        did: Vec::new(),
    };
    let result = of(kind).mob_interact(&mut used);
    let did = std::mem::take(&mut used.did);
    (result, did)
}

fn sounds_of(did: &[Did]) -> Vec<i32> {
    did.iter()
        .filter_map(|done| match done {
            Did::Sound { sound, .. } => Some(*sound),
            _ => None,
        })
        .collect()
}

fn health(kind: u16) -> f64 {
    supplied(kind, attribute(ids::MAX_HEALTH)).unwrap()
}

#[test]
fn each_is_made_with_its_attributes_and_its_type_s_size() {
    // `createAttributes`: animal attributes (follow 16, tempt 10) and each its own.
    for (kind, max_health) in [(AXOLOTL, 14.0), (FROG, 10.0), (TADPOLE, 6.0)] {
        assert_eq!(health(kind), max_health, "{kind}");
        assert_eq!(supplied(kind, attribute(ids::FOLLOW_RANGE)), Some(16.0));
        assert_eq!(supplied(kind, attribute(ids::TEMPT_RANGE)), Some(10.0));
        assert_eq!(supplied(kind, attribute(ids::MOVEMENT_SPEED)), Some(1.0));
    }
    assert_eq!(supplied(AXOLOTL, attribute(ids::ATTACK_DAMAGE)), Some(2.0));
    assert_eq!(supplied(AXOLOTL, attribute(ids::STEP_HEIGHT)), Some(1.0));
    assert_eq!(supplied(FROG, attribute(ids::ATTACK_DAMAGE)), Some(10.0));
    assert_eq!(supplied(FROG, attribute(ids::STEP_HEIGHT)), Some(1.0));
    // `sized(0.75, 0.42)` and `0.2751`; `sized(0.5, 0.5)`; `sized(0.4, 0.3)` and
    // `0.19500001`.
    let info = EntityTypeInfo::of(entity_types::AXOLOTL);
    assert_eq!(
        (info.width, info.height, info.eye_height),
        (0.75, 0.42, 0.2751)
    );
    let info = EntityTypeInfo::of(entity_types::FROG);
    assert_eq!((info.width, info.height), (0.5, 0.5));
    let info = EntityTypeInfo::of(entity_types::TADPOLE);
    assert_eq!(
        (info.width, info.height, info.eye_height),
        (0.4, 0.3, 0.195_000_01)
    );
    // Six thousand ticks of air for an axolotl, a fish's three hundred for a tadpole.
    assert_eq!(of(AXOLOTL).max_air(), 6000);
    assert_eq!(of(TADPOLE).max_air(), 300);
    assert_eq!(of(FROG).max_air(), 300);
    // None is pushed by water.
    for kind in [AXOLOTL, FROG, TADPOLE] {
        assert!(!of(kind).pushed_by_fluid());
    }
}

#[test]
fn a_baby_axolotl_is_half_its_size_with_its_eyes_at_0_09375_and_a_frog_is_never_a_baby() {
    let mut mob = Mob::fresh();
    assert_eq!(of(AXOLOTL).dimensions(&mob), None);
    animal::made_baby(&mut mob);
    assert!(of(AXOLOTL).is_baby(&mob));
    assert_eq!(
        of(AXOLOTL).dimensions(&mob),
        Some(Dimensions {
            width: 0.375,
            height: 0.21,
            eye_height: 0.093_75,
        })
    );
    assert!(of(AXOLOTL).can_be_a_baby());
    assert!(!of(FROG).can_be_a_baby());
    assert!(of(TADPOLE).can_be_a_baby() && !of(TADPOLE).is_baby(&Mob::fresh()));
}

#[test]
fn a_fresh_axolotl_keeps_no_state_and_its_own_part_follows_what_every_animal_shares() {
    assert_eq!(Mob::fresh().state, TypeState::default());
    assert_eq!(axolotl::read(&Mob::fresh()), AxolotlState::default());
    let mut mob = Mob::fresh();
    axolotl::update(&mut mob, |own| {
        own.variant = axolotl::GOLD;
        own.from_bucket = true;
    });
    assert_eq!(mob.state.version, 1, "an animal's version");
    let (shared, own) = animal::read::<AxolotlState>(&mob.state);
    assert_eq!(shared, Animal::default());
    assert_eq!(
        own,
        AxolotlState {
            variant: axolotl::GOLD,
            playing_dead: false,
            from_bucket: true,
        }
    );
    // Back to fresh, nothing is kept.
    axolotl::update(&mut mob, |own| *own = AxolotlState::default());
    assert_eq!(mob.state, TypeState::default());
}

#[test]
fn a_frog_s_variant_is_kept_by_a_number_of_clustine_s_own_and_shown_by_the_registry_s_id() {
    // Clustine's numbers, fixed for good: temperate 0, warm 1, cold 2.
    for (variant, byte) in [
        (Variant::Temperate, 0),
        (Variant::Warm, 1),
        (Variant::Cold, 2),
    ] {
        assert_eq!(postcard::to_allocvec(&variant).unwrap(), [byte]);
    }
    // The registry the server sends: cold, temperate, warm.
    assert_eq!(Variant::Cold.id(), 0);
    assert_eq!(Variant::Temperate.id(), 1);
    assert_eq!(Variant::Warm.id(), 2);
    let mut mob = Mob::fresh();
    frog::update(&mut mob, |own| own.variant = Variant::Temperate);
    assert_eq!(mob.state, TypeState::default(), "temperate is fresh");
    frog::update(&mut mob, |own| own.variant = Variant::Cold);
    assert_eq!(
        animal::read::<FrogState>(&mob.state).1.variant,
        Variant::Cold
    );
}

#[test]
fn a_tadpole_s_state_is_its_own_of_version_130_and_its_age_counts_up_unless_locked() {
    let fresh = TadpoleState::default();
    assert_eq!(tadpole::write(&fresh), TypeState::default());
    let mut grown = fresh.clone();
    assert!(!grown.set_age(1200, 50));
    let state = tadpole::write(&grown);
    assert_eq!(state.version, 130);
    assert_eq!(tadpole::read(&state), grown);
    // An animal's record is not read as a tadpole's.
    let mut cow = Mob::fresh();
    animal::made_baby(&mut cow);
    assert_eq!(tadpole::read(&cow.state), TadpoleState::default());
    // By one a tick from its stamp: 1200 at 50, 1300 at 150; and 24000 grows it up.
    assert_eq!(grown.age(50), 1200);
    assert_eq!(grown.age(150), 1300);
    assert_eq!(grown.ticks_left(150), 22_700);
    assert_eq!(grown.age(50 + 22_800), 24_000);
    assert_eq!(grown.ticks_left(50 + 30_000), 0);
    assert!(grown.set_age(24_000, 7));
    // Locked, it waits.
    let mut locked = TadpoleState {
        age_locked: true,
        ..TadpoleState::default()
    };
    locked.set_age(0, 10);
    assert_eq!(locked.age(10_000), 0);
}

#[test]
fn an_axolotl_shows_its_variant_its_playing_dead_and_its_bucket_at_18_to_20() {
    let module = of(AXOLOTL);
    let fresh = Mob::fresh();
    assert_eq!(
        module.shown(&fresh),
        [
            (16, DataShown::Boolean(false), true),
            (17, DataShown::Boolean(false), true),
            (18, DataShown::Int(0), true),
            (19, DataShown::Boolean(false), true),
            (20, DataShown::Boolean(false), true),
        ]
    );
    let mut mob = Mob::fresh();
    animal::made_baby(&mut mob);
    axolotl::update(&mut mob, |own| {
        own.variant = axolotl::BLUE;
        own.playing_dead = true;
        own.from_bucket = true;
    });
    assert_eq!(
        module.shown(&mob),
        [
            (16, DataShown::Boolean(true), false),
            (17, DataShown::Boolean(false), true),
            (18, DataShown::Int(4), false),
            (19, DataShown::Boolean(true), false),
            (20, DataShown::Boolean(true), false),
        ]
    );
    // An id out of range shows as lucy (`byId`'s `ZERO`).
    axolotl::update(&mut mob, |own| own.variant = 9);
    assert!(module.shown(&mob).contains(&(18, DataShown::Int(0), true)));
    // Its air is shown against its own most, 6000.
    let mut full = Mob::fresh();
    full.body.vitals.air = 6000;
    full.body.vitals.health = 14.0;
    let values = crate::shown::mob_values(AXOLOTL, &full.encode());
    assert!(values.contains(&(1, DataShown::Int(6000), true)));
}

#[test]
fn a_frog_shows_its_pose_at_6_and_its_variant_of_the_registry_at_18_temperate_by_default() {
    let module = of(FROG);
    let temperate = Variant::Temperate.id();
    assert_eq!(
        module.shown(&Mob::fresh()),
        [
            (16, DataShown::Boolean(false), true),
            (17, DataShown::Boolean(false), true),
            (6, DataShown::Pose(0), true),
            (18, DataShown::Holder(Holder::FrogVariant, temperate), true),
        ]
    );
    let mut mob = Mob::fresh();
    frog::update(&mut mob, |own| own.variant = Variant::Warm);
    assert!(module.shown(&mob).contains(&(
        18,
        DataShown::Holder(Holder::FrogVariant, Variant::Warm.id()),
        false
    )));
    // Its poses by their ordinals: croaking 8, long jumping 6, using its tongue 9.
    for (pose, ordinal) in [
        (frog::Pose::Croaking, 8),
        (frog::Pose::LongJumping, 6),
        (frog::Pose::UsingTongue, 9),
    ] {
        frog::update(&mut mob, |own| own.pose = pose);
        assert!(
            module
                .shown(&mob)
                .contains(&(6, DataShown::Pose(ordinal), false))
        );
    }
}

#[test]
fn a_tadpole_shows_from_bucket_never_set_at_16_and_its_age_lock_at_17() {
    let module = of(TADPOLE);
    assert_eq!(
        module.shown(&Mob::fresh()),
        [
            (16, DataShown::Boolean(false), true),
            (17, DataShown::Boolean(false), true),
        ]
    );
    let mut mob = Mob::fresh();
    tadpole::update(&mut mob, |own| own.age_locked = true);
    assert_eq!(
        module.shown(&mob),
        [
            (16, DataShown::Boolean(false), true),
            (17, DataShown::Boolean(true), false),
        ]
    );
}

#[test]
fn an_egg_s_axolotl_draws_its_group_s_two_colours_then_its_own_of_them_and_nothing_more() {
    let mut seen = [0; 5];
    for seed in 0..2000 {
        let mut level = draws(seed);
        let mut replay = level.clone();
        let mut mob = Mob::fresh();
        of(AXOLOTL).finalize_spawn(&mut Spawning {
            reason: crate::mobs::types::SpawnReason::SpawnEgg,
            group: &mut None,
            structure: None,
            moon_brightness: 1.0,
            difficulty: (0.0, 0.0),
            companions: None,
            game_time: 0,
            mob: &mut mob,
            level: &mut level,
            own: &mut draws(99),
            biome: None,
            at: BlockPos::new(0, -60, 0),
            day: 0,
        });
        let first = replay.next_int(4) as i32;
        let second = replay.next_int(4) as i32;
        let expected = [first, second][replay.next_int(2) as usize];
        let own = axolotl::read(&mob);
        assert_eq!(own.variant, expected);
        assert_eq!(level, replay, "three draws");
        assert!(!animal::is_baby(&mob), "the first of a group is grown");
        seen[own.variant as usize] += 1;
    }
    // Each common colour about a quarter of the time; blue never.
    assert!(
        seen[..4].iter().all(|&n| (400..600).contains(&n)),
        "{seen:?}"
    );
    assert_eq!(seen[4], 0);
    // A bucket's skips it whole, `Mob`'s too.
    assert!(!of(AXOLOTL).finalizes_when_poured());
    assert!(of(TADPOLE).finalizes_when_poured() && of(FROG).finalizes_when_poured());
}

#[test]
fn an_egg_s_frog_is_of_its_biome_s_variant_by_one_draw_then_draws_its_long_jump_s_cooldown() {
    for (biome, expected) in [
        (Some("minecraft:swamp"), Variant::Temperate),
        (Some("minecraft:plains"), Variant::Temperate),
        (Some("minecraft:desert"), Variant::Warm),
        (Some("minecraft:mangrove_swamp"), Variant::Warm),
        (Some("minecraft:jungle"), Variant::Warm),
        (Some("minecraft:snowy_plains"), Variant::Cold),
        (Some("minecraft:frozen_river"), Variant::Cold),
        (Some("minecraft:deep_dark"), Variant::Cold),
        (None, Variant::Temperate),
    ] {
        let mut level = draws(4);
        let mut replay = level.clone();
        let mut mob = Mob::fresh();
        of(FROG).finalize_spawn(&mut Spawning {
            reason: crate::mobs::types::SpawnReason::SpawnEgg,
            group: &mut None,
            structure: None,
            moon_brightness: 1.0,
            difficulty: (0.0, 0.0),
            companions: None,
            game_time: 0,
            mob: &mut mob,
            level: &mut level,
            own: &mut draws(99),
            biome,
            at: BlockPos::new(0, -60, 0),
            day: 0,
        });
        assert_eq!(frog::read(&mob).variant, expected, "{biome:?}");
        // `getRandomSafe` of one, then `UniformInt(100, 140)`.
        assert_eq!(replay.next_int(1), 0);
        let cooldown = 100 + replay.next_int(41);
        assert!((100..=140).contains(&cooldown));
        assert_eq!(level, replay, "{biome:?}: two draws");
        assert_eq!(animal::animal_of(&mob.state).age(0), 0, "no baby draw");
    }
}

#[test]
fn a_tadpole_s_egg_draws_nothing_of_its_own() {
    let mut level = draws(5);
    let replay = level.clone();
    let mut mob = Mob::fresh();
    let mut own = draws(99);
    let mut spawning = Spawning {
        reason: crate::mobs::types::SpawnReason::SpawnEgg,
        group: &mut None,
        structure: None,
        moon_brightness: 1.0,
        difficulty: (0.0, 0.0),
        companions: None,
        game_time: 0,
        mob: &mut mob,
        level: &mut level,
        own: &mut own,
        biome: Some("minecraft:desert"),
        at: BlockPos::new(0, -60, 0),
        day: 0,
    };
    of(TADPOLE).finalize_spawn(&mut spawning);
    of(TADPOLE).after_finalize_spawn(&mut spawning);
    assert_eq!(level, replay);
    assert_eq!(mob.state, TypeState::default());
}

#[test]
fn a_hurt_draws_one_in_three_for_playing_dead_and_a_second_only_after_a_nought() {
    let hurt = Hurt {
        no_ai: false,
        amount: 3.0,
        health: 14.0,
        max_health: 14.0,
        in_water: true,
        by_entity: true,
        playing_dead: false,
    };
    let mut plays = 0;
    for seed in 0..3000 {
        let mut stream = draws(seed);
        let mut replay = stream.clone();
        let played = axolotl::plays_dead(&mut stream, &hurt);
        let first = replay.next_int(3);
        if first == 0 {
            // A hurt of 3 beats every second draw of nought to two.
            let second = replay.next_int(3) as f32;
            assert!(second < 3.0);
            assert!(played);
            plays += 1;
        } else {
            assert!(!played);
        }
        assert_eq!(stream, replay);
    }
    assert!((850..1150).contains(&plays), "{plays}");
    // A light hurt at full health: only when the second draw is below it.
    let light = Hurt {
        amount: 1.0,
        ..hurt
    };
    for seed in 0..300 {
        let mut stream = draws(seed);
        let mut replay = stream.clone();
        let played = axolotl::plays_dead(&mut stream, &light);
        let expected = replay.next_int(3) == 0 && replay.next_int(3) == 0;
        assert_eq!(played, expected);
    }
    // Out of water, by nothing, playing dead already or a hurt past its health: the
    // draws all the same, and no play.
    for other in [
        Hurt {
            in_water: false,
            ..hurt
        },
        Hurt {
            by_entity: false,
            ..hurt
        },
        Hurt {
            playing_dead: true,
            ..hurt
        },
        Hurt {
            amount: 20.0,
            ..hurt
        },
    ] {
        for seed in 0..100 {
            let mut stream = draws(seed);
            let mut replay = stream.clone();
            assert!(!axolotl::plays_dead(&mut stream, &other));
            if replay.next_int(3) == 0 {
                replay.next_int(3);
            }
            assert_eq!(stream, replay);
        }
    }
    // Without AI, nothing is drawn.
    let mut stream = draws(1);
    let replay = stream.clone();
    assert!(!axolotl::plays_dead(
        &mut stream,
        &Hurt {
            no_ai: true,
            ..hurt
        }
    ));
    assert_eq!(stream, replay);
}

#[test]
fn a_bred_axolotl_is_blue_one_time_in_1200_else_of_either_parent_by_a_coin() {
    let (mut this, mut partner, mut blue) = (0, 0, 0);
    for seed in 0..12_000 {
        let mut stream = draws(seed);
        let mut replay = stream.clone();
        let variant = axolotl::offspring_variant(&mut stream, axolotl::GOLD, axolotl::CYAN);
        let expected = if replay.next_int(1200) == 0 {
            assert_eq!(replay.next_int(1), 0);
            axolotl::BLUE
        } else if replay.next_bool() {
            axolotl::GOLD
        } else {
            axolotl::CYAN
        };
        assert_eq!(variant, expected);
        assert_eq!(stream, replay);
        match variant {
            axolotl::GOLD => this += 1,
            axolotl::CYAN => partner += 1,
            _ => blue += 1,
        }
    }
    assert!((1..=25).contains(&blue), "{blue}");
    assert!((5600..6400).contains(&this) && (5600..6400).contains(&partner));
    // The child is kept for good, of that variant.
    let mut parent = Mob::fresh();
    axolotl::update(&mut parent, |own| own.variant = axolotl::WILD);
    let mut stream = draws(3);
    let mut replay = stream.clone();
    let child = of(AXOLOTL)
        .breed_offspring(&mut Breeding {
            this: &parent,
            partner: &parent,
            partner_kind: AXOLOTL,
            child_kind: None,
            draws: &mut stream,
            biome: None,
            egg_user: None,
        })
        .unwrap();
    assert!(child.body.flags.persistent);
    let expected = axolotl::offspring_variant(&mut replay, axolotl::WILD, axolotl::WILD);
    assert_eq!(axolotl::read(&child).variant, expected);
}

#[test]
fn an_egg_used_on_a_frog_makes_a_fresh_temperate_one_with_its_cooldown_drawn() {
    let mut parent = Mob::fresh();
    frog::update(&mut parent, |own| own.variant = Variant::Cold);
    let mut stream = draws(8);
    let mut replay = stream.clone();
    let child = of(FROG)
        .breed_offspring(&mut Breeding {
            this: &parent,
            partner: &parent,
            partner_kind: FROG,
            child_kind: None,
            draws: &mut stream,
            biome: Some("minecraft:snowy_plains"),
            egg_user: None,
        })
        .unwrap();
    assert_eq!(frog::read(&child).variant, Variant::Temperate);
    replay.next_int(41);
    assert_eq!(stream, replay);
    // A frog's breeding makes no child: it is the one that bred that is changed.
    assert!(of(FROG).bred_without_child(&parent).is_some());
    assert!(of(AXOLOTL).bred_without_child(&parent).is_none());
}

#[test]
fn a_water_bucket_takes_an_axolotl_or_a_tadpole_with_its_own_filling_sound() {
    for (kind, sound, bucket) in [
        (
            AXOLOTL,
            sounds::ITEM_BUCKET_FILL_AXOLOTL,
            items::AXOLOTL_BUCKET,
        ),
        (
            TADPOLE,
            sounds::ITEM_BUCKET_FILL_TADPOLE,
            items::TADPOLE_BUCKET,
        ),
    ] {
        let mut mob = Mob::fresh();
        mob.body.vitals.health = 5.0;
        let mut stream = draws(1);
        let (result, did) = use_with(kind, &mut mob, one(items::WATER_BUCKET), 0, &mut stream);
        assert_eq!(result, Interaction::Success);
        assert_eq!(
            did,
            [
                Did::Sound {
                    sound,
                    source: SOUND_SOURCE_NEUTRAL,
                    at: [0.5, 0.0, 0.5],
                    volume: 1.0,
                    pitch: 1.0,
                    but_the_player: false,
                },
                Did::Give {
                    stack: ItemStack::new(bucket, 1),
                    no_duplicates: false,
                },
                Did::Discard,
            ]
        );
    }
    // Not the frog.
    let mut mob = Mob::fresh();
    let (result, did) = use_with(FROG, &mut mob, one(items::WATER_BUCKET), 0, &mut draws(1));
    assert_eq!(result, Interaction::Pass);
    assert!(did.is_empty());
}

#[test]
fn a_bucket_of_tropical_fish_puts_an_axolotl_in_love_and_gives_back_a_water_bucket() {
    assert!(item_in_tag(
        items::TROPICAL_FISH_BUCKET,
        "minecraft:axolotl_food"
    ));
    assert!(!item_in_tag(items::TROPICAL_FISH, "minecraft:axolotl_food"));
    let mut mob = Mob::fresh();
    let (result, did) = use_with(
        AXOLOTL,
        &mut mob,
        one(items::TROPICAL_FISH_BUCKET),
        100,
        &mut draws(1),
    );
    assert_eq!(result, Interaction::SuccessServer);
    assert_eq!(
        did,
        [
            Did::Give {
                stack: ItemStack::new(items::WATER_BUCKET, 1),
                no_duplicates: true,
            },
            Did::Event(18),
        ]
    );
    let shared = animal::animal_of(&mob.state);
    assert!(shared.in_love(100) && shared.love(100) == 600);
    // In love already, a grown one takes no more.
    let (result, did) = use_with(
        AXOLOTL,
        &mut mob,
        one(items::TROPICAL_FISH_BUCKET),
        101,
        &mut draws(1),
    );
    assert_eq!(result, Interaction::Pass);
    assert!(did.is_empty());
    // A baby grows by a tenth of its time, and is given a water bucket too.
    let mut baby = Mob::fresh();
    animal::made_baby(&mut baby);
    let (result, did) = use_with(
        AXOLOTL,
        &mut baby,
        one(items::TROPICAL_FISH_BUCKET),
        0,
        &mut draws(1),
    );
    assert_eq!(result, Interaction::Success);
    assert!(matches!(did[..], [Did::Give { .. }]));
    assert_eq!(animal::animal_of(&baby.state).age(0), BABY_START_AGE + 2400);
}

#[test]
fn a_slime_ball_puts_a_frog_in_love_with_its_eating_sound_at_twice_the_volume() {
    assert!(item_in_tag(items::SLIME_BALL, "minecraft:frog_food"));
    let mut mob = Mob::fresh();
    let mut stream = draws(1);
    let replay = stream.clone();
    let (result, did) = use_with(FROG, &mut mob, one(items::SLIME_BALL), 7, &mut stream);
    assert_eq!(result, Interaction::SuccessServer);
    assert_eq!(
        did,
        [
            Did::Event(18),
            Did::Sound {
                sound: sounds::ENTITY_FROG_EAT,
                source: SOUND_SOURCE_NEUTRAL,
                at: [0.5, 0.0, 0.5],
                volume: 2.0,
                pitch: 1.0,
                but_the_player: false,
            },
        ]
    );
    assert_eq!(stream, replay, "its eating sound draws no pitch");
    assert!(animal::animal_of(&mob.state).in_love(7));
    // Anything else passes.
    let (result, _) = use_with(FROG, &mut Mob::fresh(), one(items::WHEAT), 7, &mut draws(1));
    assert_eq!(result, Interaction::Pass);
}

#[test]
fn a_slime_ball_takes_a_tenth_of_a_tadpole_s_time_left_in_whole_seconds() {
    let mut mob = Mob::fresh();
    let mut stream = draws(2);
    let mut replay = stream.clone();
    let (result, did) = use_with(TADPOLE, &mut mob, one(items::SLIME_BALL), 40, &mut stream);
    assert_eq!(result, Interaction::Success);
    assert!(did.is_empty());
    // At 40 of age 40: (23960 / 20) · 0.1 = 119 seconds, 2380 ticks.
    let own = tadpole::read(&mob.state);
    assert_eq!(own.age(40), 40 + 2380);
    // The particle's three draws, nothing sent.
    for _ in 0..3 {
        replay.next_double();
    }
    assert_eq!(stream, replay);
    // Near the end: 19 ticks left is no second, nothing gained; 20 is one.
    let mut near = Mob::fresh();
    tadpole::update(&mut near, |own| {
        own.set_age(23_981, 0);
    });
    use_with(TADPOLE, &mut near, one(items::SLIME_BALL), 0, &mut draws(2));
    assert_eq!(tadpole::read(&near.state).age(0), 23_981);
    // With 200 left, a tenth of 10 seconds is one, 20 ticks.
    let mut last = Mob::fresh();
    tadpole::update(&mut last, |own| {
        own.set_age(23_800, 0);
    });
    use_with(TADPOLE, &mut last, one(items::SLIME_BALL), 0, &mut draws(2));
    assert_eq!(tadpole::read(&last.state).age(0), 23_820);
    // One that reaches 24000 grows up there and then.
    let mut ready = Mob::fresh();
    tadpole::update(&mut ready, |own| {
        own.set_age(TICKS_TO_BE_FROG - 20, 0);
    });
    let (_, did) = use_with(
        TADPOLE,
        &mut ready,
        one(items::SLIME_BALL),
        0,
        &mut draws(2),
    );
    assert_eq!(tadpole::read(&ready.state).age(0), TICKS_TO_BE_FROG - 20);
    assert!(did.is_empty(), "a second's tenth is nothing");
}

const TICKS_TO_BE_FROG: i32 = tadpole::TICKS_TO_BE_FROG;

#[test]
fn feeding_never_takes_a_tadpole_to_its_end_but_one_whose_age_is_past_it_grows_up_at_once() {
    // From nothing, each feed takes a tenth of what is left in whole seconds.
    let mut mob = Mob::fresh();
    let mut fed = 0;
    let mut grew = false;
    while !grew && fed < 200 {
        let (_, did) = use_with(TADPOLE, &mut mob, one(items::SLIME_BALL), 0, &mut draws(2));
        fed += 1;
        grew = did.contains(&Did::GrowUp);
    }
    // A tenth of what is left never reaches all of it: whole seconds are taken off
    // until fewer than 200 ticks are left, after which a feed gains nothing.
    assert!(!grew, "{fed}");
    let age = tadpole::read(&mob.state).age(0);
    assert!(age > 23_800 && age < TICKS_TO_BE_FROG, "{age}");
    // So only its own ticks grow it up; a feed grows one up only where its age was
    // set past the end some other way, as a bucket's `Age` would.
    let mut past = Mob::fresh();
    tadpole::update(&mut past, |own| {
        own.age = TICKS_TO_BE_FROG + 1000;
    });
    let (_, did) = use_with(TADPOLE, &mut past, one(items::SLIME_BALL), 0, &mut draws(2));
    assert_eq!(did, [Did::GrowUp]);
}

#[test]
fn a_golden_dandelion_locks_a_tadpole_s_age_at_nothing_and_a_slime_ball_then_does_nothing() {
    let mut mob = Mob::fresh();
    tadpole::update(&mut mob, |own| {
        own.set_age(5000, 0);
    });
    let (result, did) = use_with(
        TADPOLE,
        &mut mob,
        one(items::GOLDEN_DANDELION),
        100,
        &mut draws(1),
    );
    assert_eq!(result, Interaction::Success);
    assert_eq!(
        did,
        [Did::Sound {
            sound: sounds::ITEM_GOLDEN_DANDELION_USE,
            source: animal::SOUND_SOURCE_PLAYERS,
            at: [0.5, 0.5, 0.5],
            volume: 1.0,
            pitch: 1.0,
            but_the_player: false,
        }]
    );
    let own = tadpole::read(&mob.state);
    assert!(own.age_locked);
    assert_eq!(own.age(100), 0);
    assert_eq!(own.age(9_000), 0);
    assert!(mob.body.flags.persistent);
    // Locked, a slime ball is no food: it passes (a water bucket is not held).
    let (result, _) = use_with(
        TADPOLE,
        &mut mob,
        one(items::SLIME_BALL),
        120,
        &mut draws(1),
    );
    assert_eq!(result, Interaction::Pass);
    // Within its 40 ticks the dandelion does nothing; after, it unlocks.
    let (result, _) = use_with(
        TADPOLE,
        &mut mob,
        one(items::GOLDEN_DANDELION),
        139,
        &mut draws(1),
    );
    assert_eq!(result, Interaction::Pass);
    let (result, did) = use_with(
        TADPOLE,
        &mut mob,
        one(items::GOLDEN_DANDELION),
        140,
        &mut draws(1),
    );
    assert_eq!(result, Interaction::Success);
    assert_eq!(sounds_of(&did), [sounds::ITEM_GOLDEN_DANDELION_UNUSE]);
    let own = tadpole::read(&mob.state);
    assert!(!own.age_locked);
    assert_eq!(own.age(140), 0);
    assert_eq!(own.age(240), 100);
}

#[test]
fn each_sounds_as_its_class_says() {
    let fresh = Mob::fresh();
    let source = DamageSource::of(DamageType::FALL);
    let axolotl = of(AXOLOTL);
    assert_eq!(
        axolotl.ambient_sound(&fresh),
        Some(sounds::ENTITY_AXOLOTL_IDLE_AIR)
    );
    assert_eq!(
        axolotl.ambient_sound_at(&fresh, true, false),
        Some(sounds::ENTITY_AXOLOTL_IDLE_WATER)
    );
    assert_eq!(
        axolotl.ambient_sound_at(&fresh, false, true),
        Some(sounds::ENTITY_AXOLOTL_IDLE_AIR)
    );
    assert_eq!(
        axolotl.hurt_sound(&fresh, &source),
        Some(sounds::ENTITY_AXOLOTL_HURT)
    );
    assert_eq!(
        axolotl.death_sound(&fresh),
        Some(sounds::ENTITY_AXOLOTL_DEATH)
    );
    assert_eq!(axolotl.ambient_interval(), 80);
    let frog = of(FROG);
    assert_eq!(
        frog.ambient_sound(&fresh),
        Some(sounds::ENTITY_FROG_AMBIENT)
    );
    assert_eq!(frog.step_sound(&fresh), Some(sounds::ENTITY_FROG_STEP));
    assert_eq!(
        frog.hurt_sound(&fresh, &source),
        Some(sounds::ENTITY_FROG_HURT)
    );
    assert_eq!(frog.death_sound(&fresh), Some(sounds::ENTITY_FROG_DEATH));
    let tadpole = of(TADPOLE);
    assert_eq!(tadpole.ambient_sound(&fresh), None);
    assert_eq!(tadpole.ambient_sound_at(&fresh, true, false), None);
    assert_eq!(
        tadpole.hurt_sound(&fresh, &source),
        Some(sounds::ENTITY_TADPOLE_HURT)
    );
    assert_eq!(
        tadpole.death_sound(&fresh),
        Some(sounds::ENTITY_TADPOLE_DEATH)
    );
}

#[test]
fn a_tadpole_never_despawns_nor_is_leashed_nor_gives_experience_and_an_axolotl_despawns_unless_bucketed()
 {
    let fresh = Mob::fresh();
    let tadpole = of(TADPOLE);
    assert!(!tadpole.remove_when_far_away(TADPOLE, &fresh, 0, 1.0e6));
    assert!(!tadpole.can_be_leashed(TADPOLE, &fresh));
    let mut stream = draws(1);
    let replay = stream.clone();
    assert_eq!(tadpole.experience_reward(TADPOLE, &fresh, &mut stream), 0);
    assert_eq!(stream, replay);
    let axolotl = of(AXOLOTL);
    assert!(axolotl.remove_when_far_away(AXOLOTL, &fresh, 0, 1.0e6));
    assert!(axolotl.can_be_leashed(AXOLOTL, &fresh));
    let mut poured = Mob::fresh();
    axolotl.poured_from_bucket(AXOLOTL, &mut poured);
    assert!(axolotl::read(&poured).from_bucket);
    assert!(!axolotl.remove_when_far_away(AXOLOTL, &poured, 0, 1.0e6));
    // A tadpole poured is marked nothing; a frog is kept as an animal is.
    let mut tadpole_poured = Mob::fresh();
    tadpole.poured_from_bucket(TADPOLE, &mut tadpole_poured);
    assert_eq!(tadpole_poured.state, TypeState::default());
    assert!(!of(FROG).remove_when_far_away(FROG, &fresh, 0, 1.0e6));
}

#[test]
fn a_frog_grown_from_a_tadpole_keeps_its_flags_and_its_head() {
    let mut old = Mob::fresh();
    old.body.flags.no_ai = true;
    old.body.head.head_yaw = 33.0;
    tadpole::update(&mut old, |own| own.age_locked = true);
    let grown = frog::grown_from(&old);
    assert!(grown.body.flags.no_ai);
    assert_eq!(grown.body.head.head_yaw, 33.0);
    assert_eq!(
        grown.state,
        TypeState::default(),
        "nothing of the tadpole's age"
    );
}
