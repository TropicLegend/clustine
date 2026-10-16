// Adapted from SteelMC's `steel-core/src/worldgen/structure/piece_placer/mineshaft.rs`
// (see NOTICE in this crate), which follows the game's `MineshaftPieces`.

//! The pieces of mineshafts, ordinary and of the badlands (`MineshaftPieces`): the
//! room they start from, corridors with their supports, cobwebs, rails and chests in
//! minecarts, crossings of one floor or two, and stairs. The badlands' kind is built of
//! dark oak where the other is of oak.
//!
//! A piece places nothing in a chunk where its box, grown by one, meets a liquid at
//! its faces within the chunk, or whose middle lies in a biome of
//! `#minecraft:mineshaft_blocking` (`isInInvalidLocation`).
//!
//! What a corridor keeps between runs: whether its cave spiders' spawner stands
//! (`hasPlacedSpider`), bit [`SPAWNER_PLACED`] of the piece's flags. The game's piece
//! is one object for every chunk, and once its spawner stands, other chunks no longer
//! draw where to put one.
//!
//! The chests are chests in minecarts, which are entities: the rail under one is
//! placed and the game's draws for it made (its shape, then the minecart's loot
//! seed), and the minecart itself, with its loot, is dropped, as Clustine has no
//! entities.

use clustine_data::{BlockClass, BlockState, Face, SupportType, blocks};
use clustine_noise::Random;
use clustine_structures::compat::biome_name;
use clustine_structures::mineshaft::{MineshaftPieceKind, MineshaftPiecePayload, MineshaftType};
use clustine_structures::piece::StructurePiece;
use clustine_world::BlockPos;
use clustine_world::generation::PieceState;

use super::Placing;
use super::code_piece::{CodePiece, sturdy};
use crate::blocks::{is_class, same_block};

/// The piece's flag that its one spawner stands.
pub(crate) const SPAWNER_PLACED: u16 = 1;

const CAVE_SPIDER: &str = "minecraft:cave_spider";

/// The biomes of `#minecraft:mineshaft_blocking` in the pinned jar.
const BLOCKING_BIOMES: [&str; 1] = ["minecraft:deep_dark"];

/// What [`CodePiece::place_block`] leaves standing in an ordinary mineshaft
/// (`MineShaftPiece.canBeReplaced`): its own wood and the chains.
const KEPT_NORMAL: [BlockState; 4] = [
    blocks::OAK_PLANKS,
    blocks::OAK_LOG,
    blocks::OAK_FENCE,
    blocks::IRON_CHAIN,
];
const KEPT_MESA: [BlockState; 4] = [
    blocks::DARK_OAK_PLANKS,
    blocks::DARK_OAK_LOG,
    blocks::DARK_OAK_FENCE,
    blocks::IRON_CHAIN,
];

/// The wood of a kind of mineshaft (`MineshaftStructure.Type`).
struct Wood {
    planks: BlockState,
    log: BlockState,
    fence: BlockState,
}

fn wood(kind: MineshaftType) -> Wood {
    match kind {
        MineshaftType::Normal => Wood {
            planks: blocks::OAK_PLANKS,
            log: blocks::OAK_LOG,
            fence: blocks::OAK_FENCE,
        },
        MineshaftType::Mesa => Wood {
            planks: blocks::DARK_OAK_PLANKS,
            log: blocks::DARK_OAK_LOG,
            fence: blocks::DARK_OAK_FENCE,
        },
    }
}

