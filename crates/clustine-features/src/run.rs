//! One feature run: the structures' pieces and the features of a chunk, placed as the
//! game's `ChunkGenerator.applyBiomeDecoration` places them.

use std::collections::BTreeSet;

use clustine_noise::{RandomSource, WorldgenRandom};
use clustine_structures::geometry::{BoundingBox, IVec3};
use clustine_structures::{Start, long_set_order};
use clustine_world::generation::{
    KeyOnly, Maker, Makings, Neighbourhood, PieceKey, Placing, StartRef,
};
use clustine_world::{BlockPos, ChunkPos};
use clustine_worldgen_data::feature as data;
use clustine_worldgen_data::structures::STRUCTURES;

use crate::context::{Context, Level};
use crate::settings::{FeatureSettings, STEPS};
use crate::structures::{self, StartsOf};

/// Places the structures' pieces and the features of `centre` into `view`.
///
/// In each step the structures come first, where the settings place them
/// ([`structures`]). The decoration seed comes from the world's seed and the chunk's lowest corner. The
/// features that can be placed are those the biomes anywhere in the three by three
/// chunks list; for each step, in the order of their places in the step's list, each
/// is seeded by its place and the step and placed from the corner, each in an attempt
/// of its own (ADR-0021), which the game's `try` around each feature stands for.
///
/// No entity is made: a structure's mobs are drawn for as the game draws for them.
pub fn decorate(settings: &FeatureSettings, view: &mut dyn Neighbourhood, centre: ChunkPos) {
    decorate_with(settings, view, centre, &KeyOnly, 0);
}

/// [`decorate`], the structures' entities made by `maker` in the dimension numbered
/// `dimension` (`clustine_world::generation::dimension_number`), each with the next of
/// the run's makings.
pub fn decorate_with(
    settings: &FeatureSettings,
    view: &mut dyn Neighbourhood,
    centre: ChunkPos,
    maker: &dyn Maker,
    dimension: u8,
) {
    let mut makings = Makings::new(dimension, centre);
    let origin = BlockPos::new(centre.x * 16, settings.min_y(), centre.z * 16);
    // The game seeds its generator from the clock before it sets the decoration seed,
    // which replaces that seed; nothing of it remains.
    let mut random = WorldgenRandom::from_seed(0);
    let decoration = random.set_decoration_seed(settings.seed(), origin.x, origin.z);

    let mut biomes = BTreeSet::new();
    for dx in -1..=1 {
        for dz in -1..=1 {
            let (x, z) = ((centre.x + dx) * 16, (centre.z + dz) * 16);
            for y in (settings.min_y()..settings.min_y() + settings.height()).step_by(4) {
                for cell_x in 0..4 {
                    for cell_z in 0..4 {
                        let at = BlockPos::new(x + cell_x * 4, y, z + cell_z * 4);
                        biomes.insert(view.biome(at).0);
                    }
                }
            }
        }
    }

    // The region's own random source, which the game makes once for the run.
    let mut region_random = None;
    for step in 0..STEPS.max(settings.steps()) {
        if let Some(starts_of) = settings.structures() {
            let mut run = Run {
                settings,
                view: &mut *view,
                centre,
                random: &mut random,
                region_random: &mut region_random,
                maker,
                makings: &mut makings,
            };
            run.place_structures(starts_of, decoration, step);
        }
        let mut places = BTreeSet::new();
        for biome in &biomes {
            places.extend(settings.listed(*biome, step).iter().copied());
        }
        for place in places {
            let placed = settings.step(step)[place];
            random.set_feature_seed(decoration, place as i32, step as i32);
            let name = data::placed_features()[placed].name;
            view.attempt(Placing::Feature(name), &mut |view| {
                let mut level =
                    Level::new(view, settings.zoom(), settings.min_y(), settings.height())
                        .with_maker(maker, makings);
                level.carry_region_random(region_random.take());
                let mut context = Context {
                    level: &mut level,
                    random: &mut random,
                    settings,
                    top: Some(placed),
                };
                settings.placed(placed).place(&mut context, origin);
                region_random = level.carry_region_random(None);
                // Only a placement that returns keeps what it took: one that panics
                // is undone with its entities, and gives their makings back.
                makings = level.makings();
            });
        }
    }
}

/// What a feature run carries from one placement to the next.
struct Run<'r> {
    settings: &'r FeatureSettings,
    maker: &'r dyn Maker,
    makings: &'r mut Makings,
    view: &'r mut dyn Neighbourhood,
    centre: ChunkPos,
    random: &'r mut WorldgenRandom,
    region_random: &'r mut Option<RandomSource>,
}

