// Adapted from SteelMC's `steel-core/src/worldgen/structure/piece_placer/template_piece.rs`
// and `template_processors.rs` (see NOTICE in this crate), the parts for ocean ruins,
// and checked against the pinned jar's `OceanRuinPieces$OceanRuinPiece`, `Drowned`,
// `Zombie` and `Mob`.

//! A piece of an ocean ruin, warm or cold (`OceanRuinPieces.OceanRuinPiece.postProcess`):
//! set on the ocean floor at its corner, lowered to the lowest ground under it where
//! enough of its base would stand over a drop, every time it is placed; then placed as
//! any template piece, with its markers for chests and drowned.
//!
//! - Its processors are the game's, in its order: blocks rot away to the piece's
//!   integrity, structure blocks and air are ignored, and up to five blocks of sand
//!   (warm) or gravel (cold) become suspicious, with the archaeology loot.
//! - The marker `chest` becomes a chest, waterlogged where water was, with the big or
//!   the small ruin's loot.
//! - The marker `drowned` makes a drowned, finalised from the region's random source;
//!   while the maker does not make the type, the numbers its `finalizeSpawn` draws of
//!   that source are drawn ([`drowned_draws`]). The marker becomes water, or air above
//!   the sea.

use clustine_data::blocks;
use clustine_noise::{Random, RandomSource};
use clustine_structures::compat::OceanRuinBiomeTempData;
use clustine_structures::piece::{
    StructureMirror, StructurePiece, TemplatePieceData, TemplateProcessorList,
};
use clustine_structures::place::transform_pos;
use clustine_structures::processors::{
    BlockEntityModifier, IntProvider, PosRuleTest, Processor, ProcessorRule, RuleTest,
};
use clustine_world::BlockPos;
use clustine_world::generation::PieceState;

use super::Placing;
use super::template_piece::{CHESTS, post_process, set_loot_table_if, settings, template};
use crate::blocks::{block_id, fluid_in_tag, in_block_tag};
use crate::context::{Heightmap, Level};
use crate::features::caves::templates::FeatureShapes;

/// Places a piece of an ocean ruin. `state` says how far its box has moved; the state
/// returned is to be kept.
pub(super) fn place(
    placing: &mut Placing<'_, '_, '_>,
    piece: &StructurePiece,
    data: &TemplatePieceData,
    is_large: bool,
    mut state: PieceState,
) -> PieceState {
    let TemplateProcessorList::OceanRuin {
        biome_temp,
        integrity,
    } = data.processors
    else {
        unreachable!("a piece of an ocean ruin has the ruins' processors")
    };
    let Some(template) = template(placing, data.template_id.path) else {
        return state;
    };
    let start = data.template_position;
    let floor = placing
        .level
        .height_at(Heightmap::OceanFloorWg, start.x, start.z);
    let base = BlockPos::new(start.x, floor, start.z);
    let corner = transform_pos(
        BlockPos::new(template.size[0] - 1, 0, template.size[2] - 1),
        StructureMirror::None,
        data.rotation,
        BlockPos::new(0, 0, 0),
    );
    let corner = BlockPos::new(base.x + corner.x, base.y + corner.y, base.z + corner.z);
    let position = BlockPos::new(start.x, height(placing.level, base, corner), start.z);

    let shapes = FeatureShapes {
        settings: placing.settings,
    };
    let processors = processors(biome_temp, integrity);
    let mut settings = settings(placing, placing.chunk_box, &shapes);
    settings.rotation = data.rotation;
    settings.mirror = data.mirror;
    settings.rotation_pivot = super::shipwreck::to_block(data.rotation_pivot);
    settings.liquid_settings = data.liquid_settings;
    settings.processors.extend(processors.iter());
    let bounds = post_process(
        placing,
        template,
        position,
        &settings,
        &mut |placing, marker, at| match marker {
            "chest" => {
                let water = fluid_in_tag(placing.level.block(at), "minecraft:water");
                let chest = blocks::CHEST
                    .with_property("waterlogged", if water { "true" } else { "false" })
                    .expect("a chest can be waterlogged");
                placing.level.set_block(at, chest);
                let table = if is_large {
                    "minecraft:chests/underwater_ruin_big"
                } else {
                    "minecraft:chests/underwater_ruin_small"
                };
                set_loot_table_if(placing.level, &mut *placing.random, at, table, &CHESTS);
            }
            "drowned" => {
                drowned(placing, at);
            }
            _ => {}
        },
    );
    state.shift_y = bounds.min_y() - piece.bounding_box.min_y();
    state
}

