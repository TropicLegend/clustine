//! The tamed animals in a region, from the brief's sections 5.1 to 5.8: eggs make them
//! with their defaults and their variants drawn in the game's order, taming takes their
//! foods with the game's chance, a tamed one sits and stands at its owner's click,
//! follows them and teleports to them, a tamed wolf attacks what its owner hits and
//! defends them, a wolf hit by a survival player turns on them angry, tamed pairs breed
//! tamed young with their collars mixed, and a tick of them is a function of its inputs.

use clustine_data::{attributes as ids, entity_types, items};
use clustine_world::{
    Double, Due, Effect, EffectId, Effected, Entity, EntityChange, EntityData, EntityId, EntityUse,
    FiledEffect, Float,
};

use super::*;
use crate::living::MAX_AIR;
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::types::animal::{self, Animal, Own};
use crate::mobs::types::cat::{self, CatState};
use crate::mobs::types::ocelot::{self, OcelotState};
use crate::mobs::types::parrot::{self, ParrotState};
use crate::mobs::types::tamable::Tame;
use crate::mobs::types::wolf::{self, WolfState};
use crate::region::Random;

const PET: EntityId = EntityId(40);
const OTHER: EntityId = EntityId(42);
const PREY: EntityId = EntityId(44);

/// The UUID of player 1, who owns what these tests tame.
const OWNER: u128 = 1;

/// An animal of the type `kind` at `position`, standing, of its type's full health, with
/// what every animal shares `shared` and its type's own `own`.
fn animal_of<O: Own>(
    key: EntityId,
    kind: i32,
    position: [f64; 3],
    shared: &Animal,
    own: &O,
) -> Entity {
    let mut mob = Mob::fresh();
    let kind = kind as u16;
    mob.body.vitals.health =
        crate::mobs::body::attributes::supplied(kind, attribute(ids::MAX_HEALTH)).unwrap() as f32;
    mob.body.vitals.air = MAX_AIR;
    mob.state = animal::write(shared, own);
    Entity {
        portal: Default::default(),
        key,
        kind,
        uuid: uuid::Uuid::from_u128(u128::from(key.0)),
        position,
        motion: [0.0, -0.078_400_001_525_878_9, 0.0],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: mob.encode(),
    }
}

/// A wolf at `position` tamed by `owner`, not ordered to sit, its collar `collar`.
fn tamed_wolf(key: EntityId, position: [f64; 3], owner: u128, collar: u8) -> Entity {
    let own = WolfState {
        tame: tamed_by(owner),
        collar,
        ..WolfState::default()
    };
    let mut entity = animal_of(key, entity_types::WOLF, position, &Animal::default(), &own);
    let mut mob = Mob::decode(&entity.data).unwrap();
    wolf::taming_side_effects(&mut mob, true);
    entity.data = mob.encode();
    entity
}

fn tamed_by(owner: u128) -> Tame {
    let mut tame = Tame::default();
    tame.set_tame_flag(true);
    tame.owner = Some(owner);
    tame
}

fn wild_wolf(key: EntityId, position: [f64; 3]) -> Entity {
    animal_of(
        key,
        entity_types::WOLF,
        position,
        &Animal::default(),
        &WolfState::default(),
    )
}

fn sheep_at(key: EntityId, position: [f64; 3]) -> Entity {
    animal_of(
        key,
        entity_types::SHEEP,
        position,
        &Animal::default(),
        &crate::mobs::types::sheep::SheepState::default(),
    )
}

