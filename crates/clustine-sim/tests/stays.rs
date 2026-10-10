//! Tests of what `docs/adr/0020-one-stay-per-player.md` has a region do: a join held as
//! entering until the world store has said where the player was last, the notes a tick
//! makes for the store, the store's two answers, `Ended`, the order of stays by entity
//! and hand-overs, the leave that names an attempt, and flying (its sections 3 to 6, 9,
//! 12 and 13, and the rows of section 10 that regions alone can show).
//!
//! They use the public API only, but they were written with the region and by whoever
//! built it: the tests of this from the record alone, by someone who has not read how
//! the region does it, are still to be written.
//!
//! Every tick goes through [`tick`], which holds the region to what holds of every
//! tick: the delta turns the state before into the state after, every note is true of
//! the state after or of a player the tick let go, and every `Ended` of a tick has a
//! lower number than every `Departed` of it in the same outbox.
//!
//! The scenarios of one region come first. [`World`] then puts two regions on stripes
//! under one store and two edges, each a second implementation of what the record says
//! of it: [`Store`] is the table of section 3, and [`Edges`] is sections 4.1, 4.3 and 6
//! as far as stays go. The rows of section 10 are played on it by hand, and runs made
//! up by a generator are held to what the record promises: no region ever has an
//! entering and a present stay of one player, no edge is left with a connection whose
//! stay is gone, and `Ended` is made only for a stay that is dead.
//!
//! The runs are five to a test. The environment variable `CLUSTINE_STAYS_SEEDS` says
//! how many instead, and `CLUSTINE_STAYS_SEED` names the one to run.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use clustine_data::items;
use clustine_sim::api::{HOTBAR_SLOTS, ItemStack, PlayerInput, Pose, RegionEvent, RemoteStep};
use clustine_sim::{
    Durable, EdgeEvent, Entered, EnteringState, Holdings, Knowledge, Misdirected, Place,
    PlayerChange, PlayerEvent, PlayerJoin, PlayerState, PlayerTransfer, Region, RegionConfig,
    RegionState, StayNote, TickInputs, TickOutput, Ticket,
};
use clustine_world::{
    BlockPos, ChunkArea, ChunkPos, EdgeId, EntityId, EntityIds, PlayerId, RegionId, Vec3,
};
use uuid::Uuid;

const E: EdgeId = EdgeId(1);
const F: EdgeId = EdgeId(2);
/// An edge no region of these tests is told of.
const STRANGER: EdgeId = EdgeId(99);
/// The start every edge has.
const START: u64 = 10;

/// The chunk players enter the world in, its neighbour to the east, and a chunk far
/// out of which the home region knows nothing.
const HOME: ChunkPos = ChunkPos::new(0, 0);
const EAST: ChunkPos = ChunkPos::new(1, 0);
const FAR: ChunkPos = ChunkPos::new(6, 0);

const SPAWN: Vec3 = Vec3::new(8.5, 64.0, 8.5);
/// Another point in the home chunk.
const NEARBY: Vec3 = Vec3::new(3.5, 64.0, 12.5);

/// The home region, another region, and the region a split makes.
const HOME_REGION: RegionId = RegionId(0);
const OTHER: RegionId = RegionId(1);
const PART: RegionId = RegionId(12);

/// The height of the world's lowest block.
const LOWEST_Y: i32 = -64;

fn player(n: u128) -> PlayerId {
    PlayerId(Uuid::from_u128(n))
}

fn name(n: u128) -> String {
    format!("player-{n}")
}

fn stack(item: i32, count: i32) -> Option<ItemStack> {
    Some(ItemStack { item, count })
}

/// What a player has who enters the world for the first time.
fn starting() -> [Option<ItemStack>; HOTBAR_SLOTS] {
    let mut hotbar = [None; HOTBAR_SLOTS];
    hotbar[0] = stack(items::STONE, 64);
    hotbar
}

/// What a player held who has been in the world before: nothing like [`starting`].
fn kept() -> [Option<ItemStack>; HOTBAR_SLOTS] {
    let mut hotbar = [None; HOTBAR_SLOTS];
    hotbar[3] = stack(items::DIRT, 7);
    hotbar[5] = stack(items::STONE, 2);
    hotbar
}

fn config(on: bool) -> RegionConfig {
    RegionConfig {
        spawn: SPAWN,
        starting_hotbar: starting(),
        // Nothing is given back in these tests.
        return_after: 1_000_000,
        place_by_store: on,
        lowest_y: LOWEST_Y,
    }
}

fn ids() -> EntityIds {
    EntityIds::block(3).expect("block 3 exists")
}

/// The entity of the `n`th stay the home region gives out, counted from 1.
fn entity(n: i32) -> EntityId {
    EntityId(ids().first.0 + n - 1)
}

/// An entity that no region of these tests gives out.
const TRAVELLER: EntityId = EntityId(900);

fn chunk_of(position: Vec3) -> ChunkPos {
    ChunkPos::containing(position.x, position.z)
}

/// The middle of a chunk, on the ground.
fn middle(chunk: ChunkPos) -> Vec3 {
    Vec3::new(
        f64::from(chunk.x) * 16.0 + 8.5,
        64.0,
        f64::from(chunk.z) * 16.0 + 8.5,
    )
}

/// A place the store kept: in the air above the middle of `chunk`, looking somewhere,
/// flying, with [`kept`] in the hotbar and another slot than the first in hand.
fn place_in(chunk: ChunkPos) -> Place {
    let ground = middle(chunk);
    Place {
        pose: Pose {
            position: Vec3::new(ground.x, 70.0, ground.z),
            yaw: 77.0,
            pitch: -12.0,
            on_ground: false,
        },
        flying: true,
        hotbar: kept(),
        selected_slot: 5,
    }
}

/// Where a player is and what they hold, as a region's state has it.
fn place_of(state: &PlayerState) -> Place {
    Place {
        pose: state.pose,
        flying: state.flying,
        hotbar: state.hotbar,
        selected_slot: state.selected_slot,
    }
}

/// Where a transfer has a player be and hold.
fn place_from(transfer: &PlayerTransfer) -> Place {
    Place {
        pose: transfer.pose,
        flying: transfer.flying,
        hotbar: transfer.hotbar,
        selected_slot: transfer.selected_slot,
    }
}

// What a tick is given.

fn changes(list: Vec<PlayerChange>) -> TickInputs {
    let mut inputs = TickInputs::default();
    for change in list {
        inputs.change(change);
    }
    inputs
}

fn edges(events: Vec<EdgeEvent>) -> TickInputs {
    TickInputs {
        edges: events,
        ..TickInputs::default()
    }
}

fn started(edge: EdgeId, start: u64) -> EdgeEvent {
    EdgeEvent::Started { edge, start }
}

fn join(edge: EdgeId, n: u128, attempt: u64) -> PlayerChange {
    let join = PlayerJoin {
        player: player(n),
        name: name(n),
        attempt,
    };
    PlayerChange::Join(edge, join)
}

/// A leave that names the entity of the stay.
fn leave_as(edge: EdgeId, n: u128, entity: EntityId) -> PlayerChange {
    PlayerChange::Leave(edge, player(n), Some(entity), None)
}

/// A leave that names no entity, and the attempt of the join or none.
fn leave_of(edge: EdgeId, n: u128, attempt: Option<u64>) -> PlayerChange {
    PlayerChange::Leave(edge, player(n), None, attempt)
}

fn arrive(edge: EdgeId, n: u128, transfer: PlayerTransfer) -> PlayerChange {
    PlayerChange::Arrive(edge, player(n), transfer)
}

/// The store's answer to the note of an entering stay.
fn entered(n: u128, entity: EntityId, place: Option<Place>, holder: Option<RegionId>) -> Entered {
    Entered {
        player: player(n),
        entity,
        place,
        holder,
    }
}

fn answers(entered: Vec<Entered>) -> TickInputs {
    TickInputs {
        entered,
        ..TickInputs::default()
    }
}

/// The store's word that stays are dead.
fn dead(list: &[(u128, EntityId, u32)]) -> TickInputs {
    TickInputs {
        dead: list
            .iter()
            .map(|(n, stay, hops)| (player(*n), *stay, *hops))
            .collect(),
        ..TickInputs::default()
    }
}

fn act(edge: EdgeId, n: u128, entity: EntityId, number: u64, input: PlayerInput) -> TickInputs {
    let mut inputs = TickInputs::default();
    inputs.input(edge, player(n), entity, number, input);
    inputs
}

fn walk_to(position: Vec3) -> PlayerInput {
    PlayerInput::Move {
        position: Some(position),
        rotation: None,
        on_ground: true,
    }
}

/// A player as another region lets them go, standing at `position` with [`kept`].
fn transfer(entity: EntityId, hops: u32, attempt: Option<u64>, position: Vec3) -> PlayerTransfer {
    PlayerTransfer {
        entity_id: entity,
        name: "traveller".to_owned(),
        pose: Pose::at(position),
        hotbar: kept(),
        selected_slot: 2,
        last_input: 0,
        hops,
        flying: false,
        attempt,
    }
}

// What a tick gives back.

fn ended(n: u128, entity: EntityId, attempt: Option<u64>) -> Durable {
    Durable::Ended {
        player: player(n),
        entity,
        attempt,
    }
}

/// The outbox entries of a tick with their edges, without their numbers.
fn entries(output: &TickOutput) -> Vec<(EdgeId, Durable)> {
    let entry = |(edge, _, entry): &(EdgeId, u64, Durable)| (*edge, entry.clone());
    output.durable.iter().map(entry).collect()
}

fn removed(output: &TickOutput) -> Vec<EntityId> {
    let removed = |event: &RegionEvent| match event {
        RegionEvent::EntityRemoved { entity, .. } => Some(*entity),
        _ => None,
    };
    output.events.iter().filter_map(removed).collect()
}

fn shown(output: &TickOutput) -> Vec<EntityId> {
    let spawned = |event: &RegionEvent| match event {
        RegionEvent::EntitySpawned(state) => Some(state.entity),
        _ => None,
    };
    output.events.iter().filter_map(spawned).collect()
}

/// Whether the tick showed nothing, told nobody anything and said nothing to the store.
fn silent(output: &TickOutput) -> bool {
    output.events.is_empty()
        && output.player_events.is_empty()
        && output.durable.is_empty()
        && output.stays.is_empty()
}

fn has(n: u128, entity: EntityId, hops: u32, place: Place) -> StayNote {
    StayNote::Has {
        player: player(n),
        entity,
        hops,
        place,
    }
}

fn entering_note(n: u128, entity: EntityId) -> StayNote {
    StayNote::Entering {
        player: player(n),
        entity,
    }
}

fn state_of(region: &Region, n: u128) -> PlayerState {
    region
        .player_state(player(n))
        .unwrap_or_else(|| panic!("player {n} is in the region"))
}

// ---------------------------------------------------------------------------------------
// What holds of every tick
// ---------------------------------------------------------------------------------------

/// What `docs/adr/0020-one-stay-per-player.md` says of every tick, of which `output` is
/// one that left the region with the state `after`.
fn check_tick(output: &TickOutput, after: &RegionState) {
    let tick = output.tick;
    // The notes are in the order of the players, an entering stay before one that is
    // or was here, and each is true: of the state, or of a player the tick let go.
    let key = |note: &StayNote| match note {
        StayNote::Entering { player, .. } => (*player, 0),
        StayNote::Has { player, .. } => (*player, 1),
    };
    let keys: Vec<_> = output.stays.iter().map(key).collect();
    assert!(
        keys.windows(2).all(|pair| pair[0] < pair[1]),
        "tick {tick}: {:?}",
        output.stays
    );
    for note in &output.stays {
        match note {
            StayNote::Entering { player, entity } => {
                let held = after.entering.get(player).map(|held| held.entity_id);
                assert_eq!(held, Some(*entity), "tick {tick}: {note:?}");
            }
            StayNote::Has {
                player,
                entity,
                hops,
                place,
            } => {
                let here = after
                    .players
                    .get(player)
                    .filter(|state| state.entity_id == *entity)
                    .map(|state| (state.hops, place_of(state)));
                let let_go = output.durable.iter().find_map(|(_, _, entry)| match entry {
                    Durable::Departed {
                        player: gone,
                        transfer,
                        ..
                    } if gone == player && transfer.entity_id == *entity => {
                        Some((transfer.hops, place_from(transfer)))
                    }
                    _ => None,
                });
                let said = Some((*hops, place.clone()));
                assert_eq!(here.or(let_go), said, "tick {tick}: {note:?}");
            }
        }
    }
    // Every entry is in its outbox under its number, and in every outbox the tick's
    // `Ended` entries are numbered below its `Departed` entries.
    let mut last_ended: BTreeMap<EdgeId, u64> = BTreeMap::new();
    let mut first_departed: BTreeMap<EdgeId, u64> = BTreeMap::new();
    for (edge, number, entry) in &output.durable {
        let known = after
            .edges
            .get(edge)
            .expect("the edge of an entry is known");
        assert_eq!(known.outbox.get(number), Some(entry), "tick {tick}");
        match entry {
            Durable::Ended { .. } => {
                last_ended.insert(*edge, *number);
            }
            Durable::Departed { .. } => {
                first_departed.entry(*edge).or_insert(*number);
            }
            _ => {}
        }
    }
    for (edge, ended) in last_ended {
        let departed = first_departed.get(&edge);
        assert!(
            departed.is_none_or(|departed| ended < *departed),
            "tick {tick}"
        );
    }
}

/// Ticks `region` and holds it to [`check_tick`] and to its own delta.
fn tick(region: &mut Region, inputs: &TickInputs) -> TickOutput {
    let before = region.state();
    let output = region.tick(inputs);
    let after = region.state();
    let mut applied = before;
    applied.apply(&output.delta);
    assert_eq!(applied, after, "the delta of tick {}", output.tick);
    check_tick(&output, &after);
    output
}

fn idle(region: &mut Region) -> TickOutput {
    tick(region, &TickInputs::default())
}

// ---------------------------------------------------------------------------------------
// One region
// ---------------------------------------------------------------------------------------

/// The attempt of the joins of [`entering`] and [`with_player`].
const ATTEMPT: u64 = 41;

/// The home region on open land: it holds the home chunk, knows the edges `E` and `F`
/// and has ticked once, so that no later tick is its first.
fn home(on: bool) -> Region {
    let holdings = Holdings {
        held: vec![HOME],
        pinned: Vec::new(),
    };
    let mut region = Region::new(config(on), ids(), holdings);
    let output = tick(
        &mut region,
        &edges(vec![started(E, START), started(F, START)]),
    );
    assert!(output.stays.is_empty());
    region
}

/// [`home`] for which the store keeps the players' places, with a join of player 1
/// through `E` taken: it holds their stay `entity(1)` as entering.
fn entering() -> Region {
    let mut region = home(true);
    tick(&mut region, &changes(vec![join(E, 1, ATTEMPT)]));
    assert!(region.entering_state(player(1)).is_some());
    region
}

/// [`home`] with player 1 in it at the spawn point, through `E`, as `entity(1)`. No
/// input of theirs has been applied, so the stay carries [`ATTEMPT`].
fn with_player(on: bool) -> Region {
    let mut region = home(on);
    let output = tick(&mut region, &changes(vec![join(E, 1, ATTEMPT)]));
    if on {
        assert!(output.player_events.is_empty());
        tick(
            &mut region,
            &answers(vec![entered(1, entity(1), None, None)]),
        );
    }
    let state = state_of(&region, 1);
    assert_eq!(
        (state.entity_id, state.attempt, state.hops),
        (entity(1), Some(ATTEMPT), 0)
    );
    region
}

/// Has the region learn that `chunk` is `holder`'s: a viewer's ticket, for which it
/// asks, and the store's answer.
fn learn(region: &mut Region, chunk: ChunkPos, holder: RegionId) {
    let ticket = TickInputs {
        tickets_added: vec![(chunk, Ticket::Viewer)],
        ..TickInputs::default()
    };
    assert_eq!(tick(region, &ticket).claims, [chunk]);
    let answer = TickInputs {
        foreign: vec![(chunk, holder)],
        ..TickInputs::default()
    };
    tick(region, &answer);
    assert_eq!(region.knowledge(chunk), Knowledge::Foreign(holder));
}

