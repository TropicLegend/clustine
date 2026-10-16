//! Fire and coral (step B7): fire's scheduled tick, burning out and spreading, lava's
//! random tick, which sets fire, and coral dying out of water. Ported from 26.3's
//! `FireBlock`, `BaseFireBlock`, `SoulFireBlock`, `LavaFluid.randomTick` and the coral
//! classes (`BaseCoralPlantTypeBlock`, `CoralPlantBlock`, `CoralFanBlock`,
//! `CoralWallFanBlock`, `CoralBlock`).
//!
//! What the game asks of the world here, and what Clustine answers:
//!
//! - rain: there is none yet, so `isRaining` and `isNearRain` are false and nothing
//!   puts fire out;
//! - the game rule `fire_spread_radius_around_player` (26.3's in place of
//!   `doFireTick`; 128 by default), which `ServerLevel.canSpreadFireAround` asks: fire
//!   ticks wherever a region ticks, since a region does not see the players of its
//!   neighbours and fire would stop at a region's edge if it judged by its own;
//! - the difficulty: normal, the game's default;
//! - the environment attribute `increased_fire_burnout`: by the biome at the block,
//!   those whose files in the 26.3 jar set it;
//! - TNT that fire reaches is primed (`TntBlock.prime`): Clustine has no entities, so
//!   the TNT is only burnt away like any block;
//! - fire in a frame of obsidian makes a nether portal (`BaseFireBlock.onPlace`):
//!   there are no portals yet.

use clustine_blocks::data::{has_lava, in_tag, is_block, is_class, number, relative, set, tick_id};
use clustine_blocks::shape::{
    coral_scan_for_water, coral_try_schedule_die_tick, fire_state, fire_state_with_age,
};
use clustine_blocks::survive::{
    burn_odds, can_survive, fire_can_burn, ignite_odds, sturdy, valid_fire_location,
};
use clustine_data::{BlockClass, BlockState, Face, SupportType, blocks};
use clustine_world::{BlockPos, Ticked};

use super::Random;
use crate::api::TickOutput;
use crate::region::Region;
use crate::region::updates::{RegionView, UPDATE_ALL, UPDATE_CLIENTS, UPDATE_LIMIT};

/// The game's difficulty by its id (`Difficulty.NORMAL`), which makes fire spread a
/// little faster than on easy and slower than on hard.
const DIFFICULTY: i32 = 2;

/// `Block.UPDATE_NONE` (`UPDATE_INVISIBLE | UPDATE_SKIP_BLOCK_ENTITY_SIDEEFFECTS`), with
/// which fire sets its own age: no neighbour is told, though their shapes are asked
/// again. Clustine tells clients of every change, so the first bit means nothing to it,
/// and fire has no block entity for the second.
const UPDATE_NONE: u32 = 260;

/// The tag of the blocks fire burns on for ever in the overworld (its dimension
/// type's `infiniburn`).
const INFINIBURN: &str = "minecraft:infiniburn_overworld";

/// The biomes that set `increased_fire_burnout` in 26.3's data, sorted.
const INCREASED_FIRE_BURNOUT: &[&str] = &[
    "minecraft:bamboo_jungle",
    "minecraft:frozen_peaks",
    "minecraft:jagged_peaks",
    "minecraft:jungle",
    "minecraft:mangrove_swamp",
    "minecraft:mushroom_fields",
    "minecraft:snowy_slopes",
    "minecraft:swamp",
];

