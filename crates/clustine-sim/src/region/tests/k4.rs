//! The skeletons, the creeper and the spiders in a region, from
//! `docs/groundwork/n7k-brief.md`, sections 11.2 to 11.5, and the plan's 1.3: a skeleton
//! burning in the sun walks into shade and keeps out of the sun, and runs from a wolf;
//! one in powder snow becomes a stray holding its bow; a bogged is sheared for two
//! mushrooms; a creeper lit with flint and steel explodes 30 ticks on, leaving nothing of
//! itself, and a restore in the middle of its swelling changes nothing; a spider climbs
//! out of a pit a skeleton cannot leave; an egg's spider is now and then ridden by a
//! skeleton; a cave spider's egg draws nothing but its yaw.

use clustine_data::{blocks, entity_types, items, sounds};
use clustine_world::{Entity, EntityChange, EntityData, EntityId};

use super::*;
use crate::living::MAX_AIR;
use crate::mobs::Mob;
use crate::mobs::types::{creeper, skeletons, spider};

const ONE: EntityId = EntityId(40);
const TWO: EntityId = EntityId(42);

/// Noon and midnight of the overworld clock.
const NOON: u64 = 6_000;
const MIDNIGHT: u64 = 18_000;

/// A mob of the type `kind` at `position`, standing, at its type's full health, with what
/// `prepare` does to it.
fn mob_of(key: EntityId, kind: i32, position: [f64; 3], prepare: impl FnOnce(&mut Mob)) -> Entity {
    let mut mob = Mob::fresh();
    let kind = kind as u16;
    mob.body.vitals.health = crate::mobs::body::attributes::supplied(kind, 23).unwrap() as f32;
    mob.body.vitals.air = MAX_AIR;
    prepare(&mut mob);
    Entity {
        key,
        kind,
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

/// A skeleton holding a bow, as an egg makes it.
fn skeleton_at(key: EntityId, position: [f64; 3], prepare: impl FnOnce(&mut Mob)) -> Entity {
    mob_of(key, entity_types::SKELETON, position, |mob| {
        mob.body.equipment.slots[0] = Some(one(items::BOW));
        prepare(mob);
    })
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

fn items_lying(region: &Region) -> Vec<(Entity, ItemStack)> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter_map(|entity| match entity.data {
            EntityData::Item { ref stack, .. } => Some((entity.clone(), stack.clone())),
            _ => None,
        })
        .collect()
}

fn one(item: i32) -> ItemStack {
    ItemStack::new(item, 1)
}

fn holding(region: &mut Region, stack: ItemStack) {
    checked(
        region,
        &moves(vec![set_slot(1, 0, Some(stack)), select(1, 0)]),
    );
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

fn idle(region: &mut Region) -> TickOutput {
    checked(region, &TickInputs::default())
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

fn explosions(output: &TickOutput) -> Vec<f32> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Explosion { radius, .. } => Some(*radius),
            _ => None,
        })
        .collect()
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    crate::physics::distance_sqr(a, b).sqrt()
}

/// [`on_ticking_floor_with`], the chunk at the origin roofed with persistent leaves two
/// blocks above the floor wherever `roofed(x, z)`: shade whose sky is 14, as under a
/// tree, and the open sky elsewhere (the region's light, ADR-0029).
fn on_roofed_floor_with(roofed: impl Fn(usize, usize) -> bool) -> Region {
    let leaves = BlockState::parse("minecraft:oak_leaves[persistent=true]").expect("leaves");
    let roof: Vec<(BlockPos, BlockState)> = (0..16)
        .flat_map(|x| (0..16).map(move |z| (x, z)))
        .filter(|(x, z)| roofed(*x, *z))
        .map(|(x, z)| (BlockPos::new(x as i32, -58, z as i32), leaves))
        .collect();
    on_ticking_floor_with(&roof)
}

/// The sky's light at `position`, as the region has it.
fn sky_at(region: &Region, position: [f64; 3]) -> u8 {
    let pos = crate::physics::containing(position);
    region.light_at(pos).expect("the chunk is lit").sky
}

/// The slot of `FleeSunGoal` among a skeleton's goals.
const FLEE_SUN: u16 = 1;

#[test]
fn a_skeleton_burning_in_the_sun_walks_into_the_shade_about_it() {
    // A sunlit square about the skeleton, under the open sky; shade under leaves all
    // round.
    let sunlit = |x: usize, z: usize| (6..=10).contains(&x) && (6..=10).contains(&z);
    let mut region = on_roofed_floor_with(|x, z| !sunlit(x, z));
    region.clock.day = NOON;
    let mut skeleton = skeleton_at(ONE, [8.5, -60.0, 8.5], |_| {});
    skeleton.fire_ticks = 160;
    lay(&mut region, &skeleton);
    let mut fled = false;
    let mut shaded = false;
    for _ in 0..200 {
        idle(&mut region);
        let now = mob(&region, ONE);
        fled |= now.mind.goals.0.running.contains(&FLEE_SUN);
        let at = record(&region, ONE).unwrap().position;
        let eyes = [at[0], at[1] + 1.74, at[2]];
        if fled && sky_at(&region, eyes) < 15 {
            shaded = true;
            break;
        }
    }
    assert!(fled, "`FleeSunGoal` ran");
    assert!(shaded, "it stands where the sky does not see it");
}

#[test]
fn a_skeleton_in_shade_by_day_keeps_its_strolls_out_of_the_sun() {
    // Shade under leaves but for a sunlit band along x 12 to 15.
    let sunlit = |x: usize| x >= 12;
    let mut region = on_roofed_floor_with(|x, _| !sunlit(x));
    region.clock.day = NOON;
    lay(&mut region, &skeleton_at(ONE, [9.5, -60.0, 8.5], |_| {}));
    let mut restricted = false;
    for _ in 0..600 {
        idle(&mut region);
        restricted |= skeletons::state_of(&mob(&region, ONE)).avoid_sun;
        let at = record(&region, ONE).unwrap().position;
        assert!(
            at[0] < 12.0 - 0.3 + 1.0e-9 || at[1] < -61.0,
            "it never stands in the sun: {at:?}"
        );
    }
    assert!(
        restricted,
        "`RestrictSunGoal` set its navigation to avoid the sun"
    );
}

#[test]
fn a_skeleton_runs_from_a_wolf_near_it() {
    let mut region = on_ticking_floor_with(&[]);
    region.clock.day = MIDNIGHT;
    lay(&mut region, &skeleton_at(ONE, [6.5, -60.0, 6.5], |_| {}));
    // A wolf that stands still (a wild one would hunt it, and keep up with it).
    lay(
        &mut region,
        &mob_of(TWO, entity_types::WOLF, [9.5, -60.0, 6.5], |mob| {
            mob.body.flags.no_ai = true;
        }),
    );
    let mut fled = false;
    for _ in 0..60 {
        idle(&mut region);
        let skeleton = record(&region, ONE).unwrap().position;
        let wolf = record(&region, TWO).unwrap().position;
        fled |= distance(skeleton, wolf) > 6.0;
    }
    assert!(fled);
}

#[test]
fn a_skeleton_in_powder_snow_becomes_a_stray_on_its_441st_tick_there_keeping_its_bow() {
    // A pit of powder snow one block wide, walled in stone two high.
    let mut placed = vec![
        (BlockPos::new(8, -60, 8), blocks::POWDER_SNOW),
        (BlockPos::new(8, -59, 8), blocks::POWDER_SNOW),
    ];
    for x in 7..=9 {
        for z in 7..=9 {
            if (x, z) != (8, 8) {
                for y in [-60, -59, -58] {
                    placed.push((BlockPos::new(x, y, z), blocks::STONE));
                }
            }
        }
    }
    let mut region = on_ticking_floor_with(&placed);
    region.clock.day = MIDNIGHT;
    // As its last tick left it: in the snow, never yet afflicted.
    lay(
        &mut region,
        &skeleton_at(ONE, [8.5, -60.0, 8.5], |mob| {
            skeletons::change(mob, |own| own.in_powder_snow = true);
            mob.body.equipment.slots[5] = Some(one(items::IRON_HELMET));
        }),
    );
    for tick in 1..=440 {
        let output = idle(&mut region);
        assert!(all_of(&region, entity_types::STRAY).is_empty(), "{tick}");
        let own = skeletons::state_of(&mob(&region, ONE));
        if tick < 140 {
            assert_eq!((own.affliction, own.conversion), (tick, None), "{tick}");
        } else {
            assert_eq!(own.conversion, Some(300 - (tick - 140)), "{tick}");
        }
        assert!(
            !output
                .events
                .iter()
                .any(|event| matches!(event, RegionEvent::LevelEvent { kind: 1048, .. })),
            "{tick}"
        );
    }
    let health = mob(&region, ONE).body.vitals.health;
    assert_eq!(
        health,
        crate::mobs::body::attributes::supplied(entity_types::SKELETON as u16, 23).unwrap() as f32,
        "powder snow does not hurt a skeleton"
    );
    let output = idle(&mut region);
    assert!(record(&region, ONE).is_none(), "the skeleton is gone");
    let [stray] = &all_of(&region, entity_types::STRAY)[..] else {
        panic!("one stray");
    };
    let made = Mob::decode(&stray.data).unwrap();
    assert_eq!(made.body.equipment.slots[0], Some(one(items::BOW)));
    assert_eq!(made.body.equipment.slots[5], Some(one(items::IRON_HELMET)));
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::LevelEvent {
            kind: 1048,
            position,
            ..
        } if *position == BlockPos::new(8, -60, 8)
    )));
}

