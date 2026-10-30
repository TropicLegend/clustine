//! The tamed animals' own rules on their state alone, from the brief's sections 5.1 to
//! 5.8: the variants drawn by biome and moon, the anger's time, the young of a breeding
//! and who may mate, and what clients are shown of each.

use clustine_data::entity_types;
use clustine_world::{EntityId, TypeState};

use super::animal::{self, Animal, Own, variants};
use super::cat::{self, CatState};
use super::neutral::Anger;
use super::ocelot::OcelotState;
use super::parrot::ParrotState;
use super::tamable::{self, Tame};
use super::wolf::{self, WolfState};
use super::{Breeding, Mate, of};
use crate::mobs::Mob;
use crate::region::Random;
use crate::shown::{DataShown, Holder};

fn draws() -> Random {
    Random::of_entity(7, EntityId(3), 11)
}

fn mob_with<O: Own>(own: &O) -> Mob {
    let mut mob = Mob::fresh();
    mob.state = animal::write(&Animal::default(), own);
    mob
}

fn tamed_by(owner: u128) -> Tame {
    let mut tame = Tame::default();
    tame.set_tame_flag(true);
    tame.owner = Some(owner);
    tame
}

#[test]
fn a_wolf_s_variant_is_its_biome_s_with_one_draw_and_pale_elsewhere() {
    for (biome, expected) in [
        (Some("minecraft:forest"), "minecraft:woods"),
        (Some("minecraft:grove"), "minecraft:snowy"),
        (Some("minecraft:snowy_taiga"), "minecraft:ashen"),
        (Some("minecraft:old_growth_pine_taiga"), "minecraft:black"),
        (
            Some("minecraft:old_growth_spruce_taiga"),
            "minecraft:chestnut",
        ),
        (Some("minecraft:jungle"), "minecraft:rusty"),
        (Some("minecraft:savanna"), "minecraft:spotted"),
        (Some("minecraft:badlands"), "minecraft:striped"),
        (Some("minecraft:plains"), "minecraft:pale"),
        (None, "minecraft:pale"),
    ] {
        let mut level = draws();
        let start = level.clone();
        let id = wolf::select_variant(biome, &mut level);
        assert_eq!(
            id,
            variants::id("minecraft:wolf_variant", expected),
            "{biome:?}"
        );
        // `getRandomSafe` draws `nextInt(1)` even for one.
        assert_eq!(level.draws_since(&start), 1);
    }
}

#[test]
fn a_cat_s_variant_is_one_of_ten_and_all_black_besides_only_at_the_full_moon() {
    let all_black = variants::id("minecraft:cat_variant", "minecraft:all_black");
    let day = crate::time::DAY;
    for seed in 0..200 {
        // The full moon of the first night: eleven to draw from.
        let mut level = Random::of_entity(seed, EntityId(1), 0);
        let mut replay = level.clone();
        let id = cat::select_variant(100, &mut level);
        assert_eq!(u32::from(id), replay.next_int(11));
        // The next day's waning moon: ten, all black never among them.
        let mut level = Random::of_entity(seed, EntityId(1), 0);
        let mut replay = level.clone();
        let id = cat::select_variant(day + 100, &mut level);
        assert_ne!(id, all_black);
        assert_eq!(u32::from(id), replay.next_int(10) + 1);
        // Eight days on, full again.
        let mut level = Random::of_entity(seed, EntityId(1), 0);
        let mut replay = level.clone();
        assert_eq!(
            u32::from(cat::select_variant(8 * day, &mut level)),
            replay.next_int(11)
        );
    }
}

#[test]
fn anger_lasts_until_its_end_in_game_time_and_none_is_minus_one() {
    let calm = Anger::default();
    assert_eq!(calm.end, -1);
    assert!(!calm.is_angry(0));
    let angry = Anger {
        end: 1000,
        target: Some(EntityId(9)),
    };
    assert!(angry.is_angry(999) && !angry.is_angry(1000));
}

