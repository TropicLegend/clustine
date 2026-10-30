//! What water does where the level's `water_evaporates` holds (the Nether's; p-plan
//! 17.2 A, 17.5 and 17.6): a bucket's water fizzes away instead of going in, and ice
//! leaves no water where it goes. Ice that melts by light is `random_ticks/melting.rs`'
//! and mud over dripstone `random_ticks/minerals.rs`'; this file has what they share and
//! what a player's break of ice does.
//!
//! The sponge, which `SpongeBlock` (and the wet sponge, which dries at once where water
//! evaporates) would read the same attribute for, is not in Clustine at all: named here,
//! not built.

use clustine_blocks::data::in_tag;
use clustine_data::{BlockClass, BlockState, Face, blocks, sounds};
use clustine_world::{BlockPos, PlayerId};

use super::Region;
use super::random_ticks::Random;
use crate::api::{RegionEvent, SOUND_SOURCE_BLOCKS, TickOutput};

/// The blocks below broken ice that let it become water
/// (`BlockTags.ICE_MELTS_WHEN_DESTROYED_ABOVE`).
const ICE_MELTS_WHEN_DESTROYED_ABOVE: &str = "minecraft:ice_melts_when_destroyed_above";

impl Region {
    /// Whether water evaporates in the region's level (`EnvironmentAttributes.WATER_EVAPORATES`,
    /// which the dimension's type sets for the whole level).
    pub(in crate::region) fn water_evaporates(&self) -> bool {
        self.config.dimension.rules().water_evaporates
    }

    /// Water from a bucket evaporates at `pos` (n7 BucketItem.txt 217-318, the branch of
    /// `emptyContents` where `water_evaporates` holds): `fire_extinguish` of the blocks'
    /// source at the block's centre, at volume 0.5 and a pitch of `2.6 + (a − b) × 0.8`
    /// from two floats of the level's random, heard by all but `except` (the player who
    /// emptied it, whose client plays its own); then eight `large_smoke` over the block.
    /// The region sends no particles of its own, so the smoke is the fizz that lava's
    /// meeting with water makes (level event 1501), which shows the same eight puffs and
    /// plays lava's hiss besides. No game event: `playEmptySound`, which posts
    /// `fluid_place`, is not reached.
    pub(crate) fn evaporate_water(
        &mut self,
        pos: BlockPos,
        except: Option<PlayerId>,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        let pitch = 2.6 + (random.next_float() - random.next_float()) * 0.8;
        output.events.push(RegionEvent::Sound {
            sound: sounds::BLOCK_FIRE_EXTINGUISH,
            source: SOUND_SOURCE_BLOCKS,
            position: clustine_world::Vec3::new(
                f64::from(pos.x) + 0.5,
                f64::from(pos.y) + 0.5,
                f64::from(pos.z) + 0.5,
            ),
            volume: 0.5,
            pitch,
            except,
        });
        output.events.push(RegionEvent::Fizzed { position: pos });
    }