impl Region {
    /// The random tick of `state` at `pos` if it is of this file's blocks; whether it
    /// was. Only lava ticks randomly here, and it ticks twice for each draw: as the
    /// block (`LiquidBlock.randomTick` hands it to the fluid) and as the fluid
    /// (`ServerLevel.tickChunk` asks the fluid of the same state too).
    pub(super) fn fire_random_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) -> bool {
        if !(is_class(state, BlockClass::LiquidBlock) && has_lava(state)) {
            return false;
        }
        self.lava_random_tick(pos, random, output);
        self.lava_random_tick(pos, random, output);
        true
    }

    /// The scheduled tick of fire or coral at `pos`, if `state` is one; whether it was.
    pub(crate) fn fire_block_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) -> bool {
        use BlockClass as C;
        if is_class(state, C::FireBlock) {
            self.fire_tick(pos, state, output);
            return true;
        }
        // Soul fire has no tick of its own.
        if is_class(state, C::SoulFireBlock) {
            return true;
        }
        // `CoralPlantBlock.tick`, `CoralFanBlock.tick`, `CoralWallFanBlock.tick`: out of
        // water, the coral becomes its dead form, set without telling the neighbours.
        if is_class(state, C::CoralPlantBlock)
            || is_class(state, C::CoralFanBlock)
            || is_class(state, C::CoralWallFanBlock)
        {
            if !coral_scan_for_water(&mut RegionView(self), state, pos) {
                let mut dead = set(dead_coral(state), "waterlogged", "false");
                if let Some(facing) = state.property("facing") {
                    dead = set(dead, "facing", facing);
                }
                self.set_block_updated(pos, dead, UPDATE_CLIENTS, UPDATE_LIMIT, output);
            }
            return true;
        }
        // `CoralBlock.tick`: a coral block with no water beside it dies.
        if is_class(state, C::CoralBlock) {
            if !self.water_beside(pos) {
                let dead = dead_coral(state);
                self.set_block_updated(pos, dead, UPDATE_CLIENTS, UPDATE_LIMIT, output);
            }
            return true;
        }
        false
    }

    /// What fire and coral do as they are placed (`onPlace`): fire ticks in 30 to 39
    /// ticks (`FireBlock.onPlace`), and a coral plant or fan out of water dies in 60 to
    /// 99 (`tryScheduleDieTick`). A coral block has no `onPlace`: the game schedules its
    /// death as a player places it (`CoralBlock.getStateForPlacement`) and as a
    /// neighbour changes.
    ///
    /// `BaseFireBlock.onPlace` also removes fire that cannot stay; this has neither the
    /// state that was there nor the tick's output to do so, so the rules here that place
    /// fire do it after (`place_fire`).
    pub(crate) fn fire_on_place(&mut self, pos: BlockPos, state: BlockState) {
        use BlockClass as C;
        if is_class(state, C::FireBlock) {
            let delay = fire_tick_delay(&mut self.next_draw());
            self.schedule_tick(pos, Ticked::Block(tick_id(state)), delay, 0);
        } else if is_class(state, C::CoralPlantBlock)
            || is_class(state, C::CoralFanBlock)
            || is_class(state, C::CoralWallFanBlock)
        {
            coral_try_schedule_die_tick(&mut RegionView(self), state, pos);
        }
    }

    /// `FireBlock.tick`.
    fn fire_tick(&mut self, pos: BlockPos, state: BlockState, output: &mut TickOutput) {
        let mut random = self.next_draw();
        let delay = fire_tick_delay(&mut random);
        self.schedule_tick(pos, Ticked::Block(tick_id(state)), delay, 0);
        // The game goes on after removing fire that cannot stay, and may set it again
        // with its next age below, which removes it once more as it is placed.
        if !can_survive(state, pos, &mut RegionView(self)) {
            self.remove_block(pos, output);
        }
        let below = relative(pos, Face::Down);
        let infiniburn = in_tag(self.read(below), INFINIBURN);
        let age = i32::from(number(state, "age").unwrap_or(0));
        let next_age = (age + random.next_int(3) as i32 / 2).min(15);
        if age != next_age {
            let aged = set(state, "age", &next_age.to_string());
            self.place_fire(pos, aged, UPDATE_NONE, output);
        }
        if !infiniburn {
            if !valid_fire_location(pos, &mut RegionView(self)) {
                if !sturdy(self.read(below), Face::Up, SupportType::Full) || age > 3 {
                    self.remove_block(pos, output);
                }
                return;
            }
            if age == 15 && random.next_int(4) == 0 && !fire_can_burn(self.read(below)) {
                self.remove_block(pos, output);
                return;
            }
        }
        let burnout = self.increased_fire_burnout(pos);
        let less = if burnout { -50 } else { 0 };
        for (face, chance) in [
            (Face::East, 300),
            (Face::West, 300),
            (Face::Down, 250),
            (Face::Up, 250),
            (Face::North, 300),
            (Face::South, 300),
        ] {
            self.check_burn_out(relative(pos, face), chance + less, &mut random, age, output);
        }
        for x in -1..=1 {
            for z in -1..=1 {
                for y in -1..=4 {
                    if x == 0 && y == 0 && z == 0 {
                        continue;
                    }
                    // Fire reaches up more easily than across or down.
                    let bound = if y > 1 { 100 + (y - 1) * 100 } else { 100 };
                    let target = pos.offset(x, y, z);
                    let odds = self.ignite_odds_at(target);
                    if odds <= 0 {
                        continue;
                    }
                    let mut chance = (odds + 40 + DIFFICULTY * 7) / (age + 30);
                    if burnout {
                        chance /= 2;
                    }
                    if chance > 0 && random.next_int(bound as u32) as i32 <= chance {
                        let spread_age = (age + random.next_int(5) as i32 / 4).min(15);
                        let fire =
                            fire_state_with_age(&mut RegionView(self), target, spread_age as u8);
                        self.place_fire(target, fire, UPDATE_ALL, output);
                    }
                }
            }
        }
    }

    /// `FireBlock.checkBurnOut`: the block at `pos` burns by its odds out of `chance`,
    /// to fire or to nothing.
    fn check_burn_out(
        &mut self,
        pos: BlockPos,
        chance: i32,
        random: &mut Random,
        age: i32,
        output: &mut TickOutput,
    ) {
        let odds = burn_odds(self.read(pos));
        if (random.next_int(chance as u32) as i32) >= odds {
            return;
        }
        if (random.next_int((age + 10) as u32) as i32) < 5 {
            let fire_age = (age + random.next_int(5) as i32 / 4).min(15);
            let fire = fire_state_with_age(&mut RegionView(self), pos, fire_age as u8);
            self.place_fire(pos, fire, UPDATE_ALL, output);
        } else {
            self.remove_block(pos, output);
        }
        // Burnt TNT would be primed here; with no entities it is only gone.
    }

    /// `FireBlock.getIgniteOdds` of a place: nought where it is not empty, else the best
    /// odds of the blocks around it.
    fn ignite_odds_at(&self, pos: BlockPos) -> i32 {
        if !self.block(pos).is_some_and(|state| state.props().is_air()) {
            return 0;
        }
        Face::ALL
            .into_iter()
            .map(|face| ignite_odds(self.read(relative(pos, face))))
            .max()
            .unwrap_or(0)
    }

    /// `LavaFluid.randomTick`: fire on a place above the lava beside a block lava sets
    /// alight, up to two steps away, or on top of such a block beside the lava.
    fn lava_random_tick(&mut self, pos: BlockPos, random: &mut Random, output: &mut TickOutput) {
        let tries = random.next_int(3);
        if tries > 0 {
            let mut at = pos;
            for _ in 0..tries {
                let x = random.next_int(3) as i32 - 1;
                let z = random.next_int(3) as i32 - 1;
                at = at.offset(x, 1, z);
                if !self.chunks.contains_key(&at.chunk()) {
                    return;
                }
                let here = self.read(at);
                if here.props().is_air() {
                    if Face::ALL
                        .into_iter()
                        .any(|face| self.ignited_by_lava(relative(at, face)))
                    {
                        let fire = fire_state(&mut RegionView(self), at);
                        self.place_fire(at, fire, UPDATE_ALL, output);
                        return;
                    }
                } else if in_tag(here, "minecraft:blocks_lava_fire_spread") {
                    return;
                }
            }
        } else {
            for _ in 0..3 {
                let x = random.next_int(3) as i32 - 1;
                let z = random.next_int(3) as i32 - 1;
                let at = pos.offset(x, 0, z);
                if !self.chunks.contains_key(&at.chunk()) {
                    return;
                }
                let above = relative(at, Face::Up);
                if self.read(above).props().is_air() && self.ignited_by_lava(at) {
                    // The game makes the fire of the block it lights, not of the place
                    // above it where the fire goes.
                    let fire = fire_state(&mut RegionView(self), at);
                    self.place_fire(above, fire, UPDATE_ALL, output);
                }
            }
        }
    }

    /// `LavaFluid.isFlammable`: whether lava sets the block at `pos` alight
    /// (`ignitedByLava`); nothing where the region has not loaded it.
    fn ignited_by_lava(&self, pos: BlockPos) -> bool {
        self.block(pos)
            .and_then(|state| state.block())
            .is_some_and(|block| IGNITED_BY_LAVA.binary_search(&block.name).is_ok())
    }

    /// Sets fire with `flags`, and removes it again where it cannot stay, as
    /// `BaseFireBlock.onPlace` does unless the block there was fire already.
    fn place_fire(&mut self, pos: BlockPos, fire: BlockState, flags: u32, output: &mut TickOutput) {
        let Some(old) = self.block(pos) else {
            return;
        };
        if !self.set_block_updated(pos, fire, flags, UPDATE_LIMIT, output) {
            return;
        }
        if is_block(old, fire) || self.block(pos) != Some(fire) {
            return;
        }
        if !can_survive(fire, pos, &mut RegionView(self)) {
            self.remove_block(pos, output);
        }
    }

    /// `Level.removeBlock` of fire or of a block that burnt: what is left is air, since
    /// neither holds water (a waterlogged block does not burn).
    fn remove_block(&mut self, pos: BlockPos, output: &mut TickOutput) {
        self.set_block_updated(pos, blocks::AIR, UPDATE_ALL, UPDATE_LIMIT, output);
    }

    /// `CoralBlock.scanForWater`: water on any side.
    fn water_beside(&self, pos: BlockPos) -> bool {
        Face::ALL
            .into_iter()
            .any(|face| clustine_blocks::data::has_water(self.read(relative(pos, face))))
    }

    /// Whether the biome at `pos` burns fire out faster (the environment attribute
    /// `increased_fire_burnout`).
    fn increased_fire_burnout(&self, pos: BlockPos) -> bool {
        let Some(chunk) = self.chunks.get(&pos.chunk()) else {
            return false;
        };
        let Ok(index) = usize::try_from((pos.y - chunk.min_y()).div_euclid(16)) else {
            return false;
        };
        let Some(section) = chunk.sections().get(index) else {
            return false;
        };
        let (x, z) = pos.in_chunk();
        let biome = section.biome_at(x, pos.y.rem_euclid(16) as usize, z);
        clustine_data::synced_registry("minecraft:worldgen/biome")
            .and_then(|biomes| biomes.entries.get(usize::from(biome.0)))
            .is_some_and(|name| INCREASED_FIRE_BURNOUT.binary_search(name).is_ok())
    }

    /// The block at `pos`, air where the region has not loaded it.
    fn read(&self, pos: BlockPos) -> BlockState {
        self.block(pos).unwrap_or(blocks::AIR)
    }
}

