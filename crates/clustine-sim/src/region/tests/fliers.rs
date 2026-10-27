//! The sniffer, the allay and the happy ghast in a region, from the brief's sections 7.9
//! to 7.11 and `docs/groundwork/n7j-plan.md` (sections 1 and 4.3), as a player meets
//! them: an egg makes each with no draw of its own; a sniffer egg cracks twice and
//! hatches, faster on moss; sniffers bred lay an egg; a dried ghast under water becomes a
//! ghastling by its stages; a ghastling fed grows; a harness goes on and comes off; an
//! allay takes an item, gathers what it holds and gives it back; a happy ghast holds
//! leashes and shows the quad one, stays still under a player, and floats about. With
//! their brains: a sniffer sniffs, digs and drops its seed 120 ticks in; an allay fetches
//! to its player and to a note block; a ghastling follows and is tempted, and grown up
//! forgets its brain. Across a pinned line: a sniffer tempted by a player beyond it, a
//! sniffer breeding by the region's protocol, a dig and a ghastling's panic arriving
//! whole.

use clustine_blocks::data::{number, set};
use clustine_data::{BlockState, blocks, entity_types, items, sounds};
use clustine_world::{Double, Entity, EntityChange, EntityData, EntityId, Leash, LeashHolder};

use super::*;
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::body::attributes::supplied;
use crate::mobs::brain::{self, Memories, MemoryKind, MemoryWrite, Status, Tracker, Value};
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
    ItemStack::new(item, 1)
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
    assert_eq!(allay::liked_player(&given), Some(uuid));
    // Diamonds and dirt beside it: it takes the diamonds, shown as taken, and leaves the
    // dirt.
    let at = record(&region, ONE).unwrap().position;
    lay(
        &mut region,
        &item_at(TWO, ItemStack::new(items::DIAMOND, 3), at),
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
        Some(ItemStack::new(items::DIAMOND, 3))
    );
    // An empty hand takes its item back, and what it gathered falls at its eyes: all
    // three, those it has meanwhile thrown to the player beside it (within 3 of their
    // eyes, one a tick) and those it held.
    holding(&mut region, None);
    let output = use_on(&mut region, ONE);
    assert!(sounds_of(&output).contains(&(sounds::ENTITY_ALLAY_ITEM_TAKEN, Some(player(1)))));
    let taken = mob(&region, ONE);
    assert_eq!(taken.body.equipment.slots[0], None);
    assert_eq!(allay::liked_player(&taken), None);
    assert_eq!(allay::read(&taken).inventory, None);
    let lying: i32 = items_lying(&region)
        .iter()
        .filter(|stack| stack.item == items::DIAMOND)
        .map(|stack| stack.count)
        .sum();
    assert_eq!(lying, 3);
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
    // A ghastling's is 32, and its brain flies it about (it has no goals).
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
    idle(&mut region, 1);
    let after = record(&region, ONE).unwrap();
    assert_eq!(Mob::decode(&after.data).unwrap().body.home.radius, 32);
    until(&mut region, 400, |region| {
        record(region, ONE).unwrap().position != start
    });
}

/// Ticks `region` until `done` holds of it, at most `most` times; how many it took.
fn until(region: &mut Region, most: usize, mut done: impl FnMut(&Region) -> bool) -> usize {
    for n in 1..=most {
        checked(region, &TickInputs::default());
        if done(region) {
            return n;
        }
    }
    panic!("not within {most} ticks");
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
        lay(
            &mut region,
            &mob_at(
                FOUR,
                entity_types::HAPPY_GHAST,
                [12.5, -57.0, 12.5],
                ghast_with(-24_000, &HappyGhastState::default()),
            ),
        );
        // Their brains at work: a player near with seeds and snowballs, an item to want.
        holding(&mut region, Some(one(items::SNOWBALL)));
        idle(&mut region, 60);
        [ONE, TWO, THREE, FOUR].map(|key| record(&region, key))
    };
    let first = run();
    assert_eq!(first, run());
    // A brain at work wrote its mind.
    for record in first.iter().skip(1) {
        let made = Mob::decode(&record.as_ref().expect("still here").data).unwrap();
        assert!(made.mind.brain.len() > 1, "{:?}", record);
    }
    let _ = Random::of_entity(0, ONE, 0);
}

const FOUR: EntityId = EntityId(46);