/// `OceanRuinPiece.makeSettings`'s processors, in the game's order.
fn processors(biome_temp: OceanRuinBiomeTempData, integrity: f32) -> Vec<Processor> {
    let (source, target, loot) = match biome_temp {
        OceanRuinBiomeTempData::Warm => (
            blocks::SAND,
            blocks::SUSPICIOUS_SAND,
            "minecraft:archaeology/ocean_ruin_warm",
        ),
        OceanRuinBiomeTempData::Cold => (
            blocks::GRAVEL,
            blocks::SUSPICIOUS_GRAVEL,
            "minecraft:archaeology/ocean_ruin_cold",
        ),
    };
    vec![
        Processor::BlockRot {
            rottable: None,
            integrity,
        },
        Processor::structure_and_air_ignore(),
        // `archyRuleProcessor`.
        Processor::Capped {
            delegate: Box::new(Processor::Rule(vec![ProcessorRule {
                input: RuleTest::BlockMatch(block_id(source)),
                location: RuleTest::AlwaysTrue,
                position: PosRuleTest::AlwaysTrue,
                output: target,
                modifier: BlockEntityModifier::AppendLoot(loot.to_string()),
            }])),
            limit: IntProvider::Constant(5),
        },
    ]
}

/// `OceanRuinPiece.getHeight`: the ruin goes down to one above the lowest ground under
/// its base, from `base` to `corner`, where that lies more than two below the ocean
/// floor and more columns than its width less two drop more than two.
fn height(level: &mut Level<'_>, base: BlockPos, corner: BlockPos) -> i32 {
    let mut new_y = base.y;
    let mut lowest = 512;
    let top = base.y - 1;
    let mut dropping = 0;
    for z in base.z.min(corner.z)..=base.z.max(corner.z) {
        for x in base.x.min(corner.x)..=base.x.max(corner.x) {
            let mut y = base.y - 1;
            loop {
                let state = level.block(BlockPos::new(x, y, z));
                let open = state.props().is_air()
                    || fluid_in_tag(state, "minecraft:water")
                    || in_block_tag(state, "minecraft:ice");
                if !open || y <= level.min_y() + 1 {
                    break;
                }
                y -= 1;
            }
            lowest = lowest.min(y);
            if y < top - 2 {
                dropping += 1;
            }
        }
    }
    let width = (base.x - corner.x).abs();
    if top - lowest > 2 && dropping > width - 2 {
        new_y = lowest + 1;
    }
    new_y
}

/// The marker `drowned`: the game makes a drowned there, kept from despawning, at the
/// block's bottom centre facing nowhere (`snapTo(pos, 0, 0)`, **Inferred**: the piece
/// is not in the dumps), and finalised; then the marker becomes water, or air above
/// the sea. Where the maker does not make a drowned yet, what it draws is drawn.
fn drowned(placing: &mut Placing<'_, '_, '_>, at: BlockPos) {
    let position = [
        f64::from(at.x) + 0.5,
        f64::from(at.y),
        f64::from(at.z) + 0.5,
    ];
    let settings = placing.settings;
    // Which of the two ruins this is no drowned's draws read.
    let made = placing.level.make_mob(
        settings,
        clustine_data::entity_types::DROWNED,
        true,
        position,
        None,
    );
    if !made {
        let river = biome_is_river(placing.level.biome(at).0);
        let seed = placing.settings.seed();
        let legacy = placing.settings.legacy_random_source();
        drowned_draws(placing.level.region_random(seed, legacy), river);
    }
    let sea_level = placing.settings.generation().sea_level;
    let replacement = if at.y > sea_level {
        blocks::AIR
    } else {
        blocks::WATER
    };
    placing.level.set_block(at, replacement);
}