/// Places a mineshaft piece in the centre chunk; returns its state as it is to be
/// kept.
pub(crate) fn place(
    placing: &mut Placing<'_, '_, '_>,
    piece: &StructurePiece,
    data: &MineshaftPiecePayload,
    state: PieceState,
) -> PieceState {
    let kept: &'static [BlockState] = match data.mineshaft_type {
        MineshaftType::Normal => &KEPT_NORMAL,
        MineshaftType::Mesa => &KEPT_MESA,
    };
    let mut at = CodePiece::new(placing, piece.bounding_box, piece.orientation).keeping(kept);
    if in_invalid_location(&mut at) {
        return state;
    }
    let wood = wood(data.mineshaft_type);
    match &data.kind {
        MineshaftPieceKind::Room {
            child_entrance_boxes,
        } => {
            let b = at.bounds;
            let air = blocks::CAVE_AIR;
            at.generate_box(
                [b.min_x(), b.min_y() + 1, b.min_z()],
                [b.max_x(), (b.min_y() + 3).min(b.max_y()), b.max_z()],
                air,
                air,
                false,
            );
            for entrance in child_entrance_boxes {
                at.generate_box(
                    [entrance.min_x(), entrance.max_y() - 2, entrance.min_z()],
                    [entrance.max_x(), entrance.max_y(), entrance.max_z()],
                    air,
                    air,
                    false,
                );
            }
            at.generate_upper_half_sphere(
                [b.min_x(), b.min_y() + 4, b.min_z()],
                [b.max_x(), b.max_y(), b.max_z()],
                air,
                false,
            );
            state
        }
        MineshaftPieceKind::Corridor {
            has_rails,
            spider_corridor,
            has_placed_spider,
            num_sections,
        } => {
            let placed = *has_placed_spider || state.flags & SPAWNER_PLACED != 0;
            let placed = corridor(
                &mut at,
                &wood,
                *has_rails,
                *spider_corridor,
                placed,
                *num_sections,
            );
            PieceState {
                flags: if placed {
                    state.flags | SPAWNER_PLACED
                } else {
                    state.flags
                },
                ..state
            }
        }
        MineshaftPieceKind::Crossing { is_two_floored, .. } => {
            crossing(&mut at, &wood, *is_two_floored);
            state
        }
        MineshaftPieceKind::Stairs => {
            let air = blocks::CAVE_AIR;
            at.generate_box([0, 5, 0], [2, 7, 1], air, air, false);
            at.generate_box([0, 0, 7], [2, 2, 8], air, air, false);
            for i in 0..5 {
                let bottom = 5 - i - i32::from(i < 4);
                at.generate_box([0, bottom, 2 + i], [2, 7 - i, 2 + i], air, air, false);
            }
            state
        }
    }
}

/// `MineShaftPiece.isInInvalidLocation`.
fn in_invalid_location(at: &mut CodePiece<'_, '_, '_, '_>) -> bool {
    let (b, chunk) = (at.bounds, at.chunk_box());
    let x0 = (b.min_x() - 1).max(chunk.min_x());
    let y0 = (b.min_y() - 1).max(chunk.min_y());
    let z0 = (b.min_z() - 1).max(chunk.min_z());
    let x1 = (b.max_x() + 1).min(chunk.max_x());
    let y1 = (b.max_y() + 1).min(chunk.max_y());
    let z1 = (b.max_z() + 1).min(chunk.max_z());
    // Java's division, which rounds towards nought.
    let middle = BlockPos::new((x0 + x1) / 2, (y0 + y1) / 2, (z0 + z1) / 2);
    let biome = at.level().biome(middle);
    if biome_name(biome).is_some_and(|name| BLOCKING_BIOMES.contains(&name)) {
        return true;
    }
    let level = at.level();
    let mut liquid =
        |x: i32, y: i32, z: i32| level.block(BlockPos::new(x, y, z)).props().is_liquid();
    for x in x0..=x1 {
        for z in z0..=z1 {
            if liquid(x, y0, z) || liquid(x, y1, z) {
                return true;
            }
        }
    }
    for x in x0..=x1 {
        for y in y0..=y1 {
            if liquid(x, y, z0) || liquid(x, y, z1) {
                return true;
            }
        }
    }
    for z in z0..=z1 {
        for y in y0..=y1 {
            if liquid(x0, y, z) || liquid(x1, y, z) {
                return true;
            }
        }
    }
    false
}

