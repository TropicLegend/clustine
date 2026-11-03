//! Tests of an operator's orders (`clustine gamemode`, `clustine give`; V1, step three),
//! written from the specification by someone who did not write the code: the design
//! `v1-orders-design.md` with its review ("After the review" overrides what comes before
//! it), and the game's classes. A change of mode is `ServerPlayer.setGameMode` and
//! `ServerPlayerGameMode.changeGameModeForPlayer` (false when the mode is the one the
//! player has, else the previous set, the abilities from `GameType.updatePlayerAbilities`,
//! everyone's list told `UPDATE_GAME_MODE`); a gift is `GiveCommand.giveItem`: stacks of
//! at most the item's largest, each `Inventory.add`ed (n6 `Inventory.add(int,
//! ItemStack)` @0-101: a creative player's stack that found no room at all is counted
//! added and emptied, @63-87), the pickup sound where the stack is empty afterwards,
//! else what is left dropped from the player.
//!
//! An order travels in the player's input stream (`PlayerInput::Order`); it holds for a
//! dead player and for a stay that waits for its respawn (the review, item 6). Where the
//! code and the specification disagree, the test says what the specification says and
//! is kept ignored as a finding.

use clustine_data::{BlockState, blocks, sounds};
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkPos, Dimension, EdgeId, EntityId, EntityIds, GameMode, PlayerId,
    PlayerMode, RegionId, Vec3,
};

use super::super::tnt::tests::{FLOOR, add_player, at};
use super::super::{Holdings, Region, RegionConfig};
use super::{items, items_lying, knowing_the_edges, numbered, on_ticking_floor_with, player};
use crate::api::{
    Durable, EdgeEvent, Inventory, ItemStack, Misdirected, Order, PlayerChange, PlayerEvent,
    PlayerInput, PlayerTransfer, RegionEvent, SOUND_SOURCE_PLAYERS, TickInputs, TickOutput, Ticket,
};

// --- The region of one player on a floor ----------------------------------------------

/// The region of `on_ticking_floor_with(&[])`, player 1 at the spawn in `mode`, flying
/// as `flying` says, with nothing in their inventory.
fn as_player(mode: GameMode, flying: bool) -> Region {
    let mut region = on_ticking_floor_with(&[]);
    let me = region
        .players
        .get_mut(&player(1))
        .expect("player 1 is here");
    me.mode = PlayerMode::new(mode);
    me.flying = flying;
    me.inventory = Inventory::default();
    me.selected_slot = 0;
    region
}

/// Player 1 is given `order`, as their edge passes it on.
fn ordered(region: &mut Region, order: Order) -> TickOutput {
    region.tick(&TickInputs {
        inputs: vec![numbered(1, PlayerInput::Order(order))],
        ..TickInputs::default()
    })
}

fn give(item: i32, count: i32) -> Order {
    Order::Give { item, count }
}

fn slot(region: &Region, slot: usize) -> Option<ItemStack> {
    region.players[&player(1)].inventory.0[slot].clone()
}

fn stacked(item: i32, count: i32) -> Option<ItemStack> {
    Some(ItemStack::new(item, count))
}

fn mode_changes(output: &TickOutput, id: PlayerId) -> Vec<(PlayerMode, bool)> {
    (output.player_events.iter())
        .filter(|(of, _)| *of == id)
        .filter_map(|(_, event)| match event {
            PlayerEvent::GameModeChanged { mode, flying } => Some((*mode, *flying)),
            _ => None,
        })
        .collect()
}

/// The `PlayerGameMode` events of the tick: (entity, chunk, player, mode's id).
fn list_updates(output: &TickOutput) -> Vec<(EntityId, ChunkPos, PlayerId, u8)> {
    (output.events.iter())
        .filter_map(|event| match event {
            RegionEvent::PlayerGameMode {
                entity,
                chunk,
                player,
                mode,
            } => Some((*entity, *chunk, *player, *mode)),
            _ => None,
        })
        .collect()
}

/// The pickup sounds of the tick: (position, volume, pitch).
fn pickup_sounds(output: &TickOutput) -> Vec<(Vec3, f32, f32)> {
    (output.events.iter())
        .filter_map(|event| match event {
            RegionEvent::Sound {
                sound,
                source,
                position,
                volume,
                pitch,
                ..
            } if *sound == sounds::ENTITY_ITEM_PICKUP => {
                assert_eq!(*source, SOUND_SOURCE_PLAYERS, "the players' source");
                Some((*position, *volume, *pitch))
            }
            _ => None,
        })
        .collect()
}

