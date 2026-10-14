// Written for Clustine from the pinned jar's classes (`JigsawStructure`,
// `JigsawPlacement` and its `Placer`, `StructureTemplatePool`, the pool elements,
// `StructureTemplate.getJigsaws`, `JigsawBlock.canAttach`, `PoolAliasLookup` and the
// alias bindings, `SequencedPriorityIterator`), with SteelMC's
// steel-worldgen/src/structure/jigsaw.rs (commit
// 885c4b3e60ed79862c37311780774f76806cb714) read beside them; the two records of a
// piece at the head are SteelMC's. See NOTICE in this crate.

//! Structures assembled from templates: villages, pillager outposts, bastions,
//! ancient cities, trail ruins, trial chambers, abandoned camps.
//!
//! A start draws a rotation and a first template from the structure's start pool.
//! Then piece after piece, every jigsaw block of a piece (shuffled, then by its
//! selection priority) looks in its pool, and where the assembly is at its depth in
//! the pool's fallback, for a template with a jigsaw block that fits, in a shuffled
//! order and under each of the four rotations in a shuffled order, and takes the
//! first that lies wholly in free space. Free space is the box the structure may
//! reach less the pieces placed so far; a piece placed inside its parent's box has
//! the parent's box for its free space instead. What is drawn in which order decides
//! everything, and is the game's.
//!
//! **Only sizes and jigsaw blocks of templates are read** (ADR-0019): that is all
//! the layout depends on.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use clustine_noise::random::{LegacyRandom, PositionalRandom, Random};
use clustine_worldgen_data::structure::{
    HeightProvider, Heightmap, Jigsaw, Orientation, PoolAlias, PoolElement, Projection,
};
use clustine_worldgen_data::template_pools::POOLS;

use crate::compat::{LiquidSettingsData, StructureConfigData, StructureData};
use crate::geometry::{BoundingBox, Direction, IVec3, Identifier, Rotation};
use crate::{
    GenerationStub, Structure, StructureGenerationContext, StructurePiece, StructurePiecePayload,
};

/// What a piece of a jigsaw structure is beyond its box: its element, where its
/// template's corner is and how it is turned.
#[derive(Debug, Clone)]
pub struct JigsawPieceData {
    pub pool_element: &'static PoolElement,
    pub position: IVec3,
    pub rotation: Rotation,
    pub liquid_settings: LiquidSettingsData,
}

/// Where two pieces are joined, for the bending of the ground between them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JigsawJunction {
    /// The x and z of the other piece's jigsaw block, and the ground's height there.
    pub source_pos: IVec3,
    pub delta_y: i32,
    /// How the other piece meets the ground.
    pub dest_projection: Projection,
}

/// The pool that a jigsaw block without a pool names, and that nothing is drawn
/// from.
const EMPTY_POOL: &str = "minecraft:empty";

/// A jigsaw block of a placed or tried template, in the world.
#[derive(Clone, Copy, Debug)]
struct Jig {
    pos: IVec3,
    front: Direction,
    top: Direction,
    rollable: bool,
    /// The feature's jigsaw block has no name.
    name: Option<&'static str>,
    target: &'static str,
    pool: &'static str,
    selection_priority: i32,
    placement_priority: i32,
}

fn front_and_top(orientation: Orientation) -> (Direction, Direction) {
    use Direction::{Down, East, North, South, Up, West};
    match orientation {
        Orientation::DownEast => (Down, East),
        Orientation::DownNorth => (Down, North),
        Orientation::DownSouth => (Down, South),
        Orientation::DownWest => (Down, West),
        Orientation::UpEast => (Up, East),
        Orientation::UpNorth => (Up, North),
        Orientation::UpSouth => (Up, South),
        Orientation::UpWest => (Up, West),
        Orientation::WestUp => (West, Up),
        Orientation::EastUp => (East, Up),
        Orientation::NorthUp => (North, Up),
        Orientation::SouthUp => (South, Up),
    }
}

/// The game's `Util.shuffle`.
fn shuffle<T>(list: &mut [T], random: &mut impl Random) {
    for size in (2..=list.len()).rev() {
        let other = random.next_i32_bounded(size as i32) as usize;
        list.swap(size - 1, other);
    }
}

