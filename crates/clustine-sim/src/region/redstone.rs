//! Redstone's sources, wire and the blocks power opens or lights, as the 26.3 jar's
//! classes have them (step R3a of `docs/adr/0026-redstone.md`, sections 2, 3 and 7):
//!
//! - a lever toggled by a click and a button pressed by one, each set with every
//!   update and then telling its neighbours and its support's
//!   (`LeverBlock.pull`, `ButtonBlock.press`, `updateNeighbours`); a button lets go in
//!   its own tick, 30 ticks on for one of wood and 20 for one of stone; both tell the
//!   same blocks when a powered one goes (`affectNeighborsAfterRemoval`);
//! - wire, by the game's default evaluator (`DefaultRedstoneWireEvaluator`): told of a
//!   neighbour, it goes where it can no longer stay and otherwise works out its power
//!   again; a change of power sets it without telling anyone and then updates the
//!   neighbours of seven positions in the order of Java's `HashSet`; placed or taken
//!   away, it tells the blocks around it and the wire at its corners
//!   (`onPlace`, `affectNeighborsAfterRemoval`, `updateNeighborsOfNeighboringWires`); a
//!   click turns a dot into a cross and back (`useWithoutItem`);
//! - a lamp lit at once by power and put out four ticks after it goes; doors,
//!   trapdoors and fence gates opened and closed by power, with their sounds;
//! - from R3b, a copper bulb flipped by each rise of its power, big dripleaf laid flat
//!   by power, and the dispatch to note blocks and bells (`instruments.rs`), plates,
//!   tripwire and hooks (`pressed.rs`) and rails (`rails.rs`);
//! - from R4, the dispatch to torches, repeaters, comparators, observers and target
//!   blocks (`diodes.rs`).
//!
//! The signals themselves are pure functions of the blocks, read through the region's
//! view (`clustine_blocks::signal`); the rules here run only where the updater lets
//! them, in a chunk that ticks (section 1 of the record).

use clustine_blocks::data::{HORIZONTAL, flag, is_class, relative, same_block, set_flag};
use clustine_blocks::signal;
use clustine_data::{BlockClass, BlockState, Face, sounds};
use clustine_world::{BlockPos, PlayerId, Ticked, Vec3};

use super::Region;
use super::updates::{RegionView, UPDATE_CLIENTS, UPDATE_LIMIT, fluid_block};
use super::vibrations::{Context, Vibration};
use crate::api::{RegionEvent, SOUND_SOURCE_BLOCKS, TickOutput};

/// `RedstoneLampBlock`'s delay before it goes out.
const LAMP_OFF_DELAY: i32 = 4;

impl Region {
    /// `neighborChanged` of the blocks of this step at `pos`, whose state is `state`,
    /// told that `block` changed beside it. Returns whether `state` is one of them.
    pub(super) fn redstone_neighbour_changed(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        block: BlockState,
        output: &mut TickOutput,
    ) -> bool {
        // Pistons, their heads and moving pistons (`pistons.rs`).
        if self.piston_neighbour_changed(pos, state, block, output) {
            return true;
        }
        // Torches, repeaters and comparators (`diodes.rs`).
        if self.diode_neighbour_changed(pos, state, output) {
            return true;
        }
        // Dispensers and droppers (`dispensers.rs`).
        if self.dispenser_neighbour_changed(pos, state, output) {
            return true;
        }
        if signal::is_wire(state) {
            // `RedStoneWireBlock.neighborChanged`: its source is not read without the
            // experiments.
            if clustine_blocks::can_survive(state, pos, &mut RegionView(self)) {
                self.update_power_strength(pos, state, output);
            } else {
                self.drop_resources(state, pos, output);
                self.set_block(pos, fluid_block(state), output);
            }
            return true;
        }
        if is_class(state, BlockClass::RedstoneLampBlock) {
            let lit = flag(state, "lit");
            if lit != self.has_neighbour_signal(pos) {
                if lit {
                    let block = clustine_blocks::data::tick_id(state);
                    self.schedule_tick(pos, Ticked::Block(block), LAMP_OFF_DELAY, 0);
                } else {
                    let lit = set_flag(state, "lit", true);
                    self.set_block_updated(pos, lit, UPDATE_CLIENTS, UPDATE_LIMIT, output);
                }
            }
            return true;
        }
        if is_class(state, BlockClass::DoorBlock) {
            self.door_powered(pos, state, block, output);
            return true;
        }
        if is_class(state, BlockClass::TrapDoorBlock) {
            self.trapdoor_powered(pos, state, output);
            return true;
        }
        if is_class(state, BlockClass::FenceGateBlock) {
            self.gate_powered(pos, state, output);
            return true;
        }
        if clustine_blocks::note::is_note_block(state) {
            self.note_block_powered(pos, state, output);
            return true;
        }
        if is_class(state, BlockClass::BellBlock) {
            self.bell_powered(pos, state, output);
            return true;
        }
        if is_class(state, BlockClass::CopperBulbBlock) {
            self.copper_bulb_check_and_flip(pos, state, output);
            return true;
        }
        if is_class(state, BlockClass::BigDripleafBlock) {
            // `BigDripleafBlock.neighborChanged`: power lays the leaf flat at once.
            if self.has_neighbour_signal(pos) {
                self.reset_dripleaf(pos, state, output);
            }
            return true;
        }
        false
    }