/// What the player is told when the stay of [`entering`] is placed as `place` has it.
fn spawned(place: &Place) -> (PlayerId, PlayerEvent) {
    let event = PlayerEvent::Spawned {
        attempt: ATTEMPT,
        entity_id: entity(1),
        pose: place.pose,
        flying: place.flying,
        hotbar: place.hotbar,
        selected_slot: place.selected_slot,
    };
    (player(1), event)
}

/// Where a player is who enters the world for the first time.
fn at_spawn() -> Place {
    Place {
        pose: Pose::at(SPAWN),
        flying: false,
        hotbar: starting(),
        selected_slot: 0,
    }
}

// Section 3 and section 4, steps 3 and 7.

#[test]
fn a_join_is_held_as_entering_and_named_to_the_store_and_nobody_is_told() {
    let mut region = home(true);
    let output = tick(&mut region, &changes(vec![join(E, 1, ATTEMPT)]));
    assert!(output.player_events.is_empty() && output.events.is_empty());
    assert!(output.durable.is_empty() && output.claims.is_empty());
    assert_eq!(output.stays, [entering_note(1, entity(1))]);

    let held = EnteringState {
        entity_id: entity(1),
        name: name(1),
        edge: E,
        attempt: ATTEMPT,
    };
    assert_eq!(region.entering_state(player(1)), Some(&held));
    assert_eq!(output.delta.entering, [(player(1), Some(held))]);
    assert_eq!(output.delta.next_entity_id, Some(entity(2)));
    assert!(output.delta.players.is_empty());
    // The player is nowhere: no entity, and the region counts nobody.
    assert_eq!(region.player(player(1)), None);
    assert_eq!((region.player_count(), region.crowds().len()), (0, 0));
    assert_eq!(region.entities().count(), 0);

    // The stay is named once. While the region waits for the answer, a tick has
    // nothing to say and changes nothing.
    let output = idle(&mut region);
    assert!(silent(&output));
    assert!(output.delta.changes_only_the_tick());
}

#[test]
fn an_entering_stay_without_a_place_is_placed_at_the_spawn_point_with_the_starting_hotbar() {
    // Whoever is said to hold what: there is no place to hold.
    for holder in [None, Some(OTHER)] {
        let mut region = entering();
        let answer = entered(1, entity(1), None, holder);
        let output = tick(&mut region, &answers(vec![answer]));
        assert_eq!(output.player_events, [spawned(&at_spawn())]);
        assert_eq!(shown(&output), [entity(1)]);
        assert!(output.durable.is_empty());
        assert_eq!(output.stays, [has(1, entity(1), 0, at_spawn())]);

        let state = state_of(&region, 1);
        assert_eq!(place_of(&state), at_spawn());
        assert_eq!((state.edge, state.last_input, state.hops), (E, 0, 0));
        // Until the first input of the stay is applied.
        assert_eq!(state.attempt, Some(ATTEMPT));
        assert_eq!(region.entering_state(player(1)), None);
        assert_eq!(output.delta.entering, [(player(1), None)]);
    }
}

#[test]
fn an_entering_stay_whose_place_nobody_holds_is_placed_there_as_the_player_left() {
    let place = place_in(FAR);
    let mut region = entering();
    let answer = entered(1, entity(1), Some(place.clone()), None);
    let output = tick(&mut region, &answers(vec![answer]));
    // With the look, flying, and what they held.
    assert_eq!(output.player_events, [spawned(&place)]);
    assert_eq!(shown(&output), [entity(1)]);
    assert!(output.durable.is_empty());
    assert_eq!(output.stays, [has(1, entity(1), 0, place.clone())]);
    let state = state_of(&region, 1);
    assert_eq!(place_of(&state), place);
    assert_eq!((state.attempt, state.hops), (Some(ATTEMPT), 0));
    // The region knows nothing of the chunk and claims it, because they stand in it.
    assert_eq!(output.claims, [FAR]);

    // Granted, they stay. Said to be another's, they are let go in the tick that hears
    // it, as anyone is: with one hand-over, and with the attempt of the join still, by
    // which an edge that has not read `Spawned` finds its connection (row 22 of
    // section 10).
    let mut granted = region.clone();
    let grant = TickInputs {
        granted: vec![FAR],
        ..TickInputs::default()
    };
    assert!(silent(&tick(&mut granted, &grant)));
    assert_eq!(granted.player_count(), 1);

    let anothers = TickInputs {
        foreign: vec![(FAR, OTHER)],
        ..TickInputs::default()
    };
    let output = tick(&mut region, &anothers);
    let let_go = PlayerTransfer {
        entity_id: entity(1),
        name: name(1),
        pose: place.pose,
        hotbar: place.hotbar,
        selected_slot: place.selected_slot,
        last_input: 0,
        hops: 1,
        flying: true,
        attempt: Some(ATTEMPT),
    };
    let departed = Durable::Departed {
        player: player(1),
        transfer: let_go,
        to: OTHER,
    };
    assert_eq!(output.durable, [(E, 1, departed)]);
    assert_eq!(output.stays, [has(1, entity(1), 1, place)]);
    assert!(output.events.is_empty(), "the entity lives on");
    assert_eq!(region.player_count(), 0);
}

#[test]
fn an_entering_stay_whose_place_another_region_holds_is_let_go_there_without_being_placed() {
    let place = place_in(FAR);
    let mut region = entering();
    let answer = entered(1, entity(1), Some(place.clone()), Some(OTHER));
    let output = tick(&mut region, &answers(vec![answer]));
    // Nobody is told that they entered, no entity is shown, nothing is claimed.
    assert!(output.player_events.is_empty() && output.events.is_empty());
    assert!(output.claims.is_empty());
    let let_go = PlayerTransfer {
        entity_id: entity(1),
        name: name(1),
        pose: place.pose,
        hotbar: place.hotbar,
        selected_slot: place.selected_slot,
        last_input: 0,
        hops: 1,
        flying: place.flying,
        attempt: Some(ATTEMPT),
    };
    let departed = Durable::Departed {
        player: player(1),
        transfer: let_go,
        to: OTHER,
    };
    assert_eq!(output.durable, [(E, 1, departed)]);
    assert_eq!(output.stays, [has(1, entity(1), 1, place)]);
    // The region has nothing of the stay any more.
    assert_eq!(region.entering_state(player(1)), None);
    assert_eq!(region.player(player(1)), None);
    assert_eq!(output.delta.entering, [(player(1), None)]);
    assert!(output.delta.players.is_empty());
    assert!(silent(&idle(&mut region)));
}

#[test]
fn a_place_below_the_world_counts_as_no_place_whoever_holds_it() {
    let fallen = |y: f64| {
        let mut place = place_in(FAR);
        place.pose.position.y = y;
        place
    };
    // With what they held, at the spawn point, looking ahead and not flying.
    let back = Place {
        pose: Pose::at(SPAWN),
        flying: false,
        hotbar: kept(),
        selected_slot: 5,
    };
    for holder in [None, Some(OTHER)] {
        for y in [-64.01, -70.0, -1.0e7] {
            let mut region = entering();
            let answer = entered(1, entity(1), Some(fallen(y)), holder);
            let output = tick(&mut region, &answers(vec![answer]));
            assert_eq!(output.player_events, [spawned(&back)], "{y}");
            assert!(output.durable.is_empty(), "{y}: nobody is let go");
            assert_eq!(output.stays, [has(1, entity(1), 0, back.clone())]);
            assert!(
                output.claims.is_empty(),
                "{y}: they stand in the home chunk"
            );
        }
    }
    // The lowest block itself is in the world: feet at its height are a place.
    let lowest = fallen(f64::from(LOWEST_Y));
    let mut region = entering();
    let answer = entered(1, entity(1), Some(lowest.clone()), None);
    let output = tick(&mut region, &answers(vec![answer]));
    assert_eq!(output.player_events, [spawned(&lowest)]);
    let mut region = entering();
    let answer = entered(1, entity(1), Some(lowest), Some(OTHER));
    let output = tick(&mut region, &answers(vec![answer]));
    assert!(matches!(
        entries(&output)[..],
        [(E, Durable::Departed { to: OTHER, .. })]
    ));
}

#[test]
fn an_answer_for_a_stay_the_region_does_not_hold_as_entering_is_passed_over() {
    // Another stay of the player than the one that is entering, and a player of whom
    // nothing is entering.
    let mut region = entering();
    let stale = vec![
        entered(1, entity(2), None, None),
        entered(
            1,
            EntityId(entity(1).0 - 1),
            Some(place_in(FAR)),
            Some(OTHER),
        ),
        entered(2, entity(1), None, None),
    ];
    let output = tick(&mut region, &answers(stale));
    assert!(silent(&output) && output.delta.changes_only_the_tick());
    assert!(region.entering_state(player(1)).is_some());

    // An answer that comes twice: the second finds the stay placed, and changes
    // nothing of it.
    let twice = vec![
        entered(1, entity(1), None, None),
        entered(1, entity(1), Some(place_in(FAR)), Some(OTHER)),
    ];
    let output = tick(&mut region, &answers(twice));
    assert_eq!(output.player_events.len(), 1);
    assert!(output.durable.is_empty());
    let before = region.state();
    let again = answers(vec![entered(1, entity(1), Some(place_in(FAR)), None)]);
    assert!(silent(&tick(&mut region, &again)));
    assert_eq!(region.state().players, before.players);
}

#[test]
fn an_arrival_is_named_with_the_hops_it_came_with() {
    let mut region = home(true);
    let arriving = transfer(TRAVELLER, 4, None, NEARBY);
    let place = place_from(&arriving);
    let output = tick(&mut region, &changes(vec![arrive(F, 5, arriving)]));
    assert_eq!(output.stays, [has(5, TRAVELLER, 4, place)]);
    assert!(output.durable.is_empty());

    // One that goes on to the region the chunk is believed to be of is not named:
    // nothing of it is here, and it is passed on as it came.
    let mut region = home(true);
    learn(&mut region, EAST, OTHER);
    let arriving = transfer(TRAVELLER, 4, Some(7), middle(EAST));
    let output = tick(&mut region, &changes(vec![arrive(F, 5, arriving.clone())]));
    assert!(output.stays.is_empty());
    let what = Misdirected::Arrival {
        player: player(5),
        transfer: arriving,
    };
    let holder = OTHER;
    assert_eq!(entries(&output), [(F, Durable::NotMine { what, holder })]);
}

#[test]
fn whatever_changes_of_a_player_is_named_with_their_place_and_nothing_else_is() {
    let mut region = with_player(true);
    tick(
        &mut region,
        &changes(vec![arrive(F, 5, transfer(TRAVELLER, 2, None, NEARBY))]),
    );
    // A tick in which nothing happens to anybody names nobody.
    assert!(silent(&idle(&mut region)));

    // What player 1 does names player 1, and not the other.
    let output = tick(&mut region, &act(E, 1, entity(1), 1, walk_to(NEARBY)));
    let state = state_of(&region, 1);
    assert_eq!(state.pose.position, NEARBY);
    assert_eq!(output.stays, [has(1, entity(1), 0, place_of(&state))]);
    for (number, input) in [
        (2, PlayerInput::SelectSlot { slot: 4 }),
        (3, PlayerInput::SetFlying { flying: true }),
        (
            4,
            PlayerInput::SetHotbarSlot {
                slot: 1,
                stack: stack(items::DIRT, 3),
            },
        ),
    ] {
        let output = tick(&mut region, &act(E, 1, entity(1), number, input.clone()));
        let state = state_of(&region, 1);
        assert_eq!(
            output.stays,
            [has(1, entity(1), 0, place_of(&state))],
            "{input:?}"
        );
    }
    let state = state_of(&region, 1);
    assert_eq!((state.selected_slot, state.flying), (4, true));
    assert_eq!(state.hotbar[1], stack(items::DIRT, 3));

    // An input that is not applied changes nothing and names nobody: one of another
    // stay, and one through another edge.
    let mut inputs = act(E, 1, entity(2), 9, walk_to(SPAWN));
    inputs.input(F, player(1), entity(1), 9, walk_to(SPAWN));
    assert!(silent(&tick(&mut region, &inputs)));
}

#[test]
fn a_player_who_is_let_go_is_named_with_the_place_and_the_hops_the_next_region_begins_with() {
    let mut region = with_player(true);
    learn(&mut region, EAST, OTHER);
    let step = middle(EAST);
    let output = tick(&mut region, &act(E, 1, entity(1), 1, walk_to(step)));
    let [(E, Durable::Departed { transfer, to, .. })] = &entries(&output)[..] else {
        panic!("{:?}", output.durable);
    };
    assert_eq!((*to, transfer.hops, transfer.attempt), (OTHER, 1, None));
    assert_eq!(transfer.pose.position, step);
    assert_eq!(output.stays, [has(1, entity(1), 1, place_from(transfer))]);
    assert_eq!(region.player_count(), 0);

    // The region that takes them in names them with those hops, and counts one more
    // when it lets them go in its turn.
    let mut next = home(true);
    let arriving = PlayerTransfer {
        pose: Pose::at(NEARBY),
        ..transfer.clone()
    };
    tick(&mut next, &changes(vec![arrive(E, 1, arriving)]));
    learn(&mut next, EAST, OTHER);
    let output = tick(&mut next, &act(E, 1, entity(1), 2, walk_to(step)));
    let [(E, Durable::Departed { transfer, .. })] = &entries(&output)[..] else {
        panic!("{:?}", output.durable);
    };
    assert_eq!(transfer.hops, 2);
    assert_eq!(output.stays, [has(1, entity(1), 2, place_from(transfer))]);
}

#[test]
fn the_first_tick_after_a_restore_names_every_stay_and_no_later_tick_does() {
    // A region with two players and a stay that is entering.
    let mut region = with_player(true);
    tick(
        &mut region,
        &changes(vec![arrive(F, 5, transfer(TRAVELLER, 2, None, NEARBY))]),
    );
    tick(&mut region, &changes(vec![join(F, 3, 77)]));
    let state = region.state();
    let every = vec![
        has(1, entity(1), 0, place_of(&state.players[&player(1)])),
        entering_note(3, entity(2)),
        has(5, TRAVELLER, 2, place_of(&state.players[&player(5)])),
    ];
    // The region that is run on has nothing to say.
    assert!(silent(&idle(&mut region.clone())));

    let holdings = Holdings {
        held: vec![HOME],
        pinned: Vec::new(),
    };
    let mut restored = Region::restore(config(true), state.clone(), holdings.clone());
    let output = tick(&mut restored, &TickInputs::default());
    assert_eq!(output.stays, every);
    // Naming changes nothing of the state, and happens once.
    assert!(output.delta.changes_only_the_tick());
    assert!(silent(&idle(&mut restored)));

    // The store answers the naming of the entering stay as it answered the join, and
    // the stay enters (rows 18 and 19 of section 10).
    let again = answers(vec![entered(3, entity(2), None, None)]);
    let output = tick(&mut restored, &again);
    assert!(matches!(
        output.player_events[..],
        [(_, PlayerEvent::Spawned { attempt: 77, .. })]
    ));

    // Where the store does not keep the players' places, nothing is named.
    let mut off = Region::restore(config(false), state, holdings);
    assert!(tick(&mut off, &TickInputs::default()).stays.is_empty());
}

