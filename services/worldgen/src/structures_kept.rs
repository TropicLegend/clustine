//! The structure starts a finished chunk keeps for natural spawning and the patrol
//! spawner (`docs/groundwork/n9-plan.md`, section 9.6).
//!
//! The game asks a chunk's references at spawning time (`ChunkGenerator.getMobsAt`):
//! for each structure the chunk refers to that has a spawn override for the category,
//! whether a start of it holds the position in its box (a `full` override) or in one
//! of its pieces' (a `piece` override). Clustine keeps what that needs in the chunk,
//! once, as it is generated: for every start the chunk refers to of a structure that has
//! spawn overrides, or of a village, its kind, the chunk it starts in, its box, and the
//! boxes of its pieces that reach the chunk where anything reads them.
//!
//! Which structures have overrides, and which box they ask, is the data's
//! (`data/minecraft/worldgen/structure/*.json`): the swamp hut (`piece`, a witch and a
//! cat), the monument (`full`, guardians, and nothing of the axolotls and underground
//! water creatures), the pillager outpost (`full`, pillagers), the ancient city (`full`,
//! nothing of every category), the trial chambers (`piece`, nothing of every category)
//! and the Nether's fortress (`piece`). Every other structure's overrides are empty.

use std::collections::BTreeMap;

use clustine_world::generation::{BlockBox, PieceKey, PieceState, StartRef};
use clustine_world::{BlockPos, ChunkPos, KeptBox, KeptKind, KeptStructure, OverrideBox};

use crate::stages::StartsNear;

/// How far the game grows a start's box beyond its pieces (`Structure.adjustBoundingBox`):
/// twelve blocks for a structure that adapts the terrain, none for one that does not.
/// The outpost and the villages are `beard_thin`, the ancient city `beard_box`, the
/// trial chambers `encapsulate`; the hut, the monument and the fortress adapt nothing.
fn grown_by(kind: KeptKind) -> i32 {
    match kind {
        KeptKind::SwampHut | KeptKind::Monument | KeptKind::Fortress => 0,
        KeptKind::PillagerOutpost
        | KeptKind::AncientCity
        | KeptKind::TrialChambers
        | KeptKind::VillagePlains
        | KeptKind::VillageDesert
        | KeptKind::VillageSavanna
        | KeptKind::VillageSnowy
        | KeptKind::VillageTaiga => 12,
    }
}

/// Whether anything reads the pieces of a start of `kind`: a `piece` override, and the
/// patrol spawner's stand-in for a village. A `full` override reads only the start's box,
/// so a monument's or an ancient city's many pieces are not kept.
fn keeps_pieces(kind: KeptKind) -> bool {
    kind.overrides() != Some(OverrideBox::Full)
}

fn kept_box(bounds: BlockBox) -> KeptBox {
    KeptBox {
        min: bounds.min,
        max: bounds.max,
    }
}

/// The structure starts the finished chunk at `position` keeps, in the order of its
/// references. `references` are the terrain's (the starts within eight whose box
/// reaches the chunk, `createReferences`), `starts` answers for each source they name,
/// and `pieces` says what the runs left a piece in, where any set it: a piece's box
/// stands where its state has moved it (the swamp hut settles on the ground when it is
/// placed, as the game's `updateAverageGroundHeight` moves its box), and the start's
/// box is around the pieces as made, as the game works it out before any is placed.
pub fn kept(
    position: ChunkPos,
    references: &[StartRef],
    starts: &dyn StartsNear,
    pieces: &dyn Fn(PieceKey) -> Option<PieceState>,
) -> Vec<KeptStructure> {
    let mut kept = Vec::new();
    for reference in references {
        let Some(start) = starts
            .starts(reference.source)
            .starts
            .get(usize::from(reference.index))
        else {
            continue;
        };
        let Some(kind) = KeptKind::from_name(start.structure) else {
            continue;
        };
        let Some(first) = start.pieces.first() else {
            continue;
        };
        let around = start
            .pieces
            .iter()
            .fold(first.bounds, |all, piece| BlockBox {
                min: BlockPos::new(
                    all.min.x.min(piece.bounds.min.x),
                    all.min.y.min(piece.bounds.min.y),
                    all.min.z.min(piece.bounds.min.z),
                ),
                max: BlockPos::new(
                    all.max.x.max(piece.bounds.max.x),
                    all.max.y.max(piece.bounds.max.y),
                    all.max.z.max(piece.bounds.max.z),
                ),
            });
        let mut boxes = Vec::new();
        if keeps_pieces(kind) {
            for (index, piece) in start.pieces.iter().enumerate() {
                let key = PieceKey {
                    start: *reference,
                    piece: u16::try_from(index).expect("a start has fewer pieces than that"),
                };
                let shift = pieces(key).unwrap_or(piece.state).shift_y;
                let moved = KeptBox {
                    min: BlockPos::new(
                        piece.bounds.min.x,
                        piece.bounds.min.y + shift,
                        piece.bounds.min.z,
                    ),
                    max: BlockPos::new(
                        piece.bounds.max.x,
                        piece.bounds.max.y + shift,
                        piece.bounds.max.z,
                    ),
                };
                if moved.meets(position) {
                    boxes.push(moved);
                }
            }
        }
        kept.push(KeptStructure {
            kind,
            source: reference.source,
            bounds: kept_box(around).widened(grown_by(kind)),
            pieces: boxes,
        });
    }
    kept
}