    /// `CopperBulbBlock.checkAndFlip`: power that rises turns the bulb on or off, with
    /// its sound heard by everyone near, and the bulb keeps whether it is powered. Its
    /// output for comparators is 15 while lit (`signal::analog_output`), which R4's
    /// comparators read.
    fn copper_bulb_check_and_flip(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        let powered = self.has_neighbour_signal(pos);
        if powered == flag(state, "powered") {
            return;
        }
        let mut changed = state;
        if !flag(state, "powered") {
            changed = set_flag(changed, "lit", !flag(state, "lit"));
            let sound = if flag(changed, "lit") {
                sounds::BLOCK_COPPER_BULB_TURN_ON
            } else {
                sounds::BLOCK_COPPER_BULB_TURN_OFF
            };
            play(pos, sound, 1.0, 1.0, None, output);
        }
        self.set_block(pos, set_flag(changed, "powered", powered), output);
    }

    /// `DoorBlock.neighborChanged`: a door is powered by power beside either half, and
    /// opens or closes with it, heard by everyone near. It is set with
    /// `UPDATE_CLIENTS`, and its other half follows from its shape. An update from a
    /// door of its own kind (its other half) does not move it.
    fn door_powered(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        block: BlockState,
        output: &mut TickOutput,
    ) {
        let other = if state.property("half") == Some("lower") {
            Face::Up
        } else {
            Face::Down
        };
        let powered =
            self.has_neighbour_signal(pos) || self.has_neighbour_signal(relative(pos, other));
        if same_block(block, state) || powered == flag(state, "powered") {
            return;
        }
        let changed = set_flag(set_flag(state, "powered", powered), "open", powered);
        if powered != flag(state, "open") {
            self.play_opened(pos, changed, output);
        }
        self.set_block_updated(pos, changed, UPDATE_CLIENTS, UPDATE_LIMIT, output);
    }

    /// `TrapDoorBlock.neighborChanged`: opened or closed by power beside it, heard by
    /// everyone near, set with `UPDATE_CLIENTS`; the water in it looks again.
    fn trapdoor_powered(&mut self, pos: BlockPos, state: BlockState, output: &mut TickOutput) {
        let powered = self.has_neighbour_signal(pos);
        if powered == flag(state, "powered") {
            return;
        }
        let mut changed = state;
        if flag(state, "open") != powered {
            changed = set_flag(changed, "open", powered);
            self.play_opened(pos, changed, output);
        }
        let changed = set_flag(changed, "powered", powered);
        self.set_block_updated(pos, changed, UPDATE_CLIENTS, UPDATE_LIMIT, output);
        if flag(state, "waterlogged") {
            let water = clustine_blocks::fluid::Kind::Water;
            self.schedule_tick(
                pos,
                Ticked::Fluid(water.source()),
                water.tick_delay(false),
                0,
            );
        }
    }

    /// `FenceGateBlock.neighborChanged`: opened or closed by power beside it, set with
    /// `UPDATE_CLIENTS` and then heard by everyone near.
    fn gate_powered(&mut self, pos: BlockPos, state: BlockState, output: &mut TickOutput) {
        let powered = self.has_neighbour_signal(pos);
        if powered == flag(state, "powered") {
            return;
        }
        let changed = set_flag(set_flag(state, "powered", powered), "open", powered);
        self.set_block_updated(pos, changed, UPDATE_CLIENTS, UPDATE_LIMIT, output);
        if flag(state, "open") != powered {
            self.play_opened(pos, changed, output);
        }
    }

