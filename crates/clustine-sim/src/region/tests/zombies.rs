//! The zombie family in a region, from the brief's sections 3.4, 11.1, 11.5 and 11.8
//! (`docs/groundwork/n7k-brief.md`): a zombie whose eyes are under water turns into a
//! drowned 901 ticks later, its count kept by its record and lost when it leaves the
//! water; a husk turns into a zombie; a zombie and a drowned walk to a turtle egg and
//! trample it; a drowned walks to water by day and swims up at night; an egg makes
//! them, and one used on one makes a baby.

use clustine_data::{BlockState, blocks, entity_types, items, sounds};
use clustine_world::{Entity, EntityChange, EntityId};

use super::*;
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::body::attributes::supplied;
use crate::mobs::types::zombie_family as family;

const ONE: EntityId = EntityId(40);

/// A mob of the type `kind` at `position`, fresh, of its type's full health.
fn mob_at(key: EntityId, kind: i32, position: [f64; 3]) -> Entity {
    let kind = kind as u16;
    let mut mob = Mob::fresh();
    mob.body.vitals.health =
        supplied(kind, attribute(clustine_data::attributes::MAX_HEALTH)).unwrap() as f32;
    mob.body.vitals.air = 300;
    Entity {
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

fn sounds_of(output: &TickOutput) -> Vec<i32> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound { sound, .. } => Some(*sound),
            _ => None,
        })
        .collect()
}

fn level_events(output: &TickOutput) -> Vec<i32> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::LevelEvent { kind, .. } => Some(*kind),
            _ => None,
        })
        .collect()
}

/// A well of water one block across and three deep at x = 8, z = 8 (y = -61 to -59),
/// stone round it and under it: a zombie in it stands with its eyes under water.
fn well() -> Vec<(BlockPos, BlockState)> {
    let mut blocks = vec![(BlockPos::new(8, -62, 8), blocks::STONE)];
    for x in 7..=9 {
        for z in 7..=9 {
            for y in -61..=-59 {
                let state = if x == 8 && z == 8 {
                    blocks::WATER
                } else {
                    blocks::STONE
                };
                blocks.push((BlockPos::new(x, y, z), state));
            }
        }
    }
    blocks
}

/// A pool of water from x, z = 4 to 11 and y = -61 to -57, stone under it and round it.
fn pool() -> Vec<(BlockPos, BlockState)> {
    let mut blocks = Vec::new();
    for x in 3..=12 {
        for z in 3..=12 {
            blocks.push((BlockPos::new(x, -62, z), blocks::STONE));
            for y in -61..=-57 {
                let wall = x == 3 || x == 12 || z == 3 || z == 12;
                let state = if wall { blocks::STONE } else { blocks::WATER };
                blocks.push((BlockPos::new(x, y, z), state));
            }
        }
    }
    blocks
}

/// The night, when no monster burns and a drowned swims up.
const NIGHT: u64 = 18_000;

/// The tick, counting from 1, at which the mob `key` of the region turned into another,
/// within `ticks`: the new mob, and that tick's output.
fn converted_within(
    region: &mut Region,
    key: EntityId,
    ticks: usize,
) -> Option<(usize, TickOutput)> {
    for tick in 1..=ticks {
        let output = checked(region, &TickInputs::default());
        if record(region, key).is_none() {
            return Some((tick, output));
        }
    }
    None
}

#[test]
fn a_zombie_under_water_turns_drowned_on_its_901st_tick_with_its_sword_and_its_own_health() {
    let mut region = on_ticking_floor_with(&well());
    region.clock.day = NIGHT;
    let mut zombie = mob_at(ONE, entity_types::ZOMBIE, [8.5, -61.0, 8.5]);
    let mut held = Mob::decode(&zombie.data).unwrap();
    held.body.vitals.health = 7.0;
    held.body.equipment.slots[0] = Some(ItemStack {
        item: items::IRON_SWORD,
        count: 1,
    });
    zombie.data = held.encode();
    lay(&mut region, &zombie);
    for tick in 1..=600 {
        checked(&mut region, &TickInputs::default());
        let own = family::read(&mob(&region, ONE));
        assert_eq!(own.drowning.converting, tick >= 600, "{tick}");
    }
    // Clients see it shake from then (its synched 18).
    let shown = crate::shown::mob_values(
        entity_types::ZOMBIE as u16,
        &record(&region, ONE).unwrap().data,
    );
    assert!(shown.contains(&(18, crate::shown::DataShown::Boolean(true), false)));
    let (tick, output) = converted_within(&mut region, ONE, 400).expect("it turned");
    assert_eq!(tick, 301, "on the 901st tick");
    let drowned = all_of(&region, entity_types::DROWNED);
    assert_eq!(drowned.len(), 1);
    let new = Mob::decode(&drowned[0].data).unwrap();
    assert_eq!(
        new.body.equipment.slots[0].map(|stack| stack.item),
        Some(items::IRON_SWORD)
    );
    assert_eq!(new.body.vitals.health, 20.0, "the drowned's own health");
    assert!(level_events(&output).contains(&1040), "heard");
    // `handleAttributes(CONVERSION)` gave it a knockback bonus of its own.
    assert!(
        new.body
            .attributes
            .made(attribute(clustine_data::attributes::KNOCKBACK_RESISTANCE))
            .is_some_and(|instance| instance.has_modifier("minecraft:random_spawn_bonus"))
    );
}

