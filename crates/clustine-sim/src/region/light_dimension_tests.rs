//! Light by the region's level (`docs/groundwork/p-plan.md`, sections 17.2 D1, 17.5 and
//! 17.6): a level without a sky (the Nether) lights no sky, whether the region was made
//! anew, split off, merged, restored or cloned, and drops the sky's levels it is handed;
//! the End and the overworld have a sky. The blocks' light is the same in all three.
//!
//! Each world is 5×5 chunks about the origin on a stone floor 40 blocks above the
//! level's lowest block, with glowstone on it in the chunk at the origin.

use clustine_data::{BlockState, blocks};
use clustine_world::{
    BlockPos, Chunk, ChunkPos, Dimension, EdgeId, EntityIds, RegionId, SectionLight, Vec3,
};

use super::super::tnt::tests::{add_player, idle};
use super::super::{Holdings, Region, RegionConfig};
use crate::KeptLight;
use crate::api::{EdgeEvent, Inventory, TickInputs, Ticket};
use crate::state::RegionState;

pub(in crate::region) const DIMENSIONS: [Dimension; 3] =
    [Dimension::Overworld, Dimension::Nether, Dimension::End];

const PART: RegionId = RegionId(11);
const ABSORBED: RegionId = RegionId(12);

/// The sky's level in the open in each level, as the game has it: 15 under a sky, 0
/// where the level has none.
fn open_sky(dimension: Dimension) -> u8 {
    match dimension {
        Dimension::Overworld | Dimension::End => 15,
        Dimension::Nether => 0,
    }
}

/// The floor's top in `dimension`: a test's blocks stand on it, one higher.
pub(in crate::region) fn floor_y(dimension: Dimension) -> i32 {
    dimension.rules().min_y + 40
}

/// Glowstone on the floor in the chunk at the origin.
fn glowstone(dimension: Dimension) -> BlockPos {
    BlockPos::new(8, floor_y(dimension) + 1, 8)
}