    /// The open or close sound of a door, trapdoor or gate that has become `state`,
    /// played to everyone near (`playSound(null, …)`), its pitch the game's 0.9 to 1
    /// drawn from the rules' stream.
    fn play_opened(&mut self, pos: BlockPos, state: BlockState, output: &mut TickOutput) {
        let Some(sound) = crate::placement::use_sound(state) else {
            return;
        };
        let pitch = self.next_draw().next_float() * 0.1 + 0.9;
        play(pos, sound, 1.0, pitch, None, output);
        // `block_open` or `block_close`, of no entity (`vibrations.rs`).
        self.vibrate_open(pos, state, None);
    }

    /// The `block_open` or `block_close` of a door, trapdoor or gate that has become
    /// `state` at `pos`, opened or closed by `player` or by power.
    pub(super) fn vibrate_open(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        player: Option<PlayerId>,
    ) {
        if state.property("open").is_none() {
            return;
        }
        let event = if flag(state, "open") {
            Vibration::BLOCK_OPEN
        } else {
            Vibration::BLOCK_CLOSE
        };
        let source = player.and_then(|player| self.player_source(player));
        self.vibrate_at(event, pos, Context::of(source, None));
    }

    /// `SignalGetter.hasNeighborSignal` of the region's blocks.
    fn has_neighbour_signal(&mut self, pos: BlockPos) -> bool {
        signal::has_neighbour_signal(&mut RegionView(self), pos)
    }

    /// `DefaultRedstoneWireEvaluator.updatePowerStrength`: wire at `pos`, whose state
    /// was `state` when it was asked, takes the power it is to have, set without telling
    /// anyone if it is still that state, and the neighbours of it and of its six
    /// neighbours are told, in the order of Java's `HashSet` of the seven.
    fn update_power_strength(&mut self, pos: BlockPos, state: BlockState, output: &mut TickOutput) {
        // Where the chunk does not tick, what the wire reads may not all be there
        // (section 1 of the record). The updater never tells wire there, but wire can
        // be set there otherwise, by an effect or by a step of another region: wire
        // that is there waits as an update of itself, which works its power out once
        // the chunk ticks; for wire taken away the seven are told, each kept until
        // it can run, as the game would tell them had the power changed.
        if !self.ticks_chunk(pos.chunk()) {
            if self.own_block(pos) == Some(state) {
                self.neighbour_changed_at(pos, Face::Down, state, false, output);
            } else {
                for at in signal::wire_update_order(pos) {
                    self.update_neighbours_at(at, clustine_data::blocks::REDSTONE_WIRE, output);
                }
            }
            return;
        }
        let target = signal::wire_target_strength(&mut RegionView(self), pos);
        if signal::wire_power(state) == target {
            return;
        }
        if self.block(pos) == Some(state) {
            let powered = signal::with_power(state, target);
            self.set_block_updated(pos, powered, UPDATE_CLIENTS, UPDATE_LIMIT, output);
        }
        for at in signal::wire_update_order(pos) {
            self.update_neighbours_at(at, clustine_data::blocks::REDSTONE_WIRE, output);
        }
    }

    /// `RedStoneWireBlock.onPlace`, and what a lever or a button does as it is
    /// placed (nothing), for a block that has just become `state` in place of `old`.
    pub(super) fn redstone_on_place(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        old: BlockState,
        output: &mut TickOutput,
    ) {
        // Torches and diodes tell their neighbours whenever their state changes
        // (`diodes.rs`).
        self.diode_on_place(pos, state, old, output);
        if same_block(state, old) {
            return;
        }
        // `CopperBulbBlock.onPlace`: a bulb placed beside power lights. Where the
        // chunk does not tick it waits as an update of itself, as wire does.
        if is_class(state, BlockClass::CopperBulbBlock) {
            if self.ticks_chunk(pos.chunk()) {
                self.copper_bulb_check_and_flip(pos, state, output);
            } else {
                self.neighbour_changed_at(pos, Face::Down, state, false, output);
            }
            return;
        }
        self.pressed_on_place(pos, state, old, output);
        if !signal::is_wire(state) {
            return;
        }
        self.update_power_strength(pos, state, output);
        // `Direction.Plane.VERTICAL`, which is up and then down.
        for towards in [Face::Up, Face::Down] {
            self.update_neighbours_at(relative(pos, towards), state, output);
        }
        self.update_neighbours_of_neighbouring_wires(pos, output);
    }

