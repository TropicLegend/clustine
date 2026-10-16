// Adapted from SteelMC's `steel-core/src/worldgen/template/placement.rs`.

//! Placing a template's blocks in the world as the game's
//! `StructureTemplate.placeInWorld` does: a palette chosen, every block moved to its
//! place, turned and mirrored, put through the processors (`processBlockInfos`), clipped
//! to the box, placed with its block entity's data, waterlogged from the fluid it
//! replaces, and, unless the shape is known, the shapes at the edges brought up to date.
//!
//! What it does not do yet, and why:
//!
//! - Entities are not made (`addEntitiesToWorld`): Clustine has no entities. They are
//!   kept in [`TemplateBlocks::entities`] for when it has.
//! - A block entity's data is the template's, with its `id`, `x`, `y` and `z` taken out,
//!   the modifier of a rule applied, a loot seed drawn for a container, and an empty
//!   `components` where it had none. The game loads that data into the block entity and
//!   saves it again, and each type saves what it keeps, in its own way (a chest whose
//!   loot is still to be rolled drops its `Items`, a campfire writes its `Items` even
//!   when the template has none). Those savers are each type's code and are not ported:
//!   the members are the template's, in the order a Java `HashMap` writes them.
//! - Shapes: where the shape is not known (fossils, the template feature), the game asks
//!   every placed block and its neighbours at the edges for its shape
//!   (`updateShapeAtEdge`, `Block.updateFromNeighbourShapes`). Each block's
//!   `updateShape` is its class's code, which is phase B's. The walk over the faces is
//!   ported, and each question goes to a [`ShapeUpdates`]; the one given here,
//!   [`FluidTicks`], keeps every state and schedules the ticks of fluids that the
//!   game's water, lava and waterlogged blocks schedule there. Stairs, fences, walls,
//!   panes and the like do not join up with what is beside them, and the sulfur spikes
//!   of the sulfur springs keep the thickness the template gives them.
//! - The game's `updateNeighborsAt` does nothing while features are placed, and
//!   `setChanged` nothing that shows.

use std::collections::BTreeSet;

use clustine_data::{BlockClass, BlockState, blocks};
use clustine_noise::Random;
use clustine_protocol::nbt::Nbt;
use clustine_world::generation::Neighbourhood;
use clustine_world::{BlockEntity, BlockPos, InChunk, ScheduledTick, Ticked};
use clustine_worldgen_data::structure::LiquidSettings;

use crate::blocks::{block_entity_type, is_class, java_ordered, nbt_bytes, put, remove};
use crate::geometry::{BoundingBox, Direction, IVec3, Rotation};
use crate::piece::StructureMirror;
use crate::processors::{BlockInfo, PlaceRandom, Processing, Processor, position_random};
use crate::state_transform::transform_state;
use crate::template_blocks::{TemplateBlock, TemplateBlocks};

/// The game's `StructurePlaceSettings`, with what `placeInWorld` reads of the level
/// beside it.
pub struct StructurePlaceSettings<'a> {
    pub mirror: StructureMirror,
    pub rotation: Rotation,
    /// The point the template turns about, in the template.
    pub rotation_pivot: BlockPos,
    /// Blocks outside it are not placed; `None` places all.
    pub bounding_box: Option<BoundingBox>,
    /// In the order they were added (`addProcessor`); the caller adds those the game's
    /// caller adds, such as [`Processor::structure_block_ignore`].
    pub processors: Vec<&'a Processor>,
    pub liquid_settings: LiquidSettings,
    /// Entities are not made in any case (see the module's head); kept for the caller.
    pub ignore_entities: bool,
    /// Whether the shapes at the edges are left as they are; pieces of jigsaw
    /// structures say so, fossils and the template feature do not.
    pub known_shape: bool,
    pub finalize_entities: bool,
    /// Where `getRandom` draws from.
    pub random: PlaceRandom,
    /// The world's seed (`level.getSeed()`), which `capped` draws from.
    pub world_seed: i64,
    /// What a block's shape becomes beside another.
    pub shapes: &'a dyn ShapeUpdates,
    /// Whether placed blocks are marked for post-processing: the game's `setBlock`
    /// marks only where its flags lack `UPDATE_KNOWN_SHAPE` (16). Pieces of jigsaw
    /// structures are placed with the flags 18 and mark nothing; fossils and the
    /// template feature mark.
    pub marks: bool,
}

impl<'a> StructurePlaceSettings<'a> {
    /// The game's defaults: no turn, no mirror, no box, no processor, waterlogging,
    /// each position's own random source.
    pub fn new(world_seed: i64) -> Self {
        Self {
            mirror: StructureMirror::None,
            rotation: Rotation::None,
            rotation_pivot: BlockPos::new(0, 0, 0),
            bounding_box: None,
            processors: Vec::new(),
            liquid_settings: LiquidSettings::ApplyWaterlogging,
            ignore_entities: false,
            known_shape: false,
            finalize_entities: false,
            random: PlaceRandom::Positional,
            world_seed,
            shapes: &OVERWORLD_FLUID_TICKS,
            marks: true,
        }
    }