/// `MineShaftCorridor.postProcess`. Returns whether the corridor's spawner stands.
fn corridor(
    at: &mut CodePiece<'_, '_, '_, '_>,
    wood: &Wood,
    has_rails: bool,
    spider_corridor: bool,
    mut placed_spider: bool,
    num_sections: i32,
) -> bool {
    let length = num_sections * 5 - 1;
    let air = blocks::CAVE_AIR;
    at.generate_box([0, 0, 0], [2, 1, length], air, air, false);
    at.generate_maybe_box(0.8, [0, 2, 0], [2, 2, length], air, air, false, false);
    if spider_corridor {
        at.generate_maybe_box(
            0.6,
            [0, 0, 0],
            [2, 1, length],
            blocks::COBWEB,
            air,
            false,
            true,
        );
    }

    for section in 0..num_sections {
        let z = 2 + section * 5;
        place_support(at, wood, 0, 0, z, 2, 2);
        maybe_place_cobweb(at, 0.1, 0, 2, z - 1);
        maybe_place_cobweb(at, 0.1, 2, 2, z - 1);
        maybe_place_cobweb(at, 0.1, 0, 2, z + 1);
        maybe_place_cobweb(at, 0.1, 2, 2, z + 1);
        maybe_place_cobweb(at, 0.05, 0, 2, z - 2);
        maybe_place_cobweb(at, 0.05, 2, 2, z - 2);
        maybe_place_cobweb(at, 0.05, 0, 2, z + 2);
        maybe_place_cobweb(at, 0.05, 2, 2, z + 2);
        if at.random().next_i32_bounded(100) == 0 {
            create_minecart_chest(at, 2, 0, z - 1);
        }
        if at.random().next_i32_bounded(100) == 0 {
            create_minecart_chest(at, 0, 0, z + 1);
        }
        if spider_corridor && !placed_spider {
            let spawner_z = z - 1 + at.random().next_i32_bounded(3);
            let pos = at.world_pos(1, 0, spawner_z);
            if at.inside(pos) && at.is_interior(1, 0, spawner_z) {
                placed_spider = true;
                at.place_spawner(pos, CAVE_SPIDER);
            }
        }
    }

    for x in 0..=2 {
        for z in 0..=length {
            set_planks(at, wood.planks, x, -1, z);
        }
    }

    place_double_lower_or_upper_support(at, wood, 0, -1, 2);
    if num_sections > 1 {
        place_double_lower_or_upper_support(at, wood, 0, -1, length - 2);
    }

    if has_rails {
        let rail = blocks::RAIL
            .with_property("shape", "north_south")
            .expect("a rail runs north to south");
        for z in 0..=length {
            let floor = at.block(1, -1, z);
            if !floor.props().is_air() && floor.props().is_solid_render() {
                let probability = if at.is_interior(1, 0, z) { 0.7 } else { 0.9 };
                at.maybe_generate_block(probability, 1, 0, z, rail);
            }
        }
    }
    placed_spider
}

/// `MineShaftCorridor.createChest`: a rail with a chest in a minecart on it, where
/// there is air with something under it. The minecart is dropped (see the module's
/// head), but its loot seed is drawn as the game draws it.
fn create_minecart_chest(at: &mut CodePiece<'_, '_, '_, '_>, x: i32, y: i32, z: i32) {
    let pos = at.world_pos(x, y, z);
    if !at.inside(pos)
        || !at.level().block(pos).props().is_air()
        || at.level().block(pos.offset(0, -1, 0)).props().is_air()
    {
        return;
    }
    let shape = if at.random().next_bool() {
        "north_south"
    } else {
        "east_west"
    };
    let rail = blocks::RAIL
        .with_property("shape", shape)
        .expect("a rail runs straight both ways");
    at.place_block(rail, x, y, z);
    // `chest.setLootTable(ABANDONED_MINESHAFT, random.nextLong())`.
    let _seed = at.random().next_i64();
}

