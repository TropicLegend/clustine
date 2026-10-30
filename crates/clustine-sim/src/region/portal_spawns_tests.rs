//! Family E1 of P12 in a region of each dimension (`docs/groundwork/p-plan.md`,
//! section 18.3; the brief's sections 4.2, 5.8 and 5.9): a portal block's random tick
//! making a zombified piglin in the overworld and never in the Nether or the End; and in
//! the Nether, both types made by an egg and by natural spawning where the biomes' lists
//! name them, and a magma cube's split at its death. Declared in `portal_spawns.rs`.

use clustine_data::{BlockState, attributes as ids, blocks, entity_types, items};
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkPos, Dimension, Entity, EntityChange, EntityId, EntityIds, Vec3,
};

use super::super::tnt::tests::add_player;
use super::super::{Holdings, Random, Region, RegionConfig};
use crate::api::{Inventory, PublishedCounts, TickInputs, TickOutput, Ticket};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::mobs::types::cube_mob;
use crate::spawning::rules;
use crate::state::RegionState;

const PIGLIN: i32 = entity_types::ZOMBIFIED_PIGLIN;
const MAGMA: i32 = entity_types::MAGMA_CUBE;

/// The top of the stone floor in `dimension`: three above its lowest block.
fn floor(dimension: Dimension) -> i32 {
    dimension.rules().min_y + 3
}

fn biome(name: &str) -> Biome {
    rules::biome_named(name).unwrap_or_else(|| panic!("{name} is a biome"))
}