/// The place of a pool by its name.
fn pool_index(name: &str) -> Option<usize> {
    POOLS.binary_search_by(|pool| pool.name.cmp(name)).ok()
}

/// Every pool's elements, each as often as its weight, in the data's order
/// (`StructureTemplatePool.templates`).
fn templates_of(pool: usize) -> &'static [&'static PoolElement] {
    static EXPANDED: OnceLock<Vec<Vec<&'static PoolElement>>> = OnceLock::new();
    &EXPANDED.get_or_init(|| {
        POOLS
            .iter()
            .map(|pool| {
                let mut templates = Vec::new();
                for (element, weight) in pool.elements {
                    for _ in 0..*weight {
                        templates.push(element);
                    }
                }
                templates
            })
            .collect()
    })[pool]
}

/// The tallest element of a pool (`StructureTemplatePool.getMaxSize`).
fn max_size(pool: usize) -> i32 {
    static SIZES: OnceLock<Vec<i32>> = OnceLock::new();
    SIZES.get_or_init(|| {
        (0..POOLS.len())
            .map(|pool| {
                templates_of(pool)
                    .iter()
                    .filter(|element| !matches!(element, PoolElement::Empty))
                    .map(|element| bounding_box(element, IVec3::ZERO, Rotation::None).height())
                    .max()
                    .unwrap_or(0)
            })
            .collect()
    })[pool]
}

/// The size of an element's template.
fn size(element: &PoolElement) -> IVec3 {
    match element {
        PoolElement::Empty | PoolElement::Feature { .. } => IVec3::ZERO,
        PoolElement::Single { template, .. } => clustine_worldgen_data::templates()
            .get(usize::from(*template))
            .size
            .into(),
        PoolElement::List { elements, .. } => {
            elements.iter().map(size).fold(IVec3::ZERO, IVec3::max)
        }
    }
}

/// The box of an element placed with its corner at a position
/// (`StructurePoolElement.getBoundingBox`). The empty element has none; it is never
/// asked.
fn bounding_box(element: &PoolElement, position: IVec3, rotation: Rotation) -> BoundingBox {
    match element {
        PoolElement::Empty | PoolElement::Feature { .. } => BoundingBox::new(position, position),
        PoolElement::Single { .. } => rotation.get_bounding_box(position, size(element)),
        PoolElement::List { elements, .. } => {
            let mut boxes = elements
                .iter()
                .map(|inner| bounding_box(inner, position, rotation));
            let first = boxes.next().unwrap_or(BoundingBox::new(position, position));
            boxes.fold(first, |all, one| BoundingBox::encapsulating(&all, &one))
        }
    }
}

/// The jigsaw blocks of an element placed at a position, shuffled and then ordered
/// by their selection priority, highest first (`getShuffledJigsawBlocks`).
fn shuffled_jigsaws(
    element: &PoolElement,
    position: IVec3,
    rotation: Rotation,
    random: &mut LegacyRandom,
) -> Vec<Jig> {
    match element {
        PoolElement::Empty => Vec::new(),
        // A feature stands on one jigsaw block that faces down and has no name, so
        // that it fits whatever a jigsaw block asks for; nothing is drawn.
        PoolElement::Feature { .. } => vec![Jig {
            pos: position,
            front: Direction::Down,
            top: Direction::South,
            rollable: true,
            name: None,
            target: "minecraft:empty",
            pool: EMPTY_POOL,
            selection_priority: 0,
            placement_priority: 0,
        }],
        PoolElement::List { elements, .. } => match elements.first() {
            Some(first) => shuffled_jigsaws(first, position, rotation, random),
            None => Vec::new(),
        },
        PoolElement::Single { template, .. } => {
            let table = clustine_worldgen_data::templates();
            let template = table.get(usize::from(*template));
            let mut jigsaws: Vec<Jig> = table
                .jigsaws(&template)
                .map(|block| {
                    let (front, top) = front_and_top(block.orientation);
                    Jig {
                        pos: rotation.transform_pos(block.position.into(), IVec3::ZERO) + position,
                        front: rotation.rotate(front),
                        top: rotation.rotate(top),
                        rollable: block.rollable,
                        name: Some(block.name),
                        target: block.target,
                        pool: block.pool,
                        selection_priority: block.selection_priority,
                        placement_priority: block.placement_priority,
                    }
                })
                .collect();
            shuffle(&mut jigsaws, random);
            // The game's sort is stable, as this one is.
            jigsaws.sort_by_key(|jigsaw| std::cmp::Reverse(jigsaw.selection_priority));
            jigsaws
        }
    }
}

