// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/monster_room.rs` and
// `steel-core/src/worldgen/feature/features/block_entities.rs`.

//! `monster_room`: a dungeon, a room of cobblestone and mossy cobblestone hollowed out
//! underground where its walls have a few openings, with up to two chests of loot
//! against its walls and a spawner in its middle (`MonsterRoomFeature`).
//!
//! The chests and the spawner keep block entities, whose data is what the game saves
//! for them: a chest its loot table and the seed drawn for it, the spawner the mob it
//! was given and a fresh spawner's settings. The game makes no entity here.

use clustine_data::{BlockState, Face, blocks};
use clustine_noise::Random;
use clustine_protocol::nbt::Nbt;
use clustine_world::BlockPos;

use crate::blocks::{HORIZONTAL, relative, same_block};
use crate::context::{Context, Level};
use crate::feature::Feature;
use crate::features::ground::shared::{block_entity, compound, safe_set_block};
use crate::reader::{Read, Reader};

/// The loot of a dungeon's chests.
pub const SIMPLE_DUNGEON: &str = "minecraft:chests/simple_dungeon";

/// The mobs a dungeon's spawner may have, as the game draws one (`MOBS`).
pub const MOBS: [&str; 4] = [
    "minecraft:skeleton",
    "minecraft:zombie",
    "minecraft:zombie",
    "minecraft:spider",
];

#[derive(Debug)]
pub struct MonsterRoom;

impl MonsterRoom {
    pub fn read(reader: &Reader) -> Read<Self> {
        reader.finish()?;
        Ok(Self)
    }
}

/// The game's `Direction.getClockWise` of a horizontal direction.
fn clockwise(face: Face) -> Face {
    match face {
        Face::North => Face::East,
        Face::East => Face::South,
        Face::South => Face::West,
        Face::West => Face::North,
        other => other,
    }
}

fn opposite(face: Face) -> Face {
    match face {
        Face::North => Face::South,
        Face::South => Face::North,
        Face::West => Face::East,
        Face::East => Face::West,
        Face::Up => Face::Down,
        Face::Down => Face::Up,
    }
}

fn facing(state: BlockState) -> Face {
    match state.property("facing") {
        Some("east") => Face::East,
        Some("south") => Face::South,
        Some("west") => Face::West,
        _ => Face::North,
    }
}

fn with_facing(state: BlockState, face: Face) -> BlockState {
    state
        .with_property("facing", crate::blocks::face_name(face))
        .expect("a chest faces every horizontal direction")
}

/// `StructurePiece.reorient`: a chest turned away from the one solid wall beside it,
/// or else from whichever walls block its front.
fn reorient(level: &mut Level<'_>, pos: BlockPos, state: BlockState) -> BlockState {
    let mut wall = None;
    for face in HORIZONTAL {
        let beside = level.block(relative(pos, face));
        if same_block(beside, blocks::CHEST) {
            return state;
        }
        if beside.props().is_solid_render() {
            if wall.is_some() {
                wall = None;
                break;
            }
            wall = Some(face);
        }
    }
    if let Some(face) = wall {
        return with_facing(state, opposite(face));
    }
    let mut face = facing(state);
    if level.block(relative(pos, face)).props().is_solid_render() {
        face = opposite(face);
    }
    if level.block(relative(pos, face)).props().is_solid_render() {
        face = clockwise(face);
    }
    if level.block(relative(pos, face)).props().is_solid_render() {
        face = opposite(face);
    }
    with_facing(state, face)
}

/// What the game saves for a chest whose loot is still to be rolled
/// (`RandomizableContainer.trySaveLootTable`): the table, and the seed unless it is
/// nought; and, as for every block entity it saves, its components, which are none.
pub fn chest_data(seed: i64) -> clustine_world::BlockEntity {
    let mut members = vec![("LootTable", Nbt::String(SIMPLE_DUNGEON.to_string()))];
    if seed != 0 {
        members.push(("LootTableSeed", Nbt::Long(seed)));
    }
    members.push(("components", compound(Vec::new())));
    block_entity("minecraft:chest", members)
}

/// What the game saves for a fresh spawner given a mob (`BaseSpawner.save`): its
/// settings as a new spawner has them, the mob to spawn next, no other potentials,
/// and no components.
pub fn spawner_data(mob: &str) -> clustine_world::BlockEntity {
    let entity = compound(vec![("id", Nbt::String(mob.to_string()))]);
    let spawn_data = compound(vec![("entity", entity)]);
    block_entity(
        "minecraft:mob_spawner",
        vec![
            ("Delay", Nbt::Short(20)),
            ("MinSpawnDelay", Nbt::Short(200)),
            ("MaxSpawnDelay", Nbt::Short(800)),
            ("SpawnCount", Nbt::Short(4)),
            ("MaxNearbyEntities", Nbt::Short(6)),
            ("RequiredPlayerRange", Nbt::Short(16)),
            ("SpawnRange", Nbt::Short(4)),
            ("SpawnData", spawn_data),
            ("SpawnPotentials", Nbt::List(Vec::new())),
            ("components", compound(Vec::new())),
        ],
    )
}

