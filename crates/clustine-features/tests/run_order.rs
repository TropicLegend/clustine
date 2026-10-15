//! The order of a feature run, from the game's `ChunkGenerator.applyBiomeDecoration`
//! (26.3), structures left out: which placed features a chunk's run attempts, and in
//! what order, given the biomes of the three by three chunks around it.
//!
//! Written from the specification, without the run's code. The expectations are
//! worked out here from the data crate (`biome_features`, `order`), so that a run that
//! reads the data wrongly does not agree with itself. While kinds of features are
//! still being built some place blocks and some do not, so these tests look only at
//! the sequence of attempts, never at the blocks.

use std::collections::{BTreeMap, BTreeSet};

use clustine_data::{BlockState, blocks};
use clustine_features::{FeatureSettings, decorate};
use clustine_terrain::biome::Dimension;
use clustine_world::generation::{
    FinalHeightmap, Neighbourhood, PieceKey, PieceState, Placing, StartRef, WorldgenHeightmap,
};
use clustine_world::{Biome, BlockEntity, BlockPos, ChunkPos, ScheduledTick};
use clustine_worldgen_data::feature as data;

const SEED: i64 = 20_261_015;
/// The overworld's lowest block and the y above its highest.
const MIN_Y: i32 = -64;
const TOP_Y: i32 = 320;

/// A biome's number, by its name.
fn biome(name: &str) -> u16 {
    let number = data::biome_features()
        .iter()
        .position(|biome| biome.biome == name)
        .unwrap_or_else(|| panic!("{name} is not a biome of the data"));
    u16::try_from(number).unwrap()
}

/// The names of the placed features a run attempts for a chunk whose three by three
/// holds exactly `present`, in the order the game places them: step by step, and in a
/// step by the feature's place in the overworld's order of that step.
fn expected(present: &[u16]) -> Vec<&'static str> {
    let order = data::order("overworld").unwrap();
    let mut names = Vec::new();
    for (step, ordered) in order.steps.iter().enumerate() {
        let mut places = BTreeSet::new();
        for number in present {
            let listed = &data::biome_features()[usize::from(*number)];
            let Some(features) = listed.steps.get(step) else {
                continue;
            };
            for feature in features.iter() {
                let place = ordered
                    .iter()
                    .position(|candidate| candidate == feature)
                    .unwrap_or_else(|| {
                        panic!(
                            "{} lists a feature the overworld does not order",
                            listed.biome
                        )
                    });
                places.insert(place);
            }
        }
        names.extend(
            places
                .into_iter()
                .map(|place| data::placed_features()[usize::from(ordered[place])].name),
        );
    }
    names
}

/// A view whose biomes are chosen cell by cell, whose blocks are air, and which
/// records the placed features attempted.
struct Fake {
    centre: ChunkPos,
    everywhere: u16,
    /// Biomes other than `everywhere`, by the cell: block coordinates divided by four.
    cells: BTreeMap<(i32, i32, i32), u16>,
    attempts: Vec<&'static str>,
    attempting: bool,
    blocks: BTreeMap<BlockPos, BlockState>,
}

impl Fake {
    fn new(centre: ChunkPos, everywhere: u16) -> Self {
        Self {
            centre,
            everywhere,
            cells: BTreeMap::new(),
            attempts: Vec::new(),
            attempting: false,
            blocks: BTreeMap::new(),
        }
    }

    /// Sets the biome of one cell, by the cell's coordinates.
    fn cell(&mut self, x: i32, y: i32, z: i32, number: u16) {
        self.cells.insert((x, y, z), number);
    }

    /// Sets the biome of every cell of a chunk column over the dimension's height.
    fn column(&mut self, chunk: ChunkPos, number: u16) {
        for x in 0..4 {
            for z in 0..4 {
                for y in (MIN_Y >> 2)..(TOP_Y >> 2) {
                    self.cell(chunk.x * 4 + x, y, chunk.z * 4 + z, number);
                }
            }
        }
    }

    fn in_view(&self, at: BlockPos) -> bool {
        let chunk = at.chunk();
        (chunk.x - self.centre.x).abs() <= 1
            && (chunk.z - self.centre.z).abs() <= 1
            && (MIN_Y..TOP_Y).contains(&at.y)
    }

    fn run(mut self) -> Vec<&'static str> {
        let settings = FeatureSettings::new(SEED, Dimension::Overworld).unwrap();
        let centre = self.centre;
        decorate(&settings, &mut self, centre);
        assert!(!self.attempting, "the run ended inside an attempt");
        self.attempts
    }
}

impl Neighbourhood for Fake {
    fn centre(&self) -> ChunkPos {
        self.centre
    }

    fn block(&mut self, at: BlockPos) -> BlockState {
        self.blocks.get(&at).copied().unwrap_or(blocks::AIR)
    }

