//! The keepers' own rules on their state alone, from the brief's sections 7.12 and 7.13
//! with 9.7.5 and 10.7.5: the nautilus's, the zombie nautilus's and the copper golem's
//! attributes, type states, synced data with their indices and defaults, the draws of
//! their constructors and `finalizeSpawn`s in order, each use of them, their sounds,
//! the golem's weathering and statue, and what the riding is given.

use clustine_data::{attributes as ids, entity_types, items, sounds};
use clustine_world::{BlockPos, EntityId, ItemStack, TypeState};

use super::animal::{self, Animal, BABY_START_AGE, variants};
use super::copper_golem::{self, CopperGolemState};
use super::nautilus::{self, NautilusState};
use super::tamable::Tame;
use super::{Breeding, Did, Interaction, Spawning, Used, is_built, of, zombie_nautilus};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::body::attributes::supplied;
use crate::mobs::body::hurt::{Causer, DamageSource, DamageType};
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::region::Random;
use crate::shown::{DataShown, Holder, Serialiser};

const NAUTILUS: u16 = entity_types::NAUTILUS as u16;
const ZOMBIE: u16 = entity_types::ZOMBIE_NAUTILUS as u16;
const GOLEM: u16 = entity_types::COPPER_GOLEM as u16;

/// The UUID of the player who uses these mobs.
const PLAYER: u128 = 7;

fn draws() -> Random {
    Random::of_entity(7, EntityId(3), 11)
}

fn one(item: i32) -> Option<ItemStack> {
    Some(ItemStack { item, count: 1 })
}

/// A mob of the type `kind` at its full health, with the nautilus's state `own` and
/// what every animal shares `shared`.
fn nautilus_with(kind: u16, shared: &Animal, own: &NautilusState) -> Mob {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = supplied(kind, attribute(ids::MAX_HEALTH)).unwrap() as f32;
    mob.state = animal::write(shared, own);
    mob
}

fn tamed() -> NautilusState {
    let mut tame = Tame::default();
    tame.set_tame_flag(true);
    tame.owner = Some(PLAYER);
    NautilusState {
        tame,
        ..NautilusState::default()
    }
}