/// `FireBlock.getFireTickDelay`.
fn fire_tick_delay(random: &mut Random) -> i32 {
    30 + random.next_int(10) as i32
}

/// The dead form of a coral, by its name with "dead_" in front, in its default state.
fn dead_coral(state: BlockState) -> BlockState {
    let name = state.block().map_or("", |block| block.name);
    let dead = format!("minecraft:dead_{}", name.trim_start_matches("minecraft:"));
    BlockState::from_name_and_properties(&dead, []).expect("every living coral has a dead form")
}

/// The blocks that lava sets alight (`BlockBehaviour.Properties.ignitedByLava`, and
/// what copies it with `ofLegacyCopy` or `ofFullCopy`), read from 26.3's `Blocks`;
/// sorted, which a test checks.
const IGNITED_BY_LAVA: &[&str] = &[
    "minecraft:acacia_door",
    "minecraft:acacia_fence",
    "minecraft:acacia_fence_gate",
    "minecraft:acacia_hanging_sign",
    "minecraft:acacia_leaves",
    "minecraft:acacia_log",
    "minecraft:acacia_planks",
    "minecraft:acacia_pressure_plate",
    "minecraft:acacia_shelf",
    "minecraft:acacia_sign",
    "minecraft:acacia_slab",
    "minecraft:acacia_stairs",
    "minecraft:acacia_trapdoor",
    "minecraft:acacia_wall_hanging_sign",
    "minecraft:acacia_wall_sign",
    "minecraft:acacia_wood",
    "minecraft:azalea_leaves",
    "minecraft:bamboo",
    "minecraft:bamboo_block",
    "minecraft:bamboo_door",
    "minecraft:bamboo_fence",
    "minecraft:bamboo_fence_gate",
    "minecraft:bamboo_hanging_sign",
    "minecraft:bamboo_mosaic",
    "minecraft:bamboo_mosaic_slab",
    "minecraft:bamboo_mosaic_stairs",
    "minecraft:bamboo_planks",
    "minecraft:bamboo_pressure_plate",
    "minecraft:bamboo_sapling",
    "minecraft:bamboo_shelf",
    "minecraft:bamboo_sign",
    "minecraft:bamboo_slab",
    "minecraft:bamboo_stairs",
    "minecraft:bamboo_trapdoor",
    "minecraft:bamboo_wall_hanging_sign",
    "minecraft:bamboo_wall_sign",
    "minecraft:barrel",
    "minecraft:bee_nest",
    "minecraft:beehive",
    "minecraft:birch_door",
    "minecraft:birch_fence",
    "minecraft:birch_fence_gate",
    "minecraft:birch_hanging_sign",
    "minecraft:birch_leaves",
    "minecraft:birch_log",
    "minecraft:birch_planks",
    "minecraft:birch_pressure_plate",
    "minecraft:birch_shelf",
    "minecraft:birch_sign",
    "minecraft:birch_slab",
    "minecraft:birch_stairs",
    "minecraft:birch_trapdoor",
    "minecraft:birch_wall_hanging_sign",
    "minecraft:birch_wall_sign",
    "minecraft:birch_wood",
    "minecraft:black_banner",
    "minecraft:black_bed",
    "minecraft:black_carpet",
    "minecraft:black_wall_banner",
    "minecraft:black_wool",
    "minecraft:black_wool_slab",
    "minecraft:black_wool_stairs",
    "minecraft:blue_banner",
    "minecraft:blue_bed",
    "minecraft:blue_carpet",
    "minecraft:blue_wall_banner",
    "minecraft:blue_wool",
    "minecraft:blue_wool_slab",
    "minecraft:blue_wool_stairs",
    "minecraft:bookshelf",
    "minecraft:brown_banner",
    "minecraft:brown_bed",
    "minecraft:brown_carpet",
    "minecraft:brown_mushroom_block",
    "minecraft:brown_wall_banner",
    "minecraft:brown_wool",
    "minecraft:brown_wool_slab",
    "minecraft:brown_wool_stairs",
    "minecraft:bush",
    "minecraft:cactus_flower",
    "minecraft:campfire",
    "minecraft:cartography_table",
    "minecraft:cherry_door",
    "minecraft:cherry_fence",
    "minecraft:cherry_fence_gate",
    "minecraft:cherry_hanging_sign",
    "minecraft:cherry_leaves",
    "minecraft:cherry_log",
    "minecraft:cherry_planks",
    "minecraft:cherry_pressure_plate",
    "minecraft:cherry_shelf",
    "minecraft:cherry_sign",
    "minecraft:cherry_slab",
    "minecraft:cherry_stairs",
    "minecraft:cherry_trapdoor",
    "minecraft:cherry_wall_hanging_sign",
    "minecraft:cherry_wall_sign",
    "minecraft:cherry_wood",
    "minecraft:chest",
    "minecraft:chiseled_bookshelf",
    "minecraft:composter",
    "minecraft:crafting_table",
    "minecraft:crimson_shelf",
    "minecraft:cyan_banner",
    "minecraft:cyan_bed",
    "minecraft:cyan_carpet",
    "minecraft:cyan_wall_banner",
    "minecraft:cyan_wool",
    "minecraft:cyan_wool_slab",
    "minecraft:cyan_wool_stairs",
    "minecraft:dark_oak_door",
    "minecraft:dark_oak_fence",
    "minecraft:dark_oak_fence_gate",
    "minecraft:dark_oak_hanging_sign",
    "minecraft:dark_oak_leaves",
    "minecraft:dark_oak_log",
    "minecraft:dark_oak_planks",
    "minecraft:dark_oak_pressure_plate",
    "minecraft:dark_oak_shelf",
    "minecraft:dark_oak_sign",
    "minecraft:dark_oak_slab",
    "minecraft:dark_oak_stairs",
    "minecraft:dark_oak_trapdoor",
    "minecraft:dark_oak_wall_hanging_sign",
    "minecraft:dark_oak_wall_sign",
    "minecraft:dark_oak_wood",
    "minecraft:daylight_detector",
    "minecraft:dead_bush",
    "minecraft:fern",
    "minecraft:firefly_bush",
    "minecraft:fletching_table",
    "minecraft:flowering_azalea_leaves",
    "minecraft:glow_lichen",
    "minecraft:gray_banner",
    "minecraft:gray_bed",
    "minecraft:gray_carpet",
    "minecraft:gray_wall_banner",
    "minecraft:gray_wool",
    "minecraft:gray_wool_slab",
    "minecraft:gray_wool_stairs",
    "minecraft:green_banner",
    "minecraft:green_bed",
    "minecraft:green_carpet",
    "minecraft:green_wall_banner",
    "minecraft:green_wool",
    "minecraft:green_wool_slab",
    "minecraft:green_wool_stairs",
    "minecraft:hanging_roots",
    "minecraft:jukebox",
    "minecraft:jungle_door",
    "minecraft:jungle_fence",
    "minecraft:jungle_fence_gate",
    "minecraft:jungle_hanging_sign",
    "minecraft:jungle_leaves",
    "minecraft:jungle_log",
    "minecraft:jungle_planks",
    "minecraft:jungle_pressure_plate",
    "minecraft:jungle_shelf",
    "minecraft:jungle_sign",
    "minecraft:jungle_slab",
    "minecraft:jungle_stairs",
    "minecraft:jungle_trapdoor",
    "minecraft:jungle_wall_hanging_sign",
    "minecraft:jungle_wall_sign",
    "minecraft:jungle_wood",
    "minecraft:large_fern",
    "minecraft:lectern",
    "minecraft:light_blue_banner",
    "minecraft:light_blue_bed",
    "minecraft:light_blue_carpet",
    "minecraft:light_blue_wall_banner",
    "minecraft:light_blue_wool",
    "minecraft:light_blue_wool_slab",
    "minecraft:light_blue_wool_stairs",
    "minecraft:light_gray_banner",
    "minecraft:light_gray_bed",
    "minecraft:light_gray_carpet",
    "minecraft:light_gray_wall_banner",
    "minecraft:light_gray_wool",
    "minecraft:light_gray_wool_slab",
    "minecraft:light_gray_wool_stairs",
    "minecraft:lilac",
    "minecraft:lime_banner",
    "minecraft:lime_bed",
    "minecraft:lime_carpet",
    "minecraft:lime_wall_banner",
    "minecraft:lime_wool",
    "minecraft:lime_wool_slab",
    "minecraft:lime_wool_stairs",
    "minecraft:loom",
    "minecraft:magenta_banner",
    "minecraft:magenta_bed",
    "minecraft:magenta_carpet",
    "minecraft:magenta_wall_banner",
    "minecraft:magenta_wool",
    "minecraft:magenta_wool_slab",
    "minecraft:magenta_wool_stairs",
    "minecraft:mangrove_door",
    "minecraft:mangrove_fence",
    "minecraft:mangrove_fence_gate",
    "minecraft:mangrove_hanging_sign",
    "minecraft:mangrove_leaves",
    "minecraft:mangrove_log",
    "minecraft:mangrove_planks",
    "minecraft:mangrove_pressure_plate",
    "minecraft:mangrove_roots",
    "minecraft:mangrove_shelf",
    "minecraft:mangrove_sign",
    "minecraft:mangrove_slab",
    "minecraft:mangrove_stairs",
    "minecraft:mangrove_trapdoor",
    "minecraft:mangrove_wall_hanging_sign",
    "minecraft:mangrove_wall_sign",
    "minecraft:mangrove_wood",
    "minecraft:mushroom_stem",
    "minecraft:note_block",
    "minecraft:oak_door",
    "minecraft:oak_fence",
    "minecraft:oak_fence_gate",
    "minecraft:oak_hanging_sign",
    "minecraft:oak_leaves",
    "minecraft:oak_log",
    "minecraft:oak_planks",
    "minecraft:oak_pressure_plate",
    "minecraft:oak_shelf",
    "minecraft:oak_sign",
    "minecraft:oak_slab",
    "minecraft:oak_stairs",
    "minecraft:oak_trapdoor",
    "minecraft:oak_wall_hanging_sign",
    "minecraft:oak_wall_sign",
    "minecraft:oak_wood",
    "minecraft:orange_banner",
    "minecraft:orange_bed",
    "minecraft:orange_carpet",
    "minecraft:orange_poplar_leaves",
    "minecraft:orange_wall_banner",
    "minecraft:orange_wool",
    "minecraft:orange_wool_slab",
    "minecraft:orange_wool_stairs",
    "minecraft:pale_hanging_moss",
    "minecraft:pale_moss_block",
    "minecraft:pale_moss_carpet",
    "minecraft:pale_oak_door",
    "minecraft:pale_oak_fence",
    "minecraft:pale_oak_fence_gate",
    "minecraft:pale_oak_hanging_sign",
    "minecraft:pale_oak_leaves",
    "minecraft:pale_oak_log",
    "minecraft:pale_oak_planks",
    "minecraft:pale_oak_pressure_plate",
    "minecraft:pale_oak_shelf",
    "minecraft:pale_oak_sign",
    "minecraft:pale_oak_slab",
    "minecraft:pale_oak_stairs",
    "minecraft:pale_oak_trapdoor",
    "minecraft:pale_oak_wall_hanging_sign",
    "minecraft:pale_oak_wall_sign",
    "minecraft:pale_oak_wood",
    "minecraft:peony",
    "minecraft:pink_banner",
    "minecraft:pink_bed",
    "minecraft:pink_carpet",
    "minecraft:pink_wall_banner",
    "minecraft:pink_wool",
    "minecraft:pink_wool_slab",
    "minecraft:pink_wool_stairs",
    "minecraft:pitcher_plant",
    "minecraft:poplar_door",
    "minecraft:poplar_fence",
    "minecraft:poplar_fence_gate",
    "minecraft:poplar_hanging_sign",
    "minecraft:poplar_log",
    "minecraft:poplar_planks",
    "minecraft:poplar_pressure_plate",
    "minecraft:poplar_shelf",
    "minecraft:poplar_sign",
    "minecraft:poplar_slab",
    "minecraft:poplar_stairs",
    "minecraft:poplar_trapdoor",
    "minecraft:poplar_wall_hanging_sign",
    "minecraft:poplar_wall_sign",
    "minecraft:poplar_wood",
    "minecraft:purple_banner",
    "minecraft:purple_bed",
    "minecraft:purple_carpet",
    "minecraft:purple_wall_banner",
    "minecraft:purple_wool",
    "minecraft:purple_wool_slab",
    "minecraft:purple_wool_stairs",
    "minecraft:red_banner",
    "minecraft:red_bed",
    "minecraft:red_carpet",
    "minecraft:red_mushroom_block",
    "minecraft:red_poplar_leaves",
    "minecraft:red_shrub",
    "minecraft:red_wall_banner",
    "minecraft:red_wool",
    "minecraft:red_wool_slab",
    "minecraft:red_wool_stairs",
    "minecraft:resin_clump",
    "minecraft:rose_bush",
    "minecraft:short_dry_grass",
    "minecraft:short_grass",
    "minecraft:smithing_table",
    "minecraft:soul_campfire",
    "minecraft:spruce_door",
    "minecraft:spruce_fence",
    "minecraft:spruce_fence_gate",
    "minecraft:spruce_hanging_sign",
    "minecraft:spruce_leaves",
    "minecraft:spruce_log",
    "minecraft:spruce_planks",
    "minecraft:spruce_pressure_plate",
    "minecraft:spruce_shelf",
    "minecraft:spruce_sign",
    "minecraft:spruce_slab",
    "minecraft:spruce_stairs",
    "minecraft:spruce_trapdoor",
    "minecraft:spruce_wall_hanging_sign",
    "minecraft:spruce_wall_sign",
    "minecraft:spruce_wood",
    "minecraft:straw_bed",
    "minecraft:stripped_acacia_log",
    "minecraft:stripped_acacia_wood",
    "minecraft:stripped_bamboo_block",
    "minecraft:stripped_birch_log",
    "minecraft:stripped_birch_wood",
    "minecraft:stripped_cherry_log",
    "minecraft:stripped_cherry_wood",
    "minecraft:stripped_dark_oak_log",
    "minecraft:stripped_dark_oak_wood",
    "minecraft:stripped_jungle_log",
    "minecraft:stripped_jungle_wood",
    "minecraft:stripped_mangrove_log",
    "minecraft:stripped_mangrove_wood",
    "minecraft:stripped_oak_log",
    "minecraft:stripped_oak_wood",
    "minecraft:stripped_pale_oak_log",
    "minecraft:stripped_pale_oak_wood",
    "minecraft:stripped_poplar_log",
    "minecraft:stripped_poplar_wood",
    "minecraft:stripped_spruce_log",
    "minecraft:stripped_spruce_wood",
    "minecraft:sunflower",
    "minecraft:tall_dry_grass",
    "minecraft:tall_grass",
    "minecraft:tnt",
    "minecraft:trapped_chest",
    "minecraft:vine",
    "minecraft:warped_shelf",
    "minecraft:white_banner",
    "minecraft:white_bed",
    "minecraft:white_carpet",
    "minecraft:white_wall_banner",
    "minecraft:white_wool",
    "minecraft:white_wool_slab",
    "minecraft:white_wool_stairs",
    "minecraft:yellow_banner",
    "minecraft:yellow_bed",
    "minecraft:yellow_carpet",
    "minecraft:yellow_poplar_leaves",
    "minecraft:yellow_wall_banner",
    "minecraft:yellow_wool",
    "minecraft:yellow_wool_slab",
    "minecraft:yellow_wool_stairs",
];