#[test]
fn a_tamed_wolf_s_pup_takes_a_parent_s_variant_its_owner_and_the_collars_mixed_then_draws_a_voice()
{
    let woods = variants::id("minecraft:wolf_variant", "minecraft:woods");
    let rusty = variants::id("minecraft:wolf_variant", "minecraft:rusty");
    let this = mob_with(&WolfState {
        tame: tamed_by(5),
        variant: woods,
        collar: 15,
        ..WolfState::default()
    });
    let partner = mob_with(&WolfState {
        tame: tamed_by(5),
        variant: rusty,
        collar: 0,
        ..WolfState::default()
    });
    let mut stream = draws();
    let mut replay = stream.clone();
    let child = of(entity_types::WOLF as u16)
        .breed_offspring(&mut Breeding {
            this: &this,
            partner: &partner,
            partner_kind: entity_types::WOLF as u16,
            child_kind: None,
            draws: &mut stream,
            biome: None,
            egg_user: None,
        })
        .expect("a pup");
    let (_, own) = wolf::state_of(&child);
    let variant = if replay.next_bool() { woods } else { rusty };
    // Black and white make grey by a recipe: no draw.
    let voice = replay.next_int(7) as u8;
    assert_eq!((own.variant, own.collar, own.sound), (variant, 7, voice));
    assert_eq!(own.tame.owner, Some(5));
    assert!(own.tame.is_tame() && !own.tame.ordered_to_sit);
    assert_eq!(child.body.vitals.health, 40.0);
    assert_eq!(stream, replay);

    // A wild parent's pup is wild, red-collared, and draws no collar.
    let wild = mob_with(&WolfState::default());
    let mut stream = draws();
    let mut replay = stream.clone();
    let child = of(entity_types::WOLF as u16)
        .breed_offspring(&mut Breeding {
            this: &wild,
            partner: &partner,
            partner_kind: entity_types::WOLF as u16,
            child_kind: None,
            draws: &mut stream,
            biome: None,
            egg_user: None,
        })
        .unwrap();
    let (_, own) = wolf::state_of(&child);
    let _variant = replay.next_bool();
    assert_eq!(own.sound, replay.next_int(7) as u8);
    assert!(!own.tame.is_tame());
    assert_eq!(own.collar, 14);
}

#[test]
fn only_tamed_wolves_mate_and_only_with_a_tamed_wolf_not_sitting_and_cats_likewise_but_sitting() {
    let tamed = mob_with(&WolfState {
        tame: tamed_by(1),
        ..WolfState::default()
    });
    let wild = mob_with(&WolfState::default());
    let mut sitting = tamed_by(1);
    sitting.set_in_sitting_pose(true);
    let sat = mob_with(&WolfState {
        tame: sitting.clone(),
        ..WolfState::default()
    });
    let mates = |kind: i32, this: &Mob, other: &Mob| {
        let record = |key: u64| clustine_world::Entity {
            portal: Default::default(),
            key: EntityId(key),
            kind: kind as u16,
            uuid: uuid::Uuid::from_u128(u128::from(key)),
            position: [0.0; 3],
            motion: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: true,
            hops: 0,
            lived: 0,
            fresh_at: 0,
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
            data: Mob::fresh().encode(),
        };
        let (a, b) = (record(1), record(2));
        of(kind as u16).can_mate(
            Mate {
                entity: &a,
                mob: this,
            },
            Mate {
                entity: &b,
                mob: other,
            },
        )
    };
    let wolves = entity_types::WOLF;
    assert!(mates(wolves, &tamed, &tamed));
    assert!(!mates(wolves, &wild, &tamed));
    assert!(!mates(wolves, &tamed, &wild));
    assert!(!mates(wolves, &tamed, &sat));
    let tamed_cat = mob_with(&CatState {
        tame: tamed_by(1),
        ..CatState::default()
    });
    let sat_cat = mob_with(&CatState {
        tame: sitting,
        ..CatState::default()
    });
    let cats = entity_types::CAT;
    assert!(mates(cats, &tamed_cat, &sat_cat));
    assert!(!mates(cats, &tamed_cat, &mob_with(&CatState::default())));
    // A parrot breeds nothing.
    let parrot = mob_with(&ParrotState::default());
    let mut stream = draws();
    assert!(
        of(entity_types::PARROT as u16)
            .breed_offspring(&mut Breeding {
                this: &parrot,
                partner: &parrot,
                partner_kind: entity_types::PARROT as u16,
                child_kind: None,
                draws: &mut stream,
                biome: None,
                egg_user: None,
            })
            .is_none()
    );
}

#[test]
fn a_fresh_tamed_animal_s_type_state_reads_back_and_its_owner_is_kept_by_uuid() {
    let own = WolfState {
        tame: tamed_by(u128::MAX),
        collar: 3,
        anger: Anger {
            end: 1234,
            target: Some(EntityId(8)),
        },
        ..WolfState::default()
    };
    let state = animal::write(&Animal::default(), &own);
    assert_eq!(animal::read::<WolfState>(&state).1, own);
    // A fresh wolf keeps no state at all.
    assert_eq!(
        animal::write(&Animal::default(), &WolfState::default()),
        TypeState::default()
    );
}

