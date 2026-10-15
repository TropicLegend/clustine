// Adapted from SteelMC's `steel-core/src/worldgen/feature/features/vines.rs`.

//! `vines`: a vine in an empty block, on the first side, in the game's order of
//! directions and leaving out down, that it can hold on to (`VinesFeature`).

use clustine_data::{Face, blocks};
use clustine_world::BlockPos;

use crate::blocks::{face_name, relative};
use crate::context::Context;
use crate::feature::Feature;
use crate::survive::multiface_can_attach;

/// `vines` has no settings.
#[derive(Debug)]
pub struct Vines;

impl Feature for Vines {
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        let level = &mut *context.level;
        if !level.block(origin).props().is_air() {
            return false;
        }
        for face in Face::ALL {
            if face == Face::Down {
                continue;
            }
            // `VineBlock.isAcceptableNeighbour`: the side of the block beyond that
            // faces the vine is full.
            if multiface_can_attach(level.block(relative(origin, face)), face) {
                let vine = blocks::VINE
                    .with_property(face_name(face), "true")
                    .expect("a vine has a side for every direction but down");
                level.set_block(origin, vine);
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use clustine_worldgen_data::feature::Data;

    use super::*;
    use crate::reader::Reader;
    use crate::testing::World;

    static VINES: Data = Data::Object(&[("type", Data::Text("minecraft:vines"))]);

    #[test]
    fn a_vine_holds_on_to_the_first_side_in_the_game_s_order() {
        let vines = crate::feature::read_configured(&Reader::new(&VINES, "test")).unwrap();
        let mut world = World::flat(blocks::STONE, 60);
        let at = BlockPos::new(4, 70, 4);
        // Nothing to hold on to.
        assert!(!world.with_context(0, |context| vines.place(context, at)));
        // South and west: south comes first.
        world.set(BlockPos::new(3, 70, 4), blocks::STONE);
        world.set(BlockPos::new(4, 70, 5), blocks::STONE);
        assert!(world.with_context(0, |context| vines.place(context, at)));
        let vine = world.get(at);
        assert_eq!(vine.property("south"), Some("true"));
        assert_eq!(vine.property("west"), Some("false"));
        // Not again into a block that is not air.
        assert!(!world.with_context(0, |context| vines.place(context, at)));
        // Up comes before every side.
        let under = BlockPos::new(8, 69, 8);
        world.set(BlockPos::new(8, 70, 8), blocks::STONE);
        world.set(BlockPos::new(9, 69, 8), blocks::STONE);
        assert!(world.with_context(0, |context| vines.place(context, under)));
        assert_eq!(world.get(under).property("up"), Some("true"));
        assert_eq!(world.get(under).property("east"), Some("false"));
    }
}