#[test]
fn a_skeleton_that_leaves_the_snow_starts_its_count_again() {
    let mut region = on_ticking_floor_with(&[]);
    region.clock.day = MIDNIGHT;
    lay(
        &mut region,
        &skeleton_at(ONE, [8.5, -60.0, 8.5], |mob| {
            skeletons::change(mob, |own| {
                own.affliction = 139;
                own.conversion = Some(20);
            });
        }),
    );
    idle(&mut region);
    let own = skeletons::state_of(&mob(&region, ONE));
    assert_eq!((own.affliction, own.conversion), (-1, None));
}

#[test]
fn a_bogged_sheared_drops_two_mushrooms_at_the_top_of_its_box_and_is_sheared_once() {
    let mut region = on_ticking_floor_with(&[]);
    region.clock.day = MIDNIGHT;
    lay(
        &mut region,
        &mob_of(ONE, entity_types::BOGGED, [4.5, -60.0, 4.5], |mob| {
            mob.body.flags.no_ai = true;
        }),
    );
    holding(&mut region, one(items::SHEARS));
    let output = use_on(&mut region, ONE);
    assert!(sounds_of(&output).contains(&sounds::ENTITY_BOGGED_SHEAR));
    assert!(skeletons::state_of(&mob(&region, ONE)).sheared);
    let lying = items_lying(&region);
    assert_eq!(lying.len(), 2, "{lying:?}");
    for (entity, stack) in &lying {
        assert!(
            [items::BROWN_MUSHROOM, items::RED_MUSHROOM].contains(&stack.item),
            "{stack:?}"
        );
        assert_eq!(stack.count, 1);
        // Made at the top of its box, 1.99 above its feet, not a block above as a sheep's
        // wool (each has risen by its first move since).
        assert!(
            (entity.position[1] - (-60.0 + f64::from(1.99_f32))).abs() < 0.3,
            "{:?}",
            entity.position
        );
    }
    let output = use_on(&mut region, ONE);
    assert!(!sounds_of(&output).contains(&sounds::ENTITY_BOGGED_SHEAR));
    assert_eq!(
        items_lying(&region).len(),
        2,
        "a sheared bogged gives nothing more"
    );
}