    fn contains(&self, pos: BlockPos) -> bool {
        self.bounding_box
            .is_none_or(|bounds| bounds.contains_xyz(pos.x, pos.y, pos.z))
    }
}

/// A block's `updateShape`: what `state` at `at` becomes beside `neighbour`, which lies
/// in `direction`. It may schedule ticks, as the game's do.
pub trait ShapeUpdates {
    fn update_shape(
        &self,
        world: &mut dyn Neighbourhood,
        at: BlockPos,
        state: BlockState,
        direction: Direction,
        neighbour_at: BlockPos,
        neighbour: BlockState,
    ) -> BlockState;
}

/// Shapes as they are, with the ticks of fluids: a water or lava block schedules its
/// fluid where it or its neighbour is a source (`LiquidBlock.updateShape`), a
/// waterlogged block schedules water (`SimpleWaterloggedBlock`'s blocks).
#[derive(Debug, Clone, Copy)]
pub struct FluidTicks {
    /// Thirty ticks, or ten where lava is fast (the Nether).
    pub lava_tick_delay: i32,
}

/// [`FluidTicks`] where lava is slow.
pub static OVERWORLD_FLUID_TICKS: FluidTicks = FluidTicks {
    lava_tick_delay: 30,
};

/// The ticks water waits.
const WATER_TICK_DELAY: i32 = 5;
/// `Fluids.WATER`, the still water, by its id in `clustine_data::FLUIDS`.
const WATER: u8 = 2;

impl FluidTicks {
    fn delay(&self, fluid: u8) -> i32 {
        if fluid <= WATER {
            WATER_TICK_DELAY
        } else {
            self.lava_tick_delay
        }
    }
}

impl ShapeUpdates for FluidTicks {
    fn update_shape(
        &self,
        world: &mut dyn Neighbourhood,
        at: BlockPos,
        state: BlockState,
        _: Direction,
        _: BlockPos,
        neighbour: BlockState,
    ) -> BlockState {
        if is_class(state, BlockClass::LiquidBlock) {
            if let Some(fluid) = state.props().fluid()
                && (fluid.source || neighbour.props().fluid().is_some_and(|other| other.source))
            {
                schedule_fluid(world, at, fluid.fluid, self.delay(fluid.fluid));
            }
        } else if state.property("waterlogged") == Some("true") {
            schedule_fluid(world, at, WATER, WATER_TICK_DELAY);
        }
        state
    }
}

/// `scheduleTick` of a fluid, at the game's normal priority, as a chunk being
/// generated keeps it: without the delay it was asked for (`ProtoChunkTicks.schedule`
/// saves every tick with a delay of nought).
fn schedule_fluid(world: &mut dyn Neighbourhood, at: BlockPos, fluid: u8, _delay: i32) {
    let (x, z) = at.in_chunk();
    world.schedule(
        at,
        ScheduledTick {
            at: InChunk::new(x, at.y, z),
            of: Ticked::Fluid(fluid),
            delay: 0,
            priority: 0,
        },
    );
}

/// The game's `StructureTemplate.transform`: a position in the template mirrored, then
/// turned about the pivot.
pub fn transform_pos(
    pos: BlockPos,
    mirror: StructureMirror,
    rotation: Rotation,
    pivot: BlockPos,
) -> BlockPos {
    let (x, z) = match mirror {
        StructureMirror::LeftRight => (pos.x, -pos.z),
        StructureMirror::FrontBack => (-pos.x, pos.z),
        StructureMirror::None => (pos.x, pos.z),
    };
    let turned = rotation.transform_pos(IVec3::new(x, pos.y, z), IVec3::new(pivot.x, 0, pivot.z));
    BlockPos::new(turned.x, turned.y, turned.z)
}

/// The box a template of `size` covers placed at `at` (`StructureTemplate.getBoundingBox`).
pub fn template_bounding_box(
    size: [i32; 3],
    at: BlockPos,
    mirror: StructureMirror,
    rotation: Rotation,
    pivot: BlockPos,
) -> BoundingBox {
    let a = transform_pos(BlockPos::new(0, 0, 0), mirror, rotation, pivot);
    let b = transform_pos(
        BlockPos::new(size[0] - 1, size[1] - 1, size[2] - 1),
        mirror,
        rotation,
        pivot,
    );
    BoundingBox::new(
        IVec3::new(a.x + at.x, a.y + at.y, a.z + at.z),
        IVec3::new(b.x + at.x, b.y + at.y, b.z + at.z),
    )
}

