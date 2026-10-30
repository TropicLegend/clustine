//! Monsters through the region, from the specification, by someone who wrote none of
//! their code (`docs/groundwork/n9-plan.md`, sections 4.6, 5.5 and 8.4; D9 3.3, 5.6;
//! n7 Witch): a skeleton looses one arrow at each release of its bow, never a second
//! from the bow's own release; a witch drinks off the use state, for the game's
//! `usingTime`; an enderman takes a block across a pinned line as a `SetIf`.

use clustine_data::{entity_types, items};
use clustine_world::{ChunkPos, Effected, Entity, EntityData, EntityId, ItemStack};

use super::thrown::{cow, lay, mob};
use super::*;
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::body::effects::Effect;
use crate::mobs::types::enderman;
use crate::region::tnt::tests::{idle, nothing, records, state, west_of_a_line, world};

const SEED: u64 = 0x0B19_0D0D;

fn kind(id: i32) -> u16 {
    u16::try_from(id).expect("an entity type's id")
}

/// A mob of the type `of` at `feet` at its type's full health, its AI on or off, made
/// ready by `prepare`.
fn mob_at(
    region: &mut Region,
    of: i32,
    feet: [f64; 3],
    ai: bool,
    prepare: impl FnOnce(&mut Mob, &mut Entity),
) -> EntityId {
    let key = region
        .take_entity_key(ChunkPos::containing(feet[0], feet[2]))
        .expect("a key");
    let mut entity = cow(key, feet);
    entity.kind = kind(of);
    let mut made = Mob::decode(&entity.data).expect("a mob");
    made.body.vitals.health = crate::mobs::body::attributes::supplied(
        entity.kind,
        crate::mobs::body::attribute(clustine_data::attributes::MAX_HEALTH),
    )
    .expect("a health") as f32;
    made.body.flags.no_ai = !ai;
    prepare(&mut made, &mut entity);
    entity.data = made.encode();
    lay(region, &entity);
    key
}

fn arrows(region: &Region) -> Vec<EntityId> {
    records(region)
        .into_iter()
        .filter(|entity| i32::from(entity.kind) == entity_types::ARROW)
        .map(|entity| entity.key)
        .collect()
}

/// The plan's 4.6 (finding 12): the bow's `releaseUsing` acts only for a `Player`; a
/// skeleton shoots through its `performRangedAttack` alone, so each release of its bow
/// makes one arrow and no more, every sixty ticks at easy (forty waiting, twenty
/// drawing, D9 3.3 and the plan's 7).
#[test]
fn a_skeleton_looses_one_arrow_at_each_release_of_its_bow() {
    let mut region = world(SEED, nothing);
    mob_at(
        &mut region,
        entity_types::IRON_GOLEM,
        [10.5, -60.0, 0.5],
        false,
        |_, _| {},
    );
    mob_at(
        &mut region,
        entity_types::SKELETON,
        [0.5, -60.0, 0.5],
        true,
        |made, _| {
            made.body.equipment.slots[usize::from(slots::MAIN_HAND)] =
                Some(ItemStack::new(items::BOW, 1));
            // Kept from burning in the daylight.
            made.body.equipment.slots[usize::from(slots::HEAD)] =
                Some(ItemStack::new(items::IRON_HELMET, 1));
        },
    );
    let mut seen: Vec<EntityId> = arrows(&region);
    let mut loosed_at = Vec::new();
    for tick in 1..=260 {
        idle(&mut region);
        let now = arrows(&region);
        let new: Vec<EntityId> = now
            .iter()
            .copied()
            .filter(|key| !seen.contains(key))
            .collect();
        assert!(new.len() <= 1, "tick {tick}: {} arrows at once", new.len());
        if !new.is_empty() {
            loosed_at.push(tick);
        }
        seen.extend(new);
    }
    assert!(loosed_at.len() >= 3, "{loosed_at:?}");
    for pair in loosed_at.windows(2) {
        assert_eq!(pair[1] - pair[0], 60, "{loosed_at:?}");
    }
    // Each is the skeleton's: its damage drawn by `setBaseDamageFromMob` (2 and the
    // triangle about 0.11 at easy), never the bow's plain 2.
    for key in seen {
        if let Some((_, arrow)) = region.find_entity(key)
            && let EntityData::Arrow(arrow) = &arrow.data
        {
            assert_ne!(arrow.base_damage.0, 2.0, "a mob's arrow draws its damage");
        }
    }
}

