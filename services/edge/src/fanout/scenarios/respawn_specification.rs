//! Tests of what the edge sends a client for a respawn, written from the specification
//! by someone who did not write the edge's handling of it: `docs/groundwork/p-plan.md`,
//! section 19.7 and its review (items 2 and 3), and the game's `PlayerList.respawn` in
//! `docs/groundwork/p-respawn-facts.md`, section 5. A death (`Died`) or the credits
//! (`WonGame`), or a departure out of the End by its portal, owes the client a respawn:
//! nothing is sent at the hand-over, what the regions tell meanwhile is held, and the
//! placing sends the abilities, the missing block's event where owed, `Respawn` with the
//! keep, the place, the level's information, the anchor's sound where owed, and then
//! what was held.

use clustine_protocol::packet_ids::play::clientbound as ids;
use clustine_world::{Travel, Way};

use super::*;

/// The anchor of these tests, in the Nether.
const ANCHOR: BlockPos = BlockPos::new(-45, 70, 37);

fn died(entity: EntityId) -> Durable {
    Durable::Died {
        player: player(1),
        entity,
        message: clustine_sim::api::DeathMessage::plain("death.attack.outOfWorld"),
        at: DIED_IN,
    }
}

/// Where player 1 dies in these tests: below the overworld, west of its origin.
const DIED_IN: (Dimension, BlockPos) = (Dimension::Overworld, BlockPos::new(-7, -130, 9));

/// Player 1's transfer as a region lets them go on `way` to `target` in `to`, standing
/// in `chunk` of it.
fn respawning(way: Way, to: Dimension, target: BlockPos, chunk: ChunkPos) -> PlayerTransfer {
    let mut moved = transfer(EntityId(5), 0, chunk);
    moved.dimension = to;
    moved.travel = Some(Travel { to, target, way });
    moved
}

fn placed_with(region_dimension: Dimension, at: Vec3, respawn: Respawned) -> Durable {
    Durable::Placed {
        player: player(1),
        entity: EntityId(5),
        dimension: region_dimension,
        pose: Pose {
            position: at,
            yaw: 30.0,
            pitch: 0.0,
            on_ground: true,
        },
        flying: false,
        respawn: Some(respawn),
        gateway: false,
    }
}

fn respawned(keep: u8, no_respawn_block: bool, anchor_sound: Option<BlockPos>) -> Respawned {
    Respawned {
        keep,
        no_respawn_block,
        anchor_sound,
    }
}

/// Serves `region`'s chunks of the view about `centre`, as far as the edge asks.
fn serve(edge: &mut Harness, region: RegionId, centre: ChunkPos) {
    for chunk in view(centre) {
        if edge.at(region).subscriptions.contains_key(&chunk) {
            let ask = edge.ask(region, chunk);
            edge.tell(region, snapshot(chunk, ask));
        }
    }
}

/// The index of the first `id` in `after`.
fn first(after: &[i32], id: i32) -> usize {
    after
        .iter()
        .position(|found| *found == id)
        .unwrap_or_else(|| panic!("no packet {id} in {after:?}"))
}

/// Player 1, standing in the west, dies there and is let go by the respawn's way to the
/// east, bound for `EASTERN`, with the region's word about their health told between.
/// Returns where the client's packets stood before the placing.
async fn died_and_let_go_east(edge: &mut Harness, client: &mut Client) -> usize {
    edge.say(WEST, died(EntityId(5)));
    edge.tell(
        WEST,
        WorkerToEdge::ToPlayer {
            player: player(1),
            event: PlayerEvent::Health {
                health: 0.0,
                food: 20,
                saturation: 5.0,
            },
        },
    );
    edge.say(
        WEST,
        Durable::Departed {
            player: player(1),
            transfer: respawning(
                Way::Respawn,
                Dimension::Overworld,
                block_of(EASTERN),
                EASTERN,
            ),
            to: EAST,
        },
    );
    edge.settle(WEST).await;
    edge.quiet().await;
    serve(edge, EAST, EASTERN);
    edge.quiet().await;
    client.drain();
    client.ids.len()
}

