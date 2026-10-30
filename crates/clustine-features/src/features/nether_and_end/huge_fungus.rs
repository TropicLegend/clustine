//! `huge_fungus`: a crimson or warped fungus grown huge, a stem under a hat of wart
//! with shroomlights in it, and weeping vines under the crimson's (`HugeFungusFeature`,
//! read from `d-javap/net.minecraft.world.level.levelgen.feature.HugeFungusFeature.txt`;
//! the offsets below are its bytecode's).

use clustine_data::{BlockState, blocks};
use clustine_noise::Random;
use clustine_world::BlockPos;

use crate::blocks::{read_state, same_block};
use crate::context::Context;
use crate::feature::Feature;
use crate::predicates::BlockPredicate;
use crate::reader::{Read, Reader};
use crate::values::next_int_between;

/// The chance that a fungus not planted by a player is grown with a stem three wide
/// (`HUGE_PROBABILITY`).
const HUGE_PROBABILITY: f32 = 0.06;

#[derive(Debug)]
pub struct HugeFungus {
    valid_base: BlockState,
    stem: BlockState,
    hat: BlockState,
    decor: BlockState,
    replaceable: BlockPredicate,
    /// Grown from a fungus with bone meal: it may replace what the predicate allows
    /// in its stem and clears what it grows into, and is never huge.
    planted: bool,
}

impl HugeFungus {
    pub fn read(reader: &Reader) -> Read<Self> {
        let fungus = Self {
            valid_base: read_state(&reader.field("valid_base_block")?)?,
            stem: read_state(&reader.field("stem_state")?)?,
            hat: read_state(&reader.field("hat_state")?)?,
            decor: read_state(&reader.field("decor_state")?)?,
            replaceable: BlockPredicate::read(&reader.field("replaceable_blocks")?)?,
            // `optionalFieldOf("planted", false)` (the codec's lambda, offsets 98-110).
            planted: match reader.optional("planted") {
                Some(planted) => planted.boolean()?,
                None => false,
            },
        };
        reader.finish()?;
        Ok(fungus)
    }

    /// `isReplaceable`: a block placing may overwrite (`canBeReplaced`), or, where
    /// the predicate is asked, one it allows.
    fn is_replaceable(&self, context: &mut Context<'_, '_>, pos: BlockPos, ask: bool) -> bool {
        if context.level.block(pos).props().can_be_replaced() {
            return true;
        }
        ask && self.replaceable.test(context, pos)
    }

    /// What a planted fungus does where it grows over something: the region's
    /// `destroyBlock`, which in a region of world generation sets air and drops
    /// nothing (`WorldGenRegion.destroyBlock`, dumped in `n7-javap`).
    fn clear_over_ground(&self, context: &mut Context<'_, '_>, pos: BlockPos) {
        if self.planted && !context.level.block(pos.offset(0, -1, 0)).props().is_air() {
            let state = context.level.block(pos);
            if !state.props().is_air() {
                context.level.set_block(pos, blocks::AIR);
            }
        }
    }

    /// `placeStem` (offsets 0-221): a column, or three by three without some of its
    /// corners where the fungus is huge.
    fn place_stem(&self, context: &mut Context<'_, '_>, origin: BlockPos, height: i32, huge: bool) {
        let radius = i32::from(huge);
        for x in -radius..=radius {
            for z in -radius..=radius {
                let corner = huge && x.abs() == radius && z.abs() == radius;
                for y in 0..height {
                    let pos = origin.offset(x, y, z);
                    if !self.is_replaceable(context, pos, true) {
                        continue;
                    }
                    if self.planted {
                        self.clear_over_ground(context, pos);
                        context.level.set_block(pos, self.stem);
                    } else if corner {
                        // A corner's float is drawn only where it could stand.
                        if context.random.next_f32() < 0.1 {
                            context.level.set_block(pos, self.stem);
                        }
                    } else {
                        context.level.set_block(pos, self.stem);
                    }
                }
            }
        }
    }

