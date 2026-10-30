//! The random ticks of what is not a plant and needs no light (step B7): copper that
//! weathers, budding amethyst, pointed dripstone and the sulfur spike, redstone ore,
//! nylium, farmland's moisture and dried ghasts.
//!
//! Each rule draws from the stream what the game's draws from its random, in the same
//! order and as often, so that its chances are the game's. What is left out, each said
//! again where it would be:
//!
//! - an eyeblossom's opening and closing, in the ground or in a pot (the environment
//!   attribute `eyeblossom_open`); a turtle egg's hatching is `region/turtle_eggs.rs`';
//! - rain on farmland: there is no weather, so `isRainingAt` is false;
//! - sounds, particles and game events (a turtle egg's crack, the drip of dripstone,
//!   `GameEvent.BLOCK_CHANGE`), which a region does not report;
//! - `Block.pushEntitiesUp` where farmland becomes dirt or mud becomes clay: a player
//!   standing in the block is not lifted out of it;
//! - dripstone filling a cauldron below it: the random tick only schedules the
//!   cauldron's tick (`AbstractCauldronBlock.tick`, `receiveStalactiteDrip`), which
//!   fills it, and the region has neither that tick nor which cauldrons take a drip
//!   (`canReceiveStalactiteDrip`).

use std::cmp::Ordering;

use clustine_blocks::data::{
    MAINTAINS_FARMLAND, face_name, has_water, in_tag, is_block, is_class, number, relative,
    same_block, set, set_flag, tick_id,
};
use clustine_blocks::fluid::{self, FluidState};
use clustine_data::{BLOCKS, BlockClass, BlockState, Face, FaceShape, SupportType, blocks};
use clustine_world::{BlockPos, Ticked};

use super::Random;
use crate::api::TickOutput;
use crate::region::Region;
use crate::region::updates::{UPDATE_CLIENTS, UPDATE_LIMIT};

/// `ChangeOverTimeBlock.changeOverTime`: how likely a random tick is to look at all.
const WEATHERING_CHANCE: f32 = 0.056_888_89;
/// `ChangeOverTimeBlock.SCAN_DISTANCE`: how far, counted along the axes, a block of
/// copper looks for others.
const WEATHERING_SCAN: i32 = 4;

/// `SpeleothemBlock.GROWTH_PROBABILITY_PER_RANDOM_TICK`.
const SPELEOTHEM_GROWTH_CHANCE: f32 = 0.011_377_778;
/// `SpeleothemBlock.MAX_STALAGMITE_SEARCH_RANGE_WHEN_GROWING`.
const STALAGMITE_SEARCH: i32 = 10;
/// `PointedDripstoneBlock.WATER_TRANSFER_PROBABILITY_PER_RANDOM_TICK`.
const WATER_TRANSFER_CHANCE: f32 = 0.175_781_25;
/// `PointedDripstoneBlock.LAVA_TRANSFER_PROBABILITY_PER_RANDOM_TICK`.
const LAVA_TRANSFER_CHANCE: f32 = 0.058_593_75;
/// `PointedDripstoneBlock.MAX_SEARCH_LENGTH_WHEN_CHECKING_DRIP_TYPE`.
const DRIP_SEARCH: i32 = 11;

/// `DriedGhastBlock`'s delay before it next takes up water.
const GHAST_HYDRATION_DELAY: i32 = 5000;

/// The game's `BlockTags.SPELEOTHEMS`: pointed dripstone and the sulfur spike.
const SPELEOTHEMS: &str = "minecraft:speleothems";

