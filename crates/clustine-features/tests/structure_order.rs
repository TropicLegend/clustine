//! The order in which a feature run places the structures' pieces, from the game's
//! `ChunkGenerator.applyBiomeDecoration` (26.3) as section 2 of
//! `docs/groundwork/s-plan.md` describes it: in each step, first the structures of the
//! step in the order of `STRUCTURES`, each structure's starts in the order of the
//! references' `LongOpenHashSet`, of each start the pieces that meet the centre's
//! writable box in their order and then the start's `afterPlace`; then the step's
//! features.
//!
//! Written from the specification, without the run's code. The placers are stubs
//! while phase S is built, so these tests look only at the sequence of attempts the
//! run makes through its view, never at blocks or at the random source's seeds.

use std::collections::BTreeMap;
use std::sync::Arc;

use clustine_data::{BlockState, blocks};
use clustine_features::structures::StartsOf;
use clustine_features::{FeatureSettings, decorate};
use clustine_structures::geometry::{self, BoundingBox, IVec3, Identifier};
use clustine_structures::piece::StructurePiece;
use clustine_structures::{Start, long_set_order};
use clustine_terrain::biome::Dimension;
use clustine_world::generation::{
    FinalHeightmap, Neighbourhood, PieceKey, PieceState, Placing, StartRef, WorldgenHeightmap,
};
use clustine_world::{Biome, BlockEntity, BlockPos, ChunkPos, ScheduledTick};
use clustine_worldgen_data::feature as data;
use clustine_worldgen_data::structures::STRUCTURES;

const SEED: i64 = 20_261_015;
/// The overworld's lowest block and its height.
const MIN_Y: i32 = -64;
const HEIGHT: i32 = 384;
/// A centre away from the origin and with a negative z, so that a box worked out
/// from the chunk's coordinates with the wrong sign or scale does not pass.
const CENTRE: ChunkPos = ChunkPos { x: 3, z: -2 };

/// A structure's place in `STRUCTURES`, by its name.
fn structure(name: &str) -> usize {
    STRUCTURES
        .iter()
        .position(|structure| structure.name == name)
        .unwrap_or_else(|| panic!("{name} is not a structure of the data"))
}

fn step_of(structure: usize) -> usize {
    STRUCTURES[structure].step.ordinal()
}

/// A piece whose box runs between two corners, given relative to the centre chunk's
/// lowest corner in x and z and absolutely in y.
fn piece(from: (i32, i32, i32), to: (i32, i32, i32)) -> StructurePiece {
    let (x, z) = (CENTRE.x * 16, CENTRE.z * 16);
    StructurePiece::non_jigsaw(
        Identifier::new_static("clustine", "toy"),
        BoundingBox::new(
            IVec3::new(x + from.0, from.1, z + from.2),
            IVec3::new(x + to.0, to.1, z + to.2),
        ),
        0,
        None,
    )
}

/// A piece well inside the centre chunk.
fn inside() -> StructurePiece {
    piece((2, 60, 2), (9, 70, 9))
}

/// A start of `structure` in `chunk`, whose box holds its pieces as the game's does.
fn start(structure: usize, chunk: ChunkPos, pieces: Vec<StructurePiece>) -> Start {
    let bounding_box = pieces
        .iter()
        .map(|piece| piece.bounding_box)
        .reduce(|a, b| BoundingBox::encapsulating(&a, &b))
        .unwrap_or_else(|| {
            let corner = IVec3::new(chunk.x * 16, 0, chunk.z * 16);
            BoundingBox::new(corner, corner)
        });
    Start {
        structure,
        chunk: (chunk.x, chunk.z),
        pieces,
        bounding_box,
    }
}

/// The starts of each source chunk, as the scheduler hands them to a run.
#[derive(Default)]
struct Table {
    starts: BTreeMap<(i32, i32), Vec<Start>>,
}

impl Table {
    /// Adds a start to its chunk's list and gives the reference that names it.
    fn add(&mut self, start: Start) -> StartRef {
        let list = self.starts.entry(start.chunk).or_default();
        let index = u16::try_from(list.len()).unwrap();
        let source = ChunkPos::new(start.chunk.0, start.chunk.1);
        list.push(start);
        StartRef { source, index }
    }

    fn settings(self) -> FeatureSettings {
        let starts: BTreeMap<(i32, i32), Arc<[Start]>> = self
            .starts
            .into_iter()
            .map(|(chunk, list)| (chunk, Arc::from(list)))
            .collect();
        FeatureSettings::new(SEED, Dimension::Overworld)
            .unwrap()
            .with_structures(Arc::new(Shared(starts)))
    }
}