    /// `placeHat` (offsets 0-418): layers from the hat's start up to the stem's top,
    /// each a square whose radius is drawn for it.
    fn place_hat(&self, context: &mut Context<'_, '_>, origin: BlockPos, height: i32, huge: bool) {
        let nether_wart = same_block(self.hat, blocks::NETHER_WART_BLOCK);
        let hat_height = (context.random.next_i32_bounded(1 + height / 3) + 5).min(height);
        let hat_start = height - hat_height;
        for y in hat_start..=height {
            // Drawn for every layer, whatever the layer's radius becomes.
            let mut radius = if y < height - context.random.next_i32_bounded(3) {
                2
            } else {
                1
            };
            if hat_height > 8 && y < hat_start + 4 {
                radius = 3;
            }
            if huge {
                radius += 1;
            }
            for x in -radius..=radius {
                for z in -radius..=radius {
                    let x_edge = x == -radius || x == radius;
                    let z_edge = z == -radius || z == radius;
                    let inside = !x_edge && !z_edge && y != height;
                    let corner = x_edge && z_edge;
                    let low = y < hat_start + 3;
                    let pos = origin.offset(x, y, z);
                    if !self.is_replaceable(context, pos, false) {
                        continue;
                    }
                    self.clear_over_ground(context, pos);
                    if low {
                        if !inside {
                            self.place_hat_drop_block(context, pos, nether_wart);
                        }
                    } else if inside {
                        let vines = if nether_wart { 0.1 } else { 0.0 };
                        self.place_hat_block(context, pos, 0.1, 0.2, vines);
                    } else if corner {
                        let vines = if nether_wart { 0.083 } else { 0.0 };
                        self.place_hat_block(context, pos, 0.01, 0.7, vines);
                    } else {
                        let vines = if nether_wart { 0.07 } else { 0.0 };
                        self.place_hat_block(context, pos, 5.0e-4, 0.98, vines);
                    }
                }
            }
        }
    }

    /// `placeHatBlock`: a shroomlight, else maybe wart; the vines' float is drawn
    /// wherever wart is placed, even where vines have no chance.
    fn place_hat_block(
        &self,
        context: &mut Context<'_, '_>,
        pos: BlockPos,
        decor_chance: f32,
        hat_chance: f32,
        vine_chance: f32,
    ) {
        if context.random.next_f32() < decor_chance {
            context.level.set_block(pos, self.decor);
        } else if context.random.next_f32() < hat_chance {
            context.level.set_block(pos, self.hat);
            if context.random.next_f32() < vine_chance {
                try_place_weeping_vines(context, pos);
            }
        }
    }

    /// `placeHatDropBlock`: the hat's lowest layers, wart under wart, else wart by
    /// chance, with vines under the crimson's by a chance of one in eleven.
    fn place_hat_drop_block(
        &self,
        context: &mut Context<'_, '_>,
        pos: BlockPos,
        nether_wart: bool,
    ) {
        if same_block(context.level.block(pos.offset(0, -1, 0)), self.hat) {
            context.level.set_block(pos, self.hat);
        } else if f64::from(context.random.next_f32()) < 0.15 {
            context.level.set_block(pos, self.hat);
            if nether_wart && context.random.next_i32_bounded(11) == 0 {
                try_place_weeping_vines(context, pos);
            }
        }
    }
}

/// `tryPlaceWeepingVines`: under `pos`, where it is air, vines one to five long, and
/// twice that by a chance of one in seven.
fn try_place_weeping_vines(context: &mut Context<'_, '_>, pos: BlockPos) {
    let below = pos.offset(0, -1, 0);
    if !context.level.block(below).props().is_air() {
        return;
    }
    let mut length = next_int_between(context.random, 1, 5);
    if context.random.next_i32_bounded(7) == 0 {
        length *= 2;
    }
    place_weeping_vines_column(context, below, length, 23, 25);
}

/// `placeWeepingVinesColumn`: down from `pos` through `length` and one more blocks,
/// the plant in air until the last or until the next is not air, where the tip goes
/// with an age drawn from `min_age` to `max_age` and the column ends. A block that is
/// not air is passed over.
fn place_weeping_vines_column(
    context: &mut Context<'_, '_>,
    pos: BlockPos,
    length: i32,
    min_age: i32,
    max_age: i32,
) {
    let mut at = pos;
    for step in 0..=length {
        if context.level.block(at).props().is_air() {
            if step == length || !context.level.block(at.offset(0, -1, 0)).props().is_air() {
                let age = next_int_between(context.random, min_age, max_age);
                let tip = blocks::WEEPING_VINES
                    .with_property("age", &age.to_string())
                    .expect("weeping vines have an age up to 25");
                context.level.set_block(at, tip);
                return;
            }
            context.level.set_block(at, blocks::WEEPING_VINES_PLANT);
        }
        at = at.offset(0, -1, 0);
    }
}

