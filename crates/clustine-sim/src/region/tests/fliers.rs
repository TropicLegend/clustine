//! The sniffer, the allay and the happy ghast in a region, from the brief's sections 7.9
//! to 7.11, as far as a player meets them without their brains: an egg makes each with
//! no draw of its own; a sniffer egg cracks twice and hatches, faster on moss; sniffers
//! bred lay an egg; a dried ghast under water becomes a ghastling by its stages; a
//! ghastling fed grows; a harness goes on and comes off; an allay takes an item, gathers
//! what it holds and gives it back; a happy ghast holds leashes and shows the quad one,
//! stays still under a player, and floats about.

use clustine_blocks::data::{number, set};
use clustine_data::{BlockState, blocks, entity_types, items, sounds};
use clustine_world::{Double, Entity, EntityChange, EntityData, EntityId, Leash, LeashHolder};

use super::*;
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::body::attributes::supplied;
use crate::mobs::types::allay;
use crate::mobs::types::animal::{self, Animal};
use crate::mobs::types::happy_ghast::{self, HappyGhastState};
use crate::mobs::types::sniffer::{self, SnifferState};
use crate::region::Random;
use crate::region::sniffer_eggs;

const ONE: EntityId = EntityId(40);
const TWO: EntityId = EntityId(42);
const THREE: EntityId = EntityId(44);

/// A mob of the type `kind` at `position` as `mob` has it, of its type's full health.
fn mob_at(key: EntityId, kind: i32, position: [f64; 3], mut mob: Mob) -> Entity {
    let kind = kind as u16;
    mob.body.vitals.health =
        supplied(kind, attribute(clustine_data::attributes::MAX_HEALTH)).unwrap() as f32;
    mob.body.vitals.air = crate::living::MAX_AIR;
    Entity {
        key,
        kind,
        uuid: uuid::Uuid::from_u128(u128::from(key.0)),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: mob.encode(),
    }
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

fn take_away(region: &mut Region, key: EntityId) {
    let chunk = region.entity_chunks.remove(&key).unwrap();
    region
        .chunks
        .get_mut(&chunk)
        .unwrap()
        .apply_entity(&EntityChange::Gone(key));
}

fn record(region: &Region, key: EntityId) -> Option<Entity> {
    region.find_entity(key).map(|(_, entity)| entity.clone())
}

fn mob(region: &Region, key: EntityId) -> Mob {
    Mob::decode(&record(region, key).expect("the mob is here").data).unwrap()
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

fn items_lying(region: &Region) -> Vec<ItemStack> {
    all_of(region, entity_types::ITEM)
        .into_iter()
        .filter_map(|entity| match entity.data {
            EntityData::Item { stack, .. } => Some(stack),
            _ => None,
        })
        .collect()
}

fn item_at(key: EntityId, stack: ItemStack, position: [f64; 3]) -> Entity {
    Entity {
        key,
        kind: entity_types::ITEM as u16,
        uuid: uuid::Uuid::from_u128(u128::from(key.0)),
        position,
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
        data: EntityData::Item {
            stack,
            pick_up_delay: 0,
            thrower: None,
            health: 5,
            age_offset: 0,
        },
    }
}

fn holding(region: &mut Region, stack: Option<ItemStack>) {
    checked(region, &moves(vec![set_slot(1, 0, stack), select(1, 0)]));
}

fn one(item: i32) -> ItemStack {
    ItemStack { item, count: 1 }
}

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

fn sounds_of(output: &TickOutput) -> Vec<(i32, Option<PlayerId>)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound { sound, except, .. } => Some((*sound, *except)),
            _ => None,
        })
        .collect()
}

fn idle(region: &mut Region, ticks: usize) {
    for _ in 0..ticks {
        checked(region, &TickInputs::default());
    }
}

fn ghast_with(age: i32, own: &HappyGhastState) -> Mob {
    let mut shared = Animal::default();
    shared.set_age(age, 0);
    let mut mob = Mob::fresh();
    mob.state = animal::write(&shared, own);
    mob
}

fn sniffer_in_love() -> Mob {
    let mut shared = Animal::default();
    shared.set_in_love(0);
    let mut mob = Mob::fresh();
    mob.state = animal::write(&shared, &SnifferState::default());
    mob
}