struct Shared(BTreeMap<(i32, i32), Arc<[Start]>>);

impl StartsOf for Shared {
    fn starts(&self, source: ChunkPos) -> Arc<[Start]> {
        self.0
            .get(&(source.x, source.z))
            .cloned()
            .unwrap_or_else(|| Arc::from(Vec::new()))
    }
}

/// A view of air over plains, which answers with the references and piece states a
/// test chose and records every attempt in order.
struct Recorder {
    references: Vec<StartRef>,
    states: BTreeMap<PieceKey, PieceState>,
    record: Vec<Placing>,
    attempting: bool,
    blocks: BTreeMap<BlockPos, BlockState>,
    biome: Biome,
}

impl Recorder {
    fn new(references: Vec<StartRef>) -> Self {
        let plains = data::biome_features()
            .iter()
            .position(|biome| biome.biome == "minecraft:plains")
            .expect("plains is a biome of the data");
        Self {
            references,
            states: BTreeMap::new(),
            record: Vec::new(),
            attempting: false,
            blocks: BTreeMap::new(),
            biome: Biome(u16::try_from(plains).unwrap()),
        }
    }

    fn run(mut self, settings: &FeatureSettings) -> Vec<Placing> {
        decorate(settings, &mut self, CENTRE);
        assert!(!self.attempting, "the run ended inside an attempt");
        self.record
    }

    fn in_view(at: BlockPos) -> bool {
        let chunk = at.chunk();
        (chunk.x - CENTRE.x).abs() <= 1
            && (chunk.z - CENTRE.z).abs() <= 1
            && (MIN_Y..MIN_Y + HEIGHT).contains(&at.y)
    }
}

impl Neighbourhood for Recorder {
    fn centre(&self) -> ChunkPos {
        CENTRE
    }

    fn block(&mut self, at: BlockPos) -> BlockState {
        self.blocks.get(&at).copied().unwrap_or(blocks::AIR)
    }

    fn set_block(&mut self, at: BlockPos, state: BlockState) -> bool {
        if !Self::in_view(at) {
            return false;
        }
        self.blocks.insert(at, state);
        true
    }

    fn block_entity(&mut self, _at: BlockPos) -> Option<&BlockEntity> {
        None
    }

    fn set_block_entity(&mut self, at: BlockPos, _entity: Option<BlockEntity>) -> bool {
        Self::in_view(at)
    }

    fn height(&mut self, _map: FinalHeightmap, _x: i32, _z: i32) -> i32 {
        MIN_Y
    }

    fn worldgen_height(&mut self, _map: WorldgenHeightmap, _x: i32, _z: i32) -> i32 {
        MIN_Y
    }

    fn biome(&mut self, _at: BlockPos) -> Biome {
        self.biome
    }

    fn schedule(&mut self, _at: BlockPos, _tick: ScheduledTick) {}

    fn mark(&mut self, _at: BlockPos) {}

    fn starts(&self) -> &[StartRef] {
        &self.references
    }

    fn piece(&mut self, piece: PieceKey) -> PieceState {
        self.states.get(&piece).copied().unwrap_or_default()
    }

    fn set_piece(&mut self, piece: PieceKey, state: PieceState) {
        self.states.insert(piece, state);
    }

    fn attempt(&mut self, what: Placing, place: &mut dyn FnMut(&mut dyn Neighbourhood)) -> bool {
        assert!(!self.attempting, "attempts do not nest");
        self.record.push(what);
        self.attempting = true;
        place(self);
        self.attempting = false;
        true
    }
}

/// The attempts of structures alone, features left out.
fn structural(record: &[Placing]) -> Vec<Placing> {
    record
        .iter()
        .copied()
        .filter(|what| !matches!(what, Placing::Feature(_)))
        .collect()
}

/// What a start whose pieces all meet the centre is expected to give: each piece in
/// order, then its `afterPlace`.
fn whole(reference: StartRef, structure: usize, pieces: u16) -> Vec<Placing> {
    let name = STRUCTURES[structure].name;
    (0..pieces)
        .map(|piece| {
            Placing::Piece(
                PieceKey {
                    start: reference,
                    piece,
                },
                name,
            )
        })
        .chain([Placing::AfterPlace(reference, name)])
        .collect()
}