/// Facts 5.3 (PL 959–1011): `Respawn`, then the place (`connection.teleport`, PL 993),
/// then `SetDefaultSpawnPosition` (PL 1001), then `ChangeDifficulty` (PL 1011); the
/// abilities (from `restoreFrom`) and the missing block's event before them all.
#[tokio::test]
async fn the_placing_of_a_respawn_follows_player_list_respawn_s_order() {
    let (mut edge, mut client) = standing_in_the_west().await;
    let from = died_and_let_go_east(&mut edge, &mut client).await;
    edge.say(
        EAST,
        placed_with(
            Dimension::Overworld,
            within(EASTERN),
            respawned(0, true, None),
        ),
    );
    edge.settle(EAST).await;
    edge.sync(&mut client).await;
    let after = ids_from(&client, from);
    let order = [
        ids::PLAYER_ABILITIES,
        ids::GAME_EVENT,
        ids::RESPAWN,
        ids::PLAYER_POSITION,
        ids::SET_DEFAULT_SPAWN_POSITION,
        ids::CHANGE_DIFFICULTY,
    ];
    let found: Vec<usize> = order.iter().map(|id| first(&after, *id)).collect();
    let mut sorted = found.clone();
    sorted.sort_unstable();
    assert_eq!(found, sorted, "{after:?}");
    edge.end().await;
}

/// 19.7 and facts 3 and 5.3: a death owes `Respawn` with keep 0, nothing at the
/// hand-over; the missing block's event goes after the abilities and before `Respawn`;
/// the place is absolute and still.
#[tokio::test]
async fn a_death_s_respawn_keeps_nothing_and_tells_the_missing_block_before_it() {
    let (mut edge, mut client) = standing_in_the_west().await;
    let from = died_and_let_go_east(&mut edge, &mut client).await;
    assert!(
        client.respawns.is_empty(),
        "no Respawn at the hand-over: {:?}",
        client.respawns
    );
    assert!(client.ids.contains(&ids::PLAYER_COMBAT_KILL), "the death");
    let at = within(EASTERN);
    edge.say(
        EAST,
        placed_with(Dimension::Overworld, at, respawned(0, true, None)),
    );
    edge.settle(EAST).await;
    edge.sync(&mut client).await;
    let after = ids_from(&client, from);
    assert_eq!(
        after[..3],
        [ids::PLAYER_ABILITIES, ids::GAME_EVENT, ids::RESPAWN]
    );
    let [respawn] = &client.respawns[..] else {
        panic!("one Respawn: {:?}", client.respawns);
    };
    assert_eq!(respawn.data_to_keep, 0);
    assert_eq!(respawn.spawn.dimension_name, "minecraft:overworld");
    let teleport = client.teleports.last().expect("placed");
    assert_eq!((teleport.x, teleport.y, teleport.z), (at.x, at.y, at.z));
    assert_eq!(teleport.relative_flags, 0, "absolute");
    assert_eq!(
        (
            teleport.velocity_x,
            teleport.velocity_y,
            teleport.velocity_z
        ),
        (0.0, 0.0, 0.0)
    );
    assert_eq!(
        client.chunks.keys().copied().collect::<BTreeSet<_>>(),
        view(EASTERN)
    );
    edge.end().await;
}

/// 19.9: the `Respawn` after a death says where the player died, as the game's
/// `CommonPlayerSpawnInfo` does from `ServerPlayer.getLastDeathLocation`: the dimension
/// and the block.
#[tokio::test]
async fn a_death_s_respawn_says_where_the_player_died() {
    let (mut edge, mut client) = standing_in_the_west().await;
    died_and_let_go_east(&mut edge, &mut client).await;
    edge.say(
        EAST,
        placed_with(
            Dimension::Overworld,
            within(EASTERN),
            respawned(0, true, None),
        ),
    );
    edge.settle(EAST).await;
    edge.sync(&mut client).await;
    let [respawn] = &client.respawns[..] else {
        panic!("one Respawn: {:?}", client.respawns);
    };
    assert_eq!(
        respawn.spawn.death_location,
        Some(clustine_protocol::packets::play::DeathLocation {
            dimension: "minecraft:overworld".to_owned(),
            position: clustine_protocol::codec::Position {
                x: -7,
                y: -130,
                z: 9,
            },
        })
    );
    edge.end().await;
}