#[test]
fn a_restore_in_the_middle_of_the_count_keeps_it() {
    let mut region = on_ticking_floor_with(&well());
    region.clock.day = NIGHT;
    lay(
        &mut region,
        &mob_at(ONE, entity_types::ZOMBIE, [8.5, -61.0, 8.5]),
    );
    for _ in 0..450 {
        checked(&mut region, &TickInputs::default());
    }
    // Its record, as the store keeps it, taken into a region of its own.
    let kept = record(&region, ONE).unwrap();
    let mut restored = on_ticking_floor_with(&well());
    restored.clock.day = NIGHT;
    lay(&mut restored, &kept);
    assert_eq!(family::read(&mob(&restored, ONE)).drowning.affliction, 450);
    let (tick, _) = converted_within(&mut restored, ONE, 600).expect("it turned");
    assert_eq!(tick, 451);
}

#[test]
fn leaving_the_water_starts_the_count_again() {
    let mut region = on_ticking_floor_with(&well());
    region.clock.day = NIGHT;
    lay(
        &mut region,
        &mob_at(ONE, entity_types::ZOMBIE, [8.5, -61.0, 8.5]),
    );
    for _ in 0..700 {
        checked(&mut region, &TickInputs::default());
    }
    assert!(family::read(&mob(&region, ONE)).drowning.converting);
    // Out on the floor beside the well.
    let mut out = record(&region, ONE).unwrap();
    take_away(&mut region, ONE);
    out.position = [2.5, -60.0, 2.5];
    lay(&mut region, &out);
    checked(&mut region, &TickInputs::default());
    let own = family::read(&mob(&region, ONE));
    assert!(!own.drowning.converting);
    assert_eq!(own.drowning.affliction, -1);
}

#[test]
fn a_husk_under_water_turns_into_a_zombie_heard_as_1041() {
    let mut region = on_ticking_floor_with(&well());
    region.clock.day = NIGHT;
    lay(
        &mut region,
        &mob_at(ONE, entity_types::HUSK, [8.5, -61.0, 8.5]),
    );
    let (tick, output) = converted_within(&mut region, ONE, 1000).expect("it turned");
    assert_eq!(tick, 901);
    assert_eq!(all_of(&region, entity_types::ZOMBIE).len(), 1);
    assert!(level_events(&output).contains(&1041));
}

#[test]
fn a_drowned_and_a_zombie_villager_never_drown_into_anything() {
    for kind in [entity_types::DROWNED, entity_types::ZOMBIE_VILLAGER] {
        let mut region = on_ticking_floor_with(&well());
        region.clock.day = NIGHT;
        lay(&mut region, &mob_at(ONE, kind, [8.5, -61.0, 8.5]));
        for _ in 0..950 {
            checked(&mut region, &TickInputs::default());
        }
        assert!(record(&region, ONE).is_some(), "{kind}");
        assert!(!family::read(&mob(&region, ONE)).drowning.converting);
    }
}

