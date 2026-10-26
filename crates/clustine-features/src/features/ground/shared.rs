//! What several of the ground's features share: the game's walk through a box of
//! blocks, the marks above a block placed, safe writes, the weather's tests of
//! freezing and snow, fluid states as the data writes them, and block entities' data.

use std::sync::OnceLock;

use clustine_data::{BLOCK_ENTITY_TYPES, BlockState, FLUIDS, Face, blocks};
use clustine_protocol::codec::Writer;
use clustine_protocol::nbt::Nbt;
use clustine_terrain::surface::BiomeTemperature;
use clustine_world::{Biome, BlockEntity, BlockPos};

use crate::blocks::{block_default, has_water, in_block_tag, relative, same_block};
use crate::context::Level;
use crate::reader::{Read, Reader};
use crate::survive::can_survive;

/// The positions of the box from `min` to `max`, both included, in the order of the
/// game's `BlockPos.betweenClosed`: x fastest, then y, then z. Features that draw a
/// number for each position draw them in this order.
pub fn between_closed(min: BlockPos, max: BlockPos) -> impl Iterator<Item = BlockPos> {
    (min.z..=max.z).flat_map(move |z| {
        (min.y..=max.y).flat_map(move |y| (min.x..=max.x).map(move |x| BlockPos::new(x, y, z)))
    })
}

/// `Feature.markAboveForPostProcessing`: the two blocks above `pos`, up to the first
/// air, take their shape when the chunk is finished.
pub fn mark_above_for_post_processing(level: &mut Level<'_>, pos: BlockPos) {
    let mut at = pos;
    for _ in 0..2 {
        at = at.offset(0, 1, 0);
        if level.block(at).props().is_air() {
            return;
        }
        level.mark_for_post_processing(at);
    }
}

/// `Feature.isReplaceable(BlockTags.FEATURES_CANNOT_REPLACE)`: what a monster room
/// and a lake may write over.
pub fn feature_can_replace(state: BlockState) -> bool {
    !in_block_tag(state, "minecraft:features_cannot_replace")
}

/// `Feature.safeSetBlock`: sets the block where what is there may be replaced.
pub fn safe_set_block(level: &mut Level<'_>, pos: BlockPos, state: BlockState) -> bool {
    let there = level.block(pos);
    feature_can_replace(there) && level.set_block(pos, state)
}

/// The id of a fluid by its name, an index into [`FLUIDS`].
pub fn fluid_named(name: &str) -> Option<u8> {
    FLUIDS
        .iter()
        .position(|fluid| *fluid == name)
        .map(|id| u8::try_from(id).expect("the game has few fluids"))
}

/// A fluid state as the data writes one (`FluidState.CODEC`): the fluid's name and
/// its properties, `falling` and, for a flowing fluid, `level`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluidState {
    /// The fluid, an index into [`FLUIDS`]: what a tick of it is scheduled for.
    pub fluid: u8,
    /// The block the game makes of it (`createLegacyBlock`).
    pub block: BlockState,
}

impl FluidState {
    pub fn read(reader: &Reader) -> Read<Self> {
        let name = reader.field("id")?.text()?;
        let fluid =
            fluid_named(name).ok_or_else(|| reader.error(format!("{name} is not a fluid")))?;
        let mut falling = false;
        let mut amount = None;
        if let Some(properties) = reader.optional("properties") {
            if let Some(value) = properties.optional("falling") {
                falling = match value.text()? {
                    "true" => true,
                    "false" => false,
                    other => return Err(value.error(format!("{other} is not true or false"))),
                };
            }
            if let Some(value) = properties.optional("level") {
                let text = value.text()?;
                let level: u8 = text
                    .parse()
                    .ok()
                    .filter(|level| (1..=8).contains(level))
                    .ok_or_else(|| value.error(format!("{text} is not a level of a fluid")))?;
                amount = Some(level);
            }
            properties.finish()?;
        }
        reader.finish()?;
        // The flowing fluids' blocks are the still ones'. A source is level 0 of its
        // block; a flowing fluid counts down from 8, and one that falls adds 8
        // (`FlowingFluid.getLegacyLevel`).
        let (block_name, source) = match name {
            "minecraft:water" => ("minecraft:water", true),
            "minecraft:flowing_water" => ("minecraft:water", false),
            "minecraft:lava" => ("minecraft:lava", true),
            "minecraft:flowing_lava" => ("minecraft:lava", false),
            _ => return Err(reader.error(format!("{name} makes no block"))),
        };
        if source && amount.is_some() {
            return Err(reader.error("a source has no level"));
        }
        let level = if source {
            0
        } else {
            8 - amount.unwrap_or(1) + if falling { 8 } else { 0 }
        };
        let block = block_default(block_name)
            .and_then(|state| state.with_property("level", &level.to_string()))
            .ok_or_else(|| reader.error(format!("{name} has no block of level {level}")))?;
        Ok(Self { fluid, block })
    }
}