    fn set_block(&mut self, at: BlockPos, state: BlockState) -> bool {
        if !self.in_view(at) {
            return false;
        }
        self.blocks.insert(at, state);
        true
    }

    fn block_entity(&mut self, _at: BlockPos) -> Option<&BlockEntity> {
        None
    }

    fn set_block_entity(&mut self, at: BlockPos, _entity: Option<BlockEntity>) -> bool {
        self.in_view(at)
    }

    fn height(&mut self, _map: FinalHeightmap, _x: i32, _z: i32) -> i32 {
        MIN_Y
    }

    fn worldgen_height(&mut self, _map: WorldgenHeightmap, _x: i32, _z: i32) -> i32 {
        MIN_Y
    }

    fn biome(&mut self, at: BlockPos) -> Biome {
        let cell = (at.x >> 2, at.y >> 2, at.z >> 2);
        Biome(self.cells.get(&cell).copied().unwrap_or(self.everywhere))
    }

    fn schedule(&mut self, _at: BlockPos, _tick: ScheduledTick) {}

    fn mark(&mut self, _at: BlockPos) {}

    fn starts(&self) -> &[StartRef] {
        &[]
    }

    fn piece(&mut self, _piece: PieceKey) -> PieceState {
        PieceState::default()
    }

    fn set_piece(&mut self, _piece: PieceKey, _state: PieceState) {}

    fn attempt(&mut self, what: Placing, place: &mut dyn FnMut(&mut dyn Neighbourhood)) -> bool {
        assert!(!self.attempting, "attempts do not nest");
        let Placing::Feature(name) = what else {
            panic!("a run without structures attempted {what:?}");
        };
        self.attempts.push(name);
        self.attempting = true;
        place(self);
        self.attempting = false;
        true
    }
}

/// Says where two sequences of names first part, which a plain comparison of a
/// hundred names does not.
fn assert_same(actual: &[&str], expected: &[&str]) {
    let first = actual
        .iter()
        .zip(expected)
        .position(|(a, e)| a != e)
        .unwrap_or(actual.len().min(expected.len()));
    assert!(
        actual == expected,
        "the attempts part from the expected at {first}: attempted {:?}, expected {:?} \
         ({} attempted, {} expected)",
        actual.get(first),
        expected.get(first),
        actual.len(),
        expected.len(),
    );
}

#[test]
fn one_biome_everywhere_places_its_features_step_by_step_in_the_order_of_places() {
    let plains = biome("minecraft:plains");
    let centre = ChunkPos::new(0, 0);
    let attempts = Fake::new(centre, plains).run();
    let want = expected(&[plains]);
    assert!(want.len() > 20, "plains lists features in many steps");
    assert_same(&attempts, &want);
}

#[test]
fn a_run_attempts_the_same_features_again_for_the_same_inputs() {
    let jungle = biome("minecraft:jungle");
    let centre = ChunkPos::new(-3, 7);
    let first = Fake::new(centre, jungle).run();
    let second = Fake::new(centre, jungle).run();
    assert_same(&first, &second);
    assert_same(&first, &expected(&[jungle]));
}

#[test]
fn two_biomes_in_different_chunks_place_the_union_in_place_order_once_each() {
    let plains = biome("minecraft:plains");
    let desert = biome("minecraft:desert");
    let only_plains = expected(&[plains]);
    let only_desert = expected(&[desert]);
    // Both list features the other does not, and some both list, so that the union
    // is neither alone and a feature listed twice is there to be placed once.
    assert!(only_plains.iter().any(|name| !only_desert.contains(name)));
    assert!(only_desert.iter().any(|name| !only_plains.contains(name)));
    assert!(only_plains.iter().any(|name| only_desert.contains(name)));

    let centre = ChunkPos::new(-5, 12);
    let mut fake = Fake::new(centre, plains);
    fake.column(ChunkPos::new(centre.x - 1, centre.z + 1), desert);
    let attempts = fake.run();
    assert_same(&attempts, &expected(&[plains, desert]));

    let distinct: BTreeSet<&str> = attempts.iter().copied().collect();
    assert_eq!(distinct.len(), attempts.len(), "a feature is placed once");
}

#[test]
fn a_biome_in_the_centre_and_another_around_it_place_the_union() {
    let snowy = biome("minecraft:snowy_taiga");
    let swamp = biome("minecraft:swamp");
    let centre = ChunkPos::new(100_000, -200_000);
    let mut fake = Fake::new(centre, snowy);
    fake.column(centre, swamp);
    assert_same(&fake.run(), &expected(&[snowy, swamp]));
}

