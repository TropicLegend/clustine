//! Light and growth at a pinned line (the plan's section 5): each region lights its own
//! chunks and its copies from the blocks it has, so the chunks each side ticks have the
//! light one region with the same blocks has; a torch beyond the line lights a crop on
//! this side in the tick its copy is updated; a mushroom's walk and a grass block's try
//! that land beyond the line are conditional sets the holder carries out.

use std::collections::BTreeSet;

use clustine_world::{Change, Due, EffectId, Effected, FiledEffect};

use super::plants::{successes, walk};
use super::*;

const WEST: ChunkArea = ChunkArea {
    min_x: None,
    max_x: Some(1),
};
const EAST: ChunkArea = ChunkArea {
    min_x: Some(1),
    max_x: None,
};

/// The land: chunks from x = −1 to 2 and z = −1 to 1, the floor with `placed`.
fn land(placed: &[(BlockPos, BlockState)]) -> Vec<(ChunkPos, Chunk)> {
    (-1..=2)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .map(|position| {
            let mut chunk = floor();
            for (pos, state) in placed.iter().filter(|(pos, _)| pos.chunk() == position) {
                let (x, z) = pos.in_chunk();
                chunk.set(x, pos.y, z, *state);
            }
            (position, chunk)
        })
        .collect()
}

