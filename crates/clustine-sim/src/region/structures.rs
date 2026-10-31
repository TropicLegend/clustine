//! The structure starts a chunk keeps, as natural spawning and the patrol spawner ask
//! them (`docs/groundwork/n9-plan.md`, sections 9.3 and 9.6).

use clustine_world::{BlockPos, Chunk, ChunkPos, KeptStructure, OverrideBox};

use super::Region;

/// How far around a village piece the patrol spawner's stand-in reaches, in blocks.
const VILLAGE_REACH: i32 = 32;
/// How many chunks around a position's the stand-in asks.
const VILLAGE_CHUNKS: i32 = 2;

impl Region {
    /// The chunk at `position` as the region reads it: its land's, or a border copy.
    fn chunk_or_copy(&self, position: ChunkPos) -> Option<&Chunk> {
        self.chunks
            .get(&position)
            .or_else(|| self.copies.get(&position))
    }

    /// The starts of structures with spawn overrides whose override's box holds `at`
    /// (`ChunkGenerator.getMobsAt`): those kept by the chunk of `at`, in its land or a
    /// copy, whose own box holds it for a `full` override or one of whose pieces' boxes
    /// does for a `piece` override. Natural spawning takes the first of them that has an
    /// override for the category it spawns.
    ///
    /// They come in the order the chunk keeps them, generation's (by the chunk each starts
    /// in, x counting most, then the start's place there). The game asks a hash map of
    /// structures whose order changes from run to run, so no order is more the game's;
    /// two starts of structures with overrides hold the same position rarely, if ever. A
    /// chunk the region has neither in its land nor as a copy keeps none.
    pub fn structures_at(&self, at: BlockPos) -> Vec<&KeptStructure> {
        let Some(chunk) = self.chunk_or_copy(at.chunk()) else {
            return Vec::new();
        };
        chunk
            .structures()
            .iter()
            .filter(|kept| match kept.kind.overrides() {
                Some(OverrideBox::Full) => kept.bounds.holds(at),
                Some(OverrideBox::Piece) => kept.pieces.iter().any(|piece| piece.holds(at)),
                None => false,
            })
            .collect()
    }