    /// `affectNeighborsAfterRemoval` of the blocks of this step: what `old`, which has
    /// just given way at `pos`, tells as it goes. A powered lever or button tells its
    /// neighbours and its support's; wire tells the neighbours of its own neighbours,
    /// works out the power of where it was, and tells the wire at its corners. Nothing
    /// of this when a piston moved it.
    pub(super) fn redstone_after_removal(
        &mut self,
        pos: BlockPos,
        old: BlockState,
        moved_by_piston: bool,
        output: &mut TickOutput,
    ) {
        // An observer tells what is behind it even when a piston moved it.
        self.diode_after_removal(pos, old, moved_by_piston, output);
        if moved_by_piston {
            return;
        }
        self.pressed_after_removal(pos, old, moved_by_piston, output);
        self.rail_after_removal(pos, old, output);
        if is_lever_or_button(old) {
            if flag(old, "powered") {
                self.update_attached(pos, old, output);
            }
            return;
        }
        if signal::is_wire(old) {
            for towards in Face::ALL {
                self.update_neighbours_at(relative(pos, towards), old, output);
            }
            self.update_power_strength(pos, old, output);
            self.update_neighbours_of_neighbouring_wires(pos, output);
        }
    }

    /// `RedStoneWireBlock.updateNeighborsOfNeighboringWires`: the wire beside `pos`,
    /// and then the wire above a conductor beside it or below what does not conduct.
    fn update_neighbours_of_neighbouring_wires(&mut self, pos: BlockPos, output: &mut TickOutput) {
        for towards in HORIZONTAL {
            self.check_corner_change_at(relative(pos, towards), output);
        }
        for towards in HORIZONTAL {
            let beside = relative(pos, towards);
            let conducts = self
                .block(beside)
                .is_some_and(clustine_blocks::connect::is_conductor);
            let corner = if conducts {
                relative(beside, Face::Up)
            } else {
                relative(beside, Face::Down)
            };
            self.check_corner_change_at(corner, output);
        }
    }

    /// `RedStoneWireBlock.checkCornerChangeAt`: wire at `pos` tells its neighbours and
    /// theirs.
    fn check_corner_change_at(&mut self, pos: BlockPos, output: &mut TickOutput) {
        let Some(state) = self.block(pos) else {
            return;
        };
        if !signal::is_wire(state) {
            return;
        }
        self.update_neighbours_at(pos, state, output);
        for towards in Face::ALL {
            self.update_neighbours_at(relative(pos, towards), state, output);
        }
    }

    /// `LeverBlock.updateNeighbours` and `ButtonBlock.updateNeighbours`: the neighbours
    /// of the lever or button `state` at `pos`, and then those of the block it hangs on.
    pub(super) fn update_attached(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        let Some(towards) = signal::attached_towards(state) else {
            return;
        };
        self.update_neighbours_at(pos, state, output);
        self.update_neighbours_at(relative(pos, towards.opposite()), state, output);
    }

