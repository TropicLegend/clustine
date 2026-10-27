//! The villager's day and night in a region (N10's part V1; the game's rules in
//! `docs/groundwork/n10-brief.md`, cited as V, sections 1.2 to 1.5 and 3.4): its
//! schedule reaching work with a job site, the meeting with a meeting point, and a baby's
//! play; a villager whose home is a bed lying down in it at dusk, the bed taken, and
//! getting up at dawn, or when its bed is broken, or when a player uses the bed; one
//! with no home walking to the nearest bed; and babies chasing each other.

use clustine_data::{BlockState, entity_types};
use clustine_world::{Entity, EntityChange, EntityId};

use super::*;
use crate::living::MAX_AIR;
use crate::mobs::Mob;
use crate::mobs::brain::{self, Activity, MemoryKind, Tracker, Value};
use crate::mobs::types::animal::{self, Animal};
use crate::mobs::types::villager::{self, PROVIDER, VillagerState};

const ONE: EntityId = EntityId(40);
const TWO: EntityId = EntityId(42);

/// A red bed whose head is at (2, -60, 0) and foot at (2, -60, 1), facing north.
const HEAD: BlockPos = BlockPos::new(2, -60, 0);
const FOOT: BlockPos = BlockPos::new(2, -60, 1);

fn bed(part: &str) -> BlockState {
    BlockState::parse(&format!("minecraft:red_bed[facing=north,part={part}]")).unwrap()
}