// ------------------------------------------------------------------------------------
// With their brains.

/// Grass under the whole of the origin chunk, where the sniffer may dig.
fn grass() -> Vec<(BlockPos, BlockState)> {
    (0..16)
        .flat_map(|x| (0..16).map(move |z| (BlockPos::new(x, -61, z), blocks::GRASS_BLOCK)))
        .collect()
}

fn state_of(region: &Region, key: EntityId) -> sniffer::State {
    sniffer::read(&mob(region, key)).state
}

/// The seeds a dig gave that lie in the region.
fn seeds_lying(region: &Region) -> usize {
    items_lying(region)
        .iter()
        .filter(|stack| matches!(stack.item, items::TORCHFLOWER_SEEDS | items::PITCHER_POD))
        .count()
}

/// The memories of the region's mob `key` (of a type with a brain), as of its last tick.
fn memories_of(region: &Region, key: EntityId) -> Memories {
    let entity = record(region, key).expect("the mob is here");
    let provider = crate::mobs::types::of(entity.kind)
        .brain()
        .expect("a brain");
    brain::memories_of(provider, &mob(region, key), entity.lived)
}

/// `mob` with `writes` made to its brain's memories, as of its making.
fn remembering(kind: i32, mut mob: Mob, writes: Vec<MemoryWrite>) -> Mob {
    let provider = crate::mobs::types::of(kind as u16)
        .brain()
        .expect("a brain");
    let mut memories = brain::memories_of(provider, &mob, 1);
    for write in writes {
        write.apply(&mut memories);
    }
    brain::write_memories(provider, &mut mob, memories);
    mob
}

#[test]
fn a_sniffer_on_grass_sniffs_searches_digs_and_drops_its_seed_120_ticks_into_its_dig() {
    let mut region = on_ticking_floor_with(&grass());
    lay(
        &mut region,
        &mob_at(ONE, entity_types::SNIFFER, [8.5, -60.0, 8.5], Mob::fresh()),
    );
    // It sniffs, walks to a place it chose, searching, and digs where its nose then is.
    let mut searched = false;
    until(&mut region, 60_000, |region| {
        let state = state_of(region, ONE);
        searched |= state == sniffer::State::Searching;
        searched && state == sniffer::State::Digging
    });
    // Its seed, 120 ticks into the dig.
    let n = until(&mut region, 200, |region| seeds_lying(region) > 0);
    assert_eq!(n, 120);
    assert_eq!(state_of(&region, ONE), sniffer::State::Digging);
    // It rises, is happy, and is idle again, not to sniff for 9600 ticks.
    until(&mut region, 100, |region| {
        state_of(region, ONE) == sniffer::State::Rising
    });
    until(&mut region, 100, |region| {
        state_of(region, ONE) == sniffer::State::FeelingHappy
    });
    until(&mut region, 200, |region| {
        state_of(region, ONE) == sniffer::State::Idling
    });
    let memories = memories_of(&region, ONE);
    assert!(memories.check(MemoryKind::SniffCooldown, Status::Present));
    assert!(matches!(
        memories.get(MemoryKind::SnifferExploredPositions),
        Some(Value::Blocks(list)) if list.len() == 1
    ));
}

/// The diamonds lying within `reach` of `at`, counted.
fn diamonds_near(region: &Region, at: [f64; 3], reach: f64) -> i32 {
    all_of(region, entity_types::ITEM)
        .into_iter()
        .filter(|entity| crate::physics::distance_sqr(entity.position, at) < reach * reach)
        .filter_map(|entity| match entity.data {
            EntityData::Item { stack, .. } if stack.item == items::DIAMOND => Some(stack.count),
            _ => None,
        })
        .sum()
}

fn diamonds_held(region: &Region) -> i32 {
    region.players[&player(1)]
        .inventory
        .0
        .iter()
        .flatten()
        .filter(|stack| stack.item == items::DIAMOND)
        .map(|stack| stack.count)
        .sum()
}