    /// Whether a village is near `at`, the patrol spawner's stand-in for the game's
    /// `isCloseToVillage(pos, 2)` until there are points of interest (section 9.6): a
    /// piece of a village kept by a chunk within two of `at`'s, in the region's land or
    /// its copies, whose box widened by 32 holds `at`. A chunk the region has neither
    /// way is not asked.
    pub fn village_near(&self, at: BlockPos) -> bool {
        let centre = at.chunk();
        (-VILLAGE_CHUNKS..=VILLAGE_CHUNKS).any(|dx| {
            (-VILLAGE_CHUNKS..=VILLAGE_CHUNKS).any(|dz| {
                self.chunk_or_copy(ChunkPos::new(centre.x + dx, centre.z + dz))
                    .is_some_and(|chunk| {
                        chunk.structures().iter().any(|kept| {
                            kept.kind.is_village()
                                && kept
                                    .pieces
                                    .iter()
                                    .any(|piece| piece.widened(VILLAGE_REACH).holds(at))
                        })
                    })
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::DIMENSION_TYPES;
    use clustine_world::{Biome, EntityIds, Inventory, KeptBox, KeptKind, Vec3};

    use super::super::{Holdings, LightSettings, RegionConfig};
    use super::*;

    fn region() -> Region {
        Region::new(
            RegionConfig {
                world_spawn: clustine_world::BlockPos::new(0, -60, 0),
                strongholds: Vec::new(),
                homes: Vec::new(),
                dimension: Default::default(),
                spawn: Some(Vec3::new(0.5, 64.0, 0.5)),
                starting_inventory: Inventory::default(),
                return_after: 0,
                lowest_y: -64,
                seed: 0,
                game_seed: 0,
                sea_level: 63,
                spawn_mobs: true,
                light: LightSettings::default(),
                features: Default::default(),
                random_tick_speed: crate::region::RANDOM_TICK_SPEED,
            },
            EntityIds::block(0).unwrap(),
            Holdings::default(),
        )
    }

    fn chunk_with(structures: Vec<KeptStructure>) -> Chunk {
        let overworld = DIMENSION_TYPES
            .iter()
            .find(|dimension| dimension.name == "minecraft:overworld")
            .unwrap();
        let mut chunk = Chunk::empty(overworld, Biome(0));
        chunk.set_structures(structures);
        chunk
    }

    fn kept_box(min: (i32, i32, i32), max: (i32, i32, i32)) -> KeptBox {
        KeptBox {
            min: BlockPos::new(min.0, min.1, min.2),
            max: BlockPos::new(max.0, max.1, max.2),
        }
    }

    fn kept(kind: KeptKind, bounds: KeptBox, pieces: Vec<KeptBox>) -> KeptStructure {
        KeptStructure {
            kind,
            source: ChunkPos::new(0, 0),
            bounds,
            pieces,
        }
    }

    /// A hut whose piece settled three below its start's box, and an outpost whose box
    /// holds the hut's: both kept by chunk (0, 0).
    fn hut_and_outpost() -> Vec<KeptStructure> {
        vec![
            kept(
                KeptKind::SwampHut,
                kept_box((2, 64, 2), (8, 70, 10)),
                vec![kept_box((2, 61, 2), (8, 67, 10))],
            ),
            kept(
                KeptKind::PillagerOutpost,
                kept_box((-12, 50, -12), (30, 100, 30)),
                Vec::new(),
            ),
        ]
    }

    #[test]
    fn a_piece_override_asks_the_pieces_and_a_full_one_the_start_s_box_in_the_chunk_s_order() {
        let mut region = region();
        region
            .chunks
            .insert(ChunkPos::new(0, 0), chunk_with(hut_and_outpost()));
        let kinds = |at: BlockPos| -> Vec<KeptKind> {
            region
                .structures_at(at)
                .iter()
                .map(|kept| kept.kind)
                .collect()
        };
        // On the hut's floor, settled below where its start's box was made.
        assert_eq!(
            kinds(BlockPos::new(4, 62, 5)),
            [KeptKind::SwampHut, KeptKind::PillagerOutpost]
        );
        // Above the settled piece but in the start's box: not the hut's.
        assert_eq!(kinds(BlockPos::new(4, 69, 5)), [KeptKind::PillagerOutpost]);
        // Outside the outpost's box too.
        assert_eq!(kinds(BlockPos::new(4, 101, 5)), []);
        assert_eq!(kinds(BlockPos::new(15, 49, 15)), []);
        // The outpost's box reaches chunk (1, 0), which keeps nothing here: the chunk of
        // the position is the one asked.
        assert_eq!(kinds(BlockPos::new(20, 70, 5)), []);
    }

    #[test]
    fn a_copy_is_asked_as_land_is_and_a_chunk_the_region_has_not_answers_nothing() {
        let mut region = region();
        let at = BlockPos::new(4, 62, 5);
        assert!(region.structures_at(at).is_empty());
        region
            .copies
            .insert(ChunkPos::new(0, 0), chunk_with(hut_and_outpost()));
        assert_eq!(region.structures_at(at).len(), 2);
    }

    #[test]
    fn a_village_is_never_a_structure_with_overrides() {
        let mut region = region();
        let village = kept(
            KeptKind::VillagePlains,
            kept_box((0, 60, 0), (15, 80, 15)),
            vec![kept_box((0, 60, 0), (15, 80, 15))],
        );
        region
            .chunks
            .insert(ChunkPos::new(0, 0), chunk_with(vec![village]));
        assert!(region.structures_at(BlockPos::new(4, 64, 4)).is_empty());
        assert!(region.village_near(BlockPos::new(4, 64, 4)));
    }

    #[test]
    fn a_village_is_near_within_32_blocks_of_a_piece_kept_within_two_chunks() {
        let mut region = region();
        // A village piece kept by chunk (2, 0), at x 32 to 40.
        let village = kept(
            KeptKind::VillageTaiga,
            kept_box((32, 64, 0), (40, 70, 8)),
            vec![kept_box((32, 64, 0), (40, 70, 8))],
        );
        region
            .copies
            .insert(ChunkPos::new(2, 0), chunk_with(vec![village]));
        // 32 blocks west of the piece, in chunk (0, 0): two chunks away.
        assert!(region.village_near(BlockPos::new(0, 64, 4)));
        // A block further west, in chunk (-1, 0): widened by 32, the box ends at x 0.
        assert!(!region.village_near(BlockPos::new(-1, 64, 4)));
        // Above and below by 32 and no more.
        assert!(region.village_near(BlockPos::new(36, 102, 4)));
        assert!(!region.village_near(BlockPos::new(36, 103, 4)));
        assert!(region.village_near(BlockPos::new(36, 32, 4)));
        assert!(!region.village_near(BlockPos::new(36, 31, 4)));
        // A hut is no village.
        let mut huts = self::region();
        huts.chunks
            .insert(ChunkPos::new(0, 0), chunk_with(hut_and_outpost()));
        assert!(!huts.village_near(BlockPos::new(4, 62, 5)));
    }

    #[test]
    fn a_village_piece_kept_three_chunks_away_is_not_near() {
        let mut region = region();
        let village = kept(
            KeptKind::VillageDesert,
            kept_box((48, 64, 0), (60, 70, 8)),
            vec![kept_box((48, 64, 0), (60, 70, 8))],
        );
        region
            .chunks
            .insert(ChunkPos::new(3, 0), chunk_with(vec![village]));
        // Chunk (0, 0) is three from the piece's: 33 blocks from it.
        assert!(!region.village_near(BlockPos::new(15, 64, 4)));
        // From chunk (1, 0) it is two away, and x 16 is 32 from x 48.
        assert!(region.village_near(BlockPos::new(16, 64, 4)));
    }
}
