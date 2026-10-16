// Adapted from SteelMC's `steel-core/src/worldgen/structure/piece_placer/ruined_portal.rs`
// and `template_processors.rs` (see NOTICE in this crate), and checked against the
// pinned jar's `RuinedPortalPiece`, which it follows where they differ.

//! A ruined portal (`RuinedPortalPiece.postProcess`), on the surface, partly buried,
//! under the ground, in a mountain, on the ocean floor or in the Nether.
//!
//! It is placed only by the chunk that holds the middle of its template's box, and then
//! whole: that chunk's box is widened to take the template in. Then netherrack (or,
//! where the portal is not cold, now and then magma) spreads around it over the ground,
//! with columns dripping down under it; vines grow on its sides and jungle leaves on
//! its netherrack where the setup says so.
//!
//! Where SteelMC differs, the game is followed: a block of a dripping column, as each
//! block of netherrack or magma it places, is placed (and its magma drawn for) only
//! where the block there may be replaced (`placeNetherrackOrMagma` asks before it
//! draws).

use clustine_data::{BlockState, Face, blocks, faces};
use clustine_noise::Random;
use clustine_structures::geometry::BoundingBox;
use clustine_structures::piece::{
    RuinedPortalProperties, TemplatePieceData, TemplateProcessorList,
};
use clustine_structures::processors::{
    BlockEntityModifier, PosRuleTest, Processor, ProcessorRule, RuleTest,
};
use clustine_world::BlockPos;
use clustine_world::generation::PieceState;
use clustine_worldgen_data::structure::PortalPlacement;

use super::Placing;
use super::shipwreck::to_block;
use super::template_piece::{post_process, settings, template};
use crate::blocks::{HORIZONTAL, block_id, face_name, in_block_tag, relative, same_block};
use crate::context::{Heightmap, Level};
use crate::features::caves::templates::FeatureShapes;

/// The chance of netherrack by a column's distance from the middle (`spreadNetherrack`).
const NETHERRACK_BY_DISTANCE: [f32; 14] = [
    1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.9, 0.9, 0.8, 0.7, 0.6, 0.4, 0.2,
];

/// Places a ruined portal's piece. Its box is its template's from the start and does
/// not move; `state` is returned as it is.
pub(super) fn place(
    placing: &mut Placing<'_, '_, '_>,
    data: &TemplatePieceData,
    state: PieceState,
) -> PieceState {
    let TemplateProcessorList::RuinedPortal {
        vertical_placement,
        properties,
    } = data.processors
    else {
        unreachable!("a ruined portal's piece has the portals' processors")
    };
    let Some(template) = template(placing, data.template_id.path) else {
        return state;
    };
    let position = to_block(data.template_position);
    let pivot = to_block(data.rotation_pivot);
    let bounds = clustine_structures::place::template_bounding_box(
        template.size,
        position,
        data.mirror,
        data.rotation,
        pivot,
    );
    if !placing.chunk_box.contains(bounds.center()) {
        return state;
    }
    // The game widens the box it was given, which is the start's own.
    let clip = BoundingBox::encapsulating(&placing.chunk_box, &bounds);

    let shapes = FeatureShapes {
        settings: placing.settings,
    };
    let processors = processors(vertical_placement, properties);
    let mut settings = settings(placing, clip, &shapes);
    settings.rotation = data.rotation;
    settings.mirror = data.mirror;
    settings.rotation_pivot = pivot;
    settings.liquid_settings = data.liquid_settings;
    settings.processors.extend(processors.iter());
    // The portal's `handleDataMarker` does nothing.
    let bounds = post_process(placing, template, position, &settings, &mut |_, _, _| {});

    let portal = Portal {
        placement: vertical_placement,
        properties,
        bounds,
    };
    portal.spread_netherrack(placing);
    portal.drip_columns_below(placing);
    if properties.vines || properties.overgrown {
        // `BlockPos.betweenClosedStream`: x fastest, then y, then z.
        for z in bounds.min_z()..=bounds.max_z() {
            for y in bounds.min_y()..=bounds.max_y() {
                for x in bounds.min_x()..=bounds.max_x() {
                    let at = BlockPos::new(x, y, z);
                    if properties.vines {
                        maybe_add_vines(placing, at);
                    }
                    if properties.overgrown {
                        maybe_add_leaves_above(placing, at);
                    }
                }
            }
        }
    }
    state
}