impl Region {
    /// The random tick of `state` at `pos` if it is of this file's blocks; whether it
    /// was.
    pub(super) fn mineral_random_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) -> bool {
        let Some(info) = state.block_info() else {
            return false;
        };
        match info.class {
            class if is_weathering(class) => self.weathering_tick(pos, state, random, output),
            BlockClass::BuddingAmethystBlock => self.budding_amethyst_tick(pos, random, output),
            BlockClass::PointedDripstoneBlock => {
                // `PointedDripstoneBlock.randomTick`: the drip first, with a draw of its
                // own, then the growth every speleothem has.
                let drip = random.next_float();
                self.maybe_transfer_fluid(pos, state, drip, output);
                self.speleothem_tick(pos, state, random, output);
            }
            BlockClass::SulfurSpikeBlock => self.speleothem_tick(pos, state, random, output),
            BlockClass::RedStoneOreBlock => {
                if state.property("lit") == Some("true") {
                    self.set_block(pos, set_flag(state, "lit", false), output);
                }
            }
            BlockClass::NyliumBlock => {
                if !self.can_be_nylium(pos, state) {
                    self.set_block(pos, blocks::NETHERRACK, output);
                }
            }
            BlockClass::FarmlandBlock => self.farmland_tick(pos, state, output),
            BlockClass::DriedGhastBlock => {
                // The ghast's hydration is its scheduled tick, which this one only makes
                // sure is coming; a second is not scheduled while one waits.
                let wet = state.property("waterlogged") == Some("true");
                if wet || number(state, "hydration").unwrap_or(0) > 0 {
                    let block = Ticked::Block(tick_id(state));
                    self.schedule_tick(pos, block, GHAST_HYDRATION_DELAY, 0);
                }
            }
            // A turtle egg cracks and hatches by a chance that the time of day sets
            // (`region/turtle_eggs.rs`); eyeblossoms, planted or potted, open and close
            // as the time of day says, which is not ported.
            BlockClass::TurtleEggBlock => self.turtle_egg_tick(pos, state, random, output),
            BlockClass::EyeblossomBlock => {}
            BlockClass::FlowerPotBlock => {}
            _ => return false,
        }
        true
    }

    /// `ChangeOverTimeBlock.changeOverTime` of the weathering copper blocks, with what
    /// each class asks first: a door weathers at its lower half, which the upper
    /// follows; a double chest at its left half. A chest's weathering waits while
    /// someone has it open, which no one can in a region yet.
    fn weathering_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        if is_class(state, BlockClass::WeatheringCopperDoorBlock)
            && state.property("half") != Some("lower")
        {
            return;
        }
        if is_class(state, BlockClass::WeatheringCopperChestBlock)
            && state.property("type") == Some("right")
        {
            return;
        }
        if random.next_float() >= WEATHERING_CHANCE {
            return;
        }
        if let Some(next) = self.next_weathered(pos, state, random) {
            self.set_block(pos, next, output);
        }
    }

    /// `ChangeOverTimeBlock.getNextState`: a block of copper with a less weathered one
    /// within four blocks stays as it is; otherwise it weathers by a chance that falls
    /// with the copper as weathered as itself nearby and rises with the copper more
    /// weathered.
    fn next_weathered(
        &self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
    ) -> Option<BlockState> {
        let age = weathering_stage(state)?.1;
        let mut same = 0_u32;
        let mut more = 0_u32;
        for dx in -WEATHERING_SCAN..=WEATHERING_SCAN {
            for dy in -WEATHERING_SCAN..=WEATHERING_SCAN {
                for dz in -WEATHERING_SCAN..=WEATHERING_SCAN {
                    let distance = dx.abs() + dy.abs() + dz.abs();
                    if distance == 0 || distance > WEATHERING_SCAN {
                        continue;
                    }
                    let Some((_, other)) = self
                        .block(pos.offset(dx, dy, dz))
                        .and_then(weathering_stage)
                    else {
                        continue;
                    };
                    match other.cmp(&age) {
                        Ordering::Less => return None,
                        Ordering::Greater => more += 1,
                        Ordering::Equal => same += 1,
                    }
                }
            }
        }
        let share = (more + 1) as f32 / (more + same + 1) as f32;
        // `WeatheringCopper.getChanceModifier`: unweathered copper is slower to start.
        let modifier = if age == 0 { 0.75 } else { 1.0 };
        if random.next_float() < share * share * modifier {
            next_weathering(state)
        } else {
            None
        }
    }

    /// `BuddingAmethystBlock.randomTick`: one tick in five, a bud grows on a side drawn
    /// at random, into air or a full block of water, or the bud already there that
    /// faces away from this block grows a stage.
    fn budding_amethyst_tick(
        &mut self,
        pos: BlockPos,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        if random.next_int(5) != 0 {
            return;
        }
        // `Direction.values()`, in the game's order.
        let direction = Face::ALL[random.next_int(6) as usize];
        let at = relative(pos, direction);
        let Some(there) = self.block(at) else {
            return;
        };
        let facing_out = there.property("facing") == Some(face_name(direction));
        let grown = if can_cluster_grow_at(there) {
            blocks::SMALL_AMETHYST_BUD
        } else if is_block(there, blocks::SMALL_AMETHYST_BUD) && facing_out {
            blocks::MEDIUM_AMETHYST_BUD
        } else if is_block(there, blocks::MEDIUM_AMETHYST_BUD) && facing_out {
            blocks::LARGE_AMETHYST_BUD
        } else if is_block(there, blocks::LARGE_AMETHYST_BUD) && facing_out {
            blocks::AMETHYST_CLUSTER
        } else {
            return;
        };
        let watery = FluidState::of(there).fluid == fluid::WATER;
        let grown = set(grown, "facing", face_name(direction));
        self.set_block(at, set_flag(grown, "waterlogged", watery), output);
    }

    /// `NyliumBlock.canBeNylium`: what is above lets some light through
    /// (`LightEngine.getLightDampeningInto` below 15). That is a matter of the blocks'
    /// shapes and how much light they lose, not of how much light there is.
    fn can_be_nylium(&self, pos: BlockPos, state: BlockState) -> bool {
        let above = self.block_or_void(relative(pos, Face::Up));
        !shape_occludes(state, above, Face::Up) && above.props().light_dampening() < 15
    }

    /// `FarmlandBlock.randomTick`: water within four blocks across and up to one above
    /// wets farmland at once; without it farmland dries a step at a time, and dry
    /// farmland with nothing on it that keeps it becomes dirt. Moisture changes tell
    /// only the clients (flags 2).
    fn farmland_tick(&mut self, pos: BlockPos, state: BlockState, output: &mut TickOutput) {
        let moisture = number(state, "moisture").unwrap_or(0);
        // `isRainingAt` is false: there is no weather.
        if self.is_near_water(pos) {
            if moisture < 7 {
                let wet = set(state, "moisture", "7");
                self.set_block_updated(pos, wet, UPDATE_CLIENTS, UPDATE_LIMIT, output);
            }
        } else if moisture > 0 {
            let drier = set(state, "moisture", &(moisture - 1).to_string());
            self.set_block_updated(pos, drier, UPDATE_CLIENTS, UPDATE_LIMIT, output);
        } else {
            let above = self.block(relative(pos, Face::Up)).unwrap_or(blocks::AIR);
            if !MAINTAINS_FARMLAND.contains(above) {
                // `turnToBaseBlock`: farmland's base block is dirt, as its scheduled tick
                // has it.
                self.set_block(pos, blocks::DIRT, output);
            }
        }
    }

    /// `FarmlandBlock.isNearWater`: water of any kind from four blocks away on either
    /// side, from the farmland's level to one block above.
    fn is_near_water(&self, pos: BlockPos) -> bool {
        (-4..=4).any(|dx| {
            (0..=1).any(|dy| {
                (-4..=4).any(|dz| self.block(pos.offset(dx, dy, dz)).is_some_and(has_water))
            })
        })
    }

    /// `SpeleothemBlock.randomTick`: now and then a stalactite under the block it grows
    /// on lengthens at its tip, or the stalagmite below it grows towards it.
    fn speleothem_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        if random.next_float() < SPELEOTHEM_GROWTH_CHANCE && self.is_stalactite_start(pos, state) {
            self.grow_speleothem(pos, state, random, output);
        }
    }

    /// The block at `pos`, and outside the world the game's void air.
    fn block_or_void(&self, pos: BlockPos) -> BlockState {
        self.block(pos).unwrap_or(blocks::VOID_AIR)
    }

    /// `SpeleothemBlock.isStalactiteStartPos`: the top of a stalactite.
    fn is_stalactite_start(&self, pos: BlockPos, state: BlockState) -> bool {
        is_speleothem_towards(state, Face::Down)
            && !same_block(self.block_or_void(relative(pos, Face::Up)), state)
    }

    /// `SpeleothemBlock.growStalactiteOrStalagmiteIfPossible`.
    fn grow_speleothem(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        if !self.can_speleothem_grow(pos, state) {
            return;
        }
        let Some(tip) = self.find_tip(pos, state, max_growth_length(state), false) else {
            return;
        };
        let tip_state = self.block_or_void(tip);
        if !is_free_hanging_stalactite(tip_state) || !self.can_tip_grow(tip, tip_state, state) {
            return;
        }
        // `nextBoolean`.
        if random.next_int(2) == 1 {
            self.grow_speleothem_at(tip, Face::Down, state, output);
        } else {
            self.grow_stalagmite_below(tip, state, output);
        }
    }

    /// `canGrow`: the block a speleothem grows from is above it, and for pointed
    /// dripstone still water above that.
    fn can_speleothem_grow(&self, pos: BlockPos, state: BlockState) -> bool {
        let above = self.block_or_void(relative(pos, Face::Up));
        if !is_block(above, block_to_grow_on(state)) {
            return false;
        }
        if is_class(state, BlockClass::PointedDripstoneBlock) {
            let water = FluidState::of(self.block_or_void(pos.offset(0, 2, 0)));
            return water.fluid == fluid::WATER && water.is_source();
        }
        true
    }

    /// `SpeleothemBlock.findTip`: the tip of the speleothem whose part `state` is at
    /// `pos`, looking along it at most `max - 1` blocks.
    fn find_tip(
        &self,
        pos: BlockPos,
        state: BlockState,
        max: i32,
        allow_merged: bool,
    ) -> Option<BlockPos> {
        if is_tip(state, allow_merged) {
            return Some(pos);
        }
        let direction = tip_direction(state)?;
        self.find_vertical(
            pos,
            direction,
            |other| same_block(other, state) && tip_direction(other) == Some(direction),
            |other| is_tip(other, allow_merged),
            max,
        )
    }

    /// `SpeleothemBlock.findBlockVertical`: the first of at most `max - 1` blocks from
    /// `pos` towards `direction` that is `found`, going on while each is `passed` and
    /// within the world.
    fn find_vertical(
        &self,
        pos: BlockPos,
        direction: Face,
        passed: impl Fn(BlockState) -> bool,
        found: impl Fn(BlockState) -> bool,
        max: i32,
    ) -> Option<BlockPos> {
        let mut at = pos;
        for _ in 1..max {
            at = relative(at, direction);
            let here = self.block(at);
            let state = here.unwrap_or(blocks::VOID_AIR);
            if found(state) {
                return Some(at);
            }
            // The column is loaded, so only what is outside the world has no block.
            if here.is_none() || !passed(state) {
                return None;
            }
        }
        None
    }

    /// `SpeleothemBlock.canTipGrow`: past the tip is air, or a tip of the same block
    /// coming the other way to merge with; not a fluid.
    fn can_tip_grow(&self, tip: BlockPos, tip_state: BlockState, own: BlockState) -> bool {
        let Some(direction) = tip_direction(tip_state) else {
            return false;
        };
        let next = self.block_or_void(relative(tip, direction));
        if !FluidState::of(next).is_empty() {
            return false;
        }
        if next.props().is_air() {
            return true;
        }
        is_unmerged_tip_towards(next, direction.opposite(), own)
    }

    /// `SpeleothemBlock.grow`: a new tip next to `pos` towards `direction`, or the two
    /// tips merged where it meets one coming the other way (`createMergedTips`).
    fn grow_speleothem_at(
        &mut self,
        pos: BlockPos,
        direction: Face,
        own: BlockState,
        output: &mut TickOutput,
    ) {
        let target = relative(pos, direction);
        let there = self.block_or_void(target);
        if is_unmerged_tip_towards(there, direction.opposite(), own) {
            let (stalactite, stalagmite) = if tip_direction(there) == Some(Face::Up) {
                (relative(target, Face::Up), target)
            } else {
                (target, relative(target, Face::Down))
            };
            self.create_speleothem(stalactite, Face::Down, "tip_merge", own, output);
            self.create_speleothem(stalagmite, Face::Up, "tip_merge", own, output);
        } else if there.props().is_air() || is_block(there, blocks::WATER) {
            self.create_speleothem(target, direction, "tip", own, output);
        }
    }

    /// `SpeleothemBlock.createSpeleothem`: a part of `own`'s block, waterlogged where
    /// it takes the place of still water.
    fn create_speleothem(
        &mut self,
        pos: BlockPos,
        direction: Face,
        thickness: &str,
        own: BlockState,
        output: &mut TickOutput,
    ) {
        let Some(block) = own.block() else {
            return;
        };
        let watery = FluidState::of(self.block_or_void(pos)).fluid == fluid::WATER;
        let state = set(
            block.default_state,
            "vertical_direction",
            face_name(direction),
        );
        let state = set(state, "thickness", thickness);
        self.set_block(pos, set_flag(state, "waterlogged", watery), output);
    }

    /// `SpeleothemBlock.growStalagmiteBelow`: the first stalagmite tip below that can
    /// grow grows, or a new one starts on the first floor, looking down at most ten
    /// blocks; a fluid, or for dripstone what a drip cannot pass, stops the search.
    fn grow_stalagmite_below(&mut self, tip: BlockPos, own: BlockState, output: &mut TickOutput) {
        let mut at = tip;
        for _ in 0..STALAGMITE_SEARCH {
            at = relative(at, Face::Down);
            let here = self.block_or_void(at);
            if !FluidState::of(here).is_empty() {
                return;
            }
            if is_unmerged_tip_towards(here, Face::Up, own) && self.can_tip_grow(at, here, own) {
                self.grow_speleothem_at(at, Face::Up, own, output);
                return;
            }
            let below = relative(at, Face::Down);
            if self.is_valid_speleothem_placement(at, Face::Up, own)
                && !has_water(self.block_or_void(below))
            {
                self.grow_speleothem_at(below, Face::Up, own, output);
                return;
            }
            // `blocksStalagmiteScan`: only pointed dripstone's search is stopped, by
            // what a drip cannot pass.
            if is_class(own, BlockClass::PointedDripstoneBlock) && !can_drip_through(here) {
                return;
            }
        }
    }

    /// `SpeleothemBlock.isValidSpeleothemPlacement`: what is behind `pos` against
    /// `direction` carries it, being sturdy or a part of the same block pointing the
    /// same way.
    fn is_valid_speleothem_placement(
        &self,
        pos: BlockPos,
        direction: Face,
        own: BlockState,
    ) -> bool {
        let behind = self.block_or_void(relative(pos, direction.opposite()));
        behind.props().is_face_sturdy(direction, SupportType::Full)
            || (is_speleothem_towards(behind, direction) && same_block(behind, own))
    }

    /// `PointedDripstoneBlock.maybeTransferFluid`, with the number `drip` drawn for it:
    /// water above the block a stalactite hangs from drips from its tip, and mud there
    /// dries to clay. The cauldron that a drip would fill is not looked for: see the
    /// head of this file.
    fn maybe_transfer_fluid(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        drip: f32,
        output: &mut TickOutput,
    ) {
        // The game asks whether the number exceeds both chances; water's is the larger.
        if drip > WATER_TRANSFER_CHANCE {
            return;
        }
        if !self.is_stalactite_start(pos, state) {
            return;
        }
        let Some((source, source_state, fluid)) = self.fluid_above_stalactite(pos, state) else {
            return;
        };
        let chance = match fluid {
            fluid::WATER => WATER_TRANSFER_CHANCE,
            fluid::LAVA => LAVA_TRANSFER_CHANCE,
            _ => return,
        };
        if drip >= chance {
            return;
        }
        if self.find_tip(pos, state, DRIP_SEARCH, false).is_none() {
            return;
        }
        if is_block(source_state, blocks::MUD) && fluid == fluid::WATER {
            self.set_block(source, blocks::CLAY, output);
        }
    }

    /// `PointedDripstoneBlock.getFluidAboveStalactite`: where above the block that the
    /// stalactite at `pos` hangs from its fluid comes from, what stands there and the
    /// fluid. Mud gives water where water does not evaporate, which is everywhere in
    /// the overworld.
    fn fluid_above_stalactite(
        &self,
        pos: BlockPos,
        state: BlockState,
    ) -> Option<(BlockPos, BlockState, u8)> {
        if !is_speleothem_towards(state, Face::Down) {
            return None;
        }
        // `findRootBlock`: up the stalactite to the first block that is not of it.
        let root = self.find_vertical(
            pos,
            Face::Up,
            |other| same_block(other, state) && tip_direction(other) == Some(Face::Down),
            |other| !same_block(other, state),
            DRIP_SEARCH,
        )?;
        let source = relative(root, Face::Up);
        let there = self.block_or_void(source);
        let fluid = if is_block(there, blocks::MUD) {
            fluid::WATER
        } else {
            FluidState::of(there).fluid
        };
        Some((source, there, fluid))
    }
}