/// A use of the mob `mob` of type `kind` by the player with `held` in hand, from its own
/// stream `stream`: what it answered and what it did beyond the mob.
fn used(
    kind: u16,
    mob: &mut Mob,
    held: Option<ItemStack>,
    sneaking: bool,
    stream: &mut Random,
) -> (Interaction, Vec<Did>) {
    let mut used = Used {
        key: EntityId(3),
        kind,
        mob,
        player: EntityId(1),
        player_uuid: Some(PLAYER),
        game_time: 100,
        held,
        creative: true,
        sneaking,
        carries: false,
        on_ground: true,
        from: [3.0, 64.0, 0.0],
        position: [0.0, 64.0, 0.0],
        moment: 50,
        draws: stream,
        did: Vec::new(),
    };
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

fn events_of(did: &[Did]) -> Vec<u8> {
    did.iter()
        .filter_map(|done| match done {
            Did::Event(event) => Some(*event),
            _ => None,
        })
        .collect()
}

#[test]
fn every_keeper_is_built_so_that_its_egg_makes_it() {
    for kind in [NAUTILUS, ZOMBIE, GOLEM] {
        assert!(is_built(kind), "{kind}");
    }
}

#[test]
fn the_keepers_have_the_game_s_attributes() {
    let value = |kind: u16, id: i32| supplied(kind, attribute(id)).unwrap();
    assert_eq!(value(NAUTILUS, ids::MAX_HEALTH), 15.0);
    assert_eq!(value(NAUTILUS, ids::MOVEMENT_SPEED), 1.0);
    assert_eq!(value(NAUTILUS, ids::ATTACK_DAMAGE), 3.0);
    assert_eq!(
        value(NAUTILUS, ids::KNOCKBACK_RESISTANCE),
        0.300_000_011_920_928_96
    );
    assert_eq!(value(ZOMBIE, ids::MAX_HEALTH), 15.0);
    assert_eq!(value(ZOMBIE, ids::MOVEMENT_SPEED), 1.100_000_023_841_858);
    assert_eq!(value(GOLEM, ids::MAX_HEALTH), 12.0);
    assert_eq!(value(GOLEM, ids::MOVEMENT_SPEED), 0.200_000_002_980_232_24);
    assert_eq!(value(GOLEM, ids::STEP_HEIGHT), 1.0);
}

#[test]
fn a_fresh_keeper_keeps_no_state_and_a_changed_one_reads_back_as_it_was() {
    assert_eq!(
        animal::write(&Animal::default(), &NautilusState::default()),
        TypeState::default()
    );
    let own = NautilusState {
        dashing: true,
        dash_ready_at: 90,
        under_water: true,
        attack_target_cooldown: Some(3000),
        variant: variants::id(nautilus::VARIANTS, "minecraft:warm"),
        ..tamed()
    };
    let mob = nautilus_with(ZOMBIE, &Animal::default(), &own);
    assert_eq!(nautilus::state_of(&mob).1, own);
    assert_eq!(
        copper_golem::write(&CopperGolemState::default()),
        TypeState::default()
    );
    let golem = CopperGolemState {
        weather: copper_golem::WEATHERED,
        next_weathering: 1_000_000,
        state: copper_golem::DROPPING_ITEM,
        opened_chest: Some(BlockPos::new(1, 2, 3)),
        transport_cooldown: Some(77),
    };
    assert_eq!(copper_golem::read(&copper_golem::write(&golem)), golem);
    // A state of another version is read as a fresh one's.
    let mut other = copper_golem::write(&golem);
    other.version += 1;
    assert_eq!(copper_golem::read(&other), CopperGolemState::default());
}

#[test]
fn the_nautilus_shows_its_age_its_tameness_and_its_dash_from_16_to_20() {
    let mob = nautilus_with(NAUTILUS, &Animal::default(), &NautilusState::default());
    assert_eq!(
        of(NAUTILUS).shown(&mob),
        [
            (16, DataShown::Boolean(false), true),
            (17, DataShown::Boolean(false), true),
            (18, DataShown::Byte(0), true),
            (19, DataShown::OptionalUuid(None), true),
            (20, DataShown::Boolean(false), true),
        ]
    );
    let own = NautilusState {
        dashing: true,
        ..tamed()
    };
    let mob = nautilus_with(NAUTILUS, &Animal::default(), &own);
    let shown = of(NAUTILUS).shown(&mob);
    assert_eq!(shown[2], (18, DataShown::Byte(4), false));
    assert_eq!(shown[3], (19, DataShown::OptionalUuid(Some(PLAYER)), false));
    assert_eq!(shown[4], (20, DataShown::Boolean(true), false));
}

#[test]
fn the_zombie_nautilus_shows_its_variant_at_21_temperate_by_default() {
    let temperate = variants::id(nautilus::VARIANTS, "minecraft:temperate");
    let warm = variants::id(nautilus::VARIANTS, "minecraft:warm");
    assert_eq!((temperate, warm), (0, 1));
    let mob = nautilus_with(ZOMBIE, &Animal::default(), &NautilusState::default());
    let shown = of(ZOMBIE).shown(&mob);
    assert_eq!(shown.len(), 6);
    assert_eq!(
        shown[5],
        (
            21,
            DataShown::Holder(Holder::ZombieNautilusVariant, 0),
            true
        )
    );
    let own = NautilusState {
        variant: warm,
        ..NautilusState::default()
    };
    let mob = nautilus_with(ZOMBIE, &Animal::default(), &own);
    assert_eq!(
        of(ZOMBIE).shown(&mob)[5],
        (
            21,
            DataShown::Holder(Holder::ZombieNautilusVariant, 1),
            false
        )
    );
    // The nautilus has no variant to show.
    assert_eq!(of(NAUTILUS).shown(&mob).len(), 5);
}

#[test]
fn the_copper_golem_shows_its_weathering_at_16_and_its_state_at_17() {
    let mut mob = Mob::fresh();
    assert_eq!(
        of(GOLEM).shown(&mob),
        [
            (
                16,
                DataShown::Serialised(Serialiser::WeatheringCopperState, 0),
                true
            ),
            (
                17,
                DataShown::Serialised(Serialiser::CopperGolemState, 0),
                true
            ),
        ]
    );
    copper_golem::update(&mut mob, |golem| golem.weather = copper_golem::OXIDIZED);
    copper_golem::set_state(&mut mob, copper_golem::GETTING_NO_ITEM);
    assert_eq!(
        of(GOLEM).shown(&mob),
        [
            (
                16,
                DataShown::Serialised(Serialiser::WeatheringCopperState, 3),
                false
            ),
            (
                17,
                DataShown::Serialised(Serialiser::CopperGolemState, 2),
                false
            ),
        ]
    );
}

#[test]
fn a_nautilus_s_spawn_draws_its_attack_cooldown_of_the_level_and_a_zombie_its_variant_first() {
    for seed in 0..50 {
        let mut level = Random::of_entity(seed, EntityId(1), 0);
        let mut replay = level.clone();
        let mut own = draws();
        let mut mob = nautilus_with(NAUTILUS, &Animal::default(), &NautilusState::default());
        of(NAUTILUS).finalize_spawn(&mut Spawning {
            game_time: 0,
            mob: &mut mob,
            level: &mut level,
            own: &mut own,
            biome: Some("minecraft:warm_ocean"),
            at: BlockPos::new(0, 0, 0),
            day: 0,
        });
        let cooldown = 2400 + replay.next_int(1201) as i32;
        assert_eq!(
            nautilus::state_of(&mob).1.attack_target_cooldown,
            Some(cooldown)
        );
        assert!((2400..=3600).contains(&cooldown));
        assert_eq!(level, replay);
        assert_eq!(nautilus::state_of(&mob).1.variant, 0);
    }
    for (biome, expected) in [
        (Some("minecraft:warm_ocean"), "minecraft:warm"),
        (Some("minecraft:lukewarm_ocean"), "minecraft:temperate"),
        (Some("minecraft:plains"), "minecraft:temperate"),
        (None, "minecraft:temperate"),
    ] {
        let mut level = draws();
        let mut replay = level.clone();
        let mut own = draws();
        let mut mob = nautilus_with(ZOMBIE, &Animal::default(), &NautilusState::default());
        of(ZOMBIE).finalize_spawn(&mut Spawning {
            game_time: 0,
            mob: &mut mob,
            level: &mut level,
            own: &mut own,
            biome,
            at: BlockPos::new(0, 0, 0),
            day: 0,
        });
        // `getRandomSafe`'s `nextInt(1)`, then the cooldown.
        assert_eq!(replay.next_int(1), 0);
        let cooldown = 2400 + replay.next_int(1201) as i32;
        let own = nautilus::state_of(&mob).1;
        assert_eq!(
            own.variant,
            variants::id(nautilus::VARIANTS, expected),
            "{biome:?}"
        );
        assert_eq!(own.attack_target_cooldown, Some(cooldown));
        assert_eq!(level, replay);
    }
}

#[test]
fn a_copper_golem_s_constructor_draws_its_first_transport_cooldown_and_keeps_it() {
    for seed in 0..100 {
        let mut own = Random::of_entity(seed, EntityId(2), 0);
        let mut replay = own.clone();
        let mut mob = Mob::fresh();
        of(GOLEM).construct(&mut mob, &mut own);
        let cooldown = 60 + replay.next_int(40) as i32;
        assert!((60..=99).contains(&cooldown));
        assert_eq!(
            copper_golem::state_of(&mob).transport_cooldown,
            Some(cooldown)
        );
        assert!(mob.body.flags.persistent);
        assert_eq!(own, replay);
    }
}

#[test]
fn a_wild_nautilus_is_tamed_by_a_pufferfish_one_time_in_three_and_eats_with_its_voice() {
    let mut tamed_once = false;
    let mut smoked_once = false;
    for seed in 0..60 {
        let mut mob = nautilus_with(NAUTILUS, &Animal::default(), &NautilusState::default());
        let mut stream = Random::of_entity(seed, EntityId(3), 0);
        let mut replay = stream.clone();
        let (result, did) = used(
            NAUTILUS,
            &mut mob,
            one(items::PUFFERFISH),
            false,
            &mut stream,
        );
        assert_eq!(result, Interaction::SuccessServer);
        let tames = replay.next_int(3) == 0;
        let pitch = (replay.next_float() - replay.next_float()) * 0.2 + 1.0;
        assert_eq!(stream, replay);
        let own = nautilus::state_of(&mob).1;
        assert_eq!(own.tame.is_tame(), tames);
        if tames {
            assert_eq!(own.tame.owner, Some(PLAYER));
            assert_eq!(events_of(&did), [7]);
            tamed_once = true;
        } else {
            assert_eq!(events_of(&did), [6]);
            smoked_once = true;
        }
        assert_eq!(
            did.last(),
            Some(&Did::Sound {
                sound: sounds::ENTITY_NAUTILUS_EAT,
                source: SOUND_SOURCE_NEUTRAL,
                at: [0.0, 64.0, 0.0],
                volume: 1.0,
                pitch,
                but_the_player: false,
            })
        );
        // Any use makes it persistent.
        assert!(mob.body.flags.persistent);
    }
    assert!(tamed_once && smoked_once);
}

#[test]
fn a_pufferfish_bucket_tames_too_and_leaves_a_water_bucket_but_cod_does_not_tame() {
    let mut mob = nautilus_with(NAUTILUS, &Animal::default(), &NautilusState::default());
    let (result, did) = used(
        NAUTILUS,
        &mut mob,
        one(items::PUFFERFISH_BUCKET),
        false,
        &mut draws(),
    );
    assert_eq!(result, Interaction::SuccessServer);
    assert_eq!(
        did[0],
        Did::Give {
            stack: ItemStack {
                item: items::WATER_BUCKET,
                count: 1
            },
            no_duplicates: true,
        }
    );
    // Cod is food for a tamed one, nothing to a wild adult.
    let mut mob = nautilus_with(NAUTILUS, &Animal::default(), &NautilusState::default());
    let (result, did) = used(NAUTILUS, &mut mob, one(items::COD), false, &mut draws());
    assert_eq!(result, Interaction::Pass);
    assert!(did.is_empty());
    assert!(!nautilus::state_of(&mob).1.tame.is_tame());
}

#[test]
fn a_tamed_nautilus_hurt_heals_twice_a_fish_s_nutrition_and_one_for_a_bucket() {
    for (item, healed) in [
        (items::COD, 4.0),
        (items::COOKED_SALMON, 12.0),
        (items::PUFFERFISH, 2.0),
        (items::COD_BUCKET, 1.0),
    ] {
        let mut mob = nautilus_with(NAUTILUS, &Animal::default(), &tamed());
        mob.body.vitals.health = 2.0;
        let (result, did) = used(NAUTILUS, &mut mob, one(item), false, &mut draws());
        assert_eq!(result, Interaction::Success, "{item}");
        assert_eq!(mob.body.vitals.health, 2.0 + healed, "{item}");
        assert_eq!(sounds_of(&did), [sounds::ENTITY_NAUTILUS_EAT]);
        let gave = did.iter().any(|done| matches!(done, Did::Give { .. }));
        assert_eq!(gave, item == items::COD_BUCKET);
    }
}

#[test]
fn a_tamed_nautilus_at_full_health_fed_falls_in_love_and_a_baby_grows() {
    let mut mob = nautilus_with(NAUTILUS, &Animal::default(), &tamed());
    let (result, did) = used(NAUTILUS, &mut mob, one(items::SALMON), false, &mut draws());
    assert_eq!(result, Interaction::SuccessServer);
    assert_eq!(events_of(&did), [18]);
    assert!(animal::animal_of(&mob.state).in_love(50));
    // A wild baby eats fish (its food is the tamed one's) and grows a tenth.
    let mut baby = Animal::default();
    baby.set_age(BABY_START_AGE, 50);
    let mut mob = nautilus_with(NAUTILUS, &baby, &NautilusState::default());
    let (result, did) = used(NAUTILUS, &mut mob, one(items::COD), false, &mut draws());
    assert_eq!(result, Interaction::Success);
    assert_eq!(animal::animal_of(&mob.state).age(50), -21_600);
    assert_eq!(sounds_of(&did), [sounds::ENTITY_BABY_NAUTILUS_EAT]);
}

#[test]
fn a_saddle_and_nautilus_armour_go_on_a_tamed_adult_only_with_its_own_saddling_sound() {
    // Wild: the saddle is no use, and a wild adult has nothing else to do with it.
    let mut mob = nautilus_with(NAUTILUS, &Animal::default(), &NautilusState::default());
    let (result, _) = used(NAUTILUS, &mut mob, one(items::SADDLE), false, &mut draws());
    assert_eq!(result, Interaction::Pass);
    let saddle = usize::from(slots::SADDLE);
    let body = usize::from(slots::BODY);
    assert!(mob.body.equipment.slots[saddle].is_none());
    // Tamed, out of water and under it.
    for (under_water, sound) in [
        (false, sounds::ITEM_NAUTILUS_SADDLE_EQUIP),
        (true, sounds::ITEM_NAUTILUS_SADDLE_UNDERWATER_EQUIP),
    ] {
        let own = NautilusState {
            under_water,
            ..tamed()
        };
        let mut mob = nautilus_with(NAUTILUS, &Animal::default(), &own);
        let mut stream = draws();
        let mut replay = stream.clone();
        let (result, did) = used(NAUTILUS, &mut mob, one(items::SADDLE), false, &mut stream);
        assert_eq!(result, Interaction::Success);
        assert_eq!(mob.body.equipment.slots[saddle], one(items::SADDLE));
        assert_eq!(mob.body.equipment.drop_chance(saddle), 2.0);
        assert_eq!(sounds_of(&did), [sound]);
        assert_eq!(did.last(), Some(&Did::GameEvent("minecraft:equip")));
        // The equip sound's seed.
        replay.next_u64();
        assert_eq!(stream, replay);
        assert!(nautilus::steers(&mob));
        // A second saddle does nothing more: it rides (the seam).
        let (result, did) = used(NAUTILUS, &mut mob, one(items::SADDLE), false, &mut draws());
        assert_eq!(result, Interaction::Success);
        assert!(did.is_empty());
    }
    let mut mob = nautilus_with(ZOMBIE, &Animal::default(), &tamed());
    let (result, did) = used(
        ZOMBIE,
        &mut mob,
        one(items::IRON_NAUTILUS_ARMOR),
        false,
        &mut draws(),
    );
    assert_eq!(result, Interaction::Success);
    assert_eq!(
        mob.body.equipment.slots[body],
        one(items::IRON_NAUTILUS_ARMOR)
    );
    assert_eq!(sounds_of(&did), [sounds::ITEM_ARMOR_EQUIP_NAUTILUS]);
    // A baby tamed may wear neither.
    let mut baby = Animal::default();
    baby.set_age(BABY_START_AGE, 0);
    let mut mob = nautilus_with(NAUTILUS, &baby, &tamed());
    used(NAUTILUS, &mut mob, one(items::SADDLE), false, &mut draws());
    assert!(mob.body.equipment.slots[saddle].is_none());
}

#[test]
fn a_tamed_nautilus_clicked_sneaking_or_empty_handed_takes_the_click_and_nobody_on_yet() {
    let mut mob = nautilus_with(NAUTILUS, &Animal::default(), &tamed());
    let (result, did) = used(NAUTILUS, &mut mob, None, false, &mut draws());
    assert_eq!(result, Interaction::Success);
    assert!(did.is_empty());
    let (result, did) = used(NAUTILUS, &mut mob, one(items::COD), true, &mut draws());
    assert_eq!(result, Interaction::Success);
    assert!(did.is_empty());
    // A wild one clicked with nothing passes.
    let mut mob = nautilus_with(NAUTILUS, &Animal::default(), &NautilusState::default());
    assert_eq!(
        used(NAUTILUS, &mut mob, None, false, &mut draws()).0,
        Interaction::Pass
    );
}

#[test]
fn the_zombie_nautilus_eats_with_its_own_voice_and_breeds_nothing() {
    let mut mob = nautilus_with(ZOMBIE, &Animal::default(), &NautilusState::default());
    let (result, did) = used(
        ZOMBIE,
        &mut mob,
        one(items::PUFFERFISH),
        false,
        &mut draws(),
    );
    assert_eq!(result, Interaction::SuccessServer);
    assert_eq!(sounds_of(&did), [sounds::ENTITY_ZOMBIE_NAUTILUS_EAT]);
    let parent = nautilus_with(ZOMBIE, &Animal::default(), &tamed());
    let mut stream = draws();
    let mut breeding = Breeding {
        this: &parent,
        partner: &parent,
        partner_kind: ZOMBIE,
        child_kind: None,
        draws: &mut stream,
        biome: None,
        egg_user: None,
    };
    assert!(of(ZOMBIE).breed_offspring(&mut breeding).is_none());
    // A tamed nautilus's child is tamed by the same owner; a wild one's is wild.
    let mut breeding = Breeding {
        this: &parent,
        partner: &parent,
        partner_kind: NAUTILUS,
        child_kind: None,
        draws: &mut stream,
        biome: None,
        egg_user: None,
    };
    let child = of(NAUTILUS).breed_offspring(&mut breeding).unwrap();
    let own = nautilus::state_of(&child).1;
    assert!(own.tame.is_tame() && own.tame.owner == Some(PLAYER));
    let wild = nautilus_with(NAUTILUS, &Animal::default(), &NautilusState::default());
    let mut breeding = Breeding {
        this: &wild,
        partner: &parent,
        partner_kind: NAUTILUS,
        child_kind: None,
        draws: &mut stream,
        biome: None,
        egg_user: None,
    };
    let child = of(NAUTILUS).breed_offspring(&mut breeding).unwrap();
    assert!(!nautilus::state_of(&child).1.tame.is_tame());
}

#[test]
fn the_nautilus_sounds_by_its_age_and_whether_its_eyes_are_under_water() {
    let source = DamageSource::of(DamageType::FALL);
    for (under_water, baby, ambient, hurt, death) in [
        (
            true,
            false,
            sounds::ENTITY_NAUTILUS_AMBIENT,
            sounds::ENTITY_NAUTILUS_HURT,
            sounds::ENTITY_NAUTILUS_DEATH,
        ),
        (
            false,
            false,
            sounds::ENTITY_NAUTILUS_AMBIENT_LAND,
            sounds::ENTITY_NAUTILUS_HURT_LAND,
            sounds::ENTITY_NAUTILUS_DEATH_LAND,
        ),
        (
            true,
            true,
            sounds::ENTITY_BABY_NAUTILUS_AMBIENT,
            sounds::ENTITY_BABY_NAUTILUS_HURT,
            sounds::ENTITY_BABY_NAUTILUS_DEATH,
        ),
        (
            false,
            true,
            sounds::ENTITY_BABY_NAUTILUS_AMBIENT_LAND,
            sounds::ENTITY_BABY_NAUTILUS_HURT_LAND,
            sounds::ENTITY_BABY_NAUTILUS_DEATH_LAND,
        ),
    ] {
        let mut shared = Animal::default();
        if baby {
            shared.set_age(BABY_START_AGE, 0);
        }
        let own = NautilusState {
            under_water,
            ..NautilusState::default()
        };
        let mob = nautilus_with(NAUTILUS, &shared, &own);
        let kind = of(NAUTILUS);
        assert_eq!(kind.ambient_sound(&mob), Some(ambient));
        assert_eq!(
            kind.ambient_sound_at(&mob, !under_water, true),
            Some(ambient)
        );
        assert_eq!(kind.hurt_sound(&mob, &source), Some(hurt));
        assert_eq!(kind.death_sound(&mob), Some(death));
    }
    let mob = nautilus_with(ZOMBIE, &Animal::default(), &NautilusState::default());
    assert_eq!(
        of(ZOMBIE).ambient_sound(&mob),
        Some(sounds::ENTITY_ZOMBIE_NAUTILUS_AMBIENT_LAND)
    );
    assert_eq!(
        of(ZOMBIE).hurt_sound(&mob, &source),
        Some(sounds::ENTITY_ZOMBIE_NAUTILUS_HURT_LAND)
    );
    for kind in [NAUTILUS, ZOMBIE] {
        assert_eq!(of(kind).ambient_interval(), 120);
        assert!(!of(kind).plays_step_sounds());
        assert!(!of(kind).pushed_by_fluid());
    }
}

#[test]
fn a_baby_nautilus_is_half_the_size_and_a_tamed_one_is_kept_wherever_it_is() {
    let info = clustine_data::EntityTypeInfo::of(entity_types::NAUTILUS);
    let mut baby = Animal::default();
    baby.set_age(BABY_START_AGE, 0);
    let mob = nautilus_with(NAUTILUS, &baby, &NautilusState::default());
    let size = of(NAUTILUS).dimensions(&mob).unwrap();
    assert_eq!(
        (size.width, size.height, size.eye_height),
        (info.width * 0.5, info.height * 0.5, info.eye_height * 0.5)
    );
    let wild = nautilus_with(NAUTILUS, &Animal::default(), &NautilusState::default());
    assert!(of(NAUTILUS).dimensions(&wild).is_none());
    assert!(!of(NAUTILUS).requires_custom_persistence(&wild));
    let own = nautilus_with(ZOMBIE, &Animal::default(), &tamed());
    assert!(of(ZOMBIE).requires_custom_persistence(&own));
    // Its home's radius: 32 for a grown one unsaddled, 16 saddled or a baby.
    assert_eq!(nautilus::restriction_radius(&wild), 32);
    assert_eq!(nautilus::restriction_radius(&mob), 16);
    let mut saddled = wild.clone();
    saddled.body.equipment.slots[usize::from(slots::SADDLE)] = one(items::SADDLE);
    assert_eq!(nautilus::restriction_radius(&saddled), 16);
}

#[test]
fn a_hurt_angers_a_nautilus_at_the_living_thing_that_caused_it() {
    let causer = |living| Causer {
        key: EntityId(9),
        kind: entity_types::PLAYER as u16,
        position: [0.0; 3],
        player: Some(PLAYER),
        living,
    };
    let source = DamageSource::by(DamageType::PLAYER_ATTACK, causer(true));
    assert_eq!(nautilus::anger_target(&source), Some(EntityId(9)));
    let thing = DamageSource::by(DamageType::PLAYER_ATTACK, causer(false));
    assert_eq!(nautilus::anger_target(&thing), None);
    assert_eq!(
        nautilus::anger_target(&DamageSource::of(DamageType::FALL)),
        None
    );
}

#[test]
fn the_riding_is_given_the_nautilus_s_input_speed_and_dash() {
    // Looking level and going on: straight on; looking down 90 degrees: straight down.
    assert_eq!(nautilus::ridden_input(0.0, 1.0, 0.0), [0.0, -0.0, 1.0]);
    let down = nautilus::ridden_input(0.5, 1.0, 90.0);
    assert_eq!(down[0], 0.5);
    assert!((down[1] + 1.0).abs() < 1.0e-6 && down[2].abs() < 1.0e-3);
    // Going back: half, and the other way.
    let back = nautilus::ridden_input(0.0, -1.0, 0.0);
    assert_eq!(back, [0.0, 0.0, -0.5]);
    assert_eq!(nautilus::ridden_input(1.0, 0.0, 45.0), [1.0, 0.0, 0.0]);
    assert_eq!(nautilus::ridden_rotation(30.0, 40.0), (20.0, 30.0));
    assert_eq!(nautilus::ridden_yaw(0.0, 90.0), 45.0);
    assert_eq!(nautilus::ridden_yaw(170.0, -170.0), 180.0);
    assert_eq!(nautilus::ridden_speed(1.0, true), 0.0325);
    assert_eq!(nautilus::ridden_speed(1.0, false), 0.02);
    // Saddled and ready, a jump's power gives its scale; not while it cools down.
    let mut mob = nautilus_with(NAUTILUS, &Animal::default(), &tamed());
    assert_eq!(nautilus::on_player_jump(&mob, 10, 90), None);
    mob.body.equipment.slots[usize::from(slots::SADDLE)] = one(items::SADDLE);
    assert_eq!(nautilus::on_player_jump(&mob, 10, 100), Some(1.0));
    assert_eq!(nautilus::on_player_jump(&mob, 10, 45), Some(0.6));
    let added = nautilus::execute_riders_jump(&mut mob, 10, 1.0, [0.0, 0.0, 1.0], true, 1.0, 1.0);
    assert_eq!(added[2], f64::from(1.2_f32));
    let own = nautilus::state_of(&mob).1;
    assert!(own.dashing);
    assert_eq!(nautilus::dash_cooldown(&own, 10), 40);
    assert_eq!(nautilus::dash_cooldown(&own, 49), 1);
    assert_eq!(nautilus::dash_cooldown(&own, 60), 0);
    assert_eq!(nautilus::on_player_jump(&mob, 30, 90), None);
    assert_eq!(nautilus::on_player_jump(&mob, 50, 90), Some(1.0));
    // A rider's jump started: the dash sound of its medium, a cooldown where none ran.
    let mut mob = nautilus_with(ZOMBIE, &Animal::default(), &tamed());
    let sound = nautilus::handle_start_jump(&mut mob, 5, &zombie_nautilus::ZombieNautilus);
    assert_eq!(sound, sounds::ENTITY_ZOMBIE_NAUTILUS_DASH_LAND);
    assert_eq!(nautilus::dash_cooldown(&nautilus::state_of(&mob).1, 5), 40);
}

#[test]
fn a_copper_golem_weathers_a_stage_at_its_time_drawn_from_the_last_and_stops_oxidised() {
    let mut own = CopperGolemState::default();
    let mut level = draws();
    let mut replay = level.clone();
    // Not yet drawn: its next stage drawn from now, nothing else.
    assert_eq!(
        copper_golem::update_weathering(&mut own, &mut level, 1000),
        None
    );
    let first = 1000 + i64::from(replay.next_int_between(504_000, 552_000));
    assert_eq!(own.next_weathering, first);
    // Before its time, nothing.
    let weathered = copper_golem::update_weathering(&mut own, &mut level, first - 1);
    assert_eq!(
        weathered.map(|weathered| weathered.was_oxidised),
        Some(false)
    );
    assert_eq!(own.weather, copper_golem::UNAFFECTED);
    // Long after its time: a stage a tick, each drawn from the last stage's time.
    let late = first + 10_000_000;
    copper_golem::update_weathering(&mut own, &mut level, late);
    let second = first + i64::from(replay.next_int_between(504_000, 552_000));
    assert_eq!(
        (own.weather, own.next_weathering),
        (copper_golem::EXPOSED, second)
    );
    copper_golem::update_weathering(&mut own, &mut level, late + 1);
    let third = second + i64::from(replay.next_int_between(504_000, 552_000));
    assert_eq!(
        (own.weather, own.next_weathering),
        (copper_golem::WEATHERED, third)
    );
    // Oxidised: no next stage, and nothing drawn.
    copper_golem::update_weathering(&mut own, &mut level, late + 2);
    assert_eq!(
        (own.weather, own.next_weathering),
        (copper_golem::OXIDIZED, 0)
    );
    assert_eq!(level, replay);
    let weathered = copper_golem::update_weathering(&mut own, &mut level, late + 3);
    assert_eq!(
        weathered.map(|weathered| weathered.was_oxidised),
        Some(true)
    );
    assert_eq!(level, replay);
    // Waxed, never.
    let mut waxed = CopperGolemState {
        next_weathering: copper_golem::WAXED,
        ..CopperGolemState::default()
    };
    assert_eq!(
        copper_golem::update_weathering(&mut waxed, &mut level, late),
        None
    );
    assert_eq!(waxed.weather, copper_golem::UNAFFECTED);
    assert_eq!(level, replay);
}

#[test]
fn honeycomb_waxes_a_copper_golem_and_an_axe_takes_the_wax_off_or_scrapes_a_stage() {
    let mut mob = Mob::fresh();
    copper_golem::update(&mut mob, |golem| {
        golem.weather = copper_golem::WEATHERED;
        golem.next_weathering = 600_000;
    });
    let (result, did) = used(GOLEM, &mut mob, one(items::HONEYCOMB), false, &mut draws());
    assert_eq!(result, Interaction::SuccessServer);
    assert_eq!(
        did,
        [
            Did::LevelEvent {
                kind: 3003,
                pos: BlockPos::new(0, 64, 0),
                data: 0
            },
            Did::Sound {
                sound: sounds::ITEM_HONEYCOMB_WAX_ON,
                source: crate::api::SOUND_SOURCE_BLOCKS,
                at: [0.5, 64.5, 0.5],
                volume: 1.0,
                pitch: 1.0,
                but_the_player: false,
            },
        ]
    );
    assert_eq!(
        copper_golem::state_of(&mob).next_weathering,
        copper_golem::WAXED
    );
    // Waxed, honeycomb does nothing more.
    let (result, _) = used(GOLEM, &mut mob, one(items::HONEYCOMB), false, &mut draws());
    assert_eq!(result, Interaction::Pass);
    // The axe takes the wax off and leaves the stage.
    let (result, did) = used(GOLEM, &mut mob, one(items::IRON_AXE), false, &mut draws());
    assert_eq!(result, Interaction::SuccessServer);
    assert_eq!(sounds_of(&did), [sounds::ITEM_AXE_SCRAPE]);
    assert!(did.contains(&Did::LevelEvent {
        kind: 3004,
        pos: BlockPos::new(0, 64, 0),
        data: 0
    }));
    let own = copper_golem::state_of(&mob);
    assert_eq!(
        (own.weather, own.next_weathering),
        (copper_golem::WEATHERED, copper_golem::UNSET)
    );
    // Then it scrapes a stage at a time down to unaffected, and no further.
    for expected in [copper_golem::EXPOSED, copper_golem::UNAFFECTED] {
        let (result, did) = used(GOLEM, &mut mob, one(items::IRON_AXE), false, &mut draws());
        assert_eq!(result, Interaction::SuccessServer);
        assert!(did.contains(&Did::LevelEvent {
            kind: 3005,
            pos: BlockPos::new(0, 64, 0),
            data: 0
        }));
        assert_eq!(copper_golem::state_of(&mob).weather, expected);
    }
    let (result, did) = used(GOLEM, &mut mob, one(items::IRON_AXE), false, &mut draws());
    assert_eq!(result, Interaction::Pass);
    assert!(did.is_empty());
}

#[test]
fn an_empty_hand_takes_what_a_copper_golem_holds_and_shears_take_its_poppy() {
    let mut mob = Mob::fresh();
    let hand = usize::from(slots::MAIN_HAND);
    mob.body.equipment.slots[hand] = Some(ItemStack {
        item: items::COBBLESTONE,
        count: 16,
    });
    let (result, did) = used(GOLEM, &mut mob, None, false, &mut draws());
    assert_eq!(result, Interaction::Success);
    assert!(mob.body.equipment.slots[hand].is_none());
    let eye = f64::from(clustine_data::EntityTypeInfo::of(entity_types::COPPER_GOLEM).eye_height);
    assert_eq!(
        did,
        [Did::Throw {
            stack: ItemStack {
                item: items::COBBLESTONE,
                count: 16
            },
            at: [0.0, 64.0 + eye - 0.300_000_011_920_928_96, 0.0],
            pick_up_delay: crate::region::DEFAULT_PICK_UP_DELAY,
            motion: [0.300_000_011_920_928_96, 0.0, 0.0],
        }]
    );
    // With nothing to take and nothing on its antenna, an empty hand and shears pass.
    assert_eq!(
        used(GOLEM, &mut mob, None, false, &mut draws()).0,
        Interaction::Pass
    );
    assert_eq!(
        used(GOLEM, &mut mob, one(items::SHEARS), false, &mut draws()).0,
        Interaction::Pass
    );
    let antenna = usize::from(slots::SADDLE);
    mob.body.equipment.slots[antenna] = one(items::POPPY);
    let (result, did) = used(GOLEM, &mut mob, one(items::SHEARS), false, &mut draws());
    assert_eq!(result, Interaction::Success);
    assert!(mob.body.equipment.slots[antenna].is_none());
    assert_eq!(sounds_of(&did), [sounds::ENTITY_COPPER_GOLEM_SHEAR]);
    assert!(did.contains(&Did::Drop {
        stack: ItemStack {
            item: items::POPPY,
            count: 1
        },
        at: [0.0, 65.5, 0.0],
        pick_up_delay: crate::region::DEFAULT_PICK_UP_DELAY,
        push: None,
    }));
    assert_eq!(did.last(), Some(&Did::GameEvent("minecraft:shear")));
}

#[test]
fn the_copper_golem_sounds_by_its_weathering_and_steps_at_full_volume() {
    let source = DamageSource::of(DamageType::FALL);
    for (weather, hurt, death, step) in [
        (
            copper_golem::UNAFFECTED,
            sounds::ENTITY_COPPER_GOLEM_HURT,
            sounds::ENTITY_COPPER_GOLEM_DEATH,
            sounds::ENTITY_COPPER_GOLEM_STEP,
        ),
        (
            copper_golem::EXPOSED,
            sounds::ENTITY_COPPER_GOLEM_HURT,
            sounds::ENTITY_COPPER_GOLEM_DEATH,
            sounds::ENTITY_COPPER_GOLEM_STEP,
        ),
        (
            copper_golem::WEATHERED,
            sounds::ENTITY_COPPER_GOLEM_WEATHERED_HURT,
            sounds::ENTITY_COPPER_GOLEM_WEATHERED_DEATH,
            sounds::ENTITY_COPPER_GOLEM_WEATHERED_STEP,
        ),
        (
            copper_golem::OXIDIZED,
            sounds::ENTITY_COPPER_GOLEM_OXIDIZED_HURT,
            sounds::ENTITY_COPPER_GOLEM_OXIDIZED_DEATH,
            sounds::ENTITY_COPPER_GOLEM_OXIDIZED_STEP,
        ),
    ] {
        let mut mob = Mob::fresh();
        copper_golem::update(&mut mob, |golem| golem.weather = weather);
        assert_eq!(of(GOLEM).hurt_sound(&mob, &source), Some(hurt));
        assert_eq!(of(GOLEM).death_sound(&mob), Some(death));
        assert_eq!(of(GOLEM).step_sound(&mob), Some(step));
    }
    let mob = Mob::fresh();
    assert_eq!(of(GOLEM).ambient_sound(&mob), None);
    assert_eq!(of(GOLEM).step_volume(), 1.0);
    assert_eq!(
        of(GOLEM).spawn_sound(&mob),
        Some(sounds::ENTITY_COPPER_GOLEM_SPAWN)
    );
    // Every other type keeps `Entity`'s step and no spawn sound.
    assert_eq!(of(entity_types::COW as u16).step_volume(), 0.15);
    assert!(of(entity_types::COW as u16).plays_step_sounds());
    assert_eq!(of(entity_types::COW as u16).spawn_sound(&mob), None);
}

#[test]
fn a_golem_hurt_is_idle_again_and_counts_at_the_chest_it_has_open_or_its_other_half() {
    let mut mob = Mob::fresh();
    copper_golem::set_state(&mut mob, copper_golem::GETTING_ITEM);
    of(GOLEM).actually_hurt(&mut mob, 0);
    assert_eq!(copper_golem::state_of(&mob).state, copper_golem::IDLE);
    let chest = BlockPos::new(4, 64, 4);
    let other = BlockPos::new(5, 64, 4);
    assert!(!copper_golem::has_container_open(
        &copper_golem::state_of(&mob),
        chest,
        |_| None
    ));
    copper_golem::set_opened_chest_pos(&mut mob, chest);
    let own = copper_golem::state_of(&mob);
    assert!(copper_golem::has_container_open(&own, chest, |_| None));
    assert!(!copper_golem::has_container_open(&own, other, |_| None));
    assert!(copper_golem::has_container_open(&own, other, |_| Some(
        other
    )));
    copper_golem::clear_opened_chest_pos(&mut mob);
    assert_eq!(copper_golem::state_of(&mob).opened_chest, None);
}

#[test]
fn a_statue_faces_the_golem_s_quarter_and_its_poses_turn_in_the_report_s_order() {
    assert_eq!(copper_golem::facing_of_yaw(0.0), "south");
    assert_eq!(copper_golem::facing_of_yaw(44.9), "south");
    assert_eq!(copper_golem::facing_of_yaw(45.0), "west");
    assert_eq!(copper_golem::facing_of_yaw(180.0), "north");
    assert_eq!(copper_golem::facing_of_yaw(-90.0), "east");
    assert_eq!(copper_golem::facing_of_yaw(-135.1), "north");
    let statue = copper_golem::statue(3, "east");
    assert_eq!(
        statue,
        clustine_data::BlockState::parse(
            "minecraft:oxidized_copper_golem_statue[copper_golem_pose=star,facing=east,\
             waterlogged=false]"
        )
        .unwrap()
    );
    let mut state = copper_golem::statue(0, "north");
    let mut poses = Vec::new();
    for _ in 0..4 {
        state = copper_golem::next_pose(state);
        poses.push(state.property("copper_golem_pose").unwrap());
    }
    assert_eq!(poses, ["sitting", "running", "star", "standing"]);
}