/// 19.7: the event is sent only where owed; a respawn that owes none has nothing
/// between the abilities and `Respawn`.
#[tokio::test]
async fn a_respawn_that_owes_no_event_has_none_before_it() {
    let (mut edge, mut client) = standing_in_the_west().await;
    let from = died_and_let_go_east(&mut edge, &mut client).await;
    edge.say(
        EAST,
        placed_with(
            Dimension::Overworld,
            within(EASTERN),
            respawned(0, false, None),
        ),
    );
    edge.settle(EAST).await;
    edge.sync(&mut client).await;
    let after = ids_from(&client, from);
    assert_eq!(
        after[..2],
        [ids::PLAYER_ABILITIES, ids::RESPAWN],
        "{after:?}"
    );
    edge.end().await;
}

/// 19.7, the credits: `WonGame` owes the respawn; the departure by the credits' way
/// sends no `Respawn`, and the placing sends one with keep 1 (the attribute modifiers).
#[tokio::test]
async fn the_credits_respawn_keeps_the_attribute_modifiers_and_is_sent_only_at_the_placing() {
    let (mut edge, mut client) = standing_in_the_west().await;
    edge.say(
        WEST,
        Durable::WonGame {
            player: player(1),
            entity: EntityId(5),
        },
    );
    edge.say(
        WEST,
        Durable::Departed {
            player: player(1),
            transfer: respawning(
                Way::Credits,
                Dimension::Overworld,
                block_of(EASTERN),
                EASTERN,
            ),
            to: EAST,
        },
    );
    edge.settle(WEST).await;
    edge.quiet().await;
    serve(&mut edge, EAST, EASTERN);
    edge.quiet().await;
    client.drain();
    assert!(client.respawns.is_empty(), "{:?}", client.respawns);
    let from = client.ids.len();
    edge.say(
        EAST,
        placed_with(
            Dimension::Overworld,
            within(EASTERN),
            respawned(1, false, None),
        ),
    );
    edge.settle(EAST).await;
    edge.sync(&mut client).await;
    let [respawn] = &client.respawns[..] else {
        panic!("one Respawn: {:?}", client.respawns);
    };
    assert_eq!(respawn.data_to_keep, Respawn::KEEP_ATTRIBUTE_MODIFIERS);
    let after = ids_from(&client, from);
    assert_eq!(
        after[..2],
        [ids::PLAYER_ABILITIES, ids::RESPAWN],
        "{after:?}"
    );
    edge.end().await;
}

/// The End of these tests: the Nether's region stands in for it, as the edge knows a
/// region's level only by what its stays say.
const THE_END: RegionId = NETHER;

/// Where the End's portal sends player 1 in the End, which the platform is about.
const IN_THE_END: ChunkPos = ChunkPos::new(6, 0);

/// Player 1, standing in the west, goes into the End by its portal and is placed there.
async fn in_the_end() -> (Harness, Client) {
    let regions = [WEST, EAST, NORTH, NETHER];
    let mut edge = Harness::start_with(&regions, Duration::from_secs(1_000_000)).await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    serve(&mut edge, WEST, HOME);
    edge.quiet().await;
    edge.say(
        WEST,
        Durable::Departed {
            player: player(1),
            transfer: respawning(
                Way::EndPortal,
                Dimension::End,
                block_of(IN_THE_END),
                IN_THE_END,
            ),
            to: THE_END,
        },
    );
    edge.settle(WEST).await;
    edge.quiet().await;
    edge.say(
        THE_END,
        Durable::Placed {
            player: player(1),
            entity: EntityId(5),
            dimension: Dimension::End,
            pose: Pose::at(within(IN_THE_END)),
            flying: false,
            respawn: None,
            gateway: false,
        },
    );
    edge.settle(THE_END).await;
    edge.quiet().await;
    client.drain();
    let [into] = &client.respawns[..] else {
        panic!("one Respawn into the End: {:?}", client.respawns);
    };
    assert_eq!(into.spawn.dimension_name, "minecraft:the_end");
    (edge, client)
}

