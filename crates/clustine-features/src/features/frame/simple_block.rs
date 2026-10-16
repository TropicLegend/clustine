// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/simple_block.rs`.

//! `simple_block`: one block from a provider, where it can stay
//! (`SimpleBlockFeature`). A two-block plant places both halves, the pale moss carpet
//! its carpet and what grows on top, and the block may be given a tick.

use clustine_blocks::mossy_carpet_updated;
use clustine_data::{BlockClass, BlockState, Face, blocks};
use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::blocks::{HORIZONTAL, block_id, face_name, has_water, is_class, relative, same_block};
use crate::context::{Context, Level};
use crate::feature::Feature;
use crate::providers::Provider;
use crate::reader::{Read, Reader};
use crate::survive::can_survive;

#[derive(Debug)]
pub struct SimpleBlock {
    to_place: Provider,
    schedule_tick: bool,
}

impl SimpleBlock {
    pub fn read(reader: &Reader) -> Read<Self> {
        Ok(Self {
            to_place: Provider::read(&reader.field("to_place")?)?,
            schedule_tick: match reader.optional("schedule_tick") {
                Some(tick) => tick.boolean()?,
                None => false,
            },
        })
    }
}

impl Feature for SimpleBlock {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let Some(state) = self.to_place.optional_state(context, origin) else {
            return false;
        };
        if !can_survive(state, origin, context.level) {
            return false;
        }
        if is_class(state, BlockClass::DoublePlantBlock) {
            // The upper half needs room: air, or a block it may replace that holds the
            // same fluid (water, for tall seagrass).
            let above = context.level.block(relative(origin, Face::Up));
            if !above.props().is_air()
                && (state.props().fluid() != above.props().fluid()
                    || !above.props().can_be_replaced())
            {
                return false;
            }
            place_double_plant(context.level, state, origin);
        } else if same_block(state, blocks::PALE_MOSS_CARPET) {
            let (seed, legacy) = (
                context.settings.seed(),
                context.settings.legacy_random_source(),
            );
            place_mossy_carpet(context.level, origin, seed, legacy);
        } else {
            context.level.set_block(origin, state);
        }
        if self.schedule_tick {
            let placed = context.level.block(origin);
            context
                .level
                .schedule_block_tick(origin, block_id(placed) as u16, 1);
        }
        true
    }
}

/// `DoublePlantBlock.placeAt`: the lower half at `pos` and the upper above it, each
/// waterlogged where water is, if it can be.
fn place_double_plant(level: &mut Level<'_>, state: BlockState, pos: BlockPos) {
    let above = relative(pos, Face::Up);
    let lower = with_water_of(level, pos, half(state, "lower"));
    let upper = with_water_of(level, above, half(state, "upper"));
    level.set_block(pos, lower);
    level.set_block(above, upper);
}

fn half(state: BlockState, half: &str) -> BlockState {
    state.with_property("half", half).unwrap_or(state)
}

/// `copyWaterloggedFrom`.
fn with_water_of(level: &mut Level<'_>, pos: BlockPos, state: BlockState) -> BlockState {
    if state.property("waterlogged").is_none() {
        return state;
    }
    let water = has_water(level.block(pos));
    state
        .with_property("waterlogged", if water { "true" } else { "false" })
        .unwrap_or(state)
}

/// `MossyCarpetBlock.placeAt`: the carpet with sides where it can hold on, then a
/// carpet without a base above it whose sides each stay with an even chance drawn
/// from the region's random source, where any stays.
fn place_mossy_carpet(level: &mut Level<'_>, pos: BlockPos, seed: i64, legacy: bool) {
    let carpet = blocks::PALE_MOSS_CARPET;
    let adjusted = mossy_carpet_updated(carpet, level, pos, true);
    level.set_block(pos, adjusted);
    let topper = mossy_topper(level, pos, seed, legacy);
    if !topper.props().is_air() {
        level.set_block(relative(pos, Face::Up), topper);
        let bottom = mossy_carpet_updated(adjusted, level, pos, true);
        level.set_block(pos, bottom);
    }
}

/// `MossyCarpetBlock.createTopperWithSideChance`.
fn mossy_topper(level: &mut Level<'_>, pos: BlockPos, seed: i64, legacy: bool) -> BlockState {
    let above_pos = relative(pos, Face::Up);
    let above = level.block(above_pos);
    let carpet_above = same_block(above, blocks::PALE_MOSS_CARPET);
    let based_above = carpet_above && above.property("bottom") == Some("true");
    if based_above || !carpet_above && !above.props().can_be_replaced() {
        return blocks::AIR;
    }
    let no_base = blocks::PALE_MOSS_CARPET
        .with_property("bottom", "false")
        .expect("the pale moss carpet has a bottom");
    let mut topper = mossy_carpet_updated(no_base, level, above_pos, true);
    for face in HORIZONTAL {
        let name = face_name(face);
        if topper.property(name) != Some("none") && !level.region_random(seed, legacy).next_bool() {
            topper = topper.with_property(name, "none").unwrap_or(topper);
        }
    }
    let has_faces = HORIZONTAL
        .iter()
        .any(|face| topper.property(face_name(*face)) != Some("none"));
    if has_faces && topper != above {
        topper
    } else {
        blocks::AIR
    }
}

#[cfg(test)]
mod tests {
    use clustine_world::Ticked;
    use clustine_worldgen_data::feature::Data;

    use super::*;
    use crate::testing::World;

    fn feature(data: &'static Data) -> Box<dyn Feature> {
        crate::feature::read_configured(&Reader::new(data, "test")).unwrap()
    }