fn lay(region: &mut Region, entity: &Entity) {
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .unwrap()
        .apply_entity(&EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

fn record(region: &Region, key: EntityId) -> Option<Entity> {
    region.find_entity(key).map(|(_, entity)| entity.clone())
}

fn mob(region: &Region, key: EntityId) -> Mob {
    Mob::decode(&record(region, key).expect("the mob is here").data).unwrap()
}

fn wolf_of(region: &Region, key: EntityId) -> WolfState {
    wolf::state_of(&mob(region, key)).1
}

fn all_of(region: &Region, kind: i32) -> Vec<Entity> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter(|entity| i32::from(entity.kind) == kind)
        .cloned()
        .collect()
}

fn holding(region: &mut Region, stack: Option<ItemStack>) {
    checked(region, &moves(vec![set_slot(1, 0, stack), select(1, 0)]));
}

fn one(item: i32) -> Option<ItemStack> {
    Some(ItemStack::new(item, 1))
}

/// Player 1's use of the entity `key` with what they hold, in a tick.
fn use_on(region: &mut Region, key: EntityId) -> TickOutput {
    checked(
        region,
        &moves(vec![numbered(
            1,
            PlayerInput::Interact {
                entity: key,
                location: [0.0, 0.5, 0.0],
                sneaking: false,
            },
        )]),
    )
}

/// Player 1's use of the mob `key` holding `held`, between ticks: what it drew from
/// the mob's stream aside, as it was before, and what the use showed.
fn used_now(region: &mut Region, key: EntityId, held: Option<ItemStack>) -> (Random, TickOutput) {
    let stream = region.entity_stream(key).random;
    let (chunk, entity) = region
        .find_entity(key)
        .map(|(at, found)| (at, found.clone()))
        .unwrap();
    let player_key = region.players[&player(1)].entity_id;
    let from = region.players[&player(1)].pose.position;
    let mut output = TickOutput::default();
    region.mob_used_by(
        &entity,
        chunk,
        player_key,
        held,
        true,
        false,
        [from.x, from.y, from.z],
        &mut output,
    );
    (stream, output)
}

fn entity_events(output: &TickOutput, key: EntityId) -> Vec<u8> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::EntityEvent { entity, event, .. } if *entity == key => Some(*event),
            _ => None,
        })
        .collect()
}

fn idle_for(region: &mut Region, ticks: usize) {
    for _ in 0..ticks {
        checked(region, &TickInputs::default());
    }
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    crate::physics::distance_sqr(a, b).sqrt()
}

fn owner_at(region: &Region) -> [f64; 3] {
    let at = region.players[&player(1)].pose.position;
    [at.x, at.y, at.z]
}

/// The level's draws a spawn egg makes on a mob's behalf from player 1's stream, as
/// they were before the egg; and the new mob.
fn hatched(region: &mut Region, kind: i32) -> (Random, Mob) {
    let player_key = region.players[&player(1)].entity_id;
    let level = region.entity_stream(player_key).random;
    let mut output = TickOutput::default();
    let key = region
        .spawn_mob(
            kind as u16,
            player(1),
            BlockPos::new(3, -60, 3),
            true,
            true,
            &mut output,
        )
        .expect("made");
    (level, mob(region, key))
}

/// `Mob.finalizeSpawn`'s own draws: the follow range's bonus and the left hand.
fn mob_draws(level: &mut Random, made: &Mob) {
    let bonus = 0.114_850_000_000_000_01 * (level.next_double() - level.next_double());
    let left = level.next_float() < 0.05;
    let follow = made.body.attributes.made(ids::FOLLOW_RANGE as u16).unwrap();
    assert_eq!(follow.modifiers[0].amount.to_bits(), bonus.to_bits());
    assert_eq!(made.body.flags.left_handed, left);
}