impl Run<'_> {
    /// The structures of `step`, as the game places them before the step's features:
    /// every structure of the data whose step it is, in the data's order, each seeded
    /// by its place among them, counting those with no start here; of each, the
    /// starts the centre refers to, in the order of the game's set of references; of
    /// each start, the pieces whose box meets the centre's writable box, then the
    /// start's `afterPlace`.
    fn place_structures(&mut self, starts_of: &dyn StartsOf, decoration: i64, step: usize) {
        let references = self.view.starts().to_vec();
        let of_step = STRUCTURES
            .iter()
            .enumerate()
            .filter(|(_, structure)| structure.step.ordinal() == step);
        for (place, (structure, _)) in of_step.enumerate() {
            self.random
                .set_feature_seed(decoration, place as i32, step as i32);
            // A chunk keeps the references to one structure's starts in a set of the
            // source chunks, in the order they were found.
            let of_structure: Vec<(i64, StartRef)> = references
                .iter()
                .filter(|reference| {
                    let starts = starts_of.starts(reference.source);
                    starts[usize::from(reference.index)].structure == structure
                })
                .map(|reference| (pack(reference.source), *reference))
                .collect();
            if of_structure.is_empty() {
                continue;
            }
            let inserted: Vec<i64> = of_structure.iter().map(|(packed, _)| *packed).collect();
            for packed in long_set_order(&inserted) {
                let (_, reference) = of_structure
                    .iter()
                    .find(|(of, _)| *of == packed)
                    .expect("the set gives back what was put in");
                let starts = starts_of.starts(reference.source);
                self.place_start(*reference, &starts[usize::from(reference.index)]);
            }
        }
    }

    /// `StructureStart.placeInChunk`.
    fn place_start(&mut self, reference: StartRef, start: &Start) {
        let Some(first) = start.pieces.first() else {
            return;
        };
        let name = start.data().name;
        let (min_y, height) = (self.settings.min_y(), self.settings.height());
        let (x, z) = (self.centre.x * 16, self.centre.z * 16);
        let chunk_box = BoundingBox::new(
            IVec3::new(x, min_y + 1, z),
            IVec3::new(x + 15, min_y + height - 1, z + 15),
        );
        let key = |piece: usize| PieceKey {
            start: reference,
            piece: u16::try_from(piece).expect("a start has fewer than 65,536 pieces"),
        };
        // A piece that settles its height when first placed moves its box, and the
        // game takes the pivot and the test of each piece from the box as it is now.
        let first_state = self.view.piece(key(0));
        let first_box = first
            .bounding_box
            .translate(IVec3::new(0, first_state.shift_y, 0));
        let centre = first_box.center();
        let pivot = IVec3::new(centre.x, first_box.min_y(), centre.z);
        for (index, piece) in start.pieces.iter().enumerate() {
            // A move in y alone never takes a piece into or out of the centre's
            // columns, which reach from the bottom to the top.
            if !piece.bounding_box.intersects_xz(x, z, x + 15, z + 15) {
                continue;
            }
            let state = self.view.piece(key(index));
            if !piece
                .bounding_box
                .translate(IVec3::new(0, state.shift_y, 0))
                .intersects(chunk_box)
            {
                continue;
            }
            self.attempt(
                Placing::Piece(key(index), name),
                chunk_box,
                pivot,
                |placing| {
                    let kept = structures::place_piece(placing, start, key(index), state);
                    if kept != state {
                        placing.level.view().set_piece(key(index), kept);
                    }
                },
            );
        }
        self.attempt(
            Placing::AfterPlace(reference, name),
            chunk_box,
            pivot,
            |placing| {
                structures::after_place(placing, start);
            },
        );
    }

    /// One placement in an attempt of the view (ADR-0021), with a `Level` that carries
    /// the region's random source on from the placement before.
    fn attempt(
        &mut self,
        what: Placing,
        chunk_box: BoundingBox,
        pivot: IVec3,
        mut place: impl FnMut(&mut structures::Placing<'_, '_, '_>),
    ) {
        let Self {
            settings,
            view,
            centre,
            random,
            region_random,
            maker,
            makings,
        } = self;
        view.attempt(what, &mut |view| {
            let mut level = Level::new(view, settings.zoom(), settings.min_y(), settings.height())
                .with_maker(*maker, **makings);
            level.carry_region_random(region_random.take());
            let mut placing = structures::Placing {
                level: &mut level,
                random,
                settings,
                chunk_box,
                centre: *centre,
                pivot,
            };
            place(&mut placing);
            **region_random = level.carry_region_random(None);
            // An attempt undone gives its makings back, as its blocks.
            **makings = level.makings();
        });
    }
}

/// A chunk as the game's sets of chunks hold it (`ChunkPos.asLong`).
fn pack(chunk: ChunkPos) -> i64 {
    clustine_structures::geometry::ChunkPos::new(chunk.x, chunk.z).pack()
}