    static POPPY: Data = Data::Object(&[
        ("type", Data::Text("minecraft:simple_block")),
        ("to_place", Data::Text("minecraft:poppy")),
    ]);

    #[test]
    fn a_flower_is_placed_only_where_it_can_stay() {
        let poppy = feature(&POPPY);
        let mut world = World::flat(blocks::GRASS_BLOCK, 64);
        assert!(world.with_context(0, |context| poppy.place(context, BlockPos::new(1, 65, 1))));
        assert_eq!(world.get(BlockPos::new(1, 65, 1)), blocks::POPPY);
        assert!(!world.with_context(0, |context| poppy.place(context, BlockPos::new(1, 70, 1))));
        assert_eq!(world.get(BlockPos::new(1, 70, 1)), blocks::AIR);
    }

    #[test]
    fn a_feature_outside_the_three_by_three_places_nothing() {
        static NAMED: Data = Data::Object(&[
            ("feature", Data::Text("minecraft:flower_default")),
            ("placement", Data::List(&[])),
        ]);
        let placed = crate::feature::PlacedRef::read(&Reader::new(&NAMED, "test")).unwrap();
        let mut world = World::flat(blocks::GRASS_BLOCK, 64);
        assert!(!world.with_context(0, |context| placed.place(context, BlockPos::new(40, 65, 0))));
        assert!(world.writes.is_empty());
    }

    #[test]
    fn a_tall_plant_places_both_halves() {
        static LILAC: Data = Data::Object(&[
            ("type", Data::Text("minecraft:simple_block")),
            (
                "to_place",
                Data::Object(&[
                    ("id", Data::Text("minecraft:lilac")),
                    ("properties", Data::Object(&[("half", Data::Text("lower"))])),
                ]),
            ),
        ]);
        let lilac = feature(&LILAC);
        let mut world = World::flat(blocks::GRASS_BLOCK, 64);
        assert!(world.with_context(0, |context| lilac.place(context, BlockPos::new(3, 65, 3))));
        assert_eq!(
            world.get(BlockPos::new(3, 65, 3)).property("half"),
            Some("lower")
        );
        assert_eq!(
            world.get(BlockPos::new(3, 66, 3)).property("half"),
            Some("upper")
        );
        // Not under a block it cannot replace.
        world.set(BlockPos::new(5, 66, 5), blocks::STONE);
        assert!(!world.with_context(0, |context| lilac.place(context, BlockPos::new(5, 65, 5))));
    }

    #[test]
    fn tall_seagrass_grows_up_into_water() {
        static SEAGRASS: Data = Data::Object(&[
            ("type", Data::Text("minecraft:simple_block")),
            (
                "to_place",
                Data::Object(&[
                    ("id", Data::Text("minecraft:tall_seagrass")),
                    ("properties", Data::Object(&[("half", Data::Text("lower"))])),
                ]),
            ),
        ]);
        let seagrass = feature(&SEAGRASS);
        let mut world = World::flat(blocks::WATER, 60);
        for x in 0..16 {
            for z in 0..16 {
                world.set(BlockPos::new(x, 50, z), blocks::SAND);
            }
        }
        assert!(world.with_context(0, |context| {
            seagrass.place(context, BlockPos::new(2, 51, 2))
        }));
        let upper = world.get(BlockPos::new(2, 52, 2));
        assert!(same_block(upper, blocks::TALL_SEAGRASS));
        assert_eq!(upper.property("half"), Some("upper"));
    }

    #[test]
    fn a_block_with_a_tick_is_given_one_of_its_own_block() {
        static MAGMA: Data = Data::Object(&[
            ("type", Data::Text("minecraft:simple_block")),
            ("schedule_tick", Data::Bool(true)),
            ("to_place", Data::Text("minecraft:magma_block")),
        ]);
        let magma = feature(&MAGMA);
        let mut world = World::flat(blocks::STONE, 10);
        let at = BlockPos::new(4, 11, 4);
        assert!(world.with_context(0, |context| magma.place(context, at)));
        let (pos, tick) = world.ticks[0];
        assert_eq!(pos, at);
        assert_eq!(tick.of, Ticked::Block(block_id(blocks::MAGMA_BLOCK) as u16));
        // A chunk being generated keeps no delay (`ProtoChunkTicks.schedule`).
        assert_eq!(tick.delay, 0);
    }

    #[test]
    fn a_pale_moss_carpet_by_a_wall_grows_a_side_on_top_by_the_region_s_draws() {
        static CARPET: Data = Data::Object(&[
            ("type", Data::Text("minecraft:simple_block")),
            ("to_place", Data::Text("minecraft:pale_moss_carpet")),
        ]);
        let carpet = feature(&CARPET);
        let mut world = World::flat(blocks::GRASS_BLOCK, 64);
        // A wall two high to the north.
        world.set(BlockPos::new(6, 65, 5), blocks::STONE);
        world.set(BlockPos::new(6, 66, 5), blocks::STONE);
        let at = BlockPos::new(6, 65, 6);
        assert!(world.with_context(0, |context| carpet.place(context, at)));
        let placed = world.get(at);
        assert_eq!(placed.property("bottom"), Some("true"));
        let above = world.get(BlockPos::new(6, 66, 6));
        if same_block(above, blocks::PALE_MOSS_CARPET) {
            assert_eq!(above.property("north"), Some("low"));
            assert_eq!(placed.property("north"), Some("tall"));
        } else {
            assert_eq!(above, blocks::AIR);
            assert_eq!(placed.property("north"), Some("low"));
        }
        assert_eq!(placed.property("south"), Some("none"));
    }
}
