//! A survival player's dig (`region/digging.rs`; `docs/groundwork/v-plan.md`, section
//! 4.5, step V2), rule by rule as n5 `ServerPlayerGameMode` has them: a START and a
//! STOP late enough break, one too early leaves a delayed destroy that counts the start
//! as its ticks, an ABORT takes the cracks away, a turn of the face shows in the level
//! events, what breaks at once breaks at the START, bedrock never, the wrong tool breaks
//! with no drops, the tool wears, Efficiency digs faster, and a dig goes on across a
//! hand-over.

use clustine_data::{BlockState, blocks, items};
use clustine_world::components::{Component, Kind};
use clustine_world::{EntityData, GameMode, PlayerMode};

use super::*;
use crate::api::DestroyAction;
use crate::state::{Dig, Digging};

/// Where the block dug at lies: in the floor, two blocks east of the spawn.
const AT: BlockPos = BlockPos::new(2, -61, 1);

fn state(name: &str) -> BlockState {
    BlockState::parse(name).expect("a block state")
}

/// The region of `on_ticking_floor_with` with `block` at [`AT`], player 1 at the spawn
/// in survival, on the ground, holding `held` in slot 0, and nothing else.
fn digger(block: BlockState, held: Option<ItemStack>) -> Region {
    let mut region = on_ticking_floor_with(&[(AT, block)]);
    let me = region
        .players
        .get_mut(&player(1))
        .expect("player 1 is here");
    me.mode = PlayerMode::new(GameMode::Survival);
    me.inventory = Inventory::default();
    me.inventory.set(0, held);
    me.selected_slot = 0;
    me.pose.on_ground = true;
    region
}

/// As [`digger`], for a player who has played a while: their count is 1000 more.
fn old_digger(block: BlockState, held: Option<ItemStack>) -> Region {
    let mut region = digger(block, held);
    let me = region
        .players
        .get_mut(&player(1))
        .expect("player 1 is here");
    me.game_origin -= 1000;
    region
}

fn step(region: &mut Region, action: DestroyAction, at: BlockPos, sequence: i32) -> TickOutput {
    step_from(region, action, at, Face::Top, sequence)
}

fn step_from(
    region: &mut Region,
    action: DestroyAction,
    at: BlockPos,
    face: Face,
    sequence: i32,
) -> TickOutput {
    let input = PlayerInput::Destroy {
        action,
        position: at,
        face,
        sequence,
    };
    region.tick(&moves(vec![numbered(1, input)]))
}

/// `count` ticks with nothing done, and what each sent.
fn idle(region: &mut Region, count: usize) -> Vec<TickOutput> {
    (0..count)
        .map(|_| region.tick(&TickInputs::default()))
        .collect()
}

/// The stages of cracks the outputs sent, in order, at any block.
fn stages(outputs: &[&TickOutput]) -> Vec<(BlockPos, i32)> {
    outputs
        .iter()
        .flat_map(|output| output.events.iter())
        .filter_map(|event| match event {
            RegionEvent::BlockDestruction {
                breaker,
                position,
                stage,
            } => {
                assert_eq!(*breaker, EntityId(1), "player 1 breaks");
                Some((*position, *stage))
            }
            _ => None,
        })
        .collect()
}

/// The level events of a dig the output sent: the event and its data.
fn dig_events(output: &TickOutput) -> Vec<(i32, i32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::LevelEvent {
                kind,
                position,
                data,
            } if *position == AT && (*kind == 2019 || *kind == 2020) => Some((*kind, *data)),
            _ => None,
        })
        .collect()
}

fn items_lying(region: &Region) -> Vec<ItemStack> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter_map(|entity| match entity.data {
            EntityData::Item { ref stack, .. } => Some(stack.clone()),
            _ => None,
        })
        .collect()
}

fn orbs_lying(region: &Region) -> i32 {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter_map(|entity| match entity.data {
            EntityData::ExperienceOrb { value, count, .. } => Some(value * count),
            _ => None,
        })
        .sum()
}

fn block(region: &Region, at: BlockPos) -> Option<BlockState> {
    region.own_block(at)
}

fn dig(region: &Region) -> Dig {
    region.players[&player(1)].dig
}

fn wear(region: &Region) -> i32 {
    match region.players[&player(1)].inventory.0[0]
        .as_ref()
        .and_then(|stack| stack.components.get(Kind::Damage))
    {
        Some(Component::Damage(damage)) => *damage,
        _ => 0,
    }
}

fn acknowledges(output: &TickOutput, sequence: i32) -> bool {
    told(output).contains(&acknowledged(1, sequence))
}