impl Feature for HugeFungus {
    /// `place` (offsets 0-171).
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        // On its nylium, or nothing (0-45).
        if !same_block(
            context.level.block(origin.offset(0, -1, 0)),
            self.valid_base,
        ) {
            return false;
        }
        // From 4 to 13 high, twice that by a chance of one in twelve (46-72).
        let mut height = next_int_between(context.random, 4, 13);
        if context.random.next_i32_bounded(12) == 0 {
            height *= 2;
        }
        // A fungus of generation stays below the generator's top: 128 in the Nether
        // (`getGenDepth`, 72-101).
        if !self.planted {
            let depth = context.settings.generation().height;
            if origin.y + height + 1 >= depth {
                return false;
            }
        }
        // Huge only where not planted, a float drawn only then (102-126).
        let huge = !self.planted && context.random.next_f32() < HUGE_PROBABILITY;
        // The fungus itself, if there is one, gives way (128-145).
        context.level.set_block(origin, blocks::AIR);
        self.place_stem(context, origin, height, huge);
        self.place_hat(context, origin, height, huge);
        true
    }
}

#[cfg(test)]
mod tests {
    use clustine_worldgen_data::feature as data;

    use super::*;
    use crate::testing::World;

    fn fungus(name: &str) -> Box<dyn Feature> {
        let (_, entry) = data::configured(name).unwrap();
        crate::feature::read_configured(&Reader::new(&entry.data, entry.name)).unwrap()
    }

    fn on_nylium(nylium: BlockState) -> World {
        let mut world = World::flat(nylium, 40);
        world.writes.clear();
        world
    }

    /// The draws of `place` before the stem by hand: the height and whether huge.
    fn drawn(seed: u64) -> (i32, bool) {
        let mut random = clustine_noise::WorldgenRandom::from_seed(seed);
        let mut height = random.next_i32_bounded(10) + 4;
        if random.next_i32_bounded(12) == 0 {
            height *= 2;
        }
        (height, random.next_f32() < HUGE_PROBABILITY)
    }

    #[test]
    fn a_fungus_stands_only_on_its_own_nylium() {
        let crimson = fungus("minecraft:crimson_fungus");
        let mut world = on_nylium(blocks::WARPED_NYLIUM);
        assert!(!world.with_context(1, |context| crimson.place(context, BlockPos::new(8, 41, 8))));
        assert!(world.writes.is_empty());
    }

    #[test]
    fn a_crimson_fungus_has_a_stem_of_its_height_under_a_hat_of_wart() {
        let crimson = fungus("minecraft:crimson_fungus");
        let seed = (0..200).find(|seed| !drawn(*seed).1).unwrap();
        let (height, _) = drawn(seed);
        let mut world = on_nylium(blocks::CRIMSON_NYLIUM);
        let origin = BlockPos::new(8, 41, 8);
        assert!(world.with_context(seed, |context| crimson.place(context, origin)));
        // The origin is cleared first, then the stem rises from it.
        assert_eq!(world.writes[0], (origin, blocks::AIR));
        for y in 0..height {
            assert_eq!(
                world.get(origin.offset(0, y, 0)),
                blocks::CRIMSON_STEM,
                "{y}"
            );
        }
        let allowed = [
            blocks::AIR,
            blocks::CRIMSON_STEM,
            blocks::NETHER_WART_BLOCK,
            blocks::SHROOMLIGHT,
            blocks::WEEPING_VINES_PLANT,
        ];
        for (pos, state) in &world.writes {
            assert!(
                allowed.contains(state) || same_block(*state, blocks::WEEPING_VINES),
                "{pos:?} {state:?}"
            );
            // The hat is at most two wide around the stem, three in its low layers.
            assert!((pos.x - 8).abs() <= 3 && (pos.z - 8).abs() <= 3, "{pos:?}");
            assert!(pos.y <= origin.y + height, "{pos:?}");
        }
        assert!(
            world
                .writes
                .iter()
                .any(|(_, state)| *state == blocks::NETHER_WART_BLOCK)
        );
    }