#[test]
fn a_biome_only_in_a_chunk_two_away_contributes_nothing() {
    let plains = biome("minecraft:plains");
    let mushrooms = biome("minecraft:mushroom_fields");
    assert_ne!(expected(&[plains]), expected(&[plains, mushrooms]));

    let centre = ChunkPos::new(2, -1);
    let mut fake = Fake::new(centre, plains);
    for dx in -2i32..=2 {
        for dz in -2i32..=2 {
            if dx.abs() == 2 || dz.abs() == 2 {
                fake.column(ChunkPos::new(centre.x + dx, centre.z + dz), mushrooms);
            }
        }
    }
    assert_same(&fake.run(), &expected(&[plains]));
}

#[test]
fn a_biome_in_a_single_cell_at_the_top_of_a_corner_chunk_contributes() {
    let plains = biome("minecraft:plains");
    let mushrooms = biome("minecraft:mushroom_fields");
    let centre = ChunkPos::new(-1, -1);
    let mut fake = Fake::new(centre, plains);
    // The cell of the corner chunk farthest from the centre, at the top of the world.
    let corner = ChunkPos::new(centre.x + 1, centre.z - 1);
    fake.cell(corner.x * 4 + 3, (TOP_Y >> 2) - 1, corner.z * 4, mushrooms);
    assert_same(&fake.run(), &expected(&[plains, mushrooms]));
}

#[test]
fn a_biome_in_a_single_cell_at_the_bottom_of_a_corner_chunk_contributes() {
    let forest = biome("minecraft:forest");
    let desert = biome("minecraft:desert");
    let centre = ChunkPos::new(7, 3);
    let mut fake = Fake::new(centre, forest);
    let corner = ChunkPos::new(centre.x - 1, centre.z + 1);
    fake.cell(corner.x * 4, MIN_Y >> 2, corner.z * 4 + 3, desert);
    assert_same(&fake.run(), &expected(&[forest, desert]));
}

#[test]
fn a_biome_only_above_or_below_the_dimension_contributes_nothing() {
    let plains = biome("minecraft:plains");
    let mushrooms = biome("minecraft:mushroom_fields");
    let centre = ChunkPos::new(0, 0);
    let mut fake = Fake::new(centre, plains);
    for x in -4..8 {
        for z in -4..8 {
            fake.cell(x, TOP_Y >> 2, z, mushrooms);
            fake.cell(x, (MIN_Y >> 2) - 1, z, mushrooms);
        }
    }
    assert_same(&fake.run(), &expected(&[plains]));
}

#[test]
fn a_biome_that_lists_nothing_places_nothing() {
    let barrens = biome("minecraft:end_barrens");
    assert!(
        data::biome_features()[usize::from(barrens)]
            .steps
            .iter()
            .all(|step| step.is_empty())
    );
    let attempts = Fake::new(ChunkPos::new(4, 4), barrens).run();
    assert!(attempts.is_empty(), "attempted {attempts:?}");
}

#[test]
fn a_biome_that_lists_nothing_adds_nothing_beside_another() {
    let barrens = biome("minecraft:end_barrens");
    let taiga = biome("minecraft:taiga");
    let centre = ChunkPos::new(-9, 0);
    let mut fake = Fake::new(centre, barrens);
    fake.column(ChunkPos::new(centre.x + 1, centre.z), taiga);
    assert_same(&fake.run(), &expected(&[taiga]));
}

#[test]
fn the_attempts_go_through_the_steps_from_the_first_and_by_place_within_each() {
    // The same order said another way: each attempt's step and place in the
    // overworld's order rise strictly, and each name is a placed feature's.
    let order = data::order("overworld").unwrap();
    let lush = biome("minecraft:lush_caves");
    let dark = biome("minecraft:dark_forest");
    let centre = ChunkPos::new(1, 1);
    let mut fake = Fake::new(centre, lush);
    fake.column(ChunkPos::new(0, 2), dark);
    let attempts = fake.run();
    assert!(!attempts.is_empty());

    let mut last: Option<(usize, usize)> = None;
    let mut steps = BTreeSet::new();
    for name in &attempts {
        let (index, _) =
            data::placed(name).unwrap_or_else(|| panic!("{name} is not a placed feature"));
        let index = u16::try_from(index).unwrap();
        // The first step and place after the last attempt where this feature stands.
        let next = order
            .steps
            .iter()
            .enumerate()
            .flat_map(|(step, ordered)| {
                ordered
                    .iter()
                    .enumerate()
                    .filter(move |(_, feature)| **feature == index)
                    .map(move |(place, _)| (step, place))
            })
            .find(|at| last.is_none_or(|last| *at > last))
            .unwrap_or_else(|| panic!("{name} was attempted out of the order after {last:?}"));
        steps.insert(next.0);
        last = Some(next);
    }
    assert!(steps.len() > 5, "the attempts span many steps: {steps:?}");
    assert_same(&attempts, &expected(&[lush, dark]));
}