/// 19.7 and its review, item 2: out of the End by its portal is a respawn's departure
/// with no `Died` or `WonGame` before it: no `Respawn` at the hand-over; the placing's
/// `Respawn` keeps everything (keep 3), after the missing block's event (`SP.teleport`,
/// facts 3).
#[tokio::test]
async fn out_of_the_end_by_its_portal_is_one_respawn_keeping_everything_after_the_event() {
    let (mut edge, mut client) = in_the_end().await;
    edge.say(
        THE_END,
        Durable::Departed {
            player: player(1),
            transfer: respawning(Way::EndPortal, Dimension::Overworld, block_of(HOME), HOME),
            to: WEST,
        },
    );
    edge.settle(THE_END).await;
    edge.quiet().await;
    client.drain();
    assert_eq!(client.respawns.len(), 1, "none at the hand-over out");
    serve(&mut edge, WEST, HOME);
    edge.quiet().await;
    client.drain();
    let from = client.ids.len();
    edge.say(
        WEST,
        placed_with(Dimension::Overworld, within(HOME), respawned(3, true, None)),
    );
    edge.settle(WEST).await;
    edge.sync(&mut client).await;
    let after = ids_from(&client, from);
    assert_eq!(
        after[..3],
        [ids::PLAYER_ABILITIES, ids::GAME_EVENT, ids::RESPAWN]
    );
    let [_, out] = &client.respawns[..] else {
        panic!("two Respawns in all: {:?}", client.respawns);
    };
    assert_eq!(out.data_to_keep, Respawn::KEEP_ALL_DATA);
    assert_eq!(out.spawn.dimension_name, "minecraft:overworld");
    edge.end().await;
}

/// Facts 5.4 (PL 1057–1107): the anchor's depletion, of the blocks, at the anchor's block
/// corner, volume and pitch 1, to the player, after `Respawn` and the level's
/// information. And 19.7: a death's respawn into another dimension is one `Respawn`
/// into that dimension, none at the hand-over.
#[tokio::test]
async fn a_death_s_respawn_at_an_anchor_in_the_nether_is_one_respawn_there_and_then_its_sound() {
    let regions = [WEST, EAST, NORTH, NETHER];
    let mut edge = Harness::start_with(&regions, Duration::from_secs(1_000_000)).await;
    let mut client = edge.joined(player(1), EntityId(5)).await;
    serve(&mut edge, WEST, HOME);
    edge.quiet().await;
    edge.say(WEST, died(EntityId(5)));
    edge.say(
        WEST,
        Durable::Departed {
            player: player(1),
            transfer: respawning(Way::Respawn, Dimension::Nether, ANCHOR, IN_THE_NETHER),
            to: NETHER,
        },
    );
    edge.settle(WEST).await;
    edge.quiet().await;
    client.drain();
    assert!(client.respawns.is_empty(), "{:?}", client.respawns);
    serve(&mut edge, NETHER, IN_THE_NETHER);
    edge.quiet().await;
    client.drain();
    let from = client.ids.len();
    edge.say(
        NETHER,
        placed_with(
            Dimension::Nether,
            within(IN_THE_NETHER),
            respawned(0, false, Some(ANCHOR)),
        ),
    );
    edge.settle(NETHER).await;
    edge.sync(&mut client).await;
    let [respawn] = &client.respawns[..] else {
        panic!("one Respawn: {:?}", client.respawns);
    };
    assert_eq!(respawn.spawn.dimension_name, "minecraft:the_nether");
    assert_eq!(respawn.data_to_keep, 0);
    let sounds: Vec<&Sound> = (client.heard.iter())
        .filter_map(|heard| match heard {
            Received::Sound(sound)
                if sound.sound == clustine_data::sounds::BLOCK_RESPAWN_ANCHOR_DEPLETE =>
            {
                Some(sound)
            }
            _ => None,
        })
        .collect();
    let [sound] = &sounds[..] else {
        panic!("one depletion: {:?}", client.heard);
    };
    assert_eq!(
        (sound.x, sound.y, sound.z),
        (ANCHOR.x * 8, ANCHOR.y * 8, ANCHOR.z * 8)
    );
    assert_eq!(
        sound.source,
        clustine_protocol::packets::play::SOUND_SOURCE_BLOCKS
    );
    assert_eq!((sound.volume, sound.pitch), (1.0, 1.0));
    let after = ids_from(&client, from);
    let level_information = first(&after, ids::SET_DEFAULT_SPAWN_POSITION);
    let heard = first(&after, ids::SOUND);
    assert!(first(&after, ids::RESPAWN) < level_information);
    assert!(level_information < heard, "{after:?}");
    edge.end().await;
}

