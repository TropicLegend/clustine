//! A player's water bucket where water evaporates (p-plan 17.2 A; `BucketItem.emptyContents`,
//! n7 BucketItem.txt 184-320): the fizz and nothing placed in the Nether, the water
//! placed in the overworld and the End.

use super::*;
use clustine_world::Dimension;

fn use_item(number: u128, pitch: f32) -> Input {
    numbered(
        number,
        PlayerInput::UseItem {
            rotation: (0.0, pitch),
            sneaking: false,
            sequence: 1,
        },
    )
}

/// A region of `dimension`'s level with player 1 holding `item` in the first slot of
/// the hotbar. The chunks are the overworld's floor; only the level is the
/// dimension's, which is all the rule reads.
fn holding_in(dimension: Dimension, item: i32) -> Region {
    let mut region = on_ticking_floor_with(&[]);
    region.config.dimension = dimension;
    region.tick(&moves(vec![
        set_slot(1, 0, Some(ItemStack::new(item, 1))),
        select(1, 0),
    ]));
    region
}

/// Where water evaporated: the blocks of the `fire_extinguish` sounds.
fn fizzes(output: &TickOutput) -> Vec<BlockPos> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound: clustine_data::sounds::BLOCK_FIRE_EXTINGUISH,
                position,
                ..
            } => Some(BlockPos::new(
                position.x.floor() as i32,
                position.y.floor() as i32,
                position.z.floor() as i32,
            )),
            _ => None,
        })
        .collect()
}

fn hisses(output: &TickOutput) -> Vec<(Option<PlayerId>, f32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound,
                volume,
                except,
                ..
            } if *sound == clustine_data::sounds::BLOCK_FIRE_EXTINGUISH => Some((*except, *volume)),
            _ => None,
        })
        .collect()
}

fn cods(region: &Region) -> usize {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter(|entity| i32::from(entity.kind) == clustine_data::entity_types::COD)
        .count()
}

fn water_placed(output: &TickOutput) -> bool {
    output.events.iter().any(
        |event| matches!(event, RegionEvent::BlockChanged { state, .. } if *state == blocks::WATER),
    )
}

#[test]
fn a_player_s_water_bucket_fizzes_in_the_nether_and_places_water_elsewhere() {
    for dimension in Dimension::ALL {
        let mut region = holding_in(dimension, items::WATER_BUCKET);
        let output = region.tick(&moves(vec![use_item(1, 60.0)]));
        if dimension == Dimension::Nether {
            assert!(!water_placed(&output));
            assert_eq!(fizzes(&output).len(), 1);
            assert_eq!(hisses(&output), [(Some(player(1)), 0.5)]);
        } else {
            assert!(water_placed(&output), "{dimension:?}");
            assert_eq!(hisses(&output), [], "{dimension:?}");
        }
    }
}

#[test]
fn a_player_s_fish_bucket_fizzes_in_the_nether_and_its_fish_is_made_all_the_same() {
    let mut region = holding_in(Dimension::Nether, items::COD_BUCKET);
    let output = region.tick(&moves(vec![use_item(1, 60.0)]));
    assert!(!water_placed(&output));
    assert_eq!(fizzes(&output).len(), 1);
    assert_eq!(cods(&region), 1, "the cod is made where the water fizzed");
}

#[test]
fn a_water_bucket_aimed_at_the_sky_in_the_nether_does_nothing() {
    let mut region = holding_in(Dimension::Nether, items::WATER_BUCKET);
    let output = region.tick(&moves(vec![use_item(1, -90.0)]));
    assert_eq!(fizzes(&output), []);
    assert_eq!(hisses(&output), []);
}