#[test]
fn the_first_tick_after_a_merge_and_after_a_split_names_every_stay() {
    // The home region with a player far out, placed there on joining, and a stay that
    // is entering. A region with one player is absorbed.
    let holdings = Holdings {
        held: vec![HOME, FAR],
        pinned: Vec::new(),
    };
    let mut region = Region::new(config(true), ids(), holdings);
    tick(
        &mut region,
        &edges(vec![started(E, START), started(F, START)]),
    );
    tick(&mut region, &changes(vec![join(E, 1, ATTEMPT)]));
    let far = entered(1, entity(1), Some(place_in(FAR)), None);
    tick(&mut region, &answers(vec![far]));
    tick(&mut region, &changes(vec![join(F, 3, 77)]));

    let mut other = RegionState::new(EntityIds::block(4).expect("block 4 exists"));
    other.tick = 9;
    other.edges = region.state().edges;
    let theirs = PlayerState {
        entity_id: TRAVELLER,
        name: "traveller".to_owned(),
        pose: Pose::at(middle(EAST)),
        hotbar: kept(),
        selected_slot: 2,
        last_input: 6,
        handled: None,
        edge: F,
        hops: 3,
        flying: false,
        attempt: None,
    };
    other.players.insert(player(5), theirs.clone());
    let merged = region.absorb(OTHER, &other);
    region.take_absorbed(merged, &[EAST], &[]);
    let output = tick(&mut region, &TickInputs::default());
    let far_place = place_in(FAR);
    assert_eq!(
        output.stays,
        [
            has(1, entity(1), 0, far_place.clone()),
            entering_note(3, entity(2)),
            has(5, TRAVELLER, 3, place_of(&theirs)),
        ]
    );
    assert!(silent(&idle(&mut region)));

    // The player far out is split off. Both regions name what they have, and the one
    // who went has one hand-over more; the entering stay stays (row 21 of section 10:
    // the part has the stay with its attempt, and the edge is told so).
    let splitting = region.split(&[FAR], PART, &[]).expect("player 1 goes");
    assert_eq!(splitting.part.players[&player(1)].hops, 1);
    assert_eq!(splitting.part.players[&player(1)].attempt, Some(ATTEMPT));
    assert!(splitting.part.entering.is_empty());
    assert_eq!(splitting.state.entering.len(), 1);
    let told = splitting.state.edges[&E].outbox.values().next_back();
    let split_off = Durable::SplitOff {
        region: PART,
        players: vec![(player(1), entity(1), Some(ATTEMPT))],
    };
    assert_eq!(told, Some(&split_off));
    let (_, mut part) = region.take_split(splitting, &[]);
    let output = tick(&mut part.region, &TickInputs::default());
    assert_eq!(output.stays, [has(1, entity(1), 1, far_place)]);
    assert!(silent(&idle(&mut part.region)));
    let output = tick(&mut region, &TickInputs::default());
    assert_eq!(
        output.stays,
        [
            entering_note(3, entity(2)),
            has(5, TRAVELLER, 3, place_of(&theirs)),
        ]
    );
    assert!(silent(&idle(&mut region)));
}

#[test]
fn a_player_who_is_removed_is_not_named() {
    // By a leave, by a join that is refused, by the store's word and by an edge that
    // is gone.
    let mut region = with_player(true);
    let output = tick(&mut region, &changes(vec![leave_as(E, 1, entity(1))]));
    assert!(output.stays.is_empty());
    assert_eq!(removed(&output), [entity(1)]);

    let mut region = with_player(true);
    let output = tick(&mut region, &dead(&[(1, entity(2), 0)]));
    assert!(output.stays.is_empty());
    assert_eq!(removed(&output), [entity(1)]);

    let mut region = with_player(true);
    let output = tick(&mut region, &edges(vec![EdgeEvent::Gone { edge: E }]));
    assert!(output.stays.is_empty());
    assert_eq!(removed(&output), [entity(1)]);

    // A join over them names the stay that is entering, and not the one that went.
    let mut region = with_player(true);
    let output = tick(&mut region, &changes(vec![join(F, 1, 77)]));
    assert_eq!(output.stays, [entering_note(1, entity(2))]);
    assert_eq!(removed(&output), [entity(1)]);
}

#[test]
fn where_the_store_does_not_keep_the_places_a_join_places_at_once_and_nothing_is_named() {
    let mut region = home(false);
    let output = tick(&mut region, &changes(vec![join(E, 1, ATTEMPT)]));
    assert_eq!(output.player_events, [spawned(&at_spawn())]);
    assert_eq!(shown(&output), [entity(1)]);
    assert!(output.stays.is_empty());
    assert_eq!(region.entering_state(player(1)), None);
    assert!(output.delta.entering.is_empty());

    // Neither of the store's answers is looked at.
    let mut inputs = dead(&[(1, entity(9), 0)]);
    inputs.entered = vec![entered(1, entity(1), Some(place_in(FAR)), Some(OTHER))];
    let output = tick(&mut region, &inputs);
    assert!(silent(&output) && output.delta.changes_only_the_tick());
    assert_eq!(place_of(&state_of(&region, 1)), at_spawn());

    // A join over a player and an arrival over them tell no edge anything, and neither
    // does an arrival that is passed over; nothing is named by any of it.
    let output = tick(&mut region, &changes(vec![join(F, 1, 77)]));
    assert_eq!(removed(&output), [entity(1)]);
    assert!(output.durable.is_empty() && output.stays.is_empty());
    let later = arrive(E, 1, transfer(EntityId(entity(2).0 + 5), 1, None, NEARBY));
    let output = tick(&mut region, &changes(vec![later]));
    assert_eq!(removed(&output), [entity(2)]);
    assert!(output.durable.is_empty() && output.stays.is_empty());
    let earlier = arrive(F, 1, transfer(entity(1), 3, None, NEARBY));
    let output = tick(&mut region, &changes(vec![earlier]));
    assert_eq!(removed(&output), [entity(1)]);
    assert!(output.durable.is_empty() && output.stays.is_empty());
    learn(&mut region, EAST, OTHER);
    let entity = state_of(&region, 1).entity_id;
    let output = tick(&mut region, &act(E, 1, entity, 1, walk_to(middle(EAST))));
    assert!(matches!(
        entries(&output)[..],
        [(E, Durable::Departed { .. })]
    ));
    assert!(output.stays.is_empty());
}

// Section 4.4.

#[test]
fn a_leave_without_an_entity_ends_the_entering_stay_of_its_attempt_and_no_other() {
    let mut region = entering();
    // Another attempt, none, another edge, and a leave that names the entity: none of
    // them is of this stay.
    let others = vec![
        leave_of(E, 1, Some(ATTEMPT + 1)),
        leave_of(E, 1, None),
        leave_of(F, 1, Some(ATTEMPT)),
        leave_as(E, 1, entity(1)),
        leave_of(E, 2, Some(ATTEMPT)),
    ];
    let output = tick(&mut region, &changes(others));
    assert!(silent(&output) && output.delta.changes_only_the_tick());

    let output = tick(&mut region, &changes(vec![leave_of(E, 1, Some(ATTEMPT))]));
    // The edge asked for it, so it is told nothing, and there was no entity to remove.
    assert!(silent(&output));
    assert_eq!(region.entering_state(player(1)), None);
    assert_eq!(output.delta.entering, [(player(1), None)]);

    // What the store answers afterwards finds no such stay (row 16 of section 10).
    let late = answers(vec![entered(
        1,
        entity(1),
        Some(place_in(FAR)),
        Some(OTHER),
    )]);
    let output = tick(&mut region, &late);
    assert!(silent(&output) && output.delta.changes_only_the_tick());
}

#[test]
fn a_leave_without_an_entity_ends_a_present_stay_only_while_it_carries_that_attempt() {
    for on in [false, true] {
        let mut region = with_player(on);
        let others = vec![
            leave_of(E, 1, Some(ATTEMPT + 1)),
            leave_of(E, 1, None),
            leave_of(F, 1, Some(ATTEMPT)),
        ];
        let output = tick(&mut region, &changes(others));
        assert!(
            silent(&output) && output.delta.changes_only_the_tick(),
            "{on}"
        );
        let output = tick(&mut region, &changes(vec![leave_of(E, 1, Some(ATTEMPT))]));
        assert_eq!(removed(&output), [entity(1)], "{on}");
        assert!(output.durable.is_empty() && output.stays.is_empty());

        // Once an input of the stay is applied its edge knows the entity, the stay
        // carries no attempt, and only a leave that names the entity ends it.
        let mut region = with_player(on);
        tick(&mut region, &act(E, 1, entity(1), 1, walk_to(NEARBY)));
        assert_eq!(state_of(&region, 1).attempt, None);
        let unnamed = vec![leave_of(E, 1, Some(ATTEMPT)), leave_of(E, 1, None)];
        let output = tick(&mut region, &changes(unnamed));
        assert!(silent(&output), "{on}");
        let output = tick(&mut region, &changes(vec![leave_as(E, 1, entity(1))]));
        assert_eq!(removed(&output), [entity(1)], "{on}");
    }
}

#[test]
fn a_leave_of_an_earlier_connection_that_comes_behind_a_later_join_ends_nothing() {
    // The order of events of section 4.4: what an edge kept for a region that was
    // absorbed is put behind what it kept for the survivor, so the leave of a
    // connection that ended while entering comes behind the join of the next. The
    // stay of that join is entering, or placed already.
    let earlier = 40;
    let mut region = home(true);
    let inputs = changes(vec![join(E, 1, ATTEMPT), leave_of(E, 1, Some(earlier))]);
    tick(&mut region, &inputs);
    assert!(region.entering_state(player(1)).is_some());
    let output = tick(&mut region, &changes(vec![leave_of(E, 1, Some(earlier))]));
    assert!(silent(&output));
    assert!(region.entering_state(player(1)).is_some());

    for on in [false, true] {
        let mut region = with_player(on);
        let output = tick(&mut region, &changes(vec![leave_of(E, 1, Some(earlier))]));
        assert!(silent(&output), "{on}");
        assert_eq!(region.player_count(), 1);
    }
}

// Section 5.

#[test]
fn the_stores_word_removes_a_dead_player_and_tells_their_edge() {
    // A later stay of the player is known to the store.
    let mut region = with_player(true);
    let output = tick(&mut region, &dead(&[(1, entity(2), 0)]));
    assert_eq!(removed(&output), [entity(1)]);
    // With the attempt, as long as the stay had it.
    let told = ended(1, entity(1), Some(ATTEMPT));
    assert_eq!(output.durable, [(E, 1, told)]);
    assert_eq!(region.player_count(), 0);
    assert_eq!(output.delta.players, [(player(1), None)]);

    // The same stay with more hand-overs behind it: this copy is the earlier one.
    let mut region = with_player(true);
    tick(&mut region, &act(E, 1, entity(1), 1, walk_to(NEARBY)));
    let output = tick(&mut region, &dead(&[(1, entity(1), 1)]));
    assert_eq!(removed(&output), [entity(1)]);
    assert_eq!(entries(&output), [(E, ended(1, entity(1), None))]);

    // Not below: this very stay, an earlier one, and a word about somebody else.
    let mut region = with_player(true);
    let words = [
        (1, entity(1), 0),
        (1, EntityId(entity(1).0 - 1), 5),
        (2, entity(9), 0),
    ];
    let output = tick(&mut region, &dead(&words));
    assert!(silent(&output) && output.delta.changes_only_the_tick());
    assert_eq!(region.player_count(), 1);
}

#[test]
fn the_stores_word_removes_a_dead_entering_stay_and_tells_its_edge() {
    let mut region = entering();
    // The entering stay itself, with whatever hand-overs, is not below itself.
    let output = tick(&mut region, &dead(&[(1, entity(1), 0), (1, entity(1), 7)]));
    assert!(silent(&output));
    assert!(region.entering_state(player(1)).is_some());

    let output = tick(&mut region, &dead(&[(1, entity(2), 0)]));
    // No entity was ever shown.
    assert!(output.events.is_empty());
    assert_eq!(output.durable, [(E, 1, ended(1, entity(1), Some(ATTEMPT)))]);
    assert_eq!(region.entering_state(player(1)), None);
    assert_eq!(output.delta.entering, [(player(1), None)]);
}

#[test]
fn a_region_keeps_nothing_of_the_stores_word_and_takes_a_dead_stay_in_when_it_arrives() {
    // Departure 3 of the record: a region remembers no floor. The word finds nobody;
    // the stay it was about arrives later, is taken in and named, and goes in the tick
    // that reads the store's answer to that note.
    let mut region = home(true);
    let output = tick(&mut region, &dead(&[(1, entity(4), 0)]));
    assert!(silent(&output) && output.delta.changes_only_the_tick());
    let late = transfer(entity(1), 1, None, NEARBY);
    let output = tick(&mut region, &changes(vec![arrive(E, 1, late.clone())]));
    assert_eq!(shown(&output), [entity(1)]);
    assert_eq!(output.stays, [has(1, entity(1), 1, place_from(&late))]);
    let output = tick(&mut region, &dead(&[(1, entity(4), 0)]));
    assert_eq!(removed(&output), [entity(1)]);
    assert_eq!(entries(&output), [(E, ended(1, entity(1), None))]);
}

#[test]
fn the_stores_word_is_applied_after_the_edges_and_before_what_became_of_players() {
    // An edge that is gone in the tick has nobody left to be told, and nothing is made
    // for it.
    let mut region = with_player(true);
    let mut inputs = dead(&[(1, entity(2), 0)]);
    inputs.edges = vec![EdgeEvent::Gone { edge: E }];
    let output = tick(&mut region, &inputs);
    assert_eq!(removed(&output), [entity(1)]);
    assert!(output.durable.is_empty());

    // A stay that arrives in the tick of the word comes after it: the stay that was
    // here goes by the word, and the arrival is judged against nobody.
    let mut region = with_player(true);
    let mut inputs = dead(&[(1, entity(3), 0)]);
    inputs.change(arrive(F, 1, transfer(entity(2), 1, None, NEARBY)));
    let output = tick(&mut region, &inputs);
    assert_eq!(removed(&output), [entity(1)]);
    assert_eq!(shown(&output), [entity(2)]);
    assert_eq!(entries(&output), [(E, ended(1, entity(1), Some(ATTEMPT)))]);
    assert_eq!(state_of(&region, 1).entity_id, entity(2));
}

// Section 6.

#[test]
fn a_join_over_a_present_stay_tells_the_edge_that_had_it() {
    // Through another edge and through the same one (rows 3 and 15 of section 10).
    for edge in [F, E] {
        let mut region = with_player(true);
        tick(&mut region, &act(E, 1, entity(1), 1, walk_to(NEARBY)));
        let output = tick(&mut region, &changes(vec![join(edge, 1, 77)]));
        assert_eq!(removed(&output), [entity(1)]);
        assert_eq!(entries(&output), [(E, ended(1, entity(1), None))]);
        assert_eq!(output.stays, [entering_note(1, entity(2))]);
        assert_eq!(region.player(player(1)), None);
        assert_eq!(
            region.entering_state(player(1)).map(|held| held.edge),
            Some(edge)
        );
    }
    // A join that is refused ends the stay all the same, and the edge is told first.
    let two = EntityIds {
        first: entity(1),
        end: entity(2),
    };
    let holdings = Holdings {
        held: vec![HOME],
        pinned: Vec::new(),
    };
    let mut region = Region::new(config(true), two, holdings);
    tick(
        &mut region,
        &edges(vec![started(E, START), started(F, START)]),
    );
    tick(&mut region, &changes(vec![join(E, 1, ATTEMPT)]));
    tick(
        &mut region,
        &answers(vec![entered(1, entity(1), None, None)]),
    );
    let output = tick(&mut region, &changes(vec![join(F, 1, 77)]));
    let refused = Durable::Refused {
        player: player(1),
        attempt: 77,
    };
    assert_eq!(
        output.durable,
        [(E, 1, ended(1, entity(1), Some(ATTEMPT))), (F, 1, refused)]
    );
    assert!(output.stays.is_empty());
}

