//! Beds, respawn anchors and their explosions by dimension (family C of P11,
//! `docs/groundwork/p-plan.md`, sections 17.2, 17.5 and 17.6; the game's rules in
//! `docs/groundwork/p-brief.md`, sections 1.2, 5.3 and 5.4), through the public API, in
//! all three dimensions: a bed sleeps in the overworld and explodes in the Nether and
//! the End; a straw bed breaks there; the night is skipped by the overworld's sleepers
//! alone; an anchor charges everywhere, sets the spawn in the Nether and explodes
//! elsewhere, shielded by water beside it or above it; and the explosion of a bed or an
//! anchor sets fire, hurts nobody, and across a line names no entity.
//!
//! Player 1 stands on a stone floor at y = -61 (feet at y = -60) in chunk (0, 0), whose
//! 5x5 is loaded. Every player of Clustine is a creative one.

mod n6_scene;

use clustine_data::{BlockState, blocks, entity_types, items, sounds};
use clustine_sim::TickOutput;
use clustine_sim::api::{
    EnderChest, Face, Inventory, Place, PlayerEvent, Pose, RegionEvent, Respawn, WorldClock,
    WorldPlayer,
};
use clustine_sim::{
    EdgeState, Entered, PlayerChange, PlayerJoin, Region, RegionState, TickInputs, Ticket,
};
use clustine_world::{
    Blast, BlockPos, Dimension, Effected, Entity, EntityData, EntityId, EntityIds, Float, Vec3,
};
use n6_scene::*;
use uuid::Uuid;

const NIGHT: WorldClock = WorldClock {
    game_time: 18_000,
    day: 18_000,
};
const DAY: WorldClock = WorldClock {
    game_time: 1_000,
    day: 1_000,
};

/// A bed facing north: its head at (2, -60, 0), its foot at (2, -60, 1).
const HEAD: BlockPos = BlockPos::new(2, -60, 0);
const FOOT: BlockPos = BlockPos::new(2, -60, 1);
/// Where player 1 stands: within reach of the bed and the anchor.
const FEET: Vec3 = Vec3::new(4.5, -60.0, 0.5);
const ANCHOR: BlockPos = HEAD;

fn bed(part: &str) -> BlockState {
    state(&format!("minecraft:red_bed[facing=north,part={part}]"))
}

fn straw_bed(part: &str) -> BlockState {
    state(&format!("minecraft:straw_bed[facing=north,part={part}]"))
}

fn anchor(charges: u8) -> BlockState {
    state(&format!("minecraft:respawn_anchor[charges={charges}]"))
}

fn glowstone() -> Inventory {
    inventory(&[(0, stack(items::GLOWSTONE, 3))])
}

fn bare() -> Inventory {
    inventory(&[])
}

/// A region of `dimension` alone over the land near the origin, with `world` in it and
/// the clock at `clock`, and player 1 in it at `feet`, placed in that dimension.
fn scene_in(
    dimension: Dimension,
    world: &World,
    inventory: Inventory,
    clock: WorldClock,
    feet: Vec3,
) -> Scene {
    scene_of(dimension, Layout::alone(), world, inventory, clock, feet)
}

/// As [`scene_in`], with `layout`.
fn scene_of(
    dimension: Dimension,
    layout: Layout,
    world: &World,
    inventory: Inventory,
    clock: WorldClock,
    feet: Vec3,
) -> Scene {
    let mut state = RegionState::new(EntityIds::block(layout.block).expect("a block"));
    state.edges.insert(
        EDGE,
        EdgeState {
            start: 1,
            ..EdgeState::default()
        },
    );
    let mut config = config(inventory.clone());
    config.dimension = dimension;
    let mut region = Region::restore(config, state, layout.holdings());
    region.tick(&TickInputs {
        tickets_added: layout
            .loaded
            .iter()
            .chain(&layout.foreign)
            .map(|chunk| (*chunk, Ticket::Viewer))
            .collect(),
        clock: Some(clock),
        ..TickInputs::default()
    });
    region.tick(&TickInputs {
        chunks_loaded: layout
            .loaded
            .iter()
            .map(|position| (*position, world.chunk(*position)))
            .collect(),
        copy_bases: layout
            .copies
            .iter()
            .map(|position| (*position, world.chunk(*position)))
            .collect(),
        foreign: layout
            .foreign
            .iter()
            .map(|position| (*position, OTHER))
            .collect(),
        ..TickInputs::default()
    });
    let mut scene = Scene {
        region,
        layout,
        inventory,
        actors: Vec::new(),
    };
    join_in(&mut scene, dimension, 1, feet);
    scene
}