/// The temperatures of biomes, whose noises are the same in every world.
fn temperature() -> &'static BiomeTemperature {
    static TEMPERATURE: OnceLock<BiomeTemperature> = OnceLock::new();
    TEMPERATURE.get_or_init(BiomeTemperature::new)
}

/// `Biome.warmEnoughToRain`.
pub fn warm_enough_to_rain(biome: Biome, pos: BlockPos, sea_level: i32) -> bool {
    temperature().at(biome, pos.x, pos.y, pos.z, sea_level) >= 0.15
}

/// `Biome.shouldFreeze`: whether the water source at `pos` turns to ice. The light of
/// blocks, which the game asks as well, is nought while features are placed.
pub fn should_freeze(
    level: &mut Level<'_>,
    biome: Biome,
    pos: BlockPos,
    sea_level: i32,
    check_neighbours: bool,
) -> bool {
    if warm_enough_to_rain(biome, pos, sea_level)
        || level.is_outside_build_height(pos.y)
        || level.block_light(pos) >= 10
    {
        return false;
    }
    let state = level.block(pos);
    // The still water (`Fluids.WATER`) of a block of water itself: not a flowing one,
    // not a waterlogged block.
    let still = fluid_named("minecraft:water");
    if !state.props().is_liquid() || state.props().fluid().map(|fluid| fluid.fluid) != still {
        return false;
    }
    if !check_neighbours {
        return true;
    }
    let surrounded = [Face::West, Face::East, Face::North, Face::South]
        .into_iter()
        .all(|face| has_water(level.block(relative(pos, face))));
    !surrounded
}

/// `Biome.shouldSnow`: whether a layer of snow falls at `pos`.
///
/// The game asks first whether the biome has rain or snow at all. Every biome that
/// has none is warm at every height a world has (the desert's, the savannas', the
/// badlands' and the Nether's at 2.0, the End's at 0.5, which would need 280 blocks
/// above the snow line to cool to 0.15), so that answer never changes this one.
pub fn should_snow(level: &mut Level<'_>, biome: Biome, pos: BlockPos, sea_level: i32) -> bool {
    if warm_enough_to_rain(biome, pos, sea_level)
        || level.is_outside_build_height(pos.y)
        || level.block_light(pos) >= 10
    {
        return false;
    }
    let state = level.block(pos);
    (state.props().is_air() || same_block(state, blocks::SNOW))
        && can_survive(blocks::SNOW, pos, level)
}

/// The index of a block entity type in [`BLOCK_ENTITY_TYPES`].
pub fn block_entity_type(name: &str) -> u8 {
    let index = BLOCK_ENTITY_TYPES
        .iter()
        .position(|kind| *kind == name)
        .unwrap_or_else(|| panic!("{name} is a type of block entity of the pinned jar"));
    u8::try_from(index).expect("the game has fewer than 256 types of block entity")
}

/// The data of a block entity as a chunk keeps it: one compound in the nameless form
/// of the network, its members in the order the game's `CompoundTag` writes them.
pub fn block_entity(kind: &str, members: Vec<(&str, Nbt)>) -> BlockEntity {
    let mut writer = Writer::new();
    writer.put_nbt(&compound(members));
    BlockEntity {
        kind: block_entity_type(kind),
        data: writer.into_bytes(),
    }
}

/// A compound with `members` put in this order, as the game's `CompoundTag` holds it:
/// in a Java `HashMap`, which writes its keys in the order of their buckets and, within
/// a bucket, in the order they were put.
pub fn compound(members: Vec<(&str, Nbt)>) -> Nbt {
    let mut capacity = 16_usize;
    // The map doubles once it holds more than three quarters of its capacity.
    while members.len() > capacity / 4 * 3 {
        capacity *= 2;
    }
    let mut keyed: Vec<(usize, (String, Nbt))> = members
        .into_iter()
        .map(|(key, value)| (java_bucket(key, capacity), (key.to_string(), value)))
        .collect();
    // A stable sort keeps the order of putting within a bucket.
    keyed.sort_by_key(|(bucket, _)| *bucket);
    Nbt::Compound(keyed.into_iter().map(|(_, member)| member).collect())
}

/// The bucket of a Java `HashMap` of `capacity` that a string key falls in:
/// `String.hashCode` over its UTF-16 units, spread by its upper half.
fn java_bucket(key: &str, capacity: usize) -> usize {
    let hash = key.encode_utf16().fold(0_i32, |hash, unit| {
        hash.wrapping_mul(31).wrapping_add(i32::from(unit))
    });
    let hash = hash as u32;
    ((hash ^ (hash >> 16)) as usize) & (capacity - 1)
}

#[cfg(test)]
pub mod testing {
    //! A toy world that keeps block entities, around the crate's toy world.

    use std::collections::BTreeMap;

    use clustine_data::BlockState;
    use clustine_world::generation::{
        FinalHeightmap, Neighbourhood, PieceKey, PieceState, Placing, StartRef, WorldgenHeightmap,
    };
    use clustine_world::{Biome, BlockEntity, BlockPos, ChunkPos, ScheduledTick};

    use clustine_noise::WorldgenRandom;