/// Whether a jigsaw block may be joined to another (`JigsawBlock.canAttach`).
fn can_attach(source: &Jig, target: &Jig) -> bool {
    source.front == target.front.opposite()
        && (source.rollable || source.top == target.top)
        && target.name.is_none_or(|name| source.target == name)
}

/// One of a weighted list by one draw (`WeightedList.getRandom`).
fn weighted<'a, T>(entries: &'a [(T, i32)], random: &mut impl Random) -> Option<&'a T> {
    let total: i32 = entries.iter().map(|(_, weight)| weight).sum();
    if total == 0 {
        return None;
    }
    let mut drawn = random.next_i32_bounded(total);
    for (entry, weight) in entries {
        drawn -= weight;
        if drawn < 0 {
            return Some(entry);
        }
    }
    None
}

/// The renamings of pools for one start (`PoolAliasLookup.create`), drawn from the
/// world's seed and the start's position.
fn resolve_aliases(
    aliases: &'static [PoolAlias],
    seed: i64,
    position: IVec3,
) -> BTreeMap<&'static str, &'static str> {
    let mut resolved = BTreeMap::new();
    if aliases.is_empty() {
        return resolved;
    }
    let mut source = LegacyRandom::from_seed(seed as u64);
    let mut random = source
        .next_positional()
        .at(position.x, position.y, position.z);
    fn bind(
        alias: &PoolAlias,
        random: &mut impl Random,
        resolved: &mut BTreeMap<&'static str, &'static str>,
    ) {
        match alias {
            PoolAlias::Direct { alias, target } => {
                resolved.insert(alias, target);
            }
            PoolAlias::Random { alias, targets } => {
                if let Some(target) = weighted(targets, random) {
                    resolved.insert(alias, target);
                }
            }
            PoolAlias::RandomGroup { groups } => {
                if let Some(group) = weighted(groups, random) {
                    for inner in *group {
                        bind(inner, random, resolved);
                    }
                }
            }
        }
    }
    for alias in aliases {
        bind(alias, &mut random, &mut resolved);
    }
    resolved
}

/// The space a piece may be placed in: a box, less the boxes placed in it.
struct FreeSpace {
    bounds: BoundingBox,
    taken: Vec<BoundingBox>,
}

impl FreeSpace {
    /// Whether a box lies wholly in what is free. The game asks it of shapes, with
    /// the box shrunk by a quarter of a block, which for boxes of whole blocks is
    /// this.
    fn has_room_for(&self, candidate: &BoundingBox) -> bool {
        let inside = candidate.min_x() >= self.bounds.min_x()
            && candidate.max_x() <= self.bounds.max_x()
            && candidate.min_y() >= self.bounds.min_y()
            && candidate.max_y() <= self.bounds.max_y()
            && candidate.min_z() >= self.bounds.min_z()
            && candidate.max_z() <= self.bounds.max_z();
        inside && !self.taken.iter().any(|taken| taken.intersects(*candidate))
    }
}

/// A piece as the assembly holds it.
struct Placed {
    element: &'static PoolElement,
    position: IVec3,
    rotation: Rotation,
    bounding_box: BoundingBox,
    ground_level_delta: i32,
    junctions: Vec<JigsawJunction>,
}

/// A piece whose jigsaw blocks are still to be looked at.
struct Waiting {
    piece: usize,
    free: usize,
    depth: i32,
}

/// The game's `SequencedPriorityIterator`: the highest priority first, and within
/// one priority in the order of arrival.
#[derive(Default)]
struct Queue {
    by_priority: BTreeMap<i32, std::collections::VecDeque<Waiting>>,
}

impl Queue {
    fn add(&mut self, waiting: Waiting, priority: i32) {
        self.by_priority
            .entry(priority)
            .or_default()
            .push_back(waiting);
    }