#[test]
fn an_egg_makes_each_flier_with_its_defaults_and_only_mob_s_draws_on_its_behalf() {
    for (kind, health) in [
        (entity_types::SNIFFER, 14.0),
        (entity_types::ALLAY, 20.0),
        (entity_types::HAPPY_GHAST, 20.0),
    ] {
        let mut region = on_ticking_floor_with(&[]);
        let player_key = region.players[&player(1)].entity_id;
        let mut level = region.entity_stream(player_key).random;
        let mut output = TickOutput::default();
        let pos = BlockPos::new(3, -60, 3);
        let key = region
            .spawn_mob(kind as u16, player(1), pos, true, true, &mut output)
            .expect("made");
        let made = mob(&region, key);
        assert_eq!(made.body.vitals.health, health, "{kind}");
        // The level's: its yaw, then `Mob`'s follow range and hand, and nothing of its
        // type's own.
        let _yaw = level.next_float();
        let amount = 0.114_850_000_000_000_01 * (level.next_double() - level.next_double());
        let left = level.next_float() < 0.05;
        assert_eq!(made.body.flags.left_handed, left, "{kind}");
        let follow = attribute(clustine_data::attributes::FOLLOW_RANGE);
        let mut attributes = made.body.attributes.clone();
        let instance = attributes.instance(kind as u16, follow).unwrap();
        assert_eq!(instance.modifiers[0].amount, amount, "{kind}");
        // Its own state is a fresh one's: an idle sniffer, an allay that may duplicate,
        // a grown happy ghast (its egg makes no ghastling).
        assert_eq!(made.state, clustine_world::TypeState::default(), "{kind}");
        assert!(!animal::is_baby(&made));
    }
}

#[test]
fn a_sniffer_egg_cracks_twice_and_hatches_a_snifflet_in_three_waits_halved_on_moss() {
    for (below, wait) in [(blocks::DIRT, 8000), (blocks::MOSS_BLOCK, 4000)] {
        let egg = BlockPos::new(5, -60, 5);
        let mut region = on_ticking_floor_with(&[(BlockPos::new(5, -61, 5), below)]);
        let mut output = TickOutput::default();
        region.set_block(egg, blocks::SNIFFER_EGG, &mut output);
        // Placed: a wait and up to 299 more, and on moss the boost shown.
        let delays: Vec<i32> = region
            .ticks_to_save(egg.chunk())
            .iter()
            .filter(|tick| tick.at == clustine_world::InChunk::new(5, -60, 5))
            .map(|tick| tick.delay)
            .collect();
        assert_eq!(delays.len(), 1, "{below:?}");
        assert!(
            (wait - 1..wait + 300).contains(&delays[0]),
            "{below:?}: {delays:?}"
        );
        let boosted = output
            .events
            .iter()
            .any(|event| matches!(event, RegionEvent::LevelEvent { kind: 3009, .. }));
        assert_eq!(boosted, below == blocks::MOSS_BLOCK);
        assert_eq!(sniffer_eggs::wait(below == blocks::MOSS_BLOCK), wait);
        for stage in 1..=2 {
            let mut output = TickOutput::default();
            let state = region.block(egg).unwrap();
            assert!(region.sniffer_egg_tick(egg, state, &mut output));
            assert_eq!(number(region.block(egg).unwrap(), "hatch"), Some(stage));
            assert!(sounds_of(&output).contains(&(sounds::BLOCK_SNIFFER_EGG_CRACK, None)));
            // Each crack waits again.
            assert!(region.has_scheduled_tick(
                egg,
                clustine_world::Ticked::Block(clustine_blocks::data::tick_id(state))
            ));
        }
        let mut output = TickOutput::default();
        let state = region.block(egg).unwrap();
        region.sniffer_egg_tick(egg, state, &mut output);
        assert_eq!(region.block(egg), Some(blocks::AIR));
        assert!(sounds_of(&output).contains(&(sounds::BLOCK_SNIFFER_EGG_HATCH, None)));
        let babies = all_of(&region, entity_types::SNIFFER);
        assert_eq!(babies.len(), 1);
        let baby = &babies[0];
        assert_eq!(baby.position, [5.5, -59.5, 5.5]);
        let made = Mob::decode(&baby.data).unwrap();
        assert_eq!(animal::animal_of(&made.state).age, sniffer::BABY_START_AGE);
        // Not an egg, nothing.
        assert!(!region.sniffer_egg_tick(egg, blocks::STONE, &mut output));
    }
}

