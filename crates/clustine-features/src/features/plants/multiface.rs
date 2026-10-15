// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/multiface_growth.rs`
// and the vein spreading of its `sculk_patch.rs`.

//! The game's `MultifaceBlock` and `MultifaceSpreader` as features use them: a block
//! that covers some of its six sides (glow lichen, sculk veins), and how it spreads
//! from a side it has to a neighbouring side, around a corner or across a block.
//!
//! Two kinds of spreading are ported: the default (`DefaultSpreaderConfig`, glow
//! lichen's) and the sculk vein's (`SculkVeinSpreaderConfig`), which also spreads from
//! blocks that are not veins, replaces more and stays off sculk.

use clustine_data::{BlockState, Face, SupportType, blocks};
use clustine_world::BlockPos;

use crate::blocks::{face_name, fluid_id, in_block_tag, relative, same_block};
use crate::context::Level;
use crate::survive::{multiface_can_attach, sturdy};

/// The id of still water among the fluids (`Fluids.WATER`, which a flowing water is
/// not).
pub fn still_water(state: BlockState) -> bool {
    let water = clustine_data::FLUIDS
        .iter()
        .position(|fluid| *fluid == "minecraft:water")
        .expect("the game has water");
    usize::from(fluid_id(state)) == water
}

/// Whether `state` covers its side `face` (`MultifaceBlock.hasFace`); false for a
/// block that covers no sides.
pub fn has_face(state: BlockState, face: Face) -> bool {
    state.property(face_name(face)) == Some("true")
}

/// Whether `state` covers any side.
pub fn has_any_face(state: BlockState) -> bool {
    Face::ALL.into_iter().any(|face| has_face(state, face))
}

/// `state` with its side `face` covered or not.
pub fn with_face(state: BlockState, face: Face, covered: bool) -> BlockState {
    state
        .with_property(face_name(face), if covered { "true" } else { "false" })
        .expect("a multiface block has a property for each side")
}

/// The sides `state` covers, in the game's order (`MultifaceBlock.availableFaces`).
pub fn available_faces(state: BlockState) -> Vec<Face> {
    Face::ALL
        .into_iter()
        .filter(|face| has_face(state, *face))
        .collect()
}

/// `MultifaceBlock.canAttachTo` of the neighbour of `pos` towards `face`.
pub fn can_attach(level: &mut Level<'_>, pos: BlockPos, face: Face) -> bool {
    multiface_can_attach(level.block(relative(pos, face)), face)
}

/// Where spreading goes from a side, in the order the game tries them
/// (`MultifaceSpreader.SpreadType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpreadType {
    /// To another side of the same block.
    SamePosition,
    /// To the same side of the next block.
    SamePlane,
    /// Around the edge of the block the side lies on.
    WrapAround,
}

impl SpreadType {
    /// `getSpreadPos`: the block and the side spreading from `from_face` towards
    /// `direction` reaches.
    fn spread_pos(self, pos: BlockPos, direction: Face, from_face: Face) -> (BlockPos, Face) {
        match self {
            SpreadType::SamePosition => (pos, direction),
            SpreadType::SamePlane => (relative(pos, direction), from_face),
            SpreadType::WrapAround => (
                relative(relative(pos, direction), from_face),
                direction.opposite(),
            ),
        }
    }
}

/// The game's `DEFAULT_SPREAD_ORDER`.
pub const DEFAULT_SPREAD_ORDER: [SpreadType; 3] = [
    SpreadType::SamePosition,
    SpreadType::SamePlane,
    SpreadType::WrapAround,
];

/// A block's spreader (`MultifaceSpreader` with its config).
#[derive(Debug, Clone, Copy)]
pub struct Spreader {
    /// The default state of the block that spreads.
    pub block: BlockState,
    /// Whether this is a sculk vein's (`SculkVeinSpreaderConfig`).
    pub sculk: bool,
    pub spread_types: &'static [SpreadType],
}

impl Spreader {
    /// The spreader of `block` as `getSpreader` gives it: a sculk vein's, or the
    /// default.
    pub fn of(block: BlockState) -> Self {
        Self {
            block,
            sculk: same_block(block, blocks::SCULK_VEIN),
            spread_types: &DEFAULT_SPREAD_ORDER,
        }
    }

    /// The sculk vein's spreader that stays in its own block (`getSameSpaceSpreader`).
    pub fn sculk_same_space() -> Self {
        Self {
            block: blocks::SCULK_VEIN,
            sculk: true,
            spread_types: &[SpreadType::SamePosition],
        }
    }

