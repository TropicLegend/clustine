//! `end_gateway`: a gateway in its cage of bedrock, three by five by three, with its
//! exit written into its block entity where the feature has one (`EndGatewayFeature`,
//! read from `d-javap`; the offsets below are its bytecode's). The highlands place
//! `end_gateway_return`, exact, whose exit is the platform's (100, 50, 0); the fight's
//! gateways (`end_gateway_delayed`) have none and find theirs when first used.

use clustine_data::{BlockState, blocks};
use clustine_protocol::nbt::Nbt;
use clustine_world::{BlockEntity, BlockPos};

use crate::blocks::read_vec;
use crate::context::Context;
use crate::feature::Feature;
use crate::features::ground::shared::{block_entity, compound};
use crate::reader::{Read, Reader};

/// The block the gateway feature puts at `(dx, dy, dz)` from its centre, within one
/// across and two up and down (`place`, offsets 49-281): the gateway in the middle, air
/// round it on its layer, bedrock on the plus above and below it and over and under
/// that, air in the other places.
pub fn gateway_block(dx: i32, dy: i32, dz: i32) -> BlockState {
    let same_x = dx == 0;
    let same_y = dy == 0;
    let same_z = dz == 0;
    let two_off = dy.abs() == 2;
    if same_x && same_y && same_z {
        blocks::END_GATEWAY
    } else if same_y {
        blocks::AIR
    } else if two_off && same_x && same_z {
        blocks::BEDROCK
    } else if (!same_x && !same_z) || two_off {
        blocks::AIR
    } else {
        blocks::BEDROCK
    }
}

/// What a gateway's block entity saves when its exit is set
/// (`TheEndGatewayBlockEntity.saveAdditional`, offsets 0-48, in d-javap's
/// TheEndGatewayBlockEntity.txt): its age, nought for a new one (the constructor,
/// offsets 9-16); its exit; `ExactTeleport` only where it is exact; and, as for every
/// block entity the game saves, its components, which are none.
pub fn gateway_entity(exit: BlockPos, exact: bool) -> BlockEntity {
    let mut members = vec![
        ("Age", Nbt::Long(0)),
        ("exit_portal", Nbt::IntArray(vec![exit.x, exit.y, exit.z])),
    ];
    if exact {
        members.push(("ExactTeleport", Nbt::Byte(1)));
    }
    members.push(("components", compound(Vec::new())));
    block_entity("minecraft:end_gateway", members)
}

#[derive(Debug)]
pub struct EndGateway {
    exit: Option<BlockPos>,
    exact: bool,
}

impl EndGateway {
    /// The codec (`lambda$static$0`, offsets 0-49): `exit` optional, `exact` required.
    pub fn read(reader: &Reader) -> Read<Self> {
        let exit = match reader.optional("exit") {
            Some(exit) => {
                let (x, y, z) = read_vec(&exit)?;
                Some(BlockPos::new(x, y, z))
            }
            None => None,
        };
        let exact = reader.field("exact")?.boolean()?;
        reader.finish()?;
        Ok(Self { exit, exact })
    }
}