/// `MineShaftCorridor.placeSupport`: two fence posts under a beam of planks, where the
/// ceiling above the beam is not air, and seldom a torch on the beam.
fn place_support(
    at: &mut CodePiece<'_, '_, '_, '_>,
    wood: &Wood,
    x0: i32,
    y0: i32,
    z: i32,
    y1: i32,
    x1: i32,
) {
    if (x0..=x1).any(|x| at.block(x, y1 + 1, z).props().is_air()) {
        return;
    }
    let air = blocks::CAVE_AIR;
    let west = side(wood.fence, "west");
    let east = side(wood.fence, "east");
    at.generate_box([x0, y0, z], [x0, y1 - 1, z], west, air, false);
    at.generate_box([x1, y0, z], [x1, y1 - 1, z], east, air, false);
    if at.random().next_i32_bounded(4) == 0 {
        at.generate_box([x0, y1, z], [x0, y1, z], wood.planks, air, false);
        at.generate_box([x1, y1, z], [x1, y1, z], wood.planks, air, false);
    } else {
        at.generate_box([x0, y1, z], [x1, y1, z], wood.planks, air, false);
        at.maybe_generate_block(0.05, x0 + 1, y1, z - 1, wall_torch("south"));
        at.maybe_generate_block(0.05, x0 + 1, y1, z + 1, wall_torch("north"));
    }
}

fn side(fence: BlockState, name: &str) -> BlockState {
    fence
        .with_property(name, "true")
        .expect("a fence joins on each side")
}

fn wall_torch(facing: &str) -> BlockState {
    blocks::WALL_TORCH
        .with_property("facing", facing)
        .expect("a wall torch faces every horizontal direction")
}

/// `MineShaftCorridor.maybePlaceCobWeb`: the chance is drawn only where the block is
/// inside, and a web is placed only where two of its neighbours in the chunk are
/// sturdy towards it.
fn maybe_place_cobweb(
    at: &mut CodePiece<'_, '_, '_, '_>,
    probability: f32,
    x: i32,
    y: i32,
    z: i32,
) {
    if at.is_interior(x, y, z)
        && at.random().next_f32() < probability
        && has_sturdy_neighbours(at, x, y, z, 2)
    {
        at.place_block(blocks::COBWEB, x, y, z);
    }
}

fn has_sturdy_neighbours(
    at: &mut CodePiece<'_, '_, '_, '_>,
    x: i32,
    y: i32,
    z: i32,
    count: usize,
) -> bool {
    let pos = at.world_pos(x, y, z);
    let mut found = 0;
    for face in Face::ALL {
        let (dx, dy, dz) = crate::blocks::step(face);
        let neighbour = pos.offset(dx, dy, dz);
        if at.inside(neighbour) && sturdy(at.level().block(neighbour), face.opposite()) {
            found += 1;
            if found >= count {
                return true;
            }
        }
    }
    false
}

/// `MineShaftCorridor.setPlanksBlock`: planks where the block, inside, offers no
/// sturdy top already.
fn set_planks(at: &mut CodePiece<'_, '_, '_, '_>, planks: BlockState, x: i32, y: i32, z: i32) {
    if !at.is_interior(x, y, z) {
        return;
    }
    let pos = at.world_pos(x, y, z);
    let level = at.level();
    if !sturdy(level.block(pos), Face::Up) {
        level.set_block(pos, planks);
    }
}

/// `placeDoubleLowerOrUpperSupport`: under each side of the floor that is planks, a
/// log down to the ground, or else a fence and a chain up to the ceiling.
fn place_double_lower_or_upper_support(
    at: &mut CodePiece<'_, '_, '_, '_>,
    wood: &Wood,
    x: i32,
    y: i32,
    z: i32,
) {
    if same_block(at.block(x, y, z), wood.planks) {
        fill_pillar_down_or_chain_up(at, wood, x, y, z);
    }
    if same_block(at.block(x + 2, y, z), wood.planks) {
        fill_pillar_down_or_chain_up(at, wood, x + 2, y, z);
    }
}