    /// `isValidStateForPlacement`: the side is not covered already, and its
    /// neighbour holds it.
    pub fn is_valid_state_for_placement(
        &self,
        level: &mut Level<'_>,
        old: BlockState,
        pos: BlockPos,
        face: Face,
    ) -> bool {
        if same_block(old, self.block) && has_face(old, face) {
            return false;
        }
        can_attach(level, pos, face)
    }

    /// `getStateForPlacement`: the block that `old` becomes with `face` covered, or
    /// none where it may not be. A block of water that is a source makes it
    /// waterlogged.
    pub fn state_for_placement(
        &self,
        level: &mut Level<'_>,
        old: BlockState,
        pos: BlockPos,
        face: Face,
    ) -> Option<BlockState> {
        if !self.is_valid_state_for_placement(level, old, pos, face) {
            return None;
        }
        let state = if same_block(old, self.block) {
            old
        } else if still_water(old) {
            self.block
                .with_property("waterlogged", "true")
                .expect("a multiface block can be waterlogged")
        } else {
            self.block
        };
        Some(with_face(state, face, true))
    }

    /// `isOtherBlockValidAsSource`: a sculk vein spreads from any block that is not a
    /// vein, on every side.
    fn is_other_block_valid_as_source(&self, state: BlockState) -> bool {
        self.sculk && !same_block(state, blocks::SCULK_VEIN)
    }

    /// `canSpreadFrom`.
    fn can_spread_from(&self, state: BlockState, face: Face) -> bool {
        self.is_other_block_valid_as_source(state) || has_face(state, face)
    }

    /// `stateCanBeReplaced`: whether spreading may put the block over `existing`.
    fn state_can_be_replaced(
        &self,
        level: &mut Level<'_>,
        source: BlockPos,
        pos: BlockPos,
        face: Face,
        existing: BlockState,
    ) -> bool {
        let default = |existing: BlockState| {
            existing.props().is_air()
                || same_block(existing, self.block)
                || same_block(existing, blocks::WATER)
                    && existing.props().fluid().is_some_and(|fluid| fluid.source)
        };
        if !self.sculk {
            return default(existing);
        }
        let against = level.block(relative(pos, face));
        if [blocks::SCULK, blocks::SCULK_CATALYST, blocks::MOVING_PISTON]
            .into_iter()
            .any(|block| same_block(against, block))
        {
            return false;
        }
        let distance =
            (source.x - pos.x).abs() + (source.y - pos.y).abs() + (source.z - pos.z).abs();
        if distance == 2 {
            let neighbour = level.block(relative(source, face.opposite()));
            if sturdy(neighbour, face, SupportType::Full) {
                return false;
            }
        }
        if existing.props().fluid().is_some() && !still_water(existing) {
            return false;
        }
        if in_block_tag(existing, "minecraft:fire") {
            return false;
        }
        existing.props().can_be_replaced() || default(existing)
    }

    /// `canSpreadInto`.
    fn can_spread_into(
        &self,
        level: &mut Level<'_>,
        source: BlockPos,
        (pos, face): (BlockPos, Face),
    ) -> bool {
        let existing = level.block(pos);
        self.state_can_be_replaced(level, source, pos, face, existing)
            && self.is_valid_state_for_placement(level, existing, pos, face)
    }

    /// `getSpreadFromFaceTowardDirection`: where spreading from the side
    /// `starting_face` of the block at `pos` towards `direction` can go, if anywhere.
    fn spread_from_face_toward_direction_pos(
        &self,
        level: &mut Level<'_>,
        state: BlockState,
        pos: BlockPos,
        starting_face: Face,
        direction: Face,
    ) -> Option<(BlockPos, Face)> {
        if crate::features::plants::same_axis(direction, starting_face) {
            return None;
        }
        if !self.is_other_block_valid_as_source(state)
            && (!has_face(state, starting_face) || has_face(state, direction))
        {
            return None;
        }
        self.spread_types
            .iter()
            .map(|spread| spread.spread_pos(pos, direction, starting_face))
            .find(|spread| self.can_spread_into(level, pos, *spread))
    }

    /// `spreadFromFaceTowardDirection`: spreads, and says where it went.
    pub fn spread_from_face_toward_direction(
        &self,
        level: &mut Level<'_>,
        state: BlockState,
        pos: BlockPos,
        starting_face: Face,
        direction: Face,
        post_process: bool,
    ) -> Option<(BlockPos, Face)> {
        let spread = self.spread_from_face_toward_direction_pos(
            level,
            state,
            pos,
            starting_face,
            direction,
        )?;
        self.spread_to_face(level, spread, post_process)
            .then_some(spread)
    }