#[test]
fn a_join_over_an_entering_stay_tells_its_edge_and_the_first_answer_is_passed_over() {
    // Two logins a few ticks apart, the first not yet answered (row 14 of section 10).
    let mut region = entering();
    let output = tick(&mut region, &changes(vec![join(F, 1, 77)]));
    assert!(output.events.is_empty(), "no entity was shown");
    assert_eq!(entries(&output), [(E, ended(1, entity(1), Some(ATTEMPT)))]);
    assert_eq!(output.stays, [entering_note(1, entity(2))]);
    let first = answers(vec![entered(1, entity(1), None, None)]);
    assert!(silent(&tick(&mut region, &first)));
    let second = answers(vec![entered(1, entity(2), None, None)]);
    let output = tick(&mut region, &second);
    assert!(matches!(
        output.player_events[..],
        [(_, PlayerEvent::Spawned { attempt: 77, entity_id, .. })] if entity_id == entity(2)
    ));
    assert_eq!(state_of(&region, 1).edge, F);

    // Two logins within one tick (row 13): the joins are taken in order, the second
    // removes the entering stay of the first and tells its edge, and one note is
    // made, of the later.
    let mut region = home(true);
    let both = changes(vec![join(E, 1, ATTEMPT), join(F, 1, 77)]);
    let output = tick(&mut region, &both);
    assert_eq!(entries(&output), [(E, ended(1, entity(1), Some(ATTEMPT)))]);
    assert_eq!(output.stays, [entering_note(1, entity(2))]);
    assert_eq!(region.state().entering.len(), 1);
}

#[test]
fn an_arrival_over_an_earlier_stay_tells_the_edge_of_the_stay_that_was_there() {
    let mut region = with_player(true);
    let later = transfer(entity(2), 1, Some(77), NEARBY);
    let output = tick(&mut region, &changes(vec![arrive(F, 1, later.clone())]));
    assert_eq!(removed(&output), [entity(1)]);
    assert_eq!(shown(&output), [entity(2)]);
    assert_eq!(entries(&output), [(E, ended(1, entity(1), Some(ATTEMPT)))]);
    assert_eq!(output.stays, [has(1, entity(2), 1, place_from(&later))]);
    let state = state_of(&region, 1);
    assert_eq!((state.entity_id, state.edge), (entity(2), F));
}

#[test]
fn an_arrival_of_an_earlier_stay_than_the_one_that_is_there_is_passed_over_and_its_edge_told() {
    let mut region = with_player(true);
    tick(&mut region, &changes(vec![join(F, 1, 77)]));
    tick(
        &mut region,
        &answers(vec![entered(1, entity(2), None, None)]),
    );
    let before = region.state().players;
    // Whatever hand-overs it has behind it: the entity decides first.
    let earlier = transfer(entity(1), 9, Some(ATTEMPT), NEARBY);
    let output = tick(&mut region, &changes(vec![arrive(E, 1, earlier)]));
    assert_eq!(removed(&output), [entity(1)]);
    assert!(shown(&output).is_empty());
    assert_eq!(entries(&output), [(E, ended(1, entity(1), Some(ATTEMPT)))]);
    assert!(output.stays.is_empty());
    assert_eq!(region.state().players, before);

    // Through an edge the region does not know there is nobody to tell.
    let earlier = transfer(entity(1), 9, None, NEARBY);
    let output = tick(&mut region, &changes(vec![arrive(STRANGER, 1, earlier)]));
    assert_eq!(removed(&output), [entity(1)]);
    assert!(output.durable.is_empty());
}

#[test]
fn an_arrival_against_a_later_entering_stay_is_passed_over_and_its_edge_told() {
    // Row 8a of section 10: the earlier stay's arrival lands in the home region while
    // the later one is entering there.
    let mut region = home(true);
    tick(&mut region, &changes(vec![join(E, 9, 1)]));
    tick(&mut region, &changes(vec![join(F, 1, 77)]));
    assert_eq!(
        region.entering_state(player(1)).map(|held| held.entity_id),
        Some(entity(2))
    );
    let earlier = transfer(entity(1), 2, None, NEARBY);
    let output = tick(&mut region, &changes(vec![arrive(E, 1, earlier.clone())]));
    assert_eq!(
        removed(&output),
        [entity(1)],
        "the arrival's entity is reported removed"
    );
    assert!(shown(&output).is_empty());
    assert_eq!(entries(&output), [(E, ended(1, entity(1), None))]);
    assert!(output.stays.is_empty());
    assert_eq!(region.player(player(1)), None);
    assert!(region.entering_state(player(1)).is_some());

    // Through an edge the region does not know: removed, and nobody told.
    let output = tick(
        &mut region,
        &changes(vec![arrive(STRANGER, 1, earlier.clone())]),
    );
    assert_eq!(removed(&output), [entity(1)]);
    assert!(output.durable.is_empty());

    // Where the store does not keep the places nothing is entering, and the arrival
    // is taken in as ever.
    let mut region = home(false);
    let output = tick(&mut region, &changes(vec![arrive(E, 1, earlier)]));
    assert_eq!(shown(&output), [entity(1)]);
}

#[test]
fn an_arrival_of_the_stay_that_is_there_with_no_more_hops_changes_nothing() {
    for on in [false, true] {
        let mut region = home(on);
        let here = transfer(TRAVELLER, 2, None, SPAWN);
        tick(&mut region, &changes(vec![arrive(E, 5, here)]));
        let before = region.state().players;
        // As often handed on, and less often: this very stay, or an earlier copy.
        for hops in [2, 1, 0] {
            let copy = transfer(TRAVELLER, hops, None, NEARBY);
            let output = tick(&mut region, &changes(vec![arrive(F, 5, copy)]));
            assert!(silent(&output), "{on}, {hops}: {output:?}");
            assert!(output.delta.changes_only_the_tick());
            assert_eq!(region.state().players, before);
        }
    }
}

#[test]
fn an_arrival_of_the_stay_that_is_there_with_more_hops_takes_its_place_without_a_word() {
    for on in [false, true] {
        let mut region = home(on);
        let here = transfer(TRAVELLER, 2, None, SPAWN);
        tick(&mut region, &changes(vec![arrive(E, 5, here)]));
        let later = transfer(TRAVELLER, 3, None, NEARBY);
        let output = tick(&mut region, &changes(vec![arrive(F, 5, later.clone())]));
        // The entity lives on in the arrival: nothing is reported removed, and no edge
        // is told that a stay has ended, which would put the real player out.
        assert!(removed(&output).is_empty(), "{on}");
        assert_eq!(shown(&output), [TRAVELLER]);
        assert!(output.durable.is_empty(), "{on}");
        let state = state_of(&region, 5);
        assert_eq!((state.hops, state.edge), (3, F));
        assert_eq!(state.pose.position, NEARBY);
        let named = if on {
            vec![has(5, TRAVELLER, 3, place_from(&later))]
        } else {
            Vec::new()
        };
        assert_eq!(output.stays, named);
    }
}

#[test]
fn an_entering_stay_is_placed_over_a_present_one_which_goes_first() {
    // The second guard of step 7: an earlier stay that is in the home region when the
    // answer comes. It is there by a merge here; the arrival that could bring it is
    // passed over since.
    let region = entering();
    let mut state = region.state();
    let earlier = PlayerState {
        entity_id: EntityId(entity(1).0 - 1),
        name: name(1),
        pose: Pose::at(NEARBY),
        hotbar: kept(),
        selected_slot: 1,
        last_input: 4,
        handled: None,
        edge: F,
        hops: 2,
        flying: false,
        attempt: None,
    };
    state.players.insert(player(1), earlier.clone());
    let holdings = Holdings {
        held: vec![HOME],
        pinned: Vec::new(),
    };
    let both = Region::restore(config(true), state, holdings);

    // Placed.
    let mut region = both.clone();
    let output = tick(
        &mut region,
        &answers(vec![entered(1, entity(1), None, None)]),
    );
    assert_eq!(removed(&output), [earlier.entity_id]);
    assert_eq!(shown(&output), [entity(1)]);
    assert_eq!(entries(&output), [(F, ended(1, earlier.entity_id, None))]);
    assert_eq!(state_of(&region, 1).entity_id, entity(1));
    // The first tick of this region names every stay: of the player, the one that is
    // here after it.
    assert_eq!(output.stays, [has(1, entity(1), 0, at_spawn())]);

    // Let go without being placed.
    let mut region = both.clone();
    let away = entered(1, entity(1), Some(place_in(FAR)), Some(OTHER));
    let output = tick(&mut region, &answers(vec![away]));
    assert_eq!(removed(&output), [earlier.entity_id]);
    assert!(shown(&output).is_empty());
    assert!(matches!(
        &entries(&output)[..],
        [
            (F, Durable::Ended { .. }),
            (E, Durable::Departed { to: OTHER, .. })
        ]
    ));
    assert_eq!(region.player_count(), 0);
    assert_eq!(output.stays, [has(1, entity(1), 1, place_in(FAR))]);

    // A stay that is here and not below the entering one cannot be while only the
    // home region gives out ids. If it is, the entering stay goes instead.
    let mut state = both.state();
    let later = PlayerState {
        entity_id: entity(5),
        ..earlier
    };
    state.players.insert(player(1), later);
    let holdings = Holdings {
        held: vec![HOME],
        pinned: Vec::new(),
    };
    let mut region = Region::restore(config(true), state, holdings);
    let output = tick(
        &mut region,
        &answers(vec![entered(1, entity(1), None, None)]),
    );
    assert!(output.events.is_empty());
    assert_eq!(entries(&output), [(E, ended(1, entity(1), Some(ATTEMPT)))]);
    assert_eq!(state_of(&region, 1).entity_id, entity(5));
    assert_eq!(region.entering_state(player(1)), None);
}

#[test]
fn a_merge_keeps_of_two_copies_of_a_stay_the_one_with_more_hops() {
    let copy = |hops: u32, edge: EdgeId, position: Vec3| PlayerState {
        entity_id: entity(1),
        name: name(1),
        pose: Pose::at(position),
        hotbar: kept(),
        selected_slot: 0,
        last_input: 3,
        handled: None,
        edge,
        hops,
        flying: false,
        attempt: None,
    };
    for (ours, theirs) in [(1, 3), (3, 1), (2, 2)] {
        let survivor = home(true);
        let mut state = survivor.state();
        state.players.insert(player(1), copy(ours, E, SPAWN));
        let holdings = Holdings {
            held: vec![HOME],
            pinned: Vec::new(),
        };
        let survivor = Region::restore(config(true), state.clone(), holdings);
        let mut other = RegionState::new(EntityIds::block(4).expect("block 4 exists"));
        other.edges = state.edges.clone();
        other.players.insert(player(1), copy(theirs, F, NEARBY));
        let merged = survivor.absorb(OTHER, &other);
        // With as many, the survivor's own stays.
        let expected = if theirs > ours {
            copy(theirs, F, NEARBY)
        } else {
            copy(ours, E, SPAWN)
        };
        assert_eq!(merged.players[&player(1)], expected, "{ours} and {theirs}");
    }
}

#[test]
fn a_merge_keeps_the_survivors_entering_stays_but_those_of_an_edge_it_is_reset_for() {
    let mut survivor = home(true);
    tick(
        &mut survivor,
        &changes(vec![join(E, 1, ATTEMPT), join(F, 2, 77)]),
    );
    let ours = survivor.state();
    // The other region knows `F` with a higher start, and holds a stay as entering,
    // which cannot be: only the home region does, and it is never absorbed.
    let mut other = RegionState::new(EntityIds::block(4).expect("block 4 exists"));
    other.edges = ours.edges.clone();
    other.edges.get_mut(&F).expect("it knows F").start = START + 1;
    let stray = EnteringState {
        entity_id: TRAVELLER,
        name: name(3),
        edge: E,
        attempt: 5,
    };
    other.entering.insert(player(3), stray);
    let merged = survivor.absorb(OTHER, &other);
    let kept: Vec<PlayerId> = merged.entering.keys().copied().collect();
    assert_eq!(kept, [player(1)]);
    assert_eq!(merged.entering[&player(1)], ours.entering[&player(1)]);
}

/// The home region with player 2 in it as `entity(1)`, who stands where a block of a
/// chunk the region does not hold is within reach, and player 1's stay `entity(2)`
/// entering, both through `E`.
fn about_to_pass_an_action_on() -> (Region, BlockPos) {
    let mut region = home(true);
    tick(&mut region, &changes(vec![join(E, 2, 55)]));
    tick(
        &mut region,
        &answers(vec![entered(2, entity(1), None, None)]),
    );
    let by_the_border = Vec3::new(8.5, 64.0, 1.5);
    tick(
        &mut region,
        &act(E, 2, entity(1), 1, walk_to(by_the_border)),
    );
    tick(&mut region, &changes(vec![join(E, 1, ATTEMPT)]));
    assert_eq!(
        region.entering_state(player(1)).map(|held| held.entity_id),
        Some(entity(2))
    );
    (region, BlockPos::new(8, 64, -1))
}

/// Section 6 of the record names this test. A stay that is let go without being placed
/// is decided where the store's answers are applied, in the middle of the tick, and an
/// input of the same edge makes an entry later in it. Numbered where it is decided,
/// the departure would be published behind an entry with a higher number, and its edge
/// would pass it over as a number it has had.
#[test]
fn a_stay_let_go_unplaced_in_a_tick_that_also_passes_an_action_on_is_numbered_after_it() {
    let (mut region, block) = about_to_pass_an_action_on();
    let sent = region.edge(E).expect("the region knows E").sent;
    let mut inputs = answers(vec![entered(
        1,
        entity(2),
        Some(place_in(FAR)),
        Some(OTHER),
    )]);
    let dig = PlayerInput::Dig {
        position: block,
        sequence: 3,
    };
    inputs.input(E, player(2), entity(1), 2, dig);
    let output = tick(&mut region, &inputs);
    let [
        (E, first, Durable::Remote { action, to: None }),
        (
            E,
            second,
            Durable::Departed {
                player: gone,
                transfer,
                to: OTHER,
            },
        ),
    ] = &output.durable[..]
    else {
        panic!("{:?}", output.durable);
    };
    assert_eq!((*first, *second), (sent + 1, sent + 2));
    assert_eq!(action.step, RemoteStep::Break { position: block });
    assert_eq!(
        (*gone, transfer.entity_id, transfer.hops),
        (player(1), entity(2), 1)
    );
}

#[test]
fn every_ended_of_a_tick_is_numbered_below_every_departed_of_it() {
    // One tick, one edge: the store's word ends a stay, a join ends a stay, an arrival
    // ends a stay and is itself let go at the end of the tick, an answer ends a stay
    // and lets the entering one go unplaced, and an action is passed on.
    let (mut region, block) = about_to_pass_an_action_on();
    learn(&mut region, EAST, OTHER);
    // Players 3 and 4, each with a stay in the region through `E`, and player 1's
    // earlier stay, which came back while the later one is entering: it is there by a
    // restore, as an arrival of it would be passed over.
    let stays = vec![
        arrive(E, 3, transfer(EntityId(701), 1, None, SPAWN)),
        arrive(E, 4, transfer(EntityId(702), 1, None, SPAWN)),
        arrive(E, 6, transfer(EntityId(703), 1, None, SPAWN)),
    ];
    tick(&mut region, &changes(stays));
    let mut state = region.state();
    let earlier = PlayerState {
        entity_id: EntityId(entity(1).0 - 1),
        name: name(1),
        pose: Pose::at(SPAWN),
        hotbar: kept(),
        selected_slot: 0,
        last_input: 0,
        handled: None,
        edge: E,
        hops: 0,
        flying: false,
        attempt: None,
    };
    state.players.insert(player(1), earlier);
    let holdings = Holdings {
        held: vec![HOME],
        pinned: Vec::new(),
    };
    let mut region = Region::restore(config(true), state, holdings);
    learn(&mut region, EAST, OTHER);

    let mut inputs = dead(&[(3, EntityId(800), 0)]);
    inputs.change(join(E, 4, 91));
    // Later than the stay of player 6 that is there, and into the neighbour's chunk
    // as far as the region believes: it is taken in nowhere and goes on.
    inputs.change(arrive(E, 6, transfer(EntityId(704), 2, None, middle(EAST))));
    inputs.entered = vec![entered(1, entity(2), Some(place_in(FAR)), Some(OTHER))];
    let dig = PlayerInput::Dig {
        position: block,
        sequence: 3,
    };
    inputs.input(E, player(2), entity(1), 2, dig);
    // And player 2 walks out after the dig.
    inputs.input(E, player(2), entity(1), 3, walk_to(middle(EAST)));
    let output = tick(&mut region, &inputs);

    let kinds: Vec<&str> = output
        .durable
        .iter()
        .map(|(edge, _, entry)| {
            assert_eq!(*edge, E);
            match entry {
                Durable::Ended { .. } => "ended",
                Durable::NotMine { .. } => "not mine",
                Durable::Remote { .. } => "remote",
                Durable::Departed { .. } => "departed",
                other => panic!("{other:?}"),
            }
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "ended", "ended", "ended", "not mine", "ended", "remote", "departed", "departed"
        ]
    );
    let numbers: Vec<u64> = output
        .durable
        .iter()
        .map(|(_, number, _)| *number)
        .collect();
    assert!(
        numbers.windows(2).all(|pair| pair[0] + 1 == pair[1]),
        "{numbers:?}"
    );
    // The stay let go unplaced first, then the players, in their order.
    let gone: Vec<PlayerId> = output
        .durable
        .iter()
        .filter_map(|(_, _, entry)| match entry {
            Durable::Departed { player, .. } => Some(*player),
            _ => None,
        })
        .collect();
    assert_eq!(gone, [player(1), player(2)]);
}

