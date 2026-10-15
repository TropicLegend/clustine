// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/multiface_growth.rs`.

//! `multiface_growth`: glow lichen or a sculk vein on a side of the origin, or of the
//! block next to it in one of the directions, that lies against a block of the list,
//! and by a chance spread once from there (`MultifaceGrowthFeature`).

use clustine_data::{BlockState, Face, blocks};
use clustine_noise::Random;
use clustine_world::BlockPos;

use super::multiface::Spreader;
use super::shuffle;
use crate::blocks::{BlockSet, HORIZONTAL, read_state, relative, same_block};
use crate::context::Context;
use crate::feature::Feature;
use crate::reader::{Read, Reader};

#[derive(Debug)]
pub struct MultifaceGrowth {
    /// The default state of the block placed.
    block: BlockState,
    search_range: i32,
    chance_of_spreading: f32,
    can_be_placed_on: BlockSet,
    /// The sides it may cover, in the order the game lists them: up, down, then the
    /// four walls.
    valid_directions: Vec<Face>,
}

impl MultifaceGrowth {
    pub fn read(reader: &Reader) -> Read<Self> {
        let flag = |key: &str| -> Read<bool> {
            match reader.optional(key) {
                Some(flag) => flag.boolean(),
                None => Ok(false),
            }
        };
        let block = read_state(&reader.field("block")?)?;
        if block.property("waterlogged").is_none() || block.property("north").is_none() {
            return Err(reader.error("the block of a multiface growth is not a multiface block"));
        }
        let mut valid_directions = Vec::new();
        if flag("can_place_on_ceiling")? {
            valid_directions.push(Face::Up);
        }
        if flag("can_place_on_floor")? {
            valid_directions.push(Face::Down);
        }
        if flag("can_place_on_wall")? {
            valid_directions.extend(HORIZONTAL);
        }
        Ok(Self {
            block,
            search_range: match reader.optional("search_range") {
                Some(range) => range.int()?,
                None => 10,
            },
            chance_of_spreading: match reader.optional("chance_of_spreading") {
                Some(chance) => chance.float()?,
                None => 0.5,
            },
            can_be_placed_on: BlockSet::read(&reader.field("can_be_placed_on")?)?,
            valid_directions,
        })
    }

    /// `getShuffledDirectionsExcept` (or all, where `except` is none).
    fn shuffled_directions<R: Random + ?Sized>(
        &self,
        random: &mut R,
        except: Option<Face>,
    ) -> Vec<Face> {
        let mut directions: Vec<Face> = self
            .valid_directions
            .iter()
            .copied()
            .filter(|face| Some(*face) != except)
            .collect();
        shuffle(random, &mut directions);
        directions
    }

    /// `placeGrowthIfPossible`: on the first side, in `directions`' order, that lies
    /// against a block of the list, if the block can go there; then spreading by a
    /// chance.
    fn place_if_possible(
        &self,
        context: &mut Context<'_, '_>,
        pos: BlockPos,
        old: BlockState,
        directions: &[Face],
    ) -> bool {
        let spreader = Spreader::of(self.block);
        for face in directions {
            let neighbour = context.level.block(relative(pos, *face));
            if !self.can_be_placed_on.contains(neighbour) {
                continue;
            }
            let Some(state) = spreader.state_for_placement(context.level, old, pos, *face) else {
                return false;
            };
            context.level.set_block(pos, state);
            context.level.mark_for_post_processing(pos);
            if context.random.next_f32() < self.chance_of_spreading {
                spreader.spread_from_face_toward_random_direction(
                    context.level,
                    &mut *context.random,
                    state,
                    pos,
                    *face,
                    true,
                );
            }
            return true;
        }
        false
    }
}

/// `isAirOrWater`.
fn air_or_water(state: BlockState) -> bool {
    state.props().is_air() || same_block(state, blocks::WATER)
}

impl Feature for MultifaceGrowth {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        if !air_or_water(context.level.block(origin)) {
            return false;
        }
        let search = self.shuffled_directions(&mut *context.random, None);
        let here = context.level.block(origin);
        if self.place_if_possible(context, origin, here, &search) {
            return true;
        }
        for direction in &search {
            let placement =
                self.shuffled_directions(&mut *context.random, Some(direction.opposite()));
            for _ in 0..self.search_range {
                // The game moves from the origin each time, not from where it was
                // (`setWithOffset(origin, …)`), so every step tests the one block
                // next to the origin.
                let pos = relative(origin, *direction);
                let state = context.level.block(pos);
                if !air_or_water(state) && !same_block(state, self.block) {
                    break;
                }
                if self.place_if_possible(context, pos, state, &placement) {
                    return true;
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use clustine_worldgen_data::feature::Data;

    use super::*;
    use crate::features::plants::multiface::has_face;
    use crate::testing::World;

    static LICHEN: Data = Data::Object(&[
        ("type", Data::Text("minecraft:multiface_growth")),
        ("block", Data::Text("minecraft:glow_lichen")),
        (
            "can_be_placed_on",
            Data::List(&[Data::Text("minecraft:stone")]),
        ),
        ("can_place_on_ceiling", Data::Bool(true)),
        ("can_place_on_wall", Data::Bool(true)),
        ("chance_of_spreading", Data::Number("0.0")),
        ("search_range", Data::Number("20")),
    ]);

    #[test]
    fn lichen_grows_on_a_ceiling_of_stone_and_never_on_the_floor_it_may_not() {
        let lichen = crate::feature::read_configured(&Reader::new(&LICHEN, "test")).unwrap();
        let mut world = World::flat(blocks::STONE, 60);
        let at = BlockPos::new(4, 61, 4);
        // Only a floor: lichen may not go there, nor on the air beside it.
        assert!(!world.with_context(5, |context| lichen.place(context, at)));
        world.set(BlockPos::new(4, 62, 4), blocks::STONE);
        assert!(world.with_context(5, |context| lichen.place(context, at)));
        let placed = world.get(at);
        assert!(same_block(placed, blocks::GLOW_LICHEN));
        assert!(has_face(placed, Face::Up));
        assert!(!has_face(placed, Face::Down));
        assert_eq!(world.marks, vec![at]);
    }

    #[test]
    fn lichen_searches_the_block_beside_the_origin_only() {
        let lichen = crate::feature::read_configured(&Reader::new(&LICHEN, "test")).unwrap();
        let mut world = World::flat(blocks::STONE, 0);
        // A ceiling over the block next to the origin, in every direction: the
        // lichen goes into one of them.
        let origin = BlockPos::new(8, 40, 8);
        for face in [Face::North, Face::South, Face::East, Face::West] {
            let next = relative(origin, face);
            world.set(relative(next, Face::Up), blocks::STONE);
        }
        assert!(world.with_context(2, |context| lichen.place(context, origin)));
        let placed: Vec<_> = world.writes.iter().map(|(pos, _)| *pos).collect();
        assert_eq!(placed.len(), 1);
        assert_eq!(
            (placed[0].x - 8).abs() + (placed[0].y - 40).abs() + (placed[0].z - 8).abs(),
            1
        );
    }

    #[test]
    fn a_feature_whose_block_is_not_multiface_is_refused() {
        static STONE: Data = Data::Object(&[
            ("type", Data::Text("minecraft:multiface_growth")),
            ("block", Data::Text("minecraft:stone")),
            ("can_be_placed_on", Data::List(&[])),
        ]);
        assert!(crate::feature::read_configured(&Reader::new(&STONE, "test")).is_err());
    }
}