/// Says where two sequences of attempts first part, which a plain comparison of long
/// lists does not.
fn assert_same(actual: &[Placing], expected: &[Placing]) {
    let first = actual
        .iter()
        .zip(expected)
        .position(|(a, e)| a != e)
        .unwrap_or(actual.len().min(expected.len()));
    assert!(
        actual == expected,
        "the attempts part from the expected at {first}: attempted {:?}, expected {:?} \
         ({} attempted, {} expected)\nall attempted: {actual:?}",
        actual.get(first),
        expected.get(first),
        actual.len(),
        expected.len(),
    );
}

/// The steps of the overworld in which a placed feature stands.
fn feature_steps(name: &str) -> Vec<usize> {
    let order = data::order("overworld").unwrap();
    let (index, _) = data::placed(name).unwrap_or_else(|| panic!("{name} is not placed"));
    let index = u16::try_from(index).unwrap();
    order
        .steps
        .iter()
        .enumerate()
        .filter(|(_, ordered)| ordered.contains(&index))
        .map(|(step, _)| step)
        .collect()
}

#[test]
fn structures_come_step_by_step_and_before_the_features_of_their_step() {
    let treasure = structure("minecraft:buried_treasure");
    let igloo = structure("minecraft:igloo");
    let city = structure("minecraft:ancient_city");
    assert_eq!(
        [step_of(treasure), step_of(igloo), step_of(city)],
        [3, 4, 7]
    );

    // The references name the latest step first, so that a run that follows them
    // rather than the steps is caught.
    let mut table = Table::default();
    let city_ref = table.add(start(city, ChunkPos::new(2, -2), vec![inside()]));
    let igloo_ref = table.add(start(igloo, ChunkPos::new(3, -1), vec![inside()]));
    let treasure_ref = table.add(start(treasure, CENTRE, vec![inside()]));
    let settings = table.settings();
    // A biome with features in the steps of underground structures and of underground
    // decoration (plains has none in the latter), and in that of surface structures
    // too where one has.
    let listed = |biome: &data::BiomeFeatures, step: usize| {
        biome
            .steps
            .get(step)
            .is_some_and(|features| !features.is_empty())
    };
    let candidates = || {
        data::biome_features()
            .iter()
            .enumerate()
            .filter(|(_, biome)| listed(biome, 3) && listed(biome, 7))
    };
    let (number, chosen) = candidates()
        .find(|(_, biome)| listed(biome, 4))
        .or_else(|| candidates().next())
        .expect("some biome lists features in steps 3 and 7");
    let biome = chosen.biome;
    let mut recorder = Recorder::new(vec![city_ref, igloo_ref, treasure_ref]);
    recorder.biome = Biome(u16::try_from(number).unwrap());
    let record = recorder.run(&settings);

    let mut expected = whole(treasure_ref, treasure, 1);
    expected.extend(whole(igloo_ref, igloo, 1));
    expected.extend(whole(city_ref, city, 1));
    assert_same(&structural(&record), &expected);

    // Every attempt's step and kind (structures before features) rise: a feature is
    // given the first step it stands in that is not before the last attempt's.
    let mut last = (0, 0);
    let mut feature_in = BTreeMap::<usize, usize>::new();
    for what in &record {
        let at = match what {
            Placing::Piece(_, name) | Placing::AfterPlace(_, name) => (step_of(structure(name)), 0),
            Placing::Feature(name) => {
                let step = feature_steps(name)
                    .into_iter()
                    .find(|step| *step >= last.0)
                    .unwrap_or_else(|| {
                        panic!("{name} was attempted after step {} had begun", last.0)
                    });
                *feature_in.entry(step).or_default() += 1;
                (step, 1)
            }
        };
        assert!(
            at >= last,
            "{what:?} (step {}) was attempted after an attempt of step {} {}",
            at.0,
            last.0,
            if last.1 == 0 {
                "of structures"
            } else {
                "of features"
            },
        );
        last = at;
    }
    // The biome has features in the steps of these structures, so that the order
    // checked above is between structures and features of the same step.
    for step in [3, 7] {
        assert!(
            feature_in.contains_key(&step),
            "{biome} places no feature in step {step}: {feature_in:?}"
        );
    }
}

