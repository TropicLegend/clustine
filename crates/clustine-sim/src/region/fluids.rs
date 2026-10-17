//! A fluid's tick and what it sets (`docs/adr/0024-block-behaviour-in-a-region.md`,
//! step B5): the game's `FlowingFluid.tick`, `spread` and `spreadTo`, lava's own
//! `spreadTo` and `getSpreadDelay`, the containers' `placeLiquid`, and a liquid block's
//! `shouldSpreadLiquid`, by which lava meeting water becomes obsidian, cobblestone or
//! stone. What a fluid reads to know where it goes is `clustine_blocks::fluid`, shared
//! with generation.
//!
//! Named: a block a fluid washes away drops nothing (Clustine has no items lying in
//! the world yet); lava's random ticks, which set fire, wait for step B7; a bubble column
//! does not form in water over soul sand or magma.

use clustine_blocks::BlockView;
use clustine_blocks::data::{has_water, is_block, is_class, relative};
use clustine_blocks::fluid::{
    self, FluidState, Kind, can_be_replaced_with, can_hold_specific_fluid, can_maybe_pass_through,
    is_container, is_water_hole, new_liquid, source_neighbour_count,
};
use clustine_data::{BlockClass, BlockState, Face, blocks};
use clustine_world::{BlockPos, Effect, Effected, Ticked};

use super::Region;
use super::updates::RegionView;
use crate::api::{RegionEvent, TickOutput};

/// `LiquidBlock.POSSIBLE_FLOW_DIRECTIONS`: lava looks for water against the flow from
/// each of these, so above it, then north, south, west and east of it.
const POSSIBLE_FLOW_DIRECTIONS: [Face; 5] =
    [Face::Down, Face::South, Face::North, Face::East, Face::West];

impl Region {
    /// `ServerLevel.tickFluid` and `FlowingFluid.tick`: where the block at `pos` still
    /// holds exactly `fluid`, a fluid that is not a source becomes what its neighbours
    /// make of it, and the fluid spreads.
    pub(super) fn fluid_tick(&mut self, pos: BlockPos, fluid: u8, output: &mut TickOutput) {
        let Some(mut state) = self.block(pos) else {
            return;
        };
        let mut held = FluidState::of(state);
        if held.fluid != fluid {
            return;
        }
        let Some(kind) = held.kind() else {
            return;
        };
        if !held.is_source() {
            let new = new_liquid(&mut RegionView(self), kind, pos, state);
            let delay = self.spread_delay(kind, pos, held, new);
            if new.is_empty() {
                held = new;
                state = blocks::AIR;
                self.set_block(pos, state, output);
            } else if new != held {
                held = new;
                state = new.legacy_block();
                self.set_block(pos, state, output);
                self.schedule_tick(pos, Ticked::Fluid(new.fluid), delay, 0);
            }
        }
        self.spread(kind, pos, state, held, output);
    }

    /// `getSpreadDelay`: water's tick delay; lava's, four times as long three times in
    /// four where it rises and neither it nor what it becomes falls. The game draws
    /// that from the level's random; Clustine from the position and the tick, so that
    /// a region's tick stays a function of its inputs.
    fn spread_delay(
        &mut self,
        kind: Kind,
        pos: BlockPos,
        held: FluidState,
        new: FluidState,
    ) -> i32 {
        let delay = kind.tick_delay();
        if kind != Kind::Lava || held.is_empty() || new.is_empty() || held.falling || new.falling {
            return delay;
        }
        let view = &mut RegionView(self);
        if fluid::height(view, new, pos) > fluid::height(view, held, pos)
            && !draw(pos, self.tick).is_multiple_of(4)
        {
            delay * 4
        } else {
            delay
        }
    }

