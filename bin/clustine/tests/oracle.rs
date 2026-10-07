//! Comparisons against the official Minecraft server.
//!
//! These tests need Java, the server jar downloaded by `cargo datagen`, and your
//! agreement to the Minecraft EULA, so they are ignored by default. Run them with:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --test oracle -- --ignored
//! ```

mod common;

use std::collections::BTreeSet;
use std::time::Duration;

use clustine_botswarm::{Bot, Oracle};

use common::start;

/// What a joining client is told must agree with the official server, apart from the
/// numeric ids, which each server assigns through the order of its registry entries.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn join_matches_the_official_server() {
    let oracle = Oracle::start(false).await.unwrap();
    let (server, address) = start().await;

    let mut vanilla = Bot::join(oracle.address(), "Notch").await.unwrap();
    let mut clustine = Bot::join(&address, "Notch").await.unwrap();
    let (theirs, ours) = (&vanilla.info, &clustine.info);

    assert_eq!(ours.profile.uuid, theirs.profile.uuid);
    assert_eq!(ours.profile.name, theirs.profile.name);
    assert_eq!(ours.feature_flags, theirs.feature_flags);
    assert_eq!(ours.offered_packs, theirs.offered_packs);
    assert_eq!(
        theirs.entries_with_data, 0,
        "the oracle relied on the core pack"
    );
    assert_eq!(ours.entries_with_data, 0);

    // The same registries in the same order, each with the same set of entries.
    let names = |registries: &[(String, Vec<String>)]| -> Vec<String> {
        registries.iter().map(|(name, _)| name.clone()).collect()
    };
    assert_eq!(names(&ours.registries), names(&theirs.registries));
    for ((registry, our_entries), (_, their_entries)) in
        ours.registries.iter().zip(&theirs.registries)
    {
        let our_entries: BTreeSet<_> = our_entries.iter().collect();
        let their_entries: BTreeSet<_> = their_entries.iter().collect();
        assert_eq!(our_entries, their_entries, "entries of {registry}");
    }

    // The same tags with the same members.
    let our_tags = ours.tags_by_name();
    let their_tags = theirs.tags_by_name();
    assert_eq!(
        our_tags.keys().collect::<Vec<_>>(),
        their_tags.keys().collect::<Vec<_>>()
    );
    for (registry, tags) in &their_tags {
        assert_eq!(&our_tags[registry], tags, "tags of {registry}");
    }

    // The same world as far as the join packet describes it.
    let (our_login, their_login) = (&ours.login, &theirs.login);
    assert_eq!(our_login.dimension_names, their_login.dimension_names[..1]);
    assert_eq!(our_login.dimension_name, their_login.dimension_name);
    assert_eq!(our_login.game_mode, their_login.game_mode);
    assert_eq!(our_login.is_flat, their_login.is_flat);
    assert_eq!(our_login.sea_level, their_login.sea_level);
    assert_eq!(our_login.online_mode, their_login.online_mode);
    assert_eq!(our_login.hardcore, their_login.hardcore);
    let dimension_type = |info: &clustine_botswarm::JoinInfo| {
        let (_, types) = info
            .registries
            .iter()
            .find(|(name, _)| name == "minecraft:dimension_type")
            .unwrap();
        types[info.login.dimension_type as usize].clone()
    };
    assert_eq!(dimension_type(ours), dimension_type(theirs));

    // Both keep a well-behaved client connected.
    vanilla.idle(Duration::from_secs(2)).await.unwrap();
    clustine.idle(Duration::from_secs(2)).await.unwrap();

    server.stop().await;
}