/// `RuinedPortalPiece.makeSettings`'s processors, in the game's order.
fn processors(placement: PortalPlacement, properties: RuinedPortalProperties) -> Vec<Processor> {
    let ignore = if properties.air_pocket {
        Processor::structure_block_ignore()
    } else {
        Processor::structure_and_air_ignore()
    };
    let lava = if placement == PortalPlacement::OnOceanFloor {
        replace(blocks::LAVA, None, blocks::MAGMA_BLOCK)
    } else if properties.cold {
        replace(blocks::LAVA, None, blocks::NETHERRACK)
    } else {
        replace(blocks::LAVA, Some(0.2), blocks::MAGMA_BLOCK)
    };
    let mut rules = vec![replace(blocks::GOLD_BLOCK, Some(0.3), blocks::AIR), lava];
    if !properties.cold {
        rules.push(replace(blocks::NETHERRACK, Some(0.07), blocks::MAGMA_BLOCK));
    }
    let mut list = vec![
        ignore,
        Processor::Rule(rules),
        Processor::BlockAge {
            mossiness: properties.mossiness,
        },
        Processor::ProtectedBlocks("minecraft:features_cannot_replace"),
        Processor::LavaSubmergedBlock,
    ];
    if properties.replace_with_blackstone {
        list.push(Processor::BlackstoneReplace);
    }
    list
}

/// `getBlockReplaceRule`: the block, matched always or with the chance, becomes the
/// output's default state.
fn replace(block: BlockState, chance: Option<f32>, output: BlockState) -> ProcessorRule {
    let id = block_id(block);
    ProcessorRule {
        input: match chance {
            Some(chance) => RuleTest::RandomBlockMatch(id, chance),
            None => RuleTest::BlockMatch(id),
        },
        location: RuleTest::AlwaysTrue,
        position: PosRuleTest::AlwaysTrue,
        output,
        modifier: BlockEntityModifier::Passthrough,
    }
}

/// What the portal's own steps after the template read.
struct Portal {
    placement: PortalPlacement,
    properties: RuinedPortalProperties,
    bounds: BoundingBox,
}

impl Portal {
    /// `spreadNetherrack`: over a diamond about the middle, each column by its chance
    /// takes netherrack at the ground (or, for a portal that does not follow the
    /// ground, no higher than the portal's foot), within three of the portal's foot,
    /// with leaves above where overgrown and a column dripping down.
    fn spread_netherrack(&self, placing: &mut Placing<'_, '_, '_>) {
        let follows_ground = matches!(
            self.placement,
            PortalPlacement::OnLandSurface | PortalPlacement::OnOceanFloor
        );
        let centre = self.bounds.center();
        let reach = NETHERRACK_BY_DISTANCE.len() as i32;
        let average = (self.bounds.width() + self.bounds.depth()) / 2;
        let adjustment = placing.random.next_i32_bounded(1.max(8 - average / 2));
        let foot = self.bounds.min_y();
        for x in centre.x - reach..=centre.x + reach {
            for z in centre.z - reach..=centre.z + reach {
                let distance = (x - centre.x).abs() + (z - centre.z).abs();
                let adjusted = 0.max(distance + adjustment);
                if adjusted >= reach {
                    continue;
                }
                let chance = NETHERRACK_BY_DISTANCE[adjusted as usize];
                if placing.random.next_f64() >= f64::from(chance) {
                    continue;
                }
                let surface = self.surface_y(placing.level, x, z);
                let y = if follows_ground {
                    surface
                } else {
                    foot.min(surface)
                };
                let at = BlockPos::new(x, y, z);
                if (y - foot).abs() > 3 || !self.can_replace(placing.level.block(at)) {
                    continue;
                }
                self.place_netherrack_or_magma(placing, at);
                if self.properties.overgrown {
                    maybe_add_leaves_above(placing, at);
                }
                self.drip_column(placing, BlockPos::new(x, y - 1, z));
            }
        }
    }

