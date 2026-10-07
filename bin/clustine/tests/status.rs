//! The server list ping and the pre-login behaviour, end to end over TCP.

mod common;

use clustine_botswarm::{Connection, intention};
use clustine_data::{GAME_VERSION, PROTOCOL_VERSION};
use clustine_protocol::packets::handshake::Intent;
use clustine_protocol::packets::login::ClientboundLogin;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use common::start;

#[tokio::test]
async fn ping_reports_version_and_settings() {
    let (server, address) = start().await;

    let status = clustine_botswarm::ping(&address).await.unwrap();
    assert_eq!(
        status.json,
        json!({
            "version": { "name": GAME_VERSION, "protocol": PROTOCOL_VERSION },
            "players": { "max": 7, "online": 0 },
            "description": { "text": "integration test" },
            "enforcesSecureChat": false,
        })
    );

    server.stop().await;
}

#[tokio::test]
async fn many_pings_in_parallel() {
    let (server, address) = start().await;

    let pings = (0..50).map(|_| {
        let address = address.clone();
        tokio::spawn(async move { clustine_botswarm::ping(&address).await })
    });
    for ping in pings.collect::<Vec<_>>() {
        ping.await.unwrap().unwrap();
    }

    server.stop().await;
}

#[tokio::test]
async fn outdated_client_is_told_the_server_version() {
    let (server, address) = start().await;

    let mut connection = Connection::connect(&address).await.unwrap();
    let mut handshake = intention(&address, Intent::Login).unwrap();
    handshake.protocol_version -= 1;
    connection.write(&handshake).await.unwrap();
    let frame = connection.read_frame().await.unwrap();
    let ClientboundLogin::LoginDisconnect(disconnect) = ClientboundLogin::decode(&frame).unwrap()
    else {
        panic!("expected a login disconnect");
    };
    assert!(disconnect.reason_json.contains(GAME_VERSION));

    server.stop().await;
}

/// The pre-1.7 ping is not a framed packet; the server must hang up instead of waiting
/// for the rest of a packet that never comes.
#[tokio::test]
async fn legacy_ping_is_closed_immediately() {
    let (server, address) = start().await;

    let mut stream = TcpStream::connect(&address).await.unwrap();
    stream.write_all(&[0xFE, 0x01]).await.unwrap();
    let mut received = Vec::new();
    let read = stream.read_to_end(&mut received).await;
    // Either a clean close or a reset is fine; data or a hang is not.
    assert!(matches!(read, Ok(0) | Err(_)), "{read:?}");
    assert!(received.is_empty());

    server.stop().await;
}

#[tokio::test]
async fn garbage_does_not_take_the_server_down() {
    let (server, address) = start().await;

    let mut stream = TcpStream::connect(&address).await.unwrap();
    stream
        .write_all(&[0x05, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF])
        .await
        .unwrap();
    let mut received = Vec::new();
    let _ = stream.read_to_end(&mut received).await;
    assert!(received.is_empty());

    clustine_botswarm::ping(&address).await.unwrap();
    server.stop().await;
}

#[tokio::test]
async fn stopping_closes_open_connections() {
    let (server, address) = start().await;

    let mut idle = TcpStream::connect(&address).await.unwrap();
    // Make sure the server has accepted the connection before stopping it.
    clustine_botswarm::ping(&address).await.unwrap();
    server.stop().await;

    let mut received = Vec::new();
    let read = idle.read_to_end(&mut received).await;
    assert!(matches!(read, Ok(0) | Err(_)), "{read:?}");
    assert!(TcpStream::connect(&address).await.is_err());
}

#[tokio::test]
async fn ping_counts_the_players_in_the_world() {
    let (server, address) = start().await;
    let online = |status: &clustine_botswarm::Status| status.json["players"]["online"].clone();

    let first = clustine_botswarm::Bot::join(&address, "First")
        .await
        .unwrap();
    let _second = clustine_botswarm::Bot::join(&address, "Second")
        .await
        .unwrap();
    let status = clustine_botswarm::ping(&address).await.unwrap();
    assert_eq!(online(&status), 2);

    drop(first);
    // The server notices the closed connection a moment later.
    let mut count = online(&status);
    for _ in 0..100 {
        count = online(&clustine_botswarm::ping(&address).await.unwrap());
        if count == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(count, 1);

    server.stop().await;
}

/// A connection that does not get on with it is closed: whether it says nothing at
/// all, stops after the handshake, or keeps sending a little now and then.
#[tokio::test]
async fn clients_that_dawdle_before_playing_are_dropped() {
    use std::time::{Duration, Instant};

    use clustine::Config;
    use clustine_protocol::packets::login::LoginStart;

    let limit = Duration::from_millis(400);
    let (server, address) = common::start_with(Config {
        client_timeout: limit,
        ..common::config()
    })
    .await;
    let closed_within = |started: Instant| {
        let elapsed = started.elapsed();
        assert!(
            elapsed >= limit - Duration::from_millis(50) && elapsed < limit * 4,
            "closed after {elapsed:?}"
        );
    };

    // Silence.
    let started = Instant::now();
    let mut silent = TcpStream::connect(&address).await.unwrap();
    let mut received = Vec::new();
    let _ = silent.read_to_end(&mut received).await;
    assert!(received.is_empty());
    closed_within(started);

    // A handshake and then nothing.
    let started = Instant::now();
    let mut connection = Connection::connect(&address).await.unwrap();
    let handshake = intention(&address, Intent::Login).unwrap();
    connection.write(&handshake).await.unwrap();
    assert!(connection.read_frame().await.is_err());
    closed_within(started);

    // Always something just before each wait would run out, but never done.
    let started = Instant::now();
    let mut connection = Connection::connect(&address).await.unwrap();
    tokio::time::sleep(limit / 2).await;
    connection.write(&handshake).await.unwrap();
    tokio::time::sleep(limit / 2).await;
    let login = LoginStart {
        name: "Dawdler".to_owned(),
        uuid: uuid::Uuid::nil(),
    };
    let _ = connection.write(&login).await;
    // Whatever the server sent in answer, it then hangs up.
    while connection.read_frame().await.is_ok() {}
    closed_within(started);

    // Someone who gets on with it is not affected by the short limit.
    let mut bot = clustine_botswarm::Bot::join(&address, "Brisk")
        .await
        .unwrap();
    bot.idle(limit * 3).await.unwrap();

    server.stop().await;
}
