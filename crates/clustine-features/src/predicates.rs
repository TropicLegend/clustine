// Adapted from SteelMC's `steel-core/src/worldgen/feature/predicates.rs` and the ore
// targets of `steel-core/src/worldgen/feature/features/ore.rs`.

//! What features and modifiers test blocks with: the game's block predicates
//! (`BlockPredicate`, a test of the world at a position, used by filters, columns and
//! providers) and its rule tests (`RuleTest`, a test of one block state, used by ores).

use clustine_data::{BlockState, Face, SupportType};
use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::blocks::{
    BlockSet, FluidSet, block_id, in_block_tag, offset, read_face, read_offset, read_state,
    read_vec,
};
use crate::context::{Context, Level};
use crate::reader::{Read, Reader};
use crate::survive::can_survive;
use crate::values::{Generation, VerticalAnchor};

/// An offset from the position tested.
type Offset = (i32, i32, i32);

/// A test of the world at a position (`BlockPredicate`).
#[derive(Debug, Clone, PartialEq)]
pub enum BlockPredicate {
    True,
    AllOf(Vec<BlockPredicate>),
    AnyOf(Vec<BlockPredicate>),
    Not(Box<BlockPredicate>),
    MatchingBlocks(BlockSet, Offset),
    MatchingBlockTag(&'static str, Offset),
    MatchingFluids(FluidSet, Offset),
    Solid(Offset),
    Replaceable(Offset),
    WouldSurvive(BlockState, Offset),
    HasSturdyFace(Face, Offset),
    InsideWorldBounds(Offset),
    HeightRange(VerticalAnchor, VerticalAnchor),
    /// Every position of the box from `min` to `max` around the one tested matches.
    VolumeMatch {
        min: Offset,
        max: Offset,
        predicate: Box<BlockPredicate>,
    },
}

impl BlockPredicate {
    pub fn read(reader: &Reader) -> Read<Self> {
        let offset = || read_offset(reader, "offset");
        let list = |key: &str| -> Read<Vec<BlockPredicate>> {
            reader.field(key)?.list()?.iter().map(Self::read).collect()
        };
        let predicate = match reader.kind()? {
            "minecraft:true" => Self::True,
            "minecraft:all_of" => Self::AllOf(list("predicates")?),
            "minecraft:any_of" => Self::AnyOf(list("predicates")?),
            "minecraft:not" => Self::Not(Box::new(Self::read(&reader.field("predicate")?)?)),
            "minecraft:matching_blocks" => {
                Self::MatchingBlocks(BlockSet::read(&reader.field("blocks")?)?, offset()?)
            }
            "minecraft:matching_block_tag" => {
                let tag = reader.field("tag")?;
                let name = tag.text()?;
                if crate::blocks::block_tag(name).is_none() {
                    return Err(tag.error(format!("{name} is not a tag of blocks")));
                }
                Self::MatchingBlockTag(name, offset()?)
            }
            "minecraft:matching_fluids" => {
                Self::MatchingFluids(FluidSet::read(&reader.field("fluids")?)?, offset()?)
            }
            "minecraft:solid" => Self::Solid(offset()?),
            "minecraft:replaceable" => Self::Replaceable(offset()?),
            "minecraft:would_survive" => {
                Self::WouldSurvive(read_state(&reader.field("state")?)?, offset()?)
            }
            "minecraft:has_sturdy_face" => {
                Self::HasSturdyFace(read_face(&reader.field("direction")?)?, offset()?)
            }
            "minecraft:inside_world_bounds" => Self::InsideWorldBounds(offset()?),
            "minecraft:height_range" => Self::HeightRange(
                VerticalAnchor::read(&reader.field("min_inclusive")?)?,
                VerticalAnchor::read(&reader.field("max_inclusive")?)?,
            ),
            "minecraft:volume_match" => Self::VolumeMatch {
                min: read_vec(&reader.field("min")?)?,
                max: read_vec(&reader.field("max")?)?,
                predicate: Box::new(Self::read(&reader.field("match")?)?),
            },
            other => return Err(reader.error(format!("{other} is not a block predicate"))),
        };
        reader.finish()?;
        Ok(predicate)
    }

    /// Whether the world at `pos` passes (`test(level, pos)`). The game's
    /// combinations stop at the first that decides.
    pub fn test(&self, context: &mut Context<'_, '_>, pos: BlockPos) -> bool {
        let sea_level = context.settings.generation().sea_level;
        self.test_in(context.level, sea_level, pos)
    }