#[cfg(test)]
mod tests {
    use clustine_data::BLOCKS;
    use clustine_world::{Biome, Chunk, ChunkPos, EntityIds, Vec3};

    use super::*;
    use crate::api::{HOTBAR_SLOTS, TickInputs, Ticket};
    use crate::region::{Holdings, RegionConfig};

    /// The floor's top: stone at y = -61, so a test's blocks stand at -60.
    const FLOOR: i32 = -61;

    fn at(x: i32, y: i32, z: i32) -> BlockPos {
        BlockPos::new(x, y, z)
    }

    fn state(text: &str) -> BlockState {
        BlockState::parse(text).unwrap_or_else(|| panic!("{text} is a block state"))
    }

    /// A chunk of the overworld with a stone floor and `blocks` of it set.
    fn floor_of(position: ChunkPos, blocks: &[(BlockPos, BlockState)]) -> Chunk {
        let overworld = clustine_data::DIMENSION_TYPES
            .iter()
            .find(|dimension| dimension.name == "minecraft:overworld")
            .expect("the game has the overworld");
        let mut chunk = Chunk::empty(overworld, Biome(0));
        for z in 0..16 {
            for x in 0..16 {
                chunk.set(x, FLOOR, z, blocks::STONE);
            }
        }
        for (at, state) in blocks.iter().filter(|(at, _)| at.chunk() == position) {
            let (x, z) = at.in_chunk();
            chunk.set(x, at.y, z, *state);
        }
        chunk
    }