/// The game's `processBlockInfos`: every block of `blocks` moved to its place in the
/// world and put through the processors, then the processors' `finalizeProcessing`.
/// Blocks a processor drops are left out; a block outside the box is still processed,
/// as in the game, which draws for it.
pub fn process_block_infos(
    world: &mut dyn Neighbourhood,
    position: BlockPos,
    reference: BlockPos,
    settings: &StructurePlaceSettings<'_>,
    random: &mut dyn Random,
    blocks: &[TemplateBlock],
) -> Vec<BlockInfo> {
    let mut cx = Processing {
        world,
        position,
        reference,
        random_kind: settings.random,
        random,
        world_seed: settings.world_seed,
    };
    let mut originals = Vec::with_capacity(blocks.len());
    let mut processed = Vec::with_capacity(blocks.len());
    for block in blocks {
        let at = transform_pos(
            block.pos,
            settings.mirror,
            settings.rotation,
            settings.rotation_pivot,
        );
        let original = BlockInfo {
            pos: block.pos,
            state: block.state,
            nbt: block.nbt.clone(),
        };
        let mut info = Some(BlockInfo {
            pos: BlockPos::new(at.x + position.x, at.y + position.y, at.z + position.z),
            state: block.state,
            nbt: block.nbt.clone(),
        });
        for processor in &settings.processors {
            let Some(current) = info else { break };
            info = processor.process_block(&mut cx, &original, current);
        }
        if let Some(info) = info {
            processed.push(info);
            originals.push(original);
        }
    }
    for processor in &settings.processors {
        processed = processor.finalize(&mut cx, &originals, processed);
    }
    processed
}

/// The game's `StructureTemplate.placeInWorld`: places `template` with its corner at
/// `at`, position tests measured from `reference`. `random` is the source the game's
/// caller passes, which seeds containers' loot and, where `settings.random` says so,
/// the palette and the processors. False where the template has nothing to place.
pub fn place_in_world(
    template: &TemplateBlocks,
    world: &mut dyn Neighbourhood,
    at: BlockPos,
    reference: BlockPos,
    settings: &StructurePlaceSettings<'_>,
    random: &mut dyn Random,
) -> bool {
    if template.palettes.is_empty() {
        return false;
    }
    // `getRandomPalette`: a draw even where there is one palette.
    let count = i32::try_from(template.palettes.len()).expect("fewer than 2^31 palettes");
    let index = match settings.random {
        PlaceRandom::Given => random.next_i32_bounded(count),
        PlaceRandom::Positional => position_random(at).next_i32_bounded(count),
    };
    let blocks = &template.palettes[index as usize];
    let nothing = blocks.is_empty() && (settings.ignore_entities || template.entities.is_empty());
    if nothing || template.size.iter().any(|&side| side < 1) {
        return false;
    }

    let apply_waterlogging = settings.liquid_settings == LiquidSettings::ApplyWaterlogging;
    let mut to_fill: Vec<BlockPos> = Vec::new();
    let mut locked_fluids: Vec<BlockPos> = Vec::new();
    let mut placed: Vec<BlockPos> = Vec::with_capacity(blocks.len());
    let infos = process_block_infos(world, at, reference, settings, random, blocks);
    for info in infos {
        let pos = info.pos;
        if !settings.contains(pos) {
            continue;
        }
        let previous_fluid = apply_waterlogging.then(|| world.block(pos).props().fluid());
        let state = transform_state(info.state, settings.mirror, settings.rotation);
        if info.nbt.is_some() {
            // A fresh block entity: whatever stood there goes with the barrier.
            // The game's flags here (820) hold `UPDATE_KNOWN_SHAPE`.
            set_block(world, pos, blocks::BARRIER, false);
        }
        if !set_block(world, pos, state, settings.marks) {
            continue;
        }
        placed.push(pos);
        if let Some(nbt) = info.nbt
            && let Some(kind) = state.block_info().and_then(|info| info.block_entity_type)
        {
            let seed = randomizable(kind).then(|| random.next_i64());
            let data = saved_data(kind, nbt, seed);
            let _ = world.set_block_entity(
                pos,
                Some(BlockEntity {
                    kind,
                    data: nbt_bytes(&data),
                }),
            );
        }
        if let Some(previous) = previous_fluid {
            if state.props().fluid().is_some_and(|fluid| fluid.source) {
                locked_fluids.push(pos);
            } else if is_liquid_container(state) {
                place_liquid(world, pos, state, previous);
                if !previous.is_some_and(|fluid| fluid.source) {
                    to_fill.push(pos);
                }
            }
        }
    }

    fill_from_neighbours(world, &mut to_fill, &locked_fluids);

    if !placed.is_empty() && !settings.known_shape {
        update_shape_at_edge(world, settings.shapes, &placed, settings.marks);
        for &pos in &placed {
            let state = world.block(pos);
            let mut new = state;
            for direction in UPDATE_SHAPE_ORDER {
                let (dx, dy, dz) = direction.offset();
                let neighbour_at = pos.offset(dx, dy, dz);
                let neighbour = world.block(neighbour_at);
                new = settings.shapes.update_shape(
                    world,
                    pos,
                    new,
                    direction,
                    neighbour_at,
                    neighbour,
                );
            }
            if new != state {
                // The game sets these with `UPDATE_KNOWN_SHAPE` added to its flags.
                set_block(world, pos, new, false);
            }
        }
    }
    true
}

/// `Block.UPDATE_SHAPE_ORDER`.
const UPDATE_SHAPE_ORDER: [Direction; 6] = [
    Direction::West,
    Direction::East,
    Direction::North,
    Direction::South,
    Direction::Down,
    Direction::Up,
];