#[test]
fn an_egg_makes_each_tamed_animal_wild_with_its_defaults_and_its_variants_drawn_in_the_game_s_order()
 {
    // The floor's biome is the registry's first, the badlands: a striped wolf.
    let mut region = on_ticking_floor_with(&[]);
    let (mut level, made) = hatched(&mut region, entity_types::WOLF);
    let _yaw = level.next_float();
    let variant = wolf::select_variant(Some("minecraft:badlands"), &mut level);
    assert_eq!(
        variant,
        animal::variants::id("minecraft:wolf_variant", "minecraft:striped")
    );
    let voice = level.next_int(7) as u8;
    mob_draws(&mut level, &made);
    let own = wolf::state_of(&made).1;
    assert_eq!((own.variant, own.sound), (variant, voice));
    assert!(!own.tame.is_tame() && own.tame.owner.is_none());
    assert_eq!((own.collar, own.anger.end), (14, -1));
    assert_eq!(made.body.vitals.health, 8.0);

    // A cat draws after its base, at the first day's full moon of eleven variants.
    let mut region = on_ticking_floor_with(&[]);
    let day = region.clock.day;
    let (mut level, made) = hatched(&mut region, entity_types::CAT);
    let _yaw = level.next_float();
    mob_draws(&mut level, &made);
    let mut replay = level.clone();
    let variant = cat::select_variant(day, &mut level);
    assert_eq!(variant, replay.next_int(11) as u8);
    let voice = level.next_int(2) as u8;
    let own = cat::state_of(&made).1;
    assert_eq!((own.variant, own.sound), (variant, voice));
    assert!(!own.tame.is_tame());
    assert_eq!(own.collar, 14);
    assert_eq!(made.body.vitals.health, 10.0);

    // A parrot's colour comes first, then its base's.
    let mut region = on_ticking_floor_with(&[]);
    let (mut level, made) = hatched(&mut region, entity_types::PARROT);
    let _yaw = level.next_float();
    let variant = level.next_int(5) as i32;
    mob_draws(&mut level, &made);
    let own = parrot::state_of(&made).1;
    assert_eq!(own.variant, variant);
    assert!(!own.tame.is_tame());
    assert_eq!(made.body.vitals.health, 6.0);

    // An ocelot draws nothing of its own, and trusts nobody.
    let mut region = on_ticking_floor_with(&[]);
    let (mut level, made) = hatched(&mut region, entity_types::OCELOT);
    let _yaw = level.next_float();
    mob_draws(&mut level, &made);
    assert!(!ocelot::state_of(&made).1.trusting);
    assert_eq!(made.body.vitals.health, 10.0);
}

#[test]
fn a_bone_tames_a_wild_wolf_one_time_in_three_of_its_own_draws_and_it_sits_at_forty_health() {
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &wild_wolf(PET, [2.5, -60.0, 0.5]));
    let mut tamed = false;
    for _ in 0..60 {
        let (mut replay, output) = used_now(&mut region, PET, one(items::BONE));
        let tames = replay.next_int(3) == 0;
        let own = wolf_of(&region, PET);
        if tames {
            assert_eq!(entity_events(&output, PET), [7]);
            assert!(own.tame.is_tame() && own.tame.ordered_to_sit);
            assert_eq!(own.tame.owner, Some(OWNER));
            let made = mob(&region, PET);
            assert_eq!(made.body.vitals.health, 40.0);
            assert_eq!(
                made.body
                    .attributes
                    .value(entity_types::WOLF as u16, attribute(ids::MAX_HEALTH)),
                40.0
            );
            tamed = true;
            break;
        }
        assert_eq!(entity_events(&output, PET), [6]);
        assert!(!own.tame.is_tame());
        checked(&mut region, &TickInputs::default());
    }
    assert!(tamed, "tamed within sixty bones");
    // Ordered to sit, it sits at its next turn of its goals.
    idle_for(&mut region, 3);
    assert!(wolf_of(&region, PET).tame.in_sitting_pose());
    // A second bone on a tamed wolf only toggles its sitting: it is no food of its.
    let (_, output) = used_now(&mut region, PET, one(items::BONE));
    assert!(entity_events(&output, PET).is_empty());
    assert!(!wolf_of(&region, PET).tame.ordered_to_sit);
}