#[test]
fn an_allay_given_a_diamond_fetches_diamonds_from_16_away_and_throws_them_to_its_player() {
    let mut region = on_ticking_floor_with(&[]);
    let mut made = Mob::fresh();
    made.body.flags.persistent = true;
    lay(
        &mut region,
        &mob_at(ONE, entity_types::ALLAY, [2.5, -60.0, 0.5], made),
    );
    holding(&mut region, Some(one(items::DIAMOND)));
    use_on(&mut region, ONE);
    let held = diamonds_held(&region);
    lay(
        &mut region,
        &item_at(TWO, ItemStack::new(items::DIAMOND, 3), [12.5, -60.0, 12.5]),
    );
    // It flies to them, takes them all, and brings them back.
    let three = Some(ItemStack::new(items::DIAMOND, 3));
    until(&mut region, 800, |region| {
        allay::read(&mob(region, ONE)).inventory == three
    });
    until(&mut region, 800, |region| {
        allay::read(&mob(region, ONE)).inventory.is_none()
    });
    // All three by the player (the spawn), lying or picked up.
    let by_the_player =
        diamonds_near(&region, [0.5, -60.0, 0.5], 4.0) + diamonds_held(&region) - held;
    assert_eq!(by_the_player, 3);
    // For 60 ticks after its last throw it picks nothing up.
    let memories = memories_of(&region, ONE);
    assert!(memories.check(MemoryKind::ItemPickupCooldownTicks, Status::Present));
}

#[test]
fn an_allay_takes_what_it_gathered_to_a_note_block_it_heard_rather_than_to_its_player() {
    let note = BlockPos::new(12, -60, 12);
    let mut region = on_ticking_floor_with(&[(note, blocks::NOTE_BLOCK)]);
    let mut made = Mob::fresh();
    made.body.flags.persistent = true;
    made.body.equipment.slots[0] = Some(one(items::DIAMOND));
    allay::update(&mut made, |own| {
        own.inventory = Some(ItemStack::new(items::DIAMOND, 2));
    });
    let made = remembering(
        entity_types::ALLAY,
        made,
        vec![
            MemoryWrite::Set(MemoryKind::LikedPlayer, Value::Uuid(player(1).0.as_u128())),
            MemoryWrite::Set(MemoryKind::LikedNoteblock, Value::Block(note)),
            MemoryWrite::Set(
                MemoryKind::LikedNoteblockCooldownTicks,
                Value::Int(allay::LIKED_NOTEBLOCK_TICKS),
            ),
        ],
    );
    lay(
        &mut region,
        &mob_at(ONE, entity_types::ALLAY, [6.5, -59.0, 6.5], made),
    );
    until(&mut region, 600, |region| {
        allay::read(&mob(region, ONE)).inventory.is_none()
    });
    let above = [12.5, -58.5, 12.5];
    assert_eq!(diamonds_near(&region, above, 4.0), 2);
    assert_eq!(diamonds_near(&region, [0.5, -60.0, 0.5], 4.0), 0);
}

#[test]
fn a_ghastling_flies_to_a_player_holding_a_snowball() {
    let mut region = on_ticking_floor_with(&[]);
    lay(
        &mut region,
        &mob_at(
            ONE,
            entity_types::HAPPY_GHAST,
            [9.5, -58.0, 9.5],
            ghast_with(-24_000, &HappyGhastState::default()),
        ),
    );
    holding(&mut region, Some(one(items::SNOWBALL)));
    let spawn = [0.5, -60.0, 0.5];
    until(&mut region, 800, |region| {
        crate::physics::distance_sqr(record(region, ONE).unwrap().position, spawn) < 5.0 * 5.0
    });
    // Tempted, it looks at them (its tempting player is sensed, not in its record).
    let key = region.players[&player(1)].entity_id;
    assert!(matches!(
        memories_of(&region, ONE).get(MemoryKind::LookTarget),
        Some(Value::Tracker(Tracker::Entity { key: looked, .. })) if looked == key
    ));
}

#[test]
fn a_ghastling_grown_up_forgets_its_brain_and_floats_by_its_goals() {
    let mut region = on_ticking_floor_with(&[]);
    let start = [8.5, -55.0, 8.5];
    lay(
        &mut region,
        &mob_at(
            ONE,
            entity_types::HAPPY_GHAST,
            start,
            ghast_with(-60, &HappyGhastState::default()),
        ),
    );
    until(&mut region, 200, |region| {
        !animal::is_baby(&mob(region, ONE))
    });
    let grown = mob(&region, ONE);
    assert!(
        grown
            .mind
            .brain
            .iter()
            .all(|entry| entry.id == brain::ids::STATE)
    );
    // Its goals float it about its home of 64, bare and grown; its brain never ticks.
    idle(&mut region, 40);
    let made = mob(&region, ONE);
    assert_eq!(made.body.home.radius, 64);
    assert_eq!(made.mind.brain, grown.mind.brain);
}

