//! Monster spawners in a region (`region/spawners.rs`), from `BaseSpawner.serverTick`
//! and `delay` (n7 BaseSpawner 223–619): a spawner near a player counts its delay down
//! and then spawns its mob about it and draws its delay again, telling its clients with
//! a block event; with no player near it does nothing; it spawns no more than its cap
//! of nearby mobs; a zombie's spawner needs the dark, a blaze's does not.
//!
//! The regions are those of the tests of R3a (`redstone.rs`): player 1 at the spawn,
//! (0.5, -60, 0.5), on a stone floor at y = -61, with the chunks around chunk (0, 0)
//! loaded so that they tick.

use clustine_data::entity_types;

use super::redstone::ticking_with;
use super::*;
use crate::region::spawners::{EVENT_SPAWN, LEVEL_EVENT_SPAWN, Spawner};

/// A closed room of stone whose corner is at `(x0, z0)`: walls of 11 by 11 blocks and a
/// roof at y = -57, so that its inside (from `x0 + 1` to `x0 + 9`, and three blocks
/// high) has no light at all, with the spawner at its middle on the floor.
fn room(x0: i32, z0: i32) -> Vec<(BlockPos, BlockState)> {
    let mut placed = Vec::new();
    for x in x0..=x0 + 10 {
        for z in z0..=z0 + 10 {
            for y in -60..=-57 {
                let wall = x == x0 || x == x0 + 10 || z == z0 || z == z0 + 10;
                if wall || y == -57 {
                    placed.push((BlockPos::new(x, y, z), blocks::STONE));
                }
            }
        }
    }
    placed.push((BlockPos::new(x0 + 5, -60, z0 + 5), blocks::SPAWNER));
    placed
}

/// The region with `blocks` and the spawner at `at` set as `spawner` has it.
fn with_spawner(blocks: &[(BlockPos, BlockState)], at: BlockPos, spawner: &Spawner) -> Region {
    let mut region = ticking_with(blocks);
    assert!(region.set_block_entity_data(at, spawner.write(), &mut TickOutput::default()));
    region
}

fn spawner_of(region: &Region, at: BlockPos) -> Spawner {
    Spawner::read(
        region
            .own_block_entity_data(at)
            .expect("a spawner's block entity"),
    )
}