/// Player `n` joins and enters at `feet` in `dimension`, as [`Scene::join`] has them
/// enter the overworld.
fn join_in(scene: &mut Scene, dimension: Dimension, n: u128, feet: Vec3) {
    scene.region.tick(&TickInputs {
        player_changes: vec![PlayerChange::Join(
            EDGE,
            PlayerJoin {
                player: player(n),
                name: format!("Player{n}"),
                attempt: 7000 + n as u64,
            },
        )],
        ..TickInputs::default()
    });
    let entering = scene
        .region
        .entering_state(player(n))
        .expect("the join is entering")
        .entity_id;
    scene.region.tick(&TickInputs {
        entered: vec![Entered {
            unit: None,
            player: player(n),
            entity: entering,
            place: Some(Place {
                dimension,
                travel: None,
                seen_credits: false,
                portal_cooldown: 0,
                respawn: None,
                experience: Default::default(),
                pose: Pose {
                    position: feet,
                    yaw: 0.0,
                    pitch: 0.0,
                    on_ground: true,
                },
                flying: false,
                inventory: scene.inventory.clone(),
                selected_slot: 0,
                ender: EnderChest::default(),
                effects: Vec::new(),
                time_since_rest: 0,
                raid_omen_position: None,
            }),
            holder: None,
        }],
        ..TickInputs::default()
    });
    let entity = scene
        .region
        .player(player(n))
        .unwrap_or_else(|| panic!("player {n} entered"))
        .0;
    scene.actors.push(Actor {
        id: player(n),
        entity,
        input: 0,
        sequence: 0,
    });
    let feet_now = scene.region.player(player(n)).expect("there").1.position;
    assert_eq!(feet_now, feet, "player {n} stands where the test puts them");
}

/// An explosion a tick reports: its centre, radius, blocks and the players' pushes.
type Reported = ([f64; 3], f32, i32, Vec<(EntityId, [f64; 3])>);

/// The explosions the tick reports.
fn explosions(output: &TickOutput) -> Vec<Reported> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Explosion {
                centre,
                radius,
                blocks,
                knockback,
                ..
            } => Some((*centre, *radius, *blocks, knockback.clone())),
            _ => None,
        })
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

fn messages(output: &TickOutput) -> Vec<(String, bool)> {
    output
        .player_events
        .iter()
        .filter_map(|(_, event)| match event {
            PlayerEvent::Message { key, overlay, .. } => Some((key.clone(), *overlay)),
            _ => None,
        })
        .collect()
}

fn respawn(scene: &Scene) -> Option<Respawn> {
    scene
        .region
        .player_state(player(1))
        .expect("player 1 is in the region")
        .respawn
}

fn asleep(scene: &Scene) -> bool {
    scene
        .region
        .player_state(player(1))
        .expect("player 1 is in the region")
        .sleeping
        .is_some()
}

/// The blocks of fire near the origin.
fn fires(region: &Region) -> usize {
    let fire = blocks::FIRE.block_id();
    let mut found = 0;
    for x in -10..=14 {
        for z in -12..=12 {
            for y in -64..=-52 {
                if block_at(region, BlockPos::new(x, y, z)).block_id() == fire {
                    found += 1;
                }
            }
        }
    }
    found
}

fn is_of(state: BlockState, block: BlockState) -> bool {
    state.block_id() == block.block_id()
}