/// The creeper lit with flint and steel, and the outputs of the ticks to its explosion,
/// the lighting's first.
fn light_a_creeper(region: &mut Region) -> TickOutput {
    lay(
        region,
        &mob_of(ONE, entity_types::CREEPER, [4.5, -60.0, 4.5], |mob| {
            mob.body.flags.no_ai = true;
        }),
    );
    holding(region, one(items::FLINT_AND_STEEL));
    use_on(region, ONE)
}

#[test]
fn a_creeper_lit_with_flint_and_steel_explodes_on_its_30th_tick_leaving_nothing_of_itself() {
    let mut region = on_ticking_floor_with(&[]);
    region.clock.day = MIDNIGHT;
    let lit = light_a_creeper(&mut region);
    assert!(sounds_of(&lit).contains(&sounds::ITEM_FLINTANDSTEEL_USE));
    assert!(creeper::state_of(&mob(&region, ONE)).ignited);
    // Its first swell is in the tick it is lit in, when it ticks after the use.
    let mut swells = vec![creeper::state_of(&mob(&region, ONE)).swell];
    let mut primed = sounds_of(&lit).contains(&sounds::ENTITY_CREEPER_PRIMED);
    let mut exploded_at = None;
    for tick in 2..=40 {
        let output = idle(&mut region);
        primed |= sounds_of(&output).contains(&sounds::ENTITY_CREEPER_PRIMED);
        let radii = explosions(&output);
        if !radii.is_empty() {
            assert_eq!(radii, [3.0], "a creeper's radius, unpowered");
            exploded_at = Some(tick);
            break;
        }
        swells.push(creeper::state_of(&mob(&region, ONE)).swell);
    }
    assert!(primed);
    assert_eq!(swells, (1..=29).collect::<Vec<_>>());
    assert_eq!(
        exploded_at,
        Some(30),
        "the 30th tick, the lighting's the first"
    );
    assert!(record(&region, ONE).is_none(), "the creeper is gone");
    assert!(
        items_lying(&region)
            .iter()
            .all(|(_, stack)| stack.item != items::GUNPOWDER),
        "no loot of its own"
    );
    // The floor it stood on is broken.
    assert!(
        region
            .block(BlockPos::new(4, -61, 4))
            .unwrap()
            .props()
            .is_air()
    );
}