#[test]
fn structures_of_one_step_come_in_the_order_of_the_list_whatever_the_references_order() {
    let treasure = structure("minecraft:buried_treasure");
    let mineshaft = structure("minecraft:mineshaft");
    let mesa = structure("minecraft:mineshaft_mesa");
    assert!(treasure < mineshaft && mineshaft < mesa);
    assert_eq!(step_of(treasure), step_of(mineshaft));
    assert_eq!(step_of(mineshaft), step_of(mesa));

    let mut table = Table::default();
    let mesa_ref = table.add(start(mesa, ChunkPos::new(4, -2), vec![inside(), inside()]));
    let mineshaft_ref = table.add(start(mineshaft, ChunkPos::new(2, -3), vec![inside()]));
    let treasure_ref = table.add(start(treasure, ChunkPos::new(3, -3), vec![inside()]));
    let settings = table.settings();
    let record = Recorder::new(vec![mesa_ref, mineshaft_ref, treasure_ref]).run(&settings);

    let mut expected = whole(treasure_ref, treasure, 1);
    expected.extend(whole(mineshaft_ref, mineshaft, 1));
    expected.extend(whole(mesa_ref, mesa, 2));
    assert_same(&structural(&record), &expected);
}

#[test]
fn starts_of_one_structure_come_in_the_set_order_of_their_sources() {
    let mineshaft = structure("minecraft:mineshaft");
    let igloo = structure("minecraft:igloo");
    let sources = [
        ChunkPos::new(3, -2),
        ChunkPos::new(-5, 7),
        ChunkPos::new(4, -2),
        ChunkPos::new(2, -1),
        ChunkPos::new(100, -300),
        ChunkPos::new(3, -3),
        ChunkPos::new(-1, 0),
        ChunkPos::new(0, -1),
    ];
    let packed: Vec<i64> = sources
        .iter()
        .map(|source| geometry::ChunkPos::new(source.x, source.z).pack())
        .collect();
    let ordered = long_set_order(&packed);
    assert_ne!(
        ordered, packed,
        "the sources are chosen so that the set's order is not the insertion order"
    );

    let mut table = Table::default();
    let mut references = Vec::new();
    let mut by_packed = BTreeMap::new();
    for (source, packed) in sources.iter().zip(&packed) {
        // An igloo at the same source first, so that the mineshafts' references are
        // not all at index 0 and the structures' references are interleaved.
        if source.x % 2 == 0 {
            references.push(table.add(start(igloo, *source, vec![inside()])));
        }
        let reference = table.add(start(mineshaft, *source, vec![inside()]));
        references.push(reference);
        by_packed.insert(*packed, reference);
    }
    let igloos: Vec<StartRef> = references
        .iter()
        .copied()
        .filter(|reference| !by_packed.values().any(|named| named == reference))
        .collect();
    let settings = table.settings();
    let record = Recorder::new(references.clone()).run(&settings);

    // The mineshaft is of an earlier step than the igloo.
    assert!(step_of(mineshaft) < step_of(igloo));
    let mut expected = Vec::new();
    for packed in &ordered {
        expected.extend(whole(by_packed[packed], mineshaft, 1));
    }
    let igloo_packed: Vec<i64> = igloos
        .iter()
        .map(|reference| geometry::ChunkPos::new(reference.source.x, reference.source.z).pack())
        .collect();
    for packed in long_set_order(&igloo_packed) {
        let reference = igloos
            .iter()
            .zip(&igloo_packed)
            .find(|(_, candidate)| **candidate == packed)
            .map(|(reference, _)| *reference)
            .unwrap();
        expected.extend(whole(reference, igloo, 1));
    }
    assert_same(&structural(&record), &expected);
}

#[test]
fn only_pieces_meeting_the_writable_box_of_the_centre_are_placed_judged_by_their_shifted_box() {
    let monument = structure("minecraft:monument");
    let pieces = vec![
        // 0: inside.
        inside(),
        // 1: the next chunk east, from its first column.
        piece((16, 60, 2), (20, 70, 9)),
        // 2: the chunk north, up to its last row.
        piece((2, 60, -5), (9, 70, -1)),
        // 3: the centre's last column and row only, at one block.
        piece((15, 64, 15), (15, 64, 15)),
        // 4: down to the world's lowest block, which is not writable.
        piece((0, -80, 0), (3, MIN_Y, 3)),
        // 5: the same, shifted up by one into the lowest writable block.
        piece((0, -80, 0), (3, MIN_Y, 3)),
        // 6: from the world's highest block, which is writable.
        piece((0, MIN_Y + HEIGHT - 1, 0), (3, 400, 3)),
        // 7: the same, shifted up by one out of the world.
        piece((0, MIN_Y + HEIGHT - 1, 0), (3, 400, 3)),
        // 8: inside as made, shifted far below the world.
        piece((4, 0, 4), (8, 10, 8)),
        // 9: above the world as made, shifted down into it.
        piece((4, 400, 4), (8, 410, 8)),
        // 10: inside again after those left out, so that the order is seen to go on.
        piece((0, 0, 0), (15, 0, 15)),
        // 11: a diagonal neighbour that shares no column.
        piece((-4, 60, 16), (-1, 70, 20)),
    ];
    let mut table = Table::default();
    let reference = table.add(start(monument, ChunkPos::new(2, -2), pieces));
    let settings = table.settings();
    let mut recorder = Recorder::new(vec![reference]);
    let shifted = |piece| PieceKey {
        start: reference,
        piece,
    };
    for (piece, shift_y) in [(5, 1), (7, 1), (8, -200), (9, -100)] {
        recorder.states.insert(
            shifted(piece),
            PieceState {
                shift_y,
                settled: true,
                flags: 0,
            },
        );
    }
    let record = recorder.run(&settings);

    let name = STRUCTURES[monument].name;
    let mut expected: Vec<Placing> = [0, 3, 5, 6, 9, 10]
        .into_iter()
        .map(|piece| Placing::Piece(shifted(piece), name))
        .collect();
    expected.push(Placing::AfterPlace(reference, name));
    assert_same(&structural(&record), &expected);
}