fn told_slots(output: &TickOutput) -> Vec<(u8, Option<ItemStack>)> {
    (output.player_events.iter())
        .filter_map(|(_, event)| match event {
            PlayerEvent::InventorySlot { slot, stack } => Some((*slot, stack.clone())),
            _ => None,
        })
        .collect()
}

/// The stacks lying in the region, as (item, count), in order.
fn lying(region: &Region) -> Vec<(i32, i32)> {
    let mut lying: Vec<(i32, i32)> = (items_lying(region).into_iter())
        .map(|(_, stack)| (stack.item, stack.count))
        .collect();
    lying.sort_unstable();
    lying
}

/// Every slot of the inventory's 36 holds 64 dirt.
fn full_of_dirt(region: &mut Region) {
    let me = region.players.get_mut(&player(1)).expect("here");
    for slot in 0..36 {
        me.inventory.set(slot, stacked(items::DIRT, 64));
    }
}

// --- The game mode ------------------------------------------------------------------

/// `changeGameModeForPlayer`: the creative mode becomes the previous one, survival's
/// abilities (`GameType.updatePlayerAbilities`: `mayfly` false) end the flying; the
/// client is told, and every player's list (`UPDATE_GAME_MODE` to all players).
#[test]
fn a_creative_flyer_set_to_survival_stops_flying_and_is_told_with_the_previous_mode() {
    let mut region = as_player(GameMode::Creative, true);
    let output = ordered(&mut region, Order::GameMode(GameMode::Survival));
    let expected = PlayerMode {
        current: GameMode::Survival,
        previous: Some(GameMode::Creative),
    };
    let me = &region.players[&player(1)];
    assert_eq!(me.mode, expected);
    assert!(!me.flying, "a survival player does not fly");
    assert_eq!(mode_changes(&output, player(1)), [(expected, false)]);
    assert_eq!(
        list_updates(&output),
        [(EntityId(1), ChunkPos::new(0, 0), player(1), 0)],
        "the others' lists are told survival (id 0)"
    );
    // It is the player's: a later tick keeps it, and so does their state.
    let output = region.tick(&TickInputs::default());
    assert!(mode_changes(&output, player(1)).is_empty());
    let state = region.player_state(player(1)).expect("here");
    assert_eq!(state.mode, expected);
    assert!(!state.flying);
}

/// A creative flyer set to creative again, or anyone set to the mode they have:
/// `changeGameModeForPlayer` returns false, and `setGameMode` does nothing more.
#[test]
fn the_same_mode_twice_does_nothing() {
    let mut region = as_player(GameMode::Creative, true);
    let output = ordered(&mut region, Order::GameMode(GameMode::Creative));
    assert!(mode_changes(&output, player(1)).is_empty());
    assert!(list_updates(&output).is_empty());
    let me = &region.players[&player(1)];
    assert_eq!(me.mode, PlayerMode::new(GameMode::Creative), "no previous");
    assert!(me.flying, "still flying");

    ordered(&mut region, Order::GameMode(GameMode::Survival));
    let output = ordered(&mut region, Order::GameMode(GameMode::Survival));
    assert!(mode_changes(&output, player(1)).is_empty());
    assert!(list_updates(&output).is_empty());
    assert_eq!(
        region.players[&player(1)].mode,
        PlayerMode {
            current: GameMode::Survival,
            previous: Some(GameMode::Creative),
        },
        "the previous is still creative, not survival"
    );
}

/// Survival back to creative: may fly again, but does not fly until the client starts.
#[test]
fn a_survival_player_set_to_creative_may_fly_but_is_not_flying() {
    let mut region = as_player(GameMode::Survival, false);
    let output = ordered(&mut region, Order::GameMode(GameMode::Creative));
    let expected = PlayerMode {
        current: GameMode::Creative,
        previous: Some(GameMode::Survival),
    };
    assert_eq!(mode_changes(&output, player(1)), [(expected, false)]);
    assert_eq!(
        list_updates(&output),
        [(EntityId(1), ChunkPos::new(0, 0), player(1), 1)]
    );
}