#[test]
fn an_edge_that_is_gone_or_reset_takes_its_entering_stays_with_it_and_is_told_nothing() {
    for event in [EdgeEvent::Gone { edge: E }, started(E, START + 1)] {
        let mut region = entering();
        tick(&mut region, &changes(vec![join(F, 2, 77)]));
        let output = tick(&mut region, &edges(vec![event.clone()]));
        assert!(
            output.events.is_empty() && output.durable.is_empty(),
            "{event:?}"
        );
        assert!(output.stays.is_empty());
        assert_eq!(region.entering_state(player(1)), None, "{event:?}");
        assert!(
            region.entering_state(player(2)).is_some(),
            "another edge's stays"
        );
        assert_eq!(output.delta.entering, [(player(1), None)]);
        // The store's answer finds no such stay.
        let late = answers(vec![entered(1, entity(1), None, None)]);
        assert!(silent(&tick(&mut region, &late)));
    }
}

#[test]
fn a_leave_tells_no_edge_that_a_stay_has_ended() {
    let mut region = with_player(true);
    let output = tick(&mut region, &changes(vec![leave_as(E, 1, entity(1))]));
    assert!(output.durable.is_empty());
    let mut region = entering();
    let output = tick(&mut region, &changes(vec![leave_of(E, 1, Some(ATTEMPT))]));
    assert!(output.durable.is_empty());
}

// Section 12.

#[test]
fn an_arrival_drops_only_the_waiting_inputs_of_its_own_stay_and_a_join_all_of_the_players() {
    let waiting = |inputs: &TickInputs| -> Vec<(EntityId, u64)> {
        let key = |(_, _, entity, number, _): &(EdgeId, PlayerId, EntityId, u64, PlayerInput)| {
            (*entity, *number)
        };
        inputs.inputs.iter().map(key).collect()
    };
    let mut inputs = TickInputs::default();
    inputs.input(F, player(1), entity(2), 1, walk_to(NEARBY));
    inputs.input(E, player(1), entity(1), 7, walk_to(SPAWN));
    inputs.input(F, player(2), entity(1), 3, walk_to(SPAWN));
    inputs.input(F, player(1), entity(2), 2, walk_to(SPAWN));
    // The earlier stay arrives where the later one is: what the later one did waits on.
    inputs.change(arrive(E, 1, transfer(entity(1), 1, None, NEARBY)));
    assert_eq!(
        waiting(&inputs),
        [(entity(2), 1), (entity(1), 3), (entity(2), 2)]
    );
    // What comes behind the arrival is kept, of whichever stay.
    inputs.input(E, player(1), entity(1), 8, walk_to(SPAWN));
    assert_eq!(waiting(&inputs).len(), 4);
    // A join drops all of the player's, and of nobody else.
    inputs.change(join(F, 1, 77));
    assert_eq!(waiting(&inputs), [(entity(1), 3)]);
    assert_eq!(inputs.inputs[0].1, player(2));

    // In a tick: the later stay is in the region, the earlier one's arrival is passed
    // over, and what the later one did in that tick is applied (row 9 of section 10).
    for on in [false, true] {
        let mut region = home(on);
        tick(
            &mut region,
            &changes(vec![arrive(F, 1, transfer(entity(2), 1, None, SPAWN))]),
        );
        let mut inputs = TickInputs::default();
        inputs.input(F, player(1), entity(2), 1, walk_to(NEARBY));
        inputs.change(arrive(E, 1, transfer(entity(1), 1, None, NEARBY)));
        tick(&mut region, &inputs);
        let state = state_of(&region, 1);
        assert_eq!((state.entity_id, state.last_input), (entity(2), 1), "{on}");
        assert_eq!(state.pose.position, NEARBY);
    }
}

// Section 13.

#[test]
fn flying_is_set_by_the_input_kept_handed_on_and_taken_in() {
    for on in [false, true] {
        let mut region = with_player(on);
        let fly = PlayerInput::SetFlying { flying: true };
        let output = tick(&mut region, &act(E, 1, entity(1), 1, fly));
        assert!(state_of(&region, 1).flying, "{on}");
        // It counts as a change of the player, and nobody else is shown it.
        assert_eq!(output.delta.players.len(), 1);
        assert!(output.events.is_empty());
        // Through a restore.
        let holdings = Holdings {
            held: vec![HOME],
            pinned: Vec::new(),
        };
        let restored = Region::restore(config(on), region.state(), holdings);
        assert!(state_of(&restored, 1).flying);

        learn(&mut region, EAST, OTHER);
        let output = tick(&mut region, &act(E, 1, entity(1), 2, walk_to(middle(EAST))));
        let [(E, Durable::Departed { transfer, .. })] = &entries(&output)[..] else {
            panic!("{:?}", output.durable);
        };
        assert!(transfer.flying, "{on}");
        let mut next = home(on);
        let arriving = PlayerTransfer {
            pose: Pose::at(NEARBY),
            ..transfer.clone()
        };
        tick(&mut next, &changes(vec![arrive(E, 1, arriving)]));
        assert!(state_of(&next, 1).flying);
        let land = PlayerInput::SetFlying { flying: false };
        tick(&mut next, &act(E, 1, entity(1), 3, land));
        assert!(!state_of(&next, 1).flying);
    }
}

// Section 10, the rows that one region shows.

#[test]
fn what_a_dead_stay_does_before_the_stores_word_is_read_is_applied() {
    // Row 10: the edge of the earlier stay is alive and its client does things in the
    // ticks before the stay is removed. Row 11: what it sends afterwards finds no such
    // stay.
    let mut region = with_player(true);
    let output = tick(&mut region, &act(E, 1, entity(1), 1, walk_to(NEARBY)));
    assert_eq!(output.stays.len(), 1, "named like any change");
    tick(&mut region, &dead(&[(1, entity(2), 0)]));
    let late = act(E, 1, entity(1), 2, walk_to(SPAWN));
    let output = tick(&mut region, &late);
    assert!(silent(&output) && output.delta.changes_only_the_tick());
}

// ---------------------------------------------------------------------------------------
// The store, the edges, and a world of two regions
// ---------------------------------------------------------------------------------------

/// What the world store keeps of a player (section 2 of the record).
#[derive(Debug, Clone, PartialEq)]
struct Record {
    stay: EntityId,
    hops: u32,
    place: Option<Place>,
}

/// One of the store's two answers.
#[derive(Debug, Clone, PartialEq)]
enum Answer {
    Enter(Entered),
    Dead(PlayerId, EntityId, u32),
}

/// The world store as far as stays go: the table of section 3 of the record.
#[derive(Debug, Default)]
struct Store {
    records: BTreeMap<PlayerId, Record>,
    /// How many notes were answered `Dead`.
    refused: usize,
}

impl Store {
    /// Takes the notes of one commit of a region, which is the home region or not.
    /// `holder` says who holds a chunk as the store's table has it, `None` for the
    /// home region itself and for nobody. Returns the answers for that region and
    /// what every region that runs is told.
    fn take(
        &mut self,
        home: bool,
        notes: &[StayNote],
        holder: impl Fn(ChunkPos) -> Option<RegionId>,
    ) -> (Vec<Answer>, Vec<Answer>) {
        let (mut answers, mut all) = (Vec::new(), Vec::new());
        for note in notes {
            match note {
                StayNote::Entering { player, entity } => {
                    assert!(home, "{note:?} from a region that is not joined");
                    let record = self.records.entry(*player).or_insert(Record {
                        stay: EntityId(0),
                        hops: 0,
                        place: None,
                    });
                    if *entity < record.stay {
                        self.refused += 1;
                        answers.push(Answer::Dead(*player, record.stay, record.hops));
                        continue;
                    }
                    if *entity > record.stay {
                        // The floor is raised; the place stays.
                        record.stay = *entity;
                        record.hops = 0;
                        all.push(Answer::Dead(*player, *entity, 0));
                    }
                    let place = record.place.clone();
                    let holder = place
                        .as_ref()
                        .and_then(|place| holder(chunk_of(place.pose.position)));
                    answers.push(Answer::Enter(Entered {
                        player: *player,
                        entity: *entity,
                        place,
                        holder,
                    }));
                }
                StayNote::Has {
                    player,
                    entity,
                    hops,
                    place,
                } => {
                    let record = self.records.get_mut(player);
                    let record =
                        record.unwrap_or_else(|| panic!("{note:?} of a stay never issued"));
                    assert!(*entity <= record.stay, "{note:?} of a stay never issued");
                    if *entity < record.stay {
                        self.refused += 1;
                        answers.push(Answer::Dead(*player, record.stay, record.hops));
                        continue;
                    }
                    // The record calls this a copy of the living stay, which should
                    // not exist; no run here makes one.
                    assert!(*hops >= record.hops, "{note:?} against {record:?}");
                    record.hops = *hops;
                    record.place = Some(place.clone());
                }
            }
        }
        (answers, all)
    }
}

/// A connection of a player as an edge has it.
#[derive(Debug, Clone, PartialEq)]
struct View {
    /// Which of the edge's connections it is.
    attempt: u64,
    /// The entity, once the edge has been told it.
    entity: Option<EntityId>,
    /// The region the edge takes the player to be in.
    region: RegionId,
    /// How many inputs the player has made in this connection.
    made: u64,
}

/// Something a region says to an edge about a stay.
#[derive(Debug, Clone, PartialEq)]
enum Word {
    Spawned {
        player: PlayerId,
        attempt: u64,
        entity: EntityId,
    },
    Entry(u64, Box<Durable>),
}

/// Something an edge sends a region.
#[derive(Debug, Clone, PartialEq)]
enum Message {
    Change(PlayerChange),
    Input(PlayerId, EntityId, u64, PlayerInput),
    Confirm(u64),
}

/// The edges as far as stays go: sections 4.1, 4.3 (paths 3 and `Spawned`), 4.4 and 6
/// of the record. An edge has at most one connection of a player.
#[derive(Debug, Default)]
struct Edges {
    views: BTreeMap<(EdgeId, PlayerId), View>,
    /// The number of the last connection of each edge.
    sessions: BTreeMap<EdgeId, u64>,
    /// The connections an edge ended on a region's `Ended`, each with the entity it
    /// had been told, if any.
    put_out: Vec<(EdgeId, PlayerId, Option<EntityId>)>,
    /// How often a stay was found by the attempt its transfer carries.
    by_attempt: usize,
    /// How many stays were discarded for want of a connection.
    discarded: usize,
}

impl Edges {
    /// Ends the edge's connection of the player, if it has one: the leave names the
    /// entity, or the attempt where the edge was told none.
    fn remove(&mut self, edge: EdgeId, id: PlayerId) -> Option<(RegionId, Message)> {
        let view = self.views.remove(&(edge, id))?;
        let attempt = view.entity.is_none().then_some(view.attempt);
        let leave = PlayerChange::Leave(edge, id, view.entity, attempt);
        Some((view.region, Message::Change(leave)))
    }

    /// A connection of the player has finished configuration.
    fn connect(&mut self, edge: EdgeId, id: PlayerId, n: u128) -> Vec<(RegionId, Message)> {
        // Step 1 of a login: a connection the edge has of the player is ended first.
        let mut messages: Vec<_> = self.remove(edge, id).into_iter().collect();
        let session = self.sessions.entry(edge).or_insert(0);
        *session += 1;
        let view = View {
            attempt: *session,
            entity: None,
            region: HOME_REGION,
            made: 0,
        };
        let join = PlayerJoin {
            player: id,
            name: name(n),
            attempt: view.attempt,
        };
        self.views.insert((edge, id), view);
        messages.push((HOME_REGION, Message::Change(PlayerChange::Join(edge, join))));
        messages
    }

    /// Something the player does, which the edge passes on only for a connection that
    /// has been told its entity.
    fn act(
        &mut self,
        edge: EdgeId,
        id: PlayerId,
        input: PlayerInput,
    ) -> Option<(RegionId, Message)> {
        let view = self.views.get_mut(&(edge, id))?;
        let entity = view.entity?;
        view.made += 1;
        Some((view.region, Message::Input(id, entity, view.made, input)))
    }

    /// What `edge` makes of a word from `from`. `dead` says whether the store has a
    /// later stay of a player than the entity named.
    fn read(
        &mut self,
        edge: EdgeId,
        from: RegionId,
        word: &Word,
        dead: impl Fn(PlayerId, EntityId) -> bool,
    ) -> Vec<(RegionId, Message)> {
        let (number, entry) = match word {
            Word::Spawned {
                player,
                attempt,
                entity,
            } => {
                // Only for a connection without an entity whose join it answers.
                if let Some(view) = self.views.get_mut(&(edge, *player))
                    && view.entity.is_none()
                    && view.attempt == *attempt
                {
                    view.entity = Some(*entity);
                }
                return Vec::new();
            }
            Word::Entry(number, entry) => (*number, entry.as_ref()),
        };
        let mut messages = vec![(from, Message::Confirm(number))];
        match entry {
            Durable::Departed {
                player,
                transfer,
                to,
            } => messages.push(self.pass_on(edge, from, *player, transfer, *to)),
            Durable::NotMine {
                what: Misdirected::Arrival { player, transfer },
                holder,
            } => messages.push(self.pass_on(edge, from, *player, transfer, *holder)),
            Durable::Ended {
                player,
                entity,
                attempt,
            } => {
                assert!(dead(*player, *entity), "{entry:?} for a stay that lives");
                let meant = self.views.get(&(edge, *player)).is_some_and(|view| {
                    view.entity == Some(*entity)
                        || (view.entity.is_none() && Some(view.attempt) == *attempt)
                });
                if meant {
                    let told = self.views[&(edge, *player)].entity;
                    self.put_out.push((edge, *player, told));
                    messages.extend(self.remove(edge, *player));
                }
            }
            Durable::Refused { player, attempt } => {
                let meant = (self.views.get(&(edge, *player)))
                    .is_some_and(|view| view.entity.is_none() && view.attempt == *attempt);
                assert!(!meant, "{entry:?}: the block of ids is not used up here");
            }
            other => panic!("{from} said {other:?}"),
        }
        messages
    }

    /// Passes a stay that `from` let go, or sent on, to the region `to`: with the
    /// connection that has its entity, or with the one without an entity, under
    /// `from`, whose attempt the transfer carries (path 3 of section 4.3). A stay of
    /// no connection is discarded.
    fn pass_on(
        &mut self,
        edge: EdgeId,
        from: RegionId,
        id: PlayerId,
        transfer: &PlayerTransfer,
        to: RegionId,
    ) -> (RegionId, Message) {
        let view = self.views.get_mut(&(edge, id));
        let theirs = view.filter(|view| {
            view.entity == Some(transfer.entity_id)
                || (view.entity.is_none()
                    && view.region == from
                    && Some(view.attempt) == transfer.attempt)
        });
        match theirs {
            Some(view) => {
                if view.entity.is_none() {
                    view.entity = Some(transfer.entity_id);
                    self.by_attempt += 1;
                }
                view.region = to;
                let arrival = PlayerChange::Arrive(edge, id, transfer.clone());
                (to, Message::Change(arrival))
            }
            None => {
                self.discarded += 1;
                let discard = PlayerChange::Discard {
                    entity: transfer.entity_id,
                    chunk: chunk_of(transfer.pose.position),
                };
                (to, Message::Change(discard))
            }
        }
    }
}