fn entity(n: u32, kind: i32, position: [f64; 3], data: EntityData) -> Entity {
    Entity {
        portal: Default::default(),
        key: key(n),
        kind: u16::try_from(kind).expect("fits"),
        uuid: Uuid::from_u128(u128::from(n) + 9_000),
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

/// An armour stand, which any explosion that hurts it breaks.
fn stand(n: u32, position: [f64; 3]) -> Entity {
    entity(
        n,
        entity_types::ARMOR_STAND,
        position,
        EntityData::ArmorStand {
            flags: 0,
            disabled_slots: 0,
            pose: Box::new([[Float(0.0); 3]; 6]),
            equipment: Vec::new(),
            health: Float(20.0),
            last_hit: None,
            air: 300,
            frozen: 0,
        },
    )
}

/// Dirt lying about, which an explosion that hurts it this near destroys.
fn dirt(n: u32, position: [f64; 3]) -> Entity {
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

fn present(region: &Region, n: u32) -> bool {
    all_of(region).iter().any(|entity| entity.key == key(n))
}

// ------------------------------------------------------------------------------------
// Beds.

#[test]
fn a_bed_at_night_lays_its_user_down_in_the_overworld_and_explodes_elsewhere() {
    for dimension in Dimension::ALL {
        let world = World::of(&[(HEAD, bed("head")), (FOOT, bed("foot"))]);
        let mut scene = scene_in(dimension, &world, bare(), NIGHT, FEET);
        let output = scene.click(FOOT, Face::Top);
        let blasts = explosions(&output);
        if dimension == Dimension::Overworld {
            assert!(blasts.is_empty(), "{dimension:?}");
            assert!(asleep(&scene));
            assert!(is_of(scene.block(HEAD), blocks::RED_BED));
            assert!(is_of(scene.block(FOOT), blocks::RED_BED));
            continue;
        }
        assert!(!asleep(&scene), "{dimension:?}");
        assert_eq!(blasts.len(), 1, "{dimension:?}");
        let (centre, radius, broken, knockback) = &blasts[0];
        assert_eq!(*centre, [2.5, -59.5, 0.5], "at the head's centre");
        assert_eq!(*radius, 5.0);
        assert!(*broken > 0);
        assert!(knockback.is_empty(), "nobody is pushed: {knockback:?}");
        assert_eq!(scene.block(HEAD), blocks::AIR, "{dimension:?}");
        assert!(
            !is_of(scene.block(FOOT), blocks::RED_BED),
            "both halves go: {dimension:?}"
        );
        assert_eq!(respawn(&scene), None, "no spawn is set: {dimension:?}");
        assert!(
            fires(&scene.region) > 0,
            "the blast sets fire: {dimension:?}"
        );
    }
}

#[test]
fn a_bed_by_day_sets_the_spawn_in_the_overworld_only() {
    for dimension in Dimension::ALL {
        let world = World::of(&[(HEAD, bed("head")), (FOOT, bed("foot"))]);
        let mut scene = scene_in(dimension, &world, bare(), DAY, FEET);
        let output = scene.click(HEAD, Face::Top);
        if dimension == Dimension::Overworld {
            assert_eq!(
                respawn(&scene).map(|respawn| (respawn.pos, respawn.dimension)),
                Some((HEAD, Dimension::Overworld))
            );
            assert_eq!(
                messages(&output),
                [
                    ("block.minecraft.set_spawn".to_owned(), false),
                    ("block.minecraft.bed.no_sleep".to_owned(), true),
                ]
            );
            assert!(explosions(&output).is_empty());
        } else {
            assert_eq!(respawn(&scene), None, "{dimension:?}");
            assert!(messages(&output).is_empty(), "{:?}", messages(&output));
            assert_eq!(explosions(&output).len(), 1, "{dimension:?}");
        }
    }
}

#[test]
fn a_bed_in_the_nether_or_the_end_explodes_even_when_someone_lies_in_it() {
    for dimension in [Dimension::Nether, Dimension::End] {
        let taken = |part| {
            state(&format!(
                "minecraft:red_bed[facing=north,occupied=true,part={part}]"
            ))
        };
        let world = World::of(&[(HEAD, taken("head")), (FOOT, taken("foot"))]);
        let mut scene = scene_in(dimension, &world, bare(), NIGHT, FEET);
        let output = scene.click(HEAD, Face::Top);
        assert_eq!(explosions(&output).len(), 1, "{dimension:?}");
        assert!(
            !messages(&output)
                .iter()
                .any(|(key, _)| key == "block.minecraft.bed.occupied")
        );
    }
}

#[test]
fn a_straw_bed_sleeps_in_the_overworld_and_elsewhere_breaks_without_exploding() {
    for dimension in Dimension::ALL {
        let world = World::of(&[(HEAD, straw_bed("head")), (FOOT, straw_bed("foot"))]);
        let mut scene = scene_in(dimension, &world, bare(), NIGHT, FEET);
        let output = scene.click(HEAD, Face::Top);
        assert!(explosions(&output).is_empty(), "{dimension:?}");
        if dimension == Dimension::Overworld {
            assert!(asleep(&scene));
            assert_eq!(respawn(&scene), None, "a straw bed never sets the spawn");
            continue;
        }
        assert!(!asleep(&scene), "{dimension:?}");
        assert_eq!(scene.block(HEAD), blocks::AIR, "{dimension:?}");
        assert!(sounds_of(&output).contains(&sounds::BLOCK_STRAW_BED_BREAK_LEAVE));
    }
}

#[test]
fn a_bed_s_explosion_hurts_and_pushes_nobody() {
    for dimension in [Dimension::Nether, Dimension::End] {
        let world = World::of(&[(HEAD, bed("head")), (FOOT, bed("foot"))])
            .with_entities(&[stand(1, [6.5, -60.0, 3.5]), dirt(2, [-2.5, -60.0, 3.5])]);
        let mut scene = scene_in(dimension, &world, bare(), NIGHT, FEET);
        assert!(present(&scene.region, 1) && present(&scene.region, 2));
        let output = scene.click(HEAD, Face::Top);
        assert_eq!(explosions(&output).len(), 1);
        assert!(present(&scene.region, 1), "the stand stands: {dimension:?}");
        assert!(present(&scene.region, 2), "the dirt lies: {dimension:?}");
    }
}

/// Another player of the world, `n`, in `dimension`, awake.
fn awake_elsewhere(n: u128, dimension: Dimension) -> WorldPlayer {
    WorldPlayer {
        dimension,
        player: player(n),
        entity: EntityId(1000 + n as u64),
        position: [500.5, 64.0, 0.5],
        game_mode: 1,
        sleeping: None,
    }
}

/// Player 1 lies down in the overworld at night, with player 2 awake in `dimension`; the
/// ticks until the night is skipped, if it is within 120.
fn night_skipped_with_another_in(dimension: Dimension) -> Option<u32> {
    let world = World::of(&[(HEAD, bed("head")), (FOOT, bed("foot"))]);
    let mut scene = scene_in(Dimension::Overworld, &world, bare(), NIGHT, FEET);
    scene.region.tick(&TickInputs {
        world_players: Some(vec![
            WorldPlayer {
                dimension: Dimension::Overworld,
                player: player(1),
                entity: scene.me(),
                position: [FEET.x, FEET.y, FEET.z],
                game_mode: 1,
                sleeping: None,
            },
            awake_elsewhere(2, dimension),
        ]),
        ..TickInputs::default()
    });
    scene.click(HEAD, Face::Top);
    assert!(asleep(&scene));
    (1..=120).find(|_| scene.idle().clock_jumped)
}

#[test]
fn the_night_is_skipped_by_the_overworld_s_sleepers_while_another_is_awake_in_the_nether_or_the_end()
 {
    assert_eq!(night_skipped_with_another_in(Dimension::Overworld), None);
    for dimension in [Dimension::Nether, Dimension::End] {
        let ticks = night_skipped_with_another_in(dimension);
        assert!(
            ticks.is_some_and(|ticks| ticks <= 101),
            "{dimension:?}: {ticks:?}"
        );
    }
}

// ------------------------------------------------------------------------------------
// Respawn anchors.

#[test]
fn glowstone_charges_an_anchor_in_every_dimension_and_is_kept_by_a_creative_player() {
    for dimension in Dimension::ALL {
        let world = World::of(&[(ANCHOR, anchor(0))]);
        let mut scene = scene_in(dimension, &world, glowstone(), NIGHT, FEET);
        for charges in 1..=4 {
            let output = scene.click(ANCHOR, Face::Top);
            assert_eq!(scene.block(ANCHOR), anchor(charges), "{dimension:?}");
            assert!(
                sounds_of(&output).contains(&sounds::BLOCK_RESPAWN_ANCHOR_CHARGE),
                "{dimension:?}"
            );
            assert!(explosions(&output).is_empty());
            assert_eq!(scene.holds(0), Some(stack(items::GLOWSTONE, 3)));
        }
        assert_eq!(
            respawn(&scene),
            None,
            "charging sets no spawn: {dimension:?}"
        );
    }
}

#[test]
fn glowstone_on_a_full_anchor_uses_it() {
    for dimension in Dimension::ALL {
        let world = World::of(&[(ANCHOR, anchor(4))]);
        let mut scene = scene_in(dimension, &world, glowstone(), NIGHT, FEET);
        let output = scene.click(ANCHOR, Face::Top);
        let works = dimension == Dimension::Nether;
        assert_eq!(
            explosions(&output).len(),
            usize::from(!works),
            "{dimension:?}"
        );
        assert_eq!(
            respawn(&scene).is_some(),
            works,
            "the spawn is set in the Nether alone"
        );
    }
}

#[test]
fn an_empty_anchor_used_by_an_empty_hand_does_nothing_anywhere() {
    for dimension in Dimension::ALL {
        let world = World::of(&[(ANCHOR, anchor(0))]);
        let mut scene = scene_in(dimension, &world, bare(), NIGHT, FEET);
        let output = scene.click(ANCHOR, Face::Top);
        assert_eq!(scene.block(ANCHOR), anchor(0), "{dimension:?}");
        assert!(explosions(&output).is_empty());
        assert!(messages(&output).is_empty());
        assert_eq!(respawn(&scene), None);
    }
}

#[test]
fn a_charged_anchor_sets_the_spawn_in_the_nether_once_and_keeps_its_charge() {
    let world = World::of(&[(ANCHOR, anchor(2))]);
    let mut scene = scene_in(Dimension::Nether, &world, bare(), NIGHT, FEET);
    let output = scene.click(ANCHOR, Face::Top);
    assert_eq!(
        respawn(&scene),
        Some(Respawn {
            pos: ANCHOR,
            yaw: 0.0,
            pitch: 0.0,
            dimension: Dimension::Nether,
        })
    );
    assert_eq!(
        messages(&output),
        [("block.minecraft.set_spawn".to_owned(), false)]
    );
    assert!(sounds_of(&output).contains(&sounds::BLOCK_RESPAWN_ANCHOR_SET_SPAWN));
    assert_eq!(scene.block(ANCHOR), anchor(2), "using it spends no charge");
    assert!(explosions(&output).is_empty());
    // The same spot again: nothing said, nothing heard.
    let output = scene.click(ANCHOR, Face::Top);
    assert!(messages(&output).is_empty(), "{:?}", messages(&output));
    assert!(!sounds_of(&output).contains(&sounds::BLOCK_RESPAWN_ANCHOR_SET_SPAWN));
}

#[test]
fn a_charged_anchor_explodes_in_the_overworld_and_the_end_with_fire_and_hurting_nobody() {
    for dimension in [Dimension::Overworld, Dimension::End] {
        let world = World::of(&[(ANCHOR, anchor(1))])
            .with_entities(&[stand(1, [6.5, -60.0, 3.5]), dirt(2, [-2.5, -60.0, 3.5])]);
        let mut scene = scene_in(dimension, &world, bare(), NIGHT, FEET);
        let output = scene.click(ANCHOR, Face::Top);
        let blasts = explosions(&output);
        assert_eq!(blasts.len(), 1, "{dimension:?}");
        let (centre, radius, broken, knockback) = &blasts[0];
        assert_eq!(*centre, [2.5, -59.5, 0.5]);
        assert_eq!(*radius, 5.0);
        assert!(*broken > 0);
        assert!(knockback.is_empty());
        assert_eq!(scene.block(ANCHOR), blocks::AIR, "{dimension:?}");
        assert_eq!(
            scene.block(BlockPos::new(2, -61, 0)),
            blocks::AIR,
            "the floor under it is blown away"
        );
        assert_eq!(respawn(&scene), None);
        assert!(fires(&scene.region) > 0, "{dimension:?}");
        assert!(present(&scene.region, 1) && present(&scene.region, 2));
    }
}

/// The anchor at [`ANCHOR`] with one charge used by an empty hand in `dimension`, with
/// `water` about it: the blocks its explosion reports, if it explodes.
fn anchor_blast_with(dimension: Dimension, water: &[(BlockPos, BlockState)]) -> Option<i32> {
    let mut placed = vec![(ANCHOR, anchor(1))];
    placed.extend_from_slice(water);
    let mut scene = scene_in(dimension, &World::of(&placed), bare(), NIGHT, FEET);
    let output = scene.click(ANCHOR, Face::Top);
    let blasts = explosions(&output);
    blasts.first().map(|(_, _, broken, _)| *broken)
}

#[test]
fn water_beside_or_above_an_exploding_anchor_keeps_its_blast_from_breaking_anything() {
    let east = BlockPos::new(3, -60, 0);
    let above = BlockPos::new(2, -59, 0);
    let flowing = |level: u8| state(&format!("minecraft:water[level={level}]"));
    for dimension in [Dimension::Overworld, Dimension::End] {
        // A source beside, flowing water of amount 7 (level 1) beside, or water above:
        // the anchor's own block resists as water, and every ray dies in it.
        for water in [
            vec![(east, blocks::WATER)],
            vec![(east, flowing(1))],
            vec![(above, flowing(7))],
        ] {
            assert_eq!(
                anchor_blast_with(dimension, &water),
                Some(0),
                "{dimension:?} {water:?}"
            );
        }
        // Flowing water of amount 1 (level 7) beside would not flow: the blast is whole.
        let thin = anchor_blast_with(dimension, &[(east, flowing(7))]);
        assert!(
            thin.is_some_and(|broken| broken > 0),
            "{dimension:?}: {thin:?}"
        );
        // Nor would flowing water with water under it.
        let fed = anchor_blast_with(
            dimension,
            &[
                (east, flowing(2)),
                (BlockPos::new(3, -61, 0), blocks::WATER),
            ],
        );
        assert!(
            fed.is_some_and(|broken| broken > 0),
            "{dimension:?}: {fed:?}"
        );
    }
    // In the Nether the anchor works and nothing explodes, water or not.
    assert_eq!(
        anchor_blast_with(Dimension::Nether, &[(east, blocks::WATER)]),
        None
    );
}

// ------------------------------------------------------------------------------------
// Across a pinned line.

#[test]
fn a_bed_s_explosion_at_a_line_files_blocks_beyond_it_with_fire_hurting_nobody_and_no_entity() {
    let head = BlockPos::new(14, -60, 4);
    let foot = BlockPos::new(14, -60, 5);
    let world = World::of(&[
        (head, bed("head")),
        (foot, bed("foot")),
        (BlockPos::new(16, -60, 4), blocks::DIRT),
    ])
    .with_entities(&[stand(1, [17.5, -60.0, 4.5])]);
    let mut west = scene_of(
        Dimension::Nether,
        Layout::side(true),
        &world,
        bare(),
        NIGHT,
        Vec3::new(12.5, -60.0, 4.5),
    );
    let output = west.click(head, Face::Top);
    assert_eq!(explosions(&output).len(), 1);
    let far: Vec<(BlockPos, Blast)> = output
        .effects
        .iter()
        .filter_map(|effect| match effect.what {
            Effected::Exploded { blast, .. } => Some((effect.at, blast)),
            _ => None,
        })
        .collect();
    assert!(
        far.iter().any(|(at, _)| *at == BlockPos::new(16, -60, 4)),
        "{far:?}"
    );
    for (at, blast) in &far {
        assert!(at.x >= 16);
        assert!(blast.fire && !blast.hurts, "{blast:?}");
        assert!(!blast.direct, "no entity's loot");
        assert!(blast.decay, "a block explosion's drops decay");
        assert_eq!(blast.owner, None);
        assert_eq!(blast.radius, Float(5.0));
    }
    let hits = output
        .effects
        .iter()
        .filter(|effect| matches!(effect.what, Effected::OnEntity { .. }))
        .count();
    assert_eq!(hits, 0, "the stand beyond the line is not hit");
}