#[test]
fn two_sniffers_in_love_breed_an_egg_and_no_child_with_its_plop() {
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &mob_at(
            ONE,
            entity_types::SNIFFER,
            [3.5, -60.0, 3.5],
            sniffer_in_love(),
        ),
    );
    lay(
        &mut region,
        &mob_at(
            TWO,
            entity_types::SNIFFER,
            [5.5, -60.0, 3.5],
            sniffer_in_love(),
        ),
    );
    let mut output = TickOutput::default();
    assert!(region.breed_with(TWO, ONE, &mut output));
    assert_eq!(all_of(&region, entity_types::SNIFFER).len(), 2, "no child");
    assert_eq!(items_lying(&region), [one(items::SNIFFER_EGG)]);
    for key in [ONE, TWO] {
        let shared = animal::animal_of(&mob(&region, key).state);
        assert_eq!(shared.age, animal::BRED_AGE);
        assert!(!shared.in_love(0));
    }
    assert_eq!(all_of(&region, entity_types::EXPERIENCE_ORB).len(), 1);
    assert!(sounds_of(&output).contains(&(sounds::BLOCK_SNIFFER_EGG_PLOP, None)));
    // A digging sniffer does not mate.
    let mut region = on_ticking_floor_with(&[]);
    let mut busy = sniffer_in_love();
    sniffer::update(&mut busy, |own| own.state = sniffer::State::Digging);
    lay(
        &mut region,
        &mob_at(ONE, entity_types::SNIFFER, [3.5, -60.0, 3.5], busy),
    );
    lay(
        &mut region,
        &mob_at(
            TWO,
            entity_types::SNIFFER,
            [5.5, -60.0, 3.5],
            sniffer_in_love(),
        ),
    );
    assert!(!region.breed_with(TWO, ONE, &mut TickOutput::default()));
}

fn dried_ghast(waterlogged: bool, facing: &str, hydration: u8) -> BlockState {
    let state = set(blocks::DRIED_GHAST, "facing", facing);
    let state = set(state, "hydration", &hydration.to_string());
    clustine_blocks::data::set_flag(state, "waterlogged", waterlogged)
}

#[test]
fn a_dried_ghast_under_water_takes_up_water_by_three_stages_and_wakes_as_a_ghastling() {
    let at = BlockPos::new(6, -60, 6);
    let mut region = on_ticking_floor_with(&[(at, dried_ghast(true, "east", 0))]);
    for stage in 1..=3 {
        let mut output = TickOutput::default();
        let state = region.block(at).unwrap();
        assert!(region.dried_ghast_tick(at, state, &mut output));
        assert_eq!(number(region.block(at).unwrap(), "hydration"), Some(stage));
        assert!(sounds_of(&output).contains(&(sounds::BLOCK_DRIED_GHAST_TRANSITION, None)));
    }
    let mut output = TickOutput::default();
    let state = region.block(at).unwrap();
    region.dried_ghast_tick(at, state, &mut output);
    // Its water stays where it was.
    assert_eq!(region.block(at), Some(blocks::WATER));
    let ghasts = all_of(&region, entity_types::HAPPY_GHAST);
    assert_eq!(ghasts.len(), 1);
    let ghastling = &ghasts[0];
    assert_eq!(ghastling.position, [6.5, -60.0, 6.5]);
    // Facing east, as the block did.
    assert_eq!(ghastling.yaw, 270.0);
    let made = Mob::decode(&ghastling.data).unwrap();
    assert!(animal::is_baby(&made));
    assert_eq!(made.body.head.head_yaw, 270.0);
    assert!(sounds_of(&output).contains(&(sounds::ENTITY_GHASTLING_SPAWN, None)));
    // A ghastling breathes in the water it woke in.
    idle(&mut region, 3);
    let after = mob(&region, ghastling.key);
    assert_eq!(after.body.vitals.air, crate::living::MAX_AIR);

    // Out of water, one dries a stage at a time and stays dry.
    let at = BlockPos::new(8, -60, 8);
    let mut region = on_ticking_floor_with(&[(at, dried_ghast(false, "north", 2))]);
    let mut output = TickOutput::default();
    region.dried_ghast_tick(at, dried_ghast(false, "north", 2), &mut output);
    assert_eq!(region.block(at), Some(dried_ghast(false, "north", 1)));
    region.dried_ghast_tick(at, dried_ghast(false, "north", 1), &mut output);
    region.dried_ghast_tick(at, dried_ghast(false, "north", 0), &mut output);
    assert_eq!(region.block(at), Some(dried_ghast(false, "north", 0)));
    assert!(all_of(&region, entity_types::HAPPY_GHAST).is_empty());
}