    /// [`Self::test`] with only the world and the sea level.
    pub fn test_in(&self, level: &mut Level<'_>, sea_level: i32, pos: BlockPos) -> bool {
        match self {
            Self::True => true,
            Self::AllOf(all) => all
                .iter()
                .all(|predicate| predicate.test_in(level, sea_level, pos)),
            Self::AnyOf(any) => any
                .iter()
                .any(|predicate| predicate.test_in(level, sea_level, pos)),
            Self::Not(predicate) => !predicate.test_in(level, sea_level, pos),
            Self::MatchingBlocks(blocks, by) => blocks.contains(level.block(offset(pos, *by))),
            Self::MatchingBlockTag(tag, by) => in_block_tag(level.block(offset(pos, *by)), tag),
            Self::MatchingFluids(fluids, by) => fluids.contains(level.block(offset(pos, *by))),
            Self::Solid(by) => level.block(offset(pos, *by)).props().is_solid(),
            Self::Replaceable(by) => level.block(offset(pos, *by)).props().can_be_replaced(),
            Self::WouldSurvive(state, by) => can_survive(*state, offset(pos, *by), level),
            Self::HasSturdyFace(face, by) => level
                .block(offset(pos, *by))
                .props()
                .is_face_sturdy(*face, SupportType::Full),
            Self::InsideWorldBounds(by) => !level.is_outside_build_height(offset(pos, *by).y),
            Self::HeightRange(min, max) => {
                // The world's heights, which in every dimension the data uses this in
                // agree with the generator's at the bottom and the sea.
                let at = Generation {
                    min_y: level.min_y(),
                    height: level.height(),
                    sea_level,
                };
                (min.resolve(at)..=max.resolve(at)).contains(&pos.y)
            }
            Self::VolumeMatch {
                min,
                max,
                predicate,
            } => {
                for x in min.0..=max.0 {
                    for y in min.1..=max.1 {
                        for z in min.2..=max.2 {
                            if !predicate.test_in(level, sea_level, offset(pos, (x, y, z))) {
                                return false;
                            }
                        }
                    }
                }
                true
            }
        }
    }
}

/// A test of one block state where an ore or a template would replace it (`RuleTest`).
#[derive(Debug, Clone, PartialEq)]
pub enum RuleTest {
    AlwaysTrue,
    /// The block, by its id.
    BlockMatch(usize),
    BlockStateMatch(BlockState),
    TagMatch(&'static str),
    RandomBlockMatch(usize, f32),
    RandomBlockStateMatch(BlockState, f32),
    /// The position's y lies in the range, both ends included.
    HeightMatch(i32, i32),
    AllOf(Vec<RuleTest>),
    AnyOf(Vec<RuleTest>),
    Not(Box<RuleTest>),
}

impl RuleTest {
    pub fn read(reader: &Reader) -> Read<Self> {
        let block = |key: &str| -> Read<usize> {
            let field = reader.field(key)?;
            let name = field.text()?;
            clustine_data::block_id_by_name(name)
                .ok_or_else(|| field.error(format!("{name} is not a block")))
        };
        let list = |key: &str| -> Read<Vec<RuleTest>> {
            reader.field(key)?.list()?.iter().map(Self::read).collect()
        };
        let test = match reader.field("predicate_type")?.text()? {
            "minecraft:always_true" => Self::AlwaysTrue,
            "minecraft:block_match" => Self::BlockMatch(block("block")?),
            "minecraft:blockstate_match" => {
                Self::BlockStateMatch(read_state(&reader.field("block_state")?)?)
            }
            "minecraft:tag_match" => {
                let tag = reader.field("tag")?;
                let name = tag.text()?;
                if crate::blocks::block_tag(name).is_none() {
                    return Err(tag.error(format!("{name} is not a tag of blocks")));
                }
                Self::TagMatch(name)
            }
            "minecraft:random_block_match" => {
                Self::RandomBlockMatch(block("block")?, reader.field("probability")?.float()?)
            }
            "minecraft:random_blockstate_match" => Self::RandomBlockStateMatch(
                read_state(&reader.field("block_state")?)?,
                reader.field("probability")?.float()?,
            ),
            "minecraft:height_match" => Self::HeightMatch(
                reader.field("min_inclusive")?.int()?,
                reader.field("max_inclusive")?.int()?,
            ),
            "minecraft:all_of" => Self::AllOf(list("rules")?),
            "minecraft:any_of" => Self::AnyOf(list("rules")?),
            "minecraft:not" => Self::Not(Box::new(Self::read(&reader.field("rule")?)?)),
            other => return Err(reader.error(format!("{other} is not a rule test"))),
        };
        reader.finish()?;
        Ok(test)
    }

