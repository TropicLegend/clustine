//! Where the search for a spawn begins, against the game's own `NoiseSpawnFinder`:
//! what `tests/java/GameSpawn.java` wrote, seed by seed.
//!
//! Nothing the program writes is in the repository; this test takes the file from
//! `CLUSTINE_GAME_SPAWN`.
//!
//! ```text
//! # With <classes> the jars `cargo datagen` unpacked (target/datagen/26.3/extract/classpath,
//! # every .jar under it):
//! javac -proc:none -cp <classes> -d <out> crates/clustine-terrain/tests/java/GameSpawn.java
//! java -cp <out>:<classes> GameSpawn 1000 <file>
//!
//! CLUSTINE_GAME_SPAWN=<file> \
//!     cargo test --release -p clustine-terrain --test game_spawn -- --ignored --nocapture
//! ```
//!
//! On 2026-10-15 every block of 1,000 seeds was the game's.

use clustine_terrain::spawn::search_origin;

#[test]
#[ignore = "needs what tests/java/GameSpawn.java wrote, in CLUSTINE_GAME_SPAWN"]
fn the_search_for_a_spawn_ends_on_the_games_block_for_every_seed() {
    let path = std::env::var("CLUSTINE_GAME_SPAWN")
        .expect("CLUSTINE_GAME_SPAWN names the file GameSpawn.java wrote");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"));
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("game-spawn 1"));

    let mut differing = Vec::new();
    let mut seeds = 0;
    for line in lines {
        let fields: Vec<&str> = line.split(' ').collect();
        let ["seed", seed, x, z] = fields[..] else {
            panic!("a line that is not a seed's: {line}");
        };
        let seed: i64 = seed.parse().unwrap();
        let game = [x.parse().unwrap(), z.parse().unwrap()];
        let ours = search_origin(seed);
        if ours != game {
            differing.push((seed, game, ours));
        }
        seeds += 1;
    }
    println!("{seeds} seeds, {} differing", differing.len());
    assert!(seeds > 0, "the file has no seeds");
    assert!(
        differing.is_empty(),
        "seeds where the block is not the game's (seed, game's, ours): {differing:?}"
    );
}