#[test]
fn fish_tame_a_cat_and_seeds_a_parrot_with_the_game_s_chances_and_draws() {
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &animal_of(
            PET,
            entity_types::CAT,
            [2.5, -60.0, 0.5],
            &Animal::default(),
            &CatState::default(),
        ),
    );
    let mut tamed = false;
    for _ in 0..60 {
        let (mut replay, output) = used_now(&mut region, PET, one(items::COD));
        let own = cat::state_of(&mob(&region, PET)).1;
        if replay.next_int(3) == 0 {
            assert_eq!(entity_events(&output, PET), [7]);
            assert!(own.tame.is_tame() && own.tame.ordered_to_sit);
            assert_eq!(own.tame.owner, Some(OWNER));
            // Fed, it is kept: a stray of 2400 ticks may despawn, a fed one may not.
            assert!(mob(&region, PET).body.flags.persistent);
            tamed = true;
            break;
        }
        assert_eq!(entity_events(&output, PET), [6]);
        checked(&mut region, &TickInputs::default());
    }
    assert!(tamed);

    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &animal_of(
            PET,
            entity_types::PARROT,
            [2.5, -60.0, 0.5],
            &Animal::default(),
            &ParrotState::default(),
        ),
    );
    let mut tamed = false;
    for _ in 0..100 {
        let (mut replay, output) = used_now(&mut region, PET, one(items::WHEAT_SEEDS));
        // The eating sound's pitch first, then the chance of one in ten.
        let _pitch = parrot::pitch(&mut replay);
        let own = parrot::state_of(&mob(&region, PET)).1;
        if replay.next_int(10) == 0 {
            assert_eq!(entity_events(&output, PET), [7]);
            assert_eq!(own.tame.owner, Some(OWNER));
            tamed = true;
            break;
        }
        assert_eq!(entity_events(&output, PET), [6]);
        assert!(!own.tame.is_tame());
        checked(&mut region, &TickInputs::default());
    }
    assert!(tamed);
}

#[test]
fn an_ocelot_tempted_and_offered_fish_close_by_trusts_one_time_in_three() {
    let mut region = on_ticking_floor_with(&[]);
    holding(&mut region, one(items::COD));
    lay(
        &mut region,
        &animal_of(
            PET,
            entity_types::OCELOT,
            [2.5, -60.0, 0.5],
            &Animal::default(),
            &OcelotState::default(),
        ),
    );
    // Not tempted yet (its goals have not run): the fish is an animal's use, love and
    // no trust.
    let (_, output) = used_now(&mut region, PET, one(items::COD));
    assert_eq!(entity_events(&output, PET), [18]);
    assert!(!ocelot::state_of(&mob(&region, PET)).1.trusting);
    let mut trusted = false;
    for _ in 0..400 {
        checked(&mut region, &TickInputs::default());
        let tempted =
            ocelot::goal_state::<crate::mobs::ai::goals::Tempted>(&mob(&region, PET), 1).running;
        let near = distance(record(&region, PET).unwrap().position, owner_at(&region)) < 3.0;
        if !tempted || !near {
            continue;
        }
        let (mut replay, output) = used_now(&mut region, PET, one(items::COD));
        let trusts = replay.next_int(3) == 0;
        assert_eq!(
            entity_events(&output, PET),
            if trusts { [41] } else { [40] }
        );
        assert_eq!(ocelot::state_of(&mob(&region, PET)).1.trusting, trusts);
        if trusts {
            trusted = true;
            break;
        }
    }
    assert!(trusted);
}