    #[test]
    fn a_huge_warped_fungus_has_a_stem_three_wide_and_no_vines() {
        let warped = fungus("minecraft:warped_fungus");
        let seed = (0..5000).find(|seed| drawn(*seed).1).unwrap();
        let (height, _) = drawn(seed);
        let mut world = on_nylium(blocks::WARPED_NYLIUM);
        let origin = BlockPos::new(8, 41, 8);
        assert!(world.with_context(seed, |context| warped.place(context, origin)));
        // Every side of the stem's middle is stem; the corners only by chance.
        for (x, z) in [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)] {
            for y in 0..height {
                assert_eq!(
                    world.get(origin.offset(x, y, z)),
                    blocks::WARPED_STEM,
                    "{x} {y} {z}"
                );
            }
        }
        assert!(world.writes.iter().all(|(_, state)| {
            !same_block(*state, blocks::WEEPING_VINES) && *state != blocks::WEEPING_VINES_PLANT
        }));
        assert!(
            world
                .writes
                .iter()
                .any(|(_, state)| *state == blocks::WARPED_WART_BLOCK)
        );
    }

    #[test]
    fn a_fungus_of_generation_does_not_reach_the_generator_s_top() {
        use super::super::shared::with_nether_context;

        let crimson = fungus("minecraft:crimson_fungus");
        let (height, _) = drawn(4);
        // The Nether's generator makes 128 blocks of its 256.
        let floor = 128 - height - 2;
        let mut world = World::flat(blocks::CRIMSON_NYLIUM, floor);
        let origin = BlockPos::new(8, floor + 1, 8);
        assert!(!with_nether_context(&mut world, 4, |context| crimson.place(context, origin)));
        assert!(world.writes.is_empty());
        let mut world = World::flat(blocks::CRIMSON_NYLIUM, floor - 1);
        let origin = BlockPos::new(8, floor, 8);
        assert!(with_nether_context(&mut world, 4, |context| crimson.place(context, origin)));
    }

    #[test]
    fn weeping_vines_hang_from_air_to_a_tip_aged_23_to_25() {
        let mut world = on_nylium(blocks::NETHERRACK);
        // A roof at 60 with air under it down to the ground.
        let roof = BlockPos::new(8, 60, 8);
        world.set(roof, blocks::NETHER_WART_BLOCK);
        world.writes.clear();
        world.with_context(7, |context| try_place_weeping_vines(context, roof));
        let mut random = clustine_noise::WorldgenRandom::from_seed(7);
        let mut length = random.next_i32_bounded(5) + 1;
        if random.next_i32_bounded(7) == 0 {
            length *= 2;
        }
        let age = random.next_i32_bounded(3) + 23;
        assert_eq!(world.writes.len(), length as usize + 1);
        for (index, (pos, state)) in world.writes.iter().enumerate() {
            assert_eq!(*pos, roof.offset(0, -1 - index as i32, 0));
            if index == length as usize {
                assert_eq!(state.property("age"), Some(age.to_string().as_str()));
                assert!(same_block(*state, blocks::WEEPING_VINES));
            } else {
                assert_eq!(*state, blocks::WEEPING_VINES_PLANT);
            }
        }
    }

    #[test]
    fn weeping_vines_stop_with_their_tip_above_ground() {
        let mut world = on_nylium(blocks::NETHERRACK);
        // Three blocks of air under the roof: the tip is at the third whatever the
        // length drawn.
        let roof = BlockPos::new(8, 44, 8);
        world.set(roof, blocks::NETHER_WART_BLOCK);
        world.writes.clear();
        world.with_context(3, |context| {
            place_weeping_vines_column(context, roof.offset(0, -1, 0), 10, 23, 25)
        });
        assert_eq!(world.writes.len(), 3);
        assert_eq!(world.writes[0].1, blocks::WEEPING_VINES_PLANT);
        assert_eq!(world.writes[1].1, blocks::WEEPING_VINES_PLANT);
        assert_eq!(world.writes[2].0, BlockPos::new(8, 41, 8));
        assert!(same_block(world.writes[2].1, blocks::WEEPING_VINES));
    }
}