    /// Whether `state`, at height `y`, passes. Only the random tests draw, and only
    /// once their block has matched.
    pub fn test<R: Random + ?Sized>(&self, state: BlockState, y: i32, random: &mut R) -> bool {
        match self {
            Self::AlwaysTrue => true,
            Self::BlockMatch(block) => block_id(state) == *block,
            Self::BlockStateMatch(wanted) => state == *wanted,
            Self::TagMatch(tag) => in_block_tag(state, tag),
            Self::RandomBlockMatch(block, probability) => {
                block_id(state) == *block && random.next_f32() < *probability
            }
            Self::RandomBlockStateMatch(wanted, probability) => {
                state == *wanted && random.next_f32() < *probability
            }
            Self::HeightMatch(min, max) => (*min..=*max).contains(&y),
            Self::AllOf(all) => all.iter().all(|test| test.test(state, y, random)),
            Self::AnyOf(any) => any.iter().any(|test| test.test(state, y, random)),
            Self::Not(test) => !test.test(state, y, random),
        }
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use clustine_noise::WorldgenRandom;
    use clustine_worldgen_data::feature::Data;

    use super::*;
    use crate::testing::World;

    fn predicate(data: &'static Data) -> BlockPredicate {
        BlockPredicate::read(&Reader::new(data, "test")).unwrap()
    }

    #[test]
    fn matching_blocks_looks_at_the_offset() {
        static PODZOL_BELOW: Data = Data::Object(&[
            ("type", Data::Text("minecraft:matching_blocks")),
            ("blocks", Data::Text("minecraft:podzol")),
            (
                "offset",
                Data::List(&[Data::Number("0"), Data::Number("-1"), Data::Number("0")]),
            ),
        ]);
        let test = predicate(&PODZOL_BELOW);
        let mut world = World::flat(blocks::PODZOL, 64);
        assert!(world.with_context(0, |context| test.test(context, BlockPos::new(0, 65, 0))));
        assert!(!world.with_context(0, |context| test.test(context, BlockPos::new(0, 70, 0))));
    }

    #[test]
    fn combinations_and_tags_decide_as_the_game_does() {
        static AIR_NOT_ON_PODZOL: Data = Data::Object(&[
            ("type", Data::Text("minecraft:all_of")),
            (
                "predicates",
                Data::List(&[
                    Data::Object(&[
                        ("type", Data::Text("minecraft:matching_block_tag")),
                        ("tag", Data::Text("minecraft:air")),
                    ]),
                    Data::Object(&[
                        ("type", Data::Text("minecraft:not")),
                        (
                            "predicate",
                            Data::Object(&[
                                ("type", Data::Text("minecraft:matching_blocks")),
                                ("blocks", Data::Text("minecraft:podzol")),
                                (
                                    "offset",
                                    Data::List(&[
                                        Data::Number("0"),
                                        Data::Number("-1"),
                                        Data::Number("0"),
                                    ]),
                                ),
                            ]),
                        ),
                    ]),
                ]),
            ),
        ]);
        let test = predicate(&AIR_NOT_ON_PODZOL);
        let mut world = World::flat(blocks::GRASS_BLOCK, 64);
        assert!(world.with_context(0, |context| test.test(context, BlockPos::new(1, 65, 1))));
        world.set(BlockPos::new(1, 64, 1), blocks::PODZOL);
        assert!(!world.with_context(0, |context| test.test(context, BlockPos::new(1, 65, 1))));
        assert!(!world.with_context(0, |context| test.test(context, BlockPos::new(2, 64, 2))));
    }

    #[test]
    fn the_simple_tests_read_the_block_s_properties() {
        let mut world = World::flat(blocks::STONE, 64);
        world.set(BlockPos::new(0, 65, 0), blocks::SHORT_GRASS);
        world.set(BlockPos::new(0, 66, 0), blocks::WATER);
        let at = BlockPos::new(0, 65, 0);
        let check = |world: &mut World, test: BlockPredicate| {
            world.with_context(0, |context| test.test(context, at))
        };
        assert!(!check(&mut world, BlockPredicate::Solid((0, 0, 0))));
        assert!(check(&mut world, BlockPredicate::Solid((0, -1, 0))));
        assert!(check(&mut world, BlockPredicate::Replaceable((0, 0, 0))));
        assert!(check(
            &mut world,
            BlockPredicate::HasSturdyFace(Face::Up, (0, -1, 0))
        ));
        assert!(!check(
            &mut world,
            BlockPredicate::HasSturdyFace(Face::Up, (0, 0, 0))
        ));
        static WATER: Data = Data::Text("minecraft:water");
        let water = FluidSet::read(&Reader::new(&WATER, "t")).unwrap();
        assert!(check(
            &mut world,
            BlockPredicate::MatchingFluids(water.clone(), (0, 1, 0))
        ));
        assert!(!check(
            &mut world,
            BlockPredicate::MatchingFluids(water, (0, 0, 0))
        ));
        assert!(check(
            &mut world,
            BlockPredicate::InsideWorldBounds((0, 254, 0))
        ));
        assert!(!check(
            &mut world,
            BlockPredicate::InsideWorldBounds((0, 255, 0))
        ));
        assert!(!check(
            &mut world,
            BlockPredicate::WouldSurvive(blocks::OAK_SAPLING, (5, 0, 5))
        ));
    }

    #[test]
    fn a_height_range_counts_from_the_sea() {
        static UP_TO_THE_SEA: Data = Data::Object(&[
            ("type", Data::Text("minecraft:height_range")),
            (
                "max_inclusive",
                Data::Object(&[("relative_to_sea_level", Data::Number("0"))]),
            ),
            (
                "min_inclusive",
                Data::Object(&[("above_bottom", Data::Number("0"))]),
            ),
        ]);
        let test = predicate(&UP_TO_THE_SEA);
        let mut world = World::flat(blocks::STONE, 0);
        assert!(world.with_context(0, |context| test.test(context, BlockPos::new(0, 63, 0))));
        assert!(!world.with_context(0, |context| test.test(context, BlockPos::new(0, 64, 0))));
        assert!(world.with_context(0, |context| test.test(context, BlockPos::new(0, -64, 0))));
    }

    #[test]
    fn a_volume_matches_only_if_every_block_of_it_does() {
        let test = BlockPredicate::VolumeMatch {
            min: (-2, -2, -2),
            max: (2, -1, 2),
            predicate: Box::new(BlockPredicate::Not(Box::new(
                BlockPredicate::MatchingBlockTag("minecraft:air", (0, 0, 0)),
            ))),
        };
        let mut world = World::flat(blocks::SAND, 64);
        let at = BlockPos::new(4, 65, 4);
        assert!(world.with_context(0, |context| test.test(context, at)));
        world.set(BlockPos::new(6, 63, 2), blocks::AIR);
        assert!(!world.with_context(0, |context| test.test(context, at)));
    }

    #[test]
    fn an_ore_s_rule_tests_its_block_and_height() {
        static IRON: Data = Data::Object(&[
            ("predicate_type", Data::Text("minecraft:any_of")),
            (
                "rules",
                Data::List(&[
                    Data::Object(&[
                        ("predicate_type", Data::Text("minecraft:all_of")),
                        (
                            "rules",
                            Data::List(&[
                                Data::Object(&[
                                    ("predicate_type", Data::Text("minecraft:tag_match")),
                                    (
                                        "tag",
                                        Data::Text("minecraft:height_specific_ore_replaceables"),
                                    ),
                                ]),
                                Data::Object(&[
                                    ("max_inclusive", Data::Number("2031")),
                                    ("min_inclusive", Data::Number("0")),
                                    ("predicate_type", Data::Text("minecraft:height_match")),
                                ]),
                            ]),
                        ),
                    ]),
                    Data::Object(&[
                        ("block", Data::Text("minecraft:deepslate")),
                        ("predicate_type", Data::Text("minecraft:block_match")),
                    ]),
                ]),
            ),
        ]);
        let test = RuleTest::read(&Reader::new(&IRON, "test")).unwrap();
        let mut random = WorldgenRandom::from_seed(0);
        let replaceable = crate::blocks::block_tag("minecraft:height_specific_ore_replaceables")
            .unwrap()
            .first()
            .map(|id| clustine_data::BLOCKS[*id as usize].default_state)
            .unwrap();
        assert!(test.test(replaceable, 10, &mut random));
        if !crate::blocks::same_block(replaceable, blocks::DEEPSLATE) {
            assert!(!test.test(replaceable, -1, &mut random));
        }
        assert!(test.test(blocks::DEEPSLATE, -30, &mut random));
        assert!(!test.test(blocks::DIRT, 10, &mut random));
    }

    #[test]
    fn a_random_match_draws_only_once_its_block_matched() {
        let test = RuleTest::RandomBlockMatch(block_id(blocks::STONE), 0.5);
        let mut random = WorldgenRandom::from_seed(3);
        let mut untouched = random.clone();
        assert!(!test.test(blocks::DIRT, 0, &mut random));
        assert_eq!(random.next_i64(), untouched.next_i64());
        let mut random = WorldgenRandom::from_seed(3);
        let mut expected = random.clone();
        assert_eq!(
            test.test(blocks::STONE, 0, &mut random),
            expected.next_f32() < 0.5
        );
    }
}