#[test]
fn a_ghastling_fed_snowballs_grows_and_a_grown_one_takes_a_harness_that_shears_take_off() {
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &mob_at(
            ONE,
            entity_types::HAPPY_GHAST,
            [2.5, -60.0, 0.5],
            ghast_with(-24_000, &HappyGhastState::default()),
        ),
    );
    holding(&mut region, Some(one(items::SNOWBALL)));
    let before = animal::animal_of(&mob(&region, ONE).state);
    let now = record(&region, ONE).unwrap().lived;
    use_on(&mut region, ONE);
    let after = animal::animal_of(&mob(&region, ONE).state);
    let left = -before.age(now);
    let gained = animal::speed_up_seconds(left) * 20;
    assert!(gained > 0);
    assert!(after.age(now) >= before.age(now) + gained, "{after:?}");

    // A grown one, harnessed and shorn.
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &mob_at(
            ONE,
            entity_types::HAPPY_GHAST,
            [2.5, -60.0, 0.5],
            ghast_with(0, &HappyGhastState::default()),
        ),
    );
    holding(&mut region, Some(one(items::LIGHT_BLUE_HARNESS)));
    let output = use_on(&mut region, ONE);
    let body = usize::from(crate::living::slots::BODY);
    assert_eq!(
        mob(&region, ONE).body.equipment.slots[body],
        Some(one(items::LIGHT_BLUE_HARNESS))
    );
    assert!(sounds_of(&output).contains(&(sounds::ENTITY_HAPPY_GHAST_EQUIP, None)));
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::EquipmentChanged { entity, .. } if *entity == ONE
    )));
    holding(&mut region, Some(one(items::SHEARS)));
    let output = use_on(&mut region, ONE);
    assert_eq!(mob(&region, ONE).body.equipment.slots[body], None);
    assert!(sounds_of(&output).contains(&(sounds::ENTITY_HAPPY_GHAST_UNEQUIP, None)));
    assert_eq!(items_lying(&region), [one(items::LIGHT_BLUE_HARNESS)]);
}

#[test]
fn an_allay_given_an_item_gathers_its_kind_near_and_gives_all_of_it_back() {
    let mut region = on_ticking_floor_with(&[]);
    let mut made = Mob::fresh();
    made.body.flags.persistent = true;
    lay(
        &mut region,
        &mob_at(ONE, entity_types::ALLAY, [2.5, -60.0, 0.5], made),
    );
    holding(&mut region, Some(one(items::DIAMOND)));
    let output = use_on(&mut region, ONE);
    // The giver does not hear it; the hand holds one.
    assert!(sounds_of(&output).contains(&(sounds::ENTITY_ALLAY_ITEM_GIVEN, Some(player(1)))));
    let given = mob(&region, ONE);
    assert_eq!(given.body.equipment.slots[0], Some(one(items::DIAMOND)));
    let uuid = player(1).0.as_u128();
    assert_eq!(allay::read(&given).liked_player, Some(uuid));
    // Diamonds and dirt beside it: it takes the diamonds, shown as taken, and leaves the
    // dirt.
    let at = record(&region, ONE).unwrap().position;
    lay(
        &mut region,
        &item_at(
            TWO,
            ItemStack {
                item: items::DIAMOND,
                count: 3,
            },
            at,
        ),
    );
    lay(&mut region, &item_at(THREE, one(items::DIRT), at));
    let output = checked(&mut region, &TickInputs::default());
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::ItemTaken { item, collector, count: 3, .. } if *item == TWO && *collector == ONE
    )));
    assert!(record(&region, TWO).is_none());
    assert!(record(&region, THREE).is_some());
    assert_eq!(
        allay::read(&mob(&region, ONE)).inventory,
        Some(ItemStack {
            item: items::DIAMOND,
            count: 3,
        })
    );
    // An empty hand takes its item back, and the three fall at its eyes.
    holding(&mut region, None);
    let output = use_on(&mut region, ONE);
    assert!(sounds_of(&output).contains(&(sounds::ENTITY_ALLAY_ITEM_TAKEN, Some(player(1)))));
    let taken = mob(&region, ONE);
    assert_eq!(taken.body.equipment.slots[0], None);
    assert_eq!(allay::read(&taken).liked_player, None);
    let lying = items_lying(&region);
    assert!(lying.contains(&ItemStack {
        item: items::DIAMOND,
        count: 3,
    }));
    assert!(
        region.players[&player(1)]
            .inventory
            .0
            .iter()
            .flatten()
            .any(|stack| stack.item == items::DIAMOND)
    );
}