/// Whether `class` is one of the blocks of copper that weather (`WeatheringCopper`'s
/// classes).
fn is_weathering(class: BlockClass) -> bool {
    matches!(
        class,
        BlockClass::WeatheringCopperBarsBlock
            | BlockClass::WeatheringCopperBulbBlock
            | BlockClass::WeatheringCopperChainBlock
            | BlockClass::WeatheringCopperChestBlock
            | BlockClass::WeatheringCopperDoorBlock
            | BlockClass::WeatheringCopperFullBlock
            | BlockClass::WeatheringCopperGolemStatueBlock
            | BlockClass::WeatheringCopperGrateBlock
            | BlockClass::WeatheringCopperSlabBlock
            | BlockClass::WeatheringCopperStairBlock
            | BlockClass::WeatheringCopperTrapDoorBlock
            | BlockClass::WeatheringLanternBlock
            | BlockClass::WeatheringLightningRodBlock
    )
}

/// The stages of weathering in the game's order (`WeatherState`), as the names of the
/// blocks begin.
const STAGES: [&str; 4] = ["", "exposed_", "weathered_", "oxidized_"];

/// The block's name without its namespace and its stage, and how weathered it is (the
/// ordinal of its `WeatherState`), if it is copper that weathers. Waxed copper is of
/// other classes and never is.
fn weathering_stage(state: BlockState) -> Option<(&'static str, usize)> {
    if !is_weathering(state.block_info()?.class) {
        return None;
    }
    let name = state.block()?.name.strip_prefix("minecraft:")?;
    let stage = (1..STAGES.len())
        .rev()
        .find(|stage| name.starts_with(STAGES[*stage]))
        .unwrap_or(0);
    Some((&name[STAGES[stage].len()..], stage))
}