/// 19.7's review, item 2: what the regions tell the player between the death and the
/// placing (the old region's word before the departure, the new one's before the
/// `Placed`) is told after `Respawn`, in the order it came.
#[tokio::test]
async fn what_the_regions_told_meanwhile_is_told_after_the_respawn_in_order() {
    let (mut edge, mut client) = standing_in_the_west().await;
    edge.say(WEST, died(EntityId(5)));
    edge.settle(WEST).await;
    edge.tell(
        WEST,
        WorkerToEdge::ToPlayer {
            player: player(1),
            event: PlayerEvent::Health {
                health: 7.0,
                food: 20,
                saturation: 5.0,
            },
        },
    );
    edge.say(
        WEST,
        Durable::Departed {
            player: player(1),
            transfer: respawning(
                Way::Respawn,
                Dimension::Overworld,
                block_of(EASTERN),
                EASTERN,
            ),
            to: EAST,
        },
    );
    edge.settle(WEST).await;
    edge.quiet().await;
    serve(&mut edge, EAST, EASTERN);
    edge.quiet().await;
    edge.tell(
        EAST,
        WorkerToEdge::ToPlayer {
            player: player(1),
            event: PlayerEvent::Health {
                health: 20.0,
                food: 20,
                saturation: 5.0,
            },
        },
    );
    edge.settle(EAST).await;
    client.drain();
    assert!(
        !client.ids.contains(&ids::SET_HEALTH),
        "held until the placing"
    );
    let from = client.ids.len();
    edge.say(
        EAST,
        placed_with(
            Dimension::Overworld,
            within(EASTERN),
            respawned(0, false, None),
        ),
    );
    edge.settle(EAST).await;
    edge.sync(&mut client).await;
    let after = ids_from(&client, from);
    let respawn = first(&after, ids::RESPAWN);
    let healths: Vec<usize> = (after.iter().enumerate())
        .filter(|(_, id)| **id == ids::SET_HEALTH)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(healths.len(), 2, "{after:?}");
    assert!(healths.iter().all(|index| *index > respawn), "{after:?}");
    edge.end().await;
}

