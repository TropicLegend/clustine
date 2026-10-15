//! Trees, fallen trees, huge mushrooms and root systems placed into the toy world.

use std::collections::BTreeMap;

use clustine_data::{BlockState, Face, blocks};
use clustine_noise::WorldgenRandom;
use clustine_world::generation::{
    FinalHeightmap, Neighbourhood, PieceKey, PieceState, Placing, StartRef, WorldgenHeightmap,
};
use clustine_world::{Biome, BlockEntity, BlockPos, ChunkPos, ScheduledTick, Ticked};
use clustine_worldgen_data::feature::{self as data, Data};

use super::common::{LEAVES, LOGS};
use crate::blocks::{block_id, relative};
use crate::context::{Context, Level};
use crate::dispatch::{Batch, KINDS};
use crate::feature::{Feature, read_configured};
use crate::reader::Reader;
use crate::testing::{World, settings};

/// The toy world with the block entities it is given.
struct WithEntities {
    world: World,
    entities: BTreeMap<BlockPos, BlockEntity>,
}

impl Neighbourhood for WithEntities {
    fn centre(&self) -> ChunkPos {
        self.world.centre()
    }
    fn block(&mut self, at: BlockPos) -> BlockState {
        self.world.block(at)
    }
    fn set_block(&mut self, at: BlockPos, state: BlockState) -> bool {
        self.world.set_block(at, state)
    }
    fn block_entity(&mut self, at: BlockPos) -> Option<&BlockEntity> {
        self.entities.get(&at)
    }
    fn set_block_entity(&mut self, at: BlockPos, entity: Option<BlockEntity>) -> bool {
        match entity {
            Some(entity) => {
                self.entities.insert(at, entity);
            }
            None => {
                self.entities.remove(&at);
            }
        }
        true
    }
    fn height(&mut self, map: FinalHeightmap, x: i32, z: i32) -> i32 {
        self.world.height(map, x, z)
    }
    fn worldgen_height(&mut self, map: WorldgenHeightmap, x: i32, z: i32) -> i32 {
        self.world.worldgen_height(map, x, z)
    }
    fn biome(&mut self, at: BlockPos) -> Biome {
        self.world.biome(at)
    }
    fn schedule(&mut self, at: BlockPos, tick: ScheduledTick) {
        self.world.schedule(at, tick);
    }
    fn mark(&mut self, at: BlockPos) {
        self.world.mark(at);
    }
    fn starts(&self) -> &[StartRef] {
        &[]
    }
    fn piece(&mut self, piece: PieceKey) -> PieceState {
        self.world.piece(piece)
    }
    fn set_piece(&mut self, piece: PieceKey, state: PieceState) {
        self.world.set_piece(piece, state);
    }
    fn attempt(&mut self, what: Placing, place: &mut dyn FnMut(&mut dyn Neighbourhood)) -> bool {
        self.world.attempt(what, place)
    }
}

impl WithEntities {
    fn new(world: World) -> Self {
        Self {
            world,
            entities: BTreeMap::new(),
        }
    }

    fn place(&mut self, feature: &dyn Feature, seed: u64, origin: BlockPos) -> bool {
        let settings = settings();
        let mut random = WorldgenRandom::from_seed(seed);
        let mut level = Level::new(self, settings.zoom(), -64, 384);
        let mut context = Context {
            level: &mut level,
            random: &mut random,
            settings,
            top: None,
        };
        feature.place(&mut context, origin)
    }
}

fn named(name: &str) -> &'static dyn Feature {
    let (index, _) = data::configured(name).expect("a configured feature of the data");
    settings().configured(index)
}

fn place(world: &mut World, name: &str, seed: u64, origin: BlockPos) -> bool {
    let feature = named(name);
    world.with_context(seed, |context| feature.place(context, origin))
}

fn distance(state: BlockState) -> Option<i32> {
    state.property("distance").and_then(|d| d.parse().ok())
}