// --- Giving -------------------------------------------------------------------------

/// `GiveCommand.giveItem`: 64 stone is one stack, which `Inventory.add` puts in the first
/// free slot whole; the pickup sound is played at the player, 0.2 loud, its pitch
/// `((a − b) · 0.7 + 1) · 2` with a and b in [0, 1).
#[test]
fn give_sixty_four_stone_fills_a_slot_and_plays_the_pickup_sound() {
    for mode in [GameMode::Survival, GameMode::Creative] {
        let mut region = as_player(mode, false);
        let output = ordered(&mut region, give(items::STONE, 64));
        assert_eq!(slot(&region, 0), stacked(items::STONE, 64), "{mode:?}");
        assert_eq!(slot(&region, 1), None, "{mode:?}");
        assert_eq!(
            told_slots(&output),
            [(0, stacked(items::STONE, 64))],
            "{mode:?}"
        );
        let sounds = pickup_sounds(&output);
        let [(position, volume, pitch)] = sounds[..] else {
            panic!("one pickup sound for one stack ({mode:?}): {sounds:?}");
        };
        assert_eq!(position, region.players[&player(1)].pose.position);
        assert_eq!(volume, 0.2);
        assert!((0.6..=3.4).contains(&pitch), "pitch {pitch}");
        // The fake item is never anyone's to pick up: nothing lies on the ground.
        for _ in 0..3 {
            region.tick(&TickInputs::default());
        }
        assert!(lying(&region).is_empty(), "{mode:?}: {:?}", lying(&region));
    }
}

/// 130 stone is stacks of 64, 64 and 2 (the loop of `giveItem`, `min(max, left)`), each
/// added in turn: the first fills slot 0, the second finds no room there and takes slot
/// 1, the third slot 2. Each played its sound.
#[test]
fn give_one_hundred_and_thirty_stone_makes_stacks_of_sixty_four_sixty_four_and_two() {
    let mut region = as_player(GameMode::Survival, false);
    let output = ordered(&mut region, give(items::STONE, 130));
    assert_eq!(slot(&region, 0), stacked(items::STONE, 64));
    assert_eq!(slot(&region, 1), stacked(items::STONE, 64));
    assert_eq!(slot(&region, 2), stacked(items::STONE, 2));
    assert_eq!(slot(&region, 3), None);
    assert_eq!(pickup_sounds(&output).len(), 3);
    assert!(lying(&region).is_empty());
}

/// An item whose largest stack is 16 (an ender pearl) is given in stacks of 16; one of 1
/// (a diamond sword) in single items.
#[test]
fn give_splits_by_the_item_s_own_largest_stack() {
    let mut region = as_player(GameMode::Survival, false);
    ordered(&mut region, give(items::ENDER_PEARL, 20));
    assert_eq!(slot(&region, 0), stacked(items::ENDER_PEARL, 16));
    assert_eq!(slot(&region, 1), stacked(items::ENDER_PEARL, 4));
    let output = ordered(&mut region, give(items::DIAMOND_SWORD, 3));
    for at in 2..5 {
        let found = slot(&region, at).expect("a sword");
        assert_eq!((found.item, found.count), (items::DIAMOND_SWORD, 1));
    }
    assert_eq!(pickup_sounds(&output).len(), 3);
}

/// `Inventory.add` fills a stack of the same item with room first: 10 stone onto 60 in
/// slot 0 makes it 64, the 6 left go to the first free slot.
#[test]
fn give_tops_up_a_stack_of_the_same_item_before_a_free_slot() {
    let mut region = as_player(GameMode::Survival, false);
    region
        .players
        .get_mut(&player(1))
        .expect("here")
        .inventory
        .set(0, stacked(items::STONE, 60));
    let output = ordered(&mut region, give(items::STONE, 10));
    assert_eq!(slot(&region, 0), stacked(items::STONE, 64));
    assert_eq!(slot(&region, 1), stacked(items::STONE, 6));
    assert_eq!(pickup_sounds(&output).len(), 1);
}