    /// `spreadToFace` (`placeBlock` of the config): covers the side reached, marking
    /// it for post-processing first where asked; false where the write was refused.
    fn spread_to_face(
        &self,
        level: &mut Level<'_>,
        (pos, face): (BlockPos, Face),
        post_process: bool,
    ) -> bool {
        let old = level.block(pos);
        let Some(state) = self.state_for_placement(level, old, pos, face) else {
            return false;
        };
        if post_process {
            level.mark_for_post_processing(pos);
        }
        level.set_block(pos, state)
    }

    /// `spreadFromFaceTowardRandomDirection`: towards the six directions in a shuffled
    /// order, until one spreads.
    pub fn spread_from_face_toward_random_direction<R: clustine_noise::Random + ?Sized>(
        &self,
        level: &mut Level<'_>,
        random: &mut R,
        state: BlockState,
        pos: BlockPos,
        starting_face: Face,
        post_process: bool,
    ) -> Option<(BlockPos, Face)> {
        crate::features::plants::all_shuffled(random)
            .into_iter()
            .find_map(|direction| {
                self.spread_from_face_toward_direction(
                    level,
                    state,
                    pos,
                    starting_face,
                    direction,
                    post_process,
                )
            })
    }

    /// `spreadAll`: from every side it may spread from towards every direction; how
    /// many spread.
    pub fn spread_all(
        &self,
        level: &mut Level<'_>,
        state: BlockState,
        pos: BlockPos,
        post_process: bool,
    ) -> i64 {
        let mut count = 0;
        for starting_face in Face::ALL {
            if !self.can_spread_from(state, starting_face) {
                continue;
            }
            for direction in Face::ALL {
                if self
                    .spread_from_face_toward_direction(
                        level,
                        state,
                        pos,
                        starting_face,
                        direction,
                        post_process,
                    )
                    .is_some()
                {
                    count += 1;
                }
            }
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::World;

    #[test]
    fn lichen_spreads_from_a_wall_around_onto_the_floor_beside_it() {
        // A lichen on the west face of a block of air, with stone to the west and
        // below: spreading down puts the same lichen on its floor.
        let mut world = World::flat(blocks::STONE, 60);
        let pos = BlockPos::new(4, 61, 4);
        world.set(BlockPos::new(3, 61, 4), blocks::STONE);
        let lichen = with_face(blocks::GLOW_LICHEN, Face::West, true);
        world.set(pos, lichen);
        let spreader = Spreader::of(blocks::GLOW_LICHEN);
        let spread = world.with_level(|level| {
            spreader.spread_from_face_toward_direction(
                level,
                lichen,
                pos,
                Face::West,
                Face::Down,
                true,
            )
        });
        assert_eq!(spread, Some((pos, Face::Down)));
        let now = world.get(pos);
        assert!(has_face(now, Face::West) && has_face(now, Face::Down));
        assert_eq!(world.marks, vec![pos]);
        // Not along its own axis.
        let none = world.with_level(|level| {
            spreader.spread_from_face_toward_direction(
                level,
                now,
                pos,
                Face::West,
                Face::East,
                true,
            )
        });
        assert_eq!(none, None);
    }

    #[test]
    fn a_vein_spreads_from_any_block_but_not_onto_sculk() {
        let mut world = World::flat(blocks::STONE, 60);
        let pos = BlockPos::new(4, 61, 4);
        let spreader = Spreader::sculk_same_space();
        // From air on stone: a vein on the floor.
        let count = world.with_level(|level| spreader.spread_all(level, blocks::AIR, pos, false));
        assert!(count > 0);
        assert!(has_face(world.get(pos), Face::Down));
        // Onto sculk it does not go.
        let mut world = World::flat(blocks::SCULK, 60);
        let count = world.with_level(|level| spreader.spread_all(level, blocks::AIR, pos, false));
        assert_eq!(count, 0);
        assert_eq!(world.get(pos), blocks::AIR);
    }

    #[test]
    fn a_vein_in_water_is_waterlogged_and_flowing_water_is_not_replaced() {
        let mut world = World::flat(blocks::STONE, 60);
        let pos = BlockPos::new(4, 61, 4);
        world.set(pos, blocks::WATER);
        let spreader = Spreader::of(blocks::SCULK_VEIN);
        let state = world.with_level(|level| {
            spreader.state_for_placement(level, blocks::WATER, pos, Face::Down)
        });
        assert_eq!(
            state.and_then(|state| state.property("waterlogged")),
            Some("true")
        );
        let flowing = blocks::WATER.with_property("level", "3").unwrap();
        assert!(!still_water(flowing));
        assert!(still_water(blocks::WATER));
    }
}
