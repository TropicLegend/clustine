//! The rules of P11's family D2 in a region of each dimension
//! (`docs/groundwork/p-plan.md`, 17.2 D2 with 17.5 and 17.6): fire's infiniburn tag, no
//! raid begun where `can_start_raid` is false, the custom spawners in the overworld
//! only and from its players alone, a snow golem melting by its dimension, and chorus
//! fruit's heights. Declared in `custom_spawners.rs`.

use clustine_data::{BlockState, blocks, entity_types};
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkPos, Dimension, EntityId, EntityIds, PlayerId, Vec3,
};
use uuid::Uuid;

use super::super::tnt::tests::add_player;
use super::super::{Holdings, Random, Region, RegionConfig};
use crate::api::{Inventory, TickInputs, TickOutput, Ticket, WorldPlayer};
use crate::mobs::Mob;
use crate::mobs::body::effects::{Effect, Layer};
use crate::spawning::rules;
use crate::state::RegionState;

/// The top of the stone floor in `dimension`: three above its lowest block.
fn floor(dimension: Dimension) -> i32 {
    dimension.rules().min_y + 3
}

/// Where a test's blocks stand in `dimension`, `dy` above the floor.
fn above(dimension: Dimension, x: i32, dy: i32, z: i32) -> BlockPos {
    BlockPos::new(x, floor(dimension) + dy, z)
}

fn biome(name: &str) -> Biome {
    rules::biome_named(name).unwrap_or_else(|| panic!("{name} is a biome"))
}

/// A chunk of `dimension` in the biome `biome` with a stone floor and `blocks` of it
/// set.
fn chunk_of(
    dimension: Dimension,
    biome: Biome,
    position: ChunkPos,
    blocks: &[(BlockPos, BlockState)],
) -> Chunk {
    let mut chunk = Chunk::empty(dimension.rules(), biome);
    for z in 0..16 {
        for x in 0..16 {
            chunk.set(x, floor(dimension), z, blocks::STONE);
        }
    }
    for (at, state) in blocks.iter().filter(|(at, _)| at.chunk() == position) {
        let (x, z) = at.in_chunk();
        chunk.set(x, at.y, z, *state);
    }
    chunk
}

fn config(dimension: Dimension) -> RegionConfig {
    let rules = dimension.rules();
    RegionConfig {
        strongholds: Vec::new(),
        homes: Vec::new(),
        dimension,
        spawn: Some(Vec3::new(0.5, f64::from(floor(dimension) + 1), 0.5)),
        starting_inventory: Inventory::default(),
        return_after: u64::MAX,
        lowest_y: rules.min_y,
        seed: 7,
        game_seed: 0,
        sea_level: match dimension {
            Dimension::Overworld => 63,
            Dimension::Nether => 32,
            Dimension::End => 0,
        },
        spawn_mobs: true,
        light: Default::default(),
        features: Default::default(),
        random_tick_speed: crate::region::RANDOM_TICK_SPEED,
    }
}