/// The entities of the type `kind` in the region's chunks.
fn count(region: &Region, kind: i32) -> usize {
    (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .filter_map(|position| region.chunk(position))
        .flat_map(|chunk| chunk.entities().iter())
        .filter(|entity| i32::from(entity.kind) == kind)
        .count()
}

fn level_events(output: &TickOutput, at: BlockPos) -> usize {
    output
        .events
        .iter()
        .filter(|event| {
            matches!(event, RegionEvent::LevelEvent { kind, position, data: 0 }
                if *kind == LEVEL_EVENT_SPAWN && *position == at)
        })
        .count()
}

fn spawn_block_event(output: &TickOutput, at: BlockPos) -> bool {
    output.events.iter().any(|event| {
        matches!(event, RegionEvent::BlockEvent { position, kind, data: 0, .. }
            if *kind == EVENT_SPAWN && *position == at)
    })
}

#[test]
fn a_spawner_beside_a_player_spawns_its_mob_once_its_delay_is_out() {
    let at = BlockPos::new(7, -60, 7);
    let spawner = Spawner::read(&[]).with_type("minecraft:zombie");
    let mut region = with_spawner(&room(2, 2), at, &spawner);
    // The delay of 20 is counted down, a tick at a time, and nothing comes meanwhile.
    for left in (0..20).rev() {
        let output = idle(&mut region);
        assert_eq!(spawner_of(&region, at).delay, left);
        assert_eq!(count(&region, entity_types::ZOMBIE), 0);
        assert_eq!(level_events(&output, at), 0);
    }
    // Then it tries every tick until a zombie comes, each shown with the smoke.
    let mut tries = 0;
    let (spawned, output) = loop {
        tries += 1;
        assert!(tries < 50, "no zombie came");
        let output = idle(&mut region);
        let spawned = count(&region, entity_types::ZOMBIE);
        if spawned > 0 {
            break (spawned, output);
        }
        assert_eq!(spawner_of(&region, at).delay, 0);
    };
    assert!(spawned <= 4, "no more than its count of tries");
    assert_eq!(level_events(&output, at), spawned);
    // Each in the room, made with its poof, which the clients are shown.
    let poofs = output
        .events
        .iter()
        .filter(|event| matches!(event, RegionEvent::EntityEvent { event: 20, .. }))
        .count();
    assert_eq!(poofs, spawned);
    // Its delay drawn again between 200 and 800, and the block event that starts the
    // clients' over runs in the next tick's block events.
    let delay = spawner_of(&region, at).delay;
    assert!((200..800).contains(&delay), "{delay}");
    assert!(!spawn_block_event(&output, at));
    let output = idle(&mut region);
    assert!(spawn_block_event(&output, at));
    assert_eq!(spawner_of(&region, at).delay, delay - 1);
}

#[test]
fn a_spawner_with_no_player_within_its_range_does_nothing() {
    // The room's spawner at (27, -60, 27), 37 blocks from the player.
    let at = BlockPos::new(27, -60, 27);
    let spawner = Spawner::read(&[]).with_type("minecraft:zombie");
    let mut region = with_spawner(&room(22, 22), at, &spawner);
    let written = region.own_block_entity_data(at).unwrap().to_vec();
    for _ in 0..100 {
        let output = idle(&mut region);
        assert_eq!(level_events(&output, at), 0);
    }
    assert_eq!(count(&region, entity_types::ZOMBIE), 0);
    // Not a byte of it changed: its delay waits for a player.
    assert_eq!(region.own_block_entity_data(at).unwrap(), &written[..]);
    // A range below 0 is any distance.
    let mut far = spawner;
    far.required_player_range = -1;
    let mut region = with_spawner(&room(22, 22), at, &far);
    for _ in 0..60 {
        idle(&mut region);
    }
    assert!(count(&region, entity_types::ZOMBIE) > 0);
}

#[test]
fn a_spawner_spawns_no_more_than_its_cap_of_nearby_mobs() {
    let at = BlockPos::new(7, -60, 7);
    let mut spawner = Spawner::read(&[]).with_type("minecraft:zombie");
    spawner.delay = 0;
    spawner.min_spawn_delay = 0;
    spawner.max_spawn_delay = 0;
    spawner.max_nearby_entities = 3;
    let mut region = with_spawner(&room(2, 2), at, &spawner);
    let mut most = 0;
    for _ in 0..200 {
        idle(&mut region);
        most = most.max(count(&region, entity_types::ZOMBIE));
    }
    // The room holds them within the box it counts, the spawner's block grown by its
    // range of 4.
    assert_eq!(most, 3);
    assert_eq!(count(&region, entity_types::ZOMBIE), 3);
}

#[test]
fn a_zombie_spawner_in_the_light_spawns_nothing_and_a_blaze_spawner_does() {
    // The room lit by a glowstone in its west wall: no place in it is without light.
    let at = BlockPos::new(7, -60, 7);
    let mut lit = room(2, 2);
    lit.push((BlockPos::new(2, -59, 7), blocks::GLOWSTONE));
    // The monsters' darkness is asked of a spawner's mob too: the blocks' light above
    // the overworld's limit of 0 refuses a zombie.
    let mut spawner = Spawner::read(&[]).with_type("minecraft:zombie");
    spawner.delay = 0;
    let mut region = with_spawner(&lit, at, &spawner);
    for _ in 0..100 {
        idle(&mut region);
    }
    assert_eq!(count(&region, entity_types::ZOMBIE), 0);
    // A blaze's rule asks no light (`checkAnyLightMonsterSpawnRules`), and a spawner's
    // reason asks nothing of the block below; only its walk target, light up to 12.
    let mut spawner = Spawner::read(&[]).with_type("minecraft:blaze");
    spawner.delay = 0;
    let mut region = with_spawner(&lit, at, &spawner);
    for _ in 0..60 {
        idle(&mut region);
    }
    assert!(count(&region, entity_types::BLAZE) > 0);
}

#[test]
fn a_zombie_spawner_in_the_open_by_day_spawns_nothing() {
    let at = BlockPos::new(7, -60, 7);
    let mut spawner = Spawner::read(&[]).with_type("minecraft:zombie");
    spawner.delay = 0;
    let mut region = with_spawner(&[(at, blocks::SPAWNER)], at, &spawner);
    for _ in 0..100 {
        idle(&mut region);
    }
    assert_eq!(count(&region, entity_types::ZOMBIE), 0);
    // It tries every tick, and draws no delay while nothing comes.
    assert_eq!(spawner_of(&region, at).delay, 0);
}

#[test]
fn a_spawner_s_delay_is_in_its_data_and_is_not_shown() {
    let at = BlockPos::new(7, -60, 7);
    let spawner = Spawner::read(&[]).with_type("minecraft:zombie");
    let mut region = with_spawner(&room(2, 2), at, &spawner);
    let output = idle(&mut region);
    let changed: Vec<_> = output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::BlockEntityChanged {
                position,
                data,
                seen,
            } if *position == at => Some((Spawner::read(data).delay, *seen)),
            _ => None,
        })
        .collect();
    assert_eq!(changed, vec![(19, false)]);
}

#[test]
fn a_spawner_whose_mob_is_no_type_draws_its_delay_again() {
    // A spawner just placed has nothing to spawn (`new SpawnData()`): its delay out, it
    // makes its empty next spawn, which its clients are sent, and draws its delay.
    let at = BlockPos::new(7, -60, 7);
    let mut spawner = Spawner::read(&[]);
    spawner.delay = 0;
    let mut region = with_spawner(&room(2, 2), at, &spawner);
    let output = idle(&mut region);
    let kept = spawner_of(&region, at);
    assert!((200..800).contains(&kept.delay), "{}", kept.delay);
    assert!(kept.next.is_some());
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::BlockEntityChanged { position, seen: true, .. } if *position == at
    )));
    assert_eq!(count(&region, entity_types::ZOMBIE), 0);
}

#[test]
fn an_egg_on_a_spawner_just_placed_gives_it_the_mob_and_its_clients_are_told() {
    use super::redstone::{click, holding};
    // A spawner just placed has a block entity with no data (its first tick near the
    // player wrote some, which is taken away again here).
    let at = BlockPos::new(2, -60, 2);
    let mut region = ticking_with(&[(at, blocks::SPAWNER)]);
    checked(
        &mut region,
        &holding(clustine_data::items::ZOMBIE_SPAWN_EGG),
    );
    assert!(region.set_block_entity_data(at, Vec::new(), &mut TickOutput::default()));
    // The click comes before the block entities' phase.
    let output = checked(&mut region, &click(at));
    // What a dungeon's spawner of zombies saves: a new spawner given the type.
    let dungeon =
        clustine_features::features::ground::monster_room::spawner_data("minecraft:zombie");
    let told: Vec<_> = output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::BlockEntityChanged {
                position,
                data,
                seen,
            } if *position == at => Some((data.clone(), *seen)),
            _ => None,
        })
        .collect();
    assert_eq!(told.first(), Some(&(dungeon.data, true)));
    let next = spawner_of(&region, at).next.expect("a next spawn");
    assert_eq!(next.kind(), Some(entity_types::ZOMBIE as u16));
}
