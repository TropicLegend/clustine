//! Tests of what the edge sends a client of its survival (step V4), written from the
//! specification by someone who did not write the edge's handling of it:
//! `docs/groundwork/v-plan.md`, sections 2.2 and 2.4, the build design's sections 1, 7
//! and 8, and the game's classes. A region's [`PlayerEvent::Health`] is the game's
//! `SetHealth` with the food and the saturation (`ServerPlayer.doTick @146-264`);
//! `Knocked` is the `SetEntityMotion` the game's tracker sends a player of their own
//! entity after `markHurt` (`ServerEntity.sendChanges @960-993`) or that
//! `Player.causeExtraKnockback` sends at once; `OwnData` is the player's own entity data
//! that only the server sets (`Entity.DATA_SHARED_FLAGS_ID` 0 with the burning bit 1,
//! `DATA_AIR_SUPPLY_ID` 1, `DATA_TICKS_FROZEN` 7, `LivingEntity.DATA_ARROW_COUNT_ID` 12,
//! `Player.DATA_PLAYER_ABSORPTION_ID` 17: the order of `defineId` down the classes,
//! `Avatar` defining 15 and 16); `HurtAnimation` is `ClientboundHurtAnimationPacket` of the
//! player's own entity (`ServerPlayer.indicateDamage`); `ArrowHitPlayer` is the game event
//! 6 (`ClientboundGameEventPacket.PLAY_ARROW_HIT_SOUND`, `AbstractArrow.onHitEntity`);
//! and a death is `PlayerCombatKill` with the message `death.attack.<id>` naming the
//! player and then the killer (`CombatTracker.getDeathMessage`).

use clustine_protocol::packets::play::{GameEvent, HurtAnimation, PlayerCombatKill, SetHealth};
use clustine_sim::api::{DeathMessage, Killer, OwnData};

use super::*;

fn to(event: PlayerEvent) -> WorkerToEdge {
    WorkerToEdge::ToPlayer {
        player: player(1),
        event,
    }
}

/// The wire id the client knows its own entity by: the login's.
fn own_wire_id(client: &Client) -> i32 {
    client
        .logins
        .first()
        .expect("the client logged in")
        .entity_id
}

/// Tells the client `event` from player 1's region, and takes what the edge sends.
async fn told(edge: &mut Harness, client: &mut Client, event: PlayerEvent) {
    edge.tell(WEST, to(event));
    edge.settle(WEST).await;
    edge.sync(client).await;
}

fn healths(client: &Client) -> Vec<SetHealth> {
    client
        .others
        .iter()
        .filter_map(|packet| match packet {
            ClientboundPlay::SetHealth(health) => Some(*health),
            _ => None,
        })
        .collect()
}

fn own_data_sent(client: &Client) -> Vec<SetEntityData> {
    let own = own_wire_id(client);
    client
        .entity_data
        .iter()
        .filter(|data| data.entity_id == own)
        .cloned()
        .collect()
}

/// The value at `index` in `data`.
fn value_at(data: &SetEntityData, index: u8) -> Option<EntityDataValue> {
    data.values
        .iter()
        .find(|(at, _)| *at == index)
        .map(|(_, value)| value.clone())
}

/// `ServerPlayer.doTick @146-264` sends `ClientboundSetHealthPacket(health, food,
/// saturation)` as they are: the region's health, food and saturation reach the client
/// whole.
#[tokio::test]
async fn health_is_sent_with_the_food_and_the_saturation() {
    let (mut edge, mut client) = standing_in_the_west().await;
    told(
        &mut edge,
        &mut client,
        PlayerEvent::Health {
            health: 13.5,
            food: 17,
            saturation: 2.25,
        },
    )
    .await;
    let sent = healths(&client);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].health, 13.5);
    assert_eq!(sent[0].food, 17);
    assert_eq!(sent[0].saturation, 2.25);
    edge.end().await;
}

/// `markHurt` (`ServerEntity.sendChanges @960-993`, `sendToTrackingPlayersAndSelf`): the
/// player's client is told the motion of its own entity, by the id it knows itself by.
#[tokio::test]
async fn a_knock_is_sent_as_the_motion_of_the_player_s_own_entity() {
    let (mut edge, mut client) = standing_in_the_west().await;
    let motion = [0.25, 0.4, -0.3];
    told(&mut edge, &mut client, PlayerEvent::Knocked { motion }).await;
    let own = own_wire_id(&client);
    let sent: Vec<[f64; 3]> = client
        .tracking
        .iter()
        .filter_map(|packet| match packet {
            ClientboundPlay::SetEntityMotion(sent) if sent.entity_id == own => Some(sent.motion),
            _ => None,
        })
        .collect();
    assert_eq!(sent.len(), 1, "{:?}", client.tracking);
    for (sent, wanted) in sent[0].iter().zip(motion) {
        // The motion goes as `Vec3.LP_STREAM_CODEC`'s low precision.
        assert!((sent - wanted).abs() < 0.01, "{:?} for {motion:?}", sent);
    }
    edge.end().await;
}

