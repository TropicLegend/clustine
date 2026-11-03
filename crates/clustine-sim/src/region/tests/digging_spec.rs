//! A survival player's dig within a region, tested from the specification by someone who
//! did not write the code: n5 `ServerPlayerGameMode` (`handleBlockBreakAction`, `tick`,
//! `incrementDestroyProgress`, `destroyBlock`, `destroyAndAck`, `abortDestroyBlock`),
//! `BlockBehaviour.getDestroyProgress`, `Player.getDestroySpeed`, `Item.mineBlock`,
//! `ShearsItem.mineBlock` and `ServerGamePacketListenerImpl.handlePlayerAction`.
//!
//! The ticks a dig needs are reckoned here from the game's formulas, in `f32` as the game
//! reckons them: the speed (`Player.getDestroySpeed` @0-140), divided by the hardness and
//! by 30 with the right tool or 100 without (`getDestroyProgress` @0-48); a STOP breaks
//! where that times the ticks since the start plus one is 0.7 or more (@585-607).

use clustine_data::{BlockState, ItemComponents, blocks, items};
use clustine_world::components::{Component, Kind};
use clustine_world::{EntityData, GameMode, PlayerMode};

use super::*;
use crate::api::DestroyAction;
use crate::mobs::body::effects::{Effect, Layer};
use crate::state::Dig;

/// Where the block dug at lies: in the floor, two blocks east of the spawn.
const AT: BlockPos = BlockPos::new(2, -61, 1);

fn state(name: &str) -> BlockState {
    BlockState::parse(name).expect("a block state")
}

/// Makes the player `number` of `region` a survival player on the ground holding `held`
/// in slot 0, and nothing else.
fn make_survival(region: &mut Region, number: u128, held: Option<ItemStack>) {
    let me = region
        .players
        .get_mut(&player(number))
        .expect("the player is here");
    me.mode = PlayerMode::new(GameMode::Survival);
    me.inventory = Inventory::default();
    me.inventory.set(0, held);
    me.selected_slot = 0;
    me.pose.on_ground = true;
}

/// A ticking stone floor with `blocks` set, and player 1 at the spawn in survival
/// holding `held`.
fn digger_with(blocks: &[(BlockPos, BlockState)], held: Option<ItemStack>) -> Region {
    let mut region = on_ticking_floor_with(blocks);
    make_survival(&mut region, 1, held);
    region
}

fn digger(block: BlockState, held: Option<ItemStack>) -> Region {
    digger_with(&[(AT, block)], held)
}

/// As `on_ticking_floor_with`, with players 1 and 2, both survival players holding
/// nothing.
fn two_diggers(block: BlockState) -> Region {
    let mut region = joined_in(ChunkArea::EVERYWHERE, &[1, 2]);
    let around: Vec<ChunkPos> = (-1..=1)
        .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
        .collect();
    region.tick(&tickets(around.clone(), vec![]));
    let chunks = around
        .into_iter()
        .map(|position| {
            let mut chunk = floor();
            if position == ChunkPos::new(0, 0) {
                chunk.set(AT.x as usize, AT.y, AT.z as usize, block);
            }
            (position, chunk)
        })
        .collect();
    region.tick(&TickInputs {
        chunks_loaded: chunks,
        ..TickInputs::default()
    });
    make_survival(&mut region, 1, None);
    make_survival(&mut region, 2, None);
    region
}

fn destroy(number: u128, action: DestroyAction, at: BlockPos, sequence: i32) -> Input {
    numbered(
        number,
        PlayerInput::Destroy {
            action,
            position: at,
            face: Face::Top,
            sequence,
        },
    )
}

/// One tick in which player 1's action is handled.
fn step(region: &mut Region, action: DestroyAction, at: BlockPos, sequence: i32) -> TickOutput {
    region.tick(&moves(vec![destroy(1, action, at, sequence)]))
}