    use crate::context::{Context, Level};
    use crate::testing::{World, settings};

    pub struct WithEntities {
        pub world: World,
        pub entities: BTreeMap<BlockPos, BlockEntity>,
    }

    impl WithEntities {
        pub fn new(world: World) -> Self {
            Self {
                world,
                entities: BTreeMap::new(),
            }
        }

        /// Runs `f` with a context whose random source is seeded with `seed`.
        pub fn with_context<R>(
            &mut self,
            seed: u64,
            f: impl FnOnce(&mut Context<'_, '_>) -> R,
        ) -> R {
            let settings = settings();
            let mut random = WorldgenRandom::from_seed(seed);
            let mut level = Level::new(self, settings.zoom(), -64, 384);
            let mut context = Context {
                level: &mut level,
                random: &mut random,
                settings,
                top: None,
            };
            f(&mut context)
        }
    }

    impl Neighbourhood for WithEntities {
        fn centre(&self) -> ChunkPos {
            self.world.centre()
        }

        fn block(&mut self, at: BlockPos) -> BlockState {
            self.world.block(at)
        }

        /// As a chunk does: a block that keeps an entity gets an empty one, and any
        /// other loses what was there.
        fn set_block(&mut self, at: BlockPos, state: BlockState) -> bool {
            if !self.world.set_block(at, state) {
                return false;
            }
            match state.block_info().and_then(|info| info.block_entity_type) {
                Some(kind) => {
                    let keeps = self
                        .entities
                        .get(&at)
                        .is_some_and(|there| there.kind == kind);
                    if !keeps {
                        self.entities.insert(
                            at,
                            BlockEntity {
                                kind,
                                data: Vec::new(),
                            },
                        );
                    }
                }
                None => {
                    self.entities.remove(&at);
                }
            }
            true
        }

        fn block_entity(&mut self, at: BlockPos) -> Option<&BlockEntity> {
            self.entities.get(&at)
        }

        fn set_block_entity(&mut self, at: BlockPos, entity: Option<BlockEntity>) -> bool {
            let Some(there) = self.entities.get_mut(&at) else {
                return entity.is_none();
            };
            match entity {
                Some(entity) if entity.kind != there.kind => false,
                Some(entity) => {
                    *there = entity;
                    true
                }
                None => {
                    there.data.clear();
                    true
                }
            }
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

        fn add_entity(&mut self, entity: clustine_world::Entity) -> bool {
            self.world.add_entity(entity)
        }

        fn starts(&self) -> &[StartRef] {
            &[]
        }

        fn piece(&mut self, key: PieceKey) -> PieceState {
            self.world.piece(key)
        }

        fn set_piece(&mut self, key: PieceKey, state: PieceState) {
            self.world.set_piece(key, state);
        }

        fn attempt(&mut self, _: Placing, place: &mut dyn FnMut(&mut dyn Neighbourhood)) -> bool {
            place(self);
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use clustine_worldgen_data::feature::Data;

    use super::*;

    #[test]
    fn the_box_is_walked_with_x_fastest_and_z_slowest() {
        let walked: Vec<BlockPos> =
            between_closed(BlockPos::new(0, 0, 0), BlockPos::new(1, 1, 1)).collect();
        assert_eq!(walked[0], BlockPos::new(0, 0, 0));
        assert_eq!(walked[1], BlockPos::new(1, 0, 0));
        assert_eq!(walked[2], BlockPos::new(0, 1, 0));
        assert_eq!(walked[4], BlockPos::new(0, 0, 1));
        assert_eq!(walked.len(), 8);
    }

    #[test]
    fn a_string_s_bucket_follows_java_s_hash_code() {
        // "id".hashCode() is 3355, which spreads to itself: bucket 3355 & 15 = 11.
        assert_eq!(java_bucket("id", 16), 11);
        // "Delay".hashCode() is 65915235.
        let hash = 65_915_235_u32;
        assert_eq!(
            java_bucket("Delay", 16),
            ((hash ^ (hash >> 16)) & 15) as usize
        );
    }

    #[test]
    fn a_falling_source_makes_the_source_s_block_and_ticks_the_still_fluid() {
        static WATER: Data = Data::Object(&[
            ("id", Data::Text("minecraft:water")),
            (
                "properties",
                Data::Object(&[("falling", Data::Text("true"))]),
            ),
        ]);
        static FLOWING: Data = Data::Object(&[
            ("id", Data::Text("minecraft:flowing_lava")),
            (
                "properties",
                Data::Object(&[("falling", Data::Text("true")), ("level", Data::Text("3"))]),
            ),
        ]);
        let water = FluidState::read(&Reader::new(&WATER, "t")).unwrap();
        assert_eq!(water.block, blocks::WATER);
        assert_eq!(FLUIDS[usize::from(water.fluid)], "minecraft:water");
        let lava = FluidState::read(&Reader::new(&FLOWING, "t")).unwrap();
        assert_eq!(lava.block.property("level"), Some("13"));
        assert_eq!(FLUIDS[usize::from(lava.fluid)], "minecraft:flowing_lava");
    }
}
