//! Tests of the credits, the way out of the End and the respawn's placing, tick by tick
//! (`docs/groundwork/p-plan.md`, section 19.7): a player in the End's portal who has
//! not seen the credits is taken out of the world and waits to be asked; only
//! `PerformRespawn` sends them on, to the overworld's home; that home places them at the
//! world spawn, or at their bed's stand-up place, with what the edge owes the client;
//! a bed gone owes the missing block's event and is forgotten; a player who has seen
//! the credits goes out by the portal.

use clustine_data::blocks;
use clustine_world::{
    Biome, BlockPos, Chunk, ChunkPos, Dimension, EdgeId, EntityId, EntityIds, PlayerId, RegionId,
    Travel, Vec3, Way,
};

use super::super::tnt::tests::{FLOOR, add_player, at, idle, state};
use super::super::{Holdings, Region, RegionConfig};
use crate::api::{
    Durable, EdgeEvent, Inventory, PlayerChange, PlayerInput, PlayerTransfer, Pose, RegionEvent,
    Respawn, Respawned, TickInputs, TickOutput, Ticket,
};
use crate::state::RegionState;

const EDGE: EdgeId = EdgeId(1);
const OVERWORLD_HOME: RegionId = RegionId(0);

/// The world's spawn, on the tests' floor.
const WORLD_SPAWN: BlockPos = BlockPos::new(0, FLOOR + 1, 0);