    /// A region of the world `seed` that holds the 3×3 chunks around the origin and has
    /// them loaded, with `blocks` set, so that the origin's chunk ticks.
    fn ticking_with(seed: u64, blocks: &[(BlockPos, BlockState)]) -> Region {
        let around: Vec<ChunkPos> = (-1..=1)
            .flat_map(|x| (-1..=1).map(move |z| ChunkPos::new(x, z)))
            .collect();
        let config = RegionConfig {
            spawn: Vec3::new(0.5, -60.0, 0.5),
            starting_hotbar: [None; HOTBAR_SLOTS],
            return_after: u64::MAX,
            lowest_y: -64,
            seed,
        };
        let holdings = Holdings {
            held: around.clone(),
            pinned: Vec::new(),
        };
        let mut region = Region::new(config, EntityIds::block(0).expect("ids"), holdings);
        region.tick(&TickInputs {
            tickets_added: around
                .iter()
                .map(|chunk| (*chunk, Ticket::Viewer))
                .collect(),
            ..TickInputs::default()
        });
        region.tick(&TickInputs {
            chunks_loaded: around
                .iter()
                .map(|chunk| (*chunk, floor_of(*chunk, blocks)))
                .collect(),
            ..TickInputs::default()
        });
        assert!(
            region.ticking_chunks().contains(&ChunkPos::new(0, 0)),
            "the origin's chunk ticks"
        );
        region
    }

