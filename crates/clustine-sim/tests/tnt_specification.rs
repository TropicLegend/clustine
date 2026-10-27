//! Tests of TNT and explosions in step N6, written from `docs/groundwork/n6-plan.md`
//! (section 1; decision 10; section 6 item 9; section 7 item 11), the porting brief
//! `docs/groundwork/n6-brief.md` (sections 1, 2, 9.1 and 11.1) and the public API alone,
//! by someone who did not write the code: TNT primed by flint and steel, by redstone, by
//! fire and by another explosion; the fuse; the reach of an explosion in stone, and
//! obsidian that survives it; drops merged into stacks of at most 16; the entities it
//! hits and what the owner decides of their drops; the explosion's report; and its
//! effects across a pinned line and the holder's side of them.
//!
//! Player 1 stands on a stone floor at y = -61 (feet at y = -60) in chunk (0, 0), whose
//! 5x5 is loaded. Every player of Clustine is a creative one.

mod n6_scene;

use clustine_data::{BlockState, blocks, entity_types, items, sounds};
use clustine_sim::TickOutput;
use clustine_sim::api::{Face, Inventory, RegionEvent};
use clustine_world::{
    Blast, BlockPos, Double, Effect, Effected, Entity, EntityData, EntityId, EntityUse, Float,
    Hurt, InChunk, ScheduledTick, Ticked, Vec3,
};
use n6_scene::*;
use uuid::Uuid;

/// Where the TNT of most of these tests stands, with its centre a block above the floor.
const TNT: BlockPos = BlockPos::new(8, -60, 8);
const FAR: Vec3 = Vec3::new(0.5, -60.0, 0.5);
/// The fuse of TNT set into a world: long enough to outlast the ticks that load it and
/// let player 1 in.
const FUSE: i16 = 12;

fn entity(n: u32, kind: i32, position: [f64; 3], data: EntityData) -> Entity {
    Entity {
        key: key(n),
        kind: u16::try_from(kind).expect("fits"),
        uuid: Uuid::from_u128(u128::from(n) + 7_000),
        position,
        motion: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
        hops: 0,
        lived: 1,
        fresh_at: 0,
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
        data,
    }
}

/// Primed TNT at the bottom centre of `at` with `fuse` ticks left, primed by `owner`.
fn primed(n: u32, at: BlockPos, fuse: i16, owner: Option<Uuid>) -> Entity {
    entity(
        n,
        entity_types::TNT,
        [
            f64::from(at.x) + 0.5,
            f64::from(at.y),
            f64::from(at.z) + 0.5,
        ],
        EntityData::PrimedTnt {
            fuse,
            block: blocks::TNT.0,
            power: Float(4.0),
            owner,
        },
    )
}

fn lying_dirt(n: u32, position: [f64; 3]) -> Entity {
    entity(
        n,
        entity_types::ITEM,
        position,
        EntityData::Item {
            stack: stack(items::DIRT, 1),
            pick_up_delay: 32_767,
            thrower: None,
            health: 5,
            age_offset: 0,
        },
    )
}

fn boat(n: u32, position: [f64; 3]) -> Entity {
    entity(
        n,
        entity_types::OAK_BOAT,
        position,
        EntityData::Boat {
            hurt: Hurt::default(),
            paddles: [false; 2],
            bubble_time: 0,
            out_of_control: Float(0.0),
            turning: Float(0.0),
            status: [3, 3],
            contents: Vec::new(),
        },
    )
}

fn stand(n: u32, position: [f64; 3]) -> Entity {
    let pose = [
        [0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0],
        [-10.0, 0.0, -10.0],
        [-15.0, 0.0, 10.0],
        [-1.0, 0.0, -1.0],
        [1.0, 0.0, 1.0],
    ];
    entity(
        n,
        entity_types::ARMOR_STAND,
        position,
        EntityData::ArmorStand {
            flags: 0,
            disabled_slots: 0,
            pose: Box::new(pose.map(|part: [f32; 3]| part.map(Float))),
            equipment: Vec::new(),
            health: Float(20.0),
            last_hit: None,
            air: 300,
            frozen: 0,
        },
    )
}

fn tnt_data(entity: &Entity) -> (i16, u16, f32, Option<Uuid>) {
    match &entity.data {
        EntityData::PrimedTnt {
            fuse,
            block,
            power,
            owner,
        } => (*fuse, *block, power.0, *owner),
        other => panic!("not primed TNT: {other:?}"),
    }
}

/// The explosions the tick reports, taken apart.
#[derive(Debug, Clone)]
struct Exploded {
    centre: [f64; 3],
    radius: f32,
    blocks: i32,
    knockback: Vec<(EntityId, [f64; 3])>,
    large: bool,
    sound: bool,
}

fn explosions(output: &TickOutput) -> Vec<Exploded> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Explosion {
                centre,
                radius,
                blocks,
                knockback,
                large,
                sound,
            } => Some(Exploded {
                centre: *centre,
                radius: *radius,
                blocks: *blocks,
                knockback: knockback.clone(),
                large: *large,
                sound: *sound,
            }),
            _ => None,
        })
        .collect()
}

/// Idles until an explosion is reported, for at most `ticks` ticks: its tick, counted
/// from 1 for the first idled, and its output.
fn until_it_explodes(scene: &mut Scene, ticks: u32) -> (u32, TickOutput) {
    for tick in 1..=ticks {
        let output = scene.idle();
        if !explosions(&output).is_empty() {
            return (tick, output);
        }
    }
    panic!("nothing exploded in {ticks} ticks");
}