#[test]
fn a_start_and_a_stop_ten_ticks_on_break_dirt_by_hand_and_the_others_see_it_crack() {
    // Dirt by hand: 1 / 0.5 / 30 a tick. A STOP breaks where that times the ticks since
    // the start, plus one, is 0.7 or more: ten ticks on (0.73), not nine (0.67).
    let mut region = digger(blocks::DIRT, None);
    let started = step(&mut region, DestroyAction::Start, AT, 1);
    assert!(acknowledges(&started, 1));
    assert!(dig(&region).is_destroying);
    // The START's stage, 0, then the tick's own: one tick, (1 + 1) / 15.
    assert_eq!(stages(&[&started]), [(AT, 0), (AT, 1)]);
    // The tick's level event, every fourth tick 2020, else 2019, with the face.
    assert_eq!(dig_events(&started), [(2019, 1)]);
    let waited = idle(&mut region, 9);
    assert_eq!(dig_events(&waited[2]), [(2020, 1)]);
    let mut seen: Vec<&TickOutput> = vec![&started];
    seen.extend(waited.iter());
    // Each stage once, as it changes: 10 ticks, (10 + 1) / 15.
    assert_eq!(
        stages(&seen),
        [0, 1, 2, 3, 4, 5, 6, 7].map(|stage| (AT, stage))
    );
    assert_eq!(block(&region, AT), Some(blocks::DIRT));
    let stopped = step(&mut region, DestroyAction::Stop, AT, 2);
    assert_eq!(block(&region, AT), Some(blocks::AIR));
    assert!(acknowledges(&stopped, 2));
    // The cracks go, and the block is seen breaking by everyone but the breaker.
    assert_eq!(stages(&[&stopped]), [(AT, -1)]);
    assert!(stopped.events.contains(&RegionEvent::BlockDestroyed {
        position: AT,
        state: blocks::DIRT,
        breaker: Some(player(1)),
    }));
    assert_eq!(items_lying(&region), [ItemStack::new(items::DIRT, 1)]);
    assert!(!dig(&region).is_destroying);
}

#[test]
fn a_stop_too_early_leaves_a_delayed_destroy_that_counts_the_start_as_its_ticks() {
    // Nine ticks on is too early: the dig waits as a delayed destroy, whose progress
    // the game reckons with the start's count as the ticks. A player whose count was
    // 1000 at the start breaks the block in the same tick's dig.
    let mut region = old_digger(blocks::DIRT, None);
    step(&mut region, DestroyAction::Start, AT, 1);
    idle(&mut region, 8);
    let stopped = step(&mut region, DestroyAction::Stop, AT, 2);
    assert!(acknowledges(&stopped, 2));
    assert_eq!(block(&region, AT), Some(blocks::AIR));
    assert!(!dig(&region).has_delayed);
    assert_eq!(items_lying(&region), [ItemStack::new(items::DIRT, 1)]);
    // The delayed destroy's stage is of its quirk's progress, far above 9, and no -1
    // follows its break (the game sends none either).
    let [(_, stage)] = stages(&[&stopped])[..] else {
        panic!("one stage: {:?}", stages(&[&stopped]));
    };
    assert!(stage > 9, "{stage}");
}

#[test]
fn a_delayed_destroy_of_a_player_who_has_just_come_waits_as_long_as_their_start_is_young() {
    // A player who joined a few ticks ago started at a count of a few: the delayed
    // destroy's progress, the dig's speed times that count plus one, stays below 1 for
    // good, and the block stays until they dig again.
    let mut region = digger(blocks::DIRT, None);
    step(&mut region, DestroyAction::Start, AT, 1);
    idle(&mut region, 8);
    let stopped = step(&mut region, DestroyAction::Stop, AT, 2);
    assert!(acknowledges(&stopped, 2));
    let waiting = dig(&region);
    assert!(waiting.has_delayed && !waiting.is_destroying);
    assert_eq!(waiting.delayed_pos, AT);
    idle(&mut region, 40);
    assert_eq!(block(&region, AT), Some(blocks::DIRT));
    assert!(dig(&region).has_delayed);
}

#[test]
fn an_abort_ends_the_dig_and_takes_its_cracks_away_at_both_blocks_where_they_differ() {
    let mut region = digger(blocks::DIRT, None);
    step(&mut region, DestroyAction::Start, AT, 1);
    idle(&mut region, 3);
    let aborted = step(&mut region, DestroyAction::Abort, AT, 2);
    assert!(acknowledges(&aborted, 2));
    assert_eq!(stages(&[&aborted]), [(AT, -1)]);
    assert!(!dig(&region).is_destroying);
    let after = idle(&mut region, 2);
    assert!(stages(&after.iter().collect::<Vec<_>>()).is_empty());
    assert!(dig_events(&after[0]).is_empty());
    assert_eq!(block(&region, AT), Some(blocks::DIRT));

    // Named at another block, the dig's own block loses its cracks first.
    step(&mut region, DestroyAction::Start, AT, 3);
    let elsewhere = AT.offset(1, 0, 0);
    let aborted = step(&mut region, DestroyAction::Abort, elsewhere, 4);
    assert_eq!(stages(&[&aborted]), [(AT, -1), (elsewhere, -1)]);
}