    /// Sets `state` at `pos` as a region's own change does, with every update.
    fn place(region: &mut Region, pos: BlockPos, state: BlockState) {
        region.set_block(pos, state, &mut TickOutput::default());
    }

    fn idle(region: &mut Region) {
        region.tick(&TickInputs::default());
    }

    fn is_fire(state: Option<BlockState>) -> bool {
        state.is_some_and(|state| is_block(state, blocks::FIRE))
    }

    #[test]
    fn the_tables_are_sorted_and_name_blocks_of_the_game() {
        let names = |table: &[&str]| {
            assert!(table.windows(2).all(|pair| pair[0] < pair[1]));
            for name in table {
                assert!(
                    BLOCKS.iter().any(|block| block.name == *name),
                    "{name} is a block"
                );
            }
        };
        names(IGNITED_BY_LAVA);
        let flammable: Vec<&str> = clustine_blocks::survive::FLAMMABLE
            .iter()
            .map(|(name, _, _)| *name)
            .collect();
        names(&flammable);
        let biomes = INCREASED_FIRE_BURNOUT;
        assert!(biomes.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(
            clustine_data::synced_registry("minecraft:worldgen/biome").is_some_and(|biomes| {
                INCREASED_FIRE_BURNOUT
                    .iter()
                    .all(|name| biomes.entries.contains(name))
            })
        );
    }

    #[test]
    fn the_odds_of_planks_logs_and_wool_are_the_games_and_water_puts_them_out() {
        let odds = |text: &str| (ignite_odds(state(text)), burn_odds(state(text)));
        assert_eq!(odds("minecraft:oak_planks"), (5, 20));
        assert_eq!(odds("minecraft:oak_log"), (5, 5));
        assert_eq!(odds("minecraft:red_wool"), (30, 60));
        assert_eq!(odds("minecraft:red_carpet"), (60, 20));
        assert_eq!(odds("minecraft:tnt"), (15, 100));
        assert_eq!(odds("minecraft:stone"), (0, 0));
        assert_eq!(odds("minecraft:oak_slab[waterlogged=true]"), (0, 0));
        assert_eq!(odds("minecraft:oak_slab[waterlogged=false]"), (5, 20));
    }

    #[test]
    fn fire_on_stone_burns_out_and_on_netherrack_stays() {
        let stone_fire = at(0, -60, 0);
        let netherrack = at(4, -61, 4);
        let lasting_fire = at(4, -60, 4);
        let mut region = ticking_with(1, &[(netherrack, blocks::NETHERRACK)]);
        place(&mut region, stone_fire, blocks::FIRE);
        place(&mut region, lasting_fire, blocks::FIRE);
        assert!(is_fire(region.block(stone_fire)));
        // Fire ages a step every three of its ticks or so, and goes past age 3: that
        // takes some 400 ticks, and the chance it takes 3,000 is nil.
        let mut gone = None;
        for tick in 0..3_000 {
            idle(&mut region);
            if gone.is_none() && region.block(stone_fire) == Some(blocks::AIR) {
                gone = Some(tick);
            }
            assert!(is_fire(region.block(lasting_fire)), "tick {tick}");
        }
        let gone = gone.expect("fire on stone burns out");
        assert!(
            gone > 100,
            "it burnt out after {gone} ticks, before its age passed 3"
        );
        assert_eq!(region.block(stone_fire), Some(blocks::AIR));
        let age = number(region.block(lasting_fire).expect("loaded"), "age");
        assert_eq!(age, Some(15), "fire on netherrack ages to the full");
    }

    #[test]
    fn fire_spreads_to_planks_and_logs_beside_it_and_burns_them() {
        let fire = at(0, -60, 0);
        let wood = [
            (at(1, -60, 0), blocks::OAK_PLANKS),
            (at(2, -60, 0), blocks::OAK_PLANKS),
            (at(-1, -60, 0), blocks::OAK_LOG),
            (at(-2, -60, 0), blocks::OAK_LOG),
            (at(0, -60, 1), blocks::OAK_PLANKS),
            (at(0, -60, -1), blocks::OAK_LOG),
        ];
        let mut spread = false;
        for seed in 0..4 {
            let mut region = ticking_with(seed, &wood);
            place(&mut region, fire, blocks::FIRE);
            for _ in 0..4_000 {
                idle(&mut region);
            }
            // Wood that burnt is fire or nothing now, and fire went to places it did
            // not start at.
            let burnt = wood
                .iter()
                .filter(|(pos, state)| region.block(*pos) != Some(*state))
                .count();
            let fires = (-3..=3)
                .flat_map(|x| (-60..=-56).flat_map(move |y| (-3..=3).map(move |z| at(x, y, z))))
                .filter(|pos| *pos != fire)
                .filter(|pos| is_fire(region.block(*pos)))
                .count();
            let air_or_fire = wood.iter().all(|(pos, state)| {
                let now = region.block(*pos).expect("loaded");
                now == *state || now == blocks::AIR || is_fire(Some(now))
            });
            assert!(air_or_fire, "seed {seed}: burnt wood is air or fire");
            assert!(burnt > 0, "seed {seed}: fire burns the wood beside it");
            spread |= fires > 0;
            // Nothing burns that does not burn: the stone floor is all there.
            for x in -3..=3 {
                for z in -3..=3 {
                    assert_eq!(region.block(at(x, FLOOR, z)), Some(blocks::STONE));
                }
            }
        }
        assert!(spread, "fire spreads");
    }

    #[test]
    fn fire_goes_when_its_ground_is_broken() {
        let ground = at(0, -60, 0);
        let fire = at(0, -59, 0);
        let mut region = ticking_with(2, &[(ground, blocks::OAK_PLANKS)]);
        place(&mut region, fire, blocks::FIRE);
        assert!(is_fire(region.block(fire)));
        place(&mut region, ground, blocks::AIR);
        assert_eq!(region.block(fire), Some(blocks::AIR));
    }

    #[test]
    fn fire_that_cannot_stay_where_it_spreads_is_removed_as_it_is_placed() {
        // A block of planks up in the air that burns to fire leaves fire with nothing
        // below and nothing that burns beside it, which `onPlace` takes away.
        let up = at(3, -50, 3);
        let mut region = ticking_with(3, &[(up, blocks::OAK_PLANKS)]);
        let mut output = TickOutput::default();
        region.place_fire(up, blocks::FIRE, UPDATE_ALL, &mut output);
        assert_eq!(region.block(up), Some(blocks::AIR));
    }

    #[test]
    fn lava_sets_fire_to_planks_beside_it_and_above_it() {
        assert!(blocks::LAVA.props().is_randomly_ticking());
        let lava = at(0, -60, 0);
        for (planks, name) in [(at(1, -60, 0), "beside"), (at(0, -58, 0), "above")] {
            let mut region = ticking_with(4, &[(lava, blocks::LAVA), (planks, blocks::OAK_PLANKS)]);
            let mut lit = 0;
            for draw in 0..300 {
                let mut random = Random::of_section(draw, ChunkPos::new(0, 0), 0, 0);
                let mut output = TickOutput::default();
                assert!(region.fire_random_tick(lava, blocks::LAVA, &mut random, &mut output));
                // Every fire lava sets is beside the planks; take it away again.
                for x in -2..=2 {
                    for y in -60..=-56 {
                        for z in -2..=2 {
                            let pos = at(x, y, z);
                            if is_fire(region.block(pos)) {
                                let beside = Face::ALL
                                    .into_iter()
                                    .any(|face| relative(pos, face) == planks);
                                assert!(beside, "{name}: fire at {pos:?}");
                                lit += 1;
                                region.chunks.get_mut(&pos.chunk()).expect("loaded").set(
                                    pos.in_chunk().0,
                                    pos.y,
                                    pos.in_chunk().1,
                                    blocks::AIR,
                                );
                            }
                        }
                    }
                }
            }
            assert!(lit >= 5, "{name}: lava set fire {lit} times in 300 draws");
        }
    }

    #[test]
    fn lava_sets_no_fire_where_nothing_burns() {
        let lava = at(0, -60, 0);
        let mut region = ticking_with(5, &[(lava, blocks::LAVA), (at(1, -60, 0), blocks::STONE)]);
        for draw in 0..300 {
            let mut random = Random::of_section(draw, ChunkPos::new(0, 0), 0, 0);
            region.fire_random_tick(lava, blocks::LAVA, &mut random, &mut TickOutput::default());
        }
        for x in -3..=3 {
            for y in -60..=-56 {
                for z in -3..=3 {
                    assert!(!is_fire(region.block(at(x, y, z))));
                }
            }
        }
    }

    /// How many ticks after it is placed the coral at `pos` turns to `dead`, if within
    /// `limit`.
    fn dies_after(region: &mut Region, pos: BlockPos, dead: BlockState, limit: u32) -> Option<u32> {
        for tick in 1..=limit {
            idle(region);
            if region.block(pos) == Some(dead) {
                return Some(tick);
            }
        }
        None
    }

    #[test]
    fn coral_out_of_water_dies_after_60_to_99_ticks_and_in_water_does_not() {
        let mut seen = std::collections::BTreeSet::new();
        for seed in 0..12 {
            let mut region = ticking_with(seed, &[]);
            let dry = at(2, -60, 2);
            let wet = at(6, -60, 6);
            place(
                &mut region,
                dry,
                state("minecraft:tube_coral[waterlogged=false]"),
            );
            place(
                &mut region,
                wet,
                state("minecraft:brain_coral[waterlogged=true]"),
            );
            let dead = state("minecraft:dead_tube_coral[waterlogged=false]");
            let after = dies_after(&mut region, dry, dead, 150).expect("dry coral dies");
            assert!(
                (60..=99).contains(&after),
                "seed {seed}: died after {after}"
            );
            seen.insert(after);
            for _ in 0..200 {
                idle(&mut region);
            }
            assert_eq!(
                region.block(wet),
                Some(state("minecraft:brain_coral[waterlogged=true]"))
            );
        }
        assert!(seen.len() > 3, "the time to die is drawn: {seen:?}");
    }

    #[test]
    fn a_dead_coral_fan_keeps_its_facing_and_is_not_waterlogged() {
        let wall = at(3, -60, 3);
        let fan = at(4, -60, 3);
        let mut region = ticking_with(7, &[(wall, blocks::STONE)]);
        place(
            &mut region,
            fan,
            state("minecraft:fire_coral_wall_fan[facing=east,waterlogged=false]"),
        );
        let dead = state("minecraft:dead_fire_coral_wall_fan[facing=east,waterlogged=false]");
        assert!(dies_after(&mut region, fan, dead, 150).is_some());
        // A dead fan stays where it was and keeps to its wall.
        for _ in 0..50 {
            idle(&mut region);
        }
        assert_eq!(region.block(fan), Some(dead));
        place(&mut region, wall, blocks::AIR);
        assert_eq!(region.block(fan), Some(blocks::AIR), "its wall is gone");
    }

    #[test]
    fn a_coral_block_dies_once_a_neighbour_changes_out_of_water_and_not_in_it() {
        let dry = at(2, -60, 2);
        let wet = at(8, -60, 8);
        let water = at(8, -60, 9);
        let mut region = ticking_with(8, &[(water, blocks::WATER)]);
        place(&mut region, dry, blocks::HORN_CORAL_BLOCK);
        place(&mut region, wet, blocks::HORN_CORAL_BLOCK);
        // It has no `onPlace`: placed by a rule, it waits for a neighbour.
        for _ in 0..150 {
            idle(&mut region);
        }
        assert_eq!(region.block(dry), Some(blocks::HORN_CORAL_BLOCK));
        place(&mut region, at(2, -59, 2), blocks::STONE);
        place(&mut region, at(8, -59, 8), blocks::STONE);
        assert!(dies_after(&mut region, dry, blocks::DEAD_HORN_CORAL_BLOCK, 150).is_some());
        assert_eq!(region.block(wet), Some(blocks::HORN_CORAL_BLOCK));
    }
}