/// A cube of `block` from `min` to `max`, with `hollow` left as air.
fn cube(
    block: BlockState,
    min: BlockPos,
    max: BlockPos,
    hollow: &[BlockPos],
) -> Vec<(BlockPos, BlockState)> {
    let mut placed = Vec::new();
    for x in min.x..=max.x {
        for y in min.y..=max.y {
            for z in min.z..=max.z {
                let at = BlockPos::new(x, y, z);
                if !hollow.contains(&at) {
                    placed.push((at, block));
                }
            }
        }
    }
    placed
}

fn chebyshev(a: BlockPos, b: BlockPos) -> i32 {
    (a.x - b.x)
        .abs()
        .max((a.y - b.y).abs())
        .max((a.z - b.z).abs())
}

fn flint() -> Inventory {
    inventory(&[
        (0, stack(items::FLINT_AND_STEEL, 1)),
        (1, stack(items::REDSTONE_BLOCK, 64)),
    ])
}

// ------------------------------------------------------------------------------------
// Priming (brief 1.0 items 1, 2 and 7; 1.1 to 1.3).

/// Brief 1.1 and 1.3: flint and steel used on TNT makes primed TNT at the block's
/// bottom centre, with the player as its owner, the TNT's block and power 4, a fuse of
/// 80, a motion of 0.02 sideways and 0.2 up, and `entity.tnt.primed`; the block goes, and
/// a creative player keeps the flint and steel.
#[test]
fn flint_and_steel_primes_tnt_owned_by_the_player() {
    let at = BlockPos::new(4, -60, 4);
    let mut scene = Scene::alone(
        &World::of(&[(at, blocks::TNT)]),
        flint(),
        Vec3::new(2.5, -60.0, 2.5),
    );
    let output = scene.click(at, Face::Top);
    assert_eq!(scene.block(at), blocks::AIR);
    let tnt = scene.of_kind(entity_types::TNT);
    assert_eq!(tnt.len(), 1, "one primed TNT");
    let (fuse, block, power, owner) = tnt_data(&tnt[0]);
    assert_eq!(owner, Some(player(1).0));
    assert_eq!(block, blocks::TNT.0);
    assert_eq!(power, 4.0);
    // 80 at priming; the tick that primed it may have ticked it once.
    assert!(fuse == 80 || fuse == 79, "{fuse}");
    let [x, _, z] = tnt[0].position;
    assert!(
        (x - 4.5).abs() < 0.05 && (z - 4.5).abs() < 0.05,
        "{:?}",
        tnt[0].position
    );
    let [mx, my, mz] = tnt[0].motion;
    let sideways = (mx * mx + mz * mz).sqrt();
    if fuse == 80 {
        assert!(close(sideways, 0.02), "{sideways}");
        assert!(close(my, 0.200_000_002_980_232_24), "{my}");
    } else {
        // One tick: gravity 0.04 and then the drag of 0.98.
        assert!((sideways - 0.0196).abs() < 1e-6, "{sideways}");
        assert!((my - 0.1568).abs() < 1e-6, "{my}");
    }
    assert_eq!(sounds(&output, sounds::ENTITY_TNT_PRIMED).len(), 1);
    assert_eq!(scene.holds(0), Some(stack(items::FLINT_AND_STEEL, 1)));
}

/// Brief 1.1: TNT is primed with no owner by a neighbour's change that powers it.
#[test]
fn a_redstone_block_placed_beside_tnt_primes_it_with_no_owner() {
    let at = BlockPos::new(4, -60, 4);
    let mut scene = Scene::alone(
        &World::of(&[(at, blocks::TNT)]),
        flint(),
        Vec3::new(2.5, -60.0, 2.5),
    );
    scene.select(1);
    scene.click(BlockPos::new(4, -61, 3), Face::Top);
    assert_eq!(
        scene.block(BlockPos::new(4, -60, 3)),
        blocks::REDSTONE_BLOCK
    );
    assert_eq!(scene.block(at), blocks::AIR);
    let tnt = scene.of_kind(entity_types::TNT);
    assert_eq!(tnt.len(), 1);
    assert_eq!(tnt_data(&tnt[0]).3, None);
}

/// Brief 1.0 item 1 and 1.1: fire beside TNT burns it (`checkBurnOut`) and primes it with
/// no owner. The fire stands on netherrack, which keeps it burning, and is ticked from a
/// tick scheduled in its chunk, as placing it would have scheduled one.
#[test]
fn fire_beside_tnt_burns_it_and_primes_it_with_no_owner() {
    let fire = BlockPos::new(6, -60, 5);
    let at = BlockPos::new(7, -60, 5);
    let mut world = World::of(&[
        (fire.offset(0, -1, 0), blocks::NETHERRACK),
        (fire, blocks::FIRE),
        (at, blocks::TNT),
    ]);
    world.ticks.push((
        fire,
        ScheduledTick {
            at: InChunk::new(6, -60, 5),
            of: Ticked::Block(block_id(blocks::FIRE)),
            delay: 1,
            priority: 0,
        },
    ));
    let mut scene = Scene::alone(&world, flint(), FAR);
    let mut seen = Vec::new();
    assert!(
        scene.idle_until(3_000, &mut seen, |scene, _| !scene
            .of_kind(entity_types::TNT)
            .is_empty()),
        "the fire burnt the TNT"
    );
    let tnt = scene.of_kind(entity_types::TNT);
    assert_eq!(tnt_data(&tnt[0]).3, None);
    assert_ne!(scene.block(at), blocks::TNT, "burnt to fire or air");
}