/// `WeatheringCopper.getNext`: the same block a stage more weathered, with the same
/// properties (`withPropertiesOf`). The block of copper is the one whose name changes
/// otherwise: `copper_block` becomes `exposed_copper`.
fn next_weathering(state: BlockState) -> Option<BlockState> {
    let (base, stage) = weathering_stage(state)?;
    let next = STAGES.get(stage + 1)?;
    let base = if base == "copper_block" {
        "copper"
    } else {
        base
    };
    let id = clustine_data::block_id_by_name(&format!("minecraft:{next}{base}"))?;
    let mut weathered = BLOCKS[id].default_state;
    for (name, value) in state.properties() {
        weathered = weathered.with_property(name, value).unwrap_or(weathered);
    }
    Some(weathered)
}

/// `BuddingAmethystBlock.canClusterGrowAtState`: air, or water that fills its block.
fn can_cluster_grow_at(state: BlockState) -> bool {
    state.props().is_air() || (is_block(state, blocks::WATER) && FluidState::of(state).amount == 8)
}

/// The block a speleothem grows from (`blockToGrowOn`): dripstone for pointed
/// dripstone, sulfur for the sulfur spike. The sulfur spike's is given where the game
/// makes its blocks, which is not in what this was ported from; sulfur is what the
/// game's sulfur spike clusters grow from (`sulfur_spike_cluster`'s `base_block`), as
/// dripstone is for dripstone clusters.
fn block_to_grow_on(state: BlockState) -> BlockState {
    if is_class(state, BlockClass::SulfurSpikeBlock) {
        blocks::SULFUR
    } else {
        blocks::DRIPSTONE_BLOCK
    }
}

/// `getMaxGrowthLength`: how long a speleothem grows by its random ticks.
fn max_growth_length(state: BlockState) -> i32 {
    if is_class(state, BlockClass::SulfurSpikeBlock) {
        2
    } else {
        7
    }
}

/// The way a speleothem points (`TIP_DIRECTION`).
fn tip_direction(state: BlockState) -> Option<Face> {
    match state.property("vertical_direction")? {
        "up" => Some(Face::Up),
        "down" => Some(Face::Down),
        _ => None,
    }
}

/// `SpeleothemBlock.isSpeleothemWithDirection`.
fn is_speleothem_towards(state: BlockState, direction: Face) -> bool {
    in_tag(state, SPELEOTHEMS) && tip_direction(state) == Some(direction)
}

/// `SpeleothemBlock.isTip`: a tip, or with `allow_merged` also a tip that meets another.
fn is_tip(state: BlockState, allow_merged: bool) -> bool {
    if !in_tag(state, SPELEOTHEMS) {
        return false;
    }
    match state.property("thickness") {
        Some("tip") => true,
        Some("tip_merge") => allow_merged,
        _ => false,
    }
}

/// `SpeleothemBlock.isUnmergedTipWithDirection`: a tip of `own`'s block pointing
/// `direction`.
fn is_unmerged_tip_towards(state: BlockState, direction: Face, own: BlockState) -> bool {
    is_tip(state, false) && tip_direction(state) == Some(direction) && same_block(state, own)
}

/// `SpeleothemBlock.isFreeHangingStalactite`: the tip of a stalactite in the open.
fn is_free_hanging_stalactite(state: BlockState) -> bool {
    is_speleothem_towards(state, Face::Down)
        && state.property("thickness") == Some("tip")
        && state.property("waterlogged") != Some("true")
}

