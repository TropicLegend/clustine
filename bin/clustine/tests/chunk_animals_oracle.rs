//! Comparisons with the official server of the animals a chunk is made with (N7l,
//! `docs/groundwork/n7l-plan.md`, section 6 item 3; `docs/groundwork/n7-plan.md`,
//! section 7.10): a bot visits the same places of the same seed on both servers, and
//! what it is first shown of every entity there is compared, entity by entity.
//!
//! The official server is frozen (`tick freeze`) before the bot joins, so its animals
//! are shown as generation made them. Clustine's are not frozen and may have begun to
//! move before they are shown, so the two are paired by type and the nearest place
//! within half a block; then the data sent with the spawn (colours, variants, the baby
//! flag, a horse's or a llama's markings and strength, a fox's or a panda's genes, a
//! goat's horns and screaming, the left hand in the mob's flags, a health that is not
//! the default), the attributes (bases and modifiers to the bit: a horse's health,
//! speed and jump, the follow range's random bonus) and what each holds (a fox's item)
//! are compared exactly, and the yaw and pitch exactly where the two stand at the same
//! place to the bit. The UUIDs and the head's yaw are not compared: the first comes
//! from each server's own random, and a mob turns its head as it looks about from its
//! first tick. Both randoms that decide the rest are functions of the seed and the
//! chunk (`docs/groundwork/n7l-plan.md`, section 1.1), so all of it is the game's.
//!
//! The bot is put above the place, at a height no mob looks up to, rather than at the
//! spawn: the official server ticks the chunks about its spawn for a moment before it
//! can be frozen, while a place nobody has been is made under the freeze; and places
//! can be picked for their animals. They were picked by running Clustine's own
//! generator over a grid of each seed (on 2026-10-26), for the most kinds in an area
//! of 11 by 11 chunks; eleven kinds among the six: armadillo, chicken, cow, donkey,
//! fox, goat, parrot, pig, rabbit, sheep and wolf.
//!
//! **What a mismatch prints**, for each place: the census of each server (how many of
//! each type), how many entities were paired and how many of them stood in place, a
//! line `moved:` for each pair Clustine's had moved within the half block (no failure),
//! and a line `DIFFERS:` for each failure: an entity of the official server's with
//! none of Clustine's near it ("Clustine shows none there"), one of Clustine's with
//! none of the official server's ("the official server shows none there"), or a pair
//! whose data, attributes, equipment or angles differ, with both values. An entity
//! within a block of the area's edge with no partner is not counted, since it may have
//! walked across the edge.
//!
//! They need Java, the server jar downloaded by `cargo datagen` (also for Clustine:
//! its structures are built of the jar's templates, as the official server's are), and
//! your agreement to the Minecraft EULA, so they are ignored by default. Run them on a
//! quiet machine:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test chunk_animals_oracle -- --ignored --nocapture --test-threads 1
//! ```

mod common;

use clustine_botswarm::{Oracle, OracleWorld};

use common::shown::{
    FirstShown, VISIT_VIEW, census, compare, start_seeded, visit_clustine, visit_official,
};

/// How far the compared area reaches from its centre, in chunks: 11 by 11 chunks, all
/// within the 128 blocks at which both servers show an animal to a bot above the
/// centre with a view of 8.
const REACH: i32 = 5;

/// How far Clustine's animal may have moved from where generation put it.
const TOLERANCE: f64 = 0.5;

/// A place: the chunk at the centre of the area, and what Clustine's generator made
/// there when it was picked.
type Place = ((i32, i32), &'static str);

const SEED_13579: [Place; 3] = [
    (
        (-80, -80),
        "chickens, a cow, donkeys, a parrot, rabbits and sheep",
    ),
    (
        (-40, -40),
        "chickens, cows, a parrot, pigs, a sheep and wolves",
    ),
    ((80, -80), "armadillos, chickens, a cow, pigs and sheep"),
];

const SEED_12345: [Place; 3] = [
    ((80, -40), "chickens, cows, foxes, pigs, rabbits and sheep"),
    ((-40, 0), "foxes, goats, rabbits and wolves"),
    ((-80, 40), "chickens, a cow, parrots and sheep"),
];

/// Whether `entity` stands within a block of the edge of the area about `centre`.
fn near_the_edge(entity: &FirstShown, centre: (i32, i32)) -> bool {
    let low = |c: i32| f64::from((c - REACH) * 16);
    let high = |c: i32| f64::from((c + REACH + 1) * 16);
    let (x, z) = (entity.position.0, entity.position.2);
    x - low(centre.0) < 1.0
        || high(centre.0) - x < 1.0
        || z - low(centre.1) < 1.0
        || high(centre.1) - z < 1.0
}

/// Visits `places` of `seed` on both servers and compares what each showed; panics
/// with every place's report if any differs.
async fn compare_seed(seed: i64, places: &[Place]) {
    let oracle = Oracle::start_with(false, &OracleWorld::frozen_seed(seed, VISIT_VIEW))
        .await
        .unwrap();
    let mut theirs = Vec::new();
    for (number, (centre, _)) in places.iter().enumerate() {
        let name = format!("Visitor{number}");
        theirs.push(
            visit_official(&oracle, &name, *centre, REACH)
                .await
                .unwrap_or_else(|error| panic!("the official server at {centre:?}: {error:#}")),
        );
    }
    drop(oracle);
    let (server, address) = start_seeded(seed).await.unwrap();
    let mut ours = Vec::new();
    for (number, (centre, _)) in places.iter().enumerate() {
        let name = format!("Visitor{number}");
        ours.push(
            visit_clustine(&address, &name, *centre, REACH)
                .await
                .unwrap_or_else(|error| panic!("Clustine at {centre:?}: {error:#}")),
        );
    }
    server.stop().await;

    let mut failed = Vec::new();
    for (((centre, picked), theirs), ours) in places.iter().zip(&theirs).zip(&ours) {
        let compared = compare(
            theirs,
            ours,
            TOLERANCE,
            |_| Vec::new(),
            |entity| near_the_edge(entity, *centre),
        );
        let title =
            format!("seed {seed}, chunks within {REACH} of {centre:?} ({picked} when picked)");
        println!(
            "{}  official: {:?}\n  Clustine: {:?}",
            compared.report(&title),
            census(theirs),
            census(ours)
        );
        // A place where the official server shows nothing judges nothing: the place or
        // the freeze is wrong, not Clustine.
        if theirs.len() < 4 {
            failed.push(format!(
                "{title}: the official server showed {} entities; pick another place",
                theirs.len()
            ));
        }
        if !compared.differences.is_empty() {
            failed.push(format!(
                "{title}: {} differences",
                compared.differences.len()
            ));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn the_animals_chunks_are_made_with_on_seed_13579_are_shown_like_on_the_official_server() {
    compare_seed(13579, &SEED_13579).await;
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn the_animals_chunks_are_made_with_on_seed_12345_are_shown_like_on_the_official_server() {
    compare_seed(12345, &SEED_12345).await;
}
