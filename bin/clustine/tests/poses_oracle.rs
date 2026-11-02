//! The comparison with the official server of what other players are shown of a
//! player's skin's parts, main hand, sneaking and gliding (`common/poses.rs`). Ignored,
//! as every comparison: it needs Java, the jar and agreement to the Minecraft EULA.
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test poses_oracle -- --ignored --nocapture --test-threads 1
//! ```

mod common;

use clustine_botswarm::Oracle;

use common::start;

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_player_s_poses_are_shown_as_on_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let theirs = common::poses::run(oracle.address()).await;
    drop(oracle);
    let (server, address) = start().await;
    let ours = common::poses::run(&address).await;
    server.stop().await;
    println!("official: {theirs:#?}\nClustine: {ours:#?}");
    assert_eq!(theirs, ours);
}