#[test]
fn each_tamed_animal_shows_its_tame_flags_its_owner_and_its_own_values_from_index_18() {
    let wolf = of(entity_types::WOLF as u16);
    let pale = variants::id("minecraft:wolf_variant", "minecraft:pale");
    let classic = variants::id("minecraft:wolf_sound_variant", "minecraft:classic");
    let shown = wolf.shown(&mob_with(&WolfState::default()));
    assert_eq!(
        shown,
        [
            (16, DataShown::Boolean(false), true),
            (17, DataShown::Boolean(false), true),
            (18, DataShown::Byte(0), true),
            (19, DataShown::OptionalUuid(None), true),
            (20, DataShown::Boolean(false), true),
            (21, DataShown::Int(14), true),
            (22, DataShown::Long(-1), true),
            (
                23,
                DataShown::Holder(Holder::WolfVariant, i32::from(pale)),
                true
            ),
            (
                24,
                DataShown::Holder(Holder::WolfSoundVariant, i32::from(classic)),
                true
            ),
        ]
    );
    let mut sitting = tamed_by(9);
    sitting.set_in_sitting_pose(true);
    let shown = wolf.shown(&mob_with(&WolfState {
        tame: sitting,
        collar: 11,
        interested: true,
        anger: Anger {
            end: 500,
            target: None,
        },
        ..WolfState::default()
    }));
    assert_eq!(shown[2], (18, DataShown::Byte(5), false));
    assert_eq!(shown[3], (19, DataShown::OptionalUuid(Some(9)), false));
    assert_eq!(shown[4], (20, DataShown::Boolean(true), false));
    assert_eq!(shown[5], (21, DataShown::Int(11), false));
    assert_eq!(shown[6], (22, DataShown::Long(500), false));

    let black = variants::id("minecraft:cat_variant", "minecraft:black");
    let cat_classic = variants::id("minecraft:cat_sound_variant", "minecraft:classic");
    let shown = of(entity_types::CAT as u16).shown(&mob_with(&CatState {
        tame: tamed_by(2),
        lying: true,
        crouching: true,
        ..CatState::default()
    }));
    assert_eq!(
        shown,
        [
            (16, DataShown::Boolean(false), true),
            (17, DataShown::Boolean(false), true),
            (6, DataShown::Pose(5), false),
            (18, DataShown::Byte(4), false),
            (19, DataShown::OptionalUuid(Some(2)), false),
            (
                20,
                DataShown::Holder(Holder::CatVariant, i32::from(black)),
                true
            ),
            (21, DataShown::Boolean(true), false),
            (22, DataShown::Boolean(false), true),
            (23, DataShown::Int(14), true),
            (
                24,
                DataShown::Holder(Holder::CatSoundVariant, i32::from(cat_classic)),
                true
            ),
        ]
    );

    let shown = of(entity_types::PARROT as u16).shown(&mob_with(&ParrotState {
        variant: 3,
        no_gravity: true,
        ..ParrotState::default()
    }));
    assert!(shown.contains(&(5, DataShown::Boolean(true), false)));
    assert!(shown.contains(&(18, DataShown::Byte(0), true)));
    assert!(shown.contains(&(20, DataShown::Int(3), false)));

    let shown = of(entity_types::OCELOT as u16).shown(&mob_with(&OcelotState {
        trusting: true,
        crouching: false,
    }));
    assert!(shown.contains(&(18, DataShown::Boolean(true), false)));
    assert!(shown.contains(&(6, DataShown::Pose(0), true)));

    // Through `mob_values`, sorted by index as clients are sent them.
    let data = mob_with(&WolfState::default()).encode();
    let values = crate::shown::mob_values(entity_types::WOLF as u16, &data);
    assert!(values.windows(2).all(|pair| pair[0].0 < pair[1].0));
}

#[test]
fn a_pup_and_a_kitten_have_their_own_sizes() {
    let mut baby = Animal::default();
    baby.set_age(animal::BABY_START_AGE, 0);
    let mut pup = Mob::fresh();
    pup.state = animal::write(&baby, &WolfState::default());
    let size = of(entity_types::WOLF as u16).dimensions(&pup).unwrap();
    assert_eq!(
        (size.width, size.height, size.eye_height),
        (0.3, 0.425, 0.343_75)
    );
    let mut kitten = Mob::fresh();
    kitten.state = animal::write(&baby, &CatState::default());
    let size = of(entity_types::CAT as u16).dimensions(&kitten).unwrap();
    assert_eq!(
        (size.width, size.height, size.eye_height),
        (0.3, 0.35, 0.343_75)
    );
    assert!(of(entity_types::CAT as u16).is_baby(&kitten));
}

#[test]
fn a_pet_heals_on_its_food_by_its_nutrition_and_never_past_its_most() {
    let kind = entity_types::WOLF as u16;
    let mut mob = mob_with(&WolfState::default());
    wolf::taming_side_effects(&mut mob, true);
    mob.body.vitals.health = 10.0;
    // Beef heals a wolf twice its three.
    tamable::heal(
        &mut mob,
        kind,
        2.0 * tamable::nutrition(clustine_data::items::BEEF).unwrap() as f32,
    );
    assert_eq!(mob.body.vitals.health, 16.0);
    tamable::heal(&mut mob, kind, 100.0);
    assert_eq!(mob.body.vitals.health, 40.0);
    assert!(!tamable::hurt(&mob, kind));
}