/// `WitherSkeleton.getArrow`: a wither skeleton given a bow shoots its arrows burning
/// for 100 seconds (`igniteForSeconds(100)`), where a skeleton's do not burn.
#[test]
fn a_wither_skeleton_s_arrows_fly_burning_and_a_skeleton_s_do_not() {
    for (archer, burning) in [
        (entity_types::WITHER_SKELETON, true),
        (entity_types::SKELETON, false),
    ] {
        let mut region = world(SEED, nothing);
        mob_at(
            &mut region,
            entity_types::IRON_GOLEM,
            [10.5, -60.0, 0.5],
            false,
            |_, _| {},
        );
        mob_at(&mut region, archer, [0.5, -60.0, 0.5], true, |made, _| {
            made.body.equipment.slots[usize::from(slots::MAIN_HAND)] =
                Some(ItemStack::new(items::BOW, 1));
            made.body.equipment.slots[usize::from(slots::HEAD)] =
                Some(ItemStack::new(items::IRON_HELMET, 1));
        });
        let mut fired = None;
        for _ in 0..120 {
            idle(&mut region);
            if let Some(key) = arrows(&region).first() {
                fired = region.find_entity(*key).map(|(_, arrow)| arrow.fire_ticks);
                break;
            }
        }
        let fire = fired.expect("an arrow loosed");
        assert_eq!(fire > 0, burning, "{archer}: {fire}");
    }
}

/// The plan's 5.5 (finding 11) and n7 Witch `aiStep`: a witch drinks off the use state,
/// by its own `usingTime`, set to the potion's use duration (32) as it starts and
/// counted `usingTime-- > 0` at each `aiStep` after, so that the potion is drunk at the
/// 33rd tick after it is raised. A burning witch takes fire resistance.
#[test]
fn a_burning_witch_drinks_fire_resistance_off_the_use_state_in_thirty_three_ticks() {
    let mut region = world(SEED, nothing);
    let witch = mob_at(
        &mut region,
        entity_types::WITCH,
        [0.5, -60.0, 0.5],
        true,
        |_, entity| entity.fire_ticks = 4000,
    );
    let mut raised = None;
    let mut drunk = None;
    for tick in 1..=600 {
        idle(&mut region);
        // Kept burning, so that it wants the potion until it has drunk it.
        let (chunk, record) = region.find_entity(witch).expect("the witch");
        if record.fire_ticks < 1000 {
            let lit = Entity {
                fire_ticks: 4000,
                ..record.clone()
            };
            let _ = chunk;
            lay(&mut region, &lit);
        }
        let now = mob(&region, witch);
        assert!(
            now.body.using.using.is_none(),
            "tick {tick}: the witch's drinking is not the use state"
        );
        let holds_potion = now.body.equipment.slots[usize::from(slots::MAIN_HAND)]
            .as_ref()
            .is_some_and(|stack| stack.item == items::POTION);
        if holds_potion && raised.is_none() {
            raised = Some(tick);
        }
        if now.body.effects.has(Effect::FIRE_RESISTANCE) {
            drunk = Some(tick);
            assert!(!holds_potion, "the bottle is gone as it is drunk");
            break;
        }
    }
    let raised = raised.expect("the witch raised a potion");
    let drunk = drunk.expect("the witch drank it");
    assert_eq!(drunk - raised, 33);
}

/// The plan's 8.4 "Across a line" (question 7): a block an enderman takes from a copy
/// of another region's land is taken by a `SetIf` expecting the block and setting air,
/// and the enderman carries it as the take is sent. The enderman is penned west of the
/// line by fences and roofed against the day (its daylight teleport needs the sky); the
/// only holdable blocks are the dirt east of the line.
#[test]
fn an_enderman_takes_dirt_across_a_pinned_line_by_a_set_if() {
    let dirt = clustine_data::blocks::DIRT;
    let fence = state("minecraft:oak_fence");
    let roof = clustine_data::blocks::STONE;
    let mut region = west_of_a_line(SEED, move |pos| {
        let pen_x = (12..=15).contains(&pos.x);
        let pen_z = (-2..=3).contains(&pos.z);
        if pos.y == -60 && (16..=17).contains(&pos.x) && (-4..=5).contains(&pos.z) {
            Some(dirt)
        } else if pos.y == -60
            && pen_x
            && pen_z
            && (pos.x == 12 || pos.x == 15 || pos.z == -2 || pos.z == 3)
        {
            Some(fence)
        } else if pos.y == -56 && (11..=15).contains(&pos.x) && (-3..=4).contains(&pos.z) {
            Some(roof)
        } else {
            None
        }
    });
    let enderman = mob_at(
        &mut region,
        entity_types::ENDERMAN,
        [14.4, -60.0, 0.5],
        true,
        |_, _| {},
    );
    let mut taken = None;
    for _ in 0..12_000 {
        let output = idle(&mut region);
        let sets: Vec<(BlockPos, Effected)> = output
            .effects
            .iter()
            .filter(|effect| matches!(effect.what, Effected::SetIf { .. }))
            .map(|effect| (effect.at, effect.what.clone()))
            .collect();
        if let Some(set) = sets.first() {
            taken = Some(set.clone());
            break;
        }
    }
    let (at, what) = taken.expect("the enderman took a block across the line");
    assert!((16..=17).contains(&at.x) && at.y == -60, "{at:?}");
    let Effected::SetIf {
        expected, state, ..
    } = what
    else {
        unreachable!()
    };
    assert_eq!(expected, dirt, "expecting the dirt the copy shows");
    assert!(state.is_air(), "and leaving air: {state:?}");
    let carried = enderman::carried(&mob(&region, enderman));
    assert_eq!(
        carried,
        Some(dirt),
        "it carries the dirt as the take is sent"
    );
}
