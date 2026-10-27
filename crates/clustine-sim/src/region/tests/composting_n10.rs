//! Tests of the composter's rule and a crop's bone meal (`region/composting.rs`;
//! `docs/groundwork/n10-plan.md`, section 6; n6 ComposterBlock.txt): what an item put in
//! does to the level, the bone meal at level 8, the composter's own tick from 7 to 8 in a
//! region, and how far bone meal grows a crop.

use clustine_blocks::data::{number, set};
use clustine_world::Ticked;

use super::*;
use crate::region::Random;
use crate::region::composting::{
    self, FULL, READY, bone_meal_crop, compost_chance, extract_produce, insert_item,
};

/// The floor at y = -61 above which blocks of the tests stand.
const Y: i32 = -60;

fn composter(level: u8) -> BlockState {
    set(blocks::COMPOSTER, "level", &level.to_string())
}

fn random() -> Random {
    Random::of_entity(11, EntityId(3), 0)
}

/// A region pinned to the whole world holding the chunks from x = -1 to 1 and z = -1 to
/// 1, with `blocks` set.
fn region_with(blocks: &[(BlockPos, BlockState)]) -> Region {
    let mut chunks: Vec<(ChunkPos, Chunk)> = (-1..=1)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .map(|position| (position, floor()))
        .collect();
    for (at, state) in blocks {
        let (_, chunk) = chunks
            .iter_mut()
            .find(|(position, _)| *position == at.chunk())
            .expect("within the land");
        let (x, z) = at.in_chunk();
        chunk.set(x, at.y, z, *state);
    }
    let tickets: Vec<(ChunkPos, Ticket)> = chunks
        .iter()
        .map(|(position, _)| (*position, Ticket::Viewer))
        .collect();
    let holdings = Holdings {
        held: chunks.iter().map(|(position, _)| *position).collect(),
        pinned: vec![ChunkArea {
            min_x: None,
            max_x: None,
        }],
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    let state = RegionState::new(EntityIds::block(1).expect("a block of ids"));
    Region::restore_loaded(config(), state, holdings, &tickets, chunks, Vec::new())
}

/// Whether a tick of `of` waits at `pos`: in the region's queue where its chunk ticks,
/// else kept in the chunk.
fn scheduled(region: &Region, pos: BlockPos, of: Ticked) -> bool {
    let (x, z) = pos.in_chunk();
    let at = InChunk::new(x, pos.y, z);
    region.has_scheduled_tick(pos, of)
        || region.chunk(pos.chunk()).is_some_and(|chunk| {
            chunk
                .ticks()
                .iter()
                .any(|tick| tick.at == at && tick.of == of)
        })
}

/// The datagen's `compostable` components: seeds are low (30), wheat medium (65), bread
/// medium high (85), cake always adds one; stone is not compostable.
#[test]
fn an_items_compost_chance_is_its_components() {
    assert_eq!(compost_chance(items::WHEAT_SEEDS), Some(30));
    assert_eq!(compost_chance(items::BEETROOT_SEEDS), Some(30));
    assert_eq!(compost_chance(items::WHEAT), Some(65));
    assert_eq!(compost_chance(items::BREAD), Some(85));
    assert_eq!(compost_chance(items::SUGAR_CANE), Some(50));
    assert_eq!(compost_chance(items::CAKE), Some(100));
    assert_eq!(compost_chance(items::STONE), None);
}

/// `addLayer`'s `number_dispatcher`: into an empty composter an item always adds a layer
/// and draws nothing; one that always adds one draws nothing either.
#[test]
fn the_first_layer_is_sure_and_draws_nothing() {
    let mut draws = random();
    let before = draws.clone();
    let inserted = insert_item(composter(0), items::WHEAT_SEEDS, &mut draws);
    assert_eq!(inserted.state, composter(1));
    assert!(inserted.used);
    let cake = insert_item(composter(4), items::CAKE, &mut draws);
    assert_eq!(cake.state, composter(5));
    assert_eq!(draws, before, "nothing drawn");
}

/// Above the first layer, seeds add one 30 times in 100 (one `nextInt(100)` each, a layer
/// below 30); used whether or not they do.
#[test]
fn seeds_add_a_layer_thirty_times_in_a_hundred() {
    let mut draws = random();
    let mut rose = 0;
    for _ in 0..10_000 {
        let mut expected = draws.clone();
        let layer = expected.next_int(100) < 30;
        let inserted = insert_item(composter(3), items::WHEAT_SEEDS, &mut draws);
        assert!(inserted.used);
        assert_eq!(inserted.state == composter(4), layer);
        assert_eq!(draws, expected, "one draw");
        rose += usize::from(layer);
    }
    assert!((2800..3200).contains(&rose), "{rose}");
}

/// A full or ready composter takes nothing, nor does a block that is no composter, nor
/// an item that is not compostable; none draws.
#[test]
fn a_full_composter_or_an_item_not_compostable_takes_nothing() {
    let mut draws = random();
    let before = draws.clone();
    for (state, item) in [
        (composter(FULL), items::WHEAT_SEEDS),
        (composter(READY), items::WHEAT_SEEDS),
        (composter(2), items::STONE),
        (blocks::STONE, items::WHEAT_SEEDS),
    ] {
        let inserted = insert_item(state, item, &mut draws);
        assert_eq!(inserted.state, state);
        assert!(!inserted.used);
    }
    assert_eq!(draws, before);
}

/// `extractProduce`: the bone meal half a block in from the corner and 1.01 up, moved by
/// up to 0.35 each way on x and z (two `nextFloat`s); the composter empty.
#[test]
fn a_ready_composter_gives_its_bone_meal_above_and_is_empty() {
    let pos = BlockPos::new(4, Y, -2);
    for seed in 0..50 {
        let mut draws = Random::of_entity(seed, EntityId(1), 0);
        let mut expected = draws.clone();
        let (at, emptied) = extract_produce(composter(READY), pos, &mut draws);
        assert_eq!(emptied, composter(0));
        let x = f64::from((expected.next_float() - 0.5) * 0.7);
        let z = f64::from((expected.next_float() - 0.5) * 0.7);
        assert_eq!(at, [4.5 + x, f64::from(Y) + 1.01, -1.5 + z]);
        assert!((4.15..=4.85).contains(&at[0]));
    }
}

/// `ComposterBlock.onPlace` and `tick` in a region: a composter set to level 7 waits 20
/// ticks; its tick makes it ready with `composter_ready`'s sound. One set to another
/// level waits for nothing.
#[test]
fn a_full_composter_becomes_ready_twenty_ticks_on() {
    let full = BlockPos::new(1, Y, 1);
    let half = BlockPos::new(3, Y, 1);
    let mut region = region_with(&[(full, composter(6)), (half, composter(2))]);
    let mut output = TickOutput::default();
    let block = clustine_blocks::data::tick_id(blocks::COMPOSTER);
    region.set_block(full, composter(FULL), &mut output);
    region.set_block(half, composter(3), &mut output);
    assert!(scheduled(&region, full, Ticked::Block(block)));
    assert!(!scheduled(&region, half, Ticked::Block(block)));

    let mut output = TickOutput::default();
    assert!(region.composter_tick(full, composter(FULL), &mut output));
    let (x, z) = full.in_chunk();
    assert_eq!(
        region
            .chunk(full.chunk())
            .and_then(|chunk| chunk.get(x, Y, z)),
        Some(composter(READY))
    );
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::Sound { sound, .. } if *sound == clustine_data::sounds::BLOCK_COMPOSTER_READY
    )));
    // A tick of a composter at another level, or of another block, changes nothing.
    let mut output = TickOutput::default();
    assert!(region.composter_tick(half, composter(3), &mut output));
    assert!(!region.composter_tick(half, blocks::STONE, &mut output));
    assert!(output.events.is_empty());
}