#[test]
fn a_happy_ghast_holds_a_sniffer_by_four_and_shows_it_and_a_cow_by_one_and_does_not() {
    let mut region = on_ticking_floor_with(&[]);
    let mut ghast = ghast_with(0, &HappyGhastState::default());
    // Without AI, it stays where it is and still ticks.
    ghast.body.flags.no_ai = true;
    lay(
        &mut region,
        &mob_at(ONE, entity_types::HAPPY_GHAST, [6.5, -60.0, 6.5], ghast),
    );
    let mut cow = mob_at(TWO, entity_types::COW, [3.5, -60.0, 6.5], Mob::fresh());
    cow.riding.leash = Some(Box::new(Leash {
        holder: LeashHolder::Entity(ONE),
        angular_momentum: Double(0.0),
    }));
    lay(&mut region, &cow);
    idle(&mut region, 3);
    assert_eq!(
        record(&region, TWO).unwrap().riding.leash.unwrap().holder,
        LeashHolder::Entity(ONE),
        "the cow is held"
    );
    assert!(!happy_ghast::read(&mob(&region, ONE)).leash_holder);
    // A sniffer, held by four, makes it show that it holds one.
    let mut held = mob_at(
        THREE,
        entity_types::SNIFFER,
        [6.5, -60.0, 3.0],
        sniffer_in_love(),
    );
    held.riding.leash = Some(Box::new(Leash {
        holder: LeashHolder::Entity(ONE),
        angular_momentum: Double(0.0),
    }));
    lay(&mut region, &held);
    let mut shown = false;
    for _ in 0..3 {
        let output = checked(&mut region, &TickInputs::default());
        shown |= output.events.iter().any(|event| {
            matches!(event, RegionEvent::EntityTracked(report) if report.entity == ONE && report.data.is_some())
        });
    }
    assert!(happy_ghast::read(&mob(&region, ONE)).leash_holder);
    assert!(shown, "clients are told");
    let values = crate::shown::mob_values(
        entity_types::HAPPY_GHAST as u16,
        &record(&region, ONE).unwrap().data,
    );
    assert!(values.contains(&(18, crate::shown::DataShown::Boolean(true), false)));
    // Gone, four of its ticks later it holds none.
    take_away(&mut region, THREE);
    idle(&mut region, 3);
    assert!(happy_ghast::read(&mob(&region, ONE)).leash_holder);
    idle(&mut region, 2);
    assert!(!happy_ghast::read(&mob(&region, ONE)).leash_holder);
}

#[test]
fn a_quad_leash_pulls_by_four_springs_a_quarter_each_and_a_single_one_by_one() {
    use crate::region::leashes::{End, Spring, elastic_pull_with};
    let leashee = End {
        position: [0.0, 0.0, 0.0],
        yaw: 30.0,
        width: 1.9,
        height: 1.75,
        movement: [0.0; 3],
    };
    let holder = End {
        position: [14.0, 3.0, 2.0],
        yaw: 0.0,
        width: 4.0,
        height: 4.0,
        movement: [0.0; 3],
    };
    let quad = Spring {
        quad: true,
        elastic: 6.0,
        snap: 12.0,
    };
    let (mut motion, mut momentum) = ([0.0; 3], 0.0);
    assert!(elastic_pull_with(
        &leashee,
        &holder,
        &mut motion,
        &mut momentum,
        quad
    ));
    // By hand: the four springs of `SHARED_QUAD_ATTACHMENT_POINTS`, summed in order and
    // quartered, then as for one.
    let points = [
        [-0.5, 0.5, 0.5],
        [-0.5, 0.5, -0.5],
        [0.5, 0.5, -0.5],
        [0.5, 0.5, 0.5],
    ];
    let turn = |v: [f64; 3], angle: f32| crate::region::riding::y_rot(v, -angle);
    let (mut sum, mut torque) = ([0.0_f64; 3], 0.0_f64);
    for point in points {
        let offset = turn(
            [
                point[0] * 1.9_f32 as f64,
                point[1] * 1.75_f32 as f64,
                point[2] * 1.9_f32 as f64,
            ],
            30.0 * 0.017_453_292,
        );
        let at = crate::physics::add(leashee.position, offset);
        let held = crate::physics::add(
            holder.position,
            turn([point[0] * 4.0, point[1] * 4.0, point[2] * 4.0], 0.0),
        );
        let d = crate::physics::sub(held, at);
        let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let mut force = crate::physics::scale(crate::physics::normalize(d), length - 6.0);
        let t = offset[2] * force[0] - offset[0] * force[2];
        // The leashee stands still: the force is along its (no) movement.
        force = crate::physics::scale(force, 0.300_000_011_920_928_96);
        sum = [sum[0] + force[0], sum[1] + force[1], sum[2] + force[2]];
        torque += t;
    }
    let force = crate::physics::scale(sum, 0.25);
    assert_eq!(momentum, 10.0 * (torque * 0.25));
    assert_eq!(motion, [force[0] * 0.8, force[1] * 0.2, force[2] * 0.8]);
}