/// `MineShaftPiece.fillPillarDownOrChainUp`: looks down at most 21 blocks and up at
/// most 51, alternately, and builds towards the first that can hold it.
fn fill_pillar_down_or_chain_up(
    at: &mut CodePiece<'_, '_, '_, '_>,
    wood: &Wood,
    x: i32,
    y: i32,
    z: i32,
) {
    let pos = at.world_pos(x, y, z);
    if !at.inside(pos) {
        return;
    }
    let level = at.level();
    let (min_y, max_y) = (level.min_y(), level.top_y() - 1);
    let world_y = pos.y;
    let column = |y: i32| BlockPos::new(pos.x, y, pos.z);
    let (mut check_below, mut check_above) = (true, true);
    let mut distance = 1;
    while check_below || check_above {
        if check_below {
            let below_at = column(world_y - distance);
            let below = level.block(below_at);
            let empty = replaceable_by_structures(below) && !same_block(below, blocks::LAVA);
            if !empty && sturdy(below, Face::Up) {
                for y in world_y - distance + 1..world_y {
                    level.set_block(column(y), wood.log);
                }
                return;
            }
            check_below = distance <= 20 && empty && below_at.y > min_y + 1;
        }
        if check_above {
            let above_at = column(world_y + distance);
            let above = level.block(above_at);
            let empty = replaceable_by_structures(above);
            if !empty && can_hang_chain_below(above) {
                level.set_block(column(world_y + 1), wood.fence);
                for y in world_y + 2..world_y + distance {
                    level.set_block(column(y), blocks::IRON_CHAIN);
                }
                return;
            }
            // `getMaxY()`, the highest block's y.
            check_above = distance <= 50 && empty && above_at.y < max_y;
        }
        distance += 1;
    }
}

/// `StructurePiece.isReplaceableByStructures`.
fn replaceable_by_structures(state: BlockState) -> bool {
    let props = state.props();
    props.is_air()
        || props.is_liquid()
        || same_block(state, blocks::GLOW_LICHEN)
        || same_block(state, blocks::SEAGRASS)
        || same_block(state, blocks::TALL_SEAGRASS)
}

/// `canHangChainBelow`: the middle of the block's bottom is sturdy and it does not
/// fall.
fn can_hang_chain_below(state: BlockState) -> bool {
    state
        .props()
        .is_face_sturdy(Face::Down, SupportType::Center)
        && !is_class(state, BlockClass::FallingBlock)
}

