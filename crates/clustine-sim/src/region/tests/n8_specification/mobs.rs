//! What mobs read of the region's light (the plan's 1.8 and 6.5): an undead mob burns
//! where it sees the sky (sky light 15), which glass lets through and a roof or a tree's
//! leaves (one less) do not.

use clustine_data::entity_types;
use clustine_world::{Entity, EntityChange, EntityId};

use super::*;
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::body::attributes::supplied;

const ZOMBIE: EntityId = EntityId(77);

fn zombie_at(position: [f64; 3]) -> Entity {
    let kind = entity_types::ZOMBIE as u16;
    let mut mob = Mob::fresh();
    mob.body.vitals.health = supplied(kind, attribute(clustine_data::attributes::MAX_HEALTH))
        .expect("a zombie's") as f32;
    mob.body.vitals.air = 300;
    Entity {
        portal: Default::default(),
        key: ZOMBIE,
        kind,
        uuid: uuid::Uuid::from_u128(77),
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
        data: mob.encode(),
    }
}

/// A zombie on the floor of the 5×5 that ticks, under a ceiling of `cover` over all of
/// it at y = −57, at noon: whether it catches fire within 400 ticks (a chance of 1.2 in
/// 30 a tick where it burns), and the sky's light where its eyes are.
fn burns_under(cover: Option<BlockState>) -> (bool, Option<u8>) {
    let ceiling: Vec<(BlockPos, BlockState)> = match cover {
        Some(cover) => (-32..48)
            .flat_map(|x| (-32..48).map(move |z| (at(x, ROOF, z), cover)))
            .collect(),
        None => Vec::new(),
    };
    let mut region = super::super::redstone::ticking_with(&ceiling);
    region.clock.day = NOON;
    let zombie = zombie_at([8.5, f64::from(GROUND), 8.5]);
    let chunk = zombie.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .expect("loaded")
        .apply_entity(&EntityChange::Whole(zombie.clone()));
    region.entity_chunks.insert(ZOMBIE, chunk);
    let eyes = at(8, GROUND + 1, 8);
    let sky = levels(&region, eyes).map(|(sky, _)| sky);
    for _ in 0..400 {
        checked(&mut region, &TickInputs::default());
        let (_, now) = region.find_entity(ZOMBIE).expect("the zombie is here");
        if now.fire_ticks > 0 {
            return (true, sky);
        }
    }
    (false, sky)
}

#[test]
fn a_zombie_burns_at_noon_in_the_open_and_under_glass() {
    assert_eq!(burns_under(None), (true, Some(15)));
    assert_eq!(burns_under(Some(blocks::GLASS)), (true, Some(15)));
}

#[test]
fn a_zombie_does_not_burn_under_a_roof_or_a_tree_s_leaves() {
    assert_eq!(burns_under(Some(blocks::STONE)), (false, Some(0)));
    // The sky's light is 14 in the leaves and one less a block below them: 12 at the
    // zombie's eyes, two blocks under.
    let leaves = state("minecraft:oak_leaves[persistent=true]");
    assert_eq!(burns_under(Some(leaves)), (false, Some(12)));
}