#[test]
fn a_happy_ghast_with_a_player_on_top_stays_still_and_shows_it() {
    let mut region = on_ticking_floor_with(&[]);
    let mut ghast = ghast_with(0, &HappyGhastState::default());
    ghast.body.flags.no_ai = true;
    // Its top at the player's feet (the player stands at -60 above the spawn column).
    let height = f64::from(clustine_data::EntityTypeInfo::of(entity_types::HAPPY_GHAST).height);
    lay(
        &mut region,
        &mob_at(
            ONE,
            entity_types::HAPPY_GHAST,
            [0.5, -60.0 - height, 0.5],
            ghast,
        ),
    );
    idle(&mut region, 1);
    let own = happy_ghast::read(&mob(&region, ONE));
    assert_eq!(own.still_timeout, happy_ghast::MAX_STILL_TIMEOUT);
    assert!(own.stays_still);
    // Elsewhere, one is not.
    let mut region = on_ticking_floor_with(&[]);
    let mut ghast = ghast_with(0, &HappyGhastState::default());
    ghast.body.flags.no_ai = true;
    lay(
        &mut region,
        &mob_at(ONE, entity_types::HAPPY_GHAST, [8.5, -50.0, 8.5], ghast),
    );
    idle(&mut region, 1);
    assert!(!happy_ghast::read(&mob(&region, ONE)).stays_still);
}

#[test]
fn a_grown_happy_ghast_floats_about_by_impulses_and_keeps_a_home() {
    let mut region = on_ticking_floor_with(&[]);
    let start = [8.5, -55.0, 8.5];
    lay(
        &mut region,
        &mob_at(
            ONE,
            entity_types::HAPPY_GHAST,
            start,
            ghast_with(0, &HappyGhastState::default()),
        ),
    );
    idle(&mut region, 40);
    let after = record(&region, ONE).unwrap();
    let made = Mob::decode(&after.data).unwrap();
    assert_ne!(after.position, start, "it floats");
    // It does not fall as a walker would: it has no gravity in its flight.
    assert!(after.position[1] > -60.5);
    // Its home: 64 about where it was, bare and grown.
    assert_eq!(made.body.home.radius, 64);
    assert!(made.body.home.centre.is_some());
    // A ghastling's is 32, and it does not float about without its brain.
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &mob_at(
            ONE,
            entity_types::HAPPY_GHAST,
            start,
            ghast_with(-24_000, &HappyGhastState::default()),
        ),
    );
    idle(&mut region, 10);
    let after = record(&region, ONE).unwrap();
    assert_eq!(Mob::decode(&after.data).unwrap().body.home.radius, 32);
    assert_eq!(after.position, start);
}

#[test]
fn a_tick_of_fliers_is_a_function_of_its_inputs() {
    let run = || {
        let mut region = on_ticking_floor_with(&[]);
        lay(
            &mut region,
            &mob_at(
                ONE,
                entity_types::HAPPY_GHAST,
                [8.5, -55.0, 8.5],
                ghast_with(0, &HappyGhastState::default()),
            ),
        );
        lay(
            &mut region,
            &mob_at(TWO, entity_types::ALLAY, [4.5, -60.0, 4.5], Mob::fresh()),
        );
        lay(
            &mut region,
            &mob_at(
                THREE,
                entity_types::SNIFFER,
                [10.5, -60.0, 3.5],
                Mob::fresh(),
            ),
        );
        idle(&mut region, 30);
        [ONE, TWO, THREE].map(|key| record(&region, key))
    };
    assert_eq!(run(), run());
    let _ = Random::of_entity(0, ONE, 0);
}