#[test]
fn a_zombie_walks_to_a_turtle_egg_and_tramples_it() {
    let egg = BlockPos::new(8, -60, 4);
    let mut region = on_ticking_floor_with(&[(egg, blocks::TURTLE_EGG)]);
    region.clock.day = NIGHT;
    lay(
        &mut region,
        &mob_at(ONE, entity_types::ZOMBIE, [8.5, -60.0, 11.5]),
    );
    let mut heard_digging = false;
    let mut broken_at = None;
    for tick in 1..=800 {
        let output = checked(&mut region, &TickInputs::default());
        let heard = sounds_of(&output);
        heard_digging |= heard.contains(&sounds::ENTITY_ZOMBIE_DESTROY_EGG);
        if region.block(egg) == Some(blocks::AIR) {
            assert!(heard.contains(&sounds::ENTITY_TURTLE_EGG_BREAK));
            broken_at = Some(tick);
            break;
        }
    }
    assert!(heard_digging, "it was heard at the egg");
    let broken_at = broken_at.expect("the egg was trampled");
    // It walked seven blocks and stood on the egg sixty ticks and more.
    assert!(broken_at > 61, "{broken_at}");
    let zombie = record(&region, ONE).unwrap();
    assert!(
        (zombie.position[0] - 8.5).abs() < 1.2 && (zombie.position[2] - 4.5).abs() < 1.2,
        "{:?}",
        zombie.position
    );
}

/// K 11.1 and 11.8: the drowned inherits `Zombie.registerGoals`, the turtle egg goal
/// among them, and on land at night walks to an egg and tramples it as a zombie does.
#[test]
fn a_drowned_walks_to_a_turtle_egg_and_tramples_it() {
    let egg = BlockPos::new(8, -60, 4);
    let mut region = on_ticking_floor_with(&[(egg, blocks::TURTLE_EGG)]);
    region.clock.day = NIGHT;
    lay(
        &mut region,
        &mob_at(ONE, entity_types::DROWNED, [8.5, -60.0, 11.5]),
    );
    let mut heard_digging = false;
    let mut broken_at = None;
    for tick in 1..=800 {
        let output = checked(&mut region, &TickInputs::default());
        let heard = sounds_of(&output);
        heard_digging |= heard.contains(&sounds::ENTITY_ZOMBIE_DESTROY_EGG);
        if region.block(egg) == Some(blocks::AIR) {
            assert!(heard.contains(&sounds::ENTITY_TURTLE_EGG_BREAK));
            broken_at = Some(tick);
            break;
        }
    }
    assert!(heard_digging, "it was heard at the egg");
    let broken_at = broken_at.expect("the egg was trampled");
    assert!(broken_at > 61, "{broken_at}");
}

/// Where a mob of `kind` is after `ticks` alone on the floor at night, from `start`,
/// with a turtle egg at `egg`; and whether the egg is still there.
fn after_walking_to_an_egg(
    kind: i32,
    start: [f64; 3],
    egg: BlockPos,
    ticks: usize,
) -> ([f64; 3], bool) {
    let mut region = on_ticking_floor_with(&[(egg, blocks::TURTLE_EGG)]);
    region.clock.day = NIGHT;
    lay(&mut region, &mob_at(ONE, kind, start));
    for _ in 0..ticks {
        checked(&mut region, &TickInputs::default());
    }
    let position = record(&region, ONE).expect("the mob is here").position;
    (position, region.block(egg) == Some(blocks::TURTLE_EGG))
}

/// On land the drowned walks to an egg by the zombie's own move control and path. Sent
/// along the exact diagonal of the blocks, where the path ends on the egg's block and
/// the walk can stop a little short of `acceptedDistance` (1.14 from the middle of the
/// block above the egg), the two stand in the same place and leave the egg alike.
#[test]
fn a_drowned_walks_to_an_egg_on_land_exactly_as_a_zombie_does() {
    let egg = BlockPos::new(9, -60, 9);
    let start = [0.5, -60.0, 0.5];
    let drowned = after_walking_to_an_egg(entity_types::DROWNED, start, egg, 300);
    let zombie = after_walking_to_an_egg(entity_types::ZOMBIE, start, egg, 300);
    assert_eq!(drowned, zombie);
}

/// A pool sunk into the floor, its surface the floor's: water from x, z = 4 to 11 and
/// y = -63 to -61, stone round it and under it.
fn sunk_pool() -> Vec<(BlockPos, BlockState)> {
    let mut blocks = Vec::new();
    for x in 3..=12 {
        for z in 3..=12 {
            blocks.push((BlockPos::new(x, -64, z), blocks::STONE));
            for y in -63..=-61 {
                let wall = x == 3 || x == 12 || z == 3 || z == 12;
                let state = if wall { blocks::STONE } else { blocks::WATER };
                blocks.push((BlockPos::new(x, y, z), state));
            }
        }
    }
    blocks
}