#[test]
fn a_tamed_wolf_sits_and_stands_at_its_owner_s_click_and_sits_where_its_owner_is_not_found() {
    let mut region = on_ticking_floor_with(&[]);
    holding(&mut region, None);
    lay(&mut region, &tamed_wolf(PET, [2.5, -60.0, 0.5], OWNER, 14));
    idle_for(&mut region, 3);
    assert!(!wolf_of(&region, PET).tame.in_sitting_pose());
    use_on(&mut region, PET);
    assert!(wolf_of(&region, PET).tame.ordered_to_sit);
    idle_for(&mut region, 3);
    assert!(wolf_of(&region, PET).tame.in_sitting_pose());
    use_on(&mut region, PET);
    idle_for(&mut region, 3);
    let own = wolf_of(&region, PET);
    assert!(!own.tame.ordered_to_sit && !own.tame.in_sitting_pose());

    // A wolf whose owner the world does not know sits, though nobody ordered it to,
    // and a click of a player not its owner does not stand it up.
    let mut region = on_ticking_floor_with(&[]);
    holding(&mut region, None);
    lay(&mut region, &tamed_wolf(PET, [2.5, -60.0, 0.5], 77, 14));
    idle_for(&mut region, 3);
    let own = wolf_of(&region, PET);
    assert!(own.tame.in_sitting_pose() && !own.tame.ordered_to_sit);
    use_on(&mut region, PET);
    assert!(!wolf_of(&region, PET).tame.ordered_to_sit);
}

#[test]
fn a_tamed_wolf_follows_its_owner_from_ten_blocks_and_teleports_to_them_from_twelve() {
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &tamed_wolf(PET, [11.5, -60.0, 0.5], OWNER, 14));
    let mut near = false;
    for _ in 0..200 {
        checked(&mut region, &TickInputs::default());
        if distance(record(&region, PET).unwrap().position, owner_at(&region)) < 4.0 {
            near = true;
            break;
        }
    }
    assert!(near, "walked to its owner");

    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &tamed_wolf(PET, [15.5, -60.0, 0.5], OWNER, 14));
    let mut jumped = None;
    for tick in 0..10 {
        checked(&mut region, &TickInputs::default());
        let at = record(&region, PET).unwrap().position;
        if distance(at, owner_at(&region)) < 4.5 {
            jumped = Some((tick, at));
            break;
        }
    }
    let (tick, at) = jumped.expect("teleported");
    assert!(tick <= 2, "{tick}");
    // To a block's bottom middle two or three blocks from the owner's block.
    assert_eq!(at[0].fract().abs(), 0.5);
    assert_eq!(at[2].fract().abs(), 0.5);
}

#[test]
fn a_tamed_wolf_attacks_what_its_owner_hits_and_bites_it() {
    let mut region = on_ticking_floor_with(&[]);
    holding(&mut region, None);
    lay(&mut region, &tamed_wolf(PET, [6.5, -60.0, 4.5], OWNER, 14));
    lay(&mut region, &sheep_at(PREY, [2.5, -60.0, 0.5]));
    checked(
        &mut region,
        &moves(vec![numbered(1, PlayerInput::Attack { entity: PREY })]),
    );
    let mut targeted = false;
    let mut bitten = false;
    for _ in 0..400 {
        checked(&mut region, &TickInputs::default());
        targeted |= mob(&region, PET).body.combat.target == Some(PREY);
        let Some(sheep) = record(&region, PREY) else {
            bitten = true;
            break;
        };
        let sheep = Mob::decode(&sheep.data).unwrap();
        if sheep
            .body
            .combat
            .last_hurt_by
            .is_some_and(|(by, _)| by == PET)
        {
            bitten = true;
            break;
        }
    }
    assert!(targeted && bitten, "targeted {targeted}, bitten {bitten}");
}

/// The face of player 9, entity 900, standing at `x`, of the game mode `mode`, last hurt
/// by `hurt_by`.
fn face(x: f64, mode: u8, hurt_by: Option<(EntityId, u64)>, seq: u64) -> crate::api::FaceChange {
    let mut face = crate::api::PlayerFace::of(
        &Pose::at(Vec3::new(x, -60.0, 0.5)),
        false,
        false,
        &Default::default(),
        0,
        None,
    );
    face.game_mode = mode;
    face.last_hurt_by = hurt_by;
    crate::api::FaceChange::Face {
        player: player(9),
        entity: EntityId(900),
        hops: 0,
        seq: crate::api::FaceSeq(seq, 0),
        face,
    }
}