/// The states the runs whose changes a finished chunk has left its pieces in: the
/// last run to set a piece, in ascending rank, has the last word, as a run reads what
/// the runs it saw left.
pub fn left_by<'a>(
    runs: impl IntoIterator<Item = &'a BTreeMap<PieceKey, PieceState>>,
) -> BTreeMap<PieceKey, PieceState> {
    let mut left = BTreeMap::new();
    for run in runs {
        left.extend(run.iter().map(|(key, state)| (*key, *state)));
    }
    left
}

#[cfg(test)]
mod tests {
    use clustine_worldgen_data::structure::TerrainAdaptation;
    use clustine_worldgen_data::structures::STRUCTURES;

    use super::*;
    use crate::stages::{PieceMade, Start, Starts};

    fn block_box(min: (i32, i32, i32), max: (i32, i32, i32)) -> BlockBox {
        BlockBox {
            min: BlockPos::new(min.0, min.1, min.2),
            max: BlockPos::new(max.0, max.1, max.2),
        }
    }

    fn made(min: (i32, i32, i32), max: (i32, i32, i32)) -> PieceMade {
        PieceMade {
            bounds: block_box(min, max),
            state: PieceState::default(),
        }
    }

    struct Near(BTreeMap<ChunkPos, Starts>);

    impl StartsNear for Near {
        fn starts(&self, source: ChunkPos) -> &Starts {
            &self.0[&source]
        }
    }

    fn reference(x: i32, z: i32, index: u16) -> StartRef {
        StartRef {
            source: ChunkPos::new(x, z),
            index,
        }
    }

    /// Around chunk (0, 0): in (0, 0) an igloo and then a swamp hut across the line to
    /// (1, 0); in (-1, 0) a village of three pieces, one in (0, 0); in (0, -2) an
    /// outpost of two pieces.
    fn near() -> Near {
        let mut starts = BTreeMap::new();
        starts.insert(
            ChunkPos::new(0, 0),
            Starts {
                starts: vec![
                    Start {
                        structure: "minecraft:igloo",
                        pieces: vec![made((2, 60, 2), (8, 65, 8))],
                    },
                    Start {
                        structure: "minecraft:swamp_hut",
                        pieces: vec![made((12, 64, 3), (18, 70, 11))],
                    },
                ],
            },
        );
        starts.insert(
            ChunkPos::new(-1, 0),
            Starts {
                starts: vec![Start {
                    structure: "minecraft:village_plains",
                    pieces: vec![
                        made((-20, 63, 0), (-10, 70, 10)),
                        made((-9, 63, 0), (3, 66, 4)),
                        made((-40, 63, 0), (-30, 70, 10)),
                    ],
                }],
            },
        );
        starts.insert(
            ChunkPos::new(0, -2),
            Starts {
                starts: vec![Start {
                    structure: "minecraft:pillager_outpost",
                    pieces: vec![
                        made((0, 70, -30), (14, 92, -18)),
                        made((3, 66, -20), (9, 70, -12)),
                    ],
                }],
            },
        );
        Near(starts)
    }

    fn references() -> Vec<StartRef> {
        vec![
            reference(-1, 0, 0),
            reference(0, -2, 0),
            reference(0, 0, 0),
            reference(0, 0, 1),
        ]
    }