/// Whether the biome, by its number, is of `#minecraft:more_frequent_drowned_spawns`,
/// which is `#minecraft:is_river`.
fn biome_is_river(biome: u16) -> bool {
    clustine_data::TAGS
        .iter()
        .find(|tags| tags.registry == "minecraft:worldgen/biome")
        .and_then(|tags| {
            tags.tags
                .iter()
                .find(|tag| tag.name == "minecraft:is_river")
        })
        .is_some_and(|tag| tag.entries.contains(&i32::from(biome)))
}

/// What a drowned made by a structure draws from the region's random source in
/// `finalizeSpawn` (`Drowned`, `Zombie`, `Mob`), with the difficulty's special
/// multiplier taken as nought: on easy it is always nought, and on normal or hard it is
/// in a world's first hour. Its own random source draws the rest, which nothing else
/// shares.
///
/// - `Mob`: the follow range's bonus, a triangle of two doubles; left-handed, a float.
/// - `Zombie`: picking up loot, a float; a baby, a float, and for a baby the chicken
///   jockey's two (a chicken nearby, of which a region being generated has none; else
///   a new chicken, which draws its own [`chicken_draws`]); breaking doors, a float;
///   `Drowned.populateDefaultEquipmentSlots`, a float and for one in ten an int of 16,
///   a trident below ten, else a fishing rod; enchanting the hand's item, a float where
///   it holds one. On the 31st of October a zombie draws one or two more for a pumpkin
///   on its head, which this does not (the game reads the clock there).
/// - `Drowned`: a nautilus shell, a float; for one with a trident, a float, and for an
///   adult outside a river one in two of those makes a zombie nautilus to ride, which
///   draws its own [`zombie_nautilus_draws`].
pub(super) fn drowned_draws(random: &mut RandomSource, in_river: bool) {
    mob_draws(random);
    let _pick_up_loot = random.next_f32();
    let baby = random.next_f32() < 0.05;
    if baby {
        // `canSpawnJockey` is true for a zombie's own group.
        if f64::from(random.next_f32()) < 0.05 {
            // The chickens nearby: none in a region being generated.
        } else if f64::from(random.next_f32()) < 0.05 {
            chicken_draws(random);
        }
    }
    let _break_doors = random.next_f32();
    let mut trident = false;
    if f64::from(random.next_f32()) > 0.9 {
        trident = random.next_i32_bounded(16) < 10;
        // The hand holds a trident or a fishing rod: `enchantSpawnedEquipment` draws.
        let _enchant = random.next_f32();
    }
    let _nautilus_shell = random.next_f32();
    if trident && random.next_f32() < 0.5 && !baby && !in_river {
        zombie_nautilus_draws(random);
    }
}

/// `Mob.finalizeSpawn`: `triangle(0, 0.11485)` for the follow range, then whether the
/// mob is left-handed.
fn mob_draws(random: &mut RandomSource) {
    let _follow_range = random.triangle(0.0, 0.114_850_000_000_000_01);
    let _left_handed = random.next_f32();
}

/// A chicken jockey's chicken (`Chicken.finalizeSpawn`): its variant, the one of the
/// highest priority whose biome matches, picked from a list of one; its sound, one of
/// the two; then `AgeableMob`'s, which draws nothing for the first of a group, and the
/// mob's.
fn chicken_draws(random: &mut RandomSource) {
    let _variant = random.next_i32_bounded(1);
    let _sound = random.next_i32_bounded(2);
    mob_draws(random);
}