/// 19.6 and 19.7: `Died` goes into the outbox before the respawn's `Departed`, so a
/// resumed link replays them in that order: the client sees the death, no `Respawn` at
/// the hand-over, and one `Respawn` with keep 0 at the placing.
#[tokio::test]
async fn a_resumed_link_replays_the_death_and_the_departure_and_the_respawn_follows() {
    let (mut edge, mut client) = standing_in_the_west().await;
    edge.lose(WEST).await;
    let hello = edge.relink(WEST).await;
    assert_eq!(hello.players, [player(1)]);
    edge.tell(
        WEST,
        WorkerToEdge::Welcome(Welcome::Resumed {
            entries: 2,
            presences: 1,
            applied: 0,
        }),
    );
    edge.say(WEST, died(EntityId(5)));
    edge.say(
        WEST,
        Durable::Departed {
            player: player(1),
            transfer: respawning(
                Way::Respawn,
                Dimension::Overworld,
                block_of(EASTERN),
                EASTERN,
            ),
            to: EAST,
        },
    );
    edge.tell(
        WEST,
        WorkerToEdge::Presence {
            player: player(1),
            answer: Presence::Absent,
        },
    );
    edge.quiet().await;
    serve(&mut edge, EAST, EASTERN);
    edge.quiet().await;
    client.drain();
    assert!(client.connected());
    assert!(client.ids.contains(&ids::PLAYER_COMBAT_KILL), "the death");
    assert!(client.respawns.is_empty(), "{:?}", client.respawns);
    let from = client.ids.len();
    edge.say(
        EAST,
        placed_with(
            Dimension::Overworld,
            within(EASTERN),
            respawned(0, true, None),
        ),
    );
    edge.settle(EAST).await;
    edge.sync(&mut client).await;
    let [respawn] = &client.respawns[..] else {
        panic!("one Respawn: {:?}", client.respawns);
    };
    assert_eq!(respawn.data_to_keep, 0);
    let after = ids_from(&client, from);
    assert_eq!(
        after[..3],
        [ids::PLAYER_ABILITIES, ids::GAME_EVENT, ids::RESPAWN]
    );
    edge.end().await;
}

/// 19.6 and 19.7 on a resumed link of a region that places its own dead: `Died` and
/// then `Placed` replayed show the death and then one `Respawn` with keep 0.
#[tokio::test]
async fn a_resumed_link_replays_the_death_and_the_placing_in_order() {
    let (mut edge, mut client) = standing_in_the_west().await;
    edge.lose(WEST).await;
    edge.relink(WEST).await;
    edge.tell(
        WEST,
        WorkerToEdge::Welcome(Welcome::Resumed {
            entries: 2,
            presences: 1,
            applied: 0,
        }),
    );
    edge.say(WEST, died(EntityId(5)));
    edge.say(
        WEST,
        placed_with(
            Dimension::Overworld,
            within(HOME),
            respawned(0, false, None),
        ),
    );
    edge.tell(WEST, present(player(1), EntityId(5)));
    edge.quiet().await;
    serve(&mut edge, WEST, HOME);
    edge.sync(&mut client).await;
    let kill = first(&client.ids, ids::PLAYER_COMBAT_KILL);
    let respawn = first(&client.ids, ids::RESPAWN);
    assert!(kill < respawn, "{:?}", client.ids);
    let [respawn] = &client.respawns[..] else {
        panic!("one Respawn: {:?}", client.respawns);
    };
    assert_eq!(respawn.data_to_keep, 0);
    edge.end().await;
}

/// 19.5 item 2: each hurt broadcasts its `DamageEvent`, from which the client plays the
/// hurt and shakes its camera; `ServerLevel.broadcastDamageEvent` sends it with
/// `sendToTrackingPlayersAndSelf` (d ServerLevel 2550–2557), to the hurt player too.
#[tokio::test]
async fn a_player_is_shown_their_own_hurt() {
    let (mut edge, mut client) = standing_in_the_west().await;
    edge.tell(
        WEST,
        WorkerToEdge::TickDelta {
            tick: 3,
            events: vec![RegionEvent::DamageEvent {
                entity: EntityId(5),
                chunk: HOME,
                damage_type: 0,
                cause: None,
                direct: None,
                position: None,
            }],
        },
    );
    edge.settle(WEST).await;
    edge.sync(&mut client).await;
    let own = client.own_id();
    let hurt = (client.living.iter()).any(
        |packet| matches!(packet, ClientboundPlay::DamageEvent(hurt) if hurt.entity_id == own),
    );
    assert!(hurt, "{:?}", client.living);
    edge.end().await;
}