/// A survival player whose 36 slots are full of dirt: the stone finds no room
/// (`Inventory.add` false), so `giveItem` drops it from the player (`player.drop(stack,
/// false)`) and plays no sound; it lies in the world whole. A stack partly added
/// (6 of 10 onto 60) drops the 4 left.
#[test]
fn a_survival_player_s_full_inventory_drops_what_does_not_fit() {
    let mut region = as_player(GameMode::Survival, false);
    full_of_dirt(&mut region);
    let output = ordered(&mut region, give(items::STONE, 10));
    assert!(pickup_sounds(&output).is_empty());
    assert_eq!(lying(&region), [(items::STONE, 10)]);
    for at in 0..36 {
        assert_eq!(slot(&region, at), stacked(items::DIRT, 64));
    }

    let mut region = as_player(GameMode::Survival, false);
    full_of_dirt(&mut region);
    region
        .players
        .get_mut(&player(1))
        .expect("here")
        .inventory
        .set(0, stacked(items::STONE, 60));
    let output = ordered(&mut region, give(items::STONE, 10));
    assert!(pickup_sounds(&output).is_empty(), "not added whole");
    assert_eq!(slot(&region, 0), stacked(items::STONE, 64));
    assert_eq!(lying(&region), [(items::STONE, 6)]);
}

/// A creative player whose 36 slots are full: `Inventory.add` finds no room at all, and
/// for a player with infinite materials empties the stack and returns true (@63-87):
/// `giveItem` takes it as added, plays the sound, and nothing is dropped.
#[test]
fn a_creative_player_s_full_inventory_loses_it() {
    let mut region = as_player(GameMode::Creative, false);
    full_of_dirt(&mut region);
    let output = ordered(&mut region, give(items::STONE, 10));
    assert_eq!(pickup_sounds(&output).len(), 1);
    for _ in 0..3 {
        region.tick(&TickInputs::default());
    }
    assert!(lying(&region).is_empty(), "{:?}", lying(&region));
    for at in 0..36 {
        assert_eq!(slot(&region, at), stacked(items::DIRT, 64));
    }
}

/// A creative player with room for part of the stack: `Inventory.add` empties the stack
/// only where none of it was added (@63-68: the count is still the first one); where
/// some went in it returns true with the rest in the stack (@88-101), which is not empty,
/// so `giveItem` drops the rest from the player (`player.drop`) and plays no sound.
#[test]
#[ignore = "finding: the region loses a creative player's rest where part of it was added"]
fn a_creative_player_s_partly_full_inventory_drops_the_rest() {
    let mut region = as_player(GameMode::Creative, false);
    full_of_dirt(&mut region);
    region
        .players
        .get_mut(&player(1))
        .expect("here")
        .inventory
        .set(0, stacked(items::STONE, 60));
    let output = ordered(&mut region, give(items::STONE, 10));
    assert_eq!(slot(&region, 0), stacked(items::STONE, 64));
    assert!(
        pickup_sounds(&output).is_empty(),
        "the stack was not emptied"
    );
    assert_eq!(lying(&region), [(items::STONE, 6)]);
}

/// An order for a player the region does not have does nothing (it has nobody to apply
/// it to) and breaks nothing.
#[test]
fn an_order_for_nobody_here_does_nothing() {
    let mut region = as_player(GameMode::Creative, false);
    let output = region.tick(&TickInputs {
        inputs: vec![numbered(9, PlayerInput::Order(give(items::STONE, 5)))],
        ..TickInputs::default()
    });
    assert!(pickup_sounds(&output).is_empty());
    assert!(lying(&region).is_empty());
    assert_eq!(slot(&region, 0), None);
}

// --- Dead, and in the credits ---------------------------------------------------------
//
// Worlds as `respawn_specification.rs` has them: regions of the overworld and the End
// with a stone floor, held whole or on either side of the line at block x = 16; the
// overworld's home holds the world spawn's chunk (0, 0). The store's part is played by
// hand.

const PLAYERS_EDGE: EdgeId = EdgeId(1);
const OVERWORLD_HOME: RegionId = RegionId(0);
const NETHER_HOME: RegionId = RegionId(1);
const END_HOME: RegionId = RegionId(2);
const EAST: RegionId = RegionId(5);
const WORLD_SPAWN: BlockPos = BlockPos::new(0, FLOOR + 1, 0);
const HIGH_FLOOR: i32 = 40;
const SOON: usize = 8;

fn floor_of(dimension: Dimension) -> i32 {
    if dimension == Dimension::Overworld {
        FLOOR
    } else {
        HIGH_FLOOR
    }
}