    fn next(&mut self) -> Option<Waiting> {
        let mut highest = self.by_priority.last_entry()?;
        let waiting = highest.get_mut().pop_front();
        if highest.get().is_empty() {
            highest.remove();
        }
        waiting
    }
}

struct Placer<'a> {
    config: &'static Jigsaw,
    aliases: &'a BTreeMap<&'static str, &'static str>,
    pieces: Vec<Placed>,
    free_spaces: Vec<FreeSpace>,
    queue: Queue,
}

impl Placer<'_> {
    fn lookup(&self, pool: &'static str) -> &'static str {
        self.aliases.get(pool).copied().unwrap_or(pool)
    }

    /// The game's `Placer.tryPlacingChildren`.
    fn try_placing_children(
        &mut self,
        waiting: &Waiting,
        ctx: &mut dyn StructureGenerationContext,
        random: &mut LegacyRandom,
    ) {
        let piece = waiting.piece;
        let element = self.pieces[piece].element;
        let position = self.pieces[piece].position;
        let rotation = self.pieces[piece].rotation;
        let projection = element.projection();
        let rigid = projection == Projection::Rigid;
        let piece_box = self.pieces[piece].bounding_box;
        let min_y = piece_box.min_y();
        // The free space inside this piece, made when a child is first placed there.
        let mut inner_free: Option<usize> = None;

        'jigsaws: for jigsaw in shuffled_jigsaws(element, position, rotation, random) {
            let target_pos = jigsaw.pos + jigsaw.front.offset_vec();
            let jigsaw_y = jigsaw.pos.y - min_y;
            let mut surface: Option<i32> = None;
            let Some(pool) = pool_index(self.lookup(jigsaw.pool)) else {
                continue;
            };
            if templates_of(pool).is_empty() && POOLS[pool].name != EMPTY_POOL {
                continue;
            }
            let fallback = usize::from(POOLS[pool].fallback);
            let free = if piece_box.contains(target_pos) {
                *inner_free.get_or_insert_with(|| {
                    self.free_spaces.push(FreeSpace {
                        bounds: piece_box,
                        taken: Vec::new(),
                    });
                    self.free_spaces.len() - 1
                })
            } else {
                waiting.free
            };

            let mut candidates: Vec<&'static PoolElement> = Vec::new();
            if waiting.depth != self.config.max_depth {
                let mut templates = templates_of(pool).to_vec();
                shuffle(&mut templates, random);
                candidates.extend(templates);
            }
            let mut templates = templates_of(fallback).to_vec();
            shuffle(&mut templates, random);
            candidates.extend(templates);

            for candidate in candidates {
                if matches!(candidate, PoolElement::Empty) {
                    break;
                }
                for turned in Rotation::get_shuffled(random) {
                    let targets = shuffled_jigsaws(candidate, IVec3::ZERO, turned, random);
                    let at_origin = bounding_box(candidate, IVec3::ZERO, turned);
                    // The expansion hack: a low piece is given the height of the
                    // tallest piece that its own inward jigsaw blocks could take.
                    let expand_to = if self.config.use_expansion_hack && at_origin.height() <= 16 {
                        targets
                            .iter()
                            .map(|target| {
                                if !at_origin.contains(target.pos + target.front.offset_vec()) {
                                    return 0;
                                }
                                match pool_index(self.lookup(target.pool)) {
                                    Some(pool) => max_size(pool)
                                        .max(max_size(usize::from(POOLS[pool].fallback))),
                                    None => 0,
                                }
                            })
                            .max()
                            .unwrap_or(0)
                    } else {
                        0
                    };

                    for target in &targets {
                        if !can_attach(&jigsaw, target) {
                            continue;
                        }
                        let delta = target_pos - target.pos;
                        let candidate_box = bounding_box(candidate, delta, turned);
                        let candidate_projection = candidate.projection();
                        let candidate_rigid = candidate_projection == Projection::Rigid;
                        let target_y = target.pos.y;
                        let relative_y = jigsaw_y - target_y + jigsaw.front.offset().1;
                        let new_min_y = if rigid && candidate_rigid {
                            min_y + relative_y
                        } else {
                            let surface = *surface.get_or_insert_with(|| {
                                ctx.base_height(jigsaw.pos.x, jigsaw.pos.z, false)
                            });
                            surface - target_y
                        };
                        let dy = new_min_y - candidate_box.min_y();
                        let mut moved = candidate_box.translate(IVec3::new(0, dy, 0));
                        let moved_position = delta + IVec3::new(0, dy, 0);
                        if expand_to > 0 {
                            let reach = (expand_to + 1).max(moved.max_y() - moved.min_y());
                            let top =
                                IVec3::new(moved.min_x(), moved.min_y() + reach, moved.min_z());
                            moved = BoundingBox::encapsulating(&moved, &BoundingBox::new(top, top));
                        }
                        if !self.free_spaces[free].has_room_for(&moved) {
                            continue;
                        }
                        self.free_spaces[free].taken.push(moved);

                        let ground_delta = self.pieces[piece].ground_level_delta;
                        let candidate_ground_delta = if candidate_rigid {
                            ground_delta - relative_y
                        } else {
                            1
                        };
                        let junction_y = if rigid {
                            min_y + jigsaw_y
                        } else if candidate_rigid {
                            new_min_y + target_y
                        } else {
                            let surface = *surface.get_or_insert_with(|| {
                                ctx.base_height(jigsaw.pos.x, jigsaw.pos.z, false)
                            });
                            surface + relative_y / 2
                        };
                        self.pieces[piece].junctions.push(JigsawJunction {
                            source_pos: IVec3::new(
                                target_pos.x,
                                junction_y - jigsaw_y + ground_delta,
                                target_pos.z,
                            ),
                            delta_y: relative_y,
                            dest_projection: candidate_projection,
                        });
                        self.pieces.push(Placed {
                            element: candidate,
                            position: moved_position,
                            rotation: turned,
                            bounding_box: moved,
                            ground_level_delta: candidate_ground_delta,
                            junctions: vec![JigsawJunction {
                                source_pos: IVec3::new(
                                    jigsaw.pos.x,
                                    junction_y - target_y + candidate_ground_delta,
                                    jigsaw.pos.z,
                                ),
                                delta_y: -relative_y,
                                dest_projection: projection,
                            }],
                        });
                        if waiting.depth < self.config.max_depth {
                            self.queue.add(
                                Waiting {
                                    piece: self.pieces.len() - 1,
                                    free,
                                    depth: waiting.depth + 1,
                                },
                                jigsaw.placement_priority,
                            );
                        }
                        continue 'jigsaws;
                    }
                }
            }
        }
    }
}