/// Brief 1.4: the fuse goes down by one a tick and the TNT explodes in the tick it
/// reaches 0, radius 4, at its position raised by 0.98 x 0.0625; the TNT is gone.
#[test]
fn primed_tnt_explodes_when_its_fuse_runs_out() {
    let at = BlockPos::new(4, -60, 4);
    let mut scene = Scene::alone(
        &World::of(&[(at, blocks::TNT)]),
        flint(),
        Vec3::new(2.5, -60.0, 2.5),
    );
    scene.click(at, Face::Top);
    let fuse = tnt_data(&scene.of_kind(entity_types::TNT)[0]).0;
    let (tick, output) = until_it_explodes(&mut scene, 100);
    assert_eq!(
        tick,
        u32::try_from(fuse).expect("positive"),
        "a fuse of {fuse} left"
    );
    let explosion = &explosions(&output)[0];
    assert_eq!(explosion.radius, 4.0);
    assert!(explosion.large && explosion.sound);
    assert!(explosion.blocks > 0);
    let [x, y, z] = explosion.centre;
    assert!(
        (x - 4.5).abs() < 0.5 && (z - 4.5).abs() < 0.5,
        "{:?}",
        explosion.centre
    );
    assert!((y - (-60.0 + 0.061_250_000_745_058_06)).abs() < 1e-6, "{y}");
    assert!(scene.of_kind(entity_types::TNT).is_empty(), "gone");
}

// ------------------------------------------------------------------------------------
// What it breaks (brief 2.0 items 1 to 5, 2.2 and 2.6).

/// Brief 2.2's note: a ray loses `(6 + 0.3) * 0.3` at every point in stone, so TNT
/// buried in stone breaks the blocks touching its own and none three blocks away.
#[test]
fn tnt_buried_in_stone_breaks_its_neighbours_and_nothing_three_blocks_away() {
    let mut placed = cube(
        blocks::STONE,
        BlockPos::new(2, -60, 2),
        BlockPos::new(14, -54, 14),
        &[TNT],
    );
    placed.retain(|(at, _)| *at != TNT);
    let world = World::of(&placed).with_entities(&[primed(1, TNT, FUSE, None)]);
    let mut scene = Scene::alone(&world, flint(), FAR);
    until_it_explodes(&mut scene, 20);
    for neighbour in [
        TNT.offset(1, 0, 0),
        TNT.offset(-1, 0, 0),
        TNT.offset(0, 0, 1),
        TNT.offset(0, 0, -1),
        TNT.offset(0, 1, 0),
        TNT.offset(0, -1, 0),
    ] {
        assert_eq!(scene.block(neighbour), blocks::AIR, "{neighbour:?} broken");
    }
    for (at, _) in &placed {
        if chebyshev(*at, TNT) >= 3 {
            assert_eq!(scene.block(*at), blocks::STONE, "{at:?} is out of reach");
        }
    }
    for x in 0..16 {
        for z in 0..16 {
            let at = BlockPos::new(x, -61, z);
            if chebyshev(at, TNT) >= 3 {
                assert_eq!(scene.block(at), blocks::STONE, "the floor at {at:?}");
            }
        }
    }
}

/// Brief 2.2: obsidian resists 1200, which ends a ray at once.
#[test]
fn tnt_in_a_box_of_obsidian_breaks_nothing() {
    let placed = cube(
        blocks::OBSIDIAN,
        BlockPos::new(7, -61, 7),
        BlockPos::new(9, -59, 9),
        &[TNT],
    );
    let world = World::of(&placed).with_entities(&[primed(1, TNT, FUSE, None)]);
    let mut scene = Scene::alone(&world, flint(), FAR);
    let (_, output) = until_it_explodes(&mut scene, 20);
    for (at, _) in &placed {
        assert_eq!(scene.block(*at), blocks::OBSIDIAN, "{at:?} survives");
    }
    // The TNT's own place, air, is reached and counted (brief 2.0 item 3).
    assert!(explosions(&output)[0].blocks >= 1);
    assert!(
        items_lying(&scene.all()).is_empty(),
        "nothing broke, nothing dropped"
    );
}

/// Brief 2.0 items 4 and 5: TNT drops every block it breaks, merged into stacks of at
/// most 16 before they are laid: a collector fills before the next begins, so all the
/// dirt but one stack lies in stacks of 16.
#[test]
fn the_dirt_tnt_breaks_drops_whole_in_stacks_of_at_most_sixteen() {
    let mut placed = cube(
        blocks::DIRT,
        BlockPos::new(4, -60, 4),
        BlockPos::new(12, -59, 12),
        &[TNT],
    );
    placed.retain(|(at, _)| *at != TNT);
    let world = World::of(&placed).with_entities(&[primed(1, TNT, FUSE, None)]);
    let mut scene = Scene::alone(&world, flint(), FAR);
    until_it_explodes(&mut scene, 20);
    let broken = placed
        .iter()
        .filter(|(at, _)| scene.block(*at) == blocks::AIR)
        .count();
    assert!(broken > 20, "{broken} dirt broken");
    let dirt: Vec<i32> = items_lying(&scene.all())
        .iter()
        .filter(|(stack, _)| stack.item == items::DIRT)
        .map(|(stack, _)| stack.count)
        .collect();
    assert_eq!(
        dirt.iter().sum::<i32>(),
        i32::try_from(broken).expect("few"),
        "every broken dirt drops: {dirt:?}"
    );
    assert!(dirt.iter().all(|count| *count <= 16), "{dirt:?}");
    assert!(
        dirt.iter().filter(|count| **count < 16).count() <= 1,
        "the collectors fill one by one: {dirt:?}"
    );
}