#[test]
fn a_restore_in_the_middle_of_a_creeper_s_swelling_explodes_it_at_the_same_tick() {
    let mut original = on_ticking_floor_with(&[]);
    original.clock.day = MIDNIGHT;
    light_a_creeper(&mut original);
    for _ in 0..12 {
        idle(&mut original);
    }
    let mut copy = super::scheduled_ticks::restored(&original);
    assert_eq!(creeper::state_of(&mob(&copy, ONE)).swell, 13);
    for tick in 14..=30 {
        let a = idle(&mut original);
        let b = idle(&mut copy);
        // What clients are shown and what is kept; a restored region tells its players'
        // stays again, which is the restore's own.
        assert_eq!(a.events, b.events, "{tick}");
        assert_eq!(a.entity_changes, b.entity_changes, "{tick}");
        assert_eq!(!explosions(&a).is_empty(), tick == 30, "{tick}");
    }
}

#[test]
fn a_spider_climbs_out_of_a_pit_a_skeleton_cannot_leave() {
    // A pit three across and three deep, walled in stone.
    let mut placed = Vec::new();
    for x in 5..=9 {
        for z in 5..=9 {
            if (6..=8).contains(&x) && (6..=8).contains(&z) {
                continue;
            }
            for y in -60..=-58 {
                placed.push((BlockPos::new(x, y, z), blocks::STONE));
            }
        }
    }
    let out = |at: [f64; 3]| at[1] >= -57.0;
    let mut climbed = None;
    for kind in [entity_types::SPIDER, entity_types::SKELETON] {
        let mut region = on_ticking_floor_with(&placed);
        region.clock.day = MIDNIGHT;
        lay(&mut region, &mob_of(ONE, kind, [7.5, -60.0, 7.5], |_| {}));
        let mut left = false;
        let mut flagged = false;
        for _ in 0..1200 {
            idle(&mut region);
            left |= out(record(&region, ONE).unwrap().position);
            if kind == entity_types::SPIDER {
                flagged |= spider::state_of(&mob(&region, ONE)).climbing;
            }
            if left {
                break;
            }
        }
        if kind == entity_types::SPIDER {
            assert!(flagged, "it climbed");
            climbed = Some(left);
        } else {
            assert!(!left, "a skeleton stays in the pit");
        }
    }
    assert_eq!(climbed, Some(true), "the spider is out");
}