/// `PointedDripstoneBlock.canDripThrough`: air, or a block that is neither a full
/// opaque cube nor holds a fluid and leaves free the column four pixels wide down its
/// middle (`REQUIRED_SPACE_TO_DRIP_THROUGH_NON_SOLID_BLOCK`).
///
/// The collision shape is the one at the position (0, 0, 0): a block the game shifts
/// sideways by where it stands (pointed dripstone, bamboo) is taken as unshifted.
fn can_drip_through(state: BlockState) -> bool {
    let props = state.props();
    if props.is_air() {
        return true;
    }
    if props.is_solid_render() || props.fluid().is_some() {
        return false;
    }
    let (low, high) = (6.0 / 16.0, 10.0 / 16.0);
    !props
        .collision_shape()
        .boxes()
        .any(|[x0, y0, z0, x1, y1, z1]| {
            x0 < high && x1 > low && y0 < 1.0 && y1 > 0.0 && z0 < high && z1 > low
        })
}

/// The game compares the corners of shapes with this much to spare.
const EPSILON: f64 = 1.0e-7;

/// `LightEngine.shapeOccludes`: whether light on its way out of `from` through
/// `towards` into `to` is stopped by the two faces that meet there, one of them whole
/// or the two together covering the face. The same rule as `clustine-light`'s
/// `faces::stop_light`, which this crate does not depend on.
fn shape_occludes(from: BlockState, to: BlockState, towards: Face) -> bool {
    let near = light_face(from, towards);
    let far = light_face(to, towards.opposite());
    if near.is_full() || far.is_full() {
        return true;
    }
    if near.is_empty() && far.is_empty() {
        return false;
    }
    covered([0.0, 0.0, 1.0, 1.0], near, far, 0)
}

/// What `state` covers of `face` against light: its occlusion face if it occludes and
/// does so by its shape, nothing otherwise.
fn light_face(state: BlockState, face: Face) -> FaceShape {
    let props = state.props();
    if props.occludes() && props.occludes_by_shape() {
        props.occlusion_face(face)
    } else {
        FaceShape::EMPTY
    }
}

/// Whether `area` (`[min u, min v, max u, max v]`) lies within the union of the
/// rectangles of the two shapes, of which the first `skip` are known not to touch it.
/// The first rectangle that overlaps the area is cut out of it, and what is left of
/// the area, at most four smaller ones, has to be covered by the rectangles after it.
fn covered(area: [f64; 4], near: FaceShape, far: FaceShape, skip: usize) -> bool {
    let [min_u, min_v, max_u, max_v] = area;
    if max_u - min_u <= EPSILON || max_v - min_v <= EPSILON {
        return true;
    }
    let rectangles = near.rectangles().chain(far.rectangles());
    for (index, [low_u, low_v, high_u, high_v]) in rectangles.enumerate().skip(skip) {
        let overlaps = low_u < max_u - EPSILON
            && high_u > min_u + EPSILON
            && low_v < max_v - EPSILON
            && high_v > min_v + EPSILON;
        if !overlaps {
            continue;
        }
        let next = index + 1;
        let from_u = low_u.max(min_u);
        let to_u = high_u.min(max_u);
        return covered([min_u, min_v, from_u, max_v], near, far, next)
            && covered([to_u, min_v, max_u, max_v], near, far, next)
            && covered([from_u, min_v, to_u, low_v.max(min_v)], near, far, next)
            && covered([from_u, high_v.min(max_v), to_u, max_v], near, far, next);
    }
    false
}

#[cfg(test)]
mod tests {
    use clustine_data::DIMENSION_TYPES;
    use clustine_world::{Biome, Chunk, ChunkPos, EntityIds, Vec3};

    use super::*;
    use crate::api::Inventory;
    use crate::region::{Holdings, RegionConfig};

    /// The state the game would print as `text`.
    fn state(text: &str) -> BlockState {
        BlockState::parse(text).unwrap_or_else(|| panic!("{text} is a block state"))
    }

    fn at(x: i32, y: i32, z: i32) -> BlockPos {
        BlockPos::new(x, y, z)
    }

    /// The middle of the chunk at the origin, well inside what the region has loaded.
    const MIDDLE: BlockPos = BlockPos::new(8, 0, 8);

    /// A region with the 3×3 chunks around the origin loaded, empty but for a stone
    /// floor at y = -61, so that the chunk at the origin ticks.
    fn region() -> Region {
        let config = RegionConfig {
            dimension: Default::default(),
            spawn: Some(Vec3::new(0.5, -60.0, 0.5)),
            starting_inventory: Inventory::default(),
            return_after: 0,
            lowest_y: -64,
            seed: 0,
            game_seed: 0,
            sea_level: 63,
            spawn_mobs: true,
            light: Default::default(),
            features: Default::default(),
            random_tick_speed: crate::region::RANDOM_TICK_SPEED,
        };
        let ids = EntityIds::block(0).expect("the first block of ids");
        let mut region = Region::new(config, ids, Holdings::default());
        let overworld = DIMENSION_TYPES
            .iter()
            .find(|dimension| dimension.name == "minecraft:overworld")
            .expect("the overworld");
        for x in -1..=1 {
            for z in -1..=1 {
                let mut chunk = Chunk::empty(overworld, Biome(0));
                for bx in 0..16 {
                    for bz in 0..16 {
                        chunk.set(bx, -61, bz, blocks::STONE);
                    }
                }
                region.chunks.insert(ChunkPos::new(x, z), chunk);
            }
        }
        region.update_ticking();
        region
    }

    /// Puts `state` at `pos` without telling anything.
    fn put(region: &mut Region, pos: BlockPos, state: BlockState) {
        let (x, z) = pos.in_chunk();
        let chunk = region.chunks.get_mut(&pos.chunk()).expect("a loaded chunk");
        chunk.set(x, pos.y, z, state).expect("within the world");
    }

    fn get(region: &Region, pos: BlockPos) -> BlockState {
        region.block(pos).expect("a loaded block")
    }

    /// The random tick of what stands at `pos`, from the stream numbered `seed`.
    fn random_tick(region: &mut Region, pos: BlockPos, seed: u64) -> TickOutput {
        let state = get(region, pos);
        let mut random = Random::of_section(7, ChunkPos::new(0, 0), 0, seed);
        let mut output = TickOutput::default();
        assert!(
            region.mineral_random_tick(pos, state, &mut random, &mut output),
            "{state:?} is one of this file's blocks"
        );
        output
    }