/// `MineShaftCrossing.postProcess`, in the world's coordinates, as a crossing has no
/// orientation.
fn crossing(at: &mut CodePiece<'_, '_, '_, '_>, wood: &Wood, two_floored: bool) {
    let b = at.bounds;
    let air = blocks::CAVE_AIR;
    let (x0, y0, z0, x1, y1, z1) = (
        b.min_x(),
        b.min_y(),
        b.min_z(),
        b.max_x(),
        b.max_y(),
        b.max_z(),
    );
    if two_floored {
        at.generate_box([x0 + 1, y0, z0], [x1 - 1, y0 + 2, z1], air, air, false);
        at.generate_box([x0, y0, z0 + 1], [x1, y0 + 2, z1 - 1], air, air, false);
        at.generate_box([x0 + 1, y1 - 2, z0], [x1 - 1, y1, z1], air, air, false);
        at.generate_box([x0, y1 - 2, z0 + 1], [x1, y1, z1 - 1], air, air, false);
        at.generate_box(
            [x0 + 1, y0 + 3, z0 + 1],
            [x1 - 1, y0 + 3, z1 - 1],
            air,
            air,
            false,
        );
    } else {
        at.generate_box([x0 + 1, y0, z0], [x1 - 1, y1, z1], air, air, false);
        at.generate_box([x0, y0, z0 + 1], [x1, y1, z1 - 1], air, air, false);
    }
    for (x, z) in [
        (x0 + 1, z0 + 1),
        (x0 + 1, z1 - 1),
        (x1 - 1, z0 + 1),
        (x1 - 1, z1 - 1),
    ] {
        // `placeSupportPillar`: a pillar of planks where the ceiling is not air.
        if !at.block(x, y1 + 1, z).props().is_air() {
            at.generate_box([x, y0, z], [x, y1, z], wood.planks, air, false);
        }
    }
    for x in x0..=x1 {
        for z in z0..=z1 {
            set_planks(at, wood.planks, x, y0 - 1, z);
        }
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::synced_registry;
    use clustine_structures::geometry::{BoundingBox, Direction, IVec3};
    use clustine_structures::piece::{ProceduralPieceData, StructurePiecePayload};
    use clustine_world::Biome;

    use super::*;
    use crate::structures::code_piece::tests::{piece, placing};
    use crate::testing::World;

    fn payload(mineshaft_type: MineshaftType, kind: MineshaftPieceKind) -> StructurePiecePayload {
        StructurePiecePayload::Procedural(ProceduralPieceData::Mineshaft(MineshaftPiecePayload {
            mineshaft_type,
            kind,
        }))
    }

    fn corridor_kind(rails: bool, spiders: bool, sections: i32) -> MineshaftPieceKind {
        MineshaftPieceKind::Corridor {
            has_rails: rails,
            spider_corridor: spiders,
            has_placed_spider: false,
            num_sections: sections,
        }
    }

    /// A corridor of two sections running south from (2, 20, 2).
    fn corridor_south(mineshaft_type: MineshaftType, kind: MineshaftPieceKind) -> StructurePiece {
        piece(
            BoundingBox::new(IVec3::new(2, 20, 2), IVec3::new(4, 22, 11)),
            Some(Direction::South),
            payload(mineshaft_type, kind),
        )
    }

    fn run(world: &mut World, piece: &StructurePiece, state: PieceState) -> PieceState {
        let StructurePiecePayload::Procedural(ProceduralPieceData::Mineshaft(data)) =
            &piece.payload
        else {
            unreachable!("the tests make mineshaft pieces");
        };
        placing(world, 7, |placing| place(placing, piece, data, state)).0
    }

    fn fence(state: BlockState, side: &str) -> BlockState {
        state.with_property(side, "true").unwrap()
    }

    #[test]
    fn a_corridor_in_rock_is_hollowed_and_held_by_fenced_supports() {
        let mut world = World::flat(blocks::STONE, 40);
        let corridor = corridor_south(MineshaftType::Normal, corridor_kind(false, false, 2));
        run(&mut world, &corridor, PieceState::default());
        for z in 2..=11 {
            for y in [20, 21] {
                let there = world.get(BlockPos::new(3, y, z));
                assert_eq!(there, blocks::CAVE_AIR, "at 3 {y} {z}");
            }
            // The rock under the corridor offers a sturdy top: no planks.
            assert_eq!(world.get(BlockPos::new(3, 19, z)), blocks::STONE);
        }
        for z in [4, 9] {
            for y in [20, 21] {
                assert_eq!(
                    world.get(BlockPos::new(2, y, z)),
                    fence(blocks::OAK_FENCE, "west")
                );
                assert_eq!(
                    world.get(BlockPos::new(4, y, z)),
                    fence(blocks::OAK_FENCE, "east")
                );
                assert!(world.marks.contains(&BlockPos::new(2, y, z)));
            }
            // A beam whole or of its two ends, as the game draws it.
            assert_eq!(world.get(BlockPos::new(2, 22, z)), blocks::OAK_PLANKS);
            assert_eq!(world.get(BlockPos::new(4, 22, z)), blocks::OAK_PLANKS);
        }
    }

    #[test]
    fn a_corridor_across_a_cave_lays_a_floor_and_props_it_on_logs() {
        let mut world = World::flat(blocks::STONE, 14);
        // The ocean floor terrain left, high above, makes the corridor inside.
        for x in 0..16 {
            for z in 0..16 {
                world.set(BlockPos::new(x, 60, z), blocks::STONE);
            }
        }
        let corridor = corridor_south(MineshaftType::Mesa, corridor_kind(false, false, 2));
        run(&mut world, &corridor, PieceState::default());
        for x in 2..=4 {
            for z in 2..=11 {
                assert_eq!(world.get(BlockPos::new(x, 19, z)), blocks::DARK_OAK_PLANKS);
            }
        }
        // Under both sides of the floor, at the third block from each end, a log down
        // to the rock.
        for (x, z) in [(2, 4), (4, 4), (2, 9), (4, 9)] {
            for y in 15..=18 {
                assert_eq!(world.get(BlockPos::new(x, y, z)), blocks::DARK_OAK_LOG);
            }
        }
        assert_eq!(world.get(BlockPos::new(3, 18, 4)), blocks::AIR);
        // No ceiling: no supports.
        assert!(
            !world
                .writes
                .iter()
                .any(|(_, state)| same_block(*state, blocks::DARK_OAK_FENCE))
        );
    }

    #[test]
    fn a_spider_corridor_places_one_spawner_and_keeps_that_it_did() {
        let corridor = corridor_south(MineshaftType::Normal, corridor_kind(false, true, 2));
        let spawners = |world: &World| {
            world
                .writes
                .iter()
                .filter(|(_, state)| *state == blocks::SPAWNER)
                .count()
        };
        let mut world = World::flat(blocks::STONE, 40);
        let kept = run(&mut world, &corridor, PieceState::default());
        assert_eq!(spawners(&world), 1);
        assert_eq!(kept.flags, SPAWNER_PLACED);
        assert_eq!((kept.shift_y, kept.settled), (0, false));

        // Another chunk's run of the same piece sees it placed, and draws for no other.
        let mut again = World::flat(blocks::STONE, 40);
        let kept_again = run(&mut again, &corridor, kept);
        assert_eq!(spawners(&again), 0);
        assert_eq!(kept_again, kept);
    }

    #[test]
    fn a_piece_meeting_water_at_its_edge_places_nothing() {
        let mut world = World::flat(blocks::STONE, 40);
        world.set(BlockPos::new(5, 21, 6), blocks::WATER);
        let corridor = corridor_south(MineshaftType::Normal, corridor_kind(true, false, 2));
        run(&mut world, &corridor, PieceState::default());
        assert!(world.writes.is_empty());
    }

    #[test]
    fn a_piece_in_the_deep_dark_places_nothing() {
        let mut world = World::flat(blocks::STONE, 40);
        let biomes = synced_registry("minecraft:worldgen/biome").unwrap();
        let deep_dark = biomes.id_of("minecraft:deep_dark").unwrap();
        world.biome = Biome(u16::try_from(deep_dark).unwrap());
        let corridor = corridor_south(MineshaftType::Normal, corridor_kind(true, false, 2));
        run(&mut world, &corridor, PieceState::default());
        assert!(world.writes.is_empty());
    }

    #[test]
    fn a_corridor_reaching_past_the_chunk_places_only_inside_it() {
        let mut world = World::flat(blocks::STONE, 40);
        let corridor = piece(
            BoundingBox::new(IVec3::new(8, 20, 5), IVec3::new(27, 22, 7)),
            Some(Direction::East),
            payload(MineshaftType::Normal, corridor_kind(true, false, 4)),
        );
        run(&mut world, &corridor, PieceState::default());
        assert!(!world.writes.is_empty());
        assert!(world.writes.iter().all(|(pos, _)| (0..16).contains(&pos.x)));
        // Rails, where there are any, run along the corridor, east to west.
        let rail = blocks::RAIL.with_property("shape", "east_west").unwrap();
        assert!(
            world
                .writes
                .iter()
                .filter(|(_, state)| same_block(*state, blocks::RAIL))
                .all(|(_, state)| *state == rail)
        );
    }

    #[test]
    fn a_room_is_hollowed_above_its_floor_and_domed() {
        let mut world = World::flat(blocks::STONE, 80);
        let room = piece(
            BoundingBox::new(IVec3::new(2, 30, 2), IVec3::new(12, 38, 12)),
            None,
            payload(
                MineshaftType::Normal,
                MineshaftPieceKind::Room {
                    child_entrance_boxes: Vec::new(),
                },
            ),
        );
        run(&mut world, &room, PieceState::default());
        assert_eq!(world.get(BlockPos::new(2, 30, 2)), blocks::STONE);
        assert_eq!(world.get(BlockPos::new(2, 31, 2)), blocks::CAVE_AIR);
        assert_eq!(world.get(BlockPos::new(12, 33, 12)), blocks::CAVE_AIR);
        // The dome: its middle reaches the top, its corners do not.
        assert_eq!(world.get(BlockPos::new(7, 38, 7)), blocks::CAVE_AIR);
        assert_eq!(world.get(BlockPos::new(2, 34, 2)), blocks::STONE);
    }

    #[test]
    fn a_two_floored_crossing_keeps_its_middle_floor_and_props_its_corners() {
        let mut world = World::flat(blocks::STONE, 80);
        let crossing = piece(
            BoundingBox::new(IVec3::new(2, 30, 2), IVec3::new(6, 36, 6)),
            None,
            payload(
                MineshaftType::Normal,
                MineshaftPieceKind::Crossing {
                    direction: Direction::South,
                    is_two_floored: true,
                },
            ),
        );
        run(&mut world, &crossing, PieceState::default());
        assert_eq!(world.get(BlockPos::new(4, 31, 4)), blocks::CAVE_AIR);
        assert_eq!(world.get(BlockPos::new(4, 33, 4)), blocks::CAVE_AIR);
        assert_eq!(world.get(BlockPos::new(2, 33, 4)), blocks::STONE);
        for (x, z) in [(3, 3), (3, 5), (5, 3), (5, 5)] {
            for y in 30..=36 {
                assert_eq!(world.get(BlockPos::new(x, y, z)), blocks::OAK_PLANKS);
            }
        }
    }

    #[test]
    fn stairs_step_down_five_blocks() {
        let mut world = World::flat(blocks::STONE, 80);
        let stairs = piece(
            BoundingBox::new(IVec3::new(2, 25, 2), IVec3::new(4, 32, 10)),
            Some(Direction::South),
            payload(MineshaftType::Normal, MineshaftPieceKind::Stairs),
        );
        run(&mut world, &stairs, PieceState::default());
        assert_eq!(world.get(BlockPos::new(3, 30, 2)), blocks::CAVE_AIR);
        assert_eq!(world.get(BlockPos::new(3, 29, 2)), blocks::STONE);
        assert_eq!(world.get(BlockPos::new(3, 25, 10)), blocks::CAVE_AIR);
        assert_eq!(world.get(BlockPos::new(3, 24, 10)), blocks::STONE);
        // The step at z 2 + 4 is cut from 1 to 3 above the box's floor.
        assert_eq!(world.get(BlockPos::new(3, 26, 8)), blocks::CAVE_AIR);
        assert_eq!(world.get(BlockPos::new(3, 25, 8)), blocks::STONE);
    }

    #[test]
    fn a_corridor_draws_in_the_game_s_order() {
        let mut world = World::flat(blocks::STONE, 40);
        let corridor = piece(
            BoundingBox::new(IVec3::new(2, 20, 2), IVec3::new(4, 22, 6)),
            Some(Direction::South),
            payload(MineshaftType::Normal, corridor_kind(false, false, 1)),
        );
        let StructurePiecePayload::Procedural(ProceduralPieceData::Mineshaft(data)) =
            &corridor.payload
        else {
            unreachable!("the test makes a mineshaft piece");
        };
        let (_, mut random) = placing(&mut world, 7, |placing| {
            place(placing, &corridor, data, PieceState::default())
        });

        let mut expected = clustine_noise::WorldgenRandom::from_seed(7);
        // The ceiling's chances, one for each of its 15 blocks.
        for _ in 0..15 {
            expected.next_f32();
        }
        // The support under rock: its beam, and the chances of its two torches.
        if expected.next_i32_bounded(4) != 0 {
            expected.next_f32();
            expected.next_f32();
        }
        // Eight cobwebs, all inside.
        for _ in 0..8 {
            expected.next_f32();
        }
        // Two chests, each a rail's shape and a minecart's loot seed when it comes.
        for _ in 0..2 {
            if expected.next_i32_bounded(100) == 0 {
                expected.next_bool();
                expected.next_i64();
            }
        }
        assert_eq!(random.next_i64(), expected.next_i64());
    }
}
