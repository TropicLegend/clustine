//! Comparisons with the official server of the entities structures are made with (N7l's
//! part D3, `docs/groundwork/n7l-plan.md`, sections 1.2 and 6 item 3;
//! `docs/groundwork/n7-plan.md`, section 7.10): a bot visits a village, an igloo, a
//! swamp hut, an ocean monument and a pillager outpost of seed 13579 on both servers,
//! and what it is first shown of every entity about each is compared, entity by entity,
//! as `chunk_animals_oracle.rs` compares a chunk's animals (`common/shown.rs`).
//!
//! The places are the starts that `crates/clustine-structures/tests/starts.rs` holds
//! for the seed, as the game's own classes made them. What Clustine's generator makes
//! there with the structures' blocks placed (run on 2026-10-26, with the judge's
//! settings): the plains village at chunk (58, -27) its five villagers, an iron golem,
//! two cats and its pens' chickens, pigs, sheep and a cow, besides a mineshaft's chest
//! minecart and a camp's cushion in the area; the igloo at chunk (-154, -154) its
//! basement's villager and zombie villager; the swamp hut at chunk (54, 364) its witch
//! and black cat; the monument at chunk (106, -114) its three elder guardians; the
//! outpost at chunk (-138, 83) six allays in their cages, and no caged iron golem, as
//! this outpost has no cage of one. Each place's check that the official server shows
//! its structure's own kinds (a villager, an elder guardian...) is a check of the
//! place, not of Clustine.
//!
//! **Not yet a comparison Clustine can pass**: a server's world places no structure's
//! blocks and so makes none of their entities until phase S's switch (S3,
//! `docs/groundwork/s-plan.md`) gives `FeatureStages` the starts
//! (`FeatureSettings::with_structures`, which only the judge calls today). Until then
//! each structure's entities print as "Clustine shows none there".
//!
//! The official server is frozen before the bot joins, so the villagers and the rest
//! are shown where the templates put them; Clustine's are not, and are paired within
//! a block, since a villager may have taken a step before it is shown. The data sent
//! with the spawn (the villager's type, profession and level, a baby's flag, a cat's
//! variant and collar, a horse's markings), the attributes and what each holds (the
//! armour stands' helmets, an allay's empty hands) are compared exactly, the yaw and
//! pitch where the two stand in place; not the UUIDs nor the heads (see
//! `chunk_animals_oracle.rs`). What a mismatch prints is as there: the census of each
//! server, `moved:` lines, and a `DIFFERS:` line for each entity without a partner or
//! a pair that differs, with both values.
//!
//! The region random's state after a village piece (`docs/groundwork/n7-plan.md`,
//! section 7.10) is not seen by a client but in the blocks a run places after the
//! piece, which the comparisons of the land judge; it is left to the judge in the game's
//! classes (`docs/groundwork/n7l-plan.md`, section 6 item 1).
//!
//! They need Java, the server jar downloaded by `cargo datagen` and your agreement to
//! the Minecraft EULA, so they are ignored by default. Run them on a quiet machine:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test structure_entities_oracle -- --ignored --nocapture --test-threads 1
//! ```

mod common;

use clustine_botswarm::{Oracle, OracleWorld};

use common::shown::{
    FirstShown, VISIT_VIEW, census, compare, start_seeded, visit_clustine, visit_official,
};

const SEED: i64 = 13579;

/// How far Clustine's entity may have moved from where the structure put it, in each
/// direction: a villager's step, a guardian's stroke.
const TOLERANCE: f64 = 1.0;

/// A structure visited: what it is, the chunk at the centre of the area compared, how
/// many chunks the area reaches from it (the start's box, from `starts.rs`, within it),
/// and the kinds the official server has to show there for the place to be the one
/// meant.
struct Visited {
    structure: &'static str,
    centre: (i32, i32),
    reach: i32,
    own: &'static [&'static str],
}