// ------------------------------------------------------------------------------------
// Across a pinned line.

/// A region of the land west of chunk x = 2, its chunks x of -1 to 1 loaded with a stone
/// floor, and the chunks of x = 2 another's, copied, with `beyond` in the one at z = 0.
fn at_a_line(own: &[Entity], beyond: &[Entity]) -> Region {
    let west = ChunkArea {
        min_x: None,
        max_x: Some(2),
    };
    let mut region = joined_in(west, &[1]);
    let around: Vec<ChunkPos> = (-1..=1)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect();
    region.tick(&tickets(around.clone(), vec![]));
    let chunks = around.iter().map(|position| (*position, floor())).collect();
    region.tick(&TickInputs {
        chunks_loaded: chunks,
        ..TickInputs::default()
    });
    for entity in own {
        lay(&mut region, entity);
    }
    let copies: Vec<ChunkPos> = (-1..=1).map(|z| ChunkPos::new(2, z)).collect();
    let bases = copies
        .iter()
        .map(|position| {
            let mut base = floor();
            if position.z == 0 {
                base.set_entities(beyond.to_vec());
            }
            (*position, base)
        })
        .collect();
    region.tick(&TickInputs {
        foreign: copies
            .iter()
            .map(|position| (*position, RegionId(1)))
            .collect(),
        copy_bases: bases,
        ..TickInputs::default()
    });
    region
}

/// The face of a player of another region, standing at `x` and holding `held`.
fn face_at(x: f64, held: i32, seq: u64) -> crate::api::FaceChange {
    let mut hotbar = [const { None }; clustine_world::HOTBAR_SLOTS];
    hotbar[0] = Some(one(held));
    crate::api::FaceChange::Face {
        player: player(9),
        entity: EntityId(500),
        hops: 0,
        seq: crate::api::FaceSeq(seq, 0),
        face: crate::api::PlayerFace::of(
            &Pose::at(clustine_world::Vec3::new(x, -60.0, 0.5)),
            false,
            false,
            &clustine_world::Inventory::with_hotbar(hotbar),
            0,
            None,
        ),
    }
}

#[test]
fn a_sniffer_follows_torchflower_seeds_a_player_of_the_other_region_holds_beyond_the_line() {
    let mut region = at_a_line(
        &[mob_at(
            ONE,
            entity_types::SNIFFER,
            [26.5, -60.0, 0.5],
            Mob::fresh(),
        )],
        &[],
    );
    checked(
        &mut region,
        &TickInputs {
            faces: vec![face_at(34.5, items::TORCHFLOWER_SEEDS, 1)],
            ..TickInputs::default()
        },
    );
    until(&mut region, 60, |region| {
        matches!(
            memories_of(region, ONE).get(MemoryKind::WalkTarget),
            Some(Value::Walk {
                to: Tracker::Entity {
                    key: EntityId(500),
                    ..
                },
                speed: 1.25,
                ..
            })
        )
    });
    // It walks to within 3.5 of them, on its side of the line.
    until(&mut region, 400, |region| {
        record(region, ONE).unwrap().position[0] > 29.5
    });
    assert!(record(&region, ONE).unwrap().position[0] < 32.0);
}

#[test]
fn a_ghastling_is_tempted_by_a_snowball_a_player_of_the_other_region_holds_beyond_the_line() {
    let mut region = at_a_line(
        &[mob_at(
            ONE,
            entity_types::HAPPY_GHAST,
            [24.5, -58.0, 0.5],
            ghast_with(-24_000, &HappyGhastState::default()),
        )],
        &[],
    );
    checked(
        &mut region,
        &TickInputs {
            faces: vec![face_at(36.5, items::SNOWBALL, 1)],
            ..TickInputs::default()
        },
    );
    until(&mut region, 60, |region| {
        matches!(
            memories_of(region, ONE).get(MemoryKind::WalkTarget),
            Some(Value::Walk {
                to: Tracker::Entity {
                    key: EntityId(500),
                    ..
                },
                speed: 1.25,
                ..
            })
        )
    });
    until(&mut region, 600, |region| {
        record(region, ONE).unwrap().position[0] > 28.0
    });
}