    /// `IceBlock.playerDestroy` (n8 IceBlock.txt 15-90) for `state` broken by a player at
    /// `pos`, which the break has already left as air. The game calls it only where the
    /// break makes drops (`ServerPlayerGameMode.destroyBlock` returns before it for a
    /// creative player, `preventsBlockDrops`), which `makes_drops` says; every Clustine
    /// player is creative, so in practice ice broken leaves air everywhere, and this is
    /// built for when survival comes. Where water evaporates the ice is only removed
    /// (`removeBlock` of what is air already, which changes nothing). Elsewhere it becomes
    /// water, with every update, where the block below is in
    /// `ice_melts_when_destroyed_above` or is a liquid. The tool's Silk Touch
    /// (`prevents_ice_melting`) would keep it as air too, but stacks carry no
    /// enchantments yet.
    pub(crate) fn ice_broken_by_player(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        makes_drops: bool,
        output: &mut TickOutput,
    ) {
        if !makes_drops || !clustine_blocks::data::is_class(state, BlockClass::IceBlock) {
            return;
        }
        if self.water_evaporates() {
            return;
        }
        let Some(below) = self.block(clustine_blocks::data::relative(pos, Face::Down)) else {
            return;
        };
        if in_tag(below, ICE_MELTS_WHEN_DESTROYED_ABOVE) || below.props().is_liquid() {
            self.set_block(pos, blocks::WATER, output);
        }
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::{DIMENSION_TYPES, items};
    use clustine_world::{Biome, Chunk, ChunkPos, Dimension, EntityIds, Vec3};

    use super::*;
    use crate::api::Inventory;
    use crate::region::{Holdings, RegionConfig};

    fn player(number: u128) -> PlayerId {
        PlayerId(uuid::Uuid::from_u128(number))
    }

    /// The floor of the test's chunks, within every dimension's height.
    const FLOOR: i32 = 64;

    /// A region of `dimension` with the 3×3 chunks around the origin loaded, empty but
    /// for a stone floor at `FLOOR`.
    fn region_in(dimension: Dimension) -> Region {
        let rules = dimension.rules();
        let config = RegionConfig {
            homes: Vec::new(),
            dimension,
            spawn: Some(Vec3::new(0.5, f64::from(FLOOR + 1), 0.5)),
            starting_inventory: Inventory::default(),
            return_after: 0,
            lowest_y: rules.min_y,
            seed: 0,
            game_seed: 0,
            sea_level: 63,
            spawn_mobs: false,
            light: Default::default(),
            features: Default::default(),
            random_tick_speed: crate::region::RANDOM_TICK_SPEED,
        };
        let ids = EntityIds::block(0).expect("the first block of ids");
        let mut region = Region::new(config, ids, Holdings::default());
        let kind = DIMENSION_TYPES
            .iter()
            .find(|kind| kind.name == dimension.type_name())
            .expect("the dimension's type");
        for x in -1..=1 {
            for z in -1..=1 {
                let mut chunk = Chunk::empty(kind, Biome(0));
                for bx in 0..16 {
                    for bz in 0..16 {
                        chunk.set(bx, FLOOR, bz, blocks::STONE);
                    }
                }
                region.chunks.insert(ChunkPos::new(x, z), chunk);
            }
        }
        region.update_ticking();
        region
    }

    fn put(region: &mut Region, pos: BlockPos, state: BlockState) {
        let (x, z) = pos.in_chunk();
        let chunk = region.chunks.get_mut(&pos.chunk()).expect("a loaded chunk");
        chunk.set(x, pos.y, z, state).expect("within the world");
    }

    fn get(region: &Region, pos: BlockPos) -> BlockState {
        region.block(pos).expect("a loaded block")
    }

    const ICE: BlockPos = BlockPos::new(8, FLOOR + 2, 8);

    /// Ice at `ICE` over `below`, broken by a player whose break makes drops or not.
    fn break_ice(dimension: Dimension, below: BlockState, makes_drops: bool) -> BlockState {
        let mut region = region_in(dimension);
        put(&mut region, ICE.offset(0, -1, 0), below);
        put(&mut region, ICE, blocks::ICE);
        let mut output = TickOutput::default();
        region.break_by_player(ICE, None, &mut output);
        assert_eq!(
            get(&region, ICE),
            blocks::AIR,
            "the break itself leaves air"
        );
        region.ice_broken_by_player(ICE, blocks::ICE, makes_drops, &mut output);
        get(&region, ICE)
    }

    #[test]
    fn ice_broken_in_creative_leaves_air_in_every_dimension() {
        for dimension in Dimension::ALL {
            for below in [blocks::STONE, blocks::WATER, blocks::AIR] {
                assert_eq!(
                    break_ice(dimension, below, false),
                    blocks::AIR,
                    "{dimension:?} over {below:?}"
                );
            }
        }
    }

    #[test]
    fn ice_broken_with_drops_over_a_solid_block_or_a_liquid_becomes_water_where_water_stays() {
        for dimension in [Dimension::Overworld, Dimension::End] {
            for below in [blocks::STONE, blocks::WATER, blocks::LAVA] {
                assert_eq!(
                    break_ice(dimension, below, true),
                    blocks::WATER,
                    "{dimension:?} over {below:?}"
                );
            }
        }
    }

    #[test]
    fn ice_broken_with_drops_over_air_or_a_block_not_in_the_tag_leaves_air() {
        for dimension in Dimension::ALL {
            for below in [blocks::AIR, blocks::TORCH, blocks::OAK_SAPLING] {
                assert!(!in_tag(below, ICE_MELTS_WHEN_DESTROYED_ABOVE), "{below:?}");
                assert_eq!(
                    break_ice(dimension, below, true),
                    blocks::AIR,
                    "{dimension:?} over {below:?}"
                );
            }
        }
    }

    #[test]
    fn ice_broken_with_drops_in_the_nether_leaves_air_over_anything() {
        for below in [blocks::STONE, blocks::WATER, blocks::LAVA] {
            assert_eq!(break_ice(Dimension::Nether, below, true), blocks::AIR);
        }
    }

    #[test]
    fn breaking_a_player_s_ice_wires_the_rule_in_and_leaves_air_for_a_creative_player() {
        // Through `break_by_player` alone, as a player's dig reaches it.
        for dimension in Dimension::ALL {
            let mut region = region_in(dimension);
            put(&mut region, ICE.offset(0, -1, 0), blocks::STONE);
            put(&mut region, ICE, blocks::ICE);
            let mut output = TickOutput::default();
            region.break_by_player(ICE, Some(player(1)), &mut output);
            assert_eq!(get(&region, ICE), blocks::AIR, "{dimension:?}");
        }
    }

    #[test]
    fn only_ice_itself_turns_to_water_when_broken() {
        let mut region = region_in(Dimension::Overworld);
        for state in [blocks::PACKED_ICE, blocks::BLUE_ICE, blocks::GLASS] {
            assert!(!clustine_blocks::data::is_class(
                state,
                BlockClass::IceBlock
            ));
            let mut output = TickOutput::default();
            region.ice_broken_by_player(ICE, state, true, &mut output);
            assert_eq!(get(&region, ICE), blocks::AIR, "{state:?}");
        }
    }

    #[test]
    fn water_evaporates_in_the_nether_only() {
        assert!(!region_in(Dimension::Overworld).water_evaporates());
        assert!(region_in(Dimension::Nether).water_evaporates());
        assert!(!region_in(Dimension::End).water_evaporates());
    }

    #[test]
    fn evaporating_water_fizzes_with_the_fire_s_hiss_at_half_volume_and_a_high_pitch() {
        let mut region = region_in(Dimension::Nether);
        let mut random = Random::of_section(3, ChunkPos::new(0, 0), 0, 9);
        let start = random.clone();
        let mut expected = random.clone();
        let pitch = 2.6 + (expected.next_float() - expected.next_float()) * 0.8;
        let mut output = TickOutput::default();
        region.evaporate_water(ICE, Some(player(7)), &mut random, &mut output);
        assert_eq!(random.draws_since(&start), 2, "two floats drawn");
        assert_eq!(
            output.events,
            [
                RegionEvent::Sound {
                    sound: sounds::BLOCK_FIRE_EXTINGUISH,
                    source: SOUND_SOURCE_BLOCKS,
                    position: Vec3::new(8.5, f64::from(FLOOR) + 2.5, 8.5),
                    volume: 0.5,
                    pitch,
                    except: Some(player(7)),
                },
                RegionEvent::Fizzed { position: ICE },
            ]
        );
        assert!((1.8..=3.4).contains(&pitch));
    }

    /// Where the dispensers of these tests stand, facing east, on the tests' world of
    /// `tnt/tests.rs` (a stone floor at y = -61).
    const DISPENSER: BlockPos = BlockPos::new(2, -60, 2);
    const FRONT: BlockPos = BlockPos::new(3, -60, 2);

    /// A dispenser holding `held`, with `front` before it, fired once in a region of
    /// `dimension`: the region after, and what the tick sent. The tests' world is the
    /// overworld's chunks; only the region's level is the dimension's, which is all the
    /// rule reads.
    fn dispense(
        dimension: Dimension,
        held: i32,
        front: Option<BlockState>,
    ) -> (Region, TickOutput) {
        let dispenser = BlockState::parse("minecraft:dispenser[facing=east,triggered=false]")
            .expect("a dispenser");
        let mut region = crate::region::tnt::tests::world(7, move |pos| {
            if pos == DISPENSER {
                Some(dispenser)
            } else if pos == FRONT {
                front
            } else {
                None
            }
        });
        region.config.dimension = dimension;
        let mut container = region.container_at(DISPENSER).expect("a container");
        container.slots[0] = Some(crate::api::ItemStack::new(held, 1));
        region.set_container(DISPENSER, container, &mut TickOutput::default());
        let mut output = TickOutput::default();
        assert!(region.dispenser_tick(DISPENSER, dispenser, &mut output));
        (region, output)
    }

    fn sounds_of(output: &TickOutput) -> Vec<i32> {
        output
            .events
            .iter()
            .filter_map(|event| match event {
                RegionEvent::Sound { sound, .. } => Some(*sound),
                _ => None,
            })
            .collect()
    }

    fn fizzes(output: &TickOutput) -> Vec<BlockPos> {
        output
            .events
            .iter()
            .filter_map(|event| match event {
                RegionEvent::Fizzed { position } => Some(*position),
                _ => None,
            })
            .collect()
    }

    fn first_slot(region: &Region) -> Option<i32> {
        let container = region.container_at(DISPENSER).expect("a container");
        container.slots[0].as_ref().map(|stack| stack.item)
    }

    #[test]
    fn a_dispensed_water_bucket_places_water_except_in_the_nether_where_it_fizzes() {
        for dimension in Dimension::ALL {
            let (region, output) = dispense(dimension, items::WATER_BUCKET, None);
            assert_eq!(first_slot(&region), Some(items::BUCKET), "{dimension:?}");
            if dimension == Dimension::Nether {
                assert_eq!(region.block(FRONT), Some(blocks::AIR));
                assert_eq!(sounds_of(&output), [sounds::BLOCK_FIRE_EXTINGUISH]);
                assert_eq!(fizzes(&output), [FRONT]);
            } else {
                assert_eq!(region.block(FRONT), Some(blocks::WATER), "{dimension:?}");
                assert_eq!(sounds_of(&output), [sounds::ITEM_BUCKET_EMPTY]);
                assert_eq!(fizzes(&output), [], "{dimension:?}");
            }
        }
    }

    #[test]
    fn a_dispensed_water_bucket_waterlogs_nothing_in_the_nether() {
        let slab = BlockState::parse("minecraft:oak_slab[type=bottom]").expect("a slab");
        for dimension in Dimension::ALL {
            let (region, output) = dispense(dimension, items::WATER_BUCKET, Some(slab));
            let now = region.block(FRONT).expect("loaded");
            if dimension == Dimension::Nether {
                assert_eq!(now, slab);
                assert_eq!(fizzes(&output), [FRONT]);
            } else {
                assert_eq!(now.property("waterlogged"), Some("true"), "{dimension:?}");
            }
            assert_eq!(first_slot(&region), Some(items::BUCKET), "{dimension:?}");
        }
    }

    #[test]
    fn a_dispensed_fish_bucket_s_water_fizzes_in_the_nether() {
        let (region, output) = dispense(Dimension::Nether, items::COD_BUCKET, None);
        assert_eq!(region.block(FRONT), Some(blocks::AIR));
        assert_eq!(fizzes(&output), [FRONT]);
        assert_eq!(first_slot(&region), Some(items::BUCKET));
    }

    #[test]
    fn a_dispensed_lava_bucket_places_lava_in_every_dimension() {
        for dimension in Dimension::ALL {
            let (region, output) = dispense(dimension, items::LAVA_BUCKET, None);
            assert_eq!(region.block(FRONT), Some(blocks::LAVA), "{dimension:?}");
            assert_eq!(fizzes(&output), [], "{dimension:?}");
        }
    }

    #[test]
    fn a_water_bucket_facing_stone_in_the_nether_is_dropped_without_a_fizz() {
        let (region, output) =
            dispense(Dimension::Nether, items::WATER_BUCKET, Some(blocks::STONE));
        assert_eq!(region.block(FRONT), Some(blocks::STONE));
        assert_eq!(fizzes(&output), []);
        assert_eq!(first_slot(&region), None, "the bucket is dropped");
    }
}
