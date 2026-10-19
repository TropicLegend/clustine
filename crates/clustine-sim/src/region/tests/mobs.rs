//! A mob's tick in a region on the skeleton of `crate::mobs`: what it commits.

use clustine_world::{Entity, EntityChange, EntityData, EntityField, Entry, TypeState};

use super::*;
use crate::living::MAX_AIR;
use crate::mobs::Mob;
use crate::mobs::part::body_ids;

const COW_KEY: EntityId = EntityId(40);

/// A cow at `position` with full health and air, holding `mob`'s parts.
fn cow(position: [f64; 3], on_ground: bool) -> Entity {
    let mut mob = Mob::fresh();
    mob.body.vitals.health = 10.0;
    mob.body.vitals.air = MAX_AIR;
    Entity {
        key: COW_KEY,
        kind: clustine_data::entity_types::COW as u16,
        uuid: uuid::Uuid::from_u128(40),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground,
        hops: 0,
        lived: 0,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data: mob.encode(),
    }
}

/// Lays `entity` in its loaded chunk, as a load would have it.
fn lay(region: &mut Region, entity: &Entity) {
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .unwrap()
        .apply_entity(&EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

fn record(region: &Region) -> Entity {
    region
        .find_entity(COW_KEY)
        .expect("the cow is here")
        .1
        .clone()
}

fn sets(output: &TickOutput) -> Vec<Vec<EntityField>> {
    output
        .entity_changes
        .iter()
        .filter_map(|(_, change)| match change {
            EntityChange::Set(key, fields) if *key == COW_KEY => Some(fields.clone()),
            _ => None,
        })
        .collect()
}

/// The motion a mob on the ground keeps from tick to tick: each tick's gravity, taken
/// after its move and through the air's drag, which the next move's collision cancels.
fn resting_motion() -> f64 {
    (0.0 - 0.08) * f64::from(0.98_f32)
}

#[test]
fn a_mob_in_the_air_falls_by_the_game_s_gravity_and_commits_its_move_with_its_lived() {
    let mut region = on_ticking_floor_with(&[]);
    lay(&mut region, &cow([8.5, -58.0, 8.5], false));
    // `travelInAir`: the move by the motion it has, then gravity and the air's drag; from
    // rest it moves in its second tick.
    let first = region.tick(&TickInputs::default());
    assert_eq!(record(&region).position, [8.5, -58.0, 8.5]);
    assert_eq!(record(&region).motion[1], resting_motion());
    let output = region.tick(&TickInputs::default());
    let after = record(&region);
    assert_eq!(after.position, [8.5, -58.0 + resting_motion(), 8.5]);
    assert_eq!(
        after.motion[1],
        (resting_motion() - 0.08) * f64::from(0.98_f32)
    );
    assert_eq!(after.lived, 2);
    for (tick, output) in [(1, first), (2, output)] {
        let [fields] = &sets(&output)[..] else {
            panic!("one commit: {:?}", output.entity_changes);
        };
        assert!(fields.contains(&EntityField::Lived(tick)), "{fields:?}");
        // Nothing of its parts changed.
        assert!(
            !fields
                .iter()
                .any(|field| matches!(field, EntityField::BodyEntry(_) | EntityField::BodyGone(_))),
            "{fields:?}"
        );
    }
}

#[test]
fn a_mob_standing_on_the_ground_commits_nothing_and_ages() {
    let mut region = on_ticking_floor_with(&[]);
    let mut standing = cow([8.5, -60.0, 8.5], true);
    standing.motion = [0.0, resting_motion(), 0.0];
    lay(&mut region, &standing);
    for _ in 0..3 {
        let output = region.tick(&TickInputs::default());
        assert_eq!(sets(&output), Vec::<Vec<EntityField>>::new());
        assert!(output.entities_ageing.contains(&ChunkPos::new(0, 0)));
    }
    let after = record(&region);
    assert_eq!((after.lived, after.position), (3, standing.position));
}

#[test]
fn a_mob_whose_record_this_build_cannot_read_is_left_as_it_is() {
    let mut region = on_ticking_floor_with(&[]);
    let mut unread = cow([8.5, -58.0, 8.5], false);
    unread.data = EntityData::Mob {
        body: vec![Entry {
            id: body_ids::VITALS,
            version: 200,
            bytes: vec![1, 2, 3],
        }],
        mind: Vec::new(),
        state: TypeState::default(),
    };
    lay(&mut region, &unread);
    let output = region.tick(&TickInputs::default());
    assert_eq!(output.unreadable_mobs, [COW_KEY]);
    assert_eq!(record(&region), unread);
    assert!(sets(&output).is_empty());
}

/// The read view of a mob's tick: its region's own entities before its copies', the
/// region's players, light where the chunk keeps none from the sky above its highest
/// block.
#[test]
fn a_mob_reads_its_region_s_entities_then_its_copies_players_and_the_sky_s_light() {
    use crate::mobs::view::MobWorld;
    let mut region = on_ticking_floor_with(&[]);
    let mut other = cow([9.0, -60.0, 8.5], true);
    other.key = EntityId(41);
    lay(&mut region, &cow([8.5, -60.0, 8.5], true));
    lay(&mut region, &other);
    let view = super::super::items::View {
        region: &region,
        home: ChunkPos::new(0, 0),
    };
    let around = crate::physics::aabb::Aabb::of_entity([8.5, -60.0, 8.5], 4.0, 2.0);
    let seen: Vec<(EntityId, bool)> = view
        .entities_in(&around)
        .iter()
        .map(|seen| (seen.entity.key, seen.copied))
        .collect();
    assert_eq!(seen, [(COW_KEY, false), (EntityId(41), false)]);
    let players = view.players();
    assert_eq!(players.len(), 1);
    assert!(players[0].creative && !players[0].sneaking);
    assert_eq!(players[0].eye_height, 1.62);
    assert!(view.is_loaded(BlockPos::new(8, -60, 8)));
    assert!(!view.is_loaded(BlockPos::new(1000, -60, 8)));
    // The floor's chunk keeps no light: the sky's above its top, none below.
    let above = view.light(BlockPos::new(8, -60, 8)).unwrap();
    let below = view.light(BlockPos::new(8, -62, 8)).unwrap();
    assert_eq!((above.sky, above.block, below.sky), (15, 0, 0));
}