    /// Random ticks at `pos` from streams `from`, `from + 1` and on until the block
    /// there changes, at most `limit` of them; the stream that changed it.
    fn until_changed(region: &mut Region, pos: BlockPos, from: u64, limit: u64) -> Option<u64> {
        until_changed_at(region, pos, pos, from, limit)
    }

    /// The same with the ticks at `ticked` and the block looked at at `watched`.
    fn until_changed_at(
        region: &mut Region,
        ticked: BlockPos,
        watched: BlockPos,
        from: u64,
        limit: u64,
    ) -> Option<u64> {
        let before = get(region, watched);
        (from..from + limit).find(|seed| {
            random_tick(region, ticked, *seed);
            get(region, watched) != before
        })
    }

    #[test]
    fn copper_weathers_one_stage_at_a_time_and_keeps_its_properties() {
        let mut region = region();
        let stairs = "facing=east,half=top,shape=outer_left,waterlogged=false";
        put(
            &mut region,
            MIDDLE,
            state(&format!("minecraft:cut_copper_stairs[{stairs}]")),
        );
        let mut seed = 0;
        for next in ["exposed", "weathered", "oxidized"] {
            seed = until_changed(&mut region, MIDDLE, seed + 1, 5_000).expect("it weathers");
            assert_eq!(
                get(&region, MIDDLE),
                state(&format!("minecraft:{next}_cut_copper_stairs[{stairs}]"))
            );
        }
        assert_eq!(until_changed(&mut region, MIDDLE, seed + 1, 3_000), None);
    }

    #[test]
    fn the_block_of_copper_weathers_into_exposed_copper() {
        let mut region = region();
        put(&mut region, MIDDLE, blocks::COPPER_BLOCK);
        until_changed(&mut region, MIDDLE, 0, 5_000).expect("it weathers");
        assert_eq!(get(&region, MIDDLE), blocks::EXPOSED_COPPER);
    }

    #[test]
    fn waxed_copper_never_weathers_and_is_none_of_these_blocks() {
        let mut region = region();
        for waxed in [
            blocks::WAXED_COPPER_BLOCK,
            blocks::WAXED_EXPOSED_CUT_COPPER,
            blocks::WAXED_COPPER_BULB,
        ] {
            assert!(!waxed.props().is_randomly_ticking());
            let mut random = Random::of_section(7, ChunkPos::new(0, 0), 0, 0);
            let mut output = TickOutput::default();
            put(&mut region, MIDDLE, waxed);
            assert!(!region.mineral_random_tick(MIDDLE, waxed, &mut random, &mut output));
            assert_eq!(next_weathering(waxed), None);
        }
    }

    #[test]
    fn every_copper_that_weathers_has_a_next_stage_exactly_where_the_game_ticks_it() {
        // The game ticks a block of copper randomly where it has a next stage; the data
        // says which it ticks, so the names this file builds can be held to it.
        let mut weathering = 0;
        for block in BLOCKS.iter() {
            let first = block.default_state;
            if weathering_stage(first).is_none() {
                continue;
            }
            weathering += 1;
            for id in block.first_state.0..=block.last_state.0 {
                let state = BlockState(id);
                let next = next_weathering(state);
                assert_eq!(
                    next.is_some(),
                    state.props().is_randomly_ticking(),
                    "{}",
                    block.name
                );
                if let Some(next) = next {
                    assert_eq!(
                        weathering_stage(next).map(|(_, stage)| stage),
                        weathering_stage(state).map(|(_, stage)| stage + 1)
                    );
                    assert!(
                        state.properties().eq(next.properties()),
                        "{} keeps its properties",
                        block.name
                    );
                }
            }
        }
        assert_eq!(
            weathering,
            15 * 4,
            "the fifteen kinds of copper that weathers"
        );
    }

    #[test]
    fn copper_beside_less_weathered_copper_does_not_weather() {
        let mut region = region();
        put(&mut region, MIDDLE, blocks::EXPOSED_COPPER);
        put(&mut region, MIDDLE.offset(2, 1, -1), blocks::CUT_COPPER);
        assert_eq!(until_changed(&mut region, MIDDLE, 0, 3_000), None);
        // Five blocks away along the axes it no longer counts.
        put(&mut region, MIDDLE.offset(2, 1, -1), blocks::AIR);
        put(&mut region, MIDDLE.offset(3, 1, -1), blocks::CUT_COPPER);
        assert!(until_changed(&mut region, MIDDLE, 0, 3_000).is_some());
    }

    /// How many of `trials` random ticks weather the exposed copper at the middle of a
    /// region that has `around` near it, each tick on the copper as it was.
    fn weathered_of(around: &[(BlockPos, BlockState)], trials: u64) -> u32 {
        let mut region = region();
        for (pos, state) in around {
            put(&mut region, *pos, *state);
        }
        let mut weathered = 0;
        for seed in 0..trials {
            put(&mut region, MIDDLE, blocks::EXPOSED_COPPER);
            random_tick(&mut region, MIDDLE, seed);
            if get(&region, MIDDLE) != blocks::EXPOSED_COPPER {
                assert_eq!(get(&region, MIDDLE), blocks::WEATHERED_COPPER);
                weathered += 1;
            }
        }
        weathered
    }

    #[test]
    fn the_chance_to_weather_falls_with_copper_as_weathered_nearby_and_rises_with_more() {
        let trials = 40_000;
        // Alone, exposed copper weathers in about 0.0569 of its ticks: 2,276 of these.
        let alone = weathered_of(&[], trials);
        assert!((1_900..2_700).contains(&alone), "{alone}");
        // With three as weathered nearby the share is a quarter, its square a
        // sixteenth: about 142.
        let beside = [(1, 0, 0), (0, 2, 0), (0, 0, -3)];
        let same: Vec<_> = beside
            .iter()
            .map(|(x, y, z)| (MIDDLE.offset(*x, *y, *z), blocks::EXPOSED_COPPER))
            .collect();
        let crowded = weathered_of(&same, trials);
        assert!((80..220).contains(&crowded), "{crowded}");
        // With the three more weathered instead the share is whole again.
        let more: Vec<_> = beside
            .iter()
            .map(|(x, y, z)| (MIDDLE.offset(*x, *y, *z), blocks::OXIDIZED_COPPER))
            .collect();
        let helped = weathered_of(&more, trials);
        assert!((1_900..2_700).contains(&helped), "{helped}");
    }