/// Brief 2.0 item 4: the loot of a TNT's explosion has the TNT as `this_entity`, so a
/// snow layer drops its snowball, which a rule's break does not give.
#[test]
fn a_snow_layer_broken_by_tnt_drops_a_snowball() {
    let snow = TNT.offset(2, 0, 0);
    let world = World::of(&[(snow, blocks::SNOW)]).with_entities(&[primed(1, TNT, FUSE, None)]);
    let mut scene = Scene::alone(&world, flint(), FAR);
    until_it_explodes(&mut scene, 20);
    assert_eq!(scene.block(snow), blocks::AIR);
    assert!(lying_count(&scene.all(), items::SNOWBALL) >= 1);
}

/// Brief 1.2: TNT broken by an explosion is primed again with a short fuse of 10 to 29
/// ticks, owned by the first TNT's owner, and is not dropped.
#[test]
fn tnt_in_reach_of_an_explosion_is_primed_with_a_short_fuse_and_the_first_owner() {
    let other = TNT.offset(2, 0, 0);
    let owner = player(1).0;
    let world =
        World::of(&[(other, blocks::TNT)]).with_entities(&[primed(1, TNT, FUSE, Some(owner))]);
    let mut scene = Scene::alone(&world, flint(), FAR);
    until_it_explodes(&mut scene, 20);
    assert_eq!(scene.block(other), blocks::AIR);
    let chained = scene.of_kind(entity_types::TNT);
    assert_eq!(chained.len(), 1, "{chained:?}");
    let (fuse, _, _, chained_owner) = tnt_data(&chained[0]);
    assert!((9..=29).contains(&fuse), "a short fuse: {fuse}");
    assert_eq!(chained_owner, Some(owner));
    assert_eq!(lying_count(&scene.all(), items::TNT), 0);
    let (tick, _) = until_it_explodes(&mut scene, 31);
    assert_eq!(tick, u32::try_from(fuse).expect("positive"));
}

// ------------------------------------------------------------------------------------
// Entities (brief 2.0 items 6 to 9, 2.5).

/// Brief 2.5's table: TNT kills an armour stand, destroys an item lying near it and
/// breaks a boat, which drops its item when the TNT has no owner; a creative player near
/// it who is not flying is knocked back, which the report names, and is not hurt.
#[test]
fn tnt_with_no_owner_breaks_what_is_near_and_a_boat_drops_its_item() {
    let world = World::default().with_entities(&[
        primed(1, TNT, FUSE, None),
        stand(2, [10.5, -60.0, 8.5]),
        lying_dirt(3, [8.5, -60.0, 10.5]),
        boat(4, [6.0, -60.0, 8.5]),
    ]);
    let near = Vec3::new(8.5, -60.0, 5.5);
    let mut scene = Scene::alone(&world, flint(), near);
    let (_, output) = until_it_explodes(&mut scene, 20);
    let all = scene.all();
    for gone in [key(2), key(3), key(4)] {
        assert!(
            all.iter().all(|entity| entity.key != gone),
            "{gone:?} is gone"
        );
    }
    assert_eq!(
        lying_count(&all, items::DIRT),
        0,
        "the lying item was destroyed"
    );
    assert_eq!(lying_count(&all, items::OAK_BOAT), 1, "the boat's item");
    assert_eq!(
        lying_count(&all, items::ARMOR_STAND),
        0,
        "a stand drops only its equipment"
    );
    let explosion = &explosions(&output)[0];
    let pushed: Vec<_> = explosion
        .knockback
        .iter()
        .filter(|(entity, _)| *entity == scene.me())
        .collect();
    assert_eq!(pushed.len(), 1, "{:?}", explosion.knockback);
    assert!(
        pushed[0].1[2] < 0.0,
        "pushed away, to the north: {:?}",
        pushed[0].1
    );
    assert!(
        scene.region.player(player(1)).is_some(),
        "not hurt to death"
    );
}

/// Brief 2.0 item 7: a creative player's TNT discards a vehicle it breaks without its
/// item.
#[test]
fn a_creative_players_tnt_breaks_a_boat_without_its_item() {
    let world = World::default().with_entities(&[
        primed(1, TNT, FUSE, Some(player(1).0)),
        boat(4, [6.0, -60.0, 8.5]),
    ]);
    let mut scene = Scene::alone(&world, flint(), FAR);
    until_it_explodes(&mut scene, 20);
    let all = scene.all();
    assert!(
        all.iter().all(|entity| entity.key != key(4)),
        "the boat is gone"
    );
    assert_eq!(lying_count(&all, items::OAK_BOAT), 0);
}

/// Brief 2.0 item 6: entities are hurt before any block breaks, so the drops of this
/// explosion are not hit by it.
#[test]
fn the_drops_of_an_explosion_are_not_destroyed_by_it() {
    let dirt = TNT.offset(1, 0, 0);
    let world = World::of(&[(dirt, blocks::DIRT)]).with_entities(&[primed(1, TNT, FUSE, None)]);
    let mut scene = Scene::alone(&world, flint(), FAR);
    until_it_explodes(&mut scene, 20);
    assert_eq!(scene.block(dirt), blocks::AIR);
    assert_eq!(lying_count(&scene.all(), items::DIRT), 1);
}