impl Feature for EndGateway {
    /// `place`: the blocks of the box in `betweenClosed`'s order (x fastest, then y,
    /// then z), and, as the gateway is set, its exit where the feature has one
    /// (`lambda$place$0`: `setExitPosition` on the block entity the region makes for
    /// it).
    fn place(&self, context: &mut Context<'_, '_>, origin: BlockPos) -> bool {
        for dz in -1..=1 {
            for dy in -2..=2 {
                for dx in -1..=1 {
                    let at = origin.offset(dx, dy, dz);
                    let state = gateway_block(dx, dy, dz);
                    context.level.set_block(at, state);
                    if state == blocks::END_GATEWAY
                        && let Some(exit) = self.exit
                    {
                        context
                            .level
                            .set_block_entity(at, Some(gateway_entity(exit, self.exact)));
                    }
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use clustine_protocol::codec::Reader as Bytes;

    use super::*;
    use crate::features::ground::shared::testing::WithEntities;
    use crate::testing::World;

    #[test]
    fn a_gateway_is_caged_in_bedrock_on_its_axes_with_air_round_it() {
        let count = |wanted: BlockState| {
            let mut found = 0;
            for dx in -1..=1 {
                for dy in -2..=2 {
                    for dz in -1..=1 {
                        if gateway_block(dx, dy, dz) == wanted {
                            found += 1;
                        }
                    }
                }
            }
            found
        };
        assert_eq!(gateway_block(0, 0, 0), blocks::END_GATEWAY);
        // Above and below: the plus at one off, its middle at two off.
        for dy in [-1, 1] {
            assert_eq!(gateway_block(0, dy, 0), blocks::BEDROCK);
            assert_eq!(gateway_block(1, dy, 0), blocks::BEDROCK);
            assert_eq!(gateway_block(0, dy, -1), blocks::BEDROCK);
            assert_eq!(gateway_block(1, dy, 1), blocks::AIR);
        }
        for dy in [-2, 2] {
            assert_eq!(gateway_block(0, dy, 0), blocks::BEDROCK);
            assert_eq!(gateway_block(1, dy, 0), blocks::AIR);
        }
        assert_eq!(gateway_block(1, 0, 1), blocks::AIR);
        assert_eq!(count(blocks::END_GATEWAY), 1);
        assert_eq!(count(blocks::BEDROCK), 5 + 5 + 2);
        assert_eq!(count(blocks::AIR), 45 - 1 - 12);
    }

    #[test]
    fn the_highlands_return_gateway_leads_exactly_to_the_platform() {
        let (_, entry) =
            clustine_worldgen_data::feature::configured("minecraft:end_gateway_return")
                .expect("the data has the return gateway");
        let feature = {
            let reader = Reader::new(&entry.data, entry.name);
            reader.kind().expect("a kind");
            EndGateway::read(&reader).expect("read")
        };
        assert_eq!(feature.exit, Some(BlockPos::new(100, 50, 0)));
        assert!(feature.exact);
        let mut world = WithEntities::new(World::flat(blocks::END_STONE, 60));
        let origin = BlockPos::new(8, 70, 8);
        world.with_context(1, |context| {
            assert!(feature.place(context, origin));
        });
        assert_eq!(world.world.get(origin), blocks::END_GATEWAY);
        assert_eq!(world.world.get(origin.offset(0, 2, 0)), blocks::BEDROCK);
        assert_eq!(world.world.get(origin.offset(1, -1, 0)), blocks::BEDROCK);
        assert_eq!(world.entities.len(), 1);
        let entity = &world.entities[&origin];
        assert_eq!(entity, &gateway_entity(BlockPos::new(100, 50, 0), true));
        let Ok(Some(tag)) = Bytes::new(&entity.data).nbt() else {
            panic!("the entity's data is a compound");
        };
        assert_eq!(tag.get("Age"), Some(&Nbt::Long(0)));
        assert_eq!(
            tag.get("exit_portal"),
            Some(&Nbt::IntArray(vec![100, 50, 0]))
        );
        assert_eq!(tag.get("ExactTeleport"), Some(&Nbt::Byte(1)));
    }

    #[test]
    fn a_gateway_without_an_exit_keeps_the_empty_entity_its_block_has() {
        let (_, entry) =
            clustine_worldgen_data::feature::configured("minecraft:end_gateway_delayed")
                .expect("the data has the fight's gateway");
        let feature = {
            let reader = Reader::new(&entry.data, entry.name);
            reader.kind().expect("a kind");
            EndGateway::read(&reader).expect("read")
        };
        assert_eq!(feature.exit, None);
        assert!(!feature.exact);
        let mut world = WithEntities::new(World::flat(blocks::END_STONE, 60));
        let origin = BlockPos::new(8, 70, 8);
        world.with_context(1, |context| {
            assert!(feature.place(context, origin));
        });
        assert!(world.entities[&origin].data.is_empty());
    }
}