/// The 5×5 chunks about the origin.
fn square() -> Vec<ChunkPos> {
    (-2..=2)
        .flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// A chunk of `dimension` with its stone floor and those of `placed` that are in it.
fn chunk_of(dimension: Dimension, position: ChunkPos, placed: &[(BlockPos, BlockState)]) -> Chunk {
    let mut chunk = Chunk::empty(dimension.rules(), clustine_world::Biome(0));
    let floor = floor_y(dimension);
    for z in 0..16 {
        for x in 0..16 {
            chunk.set(x, floor, z, blocks::STONE);
        }
    }
    for (pos, state) in placed.iter().filter(|(pos, _)| pos.chunk() == position) {
        let (x, z) = pos.in_chunk();
        chunk.set(x, pos.y, z, *state);
    }
    chunk
}

fn config(dimension: Dimension) -> RegionConfig {
    let rules = dimension.rules();
    RegionConfig {
        strongholds: Vec::new(),
        homes: Vec::new(),
        dimension,
        spawn: (dimension == Dimension::Overworld)
            .then(|| Vec3::new(0.5, f64::from(floor_y(dimension) + 1), 0.5)),
        starting_inventory: Inventory::default(),
        return_after: u64::MAX,
        lowest_y: rules.min_y,
        seed: 5,
        game_seed: 0,
        sea_level: 63,
        spawn_mobs: false,
        light: Default::default(),
        features: Default::default(),
        random_tick_speed: crate::region::RANDOM_TICK_SPEED,
    }
}

fn holding(held: &[ChunkPos]) -> (Holdings, Vec<(ChunkPos, Ticket)>) {
    let holdings = Holdings {
        held: held.to_vec(),
        pinned: Vec::new(),
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    let tickets = held
        .iter()
        .map(|position| (*position, Ticket::Viewer))
        .collect();
    (holdings, tickets)
}

/// A region of `dimension` that holds `held`, loaded with the floor and `placed`, not
/// ticked yet.
fn unticked(dimension: Dimension, held: &[ChunkPos], placed: &[(BlockPos, BlockState)]) -> Region {
    let chunks = held
        .iter()
        .map(|position| (*position, chunk_of(dimension, *position, placed)))
        .collect();
    let (holdings, tickets) = holding(held);
    let state = RegionState::new(EntityIds::block(0).expect("a block of ids"));
    Region::restore_loaded(
        config(dimension),
        state,
        holdings,
        &tickets,
        chunks,
        Vec::new(),
    )
}

/// The same, ticked once, so that its chunks are lit.
pub(in crate::region) fn world_in(
    dimension: Dimension,
    held: &[ChunkPos],
    placed: &[(BlockPos, BlockState)],
) -> Region {
    let mut region = unticked(dimension, held, placed);
    idle(&mut region);
    region
}

fn lit_world(dimension: Dimension) -> Region {
    world_in(
        dimension,
        &square(),
        &[(glowstone(dimension), blocks::GLOWSTONE)],
    )
}

/// The chunks `held` handed to `region` as a runner hands them back after a merge or a
/// split: tickets, then the chunks with the light kept for them.
fn handed_back(
    region: &mut Region,
    chunks: Vec<(ChunkPos, Chunk)>,
    kept: Vec<(ChunkPos, KeptLight)>,
) {
    let positions: Vec<ChunkPos> = chunks.iter().map(|(at, _)| *at).collect();
    region.tick(&TickInputs {
        tickets_added: positions.iter().map(|at| (*at, Ticket::Viewer)).collect(),
        ..TickInputs::default()
    });
    region.tick(&TickInputs {
        chunks_loaded: chunks,
        chunks_kept: kept,
        ..TickInputs::default()
    });
}

/// Asserts the light of `region` in `dimension` at the chunks `chunks`: the open sky's
/// level just above the floor, and nothing of the sky below it; and where the
/// glowstone's chunk is among them, its light beside it.
fn assert_light(region: &Region, dimension: Dimension, chunks: &[ChunkPos], what: &str) {
    let floor = floor_y(dimension);
    for chunk in chunks {
        for (x, z) in [(0, 0), (5, 11), (15, 15)] {
            let above = BlockPos::new(chunk.x * 16 + x, floor + 1, chunk.z * 16 + z);
            let light = region
                .light_at(above)
                .unwrap_or_else(|| panic!("{dimension:?}, {what}: {above:?} is lit"));
            assert_eq!(
                light.sky,
                open_sky(dimension),
                "{dimension:?}, {what}: the sky at {above:?}"
            );
            let high = BlockPos::new(above.x, floor + 100, above.z);
            assert_eq!(
                region.light_at(high).map(|light| light.sky),
                Some(open_sky(dimension)),
                "{dimension:?}, {what}: the sky at {high:?}"
            );
            let below = BlockPos::new(above.x, floor - 1, above.z);
            assert_eq!(
                region.light_at(below).map(|light| light.sky),
                Some(0),
                "{dimension:?}, {what}: under the floor at {below:?}"
            );
        }
    }
    let lamp = glowstone(dimension);
    if chunks.contains(&lamp.chunk()) {
        let beside = BlockPos::new(lamp.x + 1, lamp.y, lamp.z);
        assert_eq!(
            region.light_at(beside).map(|light| light.block),
            Some(14),
            "{dimension:?}, {what}: beside the glowstone"
        );
    }
}

#[test]
fn a_region_lights_the_sky_of_the_overworld_and_the_end_and_none_in_the_nether() {
    for dimension in DIMENSIONS {
        let region = lit_world(dimension);
        assert_eq!(region.light.sky, dimension != Dimension::Nether);
        assert_light(&region, dimension, &square(), "made");
    }
}

#[test]
fn a_region_split_off_lights_no_sky_in_the_nether_and_the_sky_elsewhere() {
    // Not the End: its land here is the fight's arena, which no split takes
    // (`docs/groundwork/p-plan.md`, section 19.5 item 6); the overworld is the sky's.
    for dimension in DIMENSIONS
        .into_iter()
        .filter(|dimension| *dimension != Dimension::End)
    {
        let mut region = lit_world(dimension);
        let floor = f64::from(floor_y(dimension) + 1);
        // The player's edge, which the split tells of who went.
        region.tick(&TickInputs {
            edges: vec![EdgeEvent::Started {
                edge: EdgeId(1),
                start: 1,
            }],
            ..TickInputs::default()
        });
        add_player(&mut region, 1, [40.5, floor, 8.5], false);
        idle(&mut region);
        let kept = region.hand_over_light();
        let splitting = region
            .split(&[ChunkPos::new(2, 0)], PART, &[])
            .expect("a split");
        let gone = splitting.chunks.clone();
        assert!(gone.contains(&ChunkPos::new(2, 0)), "{gone:?}");
        let (stayed, mut part) = region.take_split(splitting, &[]);
        let kept_of = |chunks: &[(ChunkPos, Chunk)]| -> Vec<(ChunkPos, KeptLight)> {
            kept.iter()
                .filter(|(at, _)| chunks.iter().any(|(chunk, _)| chunk == at))
                .cloned()
                .collect()
        };
        let part_kept = kept_of(&part.chunks);
        let part_chunks = std::mem::take(&mut part.chunks);
        handed_back(&mut part.region, part_chunks, part_kept);
        assert_light(&part.region, dimension, &gone, "the part of a split");
        let stayed_kept = kept_of(&stayed);
        let held: Vec<ChunkPos> = stayed.iter().map(|(at, _)| *at).collect();
        handed_back(&mut region, stayed, stayed_kept);
        assert_light(&region, dimension, &held, "what stayed after a split");
    }
}

#[test]
fn a_merged_region_lights_no_sky_in_the_nether_and_the_sky_elsewhere() {
    for dimension in DIMENSIONS {
        let ours: Vec<ChunkPos> = square().into_iter().filter(|at| at.x <= 0).collect();
        let theirs: Vec<ChunkPos> = square().into_iter().filter(|at| at.x > 0).collect();
        let placed = [(glowstone(dimension), blocks::GLOWSTONE)];
        let mut survivor = world_in(dimension, &ours, &placed);
        let mut absorbed = world_in(dimension, &theirs, &placed);
        let mut kept = survivor.hand_over_light();
        kept.extend(absorbed.hand_over_light());
        let their_chunks: Vec<(ChunkPos, Chunk)> = theirs
            .iter()
            .map(|at| (*at, absorbed.saved_chunk(*at).expect("loaded")))
            .collect();
        let merged = survivor.absorb(ABSORBED, &absorbed.state());
        let mut chunks = survivor.take_absorbed(merged, &theirs, &[], &[], &[]);
        chunks.extend(their_chunks);
        handed_back(&mut survivor, chunks, kept);
        assert_light(&survivor, dimension, &square(), "merged");
    }
}

/// A restore, a restore handed the light kept before it, and a clone: each lights its
/// chunks anew for its own level.
#[test]
fn a_restored_or_cloned_region_lights_no_sky_in_the_nether_and_the_sky_elsewhere() {
    for dimension in DIMENSIONS {
        let mut region = lit_world(dimension);
        let kept = region.hand_over_light();
        let chunks: Vec<(ChunkPos, Chunk)> = square()
            .into_iter()
            .map(|at| (at, region.saved_chunk(at).expect("loaded")))
            .collect();
        let (holdings, tickets) = holding(&square());
        let mut restored = Region::restore_loaded(
            region.config.clone(),
            region.state(),
            holdings.clone(),
            &tickets,
            chunks.clone(),
            Vec::new(),
        );
        idle(&mut restored);
        assert_light(&restored, dimension, &square(), "restored");

        let mut adopting = Region::restore_loaded(
            region.config.clone(),
            region.state(),
            holdings,
            &tickets,
            chunks,
            Vec::new(),
        );
        adopting.tick(&TickInputs {
            chunks_kept: kept,
            ..TickInputs::default()
        });
        assert_light(
            &adopting,
            dimension,
            &square(),
            "restored with its light kept",
        );

        let mut clone = region.clone();
        assert_eq!(clone.light.sky, dimension != Dimension::Nether);
        idle(&mut clone);
        assert_light(&clone, dimension, &square(), "cloned");
    }
}

/// Levels with a sky's handed to a Nether region (as levels saved before P11 could
/// be) lose the sky's as they come, and are adopted with the blocks' light alone.
#[test]
fn the_skys_levels_handed_to_a_nether_region_are_dropped() {
    let placed = [(glowstone(Dimension::End), blocks::GLOWSTONE)];
    let mut end = world_in(Dimension::End, &square(), &placed);
    let kept = end.hand_over_light();
    assert!(kept.iter().any(|(_, kept)| {
        kept.light
            .sky
            .iter()
            .any(|section| *section != SectionLight::Absent)
    }));

    // Taken in before the region's first step, when its light is still a sky's.
    let mut nether = unticked(Dimension::Nether, &square(), &placed);
    nether.take_kept_light(&kept);
    assert!(!nether.light.sky);
    assert_eq!(nether.light.kept.len(), 25);
    for (at, kept) in &nether.light.kept {
        assert!(
            kept.light
                .sky
                .iter()
                .all(|section| *section == SectionLight::Absent),
            "{at:?}"
        );
        assert!(
            kept.light
                .block
                .iter()
                .any(|section| *section != SectionLight::Absent),
            "{at:?} kept its blocks' light"
        );
    }
    idle(&mut nether);
    assert_light(
        &nether,
        Dimension::Nether,
        &square(),
        "adopting the End's levels",
    );
    // Adopted, not lit whole: the blocks' levels were the End's.
    assert!(nether.light.kept.is_empty());

    // And the Nether's own levels carry no sky to begin with.
    let mut lit = lit_world(Dimension::Nether);
    for (at, kept) in lit.hand_over_light() {
        assert!(
            kept.light
                .sky
                .iter()
                .all(|section| *section == SectionLight::Absent),
            "{at:?}"
        );
    }
}
