//! Another player's pose and appearance: their skin's parts and main hand, sneaking,
//! and gliding with an elytra, shown to a player who watches them and to one who comes
//! to see them while they sneak (`common/poses.rs`; `poses_oracle.rs` compares the scene
//! with the official server).

mod common;

use common::start;

#[tokio::test]
async fn another_player_sees_a_player_s_skin_hand_sneak_and_glide() {
    let (server, address) = start().await;
    let seen = common::poses::run(&address).await;
    server.stop().await;
    assert_eq!(seen, common::poses::expected());
}