#[test]
fn a_tamed_wolf_defends_its_owner_from_the_mob_that_hurt_them() {
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &tamed_wolf(PET, [4.5, -60.0, 0.5], 9, 14));
    lay(&mut region, &sheep_at(PREY, [8.5, -60.0, 0.5]));
    let now = region.clock.game_time;
    let mut defended = false;
    for tick in 0..20 {
        let faces = if tick == 0 {
            vec![face(6.5, 0, Some((PREY, now)), 1)]
        } else {
            Vec::new()
        };
        checked(
            &mut region,
            &TickInputs {
                faces,
                ..TickInputs::default()
            },
        );
        if mob(&region, PET).body.combat.target == Some(PREY) {
            defended = true;
            break;
        }
    }
    assert!(defended);
}

/// A hit on the region's mob `key` by player 9 of another region (entity 900), as the
/// effect of their hit comes.
fn hit_from_beyond(key: EntityId, sequence: u64) -> TickInputs {
    TickInputs {
        effects: vec![FiledEffect {
            id: EffectId { sequence, index: 0 },
            effect: Effect {
                at: BlockPos::new(4, -60, 0),
                what: Effected::OnEntity {
                    key,
                    by: EntityUse::Attack {
                        look: [Float(0.0); 2],
                        player: None,
                        attacker: EntityId(900),
                        creative: false,
                        sprinting: false,
                        damage: Float(1.0),
                        from: [Double(6.5), Double(-60.0), Double(0.5)],
                    },
                },
            },
            due: Due::FromTaking,
        }],
        ..TickInputs::default()
    }
}

#[test]
fn a_wolf_hit_by_a_survival_player_turns_on_them_angry_for_twenty_to_thirty_nine_seconds() {
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &wild_wolf(PET, [4.5, -60.0, 0.5]));
    checked(
        &mut region,
        &TickInputs {
            faces: vec![face(6.5, 0, None, 1)],
            ..TickInputs::default()
        },
    );
    checked(&mut region, &hit_from_beyond(PET, 3));
    let mut angry = None;
    for _ in 0..6 {
        checked(&mut region, &TickInputs::default());
        let own = wolf_of(&region, PET);
        if own.anger.target == Some(EntityId(900)) {
            angry = Some(own.anger);
            break;
        }
    }
    let anger = angry.expect("angry");
    let now = region.clock.game_time as i64;
    assert!(anger.is_angry(now));
    assert!(
        (400..=781).contains(&(anger.end - now)),
        "{}",
        anger.end - now
    );
    assert_eq!(mob(&region, PET).body.combat.target, Some(EntityId(900)));
    // Clients are shown the end of its anger.
    let shown = crate::shown::mob_values(
        entity_types::WOLF as u16,
        &record(&region, PET).unwrap().data,
    );
    assert!(shown.contains(&(22, crate::shown::DataShown::Long(anger.end), false)));

    // A creative player's hit makes no enemy of them.
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &wild_wolf(PET, [4.5, -60.0, 0.5]));
    checked(
        &mut region,
        &TickInputs {
            faces: vec![face(6.5, 1, None, 1)],
            ..TickInputs::default()
        },
    );
    checked(&mut region, &hit_from_beyond(PET, 3));
    idle_for(&mut region, 6);
    let own = wolf_of(&region, PET);
    assert_eq!((own.anger.target, own.anger.end), (None, -1));
    assert_eq!(mob(&region, PET).body.combat.target, None);
}