/// Brief 2.5: a player who flies in creative is not knocked back.
#[test]
fn a_flying_creative_player_is_not_knocked_back() {
    let world = World::default().with_entities(&[primed(1, TNT, FUSE, None)]);
    let mut scene = Scene::alone(&world, flint(), Vec3::new(8.5, -60.0, 5.5));
    scene.input(clustine_sim::api::PlayerInput::SetFlying { flying: true });
    let (_, output) = until_it_explodes(&mut scene, 20);
    let me = scene.me();
    assert!(
        explosions(&output)[0]
            .knockback
            .iter()
            .all(|(entity, _)| *entity != me)
    );
}

// ------------------------------------------------------------------------------------
// Across a pinned line (decision 10; section 6 item 9; section 7 item 11; brief 11.1).

const BY_THE_LINE: Vec3 = Vec3::new(4.5, -60.0, 2.5);
const NEAR_LINE: BlockPos = BlockPos::new(14, -60, 4);

fn exploded_effects(output: &TickOutput) -> Vec<(BlockPos, BlockState, Blast)> {
    output
        .effects
        .iter()
        .filter_map(|effect| match effect.what {
            Effected::Exploded { expected, blast } => Some((effect.at, expected, blast)),
            _ => None,
        })
        .collect()
}

fn blasted_effects(output: &TickOutput) -> Vec<(EntityId, f32, [f64; 3], Blast)> {
    output
        .effects
        .iter()
        .filter_map(|effect| match effect.what {
            Effected::OnEntity {
                key,
                by:
                    EntityUse::Blasted {
                        damage,
                        push,
                        blast,
                    },
            } => Some((key, damage.0, push.map(|part| part.0), blast)),
            _ => None,
        })
        .collect()
}

/// Dirt on both sides of the line at x = 16, an armour stand beyond it, and primed TNT
/// a block and a half from it on the west.
fn by_the_line() -> World {
    let mut placed = Vec::new();
    // Two rows only, so that nothing stands between the stand and the TNT.
    for x in 15..=17 {
        for z in 3..=4 {
            placed.push((BlockPos::new(x, -60, z), blocks::DIRT));
        }
    }
    World::of(&placed).with_entities(&[
        primed(1, NEAR_LINE, FUSE, Some(player(1).0)),
        stand(2, [17.5, -60.0, 7.5]),
    ])
}

/// Decision 10 and section 6 item 9: an explosion is worked out whole in the TNT's
/// region; the blocks it breaks beyond the line are effects, one each, with the state
/// the copy showed and the explosion: its source and tick, centre, radius, no decay for
/// TNT, the TNT as its direct source and the owner; the copy is not written; an entity
/// beyond the line is hit by an entity effect with the damage and the push.
#[test]
fn an_explosion_at_a_pinned_line_breaks_the_far_blocks_by_effects_that_carry_it() {
    let world = by_the_line();
    let mut west = Scene::new(Layout::side(true), &world, flint(), BY_THE_LINE);
    let (_, output) = until_it_explodes(&mut west, 20);
    let tick = west.region.tick_number();
    let far = exploded_effects(&output);
    assert!(!far.is_empty(), "{:?}", output.effects);
    for (at, expected, blast) in &far {
        assert!(at.x >= 16, "only the far side's blocks are effects: {at:?}");
        assert_eq!(
            *expected,
            block_at(&west.region, *at),
            "what the copy shows"
        );
        assert_eq!(blast.source, key(1));
        assert_eq!(blast.tick, tick);
        assert_eq!(blast.radius, Float(4.0));
        assert!(!blast.decay, "TNT's drops do not decay");
        assert!(blast.direct);
        assert_eq!(blast.owner, Some(player(1).0));
        assert!(blast.owner_creative);
        assert!((blast.centre[0].0 - 14.5).abs() < 0.5 && (blast.centre[2].0 - 4.5).abs() < 0.5);
    }
    let positions: Vec<BlockPos> = far.iter().map(|(at, _, _)| *at).collect();
    assert!(
        positions.contains(&BlockPos::new(16, -60, 4)),
        "the dirt beside the line: {positions:?}"
    );
    assert_eq!(
        block_at(&west.region, BlockPos::new(16, -60, 4)),
        blocks::DIRT,
        "the copy is not written"
    );
    assert_eq!(
        west.block(BlockPos::new(15, -60, 4)),
        blocks::AIR,
        "the near side breaks"
    );
    let hit = blasted_effects(&output);
    assert_eq!(hit.len(), 1, "{:?}", output.effects);
    let (stand_key, damage, push, blast) = hit[0];
    assert_eq!(stand_key, key(2));
    assert!(damage > 0.0);
    assert!(
        push[0] > 0.0 && push[2] > 0.0,
        "pushed away to the south-east: {push:?}"
    );
    assert_eq!(blast.source, key(1));
}