    #[test]
    fn a_copper_door_weathers_at_its_lower_half_and_the_upper_half_follows() {
        let mut region = region();
        let lower = state("minecraft:copper_door[half=lower,facing=north,open=true]");
        let upper = state("minecraft:copper_door[half=upper,facing=north,open=true]");
        put(&mut region, MIDDLE, lower);
        put(&mut region, MIDDLE.offset(0, 1, 0), upper);
        put(&mut region, MIDDLE.offset(0, -1, 0), blocks::STONE);
        assert_eq!(
            until_changed(&mut region, MIDDLE.offset(0, 1, 0), 0, 3_000),
            None,
            "the upper half does not weather of itself"
        );
        until_changed(&mut region, MIDDLE, 0, 5_000).expect("it weathers");
        assert_eq!(
            get(&region, MIDDLE),
            state("minecraft:exposed_copper_door[half=lower,facing=north,open=true]")
        );
        assert_eq!(
            get(&region, MIDDLE.offset(0, 1, 0)),
            state("minecraft:exposed_copper_door[half=upper,facing=north,open=true]")
        );
    }

    #[test]
    fn budding_amethyst_grows_buds_outwards_through_the_stages_in_order() {
        let mut region = region();
        put(&mut region, MIDDLE, blocks::BUDDING_AMETHYST);
        // Water that fills its block takes a bud; flowing water does not.
        put(&mut region, MIDDLE.offset(1, 0, 0), blocks::WATER);
        put(
            &mut region,
            MIDDLE.offset(-1, 0, 0),
            state("minecraft:water[level=3]"),
        );
        let stages = [
            blocks::SMALL_AMETHYST_BUD,
            blocks::MEDIUM_AMETHYST_BUD,
            blocks::LARGE_AMETHYST_BUD,
            blocks::AMETHYST_CLUSTER,
        ];
        let mut reached = [0; 6];
        for seed in 0..4_000 {
            let before: Vec<BlockState> = Face::ALL
                .iter()
                .map(|face| get(&region, relative(MIDDLE, *face)))
                .collect();
            random_tick(&mut region, MIDDLE, seed);
            for (index, face) in Face::ALL.iter().enumerate() {
                let now = get(&region, relative(MIDDLE, *face));
                if now == before[index] {
                    continue;
                }
                // A change is to the next stage, facing away from the budding block.
                let stage = stages
                    .iter()
                    .position(|stage| is_block(now, *stage))
                    .expect("a bud or a cluster");
                assert_eq!(stage, reached[index], "{face:?} grew in order");
                reached[index] += 1;
                assert_eq!(now.property("facing"), Some(face_name(*face)));
                assert_eq!(
                    now.property("waterlogged"),
                    Some(if *face == Face::East && stage == 0 {
                        "true"
                    } else if *face == Face::East {
                        before[index].property("waterlogged").expect("a bud")
                    } else {
                        "false"
                    })
                );
            }
        }
        // The flowing water to the west takes none; every other side is a cluster.
        for (index, face) in Face::ALL.iter().enumerate() {
            let expected = if *face == Face::West { 0 } else { 4 };
            assert_eq!(reached[index], expected, "{face:?}");
        }
        assert_eq!(
            get(&region, MIDDLE.offset(-1, 0, 0)),
            state("minecraft:water[level=3]")
        );
    }

    #[test]
    fn a_bud_that_does_not_face_away_from_the_budding_amethyst_does_not_grow() {
        let mut region = region();
        put(&mut region, MIDDLE, blocks::BUDDING_AMETHYST);
        let sideways = state("minecraft:small_amethyst_bud[facing=north]");
        for face in Face::ALL {
            put(&mut region, relative(MIDDLE, face), sideways);
        }
        for seed in 0..2_000 {
            random_tick(&mut region, MIDDLE, seed);
        }
        // Only the bud to the north faced away, and only it grew.
        for face in Face::ALL {
            let now = get(&region, relative(MIDDLE, face));
            if face == Face::North {
                assert_eq!(now, state("minecraft:amethyst_cluster[facing=north]"));
            } else {
                assert_eq!(now, sideways, "{face:?}");
            }
        }
    }

    /// A stalactite tip hanging at `MIDDLE` from dripstone, with `above` above that.
    fn stalactite_under(above: BlockState) -> Region {
        let mut region = region();
        put(&mut region, MIDDLE.offset(0, 2, 0), above);
        put(&mut region, MIDDLE.offset(0, 1, 0), blocks::DRIPSTONE_BLOCK);
        put(
            &mut region,
            MIDDLE,
            state("minecraft:pointed_dripstone[vertical_direction=down,thickness=tip]"),
        );
        region
    }

    #[test]
    fn dripstone_grows_down_from_a_ceiling_under_water() {
        let mut region = stalactite_under(blocks::WATER);
        let below = MIDDLE.offset(0, -1, 0);
        // The floor is too far below for a stalagmite, so only the stalactite grows.
        until_changed_at(&mut region, MIDDLE, below, 0, 5_000).expect("it grows");
        assert_eq!(
            get(&region, below),
            state("minecraft:pointed_dripstone[vertical_direction=down,thickness=tip]")
        );
        assert!(is_block(get(&region, MIDDLE), blocks::POINTED_DRIPSTONE));
    }

    #[test]
    fn dripstone_without_water_above_does_not_grow() {
        for above in [blocks::AIR, state("minecraft:water[level=1]"), blocks::LAVA] {
            let mut region = stalactite_under(above);
            for seed in 0..3_000 {
                let output = random_tick(&mut region, MIDDLE, seed);
                assert_eq!(output.events, [], "{above:?}");
            }
        }
    }

    #[test]
    fn a_stalactite_under_water_grows_a_stalagmite_on_the_floor_below() {
        let mut region = region();
        // Hanging at y = -54, seven blocks over the floor at y = -61.
        let tip = at(8, -54, 8);
        put(&mut region, tip.offset(0, 2, 0), blocks::WATER);
        put(&mut region, tip.offset(0, 1, 0), blocks::DRIPSTONE_BLOCK);
        put(
            &mut region,
            tip,
            state("minecraft:pointed_dripstone[vertical_direction=down,thickness=tip]"),
        );
        let floor = at(8, -60, 8);
        until_changed_at(&mut region, tip, floor, 0, 5_000).expect("a stalagmite starts");
        assert_eq!(
            get(&region, floor),
            state("minecraft:pointed_dripstone[vertical_direction=up,thickness=tip]")
        );
    }

    #[test]
    fn a_stalagmite_does_not_start_under_a_block_a_drip_cannot_pass() {
        let mut region = region();
        let tip = at(8, -54, 8);
        put(&mut region, tip.offset(0, 2, 0), blocks::WATER);
        put(&mut region, tip.offset(0, 1, 0), blocks::DRIPSTONE_BLOCK);
        put(
            &mut region,
            tip,
            state("minecraft:pointed_dripstone[vertical_direction=down,thickness=tip]"),
        );
        put(&mut region, at(8, -57, 8), blocks::OAK_PLANKS);
        let mut grown_below = false;
        for seed in 0..5_000 {
            random_tick(&mut region, tip, seed);
            grown_below |= get(&region, at(8, -56, 8)) != blocks::AIR;
            assert_eq!(get(&region, at(8, -60, 8)), blocks::AIR);
        }
        // On the planks themselves a stalagmite does start: they are a floor.
        assert!(grown_below);
    }