/// A height from a provider (`HeightProvider.sample`), in terrain whose generator
/// fills `depth` blocks from `min_y`.
pub(crate) fn sample_height(
    provider: &HeightProvider,
    random: &mut impl Random,
    min_y: i32,
    depth: i32,
) -> i32 {
    match provider {
        HeightProvider::Constant(anchor) => anchor.resolve(min_y, depth),
        HeightProvider::Uniform {
            min_inclusive,
            max_inclusive,
        } => {
            let low = min_inclusive.resolve(min_y, depth);
            let high = max_inclusive.resolve(min_y, depth);
            if low > high {
                low
            } else {
                random.next_i32_between(low, high)
            }
        }
    }
}

/// The start of a jigsaw structure (`JigsawStructure.findGenerationPoint` and
/// `JigsawPlacement.addPieces`).
pub struct JigsawStructure;

impl Structure for JigsawStructure {
    fn find_generation_point(
        &self,
        ctx: &mut dyn StructureGenerationContext,
        structure: &StructureData,
        rng: &mut LegacyRandom,
    ) -> Option<GenerationStub> {
        let StructureConfigData::Jigsaw(config) = structure.config else {
            return None;
        };
        let start_y = sample_height(&config.start_height, rng, ctx.min_y(), ctx.height());
        let position = IVec3::new(ctx.chunk_min_x(), start_y, ctx.chunk_min_z());
        let aliases = resolve_aliases(config.pool_aliases, ctx.seed(), position);

        let rotation = Rotation::get_random(rng);
        let start_pool = usize::from(config.start_pool);
        let pool = aliases
            .get(POOLS[start_pool].name)
            .and_then(|name| pool_index(name))
            .unwrap_or(start_pool);
        let templates = templates_of(pool);
        if templates.is_empty() {
            return None;
        }
        let element = templates[rng.next_i32_bounded(templates.len() as i32) as usize];
        if matches!(element, PoolElement::Empty) {
            return None;
        }
        let anchor = match config.start_jigsaw_name {
            Some(name) => {
                shuffled_jigsaws(element, position, rotation, rng)
                    .into_iter()
                    .find(|jigsaw| jigsaw.name == Some(name))?
                    .pos
            }
            None => position,
        };
        let anchor_delta = anchor - position;
        let adjusted = position - anchor_delta;
        let mut start_box = bounding_box(element, adjusted, rotation);
        let center_x = (start_box.max_x() + start_box.min_x()) / 2;
        let center_z = (start_box.max_z() + start_box.min_z()) / 2;
        let bottom_y = match config.project_start_to_heightmap {
            None => adjusted.y,
            Some(heightmap) => {
                position.y
                    + ctx.base_height(center_x, center_z, heightmap == Heightmap::OceanFloorWg)
            }
        };
        // An element stands one block into the ground (`getGroundLevelDelta`).
        let ground_level_delta = 1;
        let lift = bottom_y - (start_box.min_y() + ground_level_delta);
        start_box = start_box.translate(IVec3::new(0, lift, 0));
        let start_position = adjusted + IVec3::new(0, lift, 0);
        if config.padding_bottom != 0 || config.padding_top != 0 {
            let lowest = ctx.level_min_y() + config.padding_bottom;
            let highest = ctx.level_max_y() - 1 - config.padding_top;
            if start_box.min_y() < lowest || start_box.max_y() > highest {
                return None;
            }
        }
        let start_y = bottom_y + anchor_delta.y;

        // The game asks the biome here, before any piece but the first is made.
        let biome = ctx.biome_at(center_x, start_y, center_z);
        if !structure.allowed_biomes.contains(&biome.key) {
            return None;
        }

        let mut placer = Placer {
            config,
            aliases: &aliases,
            pieces: vec![Placed {
                element,
                position: start_position,
                rotation,
                bounding_box: start_box,
                ground_level_delta,
                junctions: Vec::new(),
            }],
            free_spaces: Vec::new(),
            queue: Queue::default(),
        };
        if config.max_depth > 0 {
            let (sideways, upwards) =
                (config.max_distance_horizontal, config.max_distance_vertical);
            let bounds = BoundingBox::new(
                IVec3::new(
                    center_x - sideways,
                    (start_y - upwards).max(ctx.level_min_y() + config.padding_bottom),
                    center_z - sideways,
                ),
                IVec3::new(
                    center_x + sideways,
                    (start_y + upwards + 1).min(ctx.level_max_y() - config.padding_top) - 1,
                    center_z + sideways,
                ),
            );
            placer.free_spaces.push(FreeSpace {
                bounds,
                taken: vec![start_box],
            });
            placer.try_placing_children(
                &Waiting {
                    piece: 0,
                    free: 0,
                    depth: 0,
                },
                ctx,
                rng,
            );
            while let Some(waiting) = placer.queue.next() {
                placer.try_placing_children(&waiting, ctx, rng);
            }
        } else {
            // Without depth the game adds no piece at all, not even the first.
            placer.pieces.clear();
        }

        let pieces = placer
            .pieces
            .into_iter()
            .map(|piece| StructurePiece {
                piece_type: Identifier::new_static("minecraft", "jigsaw"),
                bounding_box: piece.bounding_box,
                gen_depth: 0,
                orientation: None,
                payload: StructurePiecePayload::Jigsaw(JigsawPieceData {
                    pool_element: piece.element,
                    position: piece.position,
                    rotation: piece.rotation,
                    liquid_settings: config.liquid_settings,
                }),
                ground_level_delta: piece.ground_level_delta,
                junctions: piece.junctions,
                projection: Some(piece.element.projection()),
            })
            .collect();
        Some(GenerationStub {
            position: (center_x, start_y, center_z),
            pieces,
        })
    }
}