/// Whether the block at `pos` keeps a block entity of the type of `state`'s block:
/// what the game's `getBlockEntity(pos) instanceof ...` finds.
fn keeps_entity_of(level: &mut Level<'_>, pos: BlockPos, state: BlockState) -> bool {
    let kind = state.block_info().and_then(|info| info.block_entity_type);
    kind.is_some() && level.block_entity(pos).map(|entity| entity.kind) == kind
}

impl Feature for MonsterRoom {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let xr = context.random.next_i32_bounded(2) + 2;
        let (min_x, max_x) = (-xr - 1, xr + 1);
        let zr = context.random.next_i32_bounded(2) + 2;
        let (min_z, max_z) = (-zr - 1, zr + 1);

        // The floor and the ceiling must be solid, and the walls open in one to five
        // places two blocks high.
        let mut holes = 0;
        for dx in min_x..=max_x {
            for dy in -1..=4 {
                for dz in min_z..=max_z {
                    let pos = origin.offset(dx, dy, dz);
                    let solid = context.level.block(pos).props().is_solid();
                    if (dy == -1 || dy == 4) && !solid {
                        return false;
                    }
                    if (dx == min_x || dx == max_x || dz == min_z || dz == max_z)
                        && dy == 0
                        && context.level.block(pos).props().is_air()
                        && context.level.block(pos.offset(0, 1, 0)).props().is_air()
                    {
                        holes += 1;
                    }
                }
            }
        }
        if !(1..=5).contains(&holes) {
            return false;
        }

        for dx in min_x..=max_x {
            for dy in (-1..=3).rev() {
                for dz in min_z..=max_z {
                    let pos = origin.offset(dx, dy, dz);
                    let state = context.level.block(pos);
                    let shell = dx == min_x
                        || dy == -1
                        || dz == min_z
                        || dx == max_x
                        || dy == 4
                        || dz == max_z;
                    if shell {
                        let below = context.level.block(pos.offset(0, -1, 0));
                        if pos.y >= context.level.min_y() && !below.props().is_solid() {
                            context.level.set_block(pos, blocks::CAVE_AIR);
                        } else if state.props().is_solid() && !same_block(state, blocks::CHEST) {
                            let wall = if dy == -1 && context.random.next_i32_bounded(4) != 0 {
                                blocks::MOSSY_COBBLESTONE
                            } else {
                                blocks::COBBLESTONE
                            };
                            safe_set_block(context.level, pos, wall);
                        }
                    } else if !same_block(state, blocks::CHEST)
                        && !same_block(state, blocks::SPAWNER)
                    {
                        safe_set_block(context.level, pos, blocks::CAVE_AIR);
                    }
                }
            }
        }

        // Two chests, each tried at up to three places on the floor against exactly one
        // wall.
        for _ in 0..2 {
            for _ in 0..3 {
                let x = origin.x + context.random.next_i32_bounded(xr * 2 + 1) - xr;
                let z = origin.z + context.random.next_i32_bounded(zr * 2 + 1) - zr;
                let pos = BlockPos::new(x, origin.y, z);
                if !context.level.block(pos).props().is_air() {
                    continue;
                }
                let walls = HORIZONTAL
                    .into_iter()
                    .filter(|face| context.level.block(relative(pos, *face)).props().is_solid())
                    .count();
                if walls != 1 {
                    continue;
                }
                let chest = reorient(context.level, pos, blocks::CHEST);
                safe_set_block(context.level, pos, chest);
                // `RandomizableContainer.setBlockEntityLootTable`: the seed is drawn only
                // where a chest stands to take it.
                if keeps_entity_of(context.level, pos, blocks::CHEST) {
                    let seed = context.random.next_i64();
                    context.level.set_block_entity(pos, Some(chest_data(seed)));
                }
                break;
            }
        }