#[test]
fn a_turn_to_another_face_is_named_by_the_dig_s_level_events_from_then_on() {
    let mut region = digger(blocks::DIRT, None);
    let started = step_from(&mut region, DestroyAction::Start, AT, Face::Top, 1);
    assert_eq!(dig_events(&started), [(2019, 1)]);
    let turned = step_from(
        &mut region,
        DestroyAction::ChangeDirection,
        AT,
        Face::North,
        2,
    );
    assert!(acknowledges(&turned, 2));
    // North is the direction 2.
    assert_eq!(dig_events(&turned), [(2019, 2)]);
    assert_eq!(dig(&region).direction, Face::North);
    // The dig goes on as it was.
    assert!(dig(&region).is_destroying);
}

#[test]
fn a_flower_breaks_at_the_start_and_drops() {
    let poppy = state("minecraft:poppy");
    let above = AT.offset(0, 1, 0);
    let mut region = digger(blocks::DIRT, None);
    region
        .chunks
        .get_mut(&above.chunk())
        .unwrap()
        .set(2, above.y, 1, poppy);
    let started = step(&mut region, DestroyAction::Start, above, 1);
    assert!(acknowledges(&started, 1));
    assert_eq!(block(&region, above), Some(blocks::AIR));
    assert_eq!(items_lying(&region), [ItemStack::new(items::POPPY, 1)]);
    // An insta-mine begins no dig, and sends no cracks.
    assert!(!dig(&region).is_destroying);
    assert!(stages(&[&started]).is_empty());
}

#[test]
fn bedrock_never_breaks_however_long_it_is_dug() {
    let mut region = old_digger(blocks::BEDROCK, None);
    step(&mut region, DestroyAction::Start, AT, 1);
    idle(&mut region, 30);
    let stopped = step(&mut region, DestroyAction::Stop, AT, 2);
    assert!(acknowledges(&stopped, 2));
    idle(&mut region, 3);
    assert_eq!(block(&region, AT), Some(blocks::BEDROCK));
    assert!(items_lying(&region).is_empty());
}

#[test]
fn a_block_that_needs_a_tool_breaks_by_hand_with_no_drops_and_drops_with_the_tool() {
    // A snow block needs a shovel: by hand 1 / 0.2 / 100 a tick, thirteen ticks to a
    // STOP that breaks.
    let snow = blocks::SNOW_BLOCK;
    let mut region = digger(snow, None);
    step(&mut region, DestroyAction::Start, AT, 1);
    idle(&mut region, 12);
    step(&mut region, DestroyAction::Stop, AT, 2);
    assert_eq!(block(&region, AT), Some(blocks::AIR));
    assert!(items_lying(&region).is_empty());

    // A wooden shovel, 2 / 0.2 / 30 a tick: two ticks, four snowballs, one of wear.
    let shovel = ItemStack::new(items::WOODEN_SHOVEL, 1);
    let mut region = digger(snow, Some(shovel));
    step(&mut region, DestroyAction::Start, AT, 1);
    idle(&mut region, 1);
    step(&mut region, DestroyAction::Stop, AT, 2);
    assert_eq!(block(&region, AT), Some(blocks::AIR));
    assert_eq!(items_lying(&region), [ItemStack::new(items::SNOWBALL, 4)]);
    assert_eq!(wear(&region), 1);
}

#[test]
fn a_tool_wears_where_the_block_is_not_broken_at_once_whether_it_is_the_right_one_or_not() {
    // A pickaxe breaking dirt, which it is not for, wears all the same, and the dirt
    // drops, needing no tool.
    let pickaxe = ItemStack::new(items::WOODEN_PICKAXE, 1);
    let mut region = digger(blocks::DIRT, Some(pickaxe.clone()));
    step(&mut region, DestroyAction::Start, AT, 1);
    idle(&mut region, 9);
    step(&mut region, DestroyAction::Stop, AT, 2);
    assert_eq!(block(&region, AT), Some(blocks::AIR));
    assert_eq!(wear(&region), 1);
    assert_eq!(items_lying(&region), [ItemStack::new(items::DIRT, 1)]);

    // A flower's hardness is 0: no wear.
    let poppy = state("minecraft:poppy");
    let mut region = digger(poppy, Some(pickaxe));
    step(&mut region, DestroyAction::Start, AT, 1);
    assert_eq!(block(&region, AT), Some(blocks::AIR));
    assert_eq!(wear(&region), 0);
}