    #[test]
    fn the_starts_kept_are_those_with_overrides_and_villages_in_the_order_of_the_references() {
        let kept = kept(ChunkPos::new(0, 0), &references(), &near(), &|_| None);
        let kinds: Vec<KeptKind> = kept.iter().map(|kept| kept.kind).collect();
        assert_eq!(
            kinds,
            [
                KeptKind::VillagePlains,
                KeptKind::PillagerOutpost,
                KeptKind::SwampHut
            ],
            "the igloo has no overrides"
        );
        let sources: Vec<ChunkPos> = kept.iter().map(|kept| kept.source).collect();
        assert_eq!(
            sources,
            [
                ChunkPos::new(-1, 0),
                ChunkPos::new(0, -2),
                ChunkPos::new(0, 0)
            ]
        );
    }

    #[test]
    fn a_start_s_box_is_around_its_pieces_as_made_grown_where_it_adapts_the_terrain() {
        let kept = kept(ChunkPos::new(0, 0), &references(), &near(), &|_| None);
        let outpost = &kept[1];
        assert_eq!(
            outpost.bounds,
            KeptBox {
                min: BlockPos::new(-12, 54, -42),
                max: BlockPos::new(26, 104, 0),
            },
            "beard_thin: grown by twelve"
        );
        assert!(outpost.pieces.is_empty(), "a full override reads no pieces");
        let hut = &kept[2];
        assert_eq!(hut.bounds, kept_box(block_box((12, 64, 3), (18, 70, 11))));
    }

    #[test]
    fn the_pieces_kept_are_those_that_reach_the_chunk_where_their_state_has_moved_them() {
        let settled = PieceState {
            shift_y: -3,
            settled: true,
            flags: 0,
        };
        let hut = PieceKey {
            start: reference(0, 0, 1),
            piece: 0,
        };
        let kept_here = kept(ChunkPos::new(0, 0), &references(), &near(), &|key| {
            (key == hut).then_some(settled)
        });
        assert_eq!(
            kept_here[2].pieces,
            [kept_box(block_box((12, 61, 3), (18, 67, 11)))],
            "settled three below where it was made"
        );
        // The start's box is the one the game worked out before the hut settled.
        assert_eq!(kept_here[2].bounds.min.y, 64);
        // Of the village only the piece that reaches into (0, 0).
        assert_eq!(
            kept_here[0].pieces,
            [kept_box(block_box((-9, 63, 0), (3, 66, 4)))]
        );
        // Over the line, the hut's piece reaches (1, 0) too; the village's do not.
        let over = kept(
            ChunkPos::new(1, 0),
            &[reference(-1, 0, 0), reference(0, 0, 1)],
            &near(),
            &|_| None,
        );
        assert_eq!(over[0].pieces, []);
        assert_eq!(
            over[1].pieces,
            [kept_box(block_box((12, 64, 3), (18, 70, 11)))]
        );
    }

    #[test]
    fn a_chunk_without_references_keeps_nothing() {
        assert_eq!(kept(ChunkPos::new(5, 5), &[], &near(), &|_| None), []);
    }

    #[test]
    fn the_last_run_to_set_a_piece_has_the_last_word() {
        let key = PieceKey {
            start: reference(0, 0, 1),
            piece: 0,
        };
        let other = PieceKey {
            start: reference(0, 0, 0),
            piece: 0,
        };
        let state = |shift_y| PieceState {
            shift_y,
            settled: true,
            flags: 0,
        };
        let first = BTreeMap::from([(key, state(-1)), (other, state(5))]);
        let second = BTreeMap::from([(key, state(-2))]);
        let left = left_by([&first, &BTreeMap::new(), &second]);
        assert_eq!(left[&key], state(-2));
        assert_eq!(left[&other], state(5));
    }

    /// The table of growths is the data's, and every kind a chunk keeps is a structure
    /// the data has.
    #[test]
    fn every_kind_kept_is_a_structure_of_the_data_grown_as_its_terrain_adaptation_says() {
        for kind in KeptKind::ALL {
            let structure = STRUCTURES
                .iter()
                .find(|structure| structure.name == kind.name())
                .unwrap_or_else(|| panic!("{} is not a structure", kind.name()));
            let grown = if structure.terrain_adaptation == TerrainAdaptation::None {
                0
            } else {
                12
            };
            assert_eq!(grown_by(kind), grown, "{}", kind.name());
        }
    }
}