/// `WorldGenRegion.setBlock`: false where the view refuses the write; where `marks` (the
/// flags lack `UPDATE_KNOWN_SHAPE`), a state that asks for it marks its place, or the
/// one above it, for post-processing.
fn set_block(world: &mut dyn Neighbourhood, at: BlockPos, state: BlockState, marks: bool) -> bool {
    if !world.set_block(at, state) {
        return false;
    }
    if marks && let Some([dx, dy, dz]) = state.props().post_process() {
        world.mark(at.offset(i32::from(dx), i32::from(dy), i32::from(dz)));
    }
    true
}

/// Whether the block entity type keeps loot to be rolled (`RandomizableContainer`):
/// placing one draws a seed for it, whether it names a loot table or not.
fn randomizable(kind: u8) -> bool {
    [
        "minecraft:chest",
        "minecraft:trapped_chest",
        "minecraft:barrel",
        "minecraft:dispenser",
        "minecraft:dropper",
        "minecraft:hopper",
        "minecraft:shulker_box",
        "minecraft:crafter",
        "minecraft:decorated_pot",
    ]
    .iter()
    .any(|name| block_entity_type(name) == Some(kind))
}

/// The block entity's data as Clustine keeps it (see the module's head): the template's
/// compound without `id`, `x`, `y` and `z`, the drawn seed where it names a loot table
/// (the game saves no seed of nought, and none without a table), and `components`.
fn saved_data(kind: u8, nbt: Nbt, seed: Option<i64>) -> Nbt {
    let Nbt::Compound(mut members) = nbt else {
        return Nbt::Compound(vec![("components".into(), Nbt::Compound(Vec::new()))]);
    };
    for key in ["id", "x", "y", "z"] {
        remove(&mut members, key);
    }
    let mut members = crate::resave::resave(
        clustine_data::BLOCK_ENTITY_TYPES[usize::from(kind)],
        members,
    );
    if let Some(seed) = seed {
        put(&mut members, "LootTableSeed", Nbt::Long(seed));
    }
    let has_table = members.iter().any(|(key, _)| key == "LootTable");
    if !has_table
        || members
            .iter()
            .any(|(key, value)| key == "LootTableSeed" && *value == Nbt::Long(0))
    {
        remove(&mut members, "LootTableSeed");
    }
    if !members.iter().any(|(key, _)| key == "components") {
        members.push(("components".into(), Nbt::Compound(Vec::new())));
    }
    java_ordered(Nbt::Compound(members))
}

/// Whether the block takes a fluid in (`LiquidBlockContainer`): every waterloggable
/// block, and kelp and seagrass, which take none.
fn is_liquid_container(state: BlockState) -> bool {
    state.property("waterlogged").is_some()
        || is_class(state, BlockClass::KelpBlock)
        || is_class(state, BlockClass::KelpPlantBlock)
        || is_class(state, BlockClass::SeagrassBlock)
        || is_class(state, BlockClass::TallSeagrassBlock)
}

/// `LiquidBlockContainer.placeLiquid`: a waterloggable block not yet waterlogged takes
/// still water, and the water is scheduled; a lit campfire or candle goes out.
fn place_liquid(
    world: &mut dyn Neighbourhood,
    at: BlockPos,
    state: BlockState,
    fluid: Option<clustine_data::FluidProps>,
) -> bool {
    // A double slab holds no water (`SlabBlock.canPlaceLiquid`).
    if state.property("waterlogged") != Some("false")
        || state.property("type") == Some("double") && is_class(state, BlockClass::SlabBlock)
        || fluid.is_none_or(|fluid| fluid.fluid != WATER)
    {
        return false;
    }
    let mut new = state
        .with_property("waterlogged", "true")
        .expect("a waterloggable block can be waterlogged");
    if (is_class(state, BlockClass::CampfireBlock)
        || is_class(state, BlockClass::AbstractCandleBlock))
        && let Some(out) = new.with_property("lit", "false")
    {
        new = out;
    }
    // `LiquidBlockContainer.placeLiquid` sets with the flags 3, which mark.
    set_block(world, at, new, true);
    schedule_fluid(world, at, WATER, WATER_TICK_DELAY);
    true
}

/// The game's loop after the blocks are placed: a container still dry takes a source
/// from above or beside it that the template did not place, again while one does.
fn fill_from_neighbours(
    world: &mut dyn Neighbourhood,
    to_fill: &mut Vec<BlockPos>,
    locked: &[BlockPos],
) {
    const DIRECTIONS: [Direction; 5] = [
        Direction::Up,
        Direction::North,
        Direction::East,
        Direction::South,
        Direction::West,
    ];
    let mut filled = true;
    while filled && !to_fill.is_empty() {
        filled = false;
        let mut index = 0;
        while index < to_fill.len() {
            let pos = to_fill[index];
            let mut to_place = world.block(pos).props().fluid();
            for direction in DIRECTIONS {
                if to_place.is_some_and(|fluid| fluid.source) {
                    break;
                }
                let (dx, dy, dz) = direction.offset();
                let neighbour_at = pos.offset(dx, dy, dz);
                let neighbour = world.block(neighbour_at).props().fluid();
                if neighbour.is_some_and(|fluid| fluid.source) && !locked.contains(&neighbour_at) {
                    to_place = neighbour;
                }
            }
            if to_place.is_some_and(|fluid| fluid.source) {
                let state = world.block(pos);
                if is_liquid_container(state) {
                    place_liquid(world, pos, state, to_place);
                    filled = true;
                    to_fill.remove(index);
                    continue;
                }
            }
            index += 1;
        }
    }
}