/// A source of made-up numbers: the same for the same seed.
#[derive(Debug, Clone)]
struct Random(u64);

impl Random {
    fn below(&mut self, bound: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % bound
    }

    fn once_in(&mut self, times: u64) -> bool {
        self.below(times) == 0
    }

    fn pick<T: Copy>(&mut self, from: &[T]) -> T {
        from[self.below(from.len() as u64) as usize]
    }
}

/// The region east of the home region in a [`World`].
const EAST_REGION: RegionId = OTHER;

/// The stripes of a [`World`]: the home region has what is west of x = 1.
const WESTERN: ChunkArea = ChunkArea {
    min_x: None,
    max_x: Some(1),
};
const EASTERN: ChunkArea = ChunkArea {
    min_x: Some(1),
    max_x: None,
};

/// The chunks a [`World`] is about: a row through the home chunk.
fn row() -> Vec<ChunkPos> {
    (-2..=3).map(|x| ChunkPos::new(x, 0)).collect()
}

/// One region of a [`World`].
struct Site {
    region: Region,
    /// What the edges and the store have sent the region and no tick has taken.
    next: TickInputs,
    /// The number of the last message of each edge that is among `next`.
    applied: BTreeMap<EdgeId, u64>,
    /// Whether the region ticks. One that does not is without a worker, or hangs:
    /// what is sent to it waits.
    running: bool,
}

/// Two regions on stripes with one store and the edges `E` and `F`. The store answers
/// a tick's notes and claims, and the edges read what a tick says, each in order and
/// either at once or, in a made-up run, some ticks later.
struct World {
    on: bool,
    sites: BTreeMap<RegionId, Site>,
    store: Store,
    edges: Edges,
    /// The store's answers on their way to each region, in order.
    answers: BTreeMap<RegionId, VecDeque<Answer>>,
    /// What each region has said to each edge and the edge has not read, in order.
    words: BTreeMap<(EdgeId, RegionId), VecDeque<Word>>,
    /// What each edge has sent each region, numbered, for `applied`.
    sent: BTreeMap<(EdgeId, RegionId), u64>,
    /// The regions whose answers of the store wait, and the edges that read nothing.
    held_answers: BTreeSet<RegionId>,
    held_edges: BTreeSet<EdgeId>,
    /// Makes up the delays of a generated run; without it nothing is delayed.
    delays: Option<Random>,
    /// Every answer the store ever gave, to say some of them again late.
    said: Vec<(RegionId, Answer)>,
    /// What was done, for a run that fails.
    steps: Vec<String>,
    /// How many stays were let go without being placed, how often a region was
    /// restored, and how many `Ended` entries were made.
    unplaced: usize,
    restores: usize,
    ended: usize,
}

impl Drop for World {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("the steps of the world were:");
            for step in &self.steps {
                eprintln!("  {step}");
            }
        }
    }
}

impl World {
    fn new(on: bool) -> Self {
        let mut world = Self {
            on,
            sites: BTreeMap::new(),
            store: Store::default(),
            edges: Edges::default(),
            answers: BTreeMap::new(),
            words: BTreeMap::new(),
            sent: BTreeMap::new(),
            held_answers: BTreeSet::new(),
            held_edges: BTreeSet::new(),
            delays: None,
            said: Vec::new(),
            steps: Vec::new(),
            unplaced: 0,
            restores: 0,
            ended: 0,
        };
        for (id, block) in [(HOME_REGION, 3), (EAST_REGION, 4)] {
            let entity_ids = EntityIds::block(block).expect("the block exists");
            let region = Region::new(config(on), entity_ids, Self::holdings(id));
            let mut site = Site {
                region,
                next: TickInputs::default(),
                applied: BTreeMap::new(),
                running: true,
            };
            Self::hello(&mut site);
            world.sites.insert(id, site);
        }
        world.settle();
        world
    }

    fn area(region: RegionId) -> ChunkArea {
        if region == HOME_REGION {
            WESTERN
        } else {
            EASTERN
        }
    }

    /// What the store says a region holds when it is opened: its part of the row.
    fn holdings(region: RegionId) -> Holdings {
        let area = Self::area(region);
        Holdings {
            held: row()
                .into_iter()
                .filter(|chunk| area.contains(*chunk))
                .collect(),
            pinned: vec![area],
        }
    }

    /// Who holds a chunk, as the store's table has it.
    fn holder(chunk: ChunkPos) -> RegionId {
        if WESTERN.contains(chunk) {
            HOME_REGION
        } else {
            EAST_REGION
        }
    }

    /// The edges say hello to a region that has begun: they are there, and each has a
    /// viewer's ticket on every chunk of the row, so that the region learns whose the
    /// chunks beyond its stripe are.
    fn hello(site: &mut Site) {
        site.next.edges.push(started(E, START));
        site.next.edges.push(started(F, START));
        site.next.tickets_added = row()
            .into_iter()
            .map(|chunk| (chunk, Ticket::Viewer))
            .collect();
    }

    fn site(&mut self, region: RegionId) -> &mut Site {
        self.sites.get_mut(&region).expect("the region is there")
    }

    fn region(&self, region: RegionId) -> &Region {
        &self.sites[&region].region
    }

    /// Hands a region what an edge sent it.
    fn send(&mut self, edge: EdgeId, to: RegionId, message: Message) {
        let site = self.sites.get_mut(&to).expect("the region is there");
        match message {
            Message::Confirm(number) => {
                site.next.edges.push(EdgeEvent::Confirmed { edge, number });
                return;
            }
            Message::Change(change) => site.next.change(change),
            Message::Input(id, entity, number, input) => {
                site.next.input(edge, id, entity, number, input);
            }
        }
        let sent = self.sent.entry((edge, to)).or_insert(0);
        *sent += 1;
        site.applied.insert(edge, *sent);
    }

    fn send_all(&mut self, edge: EdgeId, messages: Vec<(RegionId, Message)>) {
        for (to, message) in messages {
            self.send(edge, to, message);
        }
    }

    /// A connection of player `n` through `edge` begins.
    fn connect(&mut self, edge: EdgeId, n: u128) {
        self.steps
            .push(format!("player {n} connects through {edge:?}"));
        let messages = self.edges.connect(edge, player(n), n);
        self.send_all(edge, messages);
    }

    /// The connection of player `n` through `edge` ends, if there is one.
    fn disconnect(&mut self, edge: EdgeId, n: u128) {
        self.steps
            .push(format!("player {n} disconnects from {edge:?}"));
        if let Some((to, message)) = self.edges.remove(edge, player(n)) {
            self.send(edge, to, message);
        }
    }

    /// Player `n` does something on their connection through `edge`.
    fn act(&mut self, edge: EdgeId, n: u128, input: PlayerInput) {
        if let Some((to, message)) = self.edges.act(edge, player(n), input.clone()) {
            self.steps
                .push(format!("player {n} through {edge:?} in {to}: {input:?}"));
            self.send(edge, to, message);
        }
    }

    /// Another worker carries on with the region from its state: what the store was
    /// about to tell it is lost with the handle, and its edges say hello again.
    fn restore(&mut self, region: RegionId) {
        self.steps.push(format!("{region} is restored"));
        self.restores += 1;
        self.answers.remove(&region);
        let on = self.on;
        let site = self.site(region);
        let state = site.region.state();
        site.region = Region::restore(config(on), state, Self::holdings(region));
        site.next.granted.clear();
        site.next.foreign.clear();
        site.next.entered.clear();
        site.next.dead.clear();
        site.next.tickets_removed.clear();
        Self::hello(site);
    }

    /// How many of `waiting` are handed on now: all, or in a made-up run some.
    fn ready(delays: &mut Option<Random>, waiting: usize) -> usize {
        let Some(random) = delays else {
            return waiting;
        };
        if waiting == 0 || random.once_in(3) {
            return waiting;
        }
        random.below(waiting as u64 + 1) as usize
    }

    /// One tick of every region that runs, with what reached it before.
    fn tick(&mut self) {
        self.steps.push("tick".to_owned());
        // The store's answers reach their regions.
        let regions: Vec<RegionId> = self.answers.keys().copied().collect();
        for region in regions {
            if self.held_answers.contains(&region) {
                continue;
            }
            let queue = self.answers.get_mut(&region).expect("it was listed");
            let ready = Self::ready(&mut self.delays, queue.len());
            let answers: Vec<Answer> = queue.drain(..ready).collect();
            let site = self.sites.get_mut(&region).expect("the region is there");
            for answer in answers {
                match answer {
                    Answer::Enter(entered) => site.next.entered.push(entered),
                    Answer::Dead(player, stay, hops) => site.next.dead.push((player, stay, hops)),
                }
            }
        }
        // The edges read what the regions said.
        let links: Vec<(EdgeId, RegionId)> = self.words.keys().copied().collect();
        for (edge, from) in links {
            if self.held_edges.contains(&edge) {
                continue;
            }
            let queue = self.words.get_mut(&(edge, from)).expect("it was listed");
            let ready = Self::ready(&mut self.delays, queue.len());
            let words: Vec<Word> = queue.drain(..ready).collect();
            for word in words {
                let records = &self.store.records;
                let dead = |id: PlayerId, entity: EntityId| {
                    records.get(&id).is_some_and(|record| record.stay > entity)
                };
                let messages = self.edges.read(edge, from, &word, dead);
                self.send_all(edge, messages);
            }
        }

        let regions: Vec<RegionId> = self.sites.keys().copied().collect();
        for id in regions {
            let site = self.site(id);
            if !site.running {
                continue;
            }
            let mut inputs = std::mem::take(&mut site.next);
            inputs.applied = std::mem::take(&mut site.applied).into_iter().collect();
            let output = tick(&mut site.region, &inputs);
            self.handle(id, &output);
        }
        self.check();
    }

    /// What the store and the edges are given of one tick of the region `from`.
    fn handle(&mut self, from: RegionId, output: &TickOutput) {
        let on = self.on;
        let site = self.sites.get_mut(&from).expect("the region is there");
        for chunk in &output.claims {
            match Self::holder(*chunk) {
                holder if holder == from => site.next.granted.push(*chunk),
                holder => site.next.foreign.push((*chunk, holder)),
            }
        }
        assert!(
            output.returns.is_empty(),
            "{from} gave back {:?}",
            output.returns
        );
        if !on {
            assert!(output.stays.is_empty(), "{from} named {:?}", output.stays);
            assert!(site.region.state().entering.is_empty());
        }

        // The commit: the store takes the notes, answers the region, and tells every
        // region when a floor was raised.
        let holder = |chunk| Some(Self::holder(chunk)).filter(|holder| *holder != HOME_REGION);
        let (answers, all) = self.store.take(from == HOME_REGION, &output.stays, holder);
        for answer in answers {
            self.said.push((from, answer.clone()));
            self.answers.entry(from).or_default().push_back(answer);
        }
        for answer in all {
            for region in [HOME_REGION, EAST_REGION] {
                self.said.push((region, answer.clone()));
                self.answers
                    .entry(region)
                    .or_default()
                    .push_back(answer.clone());
            }
        }

        // What the edges are told, as the runner publishes it: who entered, the entries
        // other than `Departed`, and then the departures.
        let region = &self.sites[&from].region;
        for (id, event) in &output.player_events {
            let PlayerEvent::Spawned {
                attempt, entity_id, ..
            } = event
            else {
                continue;
            };
            // The edge of a player who entered: the one they are of, or the one that
            // was told they were let go in this very tick.
            let let_go = output
                .durable
                .iter()
                .find_map(|(edge, _, entry)| match entry {
                    Durable::Departed { transfer, .. } if transfer.entity_id == *entity_id => {
                        Some(*edge)
                    }
                    _ => None,
                });
            let edge = region
                .edge_of(*entity_id)
                .or(let_go)
                .expect("somebody is told");
            let word = Word::Spawned {
                player: *id,
                attempt: *attempt,
                entity: *entity_id,
            };
            self.words.entry((edge, from)).or_default().push_back(word);
        }
        let departed = |entry: &Durable| matches!(entry, Durable::Departed { .. });
        for last in [false, true] {
            for (edge, number, entry) in &output.durable {
                if departed(entry) != last {
                    continue;
                }
                match entry {
                    Durable::Ended { .. } => {
                        assert!(on, "{from} made {entry:?}");
                        self.ended += 1;
                    }
                    Durable::Departed { transfer, .. } => {
                        let placed = output.player_events.iter().any(|(_, event)| {
                            matches!(event, PlayerEvent::Spawned { entity_id, .. } if *entity_id == transfer.entity_id)
                        });
                        // One hand-over, no input applied and the attempt of the join:
                        // if nobody was told they entered, it was never placed.
                        if transfer.hops == 1
                            && transfer.last_input == 0
                            && transfer.attempt.is_some()
                            && from == HOME_REGION
                            && !placed
                            && output.events.is_empty()
                        {
                            self.unplaced += 1;
                        }
                    }
                    _ => {}
                }
                let word = Word::Entry(*number, Box::new(entry.clone()));
                self.words.entry((*edge, from)).or_default().push_back(word);
            }
        }
    }

    /// What holds after every tick: no region has an entering and a present stay of
    /// one player.
    fn check(&self) {
        for (id, site) in &self.sites {
            let state = site.region.state();
            for player in state.entering.keys() {
                assert!(
                    !state.players.contains_key(player),
                    "{id} has an entering and a present stay of {player:?}"
                );
            }
            if *id != HOME_REGION {
                assert!(state.entering.is_empty(), "{id} holds a stay as entering");
            }
        }
    }

    /// Whether nothing is on its way that is not held back.
    fn quiet(&self) -> bool {
        let answers = self
            .answers
            .iter()
            .all(|(region, queue)| queue.is_empty() || self.held_answers.contains(region));
        let words = self
            .words
            .iter()
            .all(|((edge, _), queue)| queue.is_empty() || self.held_edges.contains(edge));
        let regions = self
            .sites
            .values()
            .all(|site| !site.running || site.next == TickInputs::default());
        answers && words && regions
    }

    /// Ticks until nothing is on its way.
    fn settle(&mut self) {
        for _ in 0..200 {
            if self.quiet() {
                return;
            }
            self.tick();
        }
        panic!("what the regions, the store and the edges say does not come to an end");
    }

    /// Every stay of player `n` that some region has, present or entering: the region,
    /// the entity, the edge, and whether it is present.
    fn stays_of(&self, n: u128) -> Vec<(RegionId, EntityId, EdgeId, bool)> {
        let mut found = Vec::new();
        for (id, site) in &self.sites {
            let state = site.region.state();
            if let Some(present) = state.players.get(&player(n)) {
                found.push((*id, present.entity_id, present.edge, true));
            }
            if let Some(held) = state.entering.get(&player(n)) {
                found.push((*id, held.entity_id, held.edge, false));
            }
        }
        found
    }

    /// The connection of player `n` through `edge`, if the edge has one.
    fn view(&self, edge: EdgeId, n: u128) -> Option<&View> {
        self.edges.views.get(&(edge, player(n)))
    }