#[test]
fn an_egg_s_spider_is_now_and_then_ridden_by_a_skeleton_with_its_bow() {
    let mut region = on_ticking_floor_with(&[]);
    let mut found = None;
    for _ in 0..2000 {
        let mut output = TickOutput::default();
        let key = region
            .spawn_mob(
                entity_types::SPIDER as u16,
                player(1),
                BlockPos::new(8, -60, 8),
                true,
                true,
                &mut output,
            )
            .expect("made");
        let spider = record(&region, key).unwrap();
        if let Some(seat) = spider.riding.passengers.first() {
            found = Some((key, seat.key));
            break;
        }
        let hops = spider.hops;
        let chunk = spider.chunk();
        region.remove_entity(key, hops, chunk, &mut output);
    }
    let (spider, rider) = found.expect("one spider in a hundred is a jockey");
    let skeleton = record(&region, rider).unwrap();
    assert_eq!(i32::from(skeleton.kind), entity_types::SKELETON);
    assert_eq!(skeleton.riding.vehicle, Some(spider));
    assert_eq!(
        Mob::decode(&skeleton.data).unwrap().body.equipment.slots[0],
        Some(one(items::BOW))
    );
    // Both are seen: the spider, then its rider.
    idle(&mut region);
    assert!(record(&region, spider).is_some() && record(&region, rider).is_some());
}

#[test]
fn a_cave_spider_s_egg_draws_nothing_of_the_level_s_random_but_its_yaw() {
    let mut region = on_ticking_floor_with(&[]);
    let player_key = region.players[&player(1)].entity_id;
    let mut expected = region.entity_stream(player_key).random;
    let mut output = TickOutput::default();
    let key = region
        .spawn_mob(
            entity_types::CAVE_SPIDER as u16,
            player(1),
            BlockPos::new(3, -60, 3),
            true,
            true,
            &mut output,
        )
        .expect("made");
    let yaw = crate::mobs::ai::maths::wrap_degrees(expected.next_float() * 360.0);
    assert_eq!(region.entity_stream(player_key).random, expected);
    assert_eq!(record(&region, key).unwrap().yaw, yaw);
    let made = mob(&region, key);
    assert!(!made.body.flags.left_handed);
    assert!(
        made.body
            .attributes
            .made(clustine_data::attributes::FOLLOW_RANGE as u16)
            .is_none_or(|instance| instance.modifiers.is_empty())
    );
}

#[test]
fn a_spider_s_egg_draws_its_yaw_the_tail_and_the_jockey_s_hundred() {
    let mut region = on_ticking_floor_with(&[]);
    let player_key = region.players[&player(1)].entity_id;
    let mut expected = region.entity_stream(player_key).random;
    let mut output = TickOutput::default();
    let key = region
        .spawn_mob(
            entity_types::SPIDER as u16,
            player(1),
            BlockPos::new(3, -60, 3),
            true,
            true,
            &mut output,
        )
        .expect("made");
    let _yaw = expected.next_float();
    let _triangle = (expected.next_double(), expected.next_double());
    let left = expected.next_float() < 0.05;
    let jockey = expected.next_int(100) == 0;
    assert_eq!(region.entity_stream(player_key).random, expected);
    let spider = record(&region, key).unwrap();
    assert_eq!(mob(&region, key).body.flags.left_handed, left);
    assert_eq!(spider.riding.passengers.is_empty(), !jockey);
}