/// Section 4 and decision 10: the drops at a line are not merged across it; the near
/// side lays the drops of its own blocks only, in its own land.
#[test]
fn the_near_side_lays_the_drops_of_its_own_blocks_only() {
    let world = by_the_line();
    let mut west = Scene::new(Layout::side(true), &world, flint(), BY_THE_LINE);
    let (_, output) = until_it_explodes(&mut west, 20);
    let near_broken = (3..=4)
        .filter(|z| west.block(BlockPos::new(15, -60, *z)) == blocks::AIR)
        .count();
    assert_eq!(
        lying_count(&west.all(), items::DIRT),
        i32::try_from(near_broken).expect("few")
    );
    let sent_dirt = output
        .arrivals
        .iter()
        .filter(|(_, entity)| {
            matches!(&entity.data, EntityData::Item { stack, .. } if stack.item == items::DIRT)
        })
        .count();
    assert_eq!(
        sent_dirt, 0,
        "no drop is sent across: {:?}",
        output.arrivals
    );
}

/// Brief 2.5: an entity more than twice the radius from the centre is not hit, and a
/// player there is not knocked back.
#[test]
fn a_player_more_than_eight_blocks_away_is_not_knocked_back() {
    let world = World::default().with_entities(&[primed(1, TNT, FUSE, None)]);
    let mut scene = Scene::alone(&world, flint(), Vec3::new(8.5, -60.0, -0.5));
    let (_, output) = until_it_explodes(&mut scene, 20);
    let me = scene.me();
    assert!(
        explosions(&output)[0]
            .knockback
            .iter()
            .all(|(entity, _)| *entity != me)
    );
}

/// Brief 1.2: TNT primed by an explosion makes no sound (unlike `prime`).
#[test]
fn tnt_primed_by_an_explosion_plays_no_priming_sound() {
    let world = World::of(&[(TNT.offset(2, 0, 0), blocks::TNT)])
        .with_entities(&[primed(1, TNT, FUSE, None)]);
    let mut scene = Scene::alone(&world, flint(), FAR);
    let (_, output) = until_it_explodes(&mut scene, 20);
    assert_eq!(scene.of_kind(entity_types::TNT).len(), 1, "chained");
    assert!(sounds(&output, sounds::ENTITY_TNT_PRIMED).is_empty());
}

/// Brief 2.2 and 2.5: water resists 100, so TNT in water breaks nothing: its rays end
/// at their first point.
#[test]
fn tnt_in_water_breaks_nothing() {
    let water = state("minecraft:water[level=0]");
    let mut placed = vec![(TNT, water)];
    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        placed.push((TNT.offset(dx, 0, dz), water));
        placed.push((TNT.offset(2 * dx, 0, 2 * dz), blocks::DIRT));
    }
    let world = World::of(&placed).with_entities(&[primed(1, TNT, FUSE, None)]);
    let mut scene = Scene::alone(&world, flint(), FAR);
    let (_, output) = until_it_explodes(&mut scene, 20);
    assert_eq!(explosions(&output)[0].blocks, 0, "no position is reached");
    assert_eq!(scene.block(TNT.offset(0, -1, 0)), blocks::STONE);
    for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        assert_eq!(scene.block(TNT.offset(2 * dx, 0, 2 * dz)), blocks::DIRT);
    }
}

/// Brief 2.6 and 3.0 item 10: a chest broken by TNT spills its contents, and drops
/// itself, as TNT drops everything.
#[test]
fn a_chest_broken_by_tnt_spills_what_it_holds_and_drops_itself() {
    let at = TNT.offset(1, 0, 0);
    let chest = state("minecraft:chest[facing=north,type=single]");
    let world = World::of(&[(at, chest)]);
    let mut scene = Scene::alone(
        &world,
        inventory(&[(0, stack(items::DIRT, 64))]),
        Vec3::new(6.5, -60.0, 8.5),
    );
    let (serial, _) = scene.open(at);
    scene.menu_click(serial, 54, 0, clustine_sim::api::ClickKind::QuickMove);
    scene.close(serial);
    // TNT placed beside the chest and lit.
    scene.input(clustine_sim::api::PlayerInput::SetInventorySlot {
        slot: 1,
        stack: Some(stack(items::TNT, 1)),
    });
    scene.walk_to(Vec3::new(6.5, -60.0, 6.5));
    scene.select(1);
    scene.click(TNT.offset(0, -1, 0), Face::Top);
    assert_eq!(scene.block(TNT), blocks::TNT);
    scene.input(clustine_sim::api::PlayerInput::SetInventorySlot {
        slot: 2,
        stack: Some(stack(items::FLINT_AND_STEEL, 1)),
    });
    scene.select(2);
    scene.click(TNT, Face::Top);
    assert_eq!(scene.of_kind(entity_types::TNT).len(), 1);
    scene.walk_to(FAR);
    until_it_explodes(&mut scene, 100);
    assert_eq!(scene.block(at), blocks::AIR);
    assert_eq!(lying_count(&scene.all(), items::DIRT), 64);
    assert_eq!(lying_count(&scene.all(), items::CHEST), 1);
}

/// Decision 10: the holder does the far side's part: it breaks the blocks, lays their
/// drops and lets the entity be hit.
#[test]
fn the_holder_breaks_the_far_blocks_lays_their_drops_and_kills_the_stand() {
    let world = by_the_line();
    let mut west = Scene::new(Layout::side(true), &world, flint(), BY_THE_LINE);
    let mut east = region_of(&Layout::side(false), &world, flint());
    let (_, output) = until_it_explodes(&mut west, 20);
    let far = exploded_effects(&output);
    let effects: Vec<Effect> = output
        .effects
        .iter()
        .filter(|effect| effect.at.chunk().x >= 1)
        .cloned()
        .collect();
    filed(&mut east, 50, effects);
    for (at, _, _) in &far {
        assert_eq!(
            block_at(&east, *at),
            blocks::AIR,
            "{at:?} broken by its holder"
        );
    }
    let dirt_broken = far
        .iter()
        .filter(|(_, expected, _)| *expected == blocks::DIRT)
        .count();
    assert_eq!(
        lying_count(&all_of(&east), items::DIRT),
        i32::try_from(dirt_broken).expect("few")
    );
    assert!(
        all_of(&east).iter().all(|entity| entity.key != key(2)),
        "the stand is killed"
    );
}