/// `StructureTemplate.updateShapeAtEdge` over the shape the placed blocks fill: for
/// every face of it, in the order of `DiscreteVoxelShape.forAllFaces`, the block inside
/// and the one outside each take their shape beside the other.
fn update_shape_at_edge(
    world: &mut dyn Neighbourhood,
    shapes: &dyn ShapeUpdates,
    placed: &[BlockPos],
    marks: bool,
) {
    let min = placed.iter().fold((i32::MAX, i32::MAX, i32::MAX), |m, p| {
        (m.0.min(p.x), m.1.min(p.y), m.2.min(p.z))
    });
    let max = placed.iter().fold((i32::MIN, i32::MIN, i32::MIN), |m, p| {
        (m.0.max(p.x), m.1.max(p.y), m.2.max(p.z))
    });
    let filled: BTreeSet<(i32, i32, i32)> = placed
        .iter()
        .map(|p| (p.x - min.0, p.y - min.1, p.z - min.2))
        .collect();
    let size = (max.0 - min.0 + 1, max.1 - min.1 + 1, max.2 - min.2 + 1);
    let mut faces = Vec::new();
    for_all_faces(
        size,
        |x, y, z| filled.contains(&(x, y, z)),
        |direction, x, y, z| {
            faces.push((direction, x, y, z));
        },
    );
    for (direction, x, y, z) in faces {
        let pos = BlockPos::new(min.0 + x, min.1 + y, min.2 + z);
        let (dx, dy, dz) = direction.offset();
        let neighbour_at = pos.offset(dx, dy, dz);
        let state = world.block(pos);
        let neighbour = world.block(neighbour_at);
        let new = shapes.update_shape(world, pos, state, direction, neighbour_at, neighbour);
        if new != state {
            set_block(world, pos, new, marks);
        }
        let new_neighbour = shapes.update_shape(
            world,
            neighbour_at,
            neighbour,
            direction.opposite(),
            pos,
            new,
        );
        if new_neighbour != neighbour {
            set_block(world, neighbour_at, new_neighbour, marks);
        }
    }
}