#[test]
fn a_sniffer_in_love_asks_a_partner_beyond_the_line_by_the_region_s_protocol() {
    let partner = mob_at(
        EntityId(5),
        entity_types::SNIFFER,
        [32.5, -60.0, 0.5],
        sniffer_in_love(),
    );
    let mut region = at_a_line(
        &[mob_at(
            ONE,
            entity_types::SNIFFER,
            [30.5, -60.0, 0.5],
            sniffer_in_love(),
        )],
        &[partner],
    );
    let mut asked = None;
    for _ in 0..200 {
        let output = checked(&mut region, &TickInputs::default());
        asked = output.effects.iter().find_map(|effect| match effect.what {
            clustine_world::Effected::OnEntity {
                key,
                by: clustine_world::EntityUse::BreedAsk { asker, number },
            } => Some((key, asker, number)),
            _ => None,
        });
        if asked.is_some() {
            break;
        }
    }
    assert_eq!(asked, Some((EntityId(5), ONE, 1)));
    // No egg here: the partner's region lays it, as the game's `this`.
    assert!(items_lying(&region).is_empty());
}

/// `entity` taken in by `region` as an arrival (as a merge, a split or a restore takes a
/// record in: whole, its sensed memories not in it).
fn take_in(region: &mut Region, entity: Entity) {
    checked(
        region,
        &TickInputs {
            arrivals: vec![clustine_world::Arrival {
                id: clustine_world::EffectId {
                    sequence: 7,
                    index: 0,
                },
                chunk: entity.chunk(),
                entity,
                due: clustine_world::Due::FromTaking,
            }],
            ..TickInputs::default()
        },
    );
}

#[test]
fn a_digging_sniffer_arriving_in_another_region_digs_on_and_drops_its_seed_at_its_tick() {
    let mut region = on_ticking_floor_with(&grass());
    let digging = remembering(
        entity_types::SNIFFER,
        Mob::fresh(),
        vec![MemoryWrite::Set(
            MemoryKind::SnifferDigging,
            Value::Bool(true),
        )],
    );
    lay(
        &mut region,
        &mob_at(ONE, entity_types::SNIFFER, [8.5, -60.0, 8.5], digging),
    );
    until(&mut region, 20, |region| {
        state_of(region, ONE) == sniffer::State::Digging
    });
    idle(&mut region, 30);
    let mut other = on_ticking_floor_with(&grass());
    take_in(&mut other, record(&region, ONE).unwrap());
    // It digs on, and its seed comes at its tick, 120 into the dig: the arrival's tick is
    // the 32nd of the dig.
    let n = until(&mut other, 200, |region| {
        assert_eq!(state_of(region, ONE), sniffer::State::Digging);
        seeds_lying(region) > 0
    });
    assert_eq!(n, 120 - 31);
    until(&mut other, 100, |region| {
        state_of(region, ONE) == sniffer::State::Rising
    });
}

#[test]
fn a_panicking_ghastling_arriving_in_another_region_panics_on_to_its_end() {
    let mut region = on_ticking_floor_with(&[]);
    let panicking = remembering(
        entity_types::HAPPY_GHAST,
        ghast_with(-24_000, &HappyGhastState::default()),
        vec![MemoryWrite::Set(MemoryKind::IsPanicking, Value::Bool(true))],
    );
    lay(
        &mut region,
        &mob_at(ONE, entity_types::HAPPY_GHAST, [8.5, -57.0, 8.5], panicking),
    );
    idle(&mut region, 10);
    assert!(memories_of(&region, ONE).check(MemoryKind::IsPanicking, Status::Present));
    let mut other = on_ticking_floor_with(&[]);
    take_in(&mut other, record(&region, ONE).unwrap());
    assert!(memories_of(&other, ONE).check(MemoryKind::IsPanicking, Status::Present));
    // It panics on, and its panic ends 101 to 121 ticks after it began, in its first
    // tick: 11 ticks behind it as it arrived.
    let n = until(&mut other, 200, |region| {
        memories_of(region, ONE).check(MemoryKind::IsPanicking, Status::Absent)
    });
    assert!((101 - 11..=121 - 11).contains(&n), "{n}");
}