    /// `FlowingFluid.spread`: down where it can, and then sideways as well if three
    /// sources are beside it; otherwise sideways, unless it is flowing and can go down.
    fn spread(
        &mut self,
        kind: Kind,
        pos: BlockPos,
        state: BlockState,
        held: FluidState,
        output: &mut TickOutput,
    ) {
        if held.is_empty() {
            return;
        }
        let below_pos = relative(pos, Face::Down);
        let Some(below) = self.block(below_pos) else {
            return;
        };
        if can_maybe_pass_through(kind, state, Face::Down, below) {
            let view = &mut RegionView(self);
            let new = new_liquid(view, kind, below_pos, below);
            let replaced = can_be_replaced_with(
                view,
                FluidState::of(below),
                below_pos,
                new.fluid,
                Face::Down,
            );
            if replaced && can_hold_specific_fluid(below, new.fluid) {
                self.spread_to(kind, below_pos, below, Face::Down, new, output);
                if source_neighbour_count(&mut RegionView(self), kind, pos) >= 3 {
                    self.spread_to_sides(kind, pos, held, state, output);
                }
                return;
            }
        }
        if held.is_source() || !is_water_hole(kind, state, below) {
            self.spread_to_sides(kind, pos, held, state, output);
        }
    }

    /// `FlowingFluid.spreadToSides`.
    fn spread_to_sides(
        &mut self,
        kind: Kind,
        pos: BlockPos,
        held: FluidState,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        let amount = if held.falling {
            7
        } else {
            i32::from(held.amount) - kind.drop_off()
        };
        if amount <= 0 {
            return;
        }
        for (face, new) in fluid::spread(&mut RegionView(self), kind, pos, state) {
            let beside = relative(pos, face);
            // Across the line the holder runs the target's side whole, from the source
            // as it is now (`docs/adr/0025-effects-between-regions.md`, section 4).
            if self.sends_to(beside.chunk()) {
                self.made.push(Effect {
                    at: beside,
                    what: Effected::Spread {
                        direction: opposite(face) as u8,
                        source: state,
                    },
                });
                continue;
            }
            let Some(there) = self.own_block(beside) else {
                continue;
            };
            self.spread_to(kind, beside, there, face, new, output);
        }
    }

    /// The target's side of `FlowingFluid.spreadTo` from a source in another region,
    /// at `pos`, whose neighbour towards `towards` held `source` with its fluid
    /// (`docs/adr/0025-effects-between-regions.md`, section 4): `canMaybePassThrough`,
    /// `getNewLiquid`, `canHoldSpecificFluid` and `canBeReplacedWith` as the game's
    /// `getSpread` asks them, reading `source` where they read the source's position and
    /// what the region has elsewhere; then the spread itself.
    pub(super) fn spread_received(
        &mut self,
        pos: BlockPos,
        towards: Face,
        source: BlockState,
        output: &mut TickOutput,
    ) {
        let Some(target) = self.own_block(pos) else {
            return;
        };
        let Some(kind) = FluidState::of(source).kind() else {
            return;
        };
        let flow = opposite(towards);
        if !can_maybe_pass_through(kind, source, flow, target) {
            return;
        }
        let mut view = Carried {
            region: self,
            at: relative(pos, towards),
            state: source,
        };
        let new = new_liquid(&mut view, kind, pos, target);
        if !can_hold_specific_fluid(target, new.fluid) {
            return;
        }
        if !can_be_replaced_with(&mut view, FluidState::of(target), pos, new.fluid, flow) {
            return;
        }
        self.spread_to(kind, pos, target, flow, new, output);
    }

    /// `spreadTo`: lava flowing down into water turns a liquid block of it to stone;
    /// otherwise a container takes the fluid in, and anything else gives way to it
    /// (lava hisses as it does).
    fn spread_to(
        &mut self,
        kind: Kind,
        pos: BlockPos,
        state: BlockState,
        direction: Face,
        new: FluidState,
        output: &mut TickOutput,
    ) {
        if kind == Kind::Lava && direction == Face::Down && has_water(state) {
            if is_class(state, BlockClass::LiquidBlock) {
                self.set_block(pos, blocks::STONE, output);
            }
            self.fizz(pos, output);
            return;
        }
        if is_container(state) {
            self.place_liquid(pos, state, new, output);
            return;
        }
        if !state.props().is_air() && kind == Kind::Lava {
            self.fizz(pos, output);
        }
        self.set_block(pos, new.legacy_block(), output);
    }

