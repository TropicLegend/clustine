//! Mobs steered by the mobs that ride them (`docs/groundwork/n9-plan.md`, section 6):
//! the rider's goals drive its mount's navigation and move control, the mount's own
//! moving goals held.

use clustine_data::entity_types;
use clustine_world::{Entity, EntityId, Riding, Seat};

use super::thrown::lay;
use super::*;
use crate::living::MAX_AIR;
use crate::mobs::Mob;
use crate::region::tnt::tests::{nothing, world};

const SEED: u64 = 0x6a0c_4e75;

/// The keys of the three, apart from any the region gives.
const CHICKEN: u64 = 9_001;
const ZOMBIE: u64 = 9_002;
const VILLAGER: u64 = 9_003;

/// A mob of the type `kind` at `feet`, standing, at its type's full health.
fn mob_of(key: u64, kind: i32, feet: [f64; 3], prepare: impl FnOnce(&mut Mob)) -> Entity {
    let kind = u16::try_from(kind).expect("ids fit");
    let mut mob = Mob::fresh();
    mob.body.vitals.health = crate::mobs::body::attributes::supplied(
        kind,
        crate::mobs::body::attribute(clustine_data::attributes::MAX_HEALTH),
    )
    .expect("a health") as f32;
    mob.body.vitals.air = MAX_AIR;
    prepare(&mut mob);
    Entity {
        key: EntityId(key),
        kind,
        uuid: uuid::Uuid::from_u128(u128::from(key) + 1),
        position: feet,
        motion: [0.0, -0.078_400_001_525_878_9, 0.0],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Riding::default(),
        data: mob.encode(),
    }
}

/// A zombie riding a chicken at `feet`, and a villager that does not move at `prey`.
fn chicken_jockey(feet: [f64; 3], prey: [f64; 3]) -> Region {
    let mut region = world(SEED, nothing);
    let mut chicken = mob_of(CHICKEN, entity_types::CHICKEN, feet, |_| {});
    let mut zombie = mob_of(ZOMBIE, entity_types::ZOMBIE, feet, |_| {});
    chicken.riding.passengers = vec![Seat {
        key: zombie.key,
        player: false,
        pending: None,
    }];
    zombie.riding.vehicle = Some(chicken.key);
    let villager = mob_of(VILLAGER, entity_types::VILLAGER, prey, |mob| {
        mob.body.flags.no_ai = true;
    });
    lay(&mut region, &chicken);
    lay(&mut region, &zombie);
    lay(&mut region, &villager);
    region
}

fn mob_at(region: &Region, key: u64) -> (Entity, Mob) {
    let (_, record) = region.find_entity(EntityId(key)).expect("here");
    (record.clone(), Mob::decode(&record.data).expect("a mob"))
}

#[test]
fn a_chicken_jockey_s_chicken_goes_where_its_zombie_s_target_is() {
    let prey = [12.5, -60.0, 4.5];
    let mut region = chicken_jockey([4.5, -60.0, 4.5], prey);
    let mut aimed = false;
    for _ in 0..200 {
        region.tick(&TickInputs::default());
        let (_, chicken) = mob_at(&region, CHICKEN);
        // The chicken's navigation carries the path its rider asked for, to the
        // villager's block.
        if chicken
            .body
            .progress
            .target
            .is_some_and(|target| (target.x - 12).abs() <= 1 && (target.z - 4).abs() <= 1)
        {
            aimed = true;
            break;
        }
    }
    assert!(
        aimed,
        "the chicken never took a path to the zombie's target"
    );
    let start = mob_at(&region, CHICKEN).0.position[0];
    for _ in 0..40 {
        region.tick(&TickInputs::default());
    }
    let (chicken, _) = mob_at(&region, CHICKEN);
    assert!(
        chicken.position[0] > start + 1.0,
        "the chicken walked towards the villager: {start} to {:?}",
        chicken.position
    );
}

#[test]
fn a_mount_steered_by_its_rider_takes_no_stroll_of_its_own_and_its_rider_has_no_path() {
    // No target: the zombie strolls, by the chicken's navigation; the zombie's own
    // navigation holds nothing.
    let mut region = chicken_jockey([4.5, -60.0, 4.5], [40.5, -60.0, 40.5]);
    let mut chicken_paths = 0;
    for _ in 0..400 {
        region.tick(&TickInputs::default());
        let (_, zombie) = mob_at(&region, ZOMBIE);
        assert!(
            zombie.body.path.followed.is_none(),
            "a rider's goals give their paths to its mount"
        );
        if mob_at(&region, CHICKEN).1.body.path.followed.is_some() {
            chicken_paths += 1;
        }
    }
    assert!(chicken_paths > 0, "the zombie's stroll drove the chicken");
}
