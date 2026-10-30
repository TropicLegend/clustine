//! The blaze and the wither skeleton in a region of the Nether (family E3 of P12,
//! `docs/groundwork/p-plan.md`, section 18): a fortress's pieces spawn their list on
//! nether bricks, blazes and wither skeletons among them, the wither skeletons with
//! their swords; a blaze's volley makes three small fireballs that set its target
//! alight; a wither skeleton's hit withers what it strikes.

use clustine_data::{blocks, entity_types, items};
use clustine_world::{
    ChunkPos, Dimension, Entity, EntityChange, EntityData, EntityId, KeptBox, KeptKind,
    KeptStructure,
};

use super::*;
use crate::api::PublishedCounts;
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::body::attributes::supplied;
use crate::mobs::body::effects::Effect;

/// The floor of the tests' Nether, its top a layer of nether bricks at `FLOOR + 1`.
const FLOOR: i32 = 64;
const STANDING: f64 = (FLOOR + 2) as f64;

/// Keyed below the attackers, so that it ticks before them and its record holds what
/// their hits leave it with.
const PIG: EntityId = EntityId(30);
const ATTACKER: EntityId = EntityId(40);

/// A region of the Nether's 5×5 chunks about the origin's, of stone under a layer of
/// nether bricks.
fn nether() -> Region {
    crate::region::portals::tests::world_in(Dimension::Nether, FLOOR, |pos| {
        (pos.y == FLOOR + 1).then_some(blocks::NETHER_BRICKS)
    })
}

/// A mob of the type `kind` at `position`, of its type's full health and air.
fn mob_at(key: EntityId, kind: i32, position: [f64; 3], mut mob: Mob) -> Entity {
    let kind = kind as u16;
    mob.body.vitals.health = supplied(kind, attribute(clustine_data::attributes::MAX_HEALTH))
        .expect("a mob's health") as f32;
    mob.body.vitals.air = crate::mobs::types::of(kind).max_air();
    Entity {
        portal: Default::default(),
        key,
        kind,
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
        data: mob.encode(),
    }
}

/// A pig with no mind of its own, which neither wanders nor flees.
fn still_pig(position: [f64; 3]) -> Entity {
    let mut mob = Mob::fresh();
    mob.body.flags.no_ai = true;
    mob_at(PIG, entity_types::PIG, position, mob)
}

/// A mob of the type `kind` whose target is the pig.
fn hunting(kind: i32, position: [f64; 3], mob: Mob) -> Entity {
    let mut mob = mob;
    mob.body.combat.target = Some(PIG);
    mob_at(ATTACKER, kind, position, mob)
}

