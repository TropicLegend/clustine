//! The server list ping and the pre-login behaviour, end to end over TCP.

use clustine::{Config, Server};
use clustine_botswarm::{Connection, intention};
use clustine_data::{GAME_VERSION, PROTOCOL_VERSION};
use clustine_protocol::packets::handshake::Intent;
use clustine_protocol::packets::login::ClientboundLogin;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

async fn start() -> (Server, String) {
    let server = Server::start(Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        description: "integration test".to_owned(),
        max_players: 7,
    })
    .await
    .unwrap();
    let address = server.address().to_string();
    (server, address)
}

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
async fn login_is_refused_with_a_message() {
    let (server, address) = start().await;

    let mut connection = Connection::connect(&address).await.unwrap();
    let handshake = intention(&address, Intent::Login).unwrap();
    connection.write(&handshake).await.unwrap();
    let frame = connection.read_frame().await.unwrap();
    let ClientboundLogin::LoginDisconnect(disconnect) = ClientboundLogin::decode(&frame).unwrap()
    else {
        panic!("expected a login disconnect");
    };
    let reason: serde_json::Value = serde_json::from_str(&disconnect.reason_json).unwrap();
    assert!(reason["text"].as_str().unwrap().contains("not accept"));

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