fn columns(xs: std::ops::RangeInclusive<i32>) -> Vec<ChunkPos> {
    xs.flat_map(|x| (-2..=2).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

struct Land {
    dimension: Dimension,
    held: Vec<ChunkPos>,
    copies: Vec<ChunkPos>,
    foreign: Vec<(ChunkPos, RegionId)>,
}

impl Land {
    fn whole(dimension: Dimension) -> Self {
        Self {
            dimension,
            held: columns(-2..=2),
            copies: Vec::new(),
            foreign: Vec::new(),
        }
    }

    fn west_of_the_line(dimension: Dimension, other: RegionId) -> Self {
        Self {
            dimension,
            held: columns(-2..=0),
            copies: columns(1..=1),
            foreign: (columns(1..=3).into_iter())
                .map(|chunk| (chunk, other))
                .collect(),
        }
    }

    fn east_of_the_line(dimension: Dimension, other: RegionId) -> Self {
        Self {
            dimension,
            held: columns(1..=3),
            copies: columns(0..=0),
            foreign: (columns(-2..=0).into_iter())
                .map(|chunk| (chunk, other))
                .collect(),
        }
    }
}

fn chunk_of(
    dimension: Dimension,
    position: ChunkPos,
    blocks: &dyn Fn(BlockPos) -> Option<BlockState>,
) -> Chunk {
    let floor = floor_of(dimension);
    let mut chunk = Chunk::empty(dimension.rules(), Biome(0));
    for z in 0..16 {
        for x in 0..16 {
            chunk.set(x, floor, z, blocks::STONE);
            for y in floor + 1..=floor + 12 {
                let pos = at(position.x * 16 + x as i32, y, position.z * 16 + z as i32);
                if let Some(state) = blocks(pos) {
                    chunk.set(x, y, z, state);
                }
            }
        }
    }
    chunk
}

fn region(land: Land, block: u32, blocks: impl Fn(BlockPos) -> Option<BlockState>) -> Region {
    let dimension = land.dimension;
    let rules = dimension.rules();
    let chunks = (land.held.iter())
        .map(|position| (*position, chunk_of(dimension, *position, &blocks)))
        .collect();
    let copies = (land.copies.iter())
        .map(|position| (*position, chunk_of(dimension, *position, &blocks)))
        .collect();
    let tickets: Vec<(ChunkPos, Ticket)> = (land.held.iter())
        .map(|position| (*position, Ticket::Viewer))
        .collect();
    let holdings = Holdings {
        held: land.held.clone(),
        pinned: Vec::new(),
        lent: Vec::new(),
        lent_away: Vec::new(),
    };
    let spawn =
        (dimension == Dimension::Overworld).then(|| Vec3::new(0.5, f64::from(FLOOR + 1), 0.5));
    let config = RegionConfig {
        world_spawn: WORLD_SPAWN,
        strongholds: Vec::new(),
        homes: vec![
            (Dimension::Overworld, OVERWORLD_HOME),
            (Dimension::Nether, NETHER_HOME),
            (Dimension::End, END_HOME),
        ],
        dimension,
        spawn,
        starting_inventory: Inventory::default(),
        default_mode: GameMode::Creative,
        force_mode: false,
        return_after: u64::MAX,
        lowest_y: rules.min_y,
        seed: 11,
        game_seed: 0,
        sea_level: 63,
        spawn_mobs: false,
        light: Default::default(),
        features: Default::default(),
        random_tick_speed: crate::region::RANDOM_TICK_SPEED,
    };
    let state = knowing_the_edges(EntityIds::block(block).expect("a block of ids"));
    let mut region = Region::restore_loaded(config, state, holdings, &tickets, chunks, copies);
    region.tick(&TickInputs {
        edges: vec![EdgeEvent::Started {
            edge: PLAYERS_EDGE,
            start: 1,
        }],
        ..TickInputs::default()
    });
    if !land.foreign.is_empty() {
        let wanted = (land.foreign.iter()).map(|(chunk, _)| (*chunk, Ticket::Viewer));
        region.tick(&TickInputs {
            tickets_added: wanted.collect(),
            ..TickInputs::default()
        });
        region.tick(&TickInputs {
            foreign: land.foreign.clone(),
            ..TickInputs::default()
        });
    }
    region
}

fn nothing(_: BlockPos) -> Option<BlockState> {
    None
}

fn idle(region: &mut Region) -> TickOutput {
    region.tick(&TickInputs::default())
}

fn next_number() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn input(id: PlayerId, entity: EntityId, input: PlayerInput) -> TickInputs {
    TickInputs {
        inputs: vec![(PLAYERS_EDGE, id, entity, next_number(), input)],
        ..TickInputs::default()
    }
}

fn arrive(id: PlayerId, transfer: PlayerTransfer) -> TickInputs {
    TickInputs {
        player_changes: vec![PlayerChange::Arrive(PLAYERS_EDGE, id, transfer)],
        ..TickInputs::default()
    }
}

fn let_go(output: &TickOutput, id: PlayerId) -> Option<(PlayerTransfer, RegionId)> {
    output.durable.iter().find_map(|(_, _, entry)| match entry {
        Durable::Departed {
            player,
            transfer,
            to,
        } if *player == id => Some((transfer.clone(), *to)),
        Durable::NotMine {
            what: Misdirected::Arrival { player, transfer },
            holder,
        } if *player == id => Some((transfer.clone(), *holder)),
        _ => None,
    })
}

fn placed(output: &TickOutput, id: PlayerId) -> bool {
    (output.durable.iter())
        .any(|(_, _, entry)| matches!(entry, Durable::Placed { player, .. } if *player == id))
}

fn died(output: &TickOutput, id: PlayerId) -> bool {
    (output.durable.iter())
        .any(|(_, _, entry)| matches!(entry, Durable::Died { player, .. } if *player == id))
}

fn won(output: &TickOutput, id: PlayerId) -> bool {
    (output.durable.iter())
        .any(|(_, _, entry)| matches!(entry, Durable::WonGame { player, .. } if *player == id))
}

/// Ticks `region` with `first`, then idly, until player `id` is placed, within [`SOON`]
/// ticks. Whether they were.
fn placed_within(region: &mut Region, first: TickInputs, id: PlayerId) -> bool {
    if placed(&region.tick(&first), id) {
        return true;
    }
    (0..SOON).any(|_| placed(&idle(region), id))
}

/// Player `number` falls into the void of the overworld `region` at (`x`, `z`) and dies;
/// the body stays.
fn dead_in(region: &mut Region, number: u128, x: f64, z: f64) -> (PlayerId, EntityId) {
    let (id, entity) = add_player(region, number, [x, -200.0, z], false);
    assert!(
        (0..60).any(|_| died(&idle(region), id)),
        "{id:?} did not die"
    );
    (id, entity)
}

/// Ticks until the body of the dead `id` is gone and its stay waits to be asked.
fn until_the_body_goes(region: &mut Region, id: PlayerId) {
    for _ in 0..25 {
        idle(region);
    }
    assert!(region.player(id).is_none(), "the body is gone");
    let waiting = region
        .waiting_state(id)
        .expect("the stay waits to be asked");
    assert!(waiting.until_asked);
}

fn survival_after_creative() -> PlayerMode {
    PlayerMode {
        current: GameMode::Survival,
        previous: Some(GameMode::Creative),
    }
}

/// An order to a dead player holds (the review, item 6: exempt from `dead_input`): one
/// while the body lies changes the mode, which the respawn keeps (`restoreFrom`); one
/// after the body went is kept by the waiting stay and carried out at the placing.
#[test]
fn an_order_to_a_dead_player_holds_through_the_respawn() {
    let mut home = region(Land::whole(Dimension::Overworld), 0, nothing);
    let (id, entity) = dead_in(&mut home, 1, 3.5, 3.5);
    let output = home.tick(&input(
        id,
        entity,
        PlayerInput::Order(Order::GameMode(GameMode::Survival)),
    ));
    assert_eq!(
        mode_changes(&output, id),
        [(survival_after_creative(), false)]
    );
    assert_eq!(
        home.player_state(id).expect("the body lies").mode,
        survival_after_creative()
    );
    until_the_body_goes(&mut home, id);
    home.tick(&input(
        id,
        entity,
        PlayerInput::Order(give(items::STONE, 3)),
    ));
    assert!(placed_within(
        &mut home,
        input(id, entity, PlayerInput::PerformRespawn),
        id
    ));
    let state = home.player_state(id).expect("respawned");
    assert_eq!(state.mode, survival_after_creative());
    assert_eq!(state.inventory.0[0], stacked(items::STONE, 3));
}

/// As above, for a death away from the overworld's home: the ask lets the stay go to the
/// home, which places it. The order the stay was given while it waited holds there.
#[test]
#[ignore = "finding: a waiting stay that is let go to another region forgets the orders it kept"]
fn an_order_to_a_dead_player_away_from_the_home_holds_through_the_respawn() {
    let mut east = region(
        Land::east_of_the_line(Dimension::Overworld, OVERWORLD_HOME),
        50,
        nothing,
    );
    let mut home = region(
        Land::west_of_the_line(Dimension::Overworld, EAST),
        0,
        nothing,
    );
    let (id, entity) = dead_in(&mut east, 1, 40.5, 8.5);
    until_the_body_goes(&mut east, id);
    east.tick(&input(
        id,
        entity,
        PlayerInput::Order(Order::GameMode(GameMode::Survival)),
    ));
    let output = east.tick(&input(id, entity, PlayerInput::PerformRespawn));
    let (transfer, to) = let_go(&output, id).expect("let go at the ask");
    assert_eq!(to, OVERWORLD_HOME);
    assert!(placed_within(&mut home, arrive(id, transfer), id));
    let state = home.player_state(id).expect("respawned");
    assert_eq!(state.mode, survival_after_creative());
}

/// A player in the credits waits in the End to be asked; an order given meanwhile is
/// kept with the stay (`asked_input` lets it by) and holds once the overworld's home
/// places them.
#[test]
#[ignore = "finding: a waiting stay that is let go to another region forgets the orders it kept"]
fn an_order_to_a_stay_waiting_in_the_credits_holds_once_it_is_placed() {
    let portal = at(2, HIGH_FLOOR + 1, 2);
    let mut end = region(Land::whole(Dimension::End), 0, move |pos| {
        (pos == portal).then_some(blocks::END_PORTAL)
    });
    let mut home = region(Land::whole(Dimension::Overworld), 100, nothing);
    let (id, entity) = add_player(&mut end, 1, [2.5, f64::from(HIGH_FLOOR + 1), 2.5], false);
    assert!(won(&idle(&mut end), id), "in the credits");
    end.tick(&input(
        id,
        entity,
        PlayerInput::Order(Order::GameMode(GameMode::Survival)),
    ));
    end.tick(&input(
        id,
        entity,
        PlayerInput::Order(give(items::STONE, 3)),
    ));
    let waiting = end.waiting_state(id).expect("waits to be asked");
    assert!(waiting.until_asked);
    let output = end.tick(&input(id, entity, PlayerInput::PerformRespawn));
    let (transfer, to) = let_go(&output, id).expect("let go to the overworld's home");
    assert_eq!(to, OVERWORLD_HOME);
    assert!(placed_within(&mut home, arrive(id, transfer), id));
    let state = home.player_state(id).expect("placed");
    assert_eq!(state.mode, survival_after_creative());
    assert_eq!(state.inventory.0[0], stacked(items::STONE, 3));
}

/// While the stay waits in the credits, an order is neither carried out (nobody is in
/// the world to carry it out on) nor taken for the ask: it is kept with the stay.
#[test]
fn an_order_to_a_stay_in_the_credits_is_kept_until_the_ask() {
    let portal = at(2, HIGH_FLOOR + 1, 2);
    let mut end = region(Land::whole(Dimension::End), 0, move |pos| {
        (pos == portal).then_some(blocks::END_PORTAL)
    });
    let (id, entity) = add_player(&mut end, 1, [2.5, f64::from(HIGH_FLOOR + 1), 2.5], false);
    assert!(won(&idle(&mut end), id), "in the credits");
    let order = Order::GameMode(GameMode::Survival);
    let output = end.tick(&input(id, entity, PlayerInput::Order(order.clone())));
    assert!(
        mode_changes(&output, id).is_empty(),
        "nobody to apply it to yet"
    );
    let waiting = end.waiting_state(id).expect("waits to be asked");
    assert!(waiting.until_asked, "an order is no ask");
    assert_eq!(waiting.kept, [PlayerInput::Order(order)]);
}