    #[test]
    fn a_stalactite_and_the_stalagmite_just_below_it_grow_into_one() {
        let mut region = stalactite_under(blocks::WATER);
        let below = MIDDLE.offset(0, -1, 0);
        put(
            &mut region,
            below,
            state("minecraft:pointed_dripstone[vertical_direction=up,thickness=tip]"),
        );
        until_changed_at(&mut region, MIDDLE, below, 0, 5_000).expect("they merge");
        assert_eq!(
            get(&region, MIDDLE),
            state("minecraft:pointed_dripstone[vertical_direction=down,thickness=tip_merge]")
        );
        assert_eq!(
            get(&region, below),
            state("minecraft:pointed_dripstone[vertical_direction=up,thickness=tip_merge]")
        );
    }

    #[test]
    fn mud_above_a_stalactite_dries_to_clay() {
        let mut region = stalactite_under(blocks::MUD);
        until_changed_at(&mut region, MIDDLE, MIDDLE.offset(0, 2, 0), 0, 500).expect("it dries");
        assert_eq!(get(&region, MIDDLE.offset(0, 2, 0)), blocks::CLAY);
    }

    #[test]
    fn a_sulfur_spike_grows_from_sulfur_without_water() {
        let mut region = region();
        put(&mut region, MIDDLE.offset(0, 1, 0), blocks::SULFUR);
        put(
            &mut region,
            MIDDLE,
            state("minecraft:sulfur_spike[vertical_direction=down,thickness=tip]"),
        );
        let below = MIDDLE.offset(0, -1, 0);
        until_changed_at(&mut region, MIDDLE, below, 0, 5_000).expect("it grows");
        assert_eq!(
            get(&region, below),
            state("minecraft:sulfur_spike[vertical_direction=down,thickness=tip]")
        );
    }

    #[test]
    fn lit_redstone_ore_goes_out_at_its_random_tick() {
        let mut region = region();
        for (lit, unlit) in [
            (
                "minecraft:redstone_ore[lit=true]",
                "minecraft:redstone_ore[lit=false]",
            ),
            (
                "minecraft:deepslate_redstone_ore[lit=true]",
                "minecraft:deepslate_redstone_ore[lit=false]",
            ),
        ] {
            put(&mut region, MIDDLE, state(lit));
            random_tick(&mut region, MIDDLE, 0);
            assert_eq!(get(&region, MIDDLE), state(unlit));
            assert!(!state(unlit).props().is_randomly_ticking());
        }
    }

    #[test]
    fn farmland_near_water_gets_wet_at_once() {
        let mut region = region();
        let farmland = state("minecraft:farmland[moisture=0]");
        put(&mut region, MIDDLE, farmland);
        // Four blocks across and one up still counts.
        put(&mut region, MIDDLE.offset(4, 1, -4), blocks::WATER);
        random_tick(&mut region, MIDDLE, 0);
        assert_eq!(
            get(&region, MIDDLE),
            state("minecraft:farmland[moisture=7]")
        );
    }

    #[test]
    fn farmland_away_from_water_dries_a_step_at_a_time() {
        let mut region = region();
        put(&mut region, MIDDLE, state("minecraft:farmland[moisture=7]"));
        // Five blocks away, or one below, is too far.
        put(&mut region, MIDDLE.offset(5, 0, 0), blocks::WATER);
        put(&mut region, MIDDLE.offset(0, -1, 0), blocks::WATER);
        for moisture in (0..7).rev() {
            random_tick(&mut region, MIDDLE, 0);
            assert_eq!(
                get(&region, MIDDLE),
                state(&format!("minecraft:farmland[moisture={moisture}]"))
            );
        }
    }

    #[test]
    fn dry_farmland_with_nothing_on_it_becomes_dirt_and_with_a_crop_stays() {
        let mut region = region();
        put(&mut region, MIDDLE, state("minecraft:farmland[moisture=0]"));
        put(&mut region, MIDDLE.offset(0, 1, 0), blocks::WHEAT);
        random_tick(&mut region, MIDDLE, 0);
        assert_eq!(
            get(&region, MIDDLE),
            state("minecraft:farmland[moisture=0]")
        );
        put(&mut region, MIDDLE.offset(0, 1, 0), blocks::AIR);
        random_tick(&mut region, MIDDLE, 0);
        assert_eq!(get(&region, MIDDLE), blocks::DIRT);
    }

    #[test]
    fn nylium_under_a_block_that_stops_light_becomes_netherrack() {
        for (above, stays) in [
            (blocks::AIR, true),
            (blocks::GLASS, true),
            (state("minecraft:oak_slab[type=top]"), true),
            (blocks::STONE, false),
            (state("minecraft:oak_slab[type=bottom]"), false),
        ] {
            let mut region = region();
            put(&mut region, MIDDLE, blocks::WARPED_NYLIUM);
            put(&mut region, MIDDLE.offset(0, 1, 0), above);
            random_tick(&mut region, MIDDLE, 0);
            let expected = if stays {
                blocks::WARPED_NYLIUM
            } else {
                blocks::NETHERRACK
            };
            assert_eq!(get(&region, MIDDLE), expected, "under {above:?}");
        }
    }

    #[test]
    fn a_wet_dried_ghast_has_its_hydration_scheduled_once() {
        let mut region = region();
        let wet = state("minecraft:dried_ghast[waterlogged=true]");
        put(&mut region, MIDDLE, wet);
        random_tick(&mut region, MIDDLE, 0);
        random_tick(&mut region, MIDDLE, 1);
        let ticks = region.ticks_to_save(ChunkPos::new(0, 0));
        assert_eq!(ticks.len(), 1, "{ticks:?}");
        assert_eq!(ticks[0].delay, GHAST_HYDRATION_DELAY - 1);
        assert_eq!(ticks[0].of, Ticked::Block(tick_id(wet)));
        // A dry ghast with no water taken up waits for none.
        let mut region = self::region();
        put(
            &mut region,
            MIDDLE,
            state("minecraft:dried_ghast[hydration=0]"),
        );
        random_tick(&mut region, MIDDLE, 0);
        assert_eq!(region.ticks_to_save(ChunkPos::new(0, 0)), []);
    }
}