/// A drowned's zombie nautilus (`ZombieNautilus.finalizeSpawn`): its variant, from a
/// list of one; the cooldown of its attacks, `UniformInt` 2400 to 3600 (`NautilusAi`);
/// then `AgeableMob`'s, nothing, and the mob's.
fn zombie_nautilus_draws(random: &mut RandomSource) {
    let _variant = random.next_i32_bounded(1);
    let _cooldown = random.next_i32_bounded(3600 - 2400 + 1);
    mob_draws(random);
}

#[cfg(test)]
mod tests {
    use clustine_noise::{PositionalRandom, RandomSource};

    use super::*;

    fn source(seed: u64) -> RandomSource {
        clustine_noise::Xoroshiro::from_seed(seed)
            .next_positional()
            .at(0, 0, 0)
    }

    #[test]
    fn a_drowned_draws_at_least_eight_numbers_in_the_game_s_order() {
        // With a seed whose drowned is neither a baby nor armed, the draws are the
        // mob's two doubles and float, then five floats.
        for seed in 0..64 {
            let mut drawn = source(seed);
            drowned_draws(&mut drawn, false);
            let mut expected = source(seed);
            let _ = expected.next_f64();
            let _ = expected.next_f64();
            let _ = expected.next_f32();
            let _ = expected.next_f32();
            let baby = expected.next_f32() < 0.05;
            let _ = expected.next_f32();
            let armed = f64::from(expected.next_f32()) > 0.9;
            if baby || armed {
                continue;
            }
            let _ = expected.next_f32();
            assert_eq!(drawn.next_i64(), expected.next_i64(), "seed {seed}");
        }
    }

    #[test]
    fn the_rivers_are_the_tag_s_biomes() {
        let river = clustine_data::synced_registry("minecraft:worldgen/biome")
            .and_then(|registry| {
                registry
                    .entries
                    .iter()
                    .position(|name| *name == "minecraft:river")
            })
            .expect("the river is a biome");
        assert!(biome_is_river(u16::try_from(river).unwrap()));
        assert!(!biome_is_river(0));
    }

    #[test]
    fn a_ruin_over_a_drop_goes_down_to_the_lowest_ground_under_it() {
        use crate::testing::World;
        // Stone up to 40, a pit to 30 under most of a base seven wide.
        let mut world = World::flat(blocks::STONE, 40);
        for x in 0..7 {
            for z in 0..7 {
                for y in 31..=40 {
                    world.set(BlockPos::new(x, y, z), blocks::WATER);
                }
            }
        }
        let y = world
            .with_level(|level| height(level, BlockPos::new(0, 41, 0), BlockPos::new(6, 41, 6)));
        assert_eq!(y, 31);
        // On flat ground it stays where the floor is.
        let mut flat = World::flat(blocks::STONE, 40);
        let y = flat
            .with_level(|level| height(level, BlockPos::new(0, 41, 0), BlockPos::new(6, 41, 6)));
        assert_eq!(y, 41);
    }

    #[test]
    fn the_processors_are_rot_then_ignore_then_five_suspicious_blocks() {
        let list = processors(OceanRuinBiomeTempData::Cold, 0.7);
        assert!(
            matches!(list[0], Processor::BlockRot { rottable: None, integrity } if integrity == 0.7)
        );
        assert_eq!(list[1], Processor::structure_and_air_ignore());
        let Processor::Capped { delegate, limit } = &list[2] else {
            panic!("the third is capped");
        };
        assert_eq!(*limit, IntProvider::Constant(5));
        let Processor::Rule(rules) = delegate.as_ref() else {
            panic!("it caps a rule");
        };
        assert_eq!(rules[0].output, blocks::SUSPICIOUS_GRAVEL);
        assert_eq!(
            rules[0].modifier,
            BlockEntityModifier::AppendLoot("minecraft:archaeology/ocean_ruin_cold".into())
        );
    }
}