/// A chunk of `dimension` in `biome` with a floor of `ground` and `blocks` set.
fn chunk_of(
    dimension: Dimension,
    biome: Biome,
    ground: BlockState,
    position: ChunkPos,
    blocks: &[(BlockPos, BlockState)],
) -> Chunk {
    let mut chunk = Chunk::empty(dimension.rules(), biome);
    for z in 0..16 {
        for x in 0..16 {
            chunk.set(x, floor(dimension), z, ground);
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

/// A region of `dimension` holding the 5×5 chunks about the origin's, in `biome` on a
/// floor of `ground` with `blocks` set, ticked once so that the middle 3×3 tick.
fn region_in(
    dimension: Dimension,
    biome: Biome,
    ground: BlockState,
    blocks: &[(BlockPos, BlockState)],
) -> Region {
    let held: Vec<ChunkPos> = (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect();
    let chunks = held
        .iter()
        .map(|position| {
            let chunk = chunk_of(dimension, biome, ground, *position, blocks);
            (*position, chunk)
        })
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

/// The region's loaded entities of the type `id`.
fn of_kind(region: &Region, id: i32) -> Vec<Entity> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter().cloned())
        .filter(|entity| i32::from(entity.kind) == id)
        .collect()
}

/// The main hand's item of the mob `entity`.
fn held(entity: &Entity) -> Option<i32> {
    let mob = Mob::decode(&entity.data).expect("a mob's record");
    mob.body.equipment.slots[usize::from(slots::MAIN_HAND)]
        .as_ref()
        .map(|stack| stack.item)
}

// ---------------------------------------------------------------------------------
// The portal's zombified piglins.

/// Where a test's portal stands: a column of three portal blocks on the floor at
/// (2, 2), its top block the one ticked.
fn portal(dimension: Dimension, ground: BlockState) -> (Region, BlockPos) {
    let base = floor(dimension);
    let mut set: Vec<(BlockPos, BlockState)> = (1..=3)
        .map(|dy| (BlockPos::new(2, base + dy, 2), blocks::NETHER_PORTAL))
        .collect();
    set.push((BlockPos::new(2, base, 2), ground));
    let region = region_in(dimension, biome("minecraft:plains"), blocks::STONE, &set);
    (region, BlockPos::new(2, base + 3, 2))
}

/// A random tick's stream whose first `nextInt(2000)` is 0: one in 2000 passes at easy.
fn lucky() -> Random {
    (0..)
        .map(|seed| Random::of_entity(seed, EntityId(0), 0))
        .find(|random| random.clone().next_int(2000) == 0)
        .expect("some stream draws 0")
}

/// One of the region's random ticks of its portal block `top`, of `random`: the
/// zombified piglins made, and what was drawn.
fn tick_portal(region: &mut Region, top: BlockPos, mut random: Random) -> (Vec<Entity>, u64) {
    let start = random.clone();
    let mut output = TickOutput::default();
    let state = region.block(top).expect("the portal is read");
    assert!(region.portal_random_tick(top, state, &mut random, &mut output));
    (of_kind(region, PIGLIN), random.draws_since(&start))
}

/// The brief's 4.2: in the overworld, one random tick in 2000 at easy, a player near,
/// a zombified piglin made on the block under the portal's column, at its middle, on
/// its portal cooldown of 300, holding its golden weapon.
#[test]
fn a_portal_in_the_overworld_makes_a_zombified_piglin_under_its_column_on_cooldown() {
    let (mut region, top) = portal(Dimension::Overworld, blocks::STONE);
    let feet = f64::from(floor(Dimension::Overworld) + 1);
    add_player(&mut region, 1, [8.5, feet, 8.5], false);
    let (made, drawn) = tick_portal(&mut region, top, lucky());
    assert_eq!(made.len(), 1, "one made");
    let piglin = &made[0];
    assert_eq!(
        piglin.position,
        [2.5, feet, 2.5],
        "on the floor under the column"
    );
    assert_eq!(piglin.portal.cooldown, 300);
    assert!(matches!(
        held(piglin),
        Some(items::GOLDEN_SWORD | items::GOLDEN_SPEAR)
    ));
    assert!(
        drawn > 2,
        "the chance, the yaw and its finalizeSpawn: {drawn}"
    );
    // A stream that draws anything but 0 makes none, and draws only its chance.
    let mut unlucky = Random::of_entity(1, EntityId(0), 0);
    while unlucky.clone().next_int(2000) == 0 {
        unlucky = Random::of_entity(2, EntityId(0), 0);
    }
    let (made, drawn) = tick_portal(&mut region, top, unlucky);
    assert_eq!(made.len(), 1, "no second");
    assert_eq!(drawn, 1);
}

/// `nether_portal_spawns_piglin` is the overworld's alone: a portal in the Nether or
/// the End makes none, and draws nothing for it.
#[test]
fn a_portal_in_the_nether_or_the_end_makes_none_and_draws_nothing() {
    for dimension in [Dimension::Nether, Dimension::End] {
        let (mut region, top) = portal(dimension, blocks::STONE);
        let feet = f64::from(floor(dimension) + 1);
        add_player(&mut region, 1, [8.5, feet, 8.5], false);
        let (made, drawn) = tick_portal(&mut region, top, lucky());
        assert!(made.is_empty(), "{dimension:?}");
        assert_eq!(drawn, 0, "{dimension:?}");
    }
}

/// No player within 128 blocks of the chunk's middle: none, after the chance alone.
#[test]
fn a_portal_far_from_every_player_makes_none() {
    let (mut region, top) = portal(Dimension::Overworld, blocks::STONE);
    let feet = f64::from(floor(Dimension::Overworld) + 1);
    add_player(&mut region, 1, [300.5, feet, 8.5], false);
    let (made, drawn) = tick_portal(&mut region, top, lucky());
    assert!(made.is_empty());
    assert_eq!(drawn, 1);
}

/// The block under the column must be one a zombified piglin may spawn on: not glass.
#[test]
fn a_portal_on_glass_makes_none() {
    let (mut region, top) = portal(Dimension::Overworld, blocks::GLASS);
    let feet = f64::from(floor(Dimension::Overworld) + 1);
    add_player(&mut region, 1, [8.5, feet, 8.5], false);
    let (made, _) = tick_portal(&mut region, top, lucky());
    assert!(made.is_empty());
}

/// `spawn_mobs` off: none, and nothing drawn.
#[test]
fn a_portal_spawns_nothing_where_mobs_do_not_spawn() {
    let (mut region, top) = portal(Dimension::Overworld, blocks::STONE);
    region.config.spawn_mobs = false;
    let feet = f64::from(floor(Dimension::Overworld) + 1);
    add_player(&mut region, 1, [8.5, feet, 8.5], false);
    let (made, drawn) = tick_portal(&mut region, top, lucky());
    assert!(made.is_empty());
    assert_eq!(drawn, 0);
}

/// A block that is no portal is not the rule's.
#[test]
fn a_random_tick_of_another_block_is_not_the_portal_s() {
    let (mut region, top) = portal(Dimension::Overworld, blocks::STONE);
    let mut random = lucky();
    let mut output = TickOutput::default();
    assert!(!region.portal_random_tick(top, blocks::STONE, &mut random, &mut output));
}

// ---------------------------------------------------------------------------------
// The Nether's.

fn nether(name: &str, ground: BlockState) -> Region {
    region_in(Dimension::Nether, biome(name), ground, &[])
}

/// An egg makes each in the Nether: the zombified piglin with its golden weapon, the
/// magma cube of size 1, 2 or 4 with its armour.
#[test]
fn an_egg_makes_a_zombified_piglin_and_a_magma_cube_in_the_nether() {
    let mut region = nether("minecraft:nether_wastes", blocks::NETHERRACK);
    let base = floor(Dimension::Nether);
    let (player, _) = add_player(&mut region, 1, [0.5, f64::from(base + 1), 0.5], false);
    let mut output = TickOutput::default();
    let pos = BlockPos::new(4, base + 1, 4);
    let piglin = region.spawn_mob(PIGLIN as u16, player, pos, false, false, &mut output);
    assert!(piglin.is_some());
    let made = of_kind(&region, PIGLIN);
    assert_eq!(made.len(), 1);
    assert!(matches!(
        held(&made[0]),
        Some(items::GOLDEN_SWORD | items::GOLDEN_SPEAR)
    ));
    let pos = BlockPos::new(-4, base + 1, 4);
    let cube = region.spawn_mob(MAGMA as u16, player, pos, false, false, &mut output);
    assert!(cube.is_some());
    let made = of_kind(&region, MAGMA);
    assert_eq!(made.len(), 1);
    let mob = Mob::decode(&made[0].data).expect("a mob");
    let size = cube_mob::size(&mob);
    assert!([1, 2, 4].contains(&size));
    let armour = mob
        .body
        .attributes
        .value(MAGMA as u16, attribute(ids::ARMOR));
    assert_eq!(armour, f64::from(size * 3));
}

/// Ticks `region` with player 1 at its middle and the others' counts published, until
/// a mob of the type `id` is made: whether one was within `limit` ticks.
fn spawns_naturally(region: &mut Region, id: i32, limit: u32) -> bool {
    let base = floor(Dimension::Nether);
    add_player(region, 1, [8.5, f64::from(base + 1), 8.5], false);
    let mut inputs = TickInputs {
        counts: Some(PublishedCounts::default()),
        ..TickInputs::default()
    };
    for _ in 0..limit {
        region.tick(&inputs);
        inputs = TickInputs::default();
        if !of_kind(region, id).is_empty() {
            return true;
        }
    }
    false
}

/// Natural spawning makes zombified piglins in the nether wastes, whose list names them
/// at a weight of 100.
#[test]
fn zombified_piglins_spawn_naturally_in_the_nether_wastes() {
    let mut region = nether("minecraft:nether_wastes", blocks::NETHERRACK);
    assert!(spawns_naturally(&mut region, PIGLIN, 2_000));
}

/// Natural spawning makes magma cubes in the basalt deltas, whose list names them at a
/// weight of 100.
#[test]
fn magma_cubes_spawn_naturally_in_the_basalt_deltas() {
    let mut region = nether("minecraft:basalt_deltas", blocks::BASALT);
    assert!(spawns_naturally(&mut region, MAGMA, 2_000));
}

/// No zombified piglin spawns on nether wart blocks: in the nether wastes on them, by
/// the time a magma cube (of a weight of 2 to the zombified piglin's 100) has spawned,
/// none has.
#[test]
fn no_zombified_piglin_spawns_on_nether_wart_blocks() {
    let mut region = nether("minecraft:nether_wastes", blocks::NETHER_WART_BLOCK);
    assert!(spawns_naturally(&mut region, MAGMA, 20_000));
    assert!(of_kind(&region, PIGLIN).is_empty());
}

/// A magma cube of size 4 at the end of its death splits into two to four of size 2,
/// each with its armour of 6 and full health.
#[test]
fn a_dying_magma_cube_splits_into_two_to_four_of_half_its_size() {
    let mut region = nether("minecraft:basalt_deltas", blocks::BASALT);
    let kind = MAGMA as u16;
    let mut mob = Mob::fresh();
    cube_mob::set_size(kind, &mut mob, 4, true);
    mob.body.vitals.health = 0.0;
    mob.body.vitals.died_from = Some(0);
    let feet = f64::from(floor(Dimension::Nether) + 1);
    let mut entity = crate::mobs::making::fresh_record(
        EntityId(500),
        kind,
        uuid::Uuid::from_u128(500),
        [0.5, feet, 0.5],
        0.0,
        &mob,
    );
    entity.lived = 19;
    entity.on_ground = true;
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .expect("the chunk is loaded")
        .apply_entity(&EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
    region.tick(&TickInputs::default());
    assert!(
        region.find_entity(EntityId(500)).is_none(),
        "the parent is gone"
    );
    let made = of_kind(&region, MAGMA);
    assert!((2..=4).contains(&made.len()), "{} made", made.len());
    for cube in &made {
        let mob = Mob::decode(&cube.data).expect("a mob");
        assert_eq!(cube_mob::size(&mob), 2);
        assert_eq!(mob.body.vitals.health, 4.0);
        assert_eq!(mob.body.attributes.value(kind, attribute(ids::ARMOR)), 6.0);
    }
}
