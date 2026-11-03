//! A survival player's break of a block another region holds (step V2c), written from
//! the specification by someone who did not write the code: n5
//! `ServerPlayerGameMode.destroyBlock` takes a copy of the held stack before
//! `ItemStack.mineBlock` wears it, and `Block.playerDestroy` drops the block by its loot
//! table with that copy (Silk Touch gives the block itself, the wrong tool nothing),
//! while `Item.mineBlock` wears the tool where the block's hardness is not 0. One break
//! looks the same to its player whichever side of a line the block is on. Clustine
//! splits it: the player's region wears the tool and sends `RemoteStep::BreakWith` with
//! the stack as it was; the holder breaks only if its block is still the same block
//! (`same_block`) as the one the player's region judged.
//!
//! The line is at block x = 16: the west is pinned to the chunks x < 1 and holds the
//! player, the east to those from x = 1 on and holds the blocks dug at. Each region is
//! one of the two, told by the test what the store would say, as in `lends.rs`.

use clustine_data::{BlockState, blocks, items};
use clustine_world::components::{Component, Kind};
use clustine_world::{EntityData, GameMode, PlayerMode};

use super::*;
use crate::api::DestroyAction;

const WEST_AREA: ChunkArea = ChunkArea {
    min_x: None,
    max_x: Some(1),
};
const EAST_AREA: ChunkArea = ChunkArea {
    min_x: Some(1),
    max_x: None,
};
const THE_WEST: RegionId = RegionId(10);
const THE_EAST: RegionId = RegionId(11);

/// The floor's block right across the line, within the player's reach.
const AT: BlockPos = BlockPos::new(16, -61, 0);
/// The block above it, standing on the floor.
const ON_IT: BlockPos = BlockPos::new(16, -60, 0);

fn state(name: &str) -> BlockState {
    BlockState::parse(name).expect("a block state")
}

/// The floor with `blocks` set in the chunk at `position`.
fn floor_in(position: ChunkPos, blocks: &[(BlockPos, BlockState)]) -> Chunk {
    let mut chunk = floor();
    for (at, block) in blocks.iter().filter(|(at, _)| at.chunk() == position) {
        let (x, z) = at.in_chunk();
        chunk.set(x, at.y, z, *block);
    }
    chunk
}

/// A region pinned to `area` that holds `own` and has been told that `theirs` are
/// `holder`'s, each a stone floor with `blocks` set: its own loaded, theirs copied.
fn by_the_line(
    area: ChunkArea,
    own: &[ChunkPos],
    theirs: &[ChunkPos],
    holder: RegionId,
    blocks: &[(BlockPos, BlockState)],
) -> Region {
    let holdings = Holdings {
        held: own.to_vec(),
        pinned: vec![area],
        ..Holdings::default()
    };
    let state = knowing_the_edges(EntityIds::block(0).unwrap());
    let mut region = Region::restore(config(), state, holdings);
    let all: Vec<ChunkPos> = own.iter().chain(theirs).copied().collect();
    let output = checked(&mut region, &tickets(all, vec![]));
    assert_eq!(output.claims, theirs);
    checked(
        &mut region,
        &TickInputs {
            foreign: theirs.iter().map(|position| (*position, holder)).collect(),
            chunks_loaded: own
                .iter()
                .map(|position| (*position, floor_in(*position, blocks)))
                .collect(),
            copy_bases: theirs
                .iter()
                .map(|position| (*position, floor_in(*position, blocks)))
                .collect(),
            ..TickInputs::default()
        },
    );
    region
}

fn west_chunks() -> [ChunkPos; 6] {
    [
        ChunkPos::new(-1, -1),
        ChunkPos::new(-1, 0),
        ChunkPos::new(-1, 1),
        ChunkPos::new(0, -1),
        ChunkPos::new(0, 0),
        ChunkPos::new(0, 1),
    ]
}

fn east_chunks() -> [ChunkPos; 6] {
    [
        ChunkPos::new(1, -1),
        ChunkPos::new(1, 0),
        ChunkPos::new(1, 1),
        ChunkPos::new(2, -1),
        ChunkPos::new(2, 0),
        ChunkPos::new(2, 1),
    ]
}