    /// A block's own `tick` for the blocks of this step: a pressed button lets go, a lit
    /// lamp with no power beside it goes out. Returns whether `state` is one of them.
    pub(super) fn redstone_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) -> bool {
        if self.pressed_tick(pos, state, output) {
            return true;
        }
        if self.detector_rail_tick(pos, state, output) {
            return true;
        }
        if self.diode_tick(pos, state, output) {
            return true;
        }
        if is_class(state, BlockClass::ButtonBlock) {
            // `ButtonBlock.checkPressed`: nothing but an arrow keeps a button pressed
            // (`projectile_hits.rs`), and one that does is looked at again as long on.
            let arrow = self.arrow_on_button(pos, state);
            if flag(state, "powered") && !arrow {
                let released = set_flag(state, "powered", false);
                self.set_block(pos, released, output);
                self.update_attached(pos, released, output);
                let sound = button_sound(state, false);
                play(pos, sound, 1.0, 1.0, None, output);
                self.vibrate_at(Vibration::BLOCK_DEACTIVATE, pos, Context::default());
            }
            if arrow {
                let block = clustine_blocks::data::tick_id(state);
                self.schedule_tick(pos, Ticked::Block(block), button_delay(state), 0);
            }
            return true;
        }
        if is_class(state, BlockClass::RedstoneLampBlock) {
            if flag(state, "lit") && !self.has_neighbour_signal(pos) {
                let unlit = set_flag(state, "lit", false);
                self.set_block_updated(pos, unlit, UPDATE_CLIENTS, UPDATE_LIMIT, output);
            }
            return true;
        }
        false
    }

    /// A click whose use is the block's own rule (`useWithoutItem`), run on the block at
    /// `pos` by `player` on its side `face`: a lever toggled, a button pressed, wire
    /// turned from a dot to a cross or back, a note block tuned, a bell rung. A use that reads what is two blocks away runs only where its
    /// chunk ticks, as the predicate worked out at the end of the tick before says
    /// (`docs/adr/0026-redstone.md`, section 1); one that cannot changes nothing.
    pub(super) fn use_by_rule(
        &mut self,
        pos: BlockPos,
        face: Face,
        player: PlayerId,
        output: &mut TickOutput,
    ) {
        let Some(state) = self.own_block(pos) else {
            return;
        };
        if is_class(state, BlockClass::AbstractBedBlock) {
            self.use_bed(pos, player, output);
            return;
        }
        if is_class(state, BlockClass::RespawnAnchorBlock) {
            self.use_anchor(pos, face, player, crate::api::Hand::Main, output);
            return;
        }
        if Region::opens_menu(state) {
            self.use_container(pos, player, output);
            return;
        }
        if is_class(state, BlockClass::CopperGolemStatueBlock) {
            self.statue_used(pos, state, player, output);
            return;
        }
        if signal::use_reads_signal(state) && !self.ticks_chunk(pos.chunk()) {
            return;
        }
        if is_class(state, BlockClass::LeverBlock) {
            // `LeverBlock.pull`, heard by everyone: the game plays it to no one's
            // exclusion, the clicking player's client included.
            let pulled = set_flag(state, "powered", !flag(state, "powered"));
            self.set_block(pos, pulled, output);
            self.update_attached(pos, pulled, output);
            let pitch = if flag(pulled, "powered") { 0.6 } else { 0.5 };
            play(pos, sounds::BLOCK_LEVER_CLICK, 0.3, pitch, None, output);
            // `LeverBlock.useWithoutItem`'s event, of the player (`vibrations.rs`).
            let event = if flag(pulled, "powered") {
                Vibration::BLOCK_ACTIVATE
            } else {
                Vibration::BLOCK_DEACTIVATE
            };
            let context = Context::of(self.player_source(player), None);
            self.vibrate_at(event, pos, context);
        } else if is_class(state, BlockClass::ButtonBlock) {
            // `ButtonBlock.press`; a button already pressed takes the click and does
            // nothing.
            if flag(state, "powered") {
                return;
            }
            let pressed = set_flag(state, "powered", true);
            self.set_block(pos, pressed, output);
            self.update_attached(pos, pressed, output);
            let block = clustine_blocks::data::tick_id(state);
            self.schedule_tick(pos, Ticked::Block(block), button_delay(state), 0);
            let sound = button_sound(state, true);
            play(pos, sound, 1.0, 1.0, Some(player), output);
            let context = Context::of(self.player_source(player), None);
            self.vibrate_at(Vibration::BLOCK_ACTIVATE, pos, context);
        } else if signal::is_wire(state) {
            let Some(used) = signal::wire_used(&mut RegionView(self), state, pos) else {
                return;
            };
            self.set_block(pos, used, output);
            self.wire_updates_on_shape_change(pos, state, used, output);
        } else if clustine_blocks::note::is_note_block(state) {
            self.note_block_used(pos, state, output);
        } else if is_class(state, BlockClass::BellBlock) {
            // `BellBlock.onHit`: the bell swings away from the side it was hit on.
            self.ring_bell(pos, Some(face), output);
        } else if !self.daylight_detector_used(pos, state, player, output) {
            self.diode_used(pos, state, player, output);
        }
    }

    /// `RedStoneWireBlock.updatesOnShapeChange`: a conductor on a side the wire has
    /// come to run towards, or stopped running towards, tells its neighbours but the
    /// wire.
    fn wire_updates_on_shape_change(
        &mut self,
        pos: BlockPos,
        old: BlockState,
        new: BlockState,
        output: &mut TickOutput,
    ) {
        for towards in HORIZONTAL {
            if signal::wire_runs(old, towards) == signal::wire_runs(new, towards) {
                continue;
            }
            let beside = relative(pos, towards);
            if self
                .block(beside)
                .is_some_and(clustine_blocks::connect::is_conductor)
            {
                self.update_neighbours_at_except(beside, new, towards.opposite(), output);
            }
        }
    }

    /// `RedStoneWireBlock.updateIndirectNeighbourShapes`: wire whose side runs towards
    /// a block that is not wire asks the wire above and below that block to take its
    /// shape again, as their neighbour beside `pos` above or below it.
    pub(super) fn wire_indirect_shapes(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        flags: u32,
        depth: i32,
        output: &mut TickOutput,
    ) {
        if !signal::is_wire(state) {
            return;
        }
        for towards in HORIZONTAL {
            if !signal::wire_runs(state, towards) {
                continue;
            }
            let beside = relative(pos, towards);
            if self.block(beside).is_some_and(signal::is_wire) {
                continue;
            }
            for vertical in [Face::Down, Face::Up] {
                let corner = relative(beside, vertical);
                if self.block(corner).is_some_and(signal::is_wire) {
                    let neighbour_pos = relative(corner, towards.opposite());
                    let neighbour = self
                        .block(neighbour_pos)
                        .unwrap_or(clustine_data::blocks::AIR);
                    self.shape_changed_at(
                        corner,
                        towards.opposite(),
                        neighbour,
                        flags,
                        depth,
                        output,
                    );
                }
            }
        }
    }
}