#[test]
fn an_ore_broken_with_the_right_tool_gives_its_experience_and_with_silk_touch_none() {
    let ore = blocks::DIAMOND_ORE;
    let pickaxe = ItemStack::new(items::IRON_PICKAXE, 1);
    let mut region = old_digger(ore, Some(pickaxe.clone()));
    step(&mut region, DestroyAction::Start, AT, 1);
    idle(&mut region, 30);
    step(&mut region, DestroyAction::Stop, AT, 2);
    assert_eq!(block(&region, AT), Some(blocks::AIR));
    assert_eq!(items_lying(&region), [ItemStack::new(items::DIAMOND, 1)]);
    let experience = orbs_lying(&region);
    assert!((3..=7).contains(&experience), "{experience}");

    let mut silky = pickaxe;
    silky.components.set(Component::Enchantments(vec![(
        "minecraft:silk_touch".to_owned(),
        1,
    )]));
    let mut region = old_digger(ore, Some(silky));
    step(&mut region, DestroyAction::Start, AT, 1);
    idle(&mut region, 30);
    step(&mut region, DestroyAction::Stop, AT, 2);
    assert_eq!(
        items_lying(&region),
        [ItemStack::new(items::DIAMOND_ORE, 1)]
    );
    assert_eq!(orbs_lying(&region), 0);
}

#[test]
fn an_efficiency_pickaxe_digs_stone_faster_by_its_attribute() {
    // An iron pickaxe: 6 / 1.5 / 30 a tick. With Efficiency V the player's
    // `mining_efficiency` is 26 once their equipment has been looked at, (6 + 26) / 1.5
    // / 30 a tick, and a STOP one tick on breaks.
    let pickaxe = ItemStack::new(items::IRON_PICKAXE, 1);
    let mut region = digger(blocks::STONE, Some(pickaxe.clone()));
    idle(&mut region, 1);
    assert_eq!(
        region.destroy_progress(player(1), blocks::STONE),
        6.0 / 1.5 / 30.0
    );

    let mut efficient = pickaxe;
    efficient.components.set(Component::Enchantments(vec![(
        "minecraft:efficiency".to_owned(),
        5,
    )]));
    let mut region = digger(blocks::STONE, Some(efficient));
    idle(&mut region, 1);
    assert_eq!(
        region.destroy_progress(player(1), blocks::STONE),
        32.0 / 1.5 / 30.0
    );
    step(&mut region, DestroyAction::Start, AT, 1);
    step(&mut region, DestroyAction::Stop, AT, 2);
    assert_eq!(block(&region, AT), Some(blocks::AIR));
    assert_eq!(
        items_lying(&region),
        [ItemStack::new(items::COBBLESTONE, 1)]
    );
}

/// Player 2 coming into the region with a dig of dirt at [`AT`] under way, begun at the
/// count 490 of the region they come from, which ended its tick at 500.
fn dig_handed_over() -> PlayerTransfer {
    let mut transfer = transfer(2, 77, 1.5, 0);
    transfer.mode = PlayerMode::new(GameMode::Survival);
    transfer.inventory = Inventory::default();
    transfer.digging = Digging {
        game_ticks: 500,
        dig: Dig {
            is_destroying: true,
            destroy_pos: AT,
            direction: Face::Top,
            start: 490,
            last_sent: 7,
            ..Dig::default()
        },
    };
    transfer
}

#[test]
fn a_dig_handed_over_goes_on_from_its_count_in_the_region_it_comes_to() {
    let mut region = digger(blocks::DIRT, None);
    let output = region.tick(&changes(vec![arrive(2, &dig_handed_over())]));
    // The arrival's tick is the dig's eleventh: (11 + 1) / 15, stage 8.
    let cracks: Vec<_> = output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::BlockDestruction { breaker, stage, .. } => Some((*breaker, *stage)),
            _ => None,
        })
        .collect();
    assert_eq!(cracks, [(EntityId(77), 8)]);
}

#[test]
fn a_stop_right_after_a_hand_over_breaks_as_it_would_have_where_the_dig_began() {
    // Ten ticks since the start, as the region the player came from counted them.
    let mut region = digger(blocks::DIRT, None);
    let stop = PlayerInput::Destroy {
        action: DestroyAction::Stop,
        position: AT,
        face: Face::Top,
        sequence: 9,
    };
    let output = region.tick(&TickInputs {
        player_changes: vec![arrive(2, &dig_handed_over())],
        inputs: vec![as_entity(77, numbered(2, stop))],
        ..TickInputs::default()
    });
    assert!(told(&output).contains(&acknowledged(2, 9)));
    assert_eq!(block(&region, AT), Some(blocks::AIR));
    assert_eq!(items_lying(&region), [ItemStack::new(items::DIRT, 1)]);
}