/// A region of `dimension` holding the 5×5 chunks about the origin's, all loaded in the
/// biome `biome` with `blocks` set, ticked once so that the middle 3×3 tick.
fn region_in(dimension: Dimension, biome: Biome, blocks: &[(BlockPos, BlockState)]) -> Region {
    let held: Vec<ChunkPos> = (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect();
    let chunks = held
        .iter()
        .map(|position| (*position, chunk_of(dimension, biome, *position, blocks)))
        .collect();
    let tickets: Vec<(ChunkPos, Ticket)> = held
        .iter()
        .map(|position| (*position, Ticket::Viewer))
        .collect();
    let holdings = Holdings {
        held,
        pinned: Vec::new(),
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    let state = RegionState::new(EntityIds::block(0).expect("a block of ids"));
    let mut region = Region::restore_loaded(
        config(dimension),
        state,
        holdings,
        &tickets,
        chunks,
        Vec::new(),
    );
    region.tick(&TickInputs::default());
    region
}

fn plains_in(dimension: Dimension, blocks: &[(BlockPos, BlockState)]) -> Region {
    region_in(dimension, biome("minecraft:plains"), blocks)
}

fn is_fire(state: Option<BlockState>) -> bool {
    state.is_some_and(|state| clustine_blocks::data::is_block(state, blocks::FIRE))
}

// ---------------------------------------------------------------------------------
// Fire's infiniburn tag.

/// Whether fire of age 4 set on `ground` in `dimension`, with nothing that burns about
/// it, is there still after 200 ticks: fire past age 3 where it cannot spread goes at its
/// next tick unless its ground is of the dimension's infiniburn tag.
fn fire_on_lasts(dimension: Dimension, ground: BlockState) -> bool {
    let below = above(dimension, 2, 0, 2);
    let at = above(dimension, 2, 1, 2);
    let mut region = plains_in(dimension, &[(below, ground)]);
    let fire = BlockState::parse("minecraft:fire[age=4]").expect("a block state");
    region.set_block(at, fire, &mut TickOutput::default());
    assert!(is_fire(region.block(at)), "{dimension:?}: fire is set");
    for _ in 0..200 {
        region.tick(&TickInputs::default());
    }
    is_fire(region.block(at))
}

#[test]
fn fire_on_netherrack_and_magma_lasts_in_every_dimension() {
    for dimension in Dimension::ALL {
        assert!(
            fire_on_lasts(dimension, blocks::NETHERRACK),
            "{dimension:?}"
        );
        assert!(
            fire_on_lasts(dimension, blocks::MAGMA_BLOCK),
            "{dimension:?}"
        );
    }
}

#[test]
fn fire_on_bedrock_lasts_in_the_end_only() {
    assert!(!fire_on_lasts(Dimension::Overworld, blocks::BEDROCK));
    assert!(!fire_on_lasts(Dimension::Nether, blocks::BEDROCK));
    assert!(fire_on_lasts(Dimension::End, blocks::BEDROCK));
}

#[test]
fn fire_on_stone_burns_out_in_every_dimension() {
    for dimension in Dimension::ALL {
        assert!(!fire_on_lasts(dimension, blocks::STONE), "{dimension:?}");
    }
}

// ---------------------------------------------------------------------------------
// Raids.

/// A region of `dimension` with the player 1 standing on its floor, whose raid omen of
/// amplifier 1 is in its last tick, kept at `place`; its effects ticked once.
fn raid_omen_ending_in(dimension: Dimension) -> (Region, PlayerId, BlockPos) {
    let place = above(dimension, 8, 1, 8);
    let mut region = plains_in(dimension, &[]);
    let feet = [8.5, f64::from(place.y), 8.5];
    let (id, _) = add_player(&mut region, 1, feet, false);
    let mut output = TickOutput::default();
    let now = region.tick;
    region.add_player_effect(id, Effect::RAID_OMEN, Layer::new(1, 0, now), &mut output);
    region
        .players
        .get_mut(&id)
        .expect("the player is here")
        .raid_omen_position = Some(place);
    region.tick_player_effects(&mut output);
    (region, id, place)
}

#[test]
fn a_raid_omen_ending_starts_a_raid_in_the_overworld_and_the_end_and_none_in_the_nether() {
    for dimension in Dimension::ALL {
        let (region, id, place) = raid_omen_ending_in(dimension);
        let raids = region.own_raids();
        if dimension == Dimension::Nether {
            assert!(raids.is_empty(), "no raid is begun in the Nether");
        } else {
            assert_eq!(raids.len(), 1, "{dimension:?}");
            assert_eq!(raids[0].1.centre, place, "{dimension:?}");
            assert_eq!(raids[0].1.omen_level, 2, "{dimension:?}");
        }
        // The omen ends either way, its place forgotten.
        let player = &region.players[&id];
        assert!(!player.effects.has(Effect::RAID_OMEN), "{dimension:?}");
        assert_eq!(player.raid_omen_position, None, "{dimension:?}");
    }
}

#[test]
fn only_the_nether_refuses_a_raid() {
    let refuses: Vec<bool> = Dimension::ALL
        .into_iter()
        .map(|dimension| !plains_in(dimension, &[]).can_start_raid())
        .collect();
    assert_eq!(refuses, [false, true, false]);
}

// ---------------------------------------------------------------------------------
// The custom spawners.

/// The custom spawners' phase of a region of `dimension` whose previous tick was at game
/// time 23 999 and which is now at 24 000, when the wandering trader's chain fires.
fn spawners_at_the_trader_s_firing(dimension: Dimension) -> (Region, TickOutput) {
    let mut region = plains_in(dimension, &[]);
    region.faces.world = Some(Vec::new());
    region.clock.game_time = 24_000;
    region.custom_spawners.last = Some(23_999);
    let mut output = TickOutput::default();
    region.run_custom_spawners(&mut output);
    (region, output)
}

#[test]
fn the_custom_spawners_run_in_the_overworld_only() {
    for dimension in Dimension::ALL {
        let (region, output) = spawners_at_the_trader_s_firing(dimension);
        if dimension == Dimension::Overworld {
            // With no player in the world the trader's firing that passes sets the
            // chance back to 25, and either way the store is told it.
            assert!(output.trader_chance.is_some(), "{dimension:?}");
            assert_eq!(region.custom_spawners.last, Some(24_000));
        } else {
            assert_eq!(output.trader_chance, None, "{dimension:?}");
            assert!(output.events.is_empty(), "{dimension:?}");
            assert_eq!(region.custom_spawners.last, Some(23_999), "{dimension:?}");
            assert_eq!(region.custom_spawners.trader_chance, None, "{dimension:?}");
        }
    }
}

fn world_player(number: u128, dimension: Dimension) -> WorldPlayer {
    WorldPlayer {
        player: PlayerId(Uuid::from_u128(number)),
        entity: EntityId(1_000_000 + u64::try_from(number).expect("a small number")),
        dimension,
        position: [0.5, 70.0, 0.5],
        game_mode: 1,
        sleeping: None,
    }
}

#[test]
fn the_spawners_choose_from_the_overworld_s_players_only() {
    let mut region = plains_in(Dimension::Overworld, &[]);
    region.faces.world = Some(vec![
        world_player(3, Dimension::End),
        world_player(2, Dimension::Overworld),
        world_player(4, Dimension::Nether),
        world_player(1, Dimension::Overworld),
    ]);
    let players = region.level_players().expect("the table is known");
    assert_eq!(
        players,
        [PlayerId(Uuid::from_u128(1)), PlayerId(Uuid::from_u128(2))]
    );
    // With every player elsewhere, the overworld's spawners find none at all.
    region.faces.world = Some(vec![
        world_player(3, Dimension::End),
        world_player(4, Dimension::Nether),
    ]);
    let mut stream = Random::of_schedule(7, 99, 0);
    assert_eq!(region.random_world_player(&mut stream), Some(None));
}

// ---------------------------------------------------------------------------------
// The snow golem.

/// The health of a snow golem without AI after one tick on the floor of `dimension` in
/// the biome `biome`, from its full 4.
fn snow_golem_health_after_a_tick(dimension: Dimension, biome_name: &str) -> f32 {
    let mut region = region_in(dimension, biome(biome_name), &[]);
    let mut golem = Mob::fresh();
    golem.body.flags.no_ai = true;
    golem.body.vitals.health = 4.0;
    let at = [4.5, f64::from(floor(dimension) + 1), 4.5];
    let mut entity =
        super::super::tnt::tests::record(&mut region, entity_types::SNOW_GOLEM, at, golem.encode());
    entity.on_ground = true;
    let key = super::super::tnt::tests::add(&mut region, entity);
    region.tick(&TickInputs::default());
    let (_, entity) = region.find_entity(key).expect("the golem is here");
    Mob::decode(&entity.data)
        .expect("a mob's record")
        .body
        .vitals
        .health
}

#[test]
fn a_snow_golem_melts_in_the_nether_whatever_its_biome_and_elsewhere_by_its_biome() {
    for dimension in Dimension::ALL {
        let melted = dimension == Dimension::Nether;
        let health = snow_golem_health_after_a_tick(dimension, "minecraft:plains");
        assert_eq!(health < 4.0, melted, "{dimension:?} in plains: {health}");
        let health = snow_golem_health_after_a_tick(dimension, "minecraft:desert");
        assert!(health < 4.0, "{dimension:?} in a desert: {health}");
    }
}

// ---------------------------------------------------------------------------------
// Chorus fruit.

#[test]
fn chorus_fruit_aims_within_each_level_s_logical_height() {
    let heights: Vec<(f64, f64)> = Dimension::ALL
        .into_iter()
        .map(|dimension| plains_in(dimension, &[]).chorus_heights())
        .collect();
    assert_eq!(heights, [(-64.0, 319.0), (0.0, 127.0), (0.0, 255.0)]);
}

#[test]
fn chorus_fruit_eaten_in_the_nether_under_its_roof_stays_under_it() {
    // A bedrock roof at 127 and a stone floor at 100, as the Nether's ceiling: a try
    // aimed above 127 would find the roof's top to stand on. From 124 a try of 16 reaches
    // up to 132, so a third or so of them would.
    assert!(clustine_blocks::data::in_tag(
        blocks::BEDROCK,
        "minecraft:entities_can_teleport_to"
    ));
    let mut blocks = Vec::new();
    for x in -12..=12 {
        for z in -12..=12 {
            blocks.push((BlockPos::new(x, 127, z), blocks::BEDROCK));
            blocks.push((BlockPos::new(x, 100, z), blocks::STONE));
        }
    }
    for seed in 0..40 {
        let mut region = plains_in(Dimension::Nether, &blocks);
        let (id, _) = add_player(&mut region, 1, [0.5, 124.0, 0.5], false);
        let mut random = Random::of_schedule(seed, 5, 0);
        let mut output = TickOutput::default();
        let moved = region.chorus_teleport(id, 16.0, &mut random, &mut output);
        let y = region.players[&id].pose.position.y;
        assert!(moved, "seed {seed}");
        assert!(y < 127.0, "seed {seed}: reached y {y}");
    }
}