/// A region pinned to `area` holding the land in it, and copying the column of the land
/// beside it across the line; or, with no area, one region holding all the land.
fn region_of(area: Option<ChunkArea>, ids: u32, land: &[(ChunkPos, Chunk)]) -> Region {
    let owns = |position: ChunkPos| area.is_none_or(|area| area.contains(position));
    let near = |position: ChunkPos| match area {
        Some(area) if area == WEST => position.x == 1,
        Some(_) => position.x == 0,
        None => false,
    };
    let chunks: Vec<(ChunkPos, Chunk)> = land
        .iter()
        .filter(|(position, _)| owns(*position))
        .cloned()
        .collect();
    let copies: Vec<(ChunkPos, Chunk)> = land
        .iter()
        .filter(|(position, _)| near(*position))
        .cloned()
        .collect();
    let tickets: Vec<(ChunkPos, Ticket)> = chunks
        .iter()
        .map(|(position, _)| (*position, Ticket::Viewer))
        .collect();
    let holdings = Holdings {
        held: chunks.iter().map(|(position, _)| *position).collect(),
        pinned: vec![area.unwrap_or(ChunkArea::EVERYWHERE)],
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    let state = RegionState::new(EntityIds::block(ids).expect("ids"));
    let mut region = Region::restore_loaded(config(), state, holdings, &tickets, chunks, copies);
    region.clock.day = NOON;
    idle(&mut region);
    region
}

/// Lights both sides of the line, near it and far, a roof over part of it and a wall
/// along it.
fn lit_line() -> Vec<(BlockPos, BlockState)> {
    let mut placed = vec![
        (at(15, GROUND, 3), blocks::TORCH),
        (at(17, GROUND, 9), blocks::GLOWSTONE),
        (at(16, GROUND + 1, -2), blocks::GLASS),
        (at(5, GROUND, 12), blocks::LANTERN),
        (at(27, GROUND, 4), blocks::TORCH),
    ];
    for x in 10..22 {
        for z in -8..4 {
            placed.push((at(x, GROUND + 3, z), blocks::STONE));
        }
    }
    for z in 6..14 {
        placed.push((at(16, GROUND + 1, z), blocks::STONE));
    }
    placed
}

#[test]
fn the_chunks_each_side_of_a_pinned_line_have_the_light_of_one_region_with_the_same_blocks() {
    let land = land(&lit_line());
    let west = region_of(Some(WEST), 1, &land);
    let east = region_of(Some(EAST), 2, &land);
    let whole = region_of(None, 3, &land);
    assert!(west.ticks_chunk(ChunkPos::new(0, 0)));
    assert!(east.ticks_chunk(ChunkPos::new(1, 0)));
    for (side, chunk) in [(&west, ChunkPos::new(0, 0)), (&east, ChunkPos::new(1, 0))] {
        for x in 0..16 {
            for z in 0..16 {
                for y in GROUND - 1..=GROUND + 4 {
                    let pos = at(chunk.x * 16 + x, y, chunk.z * 16 + z);
                    assert_eq!(levels(side, pos), levels(&whole, pos), "{pos:?}");
                }
            }
        }
    }
    // The torch west of the line lights the east's own chunk, through its copy, and the
    // glowstone east of it the west's.
    let block = |region: &Region, pos| levels(region, pos).map(|(_, block)| block);
    assert_eq!(block(&east, at(16, GROUND, 3)), Some(13));
    assert_eq!(block(&west, at(15, GROUND, 9)), Some(13));
}

#[test]
fn a_torch_set_beyond_the_line_lights_a_crop_this_side_in_the_tick_its_copy_is_updated() {
    // A dark room over all the land, wheat east of the line on farmland.
    let mut placed = Vec::new();
    for x in -16..48 {
        for z in -16..32 {
            placed.push((at(x, ROOF, z), blocks::STONE));
        }
    }
    let crop = at(17, GROUND, 8);
    placed.push((crop.offset(0, -1, 0), blocks::FARMLAND));
    placed.push((crop, blocks::WHEAT));
    let land = land(&placed);
    let mut east = region_of(Some(EAST), 2, &land);
    assert_eq!(east.raw_brightness(crop, 0), Some(0));
    // The west sets a torch at x = 13, four from the crop; its change comes to the east
    // as an update of the east's copy, and the east's light step of that tick lights it.
    let torch = at(13, GROUND, 8);
    let output = checked(
        &mut east,
        &TickInputs {
            copy_updates: vec![(torch.chunk(), vec![Change::from((torch, blocks::TORCH))])],
            ..TickInputs::default()
        },
    );
    assert_eq!(
        east.raw_brightness(crop, 0),
        Some(10),
        "tick {}",
        output.tick
    );
    // The crop now grows by it: in light 10 its random tick draws its chance.
    let drawn = tick_at(&mut east, crop, 2);
    let mut replay = Replay::of(2);
    replay.int(13);
    replay.matches(&drawn, "wheat by a torch beyond the line");
}

/// Hands the conditional sets among `made` to `holder` as a coupled maker's effects
/// of the tick `made_in`, and ticks it.
fn deliver(holder: &mut Region, made: &[clustine_world::Effect], made_in: u64) {
    let effects = made
        .iter()
        .filter(|effect| holder.chunk(effect.at.chunk()).is_some())
        .enumerate()
        .map(|(index, effect)| FiledEffect {
            id: EffectId {
                sequence: 1,
                index: u32::try_from(index).expect("few"),
            },
            effect: effect.clone(),
            due: Due::At { tick: made_in },
        })
        .collect();
    checked(
        holder,
        &TickInputs {
            effects,
            ..TickInputs::default()
        },
    );
}

#[test]
fn a_mushroom_s_walk_that_ends_beyond_the_line_is_set_there_by_the_holder() {
    let mut placed = Vec::new();
    for x in -16..48 {
        for z in -16..32 {
            placed.push((at(x, ROOF, z), blocks::STONE));
        }
    }
    let mushroom = at(15, GROUND, 8);
    placed.push((mushroom, blocks::BROWN_MUSHROOM));
    let land = land(&placed);
    let west = region_of(Some(WEST), 1, &land);
    let mut east = region_of(Some(EAST), 2, &land);
    let taken = BTreeSet::from([mushroom]);
    let (seed, landing) = successes(25, 400)
        .into_iter()
        .find_map(|seed| {
            let mut replay = Replay::of(seed);
            replay.int(25);
            let landing = walk(&mut replay, mushroom, &taken)?;
            (landing.x >= 16).then_some((seed, landing))
        })
        .expect("a walk across the line");
    let mut west = west;
    west.made.clear();
    let drawn = tick_at(&mut west, mushroom, seed);
    let mut replay = Replay::of(seed);
    replay.int(25);
    walk(&mut replay, mushroom, &taken);
    replay.matches(&drawn, "the walk");
    let sets: Vec<(BlockPos, BlockState, BlockState, u32)> = west
        .made
        .iter()
        .filter_map(|effect| match effect.what {
            Effected::SetIf {
                expected,
                state,
                flags,
            } => Some((effect.at, expected, state, flags)),
            _ => None,
        })
        .collect();
    // `setBlock(pos, state, 2)` (n8 MushroomBlock 164–169).
    assert_eq!(sets, [(landing, blocks::AIR, blocks::BROWN_MUSHROOM, 2)]);
    // The copy is the holder's to change.
    assert_eq!(read(&west, landing), blocks::AIR);
    let made = west.made.clone();
    deliver(&mut east, &made, west.tick);
    assert_eq!(read(&east, landing), blocks::BROWN_MUSHROOM);
}

#[test]
fn grass_s_try_beyond_the_line_is_set_by_the_holder_and_refused_where_the_dirt_is_gone() {
    let grass = at(15, GROUND, 4);
    let dirt = at(16, GROUND - 1, 4);
    let land = land(&[(grass, blocks::GRASS_BLOCK), (dirt, blocks::DIRT)]);
    let west = region_of(Some(WEST), 1, &land);
    let seed = (0..)
        .find(|seed| {
            let mut replay = Replay::of(*seed);
            (0..4).any(|_| {
                let dx = replay.int(3) as i32 - 1;
                let dy = replay.int(5) as i32 - 3;
                let dz = replay.int(3) as i32 - 1;
                grass.offset(dx, dy, dz) == dirt
            })
        })
        .expect("a try onto the dirt");
    for refused in [false, true] {
        let mut west = west.clone();
        west.light_step();
        let mut east = region_of(Some(EAST), 2, &land);
        west.made.clear();
        tick_at(&mut west, grass, seed);
        assert_eq!(read(&west, dirt), blocks::DIRT, "the copy is the holder's");
        if refused {
            // The holder's player tilled it meanwhile.
            put(&mut east, dirt, blocks::FARMLAND);
        }
        let made = west.made.clone();
        deliver(&mut east, &made, west.tick);
        let expected = if refused {
            blocks::FARMLAND
        } else {
            state("minecraft:grass_block[snowy=false]")
        };
        assert_eq!(read(&east, dirt), expected);
    }
}