/// A region of `dimension` with a stone floor at `floor` over the chunks within `reach`
/// of `centre`, the blocks `blocks` names above it, the homes and a known edge; the
/// overworld's home where `spawn` is given (its home chunk is the spawn's).
fn world(
    dimension: Dimension,
    spawn: Option<Vec3>,
    reach: i32,
    floor: i32,
    blocks: impl Fn(BlockPos) -> Option<clustine_data::BlockState>,
) -> Region {
    let rules = dimension.rules();
    let held: Vec<ChunkPos> = (-reach..=reach)
        .flat_map(|x| (-reach..=reach).map(move |z| ChunkPos::new(x, z)))
        .collect();
    let chunks: Vec<(ChunkPos, Chunk)> = held
        .iter()
        .map(|position| {
            let mut chunk = Chunk::empty(rules, Biome(0));
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
    let config = RegionConfig {
        world_spawn: WORLD_SPAWN,
        strongholds: Vec::new(),
        homes: vec![
            (Dimension::Overworld, OVERWORLD_HOME),
            (Dimension::Nether, RegionId(1)),
            (Dimension::End, RegionId(2)),
        ],
        dimension,
        spawn,
        starting_inventory: Inventory::default(),
        return_after: u64::MAX,
        lowest_y: rules.min_y,
        seed: 9,
        game_seed: 0,
        sea_level: 63,
        spawn_mobs: false,
        light: Default::default(),
        features: Default::default(),
        random_tick_speed: crate::region::RANDOM_TICK_SPEED,
    };
    let state = RegionState::new(EntityIds::block(0).expect("a block of ids"));
    let mut region = Region::restore_loaded(config, state, holdings, &tickets, chunks, Vec::new());
    region.tick(&TickInputs::default());
    region.tick(&TickInputs {
        edges: vec![EdgeEvent::Started {
            edge: EDGE,
            start: 1,
        }],
        ..TickInputs::default()
    });
    region
}

/// The End, with an End portal's block at (2, 41, 2) on a floor at 40.
fn the_end() -> Region {
    let portal = at(2, 41, 2);
    world(Dimension::End, None, 1, 40, |pos| {
        (pos == portal).then_some(blocks::END_PORTAL)
    })
}

/// The overworld's home, its spawn at [`WORLD_SPAWN`], with `blocks` above its floor.
fn overworld_home(blocks: impl Fn(BlockPos) -> Option<clustine_data::BlockState>) -> Region {
    let spawn = Vec3::new(0.5, f64::from(FLOOR + 1), 0.5);
    world(Dimension::Overworld, Some(spawn), 1, FLOOR, blocks)
}

fn input(id: PlayerId, entity: EntityId, number: u64, input: PlayerInput) -> TickInputs {
    TickInputs {
        inputs: vec![(EDGE, id, entity, number, input)],
        ..TickInputs::default()
    }
}

fn departed(output: &TickOutput, id: PlayerId) -> Option<(PlayerTransfer, RegionId)> {
    output.durable.iter().find_map(|(_, _, entry)| match entry {
        Durable::Departed {
            player,
            transfer,
            to,
        } if *player == id => Some((transfer.clone(), *to)),
        _ => None,
    })
}

fn placed(output: &TickOutput, id: PlayerId) -> Option<(Pose, Option<Respawned>)> {
    output.durable.iter().find_map(|(_, _, entry)| match entry {
        Durable::Placed {
            player,
            pose,
            respawn,
            ..
        } if *player == id => Some((*pose, *respawn)),
        _ => None,
    })
}

fn won(output: &TickOutput, id: PlayerId) -> bool {
    output
        .durable
        .iter()
        .any(|(_, _, entry)| matches!(entry, Durable::WonGame { player, .. } if *player == id))
}

/// A player in the End's portal who has not seen the credits, after the tick that shows
/// them: the End, the player and their entity.
fn in_the_credits() -> (Region, PlayerId, EntityId, TickOutput) {
    let mut end = the_end();
    let (id, entity) = add_player(&mut end, 1, [2.5, 41.0, 2.5], false);
    let output = idle(&mut end);
    (end, id, entity, output)
}

/// `showEndCredits`: in the tick they stand in the portal the player is out of the
/// world, removed for whoever watched, and their edge told they won; nothing sends
/// them on yet.
#[test]
fn a_player_in_the_end_s_portal_who_has_not_seen_the_credits_is_shown_them_and_waits() {
    let (end, id, entity, output) = in_the_credits();
    assert!(won(&output, id), "{:?}", output.durable);
    assert!(end.player(id).is_none(), "out of the world");
    assert!(output.events.iter().any(|event| matches!(
        event,
        RegionEvent::EntityRemoved { entity: removed, .. } if *removed == entity
    )));
    assert!(departed(&output, id).is_none());
    let waiting = end.waiting_state(id).expect("a waiting stay");
    assert!(waiting.until_asked);
    assert!(waiting.transfer.seen_credits);
}

/// Inputs of a player in the credits are dropped, and only `PerformRespawn` sends them
/// on: to the overworld's home, bound for the world spawn, with the credits' way.
#[test]
fn only_the_ask_for_the_respawn_sends_a_player_in_the_credits_on_to_the_overworld_s_home() {
    let (mut end, id, entity, _) = in_the_credits();
    let moved = PlayerInput::Move {
        position: Some(Vec3::new(30.5, 41.0, 2.5)),
        rotation: None,
        on_ground: true,
    };
    let output = end.tick(&input(id, entity, 1, moved));
    assert!(departed(&output, id).is_none());
    assert!(end.player(id).is_none());
    for _ in 0..3 {
        assert!(departed(&idle(&mut end), id).is_none(), "waits to be asked");
    }
    let output = end.tick(&input(id, entity, 2, PlayerInput::PerformRespawn));
    let (transfer, to) = departed(&output, id).expect("sent on at the ask");
    assert_eq!(to, OVERWORLD_HOME);
    assert_eq!(
        transfer.travel,
        Some(Travel {
            to: Dimension::Overworld,
            target: WORLD_SPAWN,
            way: Way::Credits,
        })
    );
    assert_eq!(transfer.dimension, Dimension::Overworld);
    assert!(transfer.seen_credits);
    assert!(end.waiting_state(id).is_none());
}

/// The credits' respawn sent to the overworld's home, with the respawn `respawn`.
fn credits_respawn(respawn: Option<Respawn>) -> (PlayerId, PlayerTransfer) {
    let (mut end, id, entity, _) = in_the_credits();
    let travel = end.respawn_travel(respawn, Way::Credits);
    if let Some(waiting) = end.waiting.get_mut(&id) {
        waiting.transfer.respawn = respawn;
        super::at_target(&mut waiting.transfer, travel);
    }
    let output = end.tick(&input(id, entity, 1, PlayerInput::PerformRespawn));
    let (transfer, _) = departed(&output, id).expect("sent on");
    (id, transfer)
}

/// The overworld's home's placing of `transfer`, within a few ticks.
fn placed_at_home(
    home: &mut Region,
    id: PlayerId,
    transfer: PlayerTransfer,
) -> (Pose, Option<Respawned>) {
    let output = home.tick(&TickInputs {
        player_changes: vec![PlayerChange::Arrive(EDGE, id, transfer)],
        ..TickInputs::default()
    });
    let mut found = placed(&output, id);
    for _ in 0..5 {
        if found.is_some() {
            break;
        }
        found = placed(&idle(home), id);
    }
    found.expect("placed within a few ticks")
}

/// A player with no respawn wakes at the world spawn, keeping everything (keep 1), with
/// no event owed.
#[test]
fn the_credits_respawn_without_a_bed_is_placed_at_the_world_spawn_keeping_everything() {
    let (id, transfer) = credits_respawn(None);
    let mut home = overworld_home(|_| None);
    let (pose, respawn) = placed_at_home(&mut home, id, transfer);
    assert_eq!(
        respawn,
        Some(Respawned {
            keep: 1,
            no_respawn_block: false,
            anchor_sound: None,
        })
    );
    assert_eq!(
        [pose.position.x, pose.position.z],
        [0.5, 0.5],
        "at the world spawn"
    );
    assert!(home.player(id).is_some());
}

/// A bed head at (4, FLOOR + 1, 4) facing north, its foot to the south.
fn bed_head() -> BlockPos {
    at(4, FLOOR + 1, 4)
}

fn a_bed(pos: BlockPos) -> Option<clustine_data::BlockState> {
    if pos == bed_head() {
        Some(state(
            "minecraft:red_bed[facing=north,occupied=false,part=head]",
        ))
    } else if pos == bed_head().offset(0, 0, 1) {
        Some(state(
            "minecraft:red_bed[facing=north,occupied=false,part=foot]",
        ))
    } else {
        None
    }
}

fn respawn_at_the_bed() -> Respawn {
    Respawn {
        pos: bed_head(),
        yaw: 0.0,
        pitch: 0.0,
        dimension: Dimension::Overworld,
    }
}

/// A bed that stands is woken at: the player is placed beside it facing it, its
/// respawn kept, with no event.
#[test]
fn the_credits_respawn_at_a_bed_that_stands_wakes_beside_it_and_keeps_it() {
    let (id, transfer) = credits_respawn(Some(respawn_at_the_bed()));
    let mut home = overworld_home(a_bed);
    let (pose, respawn) = placed_at_home(&mut home, id, transfer);
    assert_eq!(
        respawn,
        Some(Respawned {
            keep: 1,
            no_respawn_block: false,
            anchor_sound: None,
        })
    );
    let beside = (pose.position.x - 4.5).abs() <= 2.0 && (pose.position.z - 4.5).abs() <= 3.0;
    assert!(beside, "beside the bed: {pose:?}");
    assert_eq!(pose.pitch, 0.0);
    let state = home.player_state(id).expect("in the world");
    assert_eq!(state.respawn, Some(respawn_at_the_bed()));
}

/// A bed gone owes the missing block's event: the player wakes at the world spawn and
/// their respawn is forgotten.
#[test]
fn the_credits_respawn_at_a_bed_gone_is_at_the_world_spawn_with_the_event_and_forgets_it() {
    let (id, transfer) = credits_respawn(Some(respawn_at_the_bed()));
    let mut home = overworld_home(|_| None);
    let (pose, respawn) = placed_at_home(&mut home, id, transfer);
    assert_eq!(
        respawn,
        Some(Respawned {
            keep: 1,
            no_respawn_block: true,
            anchor_sound: None,
        })
    );
    assert_eq!([pose.position.x, pose.position.z], [0.5, 0.5]);
    let state = home.player_state(id).expect("in the world");
    assert_eq!(state.respawn, None);
}

/// A player who has seen the credits goes out by the End's portal to the overworld's
/// home, bound for their respawn (here none: the world spawn), with the portal's way.
#[test]
fn a_player_who_has_seen_the_credits_goes_out_of_the_end_by_its_portal() {
    let mut end = the_end();
    let (id, _) = add_player(&mut end, 1, [2.5, 41.0, 2.5], false);
    end.players.get_mut(&id).expect("added").seen_credits = true;
    let mut sent = None;
    for _ in 0..3 {
        let output = idle(&mut end);
        assert!(!won(&output, id));
        if let Some(found) = departed(&output, id) {
            sent = Some(found);
            break;
        }
    }
    let (transfer, to) = sent.expect("the portal fires at once");
    assert_eq!(to, OVERWORLD_HOME);
    assert_eq!(
        transfer.travel,
        Some(Travel {
            to: Dimension::Overworld,
            target: WORLD_SPAWN,
            way: Way::EndPortal,
        })
    );
}

/// The health told to `id` in a tick, if any.
fn health_told(output: &TickOutput, id: PlayerId) -> Option<f32> {
    output
        .player_events
        .iter()
        .find_map(|(player, event)| match event {
            crate::api::PlayerEvent::Health { health } if *player == id => Some(*health),
            _ => None,
        })
}

fn entity_event(output: &TickOutput, entity: EntityId, wanted: u8) -> bool {
    output.events.iter().any(|event| {
        matches!(event, RegionEvent::EntityEvent { entity: of, event, .. }
            if *of == entity && *event == wanted)
    })
}

fn died(output: &TickOutput, id: PlayerId) -> Option<String> {
    output.durable.iter().find_map(|(_, _, entry)| match entry {
        Durable::Died {
            player, message, ..
        } if *player == id => Some(message.clone()),
        _ => None,
    })
}

/// A player in the End fallen below its floor less 64, with a stack in their hand.
fn fallen_into_the_void() -> (Region, PlayerId, EntityId) {
    let mut end = the_end();
    let (id, entity) = add_player(&mut end, 1, [2.5, -70.0, 2.5], false);
    end.players
        .get_mut(&id)
        .expect("added")
        .inventory
        .set(0, Some(clustine_world::ItemStack::new(1, 64)));
    (end, id, entity)
}

/// `onBelowWorld` and `hurtServer`: the void takes 4 in full each time the hurt
/// cooldown has run down to half, ten ticks apart, a creative player too, each hit
/// shown with its damage event and the health told; the fifth kills, with the void's
/// message, what was carried gone and the death shown (entity event 3).
#[test]
fn the_void_hurts_a_player_by_four_every_ten_ticks_and_the_fifth_hit_kills() {
    let (mut end, id, entity) = fallen_into_the_void();
    let mut told = Vec::new();
    let mut death = None;
    for tick in 0..60 {
        let output = idle(&mut end);
        if let Some(health) = health_told(&output, id) {
            told.push((tick, health));
            let damage = output.events.iter().any(|event| {
                matches!(event, RegionEvent::DamageEvent { entity: of, .. } if *of == entity)
            });
            assert!(damage, "tick {tick}: a full hit shows its damage");
        }
        if let Some(message) = died(&output, id) {
            assert!(entity_event(&output, entity, 3));
            death = Some((tick, message));
            break;
        }
    }
    assert_eq!(
        told,
        [(0, 16.0), (10, 12.0), (20, 8.0), (30, 4.0), (40, 0.0)]
    );
    assert_eq!(death, Some((40, "death.attack.outOfWorld".to_owned())));
    let state = end.player_state(id).expect("their body stays");
    assert!(state.dead);
    assert_eq!(
        state.inventory,
        Inventory::default(),
        "dropped into the void"
    );
}

/// The body stays 20 ticks and is then removed (entity event 60); the stay waits to be
/// asked, the respawned body made with it: full health, alive. Inputs are dropped
/// meanwhile.
#[test]
fn a_dead_player_s_body_goes_after_twenty_ticks_and_the_stay_waits_to_be_asked() {
    let (mut end, id, entity) = fallen_into_the_void();
    let mut ticks = 0;
    while died(&idle(&mut end), id).is_none() {
        ticks += 1;
        assert!(ticks < 60, "dies");
    }
    let moved = PlayerInput::Move {
        position: Some(Vec3::new(2.5, 41.0, 2.5)),
        rotation: None,
        on_ground: true,
    };
    let output = end.tick(&input(id, entity, 1, moved));
    assert!(departed(&output, id).is_none());
    let pose = end.player(id).expect("the body stays").1;
    assert!(pose.position.y < -64.0, "a dead player's move is dropped");
    let mut gone = None;
    for tick in 2..=25 {
        let output = idle(&mut end);
        if entity_event(&output, entity, 60) {
            gone = Some(tick);
            break;
        }
    }
    assert_eq!(gone, Some(20), "20 ticks after the death");
    assert!(end.player(id).is_none());
    let waiting = end.waiting_state(id).expect("waits to be asked");
    assert!(waiting.until_asked);
    assert_eq!(waiting.transfer.health, 20.0);
    assert!(!waiting.transfer.dead);
    assert_eq!(
        waiting.transfer.travel.map(|travel| travel.way),
        Some(Way::Respawn)
    );
}

/// An ask while the body stays ends it at once and sends the player on to the
/// overworld's home with the respawn's way.
#[test]
fn the_ask_of_a_dead_player_ends_the_body_and_sends_them_on_at_once() {
    let (mut end, id, entity) = fallen_into_the_void();
    let mut ticks = 0;
    while died(&idle(&mut end), id).is_none() {
        ticks += 1;
        assert!(ticks < 60, "dies");
    }
    let output = end.tick(&input(id, entity, 1, PlayerInput::PerformRespawn));
    assert!(entity_event(&output, entity, 60));
    let (transfer, to) = departed(&output, id).expect("sent on at the ask");
    assert_eq!(to, OVERWORLD_HOME);
    assert_eq!(
        transfer.travel,
        Some(Travel {
            to: Dimension::Overworld,
            target: WORLD_SPAWN,
            way: Way::Respawn,
        })
    );
    assert_eq!(transfer.health, 20.0);
}

/// A player who dies in the overworld's home is placed by it at the world spawn, with
/// no departure: `Placed` owes the client a `Respawn` keeping nothing, and their full
/// health is told.
#[test]
fn a_player_who_dies_in_the_overworld_s_home_is_respawned_there_without_a_departure() {
    let mut home = overworld_home(|_| None);
    let (id, entity) = add_player(&mut home, 1, [3.5, -140.0, 3.5], false);
    let mut ticks = 0;
    while died(&idle(&mut home), id).is_none() {
        ticks += 1;
        assert!(ticks < 60, "dies");
    }
    let mut output = home.tick(&input(id, entity, 1, PlayerInput::PerformRespawn));
    assert!(departed(&output, id).is_none());
    let mut found = placed(&output, id);
    for _ in 0..5 {
        if found.is_some() {
            break;
        }
        output = idle(&mut home);
        assert!(departed(&output, id).is_none());
        found = placed(&output, id);
    }
    let (pose, respawn) = found.expect("placed within a few ticks");
    assert_eq!(
        respawn,
        Some(Respawned {
            keep: 0,
            no_respawn_block: false,
            anchor_sound: None,
        })
    );
    assert_eq!([pose.position.x, pose.position.z], [0.5, 0.5]);
    assert_eq!(health_told(&output, id), Some(20.0));
    let state = home.player_state(id).expect("in the world again");
    assert!(!state.dead);
    assert_eq!(state.health, 20.0);
}