#[test]
fn a_drowned_on_land_walks_to_water_by_day() {
    let mut region = on_ticking_floor_with(&sunk_pool());
    region.clock.day = 6_000;
    // A helmet keeps the sun off it.
    let mut drowned = mob_at(ONE, entity_types::DROWNED, [1.5, -60.0, 1.5]);
    let mut worn = Mob::decode(&drowned.data).unwrap();
    worn.body.equipment.slots[5] = Some(ItemStack {
        item: items::LEATHER_HELMET,
        count: 1,
    });
    drowned.data = worn.encode();
    lay(&mut region, &drowned);
    let mut wet = false;
    for _ in 0..600 {
        checked(&mut region, &TickInputs::default());
        let at = crate::physics::containing(record(&region, ONE).unwrap().position);
        if region
            .block(at)
            .is_some_and(clustine_blocks::data::has_water)
        {
            wet = true;
            break;
        }
    }
    assert!(wet, "it went into the water");
}

#[test]
fn a_drowned_deep_in_water_at_night_searches_for_land_and_swims_up() {
    let mut region = on_ticking_floor_with(&pool());
    region.clock.day = NIGHT;
    let start = [7.5, -61.0, 7.5];
    lay(&mut region, &mob_at(ONE, entity_types::DROWNED, start));
    let mut swam = false;
    let mut highest = start[1];
    for _ in 0..200 {
        checked(&mut region, &TickInputs::default());
        let drowned = mob(&region, ONE);
        swam |= drowned.body.swimming.swimming;
        if swam {
            assert!(family::read(&drowned).searching_for_land);
        }
        highest = highest.max(record(&region, ONE).unwrap().position[1]);
    }
    assert!(swam, "it was seen swimming");
    // Clients are told so by its shared flag 4.
    assert!(highest > start[1] + 1.0, "it rose: {highest}");
}

#[test]
fn a_drowned_in_water_by_day_neither_swims_nor_searches_for_land() {
    let mut region = on_ticking_floor_with(&pool());
    region.clock.day = 6_000;
    lay(
        &mut region,
        &mob_at(ONE, entity_types::DROWNED, [7.5, -61.0, 7.5]),
    );
    for _ in 0..200 {
        checked(&mut region, &TickInputs::default());
        let drowned = mob(&region, ONE);
        assert!(!drowned.body.swimming.swimming);
        assert!(!family::read(&drowned).searching_for_land);
        assert_eq!(
            crate::shown::mob_shared_flags(&record(&region, ONE).unwrap().data) & 0x10,
            0
        );
    }
}

#[test]
fn an_egg_makes_each_of_the_family_and_one_used_on_a_zombie_makes_a_baby_zombie() {
    for (kind, egg) in [
        (entity_types::ZOMBIE, items::ZOMBIE_SPAWN_EGG),
        (entity_types::HUSK, items::HUSK_SPAWN_EGG),
        (entity_types::DROWNED, items::DROWNED_SPAWN_EGG),
        (
            entity_types::ZOMBIE_VILLAGER,
            items::ZOMBIE_VILLAGER_SPAWN_EGG,
        ),
    ] {
        let mut region = on_ticking_floor_with(&[]);
        region.clock.day = NIGHT;
        let mut output = TickOutput::default();
        let key = region
            .spawn_mob(
                kind as u16,
                player(1),
                BlockPos::new(4, -60, 4),
                true,
                true,
                &mut output,
            )
            .expect("made");
        assert_eq!(i32::from(record(&region, key).unwrap().kind), kind);
        // Used on it, the egg makes a baby of its type at its feet.
        checked(
            &mut region,
            &moves(vec![
                set_slot(
                    1,
                    0,
                    Some(ItemStack {
                        item: egg,
                        count: 1,
                    }),
                ),
                select(1, 0),
            ]),
        );
        let before = all_of(&region, kind).len();
        checked(
            &mut region,
            &moves(vec![numbered(
                1,
                PlayerInput::Interact {
                    entity: key,
                    location: [0.0, 1.0, 0.0],
                    sneaking: false,
                },
            )]),
        );
        let all = all_of(&region, kind);
        assert_eq!(all.len(), before + 1, "{kind}");
        let babies: Vec<_> = all
            .iter()
            .filter(|entity| family::is_baby(&Mob::decode(&entity.data).unwrap()))
            .collect();
        assert_eq!(babies.len(), 1, "{kind}");
        let baby = Mob::decode(&babies[0].data).unwrap();
        assert!(!baby.body.flags.picks_up_loot, "never at easy");
        assert!(
            baby.body
                .attributes
                .made(attribute(clustine_data::attributes::MOVEMENT_SPEED))
                .is_some_and(|instance| instance.has_modifier(family::BABY_SPEED))
        );
    }
}