/// `DiscreteVoxelShape.forAllFaces`: the faces along z (north, south), then along y
/// (down, up), then along x (west, east), each axis walked as the game's `AxisCycle`
/// walks it.
fn for_all_faces(
    (x_size, y_size, z_size): (i32, i32, i32),
    full: impl Fn(i32, i32, i32) -> bool,
    mut face: impl FnMut(Direction, i32, i32, i32),
) {
    for x in 0..x_size {
        for y in 0..y_size {
            let mut last = false;
            for z in 0..=z_size {
                let now = z != z_size && full(x, y, z);
                if !last && now {
                    face(Direction::North, x, y, z);
                }
                if last && !now {
                    face(Direction::South, x, y, z - 1);
                }
                last = now;
            }
        }
    }
    for z in 0..z_size {
        for x in 0..x_size {
            let mut last = false;
            for y in 0..=y_size {
                let now = y != y_size && full(x, y, z);
                if !last && now {
                    face(Direction::Down, x, y, z);
                }
                if last && !now {
                    face(Direction::Up, x, y - 1, z);
                }
                last = now;
            }
        }
    }
    for y in 0..y_size {
        for z in 0..z_size {
            let mut last = false;
            for x in 0..=x_size {
                let now = x != x_size && full(x, y, z);
                if !last && now {
                    face(Direction::West, x, y, z);
                }
                if last && !now {
                    face(Direction::East, x - 1, y, z);
                }
                last = now;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::FLUIDS;
    use clustine_noise::WorldgenRandom;
    use clustine_protocol::codec::Reader as NbtReader;

    use super::*;
    use crate::template_blocks::parse_template;
    use crate::template_blocks::tests::{block_entry, compound, file, root, state_entry};
    use crate::testing::World;

    fn state(text: &str) -> BlockState {
        BlockState::parse(text).unwrap_or_else(|| panic!("{text} is a state"))
    }

    fn template(palette: Vec<Nbt>, blocks: Vec<Nbt>, size: [i32; 3]) -> TemplateBlocks {
        parse_template(&file(&root(size, palette, blocks))).unwrap()
    }

    /// A row of three blocks along x: stone, oak stairs facing east, and a magma block.
    fn row() -> TemplateBlocks {
        template(
            vec![
                state_entry("minecraft:stone", &[]),
                state_entry("minecraft:oak_stairs", &[("facing", "east")]),
                state_entry("minecraft:magma_block", &[]),
            ],
            vec![
                block_entry([0, 0, 0], 0, None),
                block_entry([1, 0, 0], 1, None),
                block_entry([2, 0, 0], 2, None),
            ],
            [3, 1, 1],
        )
    }

    fn place(
        world: &mut World,
        template: &TemplateBlocks,
        at: BlockPos,
        settings: &StructurePlaceSettings<'_>,
        seed: u64,
    ) -> bool {
        let mut random = WorldgenRandom::from_seed(seed);
        place_in_world(template, world, at, at, settings, &mut random)
    }

    #[test]
    fn a_template_is_placed_turned_about_its_pivot_with_its_states_turned() {
        let mut world = World::flat(blocks::DIRT, 0);
        let mut settings = StructurePlaceSettings::new(0);
        settings.rotation = Rotation::Clockwise90;
        settings.known_shape = true;
        let at = BlockPos::new(4, 10, 4);
        assert!(place(&mut world, &row(), at, &settings, 1));
        // A quarter turn clockwise about the corner takes +x to +z.
        assert_eq!(world.get(BlockPos::new(4, 10, 4)), blocks::STONE);
        assert_eq!(
            world.get(BlockPos::new(4, 10, 5)),
            state("minecraft:oak_stairs[facing=south]")
        );
        assert_eq!(world.get(BlockPos::new(4, 10, 6)), blocks::MAGMA_BLOCK);
        // Magma marks the block above it, where a bubble column may stand.
        assert_eq!(world.marks, [BlockPos::new(4, 11, 6)]);
    }

    #[test]
    fn a_mirrored_template_runs_the_other_way() {
        let mut world = World::flat(blocks::DIRT, 0);
        let mut settings = StructurePlaceSettings::new(0);
        settings.mirror = StructureMirror::FrontBack;
        let at = BlockPos::new(4, 10, 4);
        assert!(place(&mut world, &row(), at, &settings, 1));
        assert_eq!(world.get(BlockPos::new(4, 10, 4)), blocks::STONE);
        assert_eq!(
            world.get(BlockPos::new(3, 10, 4)),
            state("minecraft:oak_stairs[facing=west]")
        );
        assert_eq!(world.get(BlockPos::new(2, 10, 4)), blocks::MAGMA_BLOCK);
        assert_eq!(
            template_bounding_box(
                [3, 1, 1],
                at,
                settings.mirror,
                settings.rotation,
                BlockPos::new(0, 0, 0)
            ),
            BoundingBox::new(IVec3::new(2, 10, 4), IVec3::new(4, 10, 4))
        );
    }

    #[test]
    fn blocks_outside_the_box_are_not_placed() {
        let mut world = World::flat(blocks::DIRT, 0);
        let mut settings = StructurePlaceSettings::new(0);
        settings.bounding_box = Some(BoundingBox::new(IVec3::new(0, 0, 0), IVec3::new(5, 20, 5)));
        assert!(place(
            &mut world,
            &row(),
            BlockPos::new(4, 10, 4),
            &settings,
            1
        ));
        assert_eq!(world.get(BlockPos::new(4, 10, 4)), blocks::STONE);
        assert_eq!(
            world.get(BlockPos::new(5, 10, 4)).block().unwrap().name,
            "minecraft:oak_stairs"
        );
        assert_eq!(world.get(BlockPos::new(6, 10, 4)), blocks::AIR);
    }

    #[test]
    fn writes_the_view_refuses_place_nothing() {
        let mut world = World::flat(blocks::DIRT, 0);
        let settings = StructurePlaceSettings::new(0);
        // Two chunks from the centre: outside the three by three.
        assert!(place(
            &mut world,
            &row(),
            BlockPos::new(31, 10, 0),
            &settings,
            1
        ));
        assert_eq!(world.get(BlockPos::new(31, 10, 0)), blocks::STONE);
        assert_eq!(world.get(BlockPos::new(32, 10, 0)), blocks::AIR);
        assert!(world.marks.is_empty());
    }

    #[test]
    fn a_waterloggable_block_takes_the_still_water_it_replaces_and_schedules_it() {
        let slab = template(
            vec![state_entry(
                "minecraft:stone_slab",
                &[("type", "bottom"), ("waterlogged", "false")],
            )],
            vec![block_entry([0, 0, 0], 0, None)],
            [1, 1, 1],
        );
        let at = BlockPos::new(2, 5, 2);
        let mut world = World::flat(blocks::DIRT, 0);
        world.set(at, blocks::WATER);
        let mut settings = StructurePlaceSettings::new(0);
        settings.known_shape = true;
        assert!(place(&mut world, &slab, at, &settings, 1));
        assert_eq!(
            world.get(at),
            state("minecraft:stone_slab[waterlogged=true]")
        );
        assert_eq!(world.ticks.len(), 1);
        assert_eq!(world.ticks[0].1.of, Ticked::Fluid(WATER));
        assert_eq!(FLUIDS[usize::from(WATER)], "minecraft:water");
        // A chunk being generated keeps no delay (`ProtoChunkTicks.schedule`).
        assert_eq!(world.ticks[0].1.delay, 0);

        let mut dry = World::flat(blocks::DIRT, 0);
        dry.set(at, blocks::WATER);
        settings.liquid_settings = LiquidSettings::IgnoreWaterlogging;
        assert!(place(&mut dry, &slab, at, &settings, 1));
        assert_eq!(
            dry.get(at),
            state("minecraft:stone_slab[waterlogged=false]")
        );
        assert!(dry.ticks.is_empty());
    }

    #[test]
    fn a_dry_container_fills_from_a_source_beside_it_that_the_template_did_not_place() {
        // A slab, with a water source the template places on one side of it.
        let template = template(
            vec![
                state_entry("minecraft:stone_slab", &[("waterlogged", "false")]),
                state_entry("minecraft:water", &[("level", "0")]),
            ],
            vec![
                block_entry([1, 0, 0], 0, None),
                block_entry([0, 0, 0], 1, None),
            ],
            [2, 1, 1],
        );
        let at = BlockPos::new(2, 5, 2);
        let mut settings = StructurePlaceSettings::new(0);
        settings.known_shape = true;

        let mut locked_only = World::flat(blocks::DIRT, 0);
        assert!(place(&mut locked_only, &template, at, &settings, 1));
        assert_eq!(
            locked_only.get(BlockPos::new(3, 5, 2)),
            state("minecraft:stone_slab[waterlogged=false]"),
            "the template's own water does not fill it"
        );

        let mut beside = World::flat(blocks::DIRT, 0);
        beside.set(BlockPos::new(4, 5, 2), blocks::WATER);
        assert!(place(&mut beside, &template, at, &settings, 1));
        assert_eq!(
            beside.get(BlockPos::new(3, 5, 2)),
            state("minecraft:stone_slab[waterlogged=true]")
        );
    }

    #[test]
    fn a_chest_gets_its_data_without_its_id_and_a_loot_seed_from_the_placements_source() {
        let chest_data = compound(vec![
            (
                "LootTable",
                Nbt::String("minecraft:chests/igloo_chest".into()),
            ),
            ("components", Nbt::Compound(Vec::new())),
            ("id", Nbt::String("minecraft:chest".into())),
        ]);
        let template = template(
            vec![
                state_entry("minecraft:stone", &[]),
                state_entry("minecraft:chest", &[("facing", "north")]),
            ],
            vec![
                block_entry([0, 0, 0], 0, None),
                block_entry([1, 0, 0], 1, Some(chest_data)),
            ],
            [2, 1, 1],
        );
        let at = BlockPos::new(2, 5, 2);
        let mut world = World::flat(blocks::DIRT, 0);
        let mut settings = StructurePlaceSettings::new(0);
        settings.known_shape = true;
        let mut random = WorldgenRandom::from_seed(9);
        let mut copy = random.clone();
        assert!(place_in_world(
            &template,
            &mut world,
            at,
            at,
            &settings,
            &mut random
        ));
        // The palette is drawn from the position's own source; the seed is the first
        // draw of the placement's.
        let seed = copy.next_i64();
        let entity = world.entities.get(&BlockPos::new(3, 5, 2)).unwrap();
        assert_eq!(
            clustine_data::BLOCK_ENTITY_TYPES[usize::from(entity.kind)],
            "minecraft:chest"
        );
        let data = NbtReader::new(&entity.data).nbt().unwrap().unwrap();
        assert_eq!(data.get("id"), None);
        assert_eq!(
            data.get("LootTable"),
            Some(&Nbt::String("minecraft:chests/igloo_chest".into()))
        );
        assert_eq!(data.get("LootTableSeed"), Some(&Nbt::Long(seed)));
        assert_eq!(data.get("components"), Some(&Nbt::Compound(Vec::new())));
        assert_eq!(random.next_i64(), copy.next_i64());
        // The barrier went before the chest.
        assert!(
            world
                .writes
                .contains(&(BlockPos::new(3, 5, 2), blocks::BARRIER))
        );
    }

    #[test]
    fn a_container_without_a_loot_table_draws_a_seed_keeps_none_and_saves_its_empty_items() {
        let barrel_data = compound(vec![("id", Nbt::String("minecraft:barrel".into()))]);
        let template = template(
            vec![state_entry("minecraft:barrel", &[])],
            vec![block_entry([0, 0, 0], 0, Some(barrel_data))],
            [1, 1, 1],
        );
        let at = BlockPos::new(2, 5, 2);
        let mut world = World::flat(blocks::DIRT, 0);
        let mut settings = StructurePlaceSettings::new(0);
        settings.known_shape = true;
        let mut random = WorldgenRandom::from_seed(9);
        let mut copy = random.clone();
        assert!(place_in_world(
            &template,
            &mut world,
            at,
            at,
            &settings,
            &mut random
        ));
        copy.next_i64();
        assert_eq!(random.next_i64(), copy.next_i64());
        let data = NbtReader::new(&world.entities[&at].data)
            .nbt()
            .unwrap()
            .unwrap();
        // A barrel saves its items whether or not it has any (`saveAllItems`), and
        // no seed without a table.
        assert_eq!(
            data,
            Nbt::Compound(vec![
                ("components".into(), Nbt::Compound(Vec::new())),
                ("Items".into(), Nbt::List(Vec::new())),
            ])
        );
    }

    #[test]
    fn a_given_source_draws_the_palette_even_where_there_is_one() {
        let mut world = World::flat(blocks::DIRT, 0);
        let mut settings = StructurePlaceSettings::new(0);
        settings.random = PlaceRandom::Given;
        settings.known_shape = true;
        let mut random = WorldgenRandom::from_seed(4);
        let mut copy = random.clone();
        let at = BlockPos::new(1, 3, 1);
        assert!(place_in_world(
            &row(),
            &mut world,
            at,
            at,
            &settings,
            &mut random
        ));
        assert_eq!(copy.next_i32_bounded(1), 0);
        assert_eq!(random.next_i64(), copy.next_i64());
    }

    #[test]
    fn processors_run_in_order_and_a_dropped_block_is_not_placed() {
        let mut world = World::flat(blocks::DIRT, 0);
        let mut settings = StructurePlaceSettings::new(0);
        let ignore = Processor::BlockIgnore(vec![crate::blocks::block_id(blocks::MAGMA_BLOCK)]);
        settings.processors = vec![&ignore];
        let at = BlockPos::new(1, 3, 1);
        assert!(place(&mut world, &row(), at, &settings, 1));
        assert_eq!(world.get(BlockPos::new(3, 3, 1)), blocks::AIR);
        assert!(world.marks.is_empty());
    }

    #[test]
    fn unless_the_shape_is_known_water_at_the_edges_is_scheduled() {
        let water = template(
            vec![state_entry("minecraft:water", &[("level", "0")])],
            vec![block_entry([0, 0, 0], 0, None)],
            [1, 1, 1],
        );
        let at = BlockPos::new(2, 5, 2);
        let mut settings = StructurePlaceSettings::new(0);
        let mut world = World::flat(blocks::DIRT, 0);
        assert!(place(&mut world, &water, at, &settings, 1));
        // Six faces, each asking the water once, then the six neighbours once more.
        let at_water = world.ticks.iter().filter(|(pos, _)| *pos == at).count();
        assert_eq!(at_water, 12);

        settings.known_shape = true;
        let mut known = World::flat(blocks::DIRT, 0);
        assert!(place(&mut known, &water, at, &settings, 1));
        assert!(known.ticks.is_empty());
    }

    #[test]
    fn a_position_is_mirrored_then_turned_about_the_pivot() {
        let pivot = BlockPos::new(1, 0, 1);
        let pos = BlockPos::new(0, 3, 2);
        assert_eq!(
            transform_pos(pos, StructureMirror::None, Rotation::Clockwise90, pivot),
            BlockPos::new(0, 3, 0)
        );
        assert_eq!(
            transform_pos(pos, StructureMirror::LeftRight, Rotation::None, pivot),
            BlockPos::new(0, 3, -2)
        );
        assert_eq!(
            transform_pos(
                pos,
                StructureMirror::LeftRight,
                Rotation::Clockwise180,
                pivot
            ),
            BlockPos::new(2, 3, 4)
        );
        assert_eq!(
            transform_pos(
                pos,
                StructureMirror::FrontBack,
                Rotation::CounterClockwise90,
                pivot
            ),
            BlockPos::new(2, 3, 2)
        );
    }

    #[test]
    fn the_faces_of_a_shape_come_in_the_games_order() {
        let mut faces = Vec::new();
        for_all_faces(
            (1, 1, 1),
            |_, _, _| true,
            |d, x, y, z| faces.push((d, x, y, z)),
        );
        let directions: Vec<Direction> = faces.iter().map(|face| face.0).collect();
        assert_eq!(
            directions,
            [
                Direction::North,
                Direction::South,
                Direction::Down,
                Direction::Up,
                Direction::West,
                Direction::East
            ]
        );
        let mut two = Vec::new();
        for_all_faces(
            (2, 1, 1),
            |_, _, _| true,
            |d, x, y, z| two.push((d, x, y, z)),
        );
        assert_eq!(two.len(), 10);
        assert_eq!(two[8], (Direction::West, 0, 0, 0));
        assert_eq!(two[9], (Direction::East, 1, 0, 0));
    }

    #[test]
    fn an_empty_template_or_one_without_size_places_nothing() {
        let empty = template(
            vec![state_entry("minecraft:stone", &[])],
            Vec::new(),
            [1, 1, 1],
        );
        let settings = StructurePlaceSettings::new(0);
        let mut world = World::flat(blocks::DIRT, 0);
        assert!(!place(
            &mut world,
            &empty,
            BlockPos::new(0, 3, 0),
            &settings,
            1
        ));
        let flat = template(
            vec![state_entry("minecraft:stone", &[])],
            vec![block_entry([0, 0, 0], 0, None)],
            [1, 0, 1],
        );
        assert!(!place(
            &mut world,
            &flat,
            BlockPos::new(0, 3, 0),
            &settings,
            1
        ));
        assert!(world.writes.is_empty());
    }
}