    /// `addNetherrackDripColumnsBelowPortal`: under each block of netherrack inside
    /// the foot's rim, a column.
    fn drip_columns_below(&self, placing: &mut Placing<'_, '_, '_>) {
        let foot = self.bounds.min_y();
        for x in self.bounds.min_x() + 1..self.bounds.max_x() {
            for z in self.bounds.min_z() + 1..self.bounds.max_z() {
                let at = BlockPos::new(x, foot, z);
                if same_block(placing.level.block(at), blocks::NETHERRACK) {
                    self.drip_column(placing, BlockPos::new(x, foot - 1, z));
                }
            }
        }
    }

    /// `addNetherrackDripColumn`: one block, then down by one more while a draw falls
    /// under a half, eight at most.
    fn drip_column(&self, placing: &mut Placing<'_, '_, '_>, from: BlockPos) {
        let mut at = from;
        self.place_netherrack_or_magma(placing, at);
        let mut left = 8;
        while left > 0 && placing.random.next_f32() < 0.5 {
            at = BlockPos::new(at.x, at.y - 1, at.z);
            left -= 1;
            self.place_netherrack_or_magma(placing, at);
        }
    }

    /// `placeNetherrackOrMagma`: where the block may be replaced, magma for a draw
    /// under 0.07 where the portal is not cold, netherrack otherwise.
    fn place_netherrack_or_magma(&self, placing: &mut Placing<'_, '_, '_>, at: BlockPos) {
        if !self.can_replace(placing.level.block(at)) {
            return;
        }
        let state = if !self.properties.cold && placing.random.next_f32() < 0.07 {
            blocks::MAGMA_BLOCK
        } else {
            blocks::NETHERRACK
        };
        placing.level.set_block(at, state);
    }

    /// `canBlockBeReplacedByNetherrackOrMagma`: not air itself (cave air is), obsidian,
    /// what features may not replace, or, outside the Nether, lava.
    fn can_replace(&self, state: BlockState) -> bool {
        !same_block(state, blocks::AIR)
            && !same_block(state, blocks::OBSIDIAN)
            && !in_block_tag(state, "minecraft:features_cannot_replace")
            && (self.placement == PortalPlacement::InNether || !same_block(state, blocks::LAVA))
    }

    /// `getSurfaceY`: one below the height of the ocean floor for a portal on it, of the
    /// surface otherwise.
    fn surface_y(&self, level: &mut Level<'_>, x: i32, z: i32) -> i32 {
        let map = if self.placement == PortalPlacement::OnOceanFloor {
            Heightmap::OceanFloorWg
        } else {
            Heightmap::WorldSurfaceWg
        };
        level.height_at(map, x, z) - 1
    }
}

/// `maybeAddVines`: from a block that is neither air nor vine, a random side; where
/// that side is air and the block's collision fills its face there, a vine hangs on
/// it.
fn maybe_add_vines(placing: &mut Placing<'_, '_, '_>, at: BlockPos) {
    let state = placing.level.block(at);
    if state.props().is_air() || same_block(state, blocks::VINE) {
        return;
    }
    // `Direction.Plane.HORIZONTAL.getRandomDirection`.
    let face = HORIZONTAL[placing.random.next_i32_bounded(4) as usize];
    let beside = relative(at, face);
    if !placing.level.block(beside).props().is_air() {
        return;
    }
    let collision: Vec<[f64; 6]> = state.props().collision_shape().boxes().collect();
    if !faces::face_full(&collision, face) {
        return;
    }
    let vine = blocks::VINE
        .with_property(face_name(face.opposite()), "true")
        .expect("a vine hangs on each side");
    placing.level.set_block(beside, vine);
}