#[test]
fn a_dye_colours_a_tamed_wolf_s_collar_for_its_owner_and_a_wild_one_s_not_at_all() {
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &tamed_wolf(PET, [2.5, -60.0, 0.5], OWNER, 14));
    lay(&mut region, &tamed_wolf(OTHER, [2.5, -60.0, 3.5], 77, 14));
    used_now(&mut region, PET, one(items::LIME_DYE));
    assert_eq!(wolf_of(&region, PET).collar, 5);
    // Another's wolf keeps its collar.
    used_now(&mut region, OTHER, one(items::LIME_DYE));
    assert_eq!(wolf_of(&region, OTHER).collar, 14);
    // The same colour again changes nothing, and does not sit it down.
    used_now(&mut region, PET, one(items::LIME_DYE));
    let own = wolf_of(&region, PET);
    assert_eq!(own.collar, 5);
    assert!(!own.tame.ordered_to_sit);
}

#[test]
fn two_tamed_wolves_fed_breed_a_tamed_pup_of_their_owner_with_their_collars_mixed() {
    let mut region = on_ticking_floor_with(&[]);
    // White and red make pink.
    lay(&mut region, &tamed_wolf(PET, [2.5, -60.0, 3.5], OWNER, 0));
    lay(
        &mut region,
        &tamed_wolf(OTHER, [4.0, -60.0, 3.5], OWNER, 14),
    );
    holding(&mut region, one(items::BEEF));
    let fed = use_on(&mut region, PET);
    assert_eq!(entity_events(&fed, PET), [18]);
    use_on(&mut region, OTHER);
    let mut pup = None;
    for _ in 0..300 {
        checked(&mut region, &TickInputs::default());
        if let Some(found) = all_of(&region, entity_types::WOLF)
            .into_iter()
            .find(|wolf| wolf.key != PET && wolf.key != OTHER)
        {
            pup = Some(found);
            break;
        }
    }
    let pup = Mob::decode(&pup.expect("a pup").data).unwrap();
    let (shared, own) = wolf::state_of(&pup);
    assert!(shared.is_baby());
    assert!(own.tame.is_tame());
    assert_eq!(own.tame.owner, Some(OWNER));
    assert_eq!(own.collar, 6);
    assert_eq!(pup.body.vitals.health, 40.0);

    // A wild pair fed is in love and breeds nothing.
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &wild_wolf(PET, [2.5, -60.0, 3.5]));
    lay(&mut region, &wild_wolf(OTHER, [4.0, -60.0, 3.5]));
    holding(&mut region, one(items::BEEF));
    use_on(&mut region, PET);
    use_on(&mut region, OTHER);
    idle_for(&mut region, 200);
    assert_eq!(all_of(&region, entity_types::WOLF).len(), 2);
}

#[test]
fn a_breeding_of_tamed_cats_draws_the_variant_then_the_mixed_collar_from_the_parent_s_stream() {
    let mut region = on_ticking_floor_with(&[]);
    let tabby = animal::variants::id("minecraft:cat_variant", "minecraft:tabby");
    let calico = animal::variants::id("minecraft:cat_variant", "minecraft:calico");
    let mut love = Animal::default();
    love.set_in_love(0);
    let cat_of = |key, x, variant, collar| {
        animal_of(
            key,
            entity_types::CAT,
            [x, -60.0, 2.5],
            &love,
            &CatState {
                tame: tamed_by(OWNER),
                variant,
                collar,
                ..CatState::default()
            },
        )
    };
    lay(&mut region, &cat_of(PET, 2.5, tabby, 14));
    lay(&mut region, &cat_of(OTHER, 4.5, calico, 3));
    let mut replay = region.entity_stream(PET).random;
    let mut output = TickOutput::default();
    assert!(region.breed_with(OTHER, PET, &mut output));
    let kitten = all_of(&region, entity_types::CAT)
        .into_iter()
        .find(|cat| cat.key != PET && cat.key != OTHER)
        .expect("a kitten");
    let own = cat::state_of(&Mob::decode(&kitten.data).unwrap()).1;
    let variant = if replay.next_bool() { tabby } else { calico };
    // Red and light blue are mixed by no recipe: one of the two, by a coin.
    let collar = if replay.next_bool() { 14 } else { 3 };
    assert_eq!((own.variant, own.collar), (variant, collar));
    assert_eq!(own.tame.owner, Some(OWNER));
    // A kitten's voice is the classic one, never drawn.
    assert_eq!(own.sound, CatState::default().sound);
}