/// `CropBlock.performBonemeal`: wheat grows 2 to 5 ages (one `nextInt(4)`), never past 7;
/// beetroots a third of that (0 or 1), never past 3; the torchflower crop one age without
/// a draw, its second the torchflower; a crop grown out, or another block, takes none.
#[test]
fn bone_meal_grows_a_crop_two_to_five_ages() {
    let wheat = |age: u8| set(blocks::WHEAT, "age", &age.to_string());
    let mut seen = std::collections::BTreeSet::new();
    for seed in 0..200 {
        let mut draws = Random::of_entity(seed, EntityId(1), 0);
        let grown = bone_meal_crop(wheat(0), &mut draws).expect("young wheat grows");
        seen.insert(number(grown, "age").expect("an age"));
        let nearly = bone_meal_crop(wheat(6), &mut draws).expect("it grows");
        assert_eq!(nearly, wheat(7));
        let beetroot = bone_meal_crop(blocks::BEETROOTS, &mut draws).expect("it grows");
        assert!(matches!(number(beetroot, "age"), Some(0 | 1)));
    }
    assert_eq!(seen.into_iter().collect::<Vec<_>>(), [2, 3, 4, 5]);
    let mut draws = random();
    let before = draws.clone();
    let torch = bone_meal_crop(blocks::TORCHFLOWER_CROP, &mut draws).expect("it grows");
    assert_eq!(number(torch, "age"), Some(1));
    assert_eq!(bone_meal_crop(torch, &mut draws), Some(blocks::TORCHFLOWER));
    assert_eq!(draws, before, "the torchflower crop draws nothing");
    assert_eq!(bone_meal_crop(wheat(7), &mut draws), None);
    assert_eq!(bone_meal_crop(blocks::STONE, &mut draws), None);
    assert_eq!(composting::crop_grown_out(wheat(7)), Some(true));
    assert_eq!(composting::crop_grown_out(blocks::STONE), None);
}