/// An explosion of the holder's tests' own, by an entity of key `source` in tick
/// `tick`.
fn blast(source: u32, tick: u64, owner: Option<Uuid>) -> Blast {
    Blast {
        source: key(source),
        tick,
        centre: [Double(14.5), Double(-59.94), Double(4.5)],
        radius: Float(4.0),
        decay: false,
        direct: true,
        owner,
        owner_creative: false,
    }
}

fn dirt_field() -> (World, Vec<BlockPos>) {
    let mut placed = Vec::new();
    for x in 18..=22 {
        for z in 2..=5 {
            placed.push((BlockPos::new(x, -60, z), blocks::DIRT));
        }
    }
    let positions = placed.iter().map(|(at, _)| *at).collect();
    (World::of(&placed), positions)
}

fn explode_effects(at: &[BlockPos], expected: BlockState, blast: Blast) -> Vec<Effect> {
    at.iter()
        .map(|at| Effect {
            at: *at,
            what: Effected::Exploded { expected, blast },
        })
        .collect()
}

/// The dirt item stacks lying in `region`.
fn dirt_stacks(region: &clustine_sim::Region) -> Vec<i32> {
    items_lying(&all_of(region))
        .iter()
        .filter(|(stack, _)| stack.item == items::DIRT)
        .map(|(stack, _)| stack.count)
        .collect()
}

/// Section 6 item 9: after all of one explosion's blocks taken in a tick the holder lays
/// their drops merged within that explosion: twenty dirt in stacks of 16 and 4.
#[test]
fn the_holder_merges_the_drops_of_one_explosions_blocks_taken_in_one_tick() {
    let (world, positions) = dirt_field();
    let mut east = region_of(&Layout::side(false), &world, flint());
    filed(
        &mut east,
        50,
        explode_effects(&positions, blocks::DIRT, blast(1, 7, None)),
    );
    for at in &positions {
        assert_eq!(block_at(&east, *at), blocks::AIR);
    }
    let mut stacks = dirt_stacks(&east);
    stacks.sort_unstable();
    assert_eq!(stacks, vec![4, 16]);
}

/// Section 7 item 11: two explosions' drops never merge, even in one tick.
#[test]
fn the_drops_of_two_explosions_taken_in_one_tick_are_not_merged() {
    let (world, positions) = dirt_field();
    let mut east = region_of(&Layout::side(false), &world, flint());
    let mut effects = explode_effects(&positions[..10], blocks::DIRT, blast(1, 7, None));
    effects.extend(explode_effects(
        &positions[10..],
        blocks::DIRT,
        blast(2, 7, None),
    ));
    filed(&mut east, 50, effects);
    let mut stacks = dirt_stacks(&east);
    stacks.sort_unstable();
    assert_eq!(stacks, vec![10, 10]);
}

/// Section 6 item 9: effects of one explosion split across ticks lay apart.
#[test]
fn the_drops_of_one_explosion_taken_in_two_ticks_lie_apart() {
    let (world, positions) = dirt_field();
    let mut east = region_of(&Layout::side(false), &world, flint());
    let one = blast(1, 7, None);
    filed(
        &mut east,
        50,
        explode_effects(&positions[..10], blocks::DIRT, one),
    );
    filed(
        &mut east,
        51,
        explode_effects(&positions[10..], blocks::DIRT, one),
    );
    let mut stacks = dirt_stacks(&east);
    stacks.sort_unstable();
    assert_eq!(stacks, vec![10, 10]);
}

/// Decision 10: an explosion's effect breaks its block only if it is still the block
/// the copy showed.
#[test]
fn an_explosions_effect_on_a_block_that_changed_does_nothing() {
    let (world, positions) = dirt_field();
    let mut east = region_of(&Layout::side(false), &world, flint());
    filed(
        &mut east,
        50,
        explode_effects(&positions[..3], blocks::STONE, blast(1, 7, None)),
    );
    for at in &positions[..3] {
        assert_eq!(block_at(&east, *at), blocks::DIRT);
    }
    assert!(dirt_stacks(&east).is_empty());
}

/// Decision 10 and brief 1.2: TNT across the line broken by an explosion is primed by
/// its holder with a short fuse and the explosion's owner.
#[test]
fn tnt_across_the_line_broken_by_an_explosion_is_primed_by_its_holder() {
    let at = BlockPos::new(18, -60, 4);
    let mut east = region_of(
        &Layout::side(false),
        &World::of(&[(at, blocks::TNT)]),
        flint(),
    );
    let owner = player(1).0;
    filed(
        &mut east,
        50,
        explode_effects(&[at], blocks::TNT, blast(1, 7, Some(owner))),
    );
    assert_eq!(block_at(&east, at), blocks::AIR);
    let tnt = of_kind(&east, entity_types::TNT);
    assert_eq!(tnt.len(), 1);
    let (fuse, _, _, primed_by) = tnt_data(&tnt[0]);
    assert!((8..=29).contains(&fuse), "{fuse}");
    assert_eq!(primed_by, Some(owner));
}