/// The player's own entity data that only the server sets: air at 1, ticks frozen at 7,
/// arrows at 12 (`VarInt`s), absorption at 17 (a float), and the burning bit (1) of the
/// shared flags at 0. Sent to the player's own client, of its own entity.
#[tokio::test]
async fn own_data_is_sent_as_the_player_s_own_entity_data() {
    let (mut edge, mut client) = standing_in_the_west().await;
    told(
        &mut edge,
        &mut client,
        PlayerEvent::OwnData(OwnData {
            air: 120,
            absorption: 4.0,
            on_fire: true,
            frozen: 37,
            arrows: 2,
        }),
    )
    .await;
    let sent = own_data_sent(&client);
    let last = sent.last().expect("the player's own data is sent");
    assert_eq!(value_at(last, 1), Some(EntityDataValue::Int(120)), "air");
    assert_eq!(value_at(last, 7), Some(EntityDataValue::Int(37)), "frozen");
    assert_eq!(value_at(last, 12), Some(EntityDataValue::Int(2)), "arrows");
    assert_eq!(
        value_at(last, 17),
        Some(EntityDataValue::Float(4.0)),
        "absorption"
    );
    match value_at(last, 0) {
        Some(EntityDataValue::Byte(flags)) => assert_eq!(flags & 1, 1, "burning"),
        other => panic!("the shared flags: {other:?}"),
    }

    // Put out: the burning bit is clear.
    told(
        &mut edge,
        &mut client,
        PlayerEvent::OwnData(OwnData {
            air: 300,
            absorption: 0.0,
            on_fire: false,
            frozen: 0,
            arrows: 0,
        }),
    )
    .await;
    let sent = own_data_sent(&client);
    let last = sent.last().expect("sent again");
    match value_at(last, 0) {
        Some(EntityDataValue::Byte(flags)) => assert_eq!(flags & 1, 0, "not burning"),
        other => panic!("the shared flags: {other:?}"),
    }
    assert_eq!(value_at(last, 1), Some(EntityDataValue::Int(300)));
    edge.end().await;
}