fn lay(region: &mut Region, entity: &Entity) {
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .expect("a loaded chunk")
        .apply_entity(&EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

fn record(region: &Region, key: EntityId) -> Option<Entity> {
    region.find_entity(key).map(|(_, entity)| entity.clone())
}

/// The keys of the small fireballs whose records a tick sent whole.
fn small_fireballs(output: &TickOutput) -> Vec<EntityId> {
    output
        .entity_changes
        .iter()
        .filter_map(|(_, change)| match change {
            EntityChange::Whole(entity)
                if i32::from(entity.kind) == entity_types::SMALL_FIREBALL
                    && matches!(entity.data, EntityData::Fireball(_)) =>
            {
                Some(entity.key)
            }
            _ => None,
        })
        .collect()
}

/// The mobs a tick made, as first committed.
fn made(output: &TickOutput) -> Vec<Entity> {
    output
        .entity_changes
        .iter()
        .filter_map(|(_, change)| match change {
            EntityChange::Whole(entity) if matches!(entity.data, EntityData::Mob { .. }) => {
                Some(entity.clone())
            }
            _ => None,
        })
        .collect()
}

#[test]
fn a_fortress_s_pieces_spawn_blazes_and_sworded_wither_skeletons_of_its_list() {
    let mut region = nether();
    region.config.spawn_mobs = true;
    // One piece over the whole of the land, from the bricks up, kept by every chunk.
    let piece = KeptBox {
        min: BlockPos::new(-40, FLOOR + 1, -40),
        max: BlockPos::new(39, FLOOR + 30, 39),
    };
    let fortress = KeptStructure {
        kind: KeptKind::Fortress,
        source: ChunkPos::new(0, 0),
        bounds: piece,
        pieces: vec![piece],
    };
    for chunk in region.chunks.values_mut() {
        chunk.set_structures(vec![fortress.clone()]);
    }
    crate::region::tnt::tests::add_player(&mut region, 1, [0.5, STANDING, 0.5], false);
    let list = [
        entity_types::BLAZE,
        entity_types::ZOMBIFIED_PIGLIN,
        entity_types::WITHER_SKELETON,
        entity_types::SKELETON,
        entity_types::MAGMA_CUBE,
    ];
    let mut spawned = Vec::new();
    for _ in 0..600 {
        let inputs = TickInputs {
            counts: Some(PublishedCounts::default()),
            ..TickInputs::default()
        };
        spawned.extend(made(&region.tick(&inputs)));
        let kinds = |id: i32| {
            spawned
                .iter()
                .filter(|mob| i32::from(mob.kind) == id)
                .count()
        };
        if kinds(entity_types::BLAZE) > 0 && kinds(entity_types::WITHER_SKELETON) > 0 {
            break;
        }
    }
    assert!(
        spawned
            .iter()
            .any(|mob| i32::from(mob.kind) == entity_types::BLAZE),
        "blazes came: {:?}",
        spawned.iter().map(|mob| mob.kind).collect::<Vec<_>>()
    );
    let skeletons: Vec<&Entity> = spawned
        .iter()
        .filter(|mob| i32::from(mob.kind) == entity_types::WITHER_SKELETON)
        .collect();
    assert!(!skeletons.is_empty(), "wither skeletons came");
    for mob in &spawned {
        assert!(list.contains(&i32::from(mob.kind)), "{}", mob.kind);
        assert_eq!(mob.position[1], STANDING, "on the bricks");
    }
    for skeleton in skeletons {
        let mob = Mob::decode(&skeleton.data).expect("a mob");
        let hand = mob.body.equipment.slots[usize::from(slots::MAIN_HAND)].as_ref();
        assert_eq!(hand.map(|stack| stack.item), Some(items::STONE_SWORD));
    }
}

#[test]
fn a_blaze_s_volley_makes_three_small_fireballs_that_set_its_target_alight() {
    let mut region = nether();
    lay(&mut region, &still_pig([8.5, STANDING, 0.5]));
    lay(
        &mut region,
        &hunting(entity_types::BLAZE, [0.5, STANDING, 0.5], Mob::fresh()),
    );
    let mut shots = Vec::new();
    let mut seen: Vec<EntityId> = Vec::new();
    let mut alight = false;
    let mut hurt = false;
    for tick in 1..=100 {
        let output = checked(&mut region, &TickInputs::default());
        // A record sent whole again (a crossing into another chunk) is no new shot.
        for key in small_fireballs(&output) {
            if !seen.contains(&key) {
                seen.push(key);
                shots.push(tick);
            }
        }
        match record(&region, PIG) {
            Some(pig) => {
                alight |= pig.fire_ticks > 0;
                let mob = Mob::decode(&pig.data).expect("a mob");
                hurt |= mob.body.vitals.health < 10.0;
            }
            None => hurt = true,
        }
    }
    assert_eq!(shots.len(), 3, "{shots:?}");
    assert_eq!(shots[1] - shots[0], 6);
    assert_eq!(shots[2] - shots[1], 6);
    assert!(alight, "set alight");
    assert!(hurt, "hurt");
}

#[test]
fn a_wither_skeleton_s_hit_withers_the_pig_it_strikes() {
    let mut region = nether();
    lay(&mut region, &still_pig([2.0, STANDING, 0.5]));
    let mut skeleton = Mob::fresh();
    skeleton.body.equipment.slots[usize::from(slots::MAIN_HAND)] =
        Some(clustine_world::ItemStack::new(items::STONE_SWORD, 1));
    lay(
        &mut region,
        &hunting(
            entity_types::WITHER_SKELETON,
            [0.5, STANDING, 0.5],
            skeleton,
        ),
    );
    let mut withered = false;
    for _ in 0..80 {
        checked(&mut region, &TickInputs::default());
        let Some(pig) = record(&region, PIG) else {
            break;
        };
        let mob = Mob::decode(&pig.data).expect("a mob");
        if mob.body.effects.has(Effect::WITHER) {
            withered = true;
            break;
        }
    }
    assert!(withered, "the pig withers");
}