    /// What holds when nothing is on its way and nothing is held back: every player
    /// has at most one stay in all the regions together, it is the one the store has
    /// the record of, with the place the store has, and it is the stay of exactly the
    /// one connection an edge has of the player. So no connection is left whose stay is
    /// gone, which is what an `Ended` that was lost would leave.
    fn check_settled(&self, players: &[u128]) {
        assert!(self.held_answers.is_empty() && self.held_edges.is_empty());
        assert!(self.sites.values().all(|site| site.running));
        for n in players {
            let stays = self.stays_of(*n);
            let views: Vec<(EdgeId, &View)> = [E, F]
                .into_iter()
                .filter_map(|edge| Some((edge, self.view(edge, *n)?)))
                .collect();
            assert!(stays.len() <= 1, "player {n} has {stays:?}");
            assert_eq!(
                views.len(),
                stays.len(),
                "player {n}: {views:?} and {stays:?}"
            );
            let (Some((region, entity, edge, present)), Some((of, view))) =
                (stays.first(), views.first())
            else {
                continue;
            };
            assert!(*present, "player {n} is still entering");
            assert_eq!(
                (*of, view.entity, view.region),
                (*edge, Some(*entity), *region)
            );
            let state = state_of(self.region(*region), *n);
            assert_eq!(Self::holder(chunk_of(state.pose.position)), *region);
            let record = &self.store.records[&player(*n)];
            assert_eq!((record.stay, record.hops), (*entity, state.hops));
            assert_eq!(record.place, Some(place_of(&state)));
        }
    }
}

/// A step along the row to the middle of `chunk`.
fn step_to(chunk: ChunkPos) -> PlayerInput {
    walk_to(middle(chunk))
}

/// A chunk of the eastern region.
const OUT_EAST: ChunkPos = ChunkPos::new(2, 0);

/// A world in which player 1 has entered through `E` and walked into the eastern
/// region: the stay D of the rows of section 10. Returns its entity.
fn with_a_stay_in_the_east() -> (World, EntityId) {
    let mut world = World::new(true);
    world.connect(E, 1);
    world.settle();
    world.act(E, 1, step_to(OUT_EAST));
    world.settle();
    world.check_settled(&[1]);
    let stays = world.stays_of(1);
    let [(EAST_REGION, old, E, true)] = stays[..] else {
        panic!("{stays:?}");
    };
    (world, old)
}

// Section 10, the rows that take two regions, the store and the edges.

#[test]
fn row_1_a_second_login_through_another_edge_ends_the_stay_in_a_region_that_runs() {
    let (mut world, old) = with_a_stay_in_the_east();
    let place = place_of(&state_of(world.region(EAST_REGION), 1));
    world.connect(F, 1);
    world.settle();
    // The first connection was put out, by the region the stay was in.
    assert_eq!(world.edges.put_out, [(E, player(1), Some(old))]);
    assert_eq!(world.view(E, 1), None);
    // The new stay is where the old one was, with what it had. The home region never
    // placed it: the store named the eastern region as the holder of the place.
    world.check_settled(&[1]);
    let stays = world.stays_of(1);
    let [(EAST_REGION, new, F, true)] = stays[..] else {
        panic!("{stays:?}");
    };
    assert!(new > old);
    assert_eq!(place_of(&state_of(world.region(EAST_REGION), 1)), place);
    assert_eq!((world.unplaced, world.edges.by_attempt), (1, 1));
}

#[test]
fn row_1_the_new_stay_that_arrives_before_the_stores_word_takes_the_place_and_tells_the_edge() {
    let (mut world, old) = with_a_stay_in_the_east();
    // The store's word to the eastern region waits; the new stay's arrival does not.
    world.held_answers.insert(EAST_REGION);
    world.connect(F, 1);
    world.settle();
    assert_eq!(world.edges.put_out, [(E, player(1), Some(old))]);
    let stays = world.stays_of(1);
    assert!(matches!(stays[..], [(EAST_REGION, new, F, true)] if new > old));
    // The word finds nobody below the stay it names.
    world.held_answers.clear();
    world.settle();
    world.check_settled(&[1]);
    assert_eq!(world.edges.put_out.len(), 1);
}

#[test]
fn row_2_a_second_login_through_the_same_edge_ends_its_own_connection_first() {
    let (mut world, old) = with_a_stay_in_the_east();
    world.connect(E, 1);
    world.settle();
    // The edge ended its own view and sent the leave; an `Ended` that the store's word
    // makes all the same finds no such connection and is passed over.
    assert!(world.edges.put_out.is_empty());
    world.check_settled(&[1]);
    let stays = world.stays_of(1);
    assert!(matches!(stays[..], [(EAST_REGION, new, E, true)] if new > old));
}

#[test]
fn row_3_a_second_login_ends_a_stay_that_is_in_the_home_region() {
    let mut world = World::new(true);
    world.connect(E, 1);
    world.settle();
    world.act(E, 1, walk_to(NEARBY));
    world.settle();
    let old = world.stays_of(1)[0].1;
    world.connect(F, 1);
    world.settle();
    assert_eq!(world.edges.put_out, [(E, player(1), Some(old))]);
    world.check_settled(&[1]);
    // In place: the home region holds it, and places the player itself.
    let state = state_of(world.region(HOME_REGION), 1);
    assert_eq!(state.pose.position, NEARBY);
    assert_eq!(world.unplaced, 0);
}

#[test]
fn rows_4_and_5_a_login_does_not_wait_for_a_region_that_does_not_run() {
    let (mut world, old) = with_a_stay_in_the_east();
    world.site(EAST_REGION).running = false;
    // The first edge still sends what its player does.
    world.act(E, 1, step_to(ChunkPos::new(3, 0)));
    world.connect(F, 1);
    for _ in 0..6 {
        world.tick();
    }
    // The home region and the store answered at once: the new connection has its
    // entity and is under the region its place is in, where its arrival waits.
    let view = world.view(F, 1).expect("the connection is there").clone();
    assert_eq!(view.region, EAST_REGION);
    let new = view.entity.expect("the edge was told the entity");
    assert!(new > old);
    assert!(
        world.edges.put_out.is_empty(),
        "the old edge is told when the region runs"
    );

    // When the region runs, the arrival takes the old stay's place among the player
    // changes, before any input of the old stay that its edge kept is applied.
    world.site(EAST_REGION).running = true;
    world.tick();
    let state = state_of(world.region(EAST_REGION), 1);
    assert_eq!(state.entity_id, new);
    assert_eq!(chunk_of(state.pose.position), OUT_EAST);
    world.settle();
    assert_eq!(world.edges.put_out, [(E, player(1), Some(old))]);
    world.check_settled(&[1]);
}

#[test]
fn rows_8_and_9_a_stay_in_the_middle_of_a_hand_over_is_ended_where_it_lands() {
    let mut world = World::new(true);
    world.connect(E, 1);
    world.settle();
    // The first edge reads nothing from here on: the stay is let go, and its
    // `Departed` waits unread.
    world.held_edges.insert(E);
    world.act(E, 1, step_to(OUT_EAST));
    world.settle();
    assert!(world.stays_of(1).is_empty(), "the stay is in no region");
    let record = world.store.records[&player(1)].clone();
    assert_eq!(record.hops, 1, "the letting go wrote the place");

    // The new login enters at that place, which the eastern region holds.
    world.connect(F, 1);
    world.settle();
    let stays = world.stays_of(1);
    let [(EAST_REGION, new, F, true)] = stays[..] else {
        panic!("{stays:?}");
    };
    assert!(new > record.stay);
    assert_eq!(
        Some(place_of(&state_of(world.region(EAST_REGION), 1))),
        record.place
    );

    // The old edge wakes and passes its stay on: it lands where the later one is, is
    // passed over, and its edge is told.
    world.held_edges.clear();
    world.settle();
    assert_eq!(world.edges.put_out, [(E, player(1), Some(record.stay))]);
    world.check_settled(&[1]);
    assert_eq!(world.stays_of(1), stays);
}

#[test]
fn row_8_a_stay_that_lands_where_the_later_one_is_not_is_taken_in_named_and_removed() {
    let mut world = World::new(true);
    world.connect(E, 1);
    world.settle();
    world.held_edges.insert(E);
    world.act(E, 1, step_to(OUT_EAST));
    world.settle();
    let old = world.store.records[&player(1)].stay;
    // The later stay enters at the place and walks home before the earlier one lands.
    world.connect(F, 1);
    world.settle();
    world.act(F, 1, step_to(HOME));
    world.settle();
    assert!(matches!(world.stays_of(1)[..], [(HOME_REGION, _, F, true)]));
    let refused = world.store.refused;

    world.held_edges.clear();
    world.tick();
    // Taken in, as the eastern region has no later stay and remembers no floor.
    assert!(world.stays_of(1).contains(&(EAST_REGION, old, E, true)));
    world.settle();
    // The store answered its note with `Dead`, the region removed it and told its edge.
    assert_eq!(world.store.refused, refused + 1);
    assert_eq!(world.edges.put_out, [(E, player(1), Some(old))]);
    world.check_settled(&[1]);
}

#[test]
fn row_12_a_stay_whose_edge_is_dead_for_good_is_removed_all_the_same() {
    let (mut world, old) = with_a_stay_in_the_east();
    world.held_edges.insert(E);
    world.connect(F, 1);
    world.settle();
    let stays = world.stays_of(1);
    assert!(matches!(stays[..], [(EAST_REGION, new, F, true)] if new > old));
    // The word for the edge that is gone waits in its outbox.
    let outbox = &world
        .region(EAST_REGION)
        .edge(E)
        .expect("it knows E")
        .outbox;
    let told = outbox
        .values()
        .any(|entry| matches!(entry, Durable::Ended { entity, .. } if *entity == old));
    assert!(told, "{outbox:?}");
}

#[test]
fn rows_13_and_14_of_two_logins_the_later_enters_and_the_earlier_is_put_out() {
    // Within one tick, and a few ticks apart with the first not yet answered.
    for apart in [false, true] {
        let mut world = World::new(true);
        world.held_answers.insert(HOME_REGION);
        world.connect(E, 1);
        if apart {
            world.tick();
            world.tick();
        }
        world.connect(F, 1);
        world.tick();
        world.held_answers.clear();
        world.settle();
        // The first connection never had an entity: it is found by its attempt.
        assert_eq!(world.edges.put_out, [(E, player(1), None)], "{apart}");
        world.check_settled(&[1]);
        assert!(matches!(world.stays_of(1)[..], [(HOME_REGION, _, F, true)]));
    }
}

#[test]
fn row_16_a_player_who_leaves_while_entering_is_nowhere_whatever_comes_later() {
    let mut world = World::new(true);
    world.held_answers.insert(HOME_REGION);
    world.connect(E, 1);
    world.tick();
    assert!(matches!(
        world.stays_of(1)[..],
        [(HOME_REGION, _, E, false)]
    ));
    world.disconnect(E, 1);
    world.tick();
    assert!(world.stays_of(1).is_empty());
    world.held_answers.clear();
    world.settle();
    world.check_settled(&[1]);
    assert!(world.stays_of(1).is_empty() && world.edges.put_out.is_empty());

    // And one whose stay was let go to another region meanwhile: the edge has no
    // connection for the `Departed` and discards the stay.
    let (mut world, _) = with_a_stay_in_the_east();
    world.disconnect(E, 1);
    world.settle();
    world.held_edges.insert(E);
    world.connect(E, 1);
    world.settle();
    world.disconnect(E, 1);
    world.held_edges.clear();
    world.settle();
    world.check_settled(&[1]);
    assert!(world.stays_of(1).is_empty());
    assert_eq!(world.edges.discarded, 1);
}

#[test]
fn rows_18_and_19_a_home_region_that_is_restored_while_a_stay_is_entering_is_answered_again() {
    let mut world = World::new(true);
    world.held_answers.insert(HOME_REGION);
    world.connect(E, 1);
    world.tick();
    world.tick();
    // The answer is lost with the worker, and the store is asked again by the first
    // tick's naming.
    world.restore(HOME_REGION);
    world.held_answers.clear();
    world.settle();
    world.check_settled(&[1]);
    assert!(matches!(world.stays_of(1)[..], [(HOME_REGION, _, E, true)]));
}

#[test]
fn a_stay_that_a_region_names_after_a_restore_and_the_store_calls_dead_is_removed() {
    // Row 7 as far as two regions show it: the store's word for the eastern region is
    // lost with its worker, and said again when the region names its stays.
    let (mut world, old) = with_a_stay_in_the_east();
    world.held_answers.insert(EAST_REGION);
    world.held_edges.insert(F);
    world.connect(F, 1);
    for _ in 0..4 {
        world.tick();
    }
    assert!(world.stays_of(1).contains(&(EAST_REGION, old, E, true)));
    world.restore(EAST_REGION);
    world.held_answers.clear();
    world.tick();
    world.tick();
    world.tick();
    assert!(
        !world
            .stays_of(1)
            .iter()
            .any(|(_, entity, ..)| *entity == old)
    );
    world.held_edges.clear();
    world.settle();
    assert_eq!(world.edges.put_out, [(E, player(1), Some(old))]);
    world.check_settled(&[1]);
}

#[test]
fn leaving_and_joining_again_brings_a_player_back_in_place_and_from_below_the_world_to_the_spawn_point()
 {
    let (mut world, _) = with_a_stay_in_the_east();
    world.act(E, 1, PlayerInput::SetFlying { flying: true });
    world.act(E, 1, PlayerInput::SelectSlot { slot: 6 });
    world.settle();
    let before = place_of(&state_of(world.region(EAST_REGION), 1));
    assert!(before.flying);
    world.disconnect(E, 1);
    world.settle();
    world.connect(E, 1);
    world.settle();
    world.check_settled(&[1]);
    assert_eq!(place_of(&state_of(world.region(EAST_REGION), 1)), before);

    // Fallen through the floor: back at the spawn point with what they held.
    let mut below = middle(OUT_EAST);
    below.y = -90.0;
    world.act(E, 1, walk_to(below));
    world.settle();
    world.disconnect(E, 1);
    world.settle();
    world.connect(E, 1);
    world.settle();
    world.check_settled(&[1]);
    let state = state_of(world.region(HOME_REGION), 1);
    assert_eq!((state.pose, state.flying), (Pose::at(SPAWN), false));
    assert_eq!(state.selected_slot, 6);
}

// ---------------------------------------------------------------------------------------
// Made-up runs
// ---------------------------------------------------------------------------------------

/// The seeds of the made-up runs: five, or as many as `CLUSTINE_STAYS_SEEDS` says, or
/// the one `CLUSTINE_STAYS_SEED` names, to see a run that failed again.
fn seeds() -> Vec<u64> {
    if let Ok(seed) = std::env::var("CLUSTINE_STAYS_SEED") {
        return vec![seed.parse().expect("CLUSTINE_STAYS_SEED is a number")];
    }
    let count = match std::env::var("CLUSTINE_STAYS_SEEDS") {
        Ok(count) => count.parse().expect("CLUSTINE_STAYS_SEEDS is a number"),
        Err(_) => 5,
    };
    (1..=count).collect()
}

/// The players of a made-up run.
const PLAYERS: [u128; 4] = [1, 2, 3, 4];