/// The west, whose copy of the east has `blocks`, with player 1 standing at the line
/// (x = 14.5) on the ground, in `mode`, holding `held` in slot 0 and nothing else.
fn the_west_with(
    blocks: &[(BlockPos, BlockState)],
    mode: GameMode,
    held: Option<ItemStack>,
) -> Region {
    let theirs = [
        ChunkPos::new(1, -1),
        ChunkPos::new(1, 0),
        ChunkPos::new(1, 1),
    ];
    let mut region = by_the_line(WEST_AREA, &west_chunks(), &theirs, THE_EAST, blocks);
    checked(&mut region, &changes(vec![join(1)]));
    enter(&mut region);
    checked(&mut region, &moves(vec![walk(1, 14.5, 0.5)]));
    let me = region
        .players
        .get_mut(&player(1))
        .expect("player 1 is here");
    me.mode = PlayerMode::new(mode);
    me.inventory = Inventory::default();
    me.inventory.set(0, held);
    me.selected_slot = 0;
    me.pose.on_ground = true;
    region
}

/// As [`the_west_with`], for a survival player who has played a while: their count is
/// 1000 more, so that a delayed destroy finishes at once (the game's quirk of the
/// start as its ticks).
fn the_west_with_an_old_digger(
    blocks: &[(BlockPos, BlockState)],
    held: Option<ItemStack>,
) -> Region {
    let mut region = the_west_with(blocks, GameMode::Survival, held);
    let me = region
        .players
        .get_mut(&player(1))
        .expect("player 1 is here");
    me.game_origin -= 1000;
    region
}

/// The east, which holds the blocks dug at and is told the west's are the west's, with
/// `blocks` set and nobody in it.
fn the_east(blocks: &[(BlockPos, BlockState)]) -> Region {
    let theirs = [
        ChunkPos::new(0, -1),
        ChunkPos::new(0, 0),
        ChunkPos::new(0, 1),
    ];
    by_the_line(EAST_AREA, &east_chunks(), &theirs, THE_WEST, blocks)
}

fn step(region: &mut Region, action: DestroyAction, at: BlockPos, sequence: i32) -> TickOutput {
    let input = PlayerInput::Destroy {
        action,
        position: at,
        face: Face::Top,
        sequence,
    };
    region.tick(&moves(vec![numbered(1, input)]))
}

fn idle_for(region: &mut Region, count: usize) {
    for _ in 0..count {
        region.tick(&TickInputs::default());
    }
}

fn acknowledges(output: &TickOutput, sequence: i32) -> bool {
    told(output).contains(&acknowledged(1, sequence))
}

/// Every acknowledgement the output sends player 1.
fn acknowledgements(output: &TickOutput) -> Vec<i32> {
    told(output)
        .into_iter()
        .filter_map(|(id, event)| match event {
            PlayerEvent::Acknowledged { sequence } if id == player(1) => Some(sequence),
            _ => None,
        })
        .collect()
}

/// The damage of what player 1 holds in slot 0.
fn wear(region: &Region) -> i32 {
    match region.players[&player(1)].inventory.0[0]
        .as_ref()
        .and_then(|stack| stack.components.get(Kind::Damage))
    {
        Some(Component::Damage(damage)) => *damage,
        _ => 0,
    }
}

fn pickaxe() -> ItemStack {
    ItemStack::new(items::IRON_PICKAXE, 1)
}

fn silky(mut stack: ItemStack) -> ItemStack {
    stack.components.set(Component::Enchantments(vec![(
        "minecraft:silk_touch".to_owned(),
        1,
    )]));
    stack
}

/// The step the holder is sent of a survival break of `expected` at `position` with
/// `tool` by player 1, who is not sneaking.
fn break_with(position: BlockPos, expected: BlockState, tool: Option<ItemStack>) -> RemoteStep {
    RemoteStep::BreakWith {
        position,
        expected,
        tool,
        sneaking: false,
    }
}

fn items_lying(region: &Region) -> Vec<([f64; 3], ItemStack)> {
    region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter_map(|entity| match entity.data {
            EntityData::Item { ref stack, .. } => Some((entity.position, stack.clone())),
            _ => None,
        })
        .collect()
}