/// `maybeAddLeavesAbove`: for a draw under a half, persistent jungle leaves on
/// netherrack with air above it.
fn maybe_add_leaves_above(placing: &mut Placing<'_, '_, '_>, at: BlockPos) {
    if placing.random.next_f32() >= 0.5 {
        return;
    }
    if !same_block(placing.level.block(at), blocks::NETHERRACK) {
        return;
    }
    let above = relative(at, Face::Up);
    if !placing.level.block(above).props().is_air() {
        return;
    }
    let leaves = blocks::JUNGLE_LEAVES
        .with_property("persistent", "true")
        .expect("leaves can be persistent");
    placing.level.set_block(above, leaves);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn properties(cold: bool) -> RuinedPortalProperties {
        RuinedPortalProperties {
            cold,
            mossiness: 0.2,
            air_pocket: false,
            overgrown: false,
            vines: false,
            replace_with_blackstone: false,
        }
    }

    #[test]
    fn a_cold_portal_turns_its_lava_to_netherrack_and_has_no_magma_rule() {
        let list = processors(PortalPlacement::OnLandSurface, properties(true));
        let Processor::Rule(rules) = &list[1] else {
            panic!("the second is the rules");
        };
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[1].output, blocks::NETHERRACK);
        assert_eq!(rules[1].input, RuleTest::BlockMatch(block_id(blocks::LAVA)));
        assert_eq!(list[0], Processor::structure_and_air_ignore());
        assert_eq!(list.len(), 5);
    }

    #[test]
    fn a_portal_on_the_ocean_floor_turns_all_its_lava_to_magma() {
        let mut props = properties(false);
        props.air_pocket = true;
        props.replace_with_blackstone = true;
        let list = processors(PortalPlacement::OnOceanFloor, props);
        assert_eq!(list[0], Processor::structure_block_ignore());
        let Processor::Rule(rules) = &list[1] else {
            panic!("the second is the rules");
        };
        assert_eq!(rules[1].input, RuleTest::BlockMatch(block_id(blocks::LAVA)));
        assert_eq!(rules[1].output, blocks::MAGMA_BLOCK);
        assert_eq!(
            rules[2].input,
            RuleTest::RandomBlockMatch(block_id(blocks::NETHERRACK), 0.07)
        );
        assert_eq!(list.last(), Some(&Processor::BlackstoneReplace));
    }

    #[test]
    fn netherrack_replaces_cave_air_but_not_air_obsidian_or_lava_outside_the_nether() {
        let portal = Portal {
            placement: PortalPlacement::OnLandSurface,
            properties: properties(false),
            bounds: BoundingBox::new(
                clustine_structures::geometry::IVec3::new(0, 0, 0),
                clustine_structures::geometry::IVec3::new(1, 1, 1),
            ),
        };
        assert!(portal.can_replace(blocks::CAVE_AIR));
        assert!(portal.can_replace(blocks::STONE));
        assert!(!portal.can_replace(blocks::AIR));
        assert!(!portal.can_replace(blocks::OBSIDIAN));
        assert!(!portal.can_replace(blocks::LAVA));
        assert!(!portal.can_replace(blocks::BEDROCK));
        let nether = Portal {
            placement: PortalPlacement::InNether,
            ..portal
        };
        assert!(nether.can_replace(blocks::LAVA));
    }

    #[test]
    fn a_dripping_column_stops_at_air_without_drawing_for_magma() {
        use clustine_noise::WorldgenRandom;

        use crate::testing::World;

        let portal = Portal {
            placement: PortalPlacement::OnLandSurface,
            properties: properties(false),
            bounds: BoundingBox::new(
                clustine_structures::geometry::IVec3::new(0, 0, 0),
                clustine_structures::geometry::IVec3::new(1, 1, 1),
            ),
        };
        // Air everywhere above 0: the column's first block is not placed, and only the
        // draws that decide whether it goes on are made.
        let mut world = World::flat(blocks::STONE, 0);
        let mut random = WorldgenRandom::from_seed(5);
        world.with_level(|level| {
            let settings = crate::testing::settings();
            let mut placing = Placing {
                level,
                random: &mut random,
                settings,
                chunk_box: BoundingBox::new(
                    clustine_structures::geometry::IVec3::new(0, -63, 0),
                    clustine_structures::geometry::IVec3::new(15, 319, 15),
                ),
                centre: clustine_world::ChunkPos { x: 0, z: 0 },
                pivot: clustine_structures::geometry::IVec3::new(0, 0, 0),
            };
            portal.drip_column(&mut placing, BlockPos::new(3, 20, 3));
        });
        assert!(world.writes.is_empty());
        let mut expected = WorldgenRandom::from_seed(5);
        let mut steps = 0;
        while steps < 8 && expected.next_f32() < 0.5 {
            steps += 1;
        }
        assert_eq!(random.next_i64(), expected.next_i64());
    }
}