const VISITED: [Visited; 5] = [
    Visited {
        // The box x 872 to 992, z -520 to -365: chunks 54 to 62 and -33 to -23.
        structure: "the plains village",
        centre: (58, -28),
        reach: 5,
        own: &[
            "minecraft:villager",
            "minecraft:iron_golem",
            "minecraft:cat",
        ],
    },
    Visited {
        // The box x -2464 to -2458, z -2461 to -2452, with its basement.
        structure: "the igloo",
        centre: (-154, -154),
        reach: 1,
        own: &["minecraft:villager", "minecraft:zombie_villager"],
    },
    Visited {
        // The box x 864 to 872, z 5824 to 5830.
        structure: "the swamp hut",
        centre: (54, 364),
        reach: 1,
        own: &["minecraft:witch", "minecraft:cat"],
    },
    Visited {
        // The box x 1667 to 1724, z -1853 to -1796: chunks 104 to 107 and -116 to -113.
        structure: "the ocean monument",
        centre: (105, -115),
        reach: 3,
        own: &["minecraft:elder_guardian"],
    },
    Visited {
        // The box x -2251 to -2180, z 1285 to 1356: chunks -141 to -137 and 80 to 84.
        structure: "the pillager outpost",
        centre: (-139, 82),
        reach: 3,
        own: &["minecraft:allay"],
    },
];

/// Whether `entity` stands within a block of the edge of `visited`'s area.
fn near_the_edge(entity: &FirstShown, visited: &Visited) -> bool {
    let (centre, reach) = (visited.centre, visited.reach);
    let low = |c: i32| f64::from((c - reach) * 16);
    let high = |c: i32| f64::from((c + reach + 1) * 16);
    let (x, z) = (entity.position.0, entity.position.2);
    x - low(centre.0) < TOLERANCE
        || high(centre.0) - x < TOLERANCE
        || z - low(centre.1) < TOLERANCE
        || high(centre.1) - z < TOLERANCE
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn the_entities_structures_are_made_with_are_shown_like_on_the_official_server() {
    let oracle = Oracle::start_with(false, &OracleWorld::frozen_seed(SEED, VISIT_VIEW))
        .await
        .unwrap();
    let mut theirs = Vec::new();
    for (number, visited) in VISITED.iter().enumerate() {
        let name = format!("Visitor{number}");
        theirs.push(
            visit_official(&oracle, &name, visited.centre, visited.reach)
                .await
                .unwrap_or_else(|error| {
                    panic!("the official server at {}: {error:#}", visited.structure)
                }),
        );
    }
    drop(oracle);
    let (server, address) = start_seeded(SEED).await.unwrap();
    let mut ours = Vec::new();
    for (number, visited) in VISITED.iter().enumerate() {
        let name = format!("Visitor{number}");
        ours.push(
            visit_clustine(&address, &name, visited.centre, visited.reach)
                .await
                .unwrap_or_else(|error| panic!("Clustine at {}: {error:#}", visited.structure)),
        );
    }
    server.stop().await;

    let mut failed = Vec::new();
    for ((visited, theirs), ours) in VISITED.iter().zip(&theirs).zip(&ours) {
        let compared = compare(
            theirs,
            ours,
            TOLERANCE,
            |_| Vec::new(),
            |entity| near_the_edge(entity, visited),
        );
        let title = format!(
            "{} of seed {SEED}, chunks within {} of {:?}",
            visited.structure, visited.reach, visited.centre
        );
        let official = census(theirs);
        println!(
            "{}  official: {official:?}\n  Clustine: {:?}",
            compared.report(&title),
            census(ours)
        );
        let absent: Vec<&str> = visited
            .own
            .iter()
            .copied()
            .filter(|kind| !official.contains_key(kind))
            .collect();
        if !absent.is_empty() {
            failed.push(format!(
                "{title}: the official server shows no {absent:?}; the place is not the one meant"
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