fn idle(region: &mut Region, count: usize) -> Vec<TickOutput> {
    (0..count)
        .map(|_| region.tick(&TickInputs::default()))
        .collect()
}

/// The stages of cracks of the breaker `breaker` the outputs sent, in order.
fn stages_of(breaker: EntityId, outputs: &[TickOutput]) -> Vec<(BlockPos, i32)> {
    outputs
        .iter()
        .flat_map(|output| output.events.iter())
        .filter_map(|event| match event {
            RegionEvent::BlockDestruction {
                breaker: by,
                position,
                stage,
            } if *by == breaker => Some((*position, *stage)),
            _ => None,
        })
        .collect()
}

fn entity_of(region: &Region, number: u128) -> EntityId {
    region.players[&player(number)].entity_id
}

/// The level events 2019 and 2020 of a dig at [`AT`] the output sent.
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

fn block(region: &Region, at: BlockPos) -> Option<BlockState> {
    region.own_block(at)
}

fn dig(region: &Region) -> Dig {
    region.players[&player(1)].dig
}

fn held(region: &Region) -> Option<ItemStack> {
    region.players[&player(1)].inventory.0[0].clone()
}

fn wear(region: &Region) -> i32 {
    match held(region)
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

/// The blocks player 1 alone was shown (`ClientboundBlockUpdatePacket`), checked to come
/// before the acknowledgement of `sequence`: an action is acknowledged after it acted.
fn shown_before(output: &TickOutput, sequence: i32) -> Vec<(BlockPos, BlockState)> {
    let mine: Vec<&PlayerEvent> = output
        .player_events
        .iter()
        .filter(|(id, _)| *id == player(1))
        .map(|(_, event)| event)
        .collect();
    let acknowledgement = mine
        .iter()
        .position(|event| **event == PlayerEvent::Acknowledged { sequence })
        .expect("the action is acknowledged");
    mine.iter()
        .enumerate()
        .filter_map(|(index, event)| match event {
            PlayerEvent::BlockShown { position, state } => {
                assert!(index < acknowledgement, "shown before the acknowledgement");
                Some((*position, *state))
            }
            _ => None,
        })
        .collect()
}

/// `Player.getDestroySpeed` @0-140 for a player with no attributes changed: the item's
/// speed, Haste's `1 + (amplifier + 1) * 0.2`, Mining Fatigue's `0.3^(amplifier + 1)`
/// and a fifth off the ground.
fn destroy_speed(item: f32, haste: Option<i32>, fatigue: Option<i32>, on_ground: bool) -> f32 {
    let mut speed = item;
    if let Some(amplifier) = haste {
        speed *= 1.0 + (amplifier + 1) as f32 * 0.2;
    }
    if let Some(amplifier) = fatigue {
        speed *= 0.3f64.powf(f64::from(amplifier + 1)) as f32;
    }
    if !on_ground {
        speed /= 5.0;
    }
    speed
}

/// `getDestroyProgress` @0-48.
fn progress(speed: f32, hardness: f32, correct: bool) -> f32 {
    speed / hardness / if correct { 30.0 } else { 100.0 }
}

/// The fewest ticks since the start a STOP needs to break, by @585-607.
fn ticks_to_break(progress: f32) -> i32 {
    (0..)
        .find(|ticks| progress * (ticks + 1) as f32 >= 0.7)
        .unwrap()
}

/// A START, then idle ticks so that the STOP's tick is `ticks` after the START's (the
/// START's own tick counts the dig's first), and the STOP's output.
fn dig_for(region: &mut Region, ticks: i32) -> TickOutput {
    let started = step(region, DestroyAction::Start, AT, 1);
    assert!(acknowledges(&started, 1));
    idle(region, usize::try_from(ticks - 1).unwrap());
    step(region, DestroyAction::Stop, AT, 2)
}

/// Whether a dig of `block` for exactly `ticks` breaks it, with the player set up by
/// `set_up` and holding `held`, and that one tick fewer leaves a delayed destroy.
fn breaks_in_exactly(
    block: BlockState,
    held: Option<ItemStack>,
    set_up: impl Fn(&mut Region),
    ticks: i32,
) {
    let mut region = digger(block, held.clone());
    set_up(&mut region);
    let stopped = dig_for(&mut region, ticks);
    assert!(acknowledges(&stopped, 2));
    assert_eq!(block_at(&region), Some(blocks::AIR), "{ticks} ticks break");

    let mut region = digger(block, held);
    set_up(&mut region);
    let stopped = dig_for(&mut region, ticks - 1);
    assert!(acknowledges(&stopped, 2));
    assert_eq!(block_at(&region), Some(block), "{} ticks do not", ticks - 1);
    let waiting = dig(&region);
    assert!(waiting.has_delayed && !waiting.is_destroying);
    assert_eq!(waiting.delayed_pos, AT);
}

fn block_at(region: &Region) -> Option<BlockState> {
    block(region, AT)
}

fn give_effect(region: &mut Region, effect: Effect, amplifier: u8) {
    let now = region.tick;
    let me = region.players.get_mut(&player(1)).expect("player 1");
    me.effects.add(effect, Layer::new(amplifier, -1, now), now);
}

#[test]
fn stone_dug_with_a_wooden_pickaxe_breaks_at_a_stop_fifteen_ticks_on_and_not_fourteen() {
    // A wooden pickaxe's speed on stone is 2, its hardness 1.5, and the pickaxe is right
    // for it: 2 / 1.5 / 30 a tick, 0.71 at sixteen, 0.67 at fifteen.
    let ticks = ticks_to_break(progress(2.0, 1.5, true));
    assert_eq!(ticks, 15);
    let pickaxe = Some(ItemStack::new(items::WOODEN_PICKAXE, 1));
    breaks_in_exactly(blocks::STONE, pickaxe.clone(), |_| {}, ticks);

    let mut region = digger(blocks::STONE, pickaxe);
    dig_for(&mut region, ticks);
    assert_eq!(
        items_lying(&region),
        [ItemStack::new(items::COBBLESTONE, 1)]
    );
}

#[test]
fn a_stop_at_another_block_than_the_dig_s_does_nothing_but_is_acknowledged() {
    // @538-544: the STOP's position is compared with the dig's; elsewhere nothing is
    // done, the dig goes on, and the action is acknowledged all the same.
    let beside = AT.offset(1, 0, 0);
    let mut region = digger_with(&[(AT, blocks::DIRT), (beside, blocks::DIRT)], None);
    step(&mut region, DestroyAction::Start, AT, 1);
    idle(&mut region, 20);
    let stopped = step(&mut region, DestroyAction::Stop, beside, 2);
    assert!(acknowledges(&stopped, 2));
    assert_eq!(block(&region, AT), Some(blocks::DIRT));
    assert_eq!(block(&region, beside), Some(blocks::DIRT));
    let now = dig(&region);
    assert!(now.is_destroying && !now.has_delayed);
    assert_eq!(now.destroy_pos, AT);
    assert!(items_lying(&region).is_empty());
    // The dig of `AT` is still the one under way: a STOP there breaks it.
    let stopped = step(&mut region, DestroyAction::Stop, AT, 3);
    assert!(acknowledges(&stopped, 3));
    assert_eq!(block(&region, AT), Some(blocks::AIR));
}

#[test]
fn haste_two_digs_stone_in_fewer_ticks_and_mining_fatigue_one_in_more() {
    let pickaxe = Some(ItemStack::new(items::WOODEN_PICKAXE, 1));
    // Haste II, amplifier 1: 2 * 1.4 = 2.8, 2.8 / 1.5 / 30 a tick, eleven ticks.
    let hasted = ticks_to_break(progress(destroy_speed(2.0, Some(1), None, true), 1.5, true));
    assert_eq!(hasted, 11);
    breaks_in_exactly(
        blocks::STONE,
        pickaxe.clone(),
        |region| give_effect(region, Effect::HASTE, 1),
        hasted,
    );
    // Mining Fatigue I, amplifier 0: 2 * 0.3 = 0.6, 0.6 / 1.5 / 30 a tick, 52 ticks.
    let tired = ticks_to_break(progress(destroy_speed(2.0, None, Some(0), true), 1.5, true));
    assert_eq!(tired, 52);
    breaks_in_exactly(
        blocks::STONE,
        pickaxe,
        |region| give_effect(region, Effect::MINING_FATIGUE, 0),
        tired,
    );
}

#[test]
fn a_player_off_the_ground_digs_five_times_slower() {
    // Dirt by hand: 1 / 0.5 / 30 a tick on the ground, ten ticks; off it a fifth of
    // that, 0.2 / 0.5 / 30, 52 ticks.
    let grounded = ticks_to_break(progress(destroy_speed(1.0, None, None, true), 0.5, true));
    assert_eq!(grounded, 10);
    breaks_in_exactly(blocks::DIRT, None, |_| {}, grounded);
    let flying = ticks_to_break(progress(destroy_speed(1.0, None, None, false), 0.5, true));
    assert_eq!(flying, 52);
    breaks_in_exactly(
        blocks::DIRT,
        None,
        |region| {
            let me = region.players.get_mut(&player(1)).expect("player 1");
            me.pose.on_ground = false;
        },
        flying,
    );
}

#[test]
fn shears_breaking_a_sapling_wear_by_one_though_its_hardness_is_0() {
    // `ShearsItem.mineBlock` @26-55 asks only that the block is not in `#fire` and that
    // the tool's damage per block is above 0, not the hardness that `Item.mineBlock`
    // @26-35 asks of: shears wear on a sapling, which breaks at the START.
    let above = AT.offset(0, 1, 0);
    let sapling = state("minecraft:oak_sapling");
    let shears = ItemStack::new(items::SHEARS, 1);
    let mut region = digger_with(&[(AT, blocks::DIRT), (above, sapling)], Some(shears));
    let started = step(&mut region, DestroyAction::Start, above, 1);
    assert!(acknowledges(&started, 1));
    assert_eq!(block(&region, above), Some(blocks::AIR));
    assert_eq!(wear(&region), 1);
}

#[test]
fn a_pickaxe_breaking_a_sapling_does_not_wear() {
    // `Item.mineBlock` @26-35: a block whose hardness is 0 does not wear the tool.
    let above = AT.offset(0, 1, 0);
    let sapling = state("minecraft:oak_sapling");
    let pickaxe = ItemStack::new(items::WOODEN_PICKAXE, 1);
    let mut region = digger_with(&[(AT, blocks::DIRT), (above, sapling)], Some(pickaxe));
    let started = step(&mut region, DestroyAction::Start, above, 1);
    assert!(acknowledges(&started, 1));
    assert_eq!(block(&region, above), Some(blocks::AIR));
    assert_eq!(wear(&region), 0);
    assert_eq!(
        items_lying(&region),
        [ItemStack::new(items::OAK_SAPLING, 1)]
    );
}

#[test]
fn shears_breaking_fire_do_not_wear() {
    // `ShearsItem.mineBlock` @26-33: a block in `#fire` does not wear shears.
    let above = AT.offset(0, 1, 0);
    let fire = state("minecraft:fire");
    let shears = ItemStack::new(items::SHEARS, 1);
    let mut region = digger_with(&[(AT, blocks::DIRT), (above, fire)], Some(shears));
    let started = step(&mut region, DestroyAction::Start, above, 1);
    assert!(acknowledges(&started, 1));
    assert_eq!(block(&region, above), Some(blocks::AIR));
    assert_eq!(wear(&region), 0);
}

#[test]
fn shears_breaking_a_cobweb_wear_by_one_and_drop_it() {
    // Shears on a cobweb: speed 15, hardness 4, the right tool: 15 / 4 / 30 a tick.
    let ticks = ticks_to_break(progress(15.0, 4.0, true));
    let shears = ItemStack::new(items::SHEARS, 1);
    let mut region = digger(state("minecraft:cobweb"), Some(shears));
    let stopped = dig_for(&mut region, ticks);
    assert!(acknowledges(&stopped, 2));
    assert_eq!(block_at(&region), Some(blocks::AIR));
    assert_eq!(wear(&region), 1);
    assert_eq!(items_lying(&region), [ItemStack::new(items::COBWEB, 1)]);
}

#[test]
fn a_pickaxe_on_its_last_point_breaks_stone_drops_cobblestone_and_is_gone() {
    // `destroyBlock` @205-212 copies the stack before `mineBlock` wears it out
    // (@241), and `playerDestroy` drops with the copy (@269).
    let most = ItemComponents::of(items::WOODEN_PICKAXE)
        .max_damage
        .expect("a pickaxe wears");
    let mut pickaxe = ItemStack::new(items::WOODEN_PICKAXE, 1);
    pickaxe
        .components
        .set(Component::Damage(i32::from(most) - 1));
    let mut region = digger(blocks::STONE, Some(pickaxe));
    let stopped = dig_for(&mut region, 15);
    assert!(acknowledges(&stopped, 2));
    assert_eq!(block_at(&region), Some(blocks::AIR));
    assert_eq!(
        items_lying(&region),
        [ItemStack::new(items::COBBLESTONE, 1)]
    );
    assert_eq!(held(&region), None);
}

#[test]
fn a_debug_stick_breaks_nothing_and_the_client_is_shown_the_block() {
    // `destroyBlock` @28-31: `canDestroyBlock` is false for a debug stick, so
    // `destroyAndAck` shows the block (@20-42) and the action is acknowledged. A poppy
    // breaks at the START (hardness 0), dirt at a STOP ten ticks on.
    let above = AT.offset(0, 1, 0);
    let poppy = state("minecraft:poppy");
    let stick = ItemStack::new(items::DEBUG_STICK, 1);
    let mut region = digger_with(&[(AT, blocks::DIRT), (above, poppy)], Some(stick.clone()));
    let started = step(&mut region, DestroyAction::Start, above, 1);
    assert!(acknowledges(&started, 1));
    assert_eq!(block(&region, above), Some(poppy));
    assert_eq!(shown_before(&started, 1), [(above, poppy)]);
    assert!(items_lying(&region).is_empty());

    let mut region = digger(blocks::DIRT, Some(stick));
    let started = step(&mut region, DestroyAction::Start, AT, 1);
    assert!(acknowledges(&started, 1));
    assert_eq!(block_at(&region), Some(blocks::DIRT));
    idle(&mut region, 9);
    let stopped = step(&mut region, DestroyAction::Stop, AT, 2);
    assert!(acknowledges(&stopped, 2));
    assert_eq!(block_at(&region), Some(blocks::DIRT));
    assert_eq!(shown_before(&stopped, 2), [(AT, blocks::DIRT)]);
    assert!(items_lying(&region).is_empty());
}

#[test]
fn obsidian_dug_with_a_diamond_pickaxe_shows_each_stage_once_in_order_and_every_fourth_tick_2020() {
    // 8 / 50 / 30 a tick: stage 9 at 169 ticks, 10 only at 188; 180 ticks of dig.
    let pickaxe = ItemStack::new(items::DIAMOND_PICKAXE, 1);
    let mut region = digger(blocks::OBSIDIAN, Some(pickaxe));
    let me = entity_of(&region, 1);
    let mut outputs = vec![step(&mut region, DestroyAction::Start, AT, 1)];
    outputs.extend(idle(&mut region, 179));
    assert_eq!(
        stages_of(me, &outputs),
        (0..=9).map(|stage| (AT, stage)).collect::<Vec<_>>()
    );
    // `tick` @139-194: the dig's k-th tick plays 2020 where k % 4 is 0, else 2019, with
    // the face's ordinal (up is 1).
    for (index, output) in outputs.iter().enumerate() {
        let k = index + 1;
        let kind = if k % 4 == 0 { 2020 } else { 2019 };
        assert_eq!(dig_events(output), [(kind, 1)], "tick {k} of the dig");
    }
    assert_eq!(block_at(&region), Some(blocks::OBSIDIAN));
}

#[test]
fn a_start_above_the_build_limit_shows_air_and_is_acknowledged() {
    // @44-90: above the level's top (319 in the overworld) the client is shown the
    // block there, air, and nothing else is done.
    let mut region = digger(blocks::DIRT, None);
    let me = region.players.get_mut(&player(1)).expect("player 1");
    me.pose.position = Vec3::new(0.5, 317.0, 0.5);
    let high = BlockPos::new(0, 320, 0);
    let started = step(&mut region, DestroyAction::Start, high, 1);
    assert!(acknowledges(&started, 1));
    assert_eq!(shown_before(&started, 1), [(high, blocks::AIR)]);
    assert!(!dig(&region).is_destroying);
    assert!(stages_of(entity_of(&region, 1), &[started]).is_empty());
}

#[test]
fn a_start_on_a_note_block_plays_it() {
    // @343-351: the block's `attack`, `NoteBlock.attack` plays its note (a block event
    // of kind 0).
    let note = state("minecraft:note_block");
    let mut region = digger(note, None);
    let started = step(&mut region, DestroyAction::Start, AT, 1);
    assert!(acknowledges(&started, 1));
    let played: Vec<_> = started
        .events
        .iter()
        .filter(|event| {
            matches!(
                event,
                RegionEvent::BlockEvent { position, kind: 0, .. } if *position == AT
            )
        })
        .collect();
    assert_eq!(played.len(), 1, "{:?}", started.events);
    assert!(dig(&region).is_destroying);
}

#[test]
fn a_start_on_redstone_ore_lights_it() {
    // `RedStoneOreBlock.attack` lights the ore; the dig begins.
    let ore = state("minecraft:redstone_ore[lit=false]");
    let mut region = digger(ore, None);
    let started = step(&mut region, DestroyAction::Start, AT, 1);
    assert!(acknowledges(&started, 1));
    assert_eq!(
        block_at(&region),
        Some(state("minecraft:redstone_ore[lit=true]"))
    );
    assert!(dig(&region).is_destroying);
}

#[test]
fn when_one_of_two_diggers_breaks_the_block_the_other_s_dig_ends_with_stage_minus_one() {
    // `tick` @100-136: a dig whose block became air sends -1 and stops.
    let mut region = two_diggers(blocks::DIRT);
    let (first, second) = (entity_of(&region, 1), entity_of(&region, 2));
    region.tick(&moves(vec![
        destroy(1, DestroyAction::Start, AT, 1),
        destroy(2, DestroyAction::Start, AT, 1),
    ]));
    idle(&mut region, 9);
    let broken = region.tick(&moves(vec![destroy(2, DestroyAction::Stop, AT, 2)]));
    assert!(told(&broken).contains(&acknowledged(2, 2)));
    assert_eq!(block_at(&region), Some(blocks::AIR));
    assert_eq!(stages_of(second, std::slice::from_ref(&broken)), [(AT, -1)]);
    // Player 1's tick after the break, in the same tick as the action that broke it.
    assert_eq!(stages_of(first, std::slice::from_ref(&broken)), [(AT, -1)]);
    assert!(!region.players[&player(1)].dig.is_destroying);
    let after = idle(&mut region, 2);
    assert!(stages_of(first, &after).is_empty());
    assert!(dig_events(&after[0]).is_empty());
}