    /// `LiquidBlockContainer.placeLiquid`: a block that can be waterlogged and is not
    /// takes still water, and its tick; a lit campfire or candle goes out as it does.
    /// Kelp, seagrass and a double slab take nothing.
    fn place_liquid(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        new: FluidState,
        output: &mut TickOutput,
    ) {
        let can_be_waterlogged = state.property("waterlogged") == Some("false")
            && !(is_class(state, BlockClass::SlabBlock)
                && state.property("type") == Some("double"));
        if !can_be_waterlogged || new.fluid != fluid::WATER {
            return;
        }
        let mut waterlogged = state
            .with_property("waterlogged", "true")
            .expect("a block that can be waterlogged can be waterlogged");
        if is_class(state, BlockClass::CampfireBlock) || is_class(state, BlockClass::CandleBlock) {
            waterlogged = waterlogged
                .with_property("lit", "false")
                .unwrap_or(waterlogged);
        }
        self.set_block(pos, waterlogged, output);
        self.schedule_tick(pos, Ticked::Fluid(new.fluid), Kind::Water.tick_delay(), 0);
    }

    /// `LiquidBlock.shouldSpreadLiquid`: lava with water above or beside it becomes
    /// obsidian where it is a source and cobblestone where it flows, and lava on soul
    /// soil beside blue ice becomes basalt, and then it does not spread.
    pub(super) fn should_spread_liquid(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) -> bool {
        if FluidState::of(state).kind() != Some(Kind::Lava) {
            return true;
        }
        let on_soul_soil = self
            .block(relative(pos, Face::Down))
            .is_some_and(|below| is_block(below, blocks::SOUL_SOIL));
        for direction in POSSIBLE_FLOW_DIRECTIONS {
            let beside = self
                .block(relative(pos, opposite(direction)))
                .unwrap_or(blocks::AIR);
            if has_water(beside) {
                let source = self
                    .block(pos)
                    .is_some_and(|here| FluidState::of(here).is_source());
                let block = if source {
                    blocks::OBSIDIAN
                } else {
                    blocks::COBBLESTONE
                };
                self.set_block(pos, block, output);
                self.fizz(pos, output);
                return false;
            }
            if on_soul_soil && is_block(beside, blocks::BLUE_ICE) {
                self.set_block(pos, blocks::BASALT, output);
                self.fizz(pos, output);
                return false;
            }
        }
        true
    }

    /// Lava's hiss and smoke (the game's level event 1501).
    fn fizz(&mut self, pos: BlockPos, output: &mut TickOutput) {
        output.events.push(RegionEvent::Fizzed { position: pos });
    }
}

/// The region's blocks as the shared rules read them, but for one block that another
/// region carried with its effect, read in place of the region's copy of it.
struct Carried<'r> {
    region: &'r mut Region,
    at: BlockPos,
    state: BlockState,
}

impl BlockView for Carried<'_> {
    fn next_int(&mut self, bound: u32) -> u32 {
        RegionView(self.region).next_int(bound)
    }

    fn block(&mut self, at: BlockPos) -> BlockState {
        if at == self.at {
            self.state
        } else {
            RegionView(self.region).block(at)
        }
    }

    fn raw_brightness(&mut self, at: BlockPos) -> Option<u8> {
        RegionView(self.region).raw_brightness(at)
    }

    fn schedule_block_tick(&mut self, at: BlockPos, block: u16, delay: i32) {
        RegionView(self.region).schedule_block_tick(at, block, delay);
    }

    fn schedule_fluid_tick(&mut self, at: BlockPos, fluid: u8, delay: i32) {
        RegionView(self.region).schedule_fluid_tick(at, fluid, delay);
    }

    fn rules(&self) -> clustine_blocks::Rules {
        self.region.rules()
    }

    fn has_scheduled_block_tick(&mut self, at: BlockPos, block: u16) -> bool {
        RegionView(self.region).has_scheduled_block_tick(at, block)
    }

    fn comparator_output(&mut self, at: BlockPos) -> u8 {
        RegionView(self.region).comparator_output(at)
    }
}

/// A number drawn from the position and the tick (SplitMix64's finaliser), for what
/// the game draws from the level's random.
fn draw(pos: BlockPos, tick: u64) -> u64 {
    let mut z = (pos.x as u32 as u64)
        ^ (u64::from(pos.y as u32) << 21)
        ^ (u64::from(pos.z as u32) << 42)
        ^ tick.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn opposite(face: Face) -> Face {
    match face {
        Face::Down => Face::Up,
        Face::Up => Face::Down,
        Face::North => Face::South,
        Face::South => Face::North,
        Face::West => Face::East,
        Face::East => Face::West,
    }
}