fn stacks_lying(region: &Region) -> Vec<ItemStack> {
    items_lying(region)
        .into_iter()
        .map(|(_, stack)| stack)
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

/// The answers the holder sent `REMOTE`: each `RemoteDone`'s sequence and shown blocks.
fn done_with_shown(output: &TickOutput) -> Vec<(i32, Vec<(BlockPos, BlockState)>)> {
    output
        .durable
        .iter()
        .filter(|(edge, ..)| *edge == REMOTE)
        .filter_map(|(_, _, entry)| match entry {
            Durable::RemoteDone {
                sequence, shown, ..
            } => Some((*sequence, shown.clone())),
            _ => None,
        })
        .collect()
}

fn block_events(output: &TickOutput) -> Vec<(BlockPos, i32)> {
    output
        .events
        .iter()
        .filter_map(|event| match event {
            RegionEvent::BlockEvent { position, kind, .. } => Some((*position, *kind)),
            _ => None,
        })
        .collect()
}

/// The holder takes `step` of player 1's action `sequence`.
fn taken(region: &mut Region, sequence: i32, step: RemoteStep) -> TickOutput {
    checked(region, &remotely(vec![remote(1, sequence, step)]))
}

// 1. The player's region.

#[test]
fn a_survival_dig_of_stone_across_the_line_wears_the_pickaxe_here_and_sends_the_unworn_one() {
    // An iron pickaxe on stone: 6 / 1.5 / 30 a tick, a STOP five ticks on breaks
    // (0.8 >= 0.7), four on does not (0.67).
    let mut region = the_west_with(&[], GameMode::Survival, Some(pickaxe()));
    let started = step(&mut region, DestroyAction::Start, AT, 1);
    assert!(acknowledges(&started, 1), "the START is this region's");
    assert!(requests(&started).is_empty(), "stone has no attack");
    idle_for(&mut region, 4);
    let stopped = step(&mut region, DestroyAction::Stop, AT, 2);
    assert_eq!(
        requests(&stopped),
        [remote(
            1,
            2,
            RemoteStep::BreakWith {
                position: AT,
                expected: blocks::STONE,
                tool: Some(pickaxe()),
                sneaking: false,
            }
        )]
    );
    assert_eq!(asked_of(&stopped), [Some(THE_EAST)]);
    // The holder's answer acknowledges it, not this region.
    assert!(!acknowledges(&stopped, 2), "{:?}", told(&stopped));
    let after = region.tick(&TickInputs::default());
    assert!(!acknowledges(&after, 2));
    assert_eq!(wear(&region), 1);
    // The copy is the holder's to change.
    assert_eq!(region.block(AT), Some(blocks::STONE));
}

#[test]
fn a_sneaking_survival_player_s_break_across_the_line_says_so() {
    let mut region = the_west_with(&[], GameMode::Survival, Some(pickaxe()));
    region
        .players
        .get_mut(&player(1))
        .expect("player 1 is here")
        .keys
        .sneak = true;
    step(&mut region, DestroyAction::Start, AT, 1);
    idle_for(&mut region, 4);
    let stopped = step(&mut region, DestroyAction::Stop, AT, 2);
    let steps: Vec<RemoteStep> = requests(&stopped).into_iter().map(|a| a.step).collect();
    assert!(
        matches!(steps[..], [RemoteStep::BreakWith { sneaking: true, .. }]),
        "{steps:?}"
    );
}

// 2. and 3. The holder.

#[test]
fn the_holder_breaks_stone_with_an_iron_pickaxe_drops_cobblestone_and_shows_air() {
    let mut region = the_east(&[]);
    let output = taken(
        &mut region,
        7,
        break_with(AT, blocks::STONE, Some(pickaxe())),
    );
    assert_eq!(region.own_block(AT), Some(blocks::AIR));
    assert_eq!(done_with_shown(&output), [(7, vec![(AT, blocks::AIR)])]);
    let lying = items_lying(&region);
    assert_eq!(lying.len(), 1, "{lying:?}");
    let (position, stack) = &lying[0];
    assert_eq!(*stack, ItemStack::new(items::COBBLESTONE, 1));
    // At the block: `Block.popResource` puts it within the block's cell.
    assert!(
        (16.0..17.0).contains(&position[0])
            && (-61.0..-59.0).contains(&position[1])
            && (0.0..1.0).contains(&position[2]),
        "{position:?}"
    );
    // The others see it break.
    assert!(
        output.events.iter().any(|event| matches!(
            event,
            RegionEvent::BlockDestroyed { position, state, .. }
                if *position == AT && *state == blocks::STONE
        )),
        "{:?}",
        output.events
    );
}

#[test]
fn the_holder_drops_stone_itself_for_a_silk_touch_pickaxe() {
    let mut region = the_east(&[]);
    taken(
        &mut region,
        3,
        break_with(AT, blocks::STONE, Some(silky(pickaxe()))),
    );
    assert_eq!(region.own_block(AT), Some(blocks::AIR));
    assert_eq!(stacks_lying(&region), [ItemStack::new(items::STONE, 1)]);
}

#[test]
fn the_holder_breaks_stone_by_hand_and_drops_nothing() {
    let mut region = the_east(&[]);
    let output = taken(&mut region, 4, break_with(AT, blocks::STONE, None));
    assert_eq!(region.own_block(AT), Some(blocks::AIR));
    assert!(
        stacks_lying(&region).is_empty(),
        "{:?}",
        stacks_lying(&region)
    );
    assert_eq!(orbs_lying(&region), 0);
    assert_eq!(done_with_shown(&output), [(4, vec![(AT, blocks::AIR)])]);
}

#[test]
fn the_holder_gives_the_experience_of_diamond_ore_broken_with_an_iron_pickaxe() {
    let ore = blocks::DIAMOND_ORE;
    let mut region = the_east(&[(AT, ore)]);
    taken(&mut region, 5, break_with(AT, ore, Some(pickaxe())));
    assert_eq!(region.own_block(AT), Some(blocks::AIR));
    assert_eq!(stacks_lying(&region), [ItemStack::new(items::DIAMOND, 1)]);
    // `DropExperienceBlock`'s 3 to 7.
    let experience = orbs_lying(&region);
    assert!((3..=7).contains(&experience), "{experience}");
}

#[test]
fn the_holder_gives_no_experience_for_diamond_ore_broken_with_silk_touch() {
    let ore = blocks::DIAMOND_ORE;
    let mut region = the_east(&[(AT, ore)]);
    taken(&mut region, 5, break_with(AT, ore, Some(silky(pickaxe()))));
    assert_eq!(
        stacks_lying(&region),
        [ItemStack::new(items::DIAMOND_ORE, 1)]
    );
    assert_eq!(orbs_lying(&region), 0);
}

// 4. A block that changed on the way.

#[test]
fn the_holder_leaves_a_block_that_is_no_longer_the_one_expected_and_shows_it() {
    let mut region = the_east(&[]);
    let output = taken(
        &mut region,
        9,
        break_with(AT, blocks::DIRT, Some(pickaxe())),
    );
    assert_eq!(region.own_block(AT), Some(blocks::STONE));
    assert!(stacks_lying(&region).is_empty());
    assert_eq!(done_with_shown(&output), [(9, vec![(AT, blocks::STONE)])]);
}

#[test]
fn the_holder_breaks_redstone_ore_lit_since_the_player_s_region_saw_it_unlit() {
    let unlit = state("minecraft:redstone_ore[lit=false]");
    let lit = state("minecraft:redstone_ore[lit=true]");
    let mut region = the_east(&[(AT, lit)]);
    let output = taken(&mut region, 11, break_with(AT, unlit, Some(pickaxe())));
    assert_eq!(region.own_block(AT), Some(blocks::AIR));
    assert_eq!(done_with_shown(&output), [(11, vec![(AT, blocks::AIR)])]);
    // Four or five redstone dust, by the table with an iron pickaxe, and 1 to 5 of
    // experience.
    let dust: i32 = stacks_lying(&region)
        .iter()
        .filter(|stack| stack.item == items::REDSTONE)
        .map(|stack| stack.count)
        .sum();
    assert!((4..=5).contains(&dust), "{:?}", stacks_lying(&region));
    let experience = orbs_lying(&region);
    assert!((1..=5).contains(&experience), "{experience}");
}

// 5. A delayed destroy across the line.

#[test]
fn a_delayed_destroy_across_the_line_carries_the_stop_s_sequence_and_is_left_to_the_holder() {
    let mut region = the_west_with_an_old_digger(&[], Some(pickaxe()));
    let started = step(&mut region, DestroyAction::Start, AT, 1);
    assert!(acknowledges(&started, 1));
    // One tick on is too early for a STOP (0.27); the delayed destroy, counting the
    // start as its ticks, finishes in the same tick's dig.
    let stopped = step(&mut region, DestroyAction::Stop, AT, 2);
    assert_eq!(
        requests(&stopped),
        [remote(
            1,
            2,
            RemoteStep::BreakWith {
                position: AT,
                expected: blocks::STONE,
                tool: Some(pickaxe()),
                sneaking: false,
            }
        )]
    );
    assert_eq!(asked_of(&stopped), [Some(THE_EAST)]);
    assert!(
        acknowledgements(&stopped).is_empty(),
        "the holder's answer acknowledges it: {:?}",
        told(&stopped)
    );
    assert!(!region.players[&player(1)].dig.has_delayed);
    assert_eq!(wear(&region), 1);
    // Nothing is sent again.
    let after = region.tick(&TickInputs::default());
    assert!(requests(&after).is_empty());
    assert!(acknowledgements(&after).is_empty());
}

// 6. START's attack.

#[test]
fn a_start_on_a_note_block_across_the_line_sends_an_unanswered_attack_that_plays_it() {
    let note = state("minecraft:note_block[instrument=harp,note=0,powered=false]");
    let mut west = the_west_with(&[(ON_IT, note)], GameMode::Survival, None);
    let started = step(&mut west, DestroyAction::Start, ON_IT, 1);
    assert_eq!(
        requests(&started),
        [remote(
            1,
            RemoteAction::UNANSWERED,
            RemoteStep::Attack { position: ON_IT }
        )]
    );
    assert_eq!(asked_of(&started), [Some(THE_EAST)]);
    // The START itself is acknowledged here, as it is acted on.
    assert!(acknowledges(&started, 1));
    assert!(west.players[&player(1)].dig.is_destroying);

    let mut east = the_east(&[(ON_IT, note)]);
    let output = taken(
        &mut east,
        RemoteAction::UNANSWERED,
        RemoteStep::Attack { position: ON_IT },
    );
    // `NoteBlock.attack` plays it: its block event of kind 0, as a click's.
    assert_eq!(block_events(&output), [(ON_IT, 0)]);
    assert_eq!(east.own_block(ON_IT), Some(note), "an attack does not tune");
    // Nobody waits for it: no `RemoteDone`.
    assert!(outcomes(&output).is_empty(), "{:?}", outcomes(&output));
}

#[test]
fn a_start_on_dirt_across_the_line_sends_no_attack() {
    let mut west = the_west_with(&[(ON_IT, blocks::DIRT)], GameMode::Survival, None);
    let started = step(&mut west, DestroyAction::Start, ON_IT, 1);
    assert!(requests(&started).is_empty(), "{:?}", requests(&started));
    assert!(acknowledges(&started, 1));
}

#[test]
fn an_attack_on_redstone_ore_lights_it_at_the_holder_without_an_answer() {
    let unlit = state("minecraft:redstone_ore[lit=false]");
    let mut west = the_west_with(&[(AT, unlit)], GameMode::Survival, Some(pickaxe()));
    let started = step(&mut west, DestroyAction::Start, AT, 1);
    assert_eq!(
        requests(&started),
        [remote(
            1,
            RemoteAction::UNANSWERED,
            RemoteStep::Attack { position: AT }
        )]
    );
    let mut east = the_east(&[(AT, unlit)]);
    let output = taken(
        &mut east,
        RemoteAction::UNANSWERED,
        RemoteStep::Attack { position: AT },
    );
    assert_eq!(
        east.own_block(AT),
        Some(state("minecraft:redstone_ore[lit=true]"))
    );
    assert!(outcomes(&output).is_empty(), "{:?}", outcomes(&output));
}

// 7. The debug stick.

#[test]
fn a_debug_stick_breaks_nothing_across_the_line_and_the_block_is_shown_here() {
    let poppy = state("minecraft:poppy");
    let stick = ItemStack::new(items::DEBUG_STICK, 1);
    let mut west = the_west_with(&[(ON_IT, poppy)], GameMode::Survival, Some(stick));
    // A poppy breaks at once, so the START is the break: `canDestroyBlock` refuses.
    let started = step(&mut west, DestroyAction::Start, ON_IT, 1);
    assert!(requests(&started).is_empty(), "{:?}", requests(&started));
    assert!(acknowledges(&started, 1));
    assert!(
        started.player_events.contains(&(
            player(1),
            PlayerEvent::BlockShown {
                position: ON_IT,
                state: poppy,
            }
        )),
        "{:?}",
        started.player_events
    );
}

// 8. A creative player.

#[test]
fn a_creative_player_s_break_across_the_line_stays_a_plain_break_and_wears_nothing() {
    let mut west = the_west_with(&[], GameMode::Creative, Some(pickaxe()));
    let started = step(&mut west, DestroyAction::Start, AT, 3);
    assert_eq!(
        requests(&started),
        [remote(1, 3, RemoteStep::Break { position: AT })]
    );
    assert!(!acknowledges(&started, 3));
    assert_eq!(wear(&west), 0);
}