/// Section 6 item 9 and brief 11.1: TNT across a line burnt by fire is removed and then
/// primed by its holder, with no owner and the full fuse.
#[test]
fn tnt_burnt_across_the_line_is_primed_by_its_holder_with_no_owner() {
    let at = BlockPos::new(18, -60, 4);
    let mut east = region_of(
        &Layout::side(false),
        &World::of(&[(at, blocks::TNT)]),
        flint(),
    );
    filed(
        &mut east,
        50,
        vec![Effect {
            at,
            what: Effected::Burnt {
                expected: blocks::TNT,
            },
        }],
    );
    assert_eq!(block_at(&east, at), blocks::AIR);
    let tnt = of_kind(&east, entity_types::TNT);
    assert_eq!(tnt.len(), 1);
    let (fuse, _, _, owner) = tnt_data(&tnt[0]);
    assert!((78..=80).contains(&fuse), "{fuse}");
    assert_eq!(owner, None);
}

/// Decision 10 and brief 11.1: a container across the line broken by an explosion
/// spills what it holds on the far side, and drops itself, as TNT drops everything.
#[test]
fn a_chest_across_the_line_broken_by_an_explosion_spills_there() {
    let at = BlockPos::new(18, -60, 4);
    let chest = state("minecraft:chest[facing=north,type=single]");
    let world = World::of(&[(at, chest)]);
    let mut east = Scene::new(
        Layout::side(false),
        &world,
        inventory(&[(0, stack(items::DIRT, 64))]),
        Vec3::new(20.5, -60.0, 2.5),
    );
    let (serial, _) = east.open(at);
    east.menu_click(serial, 27 + 27, 0, clustine_sim::api::ClickKind::QuickMove);
    east.close(serial);
    filed(
        &mut east.region,
        50,
        explode_effects(&[at], chest, blast(1, 7, None)),
    );
    assert_eq!(east.block(at), blocks::AIR);
    assert_eq!(lying_count(&east.all(), items::DIRT), 64);
    assert_eq!(lying_count(&east.all(), items::CHEST), 1);
}

/// Decision 10: an entity across the line is hit by the entity effect, as the exploding
/// region worked it out.
#[test]
fn a_stand_across_the_line_blasted_by_an_entity_effect_is_killed_by_its_holder() {
    let world = World::default().with_entities(&[stand(2, [18.5, -60.0, 4.5])]);
    let mut east = region_of(&Layout::side(false), &world, flint());
    filed(
        &mut east,
        50,
        vec![Effect {
            at: BlockPos::new(18, -60, 4),
            what: Effected::OnEntity {
                key: key(2),
                by: EntityUse::Blasted {
                    damage: Float(30.0),
                    push: [Double(0.5), Double(0.1), Double(0.0)],
                    blast: blast(1, 7, None),
                },
            },
        }],
    );
    assert!(all_of(&east).iter().all(|entity| entity.key != key(2)));
    assert!(of_kind(&east, entity_types::ARMOR_STAND).is_empty());
}

// ------------------------------------------------------------------------------------
// The TNT cart (plan section 1; brief 1.6).

fn tnt_cart(n: u32, position: [f64; 3]) -> Entity {
    entity(
        n,
        entity_types::TNT_MINECART,
        position,
        EntityData::Minecart {
            hurt: Hurt::default(),
            display: None,
            display_offset: None,
            flipped: false,
            contents: Vec::new(),
            fuel: 0,
            push: [Double(0.0); 2],
            fuse: -1,
            enabled: true,
        },
    )
}

fn cart_fuse(entity: &Entity) -> i32 {
    match &entity.data {
        EntityData::Minecart { fuse, .. } => *fuse,
        other => panic!("not a cart: {other:?}"),
    }
}

/// Brief 1.6: a TNT cart on a powered activator rail is primed with a fuse of 80 (with
/// entity event 70 and `entity.tnt.primed`), and explodes when it runs out.
#[test]
fn a_tnt_cart_on_a_powered_activator_rail_is_primed_and_explodes() {
    let rail = BlockPos::new(8, -60, 8);
    let world = World::of(&[(
        rail,
        state("minecraft:activator_rail[powered=true,shape=north_south]"),
    )])
    .with_entities(&[tnt_cart(1, [8.5, -60.0, 8.5])]);
    let mut scene = Scene::alone(&world, flint(), FAR);
    let mut seen = Vec::new();
    assert!(
        scene.idle_until(5, &mut seen, |scene, _| scene
            .of_kind(entity_types::TNT_MINECART)
            .iter()
            .any(|cart| cart_fuse(cart) >= 0)),
        "primed"
    );
    // Primed in one of the ticks that loaded it and let player 1 in: 80 less those.
    let fuse = cart_fuse(&scene.of_kind(entity_types::TNT_MINECART)[0]);
    assert!((74..=80).contains(&fuse), "{fuse}");
    let (tick, output) = until_it_explodes(&mut scene, 90);
    assert!(
        tick.abs_diff(u32::try_from(fuse).expect("positive")) <= 1,
        "{tick} after a fuse of {fuse}"
    );
    let explosion = &explosions(&output)[0];
    assert!(explosion.radius >= 4.0, "{}", explosion.radius);
    assert!(scene.of_kind(entity_types::TNT_MINECART).is_empty());
}