/// `ServerPlayer.indicateDamage`: `ClientboundHurtAnimationPacket(id, yaw)` of the player's
/// own entity, to them.
#[tokio::test]
async fn a_hurt_s_shake_is_sent_as_the_hurt_animation_of_the_player_s_own_entity() {
    let (mut edge, mut client) = standing_in_the_west().await;
    told(
        &mut edge,
        &mut client,
        PlayerEvent::HurtAnimation { yaw: 135.0 },
    )
    .await;
    let own = own_wire_id(&client);
    let sent: Vec<HurtAnimation> = client
        .others
        .iter()
        .filter_map(|packet| match packet {
            ClientboundPlay::HurtAnimation(sent) => Some(sent.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(sent.len(), 1, "{:?}", client.others);
    assert_eq!(sent[0].entity_id, own);
    assert_eq!(sent[0].yaw, 135.0);
    edge.end().await;
}

/// `AbstractArrow.onHitEntity @377-428`: the shooter's client is sent the game event 6
/// (`PLAY_ARROW_HIT_SOUND`) with the value 0.
#[tokio::test]
async fn an_arrow_s_hit_on_a_player_is_game_event_six_to_its_shooter() {
    let (mut edge, mut client) = standing_in_the_west().await;
    // The join's game events (the wait for the level's chunks) are before.
    let before = client.others.len();
    told(&mut edge, &mut client, PlayerEvent::ArrowHitPlayer).await;
    let sent: Vec<GameEvent> = client.others[before..]
        .iter()
        .filter_map(|packet| match packet {
            ClientboundPlay::GameEvent(sent) => Some(sent.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(sent.len(), 1, "{:?}", &client.others[before..]);
    assert_eq!(sent[0].event, 6);
    assert_eq!(sent[0].value, 0.0);
    edge.end().await;
}

/// The `with` list of a death's message, and its key.
fn message_of(kill: &PlayerCombatKill) -> (String, Vec<Nbt>) {
    let Nbt::Compound(fields) = &kill.message else {
        panic!("a compound: {:?}", kill.message);
    };
    let key = fields
        .iter()
        .find_map(|(name, value)| match (name.as_str(), value) {
            ("translate", Nbt::String(key)) => Some(key.clone()),
            _ => None,
        })
        .expect("a translated message");
    let with = fields
        .iter()
        .find_map(|(name, value)| match (name.as_str(), value) {
            ("with", Nbt::List(with)) => Some(with.clone()),
            _ => None,
        })
        .unwrap_or_default();
    (key, with)
}

/// Whether `component` says `text` or translates `key`.
fn says(component: &Nbt, field: &str, wanted: &str) -> bool {
    match component {
        Nbt::Compound(fields) => fields.iter().any(|(name, value)| {
            name == field && matches!(value, Nbt::String(said) if said == wanted)
        }),
        Nbt::String(said) => field == "text" && said == wanted,
        _ => false,
    }
}

/// `CombatTracker.getDeathMessage`: `death.attack.mob` with the dead player and then the
/// killer, a zombie with no name of its own shown by its type's name
/// (`entity.minecraft.zombie`); a player killer by their name (`death.attack.player`).
/// Sent as `PlayerCombatKill` of the player's own entity.
#[tokio::test]
async fn a_death_s_message_names_the_player_and_then_their_killer() {
    for (key, killer, shown) in [
        (
            "death.attack.mob",
            Killer {
                entity: EntityId(44),
                kind: u16::try_from(clustine_data::entity_types::ZOMBIE).expect("fits"),
                name: None,
                player: false,
                weapon: None,
            },
            ("translate", "entity.minecraft.zombie"),
        ),
        (
            "death.attack.player",
            Killer {
                entity: EntityId(45),
                kind: u16::try_from(clustine_data::entity_types::PLAYER).expect("fits"),
                name: Some("Steve".to_owned()),
                player: true,
                weapon: None,
            },
            ("text", "Steve"),
        ),
    ] {
        let (mut edge, mut client) = standing_in_the_west().await;
        edge.say(
            WEST,
            Durable::Died {
                player: player(1),
                entity: EntityId(5),
                message: DeathMessage {
                    key: key.to_owned(),
                    killer: Some(killer),
                    item: None,
                },
                at: (Dimension::Overworld, BlockPos::new(0, -60, 0)),
            },
        );
        edge.settle(WEST).await;
        edge.sync(&mut client).await;
        let kills: Vec<PlayerCombatKill> = client
            .others
            .iter()
            .filter_map(|packet| match packet {
                ClientboundPlay::PlayerCombatKill(kill) => Some(kill.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(kills.len(), 1, "{:?}", client.others);
        assert_eq!(kills[0].player_id, own_wire_id(&client));
        let (told_key, with) = message_of(&kills[0]);
        assert_eq!(told_key, key);
        assert_eq!(with.len(), 2, "the player and the killer: {with:?}");
        assert!(
            says(&with[0], "text", PLAYER_NAME),
            "the player first: {:?}",
            with[0]
        );
        assert!(
            says(&with[1], shown.0, shown.1),
            "the killer second: {:?}",
            with[1]
        );
        edge.end().await;
    }
}

/// The name the harness's player 1 joined with: its first session's.
const PLAYER_NAME: &str = "Player1";

/// `ServerPlayer.resetSentInfo` and the first `doTick @146-264`: the game tells a joining
/// player their health, food and saturation at once. The region says it in the tick it
/// places them, right after `Spawned`; the client is sent it after its login.
#[tokio::test]
async fn the_health_told_with_a_join_reaches_the_client_after_its_login() {
    let mut edge = Harness::start().await;
    let mut client = edge.join_asking(player(1), None).await;
    let (_, join) = edge.next_numbered(WEST).await;
    assert!(matches!(join, EdgeToWorker::PlayerJoin(_)), "{join:?}");
    edge.tell(WEST, edge.spawned(player(1), EntityId(5)));
    edge.tell(
        WEST,
        to(PlayerEvent::Health {
            health: 7.0,
            food: 3,
            saturation: 0.0,
        }),
    );
    edge.settle(WEST).await;
    for chunk in view(HOME) {
        let ask = edge.ask(WEST, chunk);
        edge.tell(WEST, snapshot(chunk, ask));
    }
    edge.quiet().await;
    edge.sync(&mut client).await;
    assert_eq!(client.logins.len(), 1, "logged in");
    let sent = healths(&client);
    assert_eq!(sent.len(), 1, "{:?}", client.ids);
    assert_eq!(
        (sent[0].health, sent[0].food, sent[0].saturation),
        (7.0, 3, 0.0)
    );
    edge.end().await;
}
