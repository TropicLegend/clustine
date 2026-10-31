//! The strongholds a world keeps for its eyes of ender (`docs/groundwork/p-plan.md`,
//! section 19.2 P10-0): the generator names the chunks of the `minecraft:strongholds`
//! set's concentric rings, in the overworld of a world with structures only.

use clustine_world::{ChunkGenerator, ChunkPos, Dimension};
use clustine_worldgen::TerrainGenerator;

/// The set's 128 strongholds, the first ring's three about 128 chunks from the origin
/// (`4 × distance` of 32, give or take `1.25 × distance`, and the biome a stronghold
/// prefers within 112 blocks), each in a ring further out than the last's mean, and
/// the same for the same seed.
#[test]
fn a_seeded_overworld_names_its_128_strongholds_on_their_rings_and_the_others_none() {
    let overworld = TerrainGenerator::new(42, Dimension::Overworld);
    let strongholds = overworld.strongholds();
    assert_eq!(strongholds.len(), 128);
    let distance = |chunk: &ChunkPos| f64::from(chunk.x).hypot(f64::from(chunk.z));
    for first in &strongholds[..3] {
        let far = distance(first);
        assert!(
            (80.0..=176.0).contains(&far),
            "{first:?} is {far} chunks out"
        );
    }
    assert_eq!(
        TerrainGenerator::new(42, Dimension::Overworld).strongholds(),
        strongholds
    );
    for other in [Dimension::Nether, Dimension::End] {
        assert!(
            TerrainGenerator::new(42, other).strongholds().is_empty(),
            "{other:?}"
        );
    }
}