/// Every leaf a tree placed is given a distance no less than that of the shortest
/// way through leaves to a log, and below seven exactly where that way is shorter
/// than seven. The game's walk may give a leaf more than the shortest way: a leaf
/// found again from a neighbour at its own distance, before it was taken, is taken
/// once more at the next (see `a_leaf_found_twice_keeps_the_greater_distance`).
fn assert_leaf_distances(world: &World) {
    let leaves: Vec<BlockPos> = world
        .writes
        .iter()
        .map(|(pos, _)| *pos)
        .filter(|pos| LEAVES.contains(world.get(*pos)))
        .collect();
    assert!(!leaves.is_empty());
    let mut shortest: BTreeMap<BlockPos, i32> = leaves.iter().map(|pos| (*pos, 7)).collect();
    loop {
        let mut changed = false;
        for pos in &leaves {
            let nearest = Face::ALL
                .iter()
                .map(|face| {
                    let neighbour = relative(*pos, *face);
                    if LOGS.contains(world.get(neighbour)) {
                        0
                    } else {
                        shortest.get(&neighbour).copied().unwrap_or(7)
                    }
                })
                .min()
                .expect("six sides");
            let way = (nearest + 1).min(7);
            if way < shortest[pos] {
                shortest.insert(*pos, way);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for pos in leaves {
        let own = distance(world.get(pos)).expect("leaves have a distance");
        let way = shortest[&pos];
        assert!(own >= way, "leaves at {pos:?}: {own} < {way}");
        assert_eq!(own == 7, way == 7, "leaves at {pos:?}: {own}, {way}");
    }
}

#[test]
fn every_kind_of_this_step_is_read_by_it() {
    static EMPTY: Data = Data::Object(&[]);
    for (kind, batch) in KINDS {
        if batch != Batch::Trees {
            continue;
        }
        let reader = Reader::new(&EMPTY, kind);
        let error = super::read(kind, &reader).unwrap_err();
        assert!(!error.what.contains("is not a kind"), "{kind}: {error}");
    }
}

#[test]
fn every_tree_of_the_data_is_read_and_placed_on_grass_without_a_failure() {
    let mut placed = 0;
    for entry in data::configured_features() {
        let feature = read_configured(&Reader::new(&entry.data, entry.name)).unwrap();
        let kind = entry.data.get("type").and_then(|kind| match kind {
            Data::Text(kind) => Some(*kind),
            _ => None,
        });
        if !matches!(
            kind,
            Some(
                "minecraft:tree"
                    | "minecraft:fallen_tree"
                    | "minecraft:huge_brown_mushroom"
                    | "minecraft:huge_red_mushroom"
                    | "minecraft:root_system"
            )
        ) {
            continue;
        }
        assert!(feature.built(), "{}", entry.name);
        for seed in 0..4 {
            let mut world = World::flat(blocks::GRASS_BLOCK, 64);
            if feature.place_in(&mut world, seed, BlockPos::new(8, 65, 8)) {
                placed += 1;
            }
            assert!(
                world.writes.iter().all(|(pos, _)| {
                    let chunk = pos.chunk();
                    chunk.x.abs() <= 1 && chunk.z.abs() <= 1
                }),
                "{}",
                entry.name
            );
        }
    }
    assert!(placed > 100, "{placed}");
}

trait PlaceIn {
    fn place_in(&self, world: &mut World, seed: u64, origin: BlockPos) -> bool;
}

impl PlaceIn for Box<dyn Feature> {
    fn place_in(&self, world: &mut World, seed: u64, origin: BlockPos) -> bool {
        world.with_context(seed, |context| self.place(context, origin))
    }
}

#[test]
fn an_oak_stands_on_dirt_with_a_straight_trunk_and_leaves_at_their_distance() {
    let mut world = World::flat(blocks::GRASS_BLOCK, 64);
    let origin = BlockPos::new(8, 65, 8);
    assert!(place(&mut world, "minecraft:oak", 3, origin));
    assert_eq!(world.get(BlockPos::new(8, 64, 8)), blocks::DIRT);
    let height = (0..)
        .take_while(|y| LOGS.contains(world.get(origin.offset(0, *y, 0))))
        .count() as i32;
    assert!((4..=6).contains(&height), "{height}");
    for y in 0..height {
        assert_eq!(
            world.get(origin.offset(0, y, 0)).property("axis"),
            Some("y")
        );
    }
    // The blob's top layer is a plus over the trunk.
    let top = origin.offset(0, height, 0);
    assert!(LEAVES.contains(world.get(top)));
    assert_eq!(distance(world.get(top)), Some(1));
    assert!(!LEAVES.contains(world.get(top.offset(0, 1, 0))));
    assert_leaf_distances(&world);
}

#[test]
fn trees_of_every_kind_give_their_leaves_the_distance_of_the_walk_from_the_logs() {
    for name in [
        "minecraft:birch",
        "minecraft:spruce",
        "minecraft:pine",
        "minecraft:fancy_oak",
        "minecraft:acacia",
        "minecraft:dark_oak",
        "minecraft:jungle_tree",
        "minecraft:mega_jungle_tree",
        "minecraft:mega_spruce",
        "minecraft:cherry",
        "minecraft:jungle_bush",
        "minecraft:red_poplar",
        "minecraft:pale_oak",
        "minecraft:swamp_oak",
    ] {
        let mut world = World::flat(blocks::GRASS_BLOCK, 64);
        assert!(
            place(&mut world, name, 11, BlockPos::new(8, 65, 8)),
            "{name}"
        );
        assert_leaf_distances(&world);
    }
}

#[test]
fn a_tree_at_the_edge_of_the_three_by_three_is_cut_off_there() {
    let mut world = World::flat(blocks::GRASS_BLOCK, 64);
    // The last writable column: the leaves to the east fall outside.
    let origin = BlockPos::new(31, 65, 8);
    assert!(place(&mut world, "minecraft:oak", 3, origin));
    assert!(LOGS.contains(world.get(origin)));
    assert!(LEAVES.contains(world.get(origin.offset(-1, 3, 0))));
    assert_eq!(world.get(origin.offset(1, 3, 0)), blocks::AIR);
    assert!(world.writes.iter().all(|(pos, _)| pos.x <= 31));
}

#[test]
fn a_tree_without_room_is_not_placed() {
    let mut world = World::flat(blocks::GRASS_BLOCK, 64);
    let origin = BlockPos::new(8, 65, 8);
    world.set(origin.offset(0, 2, 0), blocks::STONE);
    assert!(!place(&mut world, "minecraft:oak", 3, origin));
    assert!(world.writes.is_empty());
}

#[test]
fn water_beside_a_trunk_is_given_a_tick() {
    let mut world = World::flat(blocks::GRASS_BLOCK, 64);
    let origin = BlockPos::new(8, 65, 8);
    let water = origin.offset(1, 0, 0);
    world.set(water, blocks::WATER);
    assert!(place(&mut world, "minecraft:oak", 3, origin));
    let ticks: Vec<_> = world
        .ticks
        .iter()
        .filter(|(pos, _)| *pos == water)
        .collect();
    assert_eq!(ticks.len(), 1);
    assert_eq!(ticks[0].1.of, Ticked::Fluid(2));
    assert_eq!(ticks[0].1.delay, 5);
}

#[test]
fn leaves_of_another_tree_beside_a_tree_are_given_a_tick() {
    let mut world = World::flat(blocks::GRASS_BLOCK, 64);
    let origin = BlockPos::new(8, 65, 8);
    // Leaves of no tree beside the foot of the trunk: the walk reaches them, and
    // they ask for a tick across their faces with the air.
    let other = origin.offset(1, 0, 0);
    let leaves = blocks::OAK_LEAVES.with_property("distance", "7").unwrap();
    world.set(other, leaves);
    assert!(place(&mut world, "minecraft:oak", 3, origin));
    assert_eq!(distance(world.get(other)), Some(1));
    assert!(world.ticks.iter().any(|(pos, tick)| {
        *pos == other && tick.of == Ticked::Block(block_id(blocks::OAK_LEAVES) as u16)
    }));
}

#[test]
fn a_beehive_is_hung_under_the_leaves_with_two_or_three_bees() {
    static TREE: Data = Data::Object(&[
        ("type", Data::Text("minecraft:tree")),
        ("below_trunk_provider", Data::Text("minecraft:dirt")),
        (
            "decorators",
            Data::List(&[Data::Object(&[
                ("type", Data::Text("minecraft:beehive")),
                ("probability", Data::Number("1.0")),
            ])]),
        ),
        (
            "foliage_placer",
            Data::Object(&[
                ("type", Data::Text("minecraft:blob_foliage_placer")),
                ("height", Data::Number("3")),
                ("offset", Data::Number("0")),
                ("radius", Data::Number("2")),
            ]),
        ),
        ("foliage_provider", Data::Text("minecraft:birch_leaves")),
        (
            "minimum_size",
            Data::Object(&[("type", Data::Text("minecraft:two_layers_feature_size"))]),
        ),
        (
            "trunk_placer",
            Data::Object(&[
                ("type", Data::Text("minecraft:straight_trunk_placer")),
                ("base_height", Data::Number("5")),
                ("height_rand_a", Data::Number("0")),
                ("height_rand_b", Data::Number("0")),
            ]),
        ),
        ("trunk_provider", Data::Text("minecraft:birch_log")),
    ]);
    let tree = read_configured(&Reader::new(&TREE, "test")).unwrap();
    let mut world = WithEntities::new(World::flat(blocks::GRASS_BLOCK, 64));
    let origin = BlockPos::new(8, 65, 8);
    assert!(world.place(tree.as_ref(), 5, origin));
    let (hive, entity) = world
        .entities
        .iter()
        .next()
        .map(|(pos, entity)| (*pos, entity.clone()))
        .expect("a hive");
    let state = world.world.get(hive);
    assert_eq!(state.block_id(), blocks::BEE_NEST.block_id());
    assert_eq!(state.property("facing"), Some("south"));
    // The lowest leaves are at the trunk's top three blocks: the hive is one below.
    assert_eq!(hive.y, origin.y + 5 - 3 - 1);
    assert_eq!(hive.z - origin.z + (hive.x - origin.x).abs(), 1);
    let bees = entity
        .data
        .windows(13)
        .filter(|window| *window == b"minecraft:bee")
        .count();
    assert!((2..=3).contains(&bees), "{bees}");
}

#[test]
fn a_huge_red_mushroom_has_a_stem_under_a_domed_cap() {
    let mut world = World::flat(blocks::MYCELIUM, 64);
    let origin = BlockPos::new(8, 65, 8);
    assert!(place(&mut world, "minecraft:huge_red_mushroom", 1, origin));
    let height = (0..)
        .take_while(|y| {
            world.get(origin.offset(0, *y, 0)).block_id() == blocks::MUSHROOM_STEM.block_id()
        })
        .count() as i32;
    assert!((4..=12).contains(&height), "{height}");
    let top = world.get(origin.offset(0, height, 0));
    assert_eq!(top.block_id(), blocks::RED_MUSHROOM_BLOCK.block_id());
    assert_eq!(top.property("up"), Some("true"));
    // The sides of the dome hang two below the top.
    let side = world.get(origin.offset(2, height - 2, 0));
    assert_eq!(side.block_id(), blocks::RED_MUSHROOM_BLOCK.block_id());
    assert_eq!(side.property("east"), Some("true"));
}

#[test]
fn a_huge_brown_mushroom_has_a_flat_cap_without_corners() {
    let mut world = World::flat(blocks::MYCELIUM, 64);
    let origin = BlockPos::new(8, 65, 8);
    assert!(place(
        &mut world,
        "minecraft:huge_brown_mushroom",
        1,
        origin
    ));
    let height = (0..)
        .take_while(|y| {
            world.get(origin.offset(0, *y, 0)).block_id() == blocks::MUSHROOM_STEM.block_id()
        })
        .count() as i32;
    let cap = origin.offset(0, height, 0);
    assert_eq!(
        world.get(cap).block_id(),
        blocks::BROWN_MUSHROOM_BLOCK.block_id()
    );
    assert_eq!(world.get(cap.offset(3, 0, 3)), blocks::AIR);
    assert_eq!(
        world.get(cap.offset(3, 0, 2)).property("east"),
        Some("true")
    );
}

#[test]
fn a_huge_mushroom_needs_its_ground() {
    let mut world = World::flat(blocks::STONE, 64);
    assert!(!place(
        &mut world,
        "minecraft:huge_red_mushroom",
        1,
        BlockPos::new(8, 65, 8)
    ));
    assert!(world.writes.is_empty());
}

#[test]
fn a_fallen_tree_leaves_a_stump_and_a_log_lying_on_the_ground() {
    let mut world = World::flat(blocks::GRASS_BLOCK, 64);
    let origin = BlockPos::new(8, 65, 8);
    assert!(place(&mut world, "minecraft:fallen_oak_tree", 2, origin));
    assert_eq!(world.get(origin).block_id(), blocks::OAK_LOG.block_id());
    assert_eq!(world.get(origin).property("axis"), Some("y"));
    let lying: Vec<BlockPos> = world
        .writes
        .iter()
        .filter(|(pos, state)| {
            pos.y == 65 && *pos != origin && state.block_id() == blocks::OAK_LOG.block_id()
        })
        .map(|(pos, _)| *pos)
        .collect();
    // Four to seven long, less the two the game takes off.
    assert!((2..=5).contains(&lying.len()), "{lying:?}");
    let axis = world.get(lying[0]).property("axis");
    assert!(axis == Some("x") || axis == Some("z"));
}

#[test]
fn an_azalea_grows_over_a_cave_with_rooted_dirt_down_to_it() {
    // Dirt to 64 with a cave from 40 to 50 under the origin's column.
    let mut world = World::flat(blocks::DIRT, 64);
    for x in 0..16 {
        for z in 0..16 {
            for y in 40..=50 {
                world.set(BlockPos::new(x, y, z), blocks::AIR);
            }
            world.set(BlockPos::new(x, 64, z), blocks::GRASS_BLOCK);
        }
    }
    let origin = BlockPos::new(8, 50, 8);
    assert!(place(&mut world, "minecraft:rooted_azalea_tree", 7, origin));
    assert!(LOGS.contains(world.get(BlockPos::new(8, 65, 8))));
    assert!(
        world
            .writes
            .iter()
            .any(|(pos, state)| pos.y < 64 && *state == blocks::ROOTED_DIRT)
    );
}

#[test]
fn a_leaf_found_twice_keeps_the_greater_distance() {
    use super::common::Placement;
    use super::leaves::{Bounds, update_leaves};
    let mut world = World::flat(blocks::STONE, 0);
    let leaves = blocks::OAK_LEAVES.with_property("distance", "7").unwrap();
    let (low, high) = (BlockPos::new(1, 10, 0), BlockPos::new(1, 11, 0));
    let mut placement = Placement::default();
    world.with_level(|level| {
        for y in [10, 11] {
            placement.set_trunk(level, BlockPos::new(0, y, 0), blocks::OAK_LOG);
        }
        placement.set_foliage(level, low, leaves);
        placement.set_foliage(level, high, leaves);
        let bounds = Bounds::around(&placement).unwrap();
        update_leaves(level, bounds, &placement);
    });
    // Both are beside a log; the one taken first finds the other before it is taken,
    // at two, and the other is taken again at two after it was given one.
    let mut found = [distance(world.get(low)), distance(world.get(high))];
    found.sort();
    assert_eq!(found, [Some(1), Some(2)]);
}

#[test]
fn mushrooms_on_a_fallen_log_are_not_marked_as_a_tree_sets_its_decorations() {
    static FALLEN: Data = Data::Object(&[
        ("type", Data::Text("minecraft:fallen_tree")),
        (
            "log_decorators",
            Data::List(&[Data::Object(&[
                ("type", Data::Text("minecraft:attached_to_logs")),
                ("block_provider", Data::Text("minecraft:red_mushroom")),
                ("directions", Data::List(&[Data::Text("up")])),
                ("probability", Data::Number("1.0")),
            ])]),
        ),
        ("log_length", Data::Number("6")),
        ("stump_decorators", Data::List(&[])),
        ("trunk_provider", Data::Text("minecraft:oak_log")),
    ]);
    let fallen = read_configured(&Reader::new(&FALLEN, "test")).unwrap();
    let mut world = World::flat(blocks::GRASS_BLOCK, 64);
    assert!(fallen.place_in(&mut world, 4, BlockPos::new(8, 65, 8)));
    let mushrooms = world
        .writes
        .iter()
        .filter(|(_, state)| *state == blocks::RED_MUSHROOM)
        .count();
    assert_eq!(mushrooms, 4);
    assert!(world.marks.is_empty(), "{:?}", world.marks);
}

#[test]
fn a_mangrove_raises_its_trunk_on_roots_waterlogged_in_water() {
    // Two blocks of mud on dirt, under shallow water: the roots stop at the dirt.
    let mut world = World::flat(blocks::DIRT, 62);
    for x in -8..24 {
        for z in -8..24 {
            world.set(BlockPos::new(x, 61, z), blocks::MUD);
            world.set(BlockPos::new(x, 62, z), blocks::MUD);
            world.set(BlockPos::new(x, 63, z), blocks::WATER);
        }
    }
    let origin = BlockPos::new(8, 63, 8);
    let mut placed = false;
    for seed in 0..8 {
        if place(&mut world, "minecraft:mangrove", seed, origin) {
            placed = true;
            break;
        }
    }
    assert!(placed);
    let roots: Vec<BlockState> = world
        .writes
        .iter()
        .filter(|(pos, state)| pos.y == 63 && state.block_id() == blocks::MANGROVE_ROOTS.block_id())
        .map(|(_, state)| *state)
        .collect();
    assert!(!roots.is_empty());
    assert!(
        roots
            .iter()
            .all(|state| state.property("waterlogged") == Some("true"))
    );
    assert!(
        world
            .writes
            .iter()
            .any(|(_, state)| state.block_id() == blocks::MANGROVE_LOG.block_id())
    );
}