/// Whether `state` is a lever or a button.
fn is_lever_or_button(state: BlockState) -> bool {
    is_class(state, BlockClass::LeverBlock) || is_class(state, BlockClass::ButtonBlock)
}

/// How long a button stays pressed, by its block set as the game's `Blocks` makes
/// them: 20 ticks for stone and polished blackstone, 30 for wood.
pub(super) fn button_delay(state: BlockState) -> i32 {
    if is_stone_button(state) { 20 } else { 30 }
}

fn is_stone_button(state: BlockState) -> bool {
    state
        .block()
        .is_some_and(|block| block.name.contains("stone"))
}

/// A button's click as it is pressed or let go, by its block set (`BlockSetType`'s
/// `buttonClickOn` and `buttonClickOff`).
pub(super) fn button_sound(state: BlockState, on: bool) -> i32 {
    use sounds as s;
    let name = state.block().map_or("", |block| block.name);
    let (on_sound, off_sound) = if is_stone_button(state) {
        (
            s::BLOCK_STONE_BUTTON_CLICK_ON,
            s::BLOCK_STONE_BUTTON_CLICK_OFF,
        )
    } else if name.contains("cherry") {
        (
            s::BLOCK_CHERRY_WOOD_BUTTON_CLICK_ON,
            s::BLOCK_CHERRY_WOOD_BUTTON_CLICK_OFF,
        )
    } else if name.contains("bamboo") {
        (
            s::BLOCK_BAMBOO_WOOD_BUTTON_CLICK_ON,
            s::BLOCK_BAMBOO_WOOD_BUTTON_CLICK_OFF,
        )
    } else if name.contains("crimson") || name.contains("warped") {
        (
            s::BLOCK_NETHER_WOOD_BUTTON_CLICK_ON,
            s::BLOCK_NETHER_WOOD_BUTTON_CLICK_OFF,
        )
    } else {
        (
            s::BLOCK_WOODEN_BUTTON_CLICK_ON,
            s::BLOCK_WOODEN_BUTTON_CLICK_OFF,
        )
    };
    if on { on_sound } else { off_sound }
}

/// `Level.playSound(entity, pos, …)` of a block's sound, at the middle of the block,
/// from the blocks; `except` is the player whose own client plays it.
pub(super) fn play(
    pos: BlockPos,
    sound: i32,
    volume: f32,
    pitch: f32,
    except: Option<PlayerId>,
    output: &mut TickOutput,
) {
    output.events.push(RegionEvent::Sound {
        sound,
        source: SOUND_SOURCE_BLOCKS,
        position: Vec3::new(
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 0.5,
            f64::from(pos.z) + 0.5,
        ),
        volume,
        pitch,
        except,
    });
}