        safe_set_block(context.level, origin, blocks::SPAWNER);
        if keeps_entity_of(context.level, origin, blocks::SPAWNER) {
            let mob = MOBS[context.random.next_i32_bounded(4) as usize];
            context
                .level
                .set_block_entity(origin, Some(spawner_data(mob)));
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::BLOCK_ENTITY_TYPES;
    use clustine_protocol::codec::Reader as NbtReader;

    use super::*;
    use crate::features::ground::shared::testing::WithEntities;
    use crate::testing::World;

    fn read(data: &[u8]) -> Nbt {
        let mut reader = NbtReader::new(data);
        let nbt = reader.nbt().unwrap().unwrap();
        reader.finish().unwrap();
        nbt
    }

    /// Stone with a hollow where the room will be, around (8, 40, 8), and a doorway
    /// two blocks high through its east wall, whichever size the room draws.
    fn cave() -> WithEntities {
        let mut world = World::flat(blocks::STONE, 100);
        for x in 6..=10 {
            for z in 6..=10 {
                for y in 40..=42 {
                    world.set(BlockPos::new(x, y, z), blocks::AIR);
                }
            }
        }
        for x in 11..=13 {
            for y in 40..=41 {
                world.set(BlockPos::new(x, y, 8), blocks::AIR);
            }
        }
        WithEntities::new(world)
    }

    #[test]
    fn a_room_with_a_doorway_is_walled_and_gets_a_spawner_and_its_chests() {
        let mut placed = 0;
        for seed in 0..30 {
            let mut world = cave();
            let origin = BlockPos::new(8, 40, 8);
            if !world.with_context(seed, |context| MonsterRoom.place(context, origin)) {
                continue;
            }
            placed += 1;
            assert_eq!(world.world.get(origin), blocks::SPAWNER);
            let spawner = world.entities.get(&origin).unwrap();
            assert_eq!(
                BLOCK_ENTITY_TYPES[usize::from(spawner.kind)],
                "minecraft:mob_spawner"
            );
            let data = read(&spawner.data);
            let id = data
                .get("SpawnData")
                .and_then(|spawn| spawn.get("entity"))
                .and_then(|entity| entity.get("id"))
                .unwrap();
            let Nbt::String(id) = id else { panic!() };
            assert!(MOBS.contains(&id.as_str()));
            assert_eq!(data.get("Delay"), Some(&Nbt::Short(20)));
            assert_eq!(data.get("SpawnPotentials"), Some(&Nbt::List(Vec::new())));
            // Every chest has the dungeon's loot and a seed.
            for (pos, entity) in &world.entities {
                if *pos == origin {
                    continue;
                }
                assert_eq!(
                    BLOCK_ENTITY_TYPES[usize::from(entity.kind)],
                    "minecraft:chest"
                );
                let data = read(&entity.data);
                assert_eq!(
                    data.get("LootTable"),
                    Some(&Nbt::String(SIMPLE_DUNGEON.to_string()))
                );
                assert!(matches!(data.get("LootTableSeed"), Some(Nbt::Long(_))));
                assert_eq!(pos.y, 40);
            }
            // The floor is cobblestone, mossy or not.
            let floor = world.world.get(BlockPos::new(8, 39, 8));
            assert!(floor == blocks::COBBLESTONE || floor == blocks::MOSSY_COBBLESTONE);
        }
        assert!(placed > 0);
    }

    #[test]
    fn a_room_in_solid_stone_has_no_opening_and_is_not_placed() {
        let mut world = WithEntities::new(World::flat(blocks::STONE, 100));
        assert!(!world.with_context(1, |context| {
            MonsterRoom.place(context, BlockPos::new(8, 40, 8))
        }));
        assert!(world.world.writes.is_empty());
    }

    #[test]
    fn a_spawner_s_data_is_what_the_game_saves_for_a_fresh_spawner() {
        let data = read(&spawner_data("minecraft:zombie").data);
        let Nbt::Compound(members) = &data else {
            panic!()
        };
        let keys: Vec<&str> = members.iter().map(|(key, _)| key.as_str()).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            [
                "Delay",
                "MaxNearbyEntities",
                "MaxSpawnDelay",
                "MinSpawnDelay",
                "RequiredPlayerRange",
                "SpawnCount",
                "SpawnData",
                "SpawnPotentials",
                "SpawnRange",
                "components"
            ]
        );
        assert_eq!(data.get("components"), Some(&Nbt::Compound(Vec::new())));
        assert_eq!(data.get("SpawnCount"), Some(&Nbt::Short(4)));
        assert_eq!(data.get("MaxSpawnDelay"), Some(&Nbt::Short(800)));
        // An empty list is written with no type of element, as the game writes it.
        let bytes = spawner_data("minecraft:zombie").data;
        assert_eq!(bytes[0], 10);
    }

    #[test]
    fn a_chest_of_seed_nought_saves_no_seed() {
        let data = read(&chest_data(0).data);
        assert_eq!(data.get("LootTableSeed"), None);
        assert_eq!(data.get("components"), Some(&Nbt::Compound(Vec::new())));
    }

    #[test]
    fn a_chest_turns_away_from_its_one_wall() {
        let mut world = World::flat(blocks::STONE, 30);
        let pos = BlockPos::new(5, 40, 5);
        world.set(BlockPos::new(5, 40, 4), blocks::STONE);
        world.with_level(|level| {
            let chest = reorient(level, pos, blocks::CHEST);
            assert_eq!(chest.property("facing"), Some("south"));
        });
    }
}