/// A villager at `position`, of the ageable part `shared`, remembering `memories`.
fn villager_at(
    key: EntityId,
    position: [f64; 3],
    shared: &Animal,
    memories: &[(MemoryKind, Value)],
) -> Entity {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 20.0;
    mob.body.vitals.air = MAX_AIR;
    mob.body.flags.picks_up_loot = true;
    mob.state = animal::write(shared, &VillagerState::default());
    let mut remembered = brain::memories_of(&PROVIDER, &mob, 0);
    for (kind, value) in memories {
        remembered.set(*kind, value.clone());
    }
    brain::write_memories(&PROVIDER, &mut mob, remembered);
    Entity {
        key,
        kind: entity_types::VILLAGER as u16,
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

fn baby() -> Animal {
    let mut shared = Animal::default();
    shared.set_age(animal::BABY_START_AGE, 0);
    shared
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

fn record(region: &Region, key: EntityId) -> Entity {
    region
        .find_entity(key)
        .map(|(_, entity)| entity.clone())
        .expect("the villager is here")
}

fn mob(region: &Region, key: EntityId) -> Mob {
    Mob::decode(&record(region, key).data).unwrap()
}

fn own(region: &Region, key: EntityId) -> VillagerState {
    villager::state_of(&mob(region, key)).1
}

/// Its memory of `kind` as its last tick left it.
fn memory(region: &Region, key: EntityId, kind: MemoryKind) -> Option<Value> {
    let entity = record(region, key);
    let mob = Mob::decode(&entity.data).unwrap();
    brain::memories_of(&PROVIDER, &mob, entity.lived).get(kind)
}

/// The activity beside the core its brain has active.
fn active(region: &Region, key: EntityId) -> Option<Activity> {
    let entity = record(region, key);
    let mob = Mob::decode(&entity.data).unwrap();
    brain::Brain::read(
        &PROVIDER,
        &mob.mind.brain,
        Some(&Default::default()),
        entity.lived,
    )
    .state
    .active
}

fn idle(region: &mut Region) -> TickOutput {
    checked(region, &TickInputs::default())
}

/// Ticks with no inputs until `done` holds, at most `bound`.
fn until(region: &mut Region, bound: usize, mut done: impl FnMut(&Region) -> bool) -> bool {
    for _ in 0..bound {
        idle(region);
        if done(region) {
            return true;
        }
    }
    false
}

fn occupied(region: &Region) -> bool {
    clustine_blocks::data::flag(region.block(HEAD).unwrap(), "occupied")
}

#[test]
fn the_schedule_reaches_work_with_a_job_site_and_the_meeting_with_a_meeting_point() {
    let lectern = Value::Block(BlockPos::new(9, -60, 9));
    let bell = Value::Block(BlockPos::new(10, -60, 10));
    for (day, memories, expected) in [
        // Work time: with a job site, work; without one, idle (the activity's condition).
        (
            2500,
            vec![(MemoryKind::JobSite, lectern.clone())],
            Activity::Work,
        ),
        (2500, vec![], Activity::Idle),
        // Meeting time: with a meeting point, the meeting; without one, idle.
        (
            9500,
            vec![(MemoryKind::MeetingPoint, bell.clone())],
            Activity::Meet,
        ),
        (
            9500,
            vec![(MemoryKind::JobSite, lectern.clone())],
            Activity::Idle,
        ),
        // Dusk to dawn: rest, whatever it remembers.
        (
            12_500,
            vec![(MemoryKind::JobSite, lectern.clone())],
            Activity::Rest,
        ),
    ] {
        let mut region = on_ticking_floor_with(&[]);
        region.clock.day = day;
        lay(
            &mut region,
            &villager_at(ONE, [7.5, -60.0, 7.5], &Animal::default(), &memories),
        );
        idle(&mut region);
        assert_eq!(active(&region, ONE), Some(expected), "{day} {memories:?}");
    }
    // At 2000 the schedule's next update (more than 20 ticks after the last) switches
    // an idle villager with a job site to work.
    let mut region = on_ticking_floor_with(&[]);
    region.clock.day = 1990;
    lay(
        &mut region,
        &villager_at(
            ONE,
            [7.5, -60.0, 7.5],
            &Animal::default(),
            &[(MemoryKind::JobSite, lectern)],
        ),
    );
    idle(&mut region);
    assert_eq!(active(&region, ONE), Some(Activity::Idle));
    assert!(until(&mut region, 40, |region| {
        active(region, ONE) == Some(Activity::Work)
    }));
}

#[test]
fn a_baby_plays_from_3000_idles_at_6000_and_rests_at_dusk() {
    for (day, expected) in [
        (3500, Activity::Play),
        (6500, Activity::Idle),
        (10_500, Activity::Play),
        (12_500, Activity::Rest),
    ] {
        let mut region = on_ticking_floor_with(&[]);
        region.clock.day = day;
        lay(
            &mut region,
            &villager_at(ONE, [7.5, -60.0, 7.5], &baby(), &[]),
        );
        idle(&mut region);
        assert_eq!(active(&region, ONE), Some(expected), "{day}");
    }
}

/// A region with the bed and a villager whose home it is standing beside its head, at
/// `day`.
fn bedroom(day: u64) -> Region {
    let mut region = on_ticking_floor_with(&[(HEAD, bed("head")), (FOOT, bed("foot"))]);
    region.clock.day = day;
    lay(
        &mut region,
        &villager_at(
            ONE,
            [1.5, -60.0, 0.5],
            &Animal::default(),
            &[(MemoryKind::Home, Value::Block(HEAD))],
        ),
    );
    region
}

#[test]
fn at_dusk_a_villager_beside_its_bed_lies_down_in_it_and_the_bed_is_taken() {
    let mut region = bedroom(12_500);
    assert!(until(&mut region, 5, |region| own(region, ONE)
        .sleeping
        .is_some()));
    assert_eq!(own(&region, ONE).sleeping, Some(HEAD));
    assert!(occupied(&region), "its block write takes the bed");
    // It lies at the bed's middle, the bed's height and an eighth up (it settles on the
    // bed's top as it falls), and remembers when it slept.
    let at = record(&region, ONE).position;
    assert_eq!((at[0], at[2]), (2.5, 0.5));
    assert!(at[1] > -60.0 + 0.4 && at[1] <= -60.0 + 0.6875, "{at:?}");
    assert!(matches!(
        memory(&region, ONE, MemoryKind::LastSlept),
        Some(Value::Long(_))
    ));
    // Its neighbours read that it sleeps and when it slept.
    let shown = own(&region, ONE).shown;
    assert!(shown.sleeping);
    assert!(shown.last_slept.is_some());
    // It keeps lying through the night.
    for _ in 0..100 {
        idle(&mut region);
    }
    assert_eq!(own(&region, ONE).sleeping, Some(HEAD));
    assert!(occupied(&region));
}

#[test]
fn at_dawn_it_gets_up_beside_the_bed_which_is_free_again() {
    let mut region = bedroom(23_900);
    assert!(until(&mut region, 5, |region| own(region, ONE)
        .sleeping
        .is_some()));
    // Its schedule's idle at 10 of the day (24_010) ends its rest: it gets up.
    assert!(until(&mut region, 200, |region| own(region, ONE)
        .sleeping
        .is_none()));
    assert_ne!(active(&region, ONE), Some(Activity::Rest));
    assert!(!occupied(&region));
    let at = record(&region, ONE).position;
    let head_middle = [2.5, -60.0, 0.5];
    assert!(
        crate::physics::distance_sqr(at, head_middle) < 4.0,
        "beside the bed: {at:?}"
    );
    assert!(at[1] >= -60.0 && at[1] < -59.0, "on the floor: {at:?}");
    assert!(matches!(
        memory(&region, ONE, MemoryKind::LastWoken),
        Some(Value::Long(_))
    ));
    assert!(!own(&region, ONE).shown.sleeping);
}

#[test]
fn a_villager_whose_bed_is_broken_gets_up() {
    let mut region = bedroom(12_500);
    assert!(until(&mut region, 5, |region| own(region, ONE)
        .sleeping
        .is_some()));
    let mut output = TickOutput::default();
    region.set_block(HEAD, clustine_data::blocks::AIR, &mut output);
    region.set_block(FOOT, clustine_data::blocks::AIR, &mut output);
    assert!(until(&mut region, 2, |region| own(region, ONE)
        .sleeping
        .is_none()));
}

#[test]
fn a_player_using_the_bed_a_villager_sleeps_in_wakes_it_and_does_not_lie_down() {
    let mut region = bedroom(13_000);
    let user = region.players.get_mut(&player(1)).unwrap();
    user.pose.position = Vec3::new(4.5, -60.0, 0.5);
    assert!(until(&mut region, 5, |region| own(region, ONE)
        .sleeping
        .is_some()));
    let output = region.tick(&moves(vec![place(1, HEAD.x, HEAD.y, HEAD.z, Face::Top)]));
    // Not "occupied": the villager was there to wake.
    let said: Vec<&str> = output
        .player_events
        .iter()
        .filter_map(|(_, event)| match event {
            PlayerEvent::Message { key, .. } => Some(key.as_str()),
            _ => None,
        })
        .collect();
    assert!(!said.contains(&"block.minecraft.bed.occupied"), "{said:?}");
    assert!(region.players[&player(1)].sleeping.is_none());
    // It gets up at its tick (the players' inputs come first, so in this one or the
    // next), the bed free; and it rests on, and may lie down again after 100 ticks.
    assert!(
        until(&mut region, 1, |region| own(region, ONE).sleeping.is_none())
            || own(&region, ONE).sleeping.is_none()
    );
    assert!(!occupied(&region));
    assert!(matches!(
        memory(&region, ONE, MemoryKind::LastWoken),
        Some(Value::Long(_))
    ));
    // A use of a bed nobody sleeps in, taken by nobody, is no kick and says nothing of
    // it.
    let output = region.tick(&moves(vec![place(1, HEAD.x, HEAD.y, HEAD.z, Face::Top)]));
    let said: Vec<String> = output
        .player_events
        .iter()
        .filter_map(|(_, event)| match event {
            PlayerEvent::Message { key, .. } => Some(key.clone()),
            _ => None,
        })
        .collect();
    assert!(!said.iter().any(|key| key == "block.minecraft.bed.occupied"));
}

/// A pen of stone three blocks high round x, z = 2 to 13 in the chunk whose entities
/// the region ticks, with `inside` laid in it.
fn pen(inside: &[(BlockPos, BlockState)]) -> Vec<(BlockPos, BlockState)> {
    let mut placed = inside.to_vec();
    for x in 1..=14 {
        for z in 1..=14 {
            if x == 1 || x == 14 || z == 1 || z == 14 {
                for y in -60..=-58 {
                    placed.push((BlockPos::new(x, y, z), clustine_data::blocks::STONE));
                }
            }
        }
    }
    placed
}

#[test]
fn at_dusk_a_villager_with_no_home_walks_to_the_nearest_bed() {
    // `SetClosestHomeAsWalkTarget` (in the rest's `RunOne` of the homeless): the bed some
    // blocks off, reached by a path, close enough at 1.
    let head = BlockPos::new(4, -60, 4);
    let foot = BlockPos::new(4, -60, 5);
    let mut region = on_ticking_floor_with(&pen(&[(head, bed("head")), (foot, bed("foot"))]));
    region.clock.day = 12_500;
    lay(
        &mut region,
        &villager_at(ONE, [10.5, -60.0, 10.5], &Animal::default(), &[]),
    );
    // Its `RunOne` puts the search first one time in five (its weight among the three
    // that start in the open), and waits out each walk and pause in between.
    let walked = until(&mut region, 3000, |region| {
        matches!(
            memory(region, ONE, MemoryKind::WalkTarget),
            Some(Value::Walk {
                to: Tracker::Block(to),
                close_enough: 1,
                ..
            }) if to == head
        )
    });
    assert!(walked, "it sets out for the bed");
    assert_eq!(active(&region, ONE), Some(Activity::Rest));
}

#[test]
fn babies_play_tag_one_chasing_the_other() {
    // Two babies at play time within sight of each other: soon one chases the other
    // (`PlayTagWithOtherKids`: its interaction target, as its neighbours read it).
    let mut region = on_ticking_floor_with(&pen(&[]));
    region.clock.day = 3500;
    lay(
        &mut region,
        &villager_at(ONE, [5.5, -60.0, 7.5], &baby(), &[]),
    );
    lay(
        &mut region,
        &villager_at(TWO, [9.5, -60.0, 7.5], &baby(), &[]),
    );
    let chased = until(&mut region, 600, |region| {
        own(region, ONE).shown.interaction_target == Some(TWO)
            || own(region, TWO).shown.interaction_target == Some(ONE)
    });
    assert!(chased);
    assert_eq!(active(&region, ONE), Some(Activity::Play));
    assert!(matches!(
        memory(&region, ONE, MemoryKind::VisibleVillagerBabies),
        Some(Value::Entities(list)) if list == [TWO]
    ));
}