#[test]
fn after_place_comes_once_per_start_even_when_none_of_its_pieces_meets_the_centre() {
    let pyramid = structure("minecraft:desert_pyramid");
    let mansion = structure("minecraft:mansion");
    assert!(pyramid < mansion);
    assert_eq!(step_of(pyramid), step_of(mansion));

    let mut table = Table::default();
    // The pyramid's pieces are all in chunks around the centre, never in it.
    let pyramid_ref = table.add(start(
        pyramid,
        ChunkPos::new(4, -2),
        vec![
            piece((16, 60, 0), (30, 70, 15)),
            piece((-10, 60, 0), (-1, 70, 15)),
        ],
    ));
    let mansion_ref = table.add(start(
        mansion,
        ChunkPos::new(3, -3),
        vec![inside(), piece((0, 60, 16), (15, 70, 20)), inside()],
    ));
    let settings = table.settings();
    let record = Recorder::new(vec![mansion_ref, pyramid_ref]).run(&settings);

    let mansion_name = STRUCTURES[mansion].name;
    let expected = vec![
        Placing::AfterPlace(pyramid_ref, STRUCTURES[pyramid].name),
        Placing::Piece(
            PieceKey {
                start: mansion_ref,
                piece: 0,
            },
            mansion_name,
        ),
        Placing::Piece(
            PieceKey {
                start: mansion_ref,
                piece: 2,
            },
            mansion_name,
        ),
        Placing::AfterPlace(mansion_ref, mansion_name),
    ];
    assert_same(&structural(&record), &expected);
}

#[test]
fn a_start_with_no_pieces_is_skipped_without_its_after_place() {
    let igloo = structure("minecraft:igloo");
    let outpost = structure("minecraft:pillager_outpost");
    let mut table = Table::default();
    let empty = table.add(start(igloo, CENTRE, Vec::new()));
    let outpost_ref = table.add(start(outpost, CENTRE, vec![inside()]));
    let settings = table.settings();
    let record = Recorder::new(vec![empty, outpost_ref]).run(&settings);
    assert_same(&structural(&record), &whole(outpost_ref, outpost, 1));
}

#[test]
fn without_structures_in_the_settings_no_piece_or_after_place_is_attempted() {
    let igloo = structure("minecraft:igloo");
    let mut table = Table::default();
    let reference = table.add(start(igloo, CENTRE, vec![inside()]));
    // The same references, but settings that were never given the starts.
    drop(table);
    let settings = FeatureSettings::new(SEED, Dimension::Overworld).unwrap();
    let record = Recorder::new(vec![reference]).run(&settings);
    assert!(!record.is_empty(), "plains places features");
    assert_same(&structural(&record), &[]);
}

#[test]
fn the_same_inputs_give_the_same_attempts() {
    let make = || {
        let mut table = Table::default();
        let a = table.add(start(
            structure("minecraft:village_plains"),
            ChunkPos::new(2, -2),
            vec![inside(), inside()],
        ));
        let b = table.add(start(
            structure("minecraft:trial_chambers"),
            ChunkPos::new(4, -1),
            vec![inside()],
        ));
        (table.settings(), vec![b, a])
    };
    let (settings, references) = make();
    let first = Recorder::new(references.clone()).run(&settings);
    let (settings, _) = make();
    let second = Recorder::new(references).run(&settings);
    assert_same(&first, &second);
    assert_eq!(structural(&first).len(), 5);
}