#[test]
fn a_cookie_kills_a_parrot_and_a_tamed_one_on_the_ground_sits_at_its_owner_s_click() {
    let mut region = on_ticking_floor_with(&[]);
    let own = ParrotState {
        tame: tamed_by(OWNER),
        ..ParrotState::default()
    };
    lay(
        &mut region,
        &animal_of(
            PET,
            entity_types::PARROT,
            [2.5, -60.0, 0.5],
            &Animal::default(),
            &own,
        ),
    );
    used_now(&mut region, PET, None);
    assert!(parrot::state_of(&mob(&region, PET)).1.tame.ordered_to_sit);
    used_now(&mut region, PET, one(items::COOKIE));
    assert!(mob(&region, PET).body.vitals.dying());
}

#[test]
fn a_parrot_flies_without_gravity_while_its_move_control_steers_it() {
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &animal_of(
            PET,
            entity_types::PARROT,
            [2.5, -60.0, 0.5],
            &Animal::default(),
            &ParrotState::default(),
        ),
    );
    let mut flew = false;
    for _ in 0..600 {
        checked(&mut region, &TickInputs::default());
        let own = parrot::state_of(&mob(&region, PET)).1;
        let at = record(&region, PET).unwrap().position;
        if own.no_gravity && at[1] > -59.0 {
            flew = true;
            break;
        }
    }
    assert!(flew, "it rose off the ground");
}

#[test]
fn a_tick_of_tamed_animals_is_a_function_of_its_inputs() {
    let run = || {
        let mut region = on_ticking_floor_with(&[]);
        holding(&mut region, one(items::COD));
        lay(&mut region, &tamed_wolf(PET, [9.5, -60.0, 3.5], OWNER, 14));
        lay(&mut region, &wild_wolf(EntityId(46), [12.5, -60.0, 12.5]));
        lay(&mut region, &sheep_at(PREY, [14.5, -60.0, 12.5]));
        lay(
            &mut region,
            &animal_of(
                OTHER,
                entity_types::CAT,
                [3.5, -60.0, 6.5],
                &Animal::default(),
                &CatState::default(),
            ),
        );
        lay(
            &mut region,
            &animal_of(
                EntityId(48),
                entity_types::OCELOT,
                [6.5, -60.0, 10.5],
                &Animal::default(),
                &OcelotState::default(),
            ),
        );
        lay(
            &mut region,
            &animal_of(
                EntityId(50),
                entity_types::PARROT,
                [1.5, -60.0, 12.5],
                &Animal::default(),
                &ParrotState::default(),
            ),
        );
        let mut changes = Vec::new();
        for _ in 0..200 {
            let output = checked(&mut region, &TickInputs::default());
            changes.push((output.entity_changes.clone(), output.effects.clone()));
        }
        let records: Vec<Option<Entity>> =
            [PET, EntityId(46), PREY, OTHER, EntityId(48), EntityId(50)]
                .into_iter()
                .map(|key| record(&region, key))
                .collect();
        (changes, records)
    };
    assert_eq!(run(), run());
}

#[test]
fn every_tamed_animal_s_record_carries_what_clients_are_shown_of_it() {
    let wolf = tamed_wolf(PET, [2.5, -60.0, 0.5], OWNER, 5);
    let shown = crate::shown::mob_values(entity_types::WOLF as u16, &wolf.data);
    assert!(shown.contains(&(18, crate::shown::DataShown::Byte(4), false)));
    assert!(shown.contains(&(
        19,
        crate::shown::DataShown::OptionalUuid(Some(OWNER)),
        false
    )));
    assert!(shown.contains(&(21, crate::shown::DataShown::Int(5), false)));
    let EntityData::Mob { .. } = wolf.data else {
        unreachable!();
    };
}