/// Plays `rounds` rounds on a world: players connect through either edge, whether or
/// not they are somewhere already, do things, walk across the line, fall through the
/// floor and disconnect; regions are restored and stand still; the store's answers and
/// what the edges read come late, and answers the store gave long ago are said again.
/// Every forty rounds everything is let through and the world is held to
/// [`World::check_settled`]. Returns the world.
fn made_up_run(on: bool, seed: u64, rounds: usize) -> World {
    let mut world = World::new(on);
    world
        .steps
        .push(format!("seed {seed} (CLUSTINE_STAYS_SEED={seed})"));
    let mut random = Random(0x9E37_79B9_7F4A_7C15 ^ (seed << 24) ^ seed);
    world.delays = Some(Random(0xD1B5_4A32_D192_ED03 ^ seed));
    for round in 0..rounds {
        for n in PLAYERS {
            let edge = random.pick(&[E, F]);
            match random.below(40) {
                0..=2 => world.connect(edge, n),
                3 => world.disconnect(edge, n),
                4..=15 => {
                    let x = random.below(900) as f64 / 10.0 - 30.0;
                    // Now and then through the floor.
                    let y = if random.once_in(12) { -80.0 } else { 64.0 };
                    world.act(edge, n, walk_to(Vec3::new(x, y, 8.5)));
                }
                16 => {
                    let flying = random.once_in(2);
                    world.act(edge, n, PlayerInput::SetFlying { flying });
                }
                17 => {
                    let slot = random.below(9) as u8;
                    world.act(edge, n, PlayerInput::SelectSlot { slot });
                }
                _ => {}
            }
        }
        if random.once_in(30) {
            world.restore(random.pick(&[HOME_REGION, EAST_REGION]));
        }
        if random.once_in(25) {
            let site = world.site(EAST_REGION);
            site.running = !site.running;
        }
        if random.once_in(20) {
            let region = random.pick(&[HOME_REGION, EAST_REGION]);
            if !world.held_answers.remove(&region) {
                world.held_answers.insert(region);
            }
        }
        if random.once_in(20) {
            let edge = random.pick(&[E, F]);
            if !world.held_edges.remove(&edge) {
                world.held_edges.insert(edge);
            }
        }
        // An answer the store gave long ago reaches its region again: a word that a
        // stay is dead never touches a later one, and an answer to a stay that is no
        // longer entering is passed over.
        if on && !world.said.is_empty() && random.once_in(6) {
            let index = random.below(world.said.len() as u64) as usize;
            let (region, answer) = world.said[index].clone();
            // The place of an answer that is said again is the one the store has now.
            let current = match answer {
                Answer::Enter(entered) => {
                    let record = &world.store.records[&entered.player];
                    (record.stay == entered.entity).then(|| {
                        let place = record.place.clone();
                        let holder = (place.as_ref())
                            .map(|place| World::holder(chunk_of(place.pose.position)))
                            .filter(|holder| *holder != HOME_REGION);
                        Answer::Enter(Entered {
                            place,
                            holder,
                            ..entered
                        })
                    })
                }
                dead => Some(dead),
            };
            if let Some(answer) = current {
                world.answers.entry(region).or_default().push_back(answer);
            }
        }
        world.tick();

        if round % 40 == 39 {
            world.held_answers.clear();
            world.held_edges.clear();
            world.site(EAST_REGION).running = true;
            let delays = world.delays.take();
            world.settle();
            if on {
                world.check_settled(&PLAYERS);
            }
            world.delays = delays;
        }
    }
    world
}

#[test]
fn made_up_logins_leaves_and_late_answers_never_leave_two_stays_or_a_connection_without_one() {
    let mut seen = [0usize; 6];
    for seed in seeds() {
        let world = made_up_run(true, seed, 1200);
        seen[0] += world.ended;
        seen[1] += world.edges.put_out.len();
        seen[2] += world.unplaced;
        seen[3] += world.edges.by_attempt;
        seen[4] += world.store.refused;
        seen[5] += world.restores;
    }
    // The runs were about something: stays were ended and connections put out, stays
    // were let go unplaced and found by their attempt, the store called stays dead
    // that were named to it, and regions were restored.
    assert!(seen.iter().all(|count| *count >= 20), "{seen:?}");
}

#[test]
fn where_the_store_does_not_keep_the_places_no_made_up_run_ever_makes_a_note() {
    // `World::handle` holds every tick to it: no note, no entering stay, no `Ended`.
    for seed in seeds() {
        let world = made_up_run(false, seed, 600);
        assert!(world.store.records.is_empty());
        assert_eq!((world.ended, world.unplaced), (0, 0));
        assert!(world.edges.put_out.is_empty());
    }
}

/// The chunks of the region of the runs below: it holds them all from the start, so
/// that a region restored from its state knows of chunks what the region run on does.
fn square() -> Vec<ChunkPos> {
    (-3..=3)
        .flat_map(|x| (-3..=3).map(move |z| ChunkPos::new(x, z)))
        .collect()
}

/// A made-up point of the square.
fn somewhere(random: &mut Random) -> Vec3 {
    let along = |random: &mut Random| random.below(700) as f64 / 10.0 - 35.0;
    Vec3::new(along(random), 64.0, along(random))
}

/// Makes up what one tick of a region with the state `state` is given: edges that
/// start anew and are gone, joins, leaves, arrivals of stays the region knows and of
/// others, inputs, and the store's word that stays are dead. The store's answers to
/// entering stays are added by the run.
fn made_up_inputs(
    random: &mut Random,
    state: &RegionState,
    starts: &mut BTreeMap<EdgeId, u64>,
) -> TickInputs {
    let mut inputs = TickInputs::default();
    if random.once_in(40) {
        let edge = random.pick(&[E, F]);
        if random.once_in(3) {
            starts.remove(&edge);
            inputs.edges.push(EdgeEvent::Gone { edge });
        } else {
            let start = starts.entry(edge).or_insert(START);
            *start += 1;
            inputs.edges.push(started(edge, *start));
        }
    }
    for edge in [E, F] {
        if !starts.contains_key(&edge) && random.once_in(4) {
            starts.insert(edge, START);
            inputs.edges.push(started(edge, START));
        }
        if let Some(known) = state.edges.get(&edge)
            && random.once_in(3)
        {
            let number = random.below(known.sent + 1);
            inputs.edges.push(EdgeEvent::Confirmed { edge, number });
        }
    }
    for _ in 0..random.below(4) {
        let n = random.pick(&PLAYERS);
        let id = player(n);
        let present = state.players.get(&id);
        let held = state.entering.get(&id);
        let edge = match present {
            Some(present) if !random.once_in(4) => present.edge,
            _ => random.pick(&[E, F, STRANGER]),
        };
        // An entity of the player's, one before it, one after it, or one from
        // elsewhere.
        let own = present.map_or(state.next_entity_id, |present| present.entity_id);
        let named = EntityId(own.0 + random.pick(&[0, 0, -1, 1, -3, 2]));
        match random.below(14) {
            0 | 1 => inputs.change(join(edge, n, 100 + random.below(4))),
            2 => inputs.change(leave_as(edge, n, named)),
            3 => {
                let attempt = match (present.and_then(|present| present.attempt), held) {
                    (Some(attempt), _) if !random.once_in(3) => Some(attempt),
                    (_, Some(held)) if !random.once_in(3) => Some(held.attempt),
                    _ => random.once_in(2).then(|| 100 + random.below(4)),
                };
                inputs.change(leave_of(edge, n, attempt));
            }
            4 | 5 => {
                // No stay arrives that the region has yet to give out, nor one that is
                // not below a stay it holds as entering: only the home region gives out
                // ids, in ascending order, and an entering stay has not left it.
                let to_come = ids().contains(named) && named >= state.next_entity_id;
                if to_come || held.is_some_and(|held| named >= held.entity_id) {
                    continue;
                }
                let arriving = PlayerTransfer {
                    name: name(n),
                    flying: random.once_in(2),
                    last_input: random.below(20),
                    ..transfer(
                        named,
                        random.below(4) as u32,
                        random.once_in(3).then(|| 100 + random.below(4)),
                        somewhere(random),
                    )
                };
                inputs.change(PlayerChange::Arrive(edge, id, arriving));
            }
            6 => {
                let stay = EntityId(named.0 + random.pick(&[0, 1, 1, 2]));
                inputs.dead.push((id, stay, random.below(3) as u32));
            }
            _ => {
                let last = present.map_or(0, |present| present.last_input);
                let number = last + random.pick(&[1, 1, 1, 2, 0]);
                let input = match random.below(6) {
                    0 => PlayerInput::SetFlying {
                        flying: random.once_in(2),
                    },
                    1 => PlayerInput::SelectSlot {
                        slot: random.below(9) as u8,
                    },
                    _ => walk_to(somewhere(random)),
                };
                inputs.input(edge, id, named, number, input);
            }
        }
    }
    inputs
}

/// What the store answers the entering stays a tick named, in a run of one region:
/// with a place or none, held by another region or by nobody, and now and then for a
/// stay that is not the one named.
fn made_up_answers(random: &mut Random, output: &TickOutput) -> Vec<Entered> {
    let mut answers = Vec::new();
    for note in &output.stays {
        let StayNote::Entering { player, entity } = note else {
            continue;
        };
        if random.once_in(5) {
            continue;
        }
        let place = (!random.once_in(3)).then(|| {
            let mut place = place_in(HOME);
            place.pose.position = somewhere(random);
            if random.once_in(6) {
                place.pose.position.y = -80.0;
            }
            place
        });
        let entity = EntityId(entity.0 - i32::from(random.once_in(8)));
        answers.push(Entered {
            player: *player,
            entity,
            place,
            holder: random.once_in(3).then_some(OTHER),
        });
    }
    answers
}

/// A region restored from the state after any tick does what the region that is run on
/// does, input for input, but for the notes of its first tick, which names every stay
/// (section 3 of the record). Where the store does not keep the places it does the
/// same from its first tick on.
#[test]
fn a_region_restored_after_any_tick_does_what_the_region_run_on_does_but_for_its_first_notes() {
    for on in [true, false] {
        let mut compared = 0;
        let mut named = 0;
        for seed in seeds() {
            let mut random = Random(0xC2B2_AE3D_27D4_EB4F ^ (seed << 16) ^ seed);
            let holdings = Holdings {
                held: square(),
                pinned: vec![ChunkArea::EVERYWHERE],
            };
            let mut region = Region::new(config(on), ids(), holdings.clone());
            let mut starts: BTreeMap<EdgeId, u64> = BTreeMap::new();
            let mut pending: Vec<Entered> = Vec::new();
            // The copy that was restored last, and how many ticks it has run.
            let mut restored: Option<(Region, usize)> = None;
            for round in 0..700 {
                if round % 9 == 0 {
                    let copy = Region::restore(config(on), region.state(), holdings.clone());
                    restored = Some((copy, 0));
                }
                let mut inputs = made_up_inputs(&mut random, &region.state(), &mut starts);
                // The store's answers come a tick later, or two.
                if !random.once_in(3) {
                    inputs.entered = std::mem::take(&mut pending);
                }
                let output = tick(&mut region, &inputs);
                pending.extend(made_up_answers(&mut random, &output));
                if !on {
                    assert!(output.stays.is_empty(), "seed {seed}, round {round}");
                }
                assert!(output.claims.is_empty() && output.returns.is_empty());

                let Some((copy, ticks)) = &mut restored else {
                    continue;
                };
                let context = format!("{on}, seed {seed}, round {round}: {inputs:#?}");
                let mut theirs = tick(copy, &inputs);
                let state = region.state();
                assert_eq!(copy.state(), state, "{context}");
                if *ticks == 0 && on {
                    // Every stay the region has after the tick, and whoever it let go.
                    let said: BTreeSet<(PlayerId, bool)> = theirs
                        .stays
                        .iter()
                        .map(|note| match note {
                            StayNote::Entering { player, .. } => (*player, false),
                            StayNote::Has { player, .. } => (*player, true),
                        })
                        .collect();
                    let let_go = output.stays.iter().filter_map(|note| match note {
                        StayNote::Has { player, .. } if !state.players.contains_key(player) => {
                            Some((*player, true))
                        }
                        _ => None,
                    });
                    let every: BTreeSet<(PlayerId, bool)> = (state.players.keys())
                        .map(|player| (*player, true))
                        .chain(state.entering.keys().map(|player| (*player, false)))
                        .chain(let_go)
                        .collect();
                    assert_eq!(said, every, "{context}");
                    // What the region run on named, the restored one named the same.
                    for note in &output.stays {
                        assert!(theirs.stays.contains(note), "{context}");
                    }
                    named += theirs.stays.len() - output.stays.len();
                    theirs.stays = output.stays.clone();
                }
                assert_eq!(theirs, output, "{context}");
                *ticks += 1;
                compared += 1;
            }
        }
        assert!(compared > 2000, "{compared}");
        assert_eq!(named > 200, on, "{named}");
    }
}

/// The same made-up ticks keep to what the record says of a region by itself: it
/// never has an entering and a present stay of one player after a tick, and every stay
/// that leaves it without its own edge having asked, and without the edge being gone
/// or reset in that tick, has its edge told with exactly one `Ended`.
#[test]
fn made_up_ticks_of_one_region_never_leave_two_stays_and_tell_every_edge_whose_stay_they_end() {
    let mut seen = [0usize; 3];
    for seed in seeds() {
        let mut random = Random(0x1656_67B1_9E37_79F9 ^ (seed << 16) ^ seed);
        let holdings = Holdings {
            held: square(),
            pinned: vec![ChunkArea::EVERYWHERE],
        };
        let mut region = Region::new(config(true), ids(), holdings);
        let mut starts: BTreeMap<EdgeId, u64> = BTreeMap::new();
        let mut pending: Vec<Entered> = Vec::new();
        for round in 0..3000 {
            let before = region.state();
            let mut inputs = made_up_inputs(&mut random, &before, &mut starts);
            if !random.once_in(3) {
                inputs.entered = std::mem::take(&mut pending);
            }
            let output = tick(&mut region, &inputs);
            pending.extend(made_up_answers(&mut random, &output));
            let after = region.state();
            let context = format!("seed {seed}, round {round}: {inputs:#?}\n{output:#?}");
            for id in after.entering.keys() {
                assert!(!after.players.contains_key(id), "{context}");
            }

            // The stays that were in the region and are not: each went by a leave of
            // its edge, with its edge, by being let go, or with an `Ended` to its edge.
            // One of which a copy arrived in the tick is not judged.
            let reset: BTreeSet<EdgeId> = inputs
                .edges
                .iter()
                .filter_map(|event| match event {
                    EdgeEvent::Gone { edge } | EdgeEvent::Started { edge, .. } => Some(*edge),
                    EdgeEvent::Confirmed { .. } => None,
                })
                .collect();
            let mut gone: Vec<(PlayerId, EntityId, EdgeId, Option<u64>)> = Vec::new();
            for (id, was) in &before.players {
                let stays = (after.players.get(id)).is_some_and(|is| is.entity_id == was.entity_id);
                if !stays {
                    gone.push((*id, was.entity_id, was.edge, was.attempt));
                }
            }
            for (id, was) in &before.entering {
                let stays =
                    (after.entering.get(id)).is_some_and(|is| is.entity_id == was.entity_id);
                let placed =
                    (after.players.get(id)).is_some_and(|is| is.entity_id == was.entity_id);
                if !stays && !placed {
                    gone.push((*id, was.entity_id, was.edge, Some(was.attempt)));
                }
            }
            let told: Vec<(EdgeId, PlayerId, EntityId)> = output
                .durable
                .iter()
                .filter_map(|(edge, _, entry)| match entry {
                    Durable::Ended { player, entity, .. } => Some((*edge, *player, *entity)),
                    _ => None,
                })
                .collect();
            for (id, entity, edge, attempt) in &gone {
                let left = inputs.player_changes.iter().any(|change| match change {
                    PlayerChange::Leave(of, who, named, by) => {
                        of == edge
                            && who == id
                            && (*named == Some(*entity)
                                || (named.is_none() && by.is_some() && by == attempt))
                    }
                    _ => false,
                });
                let let_go = output.durable.iter().any(|(_, _, entry)| {
                    matches!(entry, Durable::Departed { transfer, .. } if transfer.entity_id == *entity)
                });
                let copied = inputs.player_changes.iter().any(|change| {
                    matches!(change, PlayerChange::Arrive(_, who, transfer) if who == id && transfer.entity_id == *entity)
                });
                let count = told
                    .iter()
                    .filter(|(to, who, which)| (to, who, which) == (edge, id, entity))
                    .count();
                if copied {
                    // A copy of the stay arrived in the tick: it took the place, or
                    // was itself passed over with a word to the edge it came through.
                    continue;
                }
                if left || let_go || reset.contains(edge) {
                    assert!(count <= 1, "{context}");
                    seen[1] += 1;
                } else {
                    assert_eq!(count, 1, "{id:?} lost {entity:?}: {context}");
                    seen[0] += 1;
                }
            }
            seen[2] += usize::from(!after.entering.is_empty());
        }
    }
    assert!(seen.iter().all(|count| *count >= 100), "{seen:?}");
}
