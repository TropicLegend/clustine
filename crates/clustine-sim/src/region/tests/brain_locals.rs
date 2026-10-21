//! What a mob's brain does to its region after its tick, from the game's
//! `DoorBlock.setOpen` and `RamTarget`'s hit (`docs/groundwork/n7j-plan.md`, section 4.1
//! item 5): a door opened with its other half, its sound and its event, and none where
//! it is already open; a hit's knockback after the hurt, and none where the hurt did not
//! take.

use clustine_data::{BlockState, entity_types, sounds};
use clustine_world::{Entity, EntityChange, EntityId};

use super::*;
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::body::attributes::supplied;
use crate::mobs::tick::Local;

const RAMMER: EntityId = EntityId(40);
const RAMMED: EntityId = EntityId(42);

fn door(half: &str, open: bool) -> BlockState {
    let text = format!("minecraft:oak_door[half={half},open={open}]");
    BlockState::parse(&text).unwrap_or_else(|| panic!("{text} is a block state"))
}

fn block_at(region: &Region, position: BlockPos) -> Option<BlockState> {
    let (x, z) = position.in_chunk();
    region.chunk(position.chunk())?.get(x, position.y, z)
}

/// A closed oak door on the floor of the region's ticking chunk.
fn with_door(open: bool) -> (Region, BlockPos, BlockPos) {
    let (lower, upper) = (BlockPos::new(2, -60, 3), BlockPos::new(2, -59, 3));
    let region =
        on_ticking_floor_with(&[(lower, door("lower", open)), (upper, door("upper", open))]);
    (region, lower, upper)
}

#[test]
fn a_door_a_mob_opens_opens_with_its_other_half_and_is_heard_at_the_pitch_drawn() {
    let (mut region, lower, upper) = with_door(false);
    let mut output = TickOutput::default();
    region.apply_locals(
        RAMMER,
        vec![Local::OpenDoor {
            pos: lower,
            open: true,
            pitch: 0.95,
        }],
        &mut output,
    );
    assert_eq!(block_at(&region, lower), Some(door("lower", true)));
    assert_eq!(block_at(&region, upper), Some(door("upper", true)));
    let heard: Vec<_> = output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound,
                source,
                position,
                volume,
                pitch,
                ..
            } => Some((*sound, *source, *position, *volume, *pitch)),
            _ => None,
        })
        .collect();
    assert_eq!(
        heard,
        [(
            sounds::BLOCK_WOODEN_DOOR_OPEN,
            crate::api::SOUND_SOURCE_BLOCKS,
            clustine_world::Vec3::new(2.5, -59.5, 3.5),
            1.0,
            0.95
        )]
    );
}

#[test]
fn a_door_a_mob_closes_is_heard_closing() {
    let (mut region, lower, upper) = with_door(true);
    let mut output = TickOutput::default();
    region.apply_locals(
        RAMMER,
        vec![Local::OpenDoor {
            pos: upper,
            open: false,
            pitch: 0.9,
        }],
        &mut output,
    );
    assert_eq!(block_at(&region, lower), Some(door("lower", false)));
    assert_eq!(block_at(&region, upper), Some(door("upper", false)));
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::Sound { sound, .. } if *sound == sounds::BLOCK_WOODEN_DOOR_CLOSE
    )));
}

#[test]
fn a_door_already_open_is_left_as_it_is_and_silent() {
    let (mut region, lower, _) = with_door(true);
    let mut output = TickOutput::default();
    region.apply_locals(
        RAMMER,
        vec![Local::OpenDoor {
            pos: lower,
            open: true,
            pitch: 0.95,
        }],
        &mut output,
    );
    assert!(output.events.is_empty(), "{:?}", output.events);
}

/// A cow of full health, or dead, in the air at `position`.
fn cow_at(key: EntityId, position: [f64; 3], alive: bool) -> Entity {
    let kind = entity_types::COW as u16;
    let mut mob = Mob::fresh();
    if alive {
        mob.body.vitals.health =
            supplied(kind, attribute(clustine_data::attributes::MAX_HEALTH)).unwrap() as f32;
    }
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

/// The rammed cow's motion after a hit of 2 from the west with `knockback`.
fn rammed(alive: bool, knockback: Option<[f64; 3]>) -> [f64; 3] {
    let mut region = on_ticking_floor_with(&[]);
    let cow = cow_at(RAMMED, [4.5, -55.0, 4.5], alive);
    let chunk = cow.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .unwrap()
        .apply_entity(&EntityChange::Whole(cow));
    region.entity_chunks.insert(RAMMED, chunk);
    let mut output = TickOutput::default();
    region.apply_locals(
        RAMMER,
        vec![Local::Hurt {
            key: RAMMED,
            kind: entity_types::GOAT as u16,
            from: [2.5, -55.0, 4.5],
            amount: 2.0,
            sound: None,
            knockback,
        }],
        &mut output,
    );
    region.find_entity(RAMMED).unwrap().1.motion
}

#[test]
fn a_hits_knockback_pushes_the_hurt_mob_after_the_hurts_own() {
    let hurt = rammed(true, None);
    let pushed = rammed(true, Some([0.5, -1.0, 0.0]));
    // `knockback(0.5, -1, 0)` of the motion the hurt left: half of it kept sideways, and
    // pushed 0.5 away from (-1, 0), so east; in the air, its fall kept.
    assert_eq!(pushed, [hurt[0] / 2.0 + 0.5, hurt[1], hurt[2] / 2.0]);
}

#[test]
fn a_hit_that_does_not_take_pushes_nothing() {
    // A dead mob's `hurtServer` refuses the hurt before anything else.
    assert_eq!(rammed(false, Some([0.5, -1.0, 0.0])), [0.0; 3]);
}
