//! What players and other entities press and step on (step R3b of
//! `docs/adr/0026-redstone.md`, and step N6e of `docs/groundwork/n6-plan.md` for the
//! entities that are not players), as the 26.3 jar's classes have them: pressure
//! plates, tripwire and its hooks, big dripleaf, and redstone ore.
//!
//! The game applies an entity's effects of blocks in its tick
//! (`Entity.applyEffectsFromBlocks`): `stepOn` of the block it stands on, then
//! `entityInside` of each block its box is in. For players that is a phase of its own
//! after the block events and before the other entities, for each player in the order
//! of their ids, with the box where the player stands at the end of the tick's inputs:
//! the game sweeps the box along the tick's movement, which a player walking fast over
//! a plate in one tick would press, and this does not. Whether a player sneaks is not
//! region state, so none steps carefully. Every other entity's walk (an item's, a
//! cart's, a boat's, a falling block's, primed TNT's, an armour stand's) hands back the
//! plates, tripwire and detector rails it met, and the region runs their `entityInside`
//! for it in its own tick, in the order of the entities ([`Region::entity_touched`]).
//!
//! The count of what presses a plate, and of what keeps tripwire down, is every entity
//! of the region's own chunks whose box meets the block's box, and every player of the
//! region. The brief's finding 6 asks when it is taken: at the moment the game takes
//! it, in the tick of the entity that walked in, with the entities that have ticked
//! already in this tick where their ticks left them and the rest where they were, as
//! their records stand, and the entity that walked in where its walk has it, which its
//! record may not say yet. The scheduled looks again count every entity where the last
//! tick left it. As the region's entities tick in the order of their keys and not of
//! the game's list, which of two entities has moved when the other counts is not always
//! the game's. A marker armour stand presses nothing; stone and polished blackstone
//! plates count only living entities (players and armour stands); none of Clustine's
//! players is a spectator.
//!
//! At a line (`docs/groundwork/n6-plan.md`, section 6 item 6 and section 7 item 12):
//! the entities of copies and the players of other regions are not counted yet, which
//! step N6g brings ([`Region::pressing`] is where).
//!
//! - A plate is pressed while an entity's box overlaps its lower quarter, set with
//!   `UPDATE_CLIENTS` and telling the blocks around it and below it, and looks again
//!   by its own tick, 20 ticks on (10 for weighted plates, whose power is the count
//!   of what stands on them against 15 for gold and 150 for iron).
//! - Tripwire is pressed while a box overlaps its shape, looks again ten ticks on, and
//!   tells the hooks along its line, which power the blocks they hang on; a hook
//!   finds the hook facing it along its line and the tripwire between them
//!   (`TripWireHookBlock.calculateState`). The line reads up to 41 blocks, so a hook
//!   whose line meets a block the region cannot read does nothing rather than take
//!   it for air.
//! - Big dripleaf tilts under a player standing on it, by its own ticks, and power
//!   beside it holds it flat.
//! - Redstone ore lights when a player clicks it or stands on it; its random tick puts
//!   it out (`random_ticks/minerals.rs`). A left click lights it only in survival,
//!   which no Clustine player is in (`instruments.rs` says why).

use clustine_blocks::data::{facing, flag, is_class, relative, same_block, set, set_flag, tick_id};
use clustine_blocks::signal::{self, MAX_SIGNAL};
use clustine_data::{BlockClass, BlockState, Face, blocks, entity_types, sounds};
use clustine_world::{BlockPos, EntityData, EntityId, Ticked, Vec3};

use super::Region;
use super::updates::{RegionView, UPDATE_CLIENTS, UPDATE_LIMIT};
use crate::api::{RegionEvent, SOUND_SOURCE_BLOCKS, TickOutput};
use crate::physics::aabb::Aabb;

/// An entity other than a player whose walk met a block the region runs the
/// `entityInside` of for it: its key, its box where its walk has it (its record may
/// still say where its tick began), and whether it is living, which a stone plate asks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Toucher {
    pub(crate) key: EntityId,
    pub(crate) bounds: Aabb,
    pub(crate) living: bool,
}

/// `BasePressurePlateBlock.TOUCH_AABB`: the part of a plate's block an entity presses
/// it in, a column 14 wide and 4 high.
const TOUCH: [f64; 6] = [
    1.0 / 16.0,
    0.0,
    1.0 / 16.0,
    15.0 / 16.0,
    4.0 / 16.0,
    15.0 / 16.0,
];

/// `TripWireBlock.SHAPE_ATTACHED` and `SHAPE_NOT_ATTACHED`, their bounds.
const TRIPWIRE_ATTACHED: [f64; 6] = [0.0, 1.0 / 16.0, 0.0, 1.0, 2.5 / 16.0, 1.0];
const TRIPWIRE_LOOSE: [f64; 6] = [0.0, 0.0, 0.0, 1.0, 8.0 / 16.0, 1.0];

/// How long tripwire stays pressed before it looks again.
const TRIPWIRE_DELAY: i32 = 10;
/// How far along its line a hook looks for the hook facing it.
const HOOK_REACH: i32 = 42;

/// `Entity.checkInsideBlocks` deflates the box by this much.
const INSIDE_MARGIN: f64 = 1.0e-5;

/// `BigDripleafBlock.DELAY_UNTIL_NEXT_TILT_STATE`.
const DRIPLEAF_UNSTABLE: i32 = 10;
const DRIPLEAF_PARTIAL: i32 = 10;
const DRIPLEAF_FULL: i32 = 100;

impl Region {
    /// The phase in which the region's players act on the blocks they stand on and
    /// in, in the order of their ids.
    pub(crate) fn players_touch_blocks(&mut self, output: &mut TickOutput) {
        let players: Vec<(Vec3, bool)> = self
            .players
            .values()
            .map(|player| (player.pose.position, player.pose.on_ground))
            .collect();
        for (feet, on_ground) in players {
            if on_ground {
                // `getOnPosLegacy`: the block a fifth of a block below the feet.
                let on = BlockPos::new(
                    feet.x.floor() as i32,
                    (feet.y - 0.2).floor() as i32,
                    feet.z.floor() as i32,
                );
                if self.ticks_chunk(on.chunk())
                    && let Some(state) = self.own_block(on)
                    && is_class(state, BlockClass::RedStoneOreBlock)
                {
                    self.light_redstone_ore(on, state, output);
                }
            }
            let body = crate::placement::body(feet, false);
            let inside = [
                body[0] + INSIDE_MARGIN,
                body[1] + INSIDE_MARGIN,
                body[2] + INSIDE_MARGIN,
                body[3] - INSIDE_MARGIN,
                body[4] - INSIDE_MARGIN,
                body[5] - INSIDE_MARGIN,
            ];
            // `BlockPos.betweenClosed`: x first, then y, then z.
            let (low, high) = (
                [inside[0], inside[1], inside[2]].map(|v| v.floor() as i32),
                [inside[3], inside[4], inside[5]].map(|v| v.floor() as i32),
            );
            for z in low[2]..=high[2] {
                for y in low[1]..=high[1] {
                    for x in low[0]..=high[0] {
                        let pos = BlockPos::new(x, y, z);
                        self.entity_inside(pos, feet, on_ground, &inside, output);
                    }
                }
            }
        }
    }

    /// `entityInside` of the block at `pos` for a player with its feet at `feet` and
    /// the deflated box `inside`.
    fn entity_inside(
        &mut self,
        pos: BlockPos,
        feet: Vec3,
        on_ground: bool,
        inside: &[f64; 6],
        output: &mut TickOutput,
    ) {
        if !self.ticks_chunk(pos.chunk()) {
            return;
        }
        let Some(state) = self.own_block(pos) else {
            return;
        };
        if let Some(power) = signal::plate_signal(state) {
            if power == 0 {
                self.check_plate(pos, state, None, output);
            }
        } else if is_class(state, BlockClass::TripWireBlock) {
            let shape = if flag(state, "attached") {
                TRIPWIRE_ATTACHED
            } else {
                TRIPWIRE_LOOSE
            };
            let tripwire = Ticked::Block(tick_id(state));
            if !flag(state, "powered")
                && !self.has_scheduled_tick(pos, tripwire)
                && overlaps(&moved(shape, pos), inside)
            {
                self.tripwire_pressed(pos, true, output);
            }
        } else if is_class(state, BlockClass::BigDripleafBlock) {
            // `BigDripleafBlock.entityInside` and `canEntityTilt`: on the ground, above
            // the leaf's stem.
            let above_stem = feet.y > f64::from(pos.y as f32 + 0.6875);
            if state.property("tilt") == Some("none")
                && on_ground
                && above_stem
                && !signal::has_neighbour_signal(&mut RegionView(self), pos)
            {
                self.tilt_dripleaf(pos, state, "unstable", None, output);
            }
        }
    }

    /// `entityInside` of the block at `pos` for `toucher`, an entity other than a player
    /// whose walk met it, in its tick: an unpowered plate counts what is on it
    /// (`BasePressurePlateBlock.entityInside`), unpowered tripwire with no look due is
    /// pressed by the entity itself (`TripWireBlock.entityInside`, which hands it
    /// `List.of(entity)`; no entity that walks ignores triggers), and an unpowered
    /// detector rail looks for carts (`DetectorRailBlock.entityInside`), which tells its
    /// comparators even where it finds none.
    pub(crate) fn entity_touched(
        &mut self,
        pos: BlockPos,
        toucher: &Toucher,
        output: &mut TickOutput,
    ) {
        if !self.ticks_chunk(pos.chunk()) {
            return;
        }
        let Some(state) = self.own_block(pos) else {
            return;
        };
        if let Some(power) = signal::plate_signal(state) {
            if power == 0 {
                self.check_plate(pos, state, Some(toucher), output);
            }
        } else if is_class(state, BlockClass::TripWireBlock) {
            let tripwire = Ticked::Block(tick_id(state));
            if !flag(state, "powered") && !self.has_scheduled_tick(pos, tripwire) {
                self.tripwire_pressed(pos, true, output);
            }
        } else if is_class(state, BlockClass::DetectorRailBlock) && !flag(state, "powered") {
            self.detector_rail_check(pos, state, output);
        }
    }

    /// `RedStoneOreBlock.interact`: unlit ore lights, with every update.
    pub(super) fn light_redstone_ore(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        if !flag(state, "lit") {
            self.set_block(pos, set_flag(state, "lit", true), output);
        }
    }

    /// `BasePressurePlateBlock.checkPressed`: the plate takes the power of what stands
    /// on it, tells the blocks around and below it of a change, clicks as it goes down
    /// or up, and looks again later while pressed. `toucher` is the entity in whose
    /// tick it checks, if any.
    fn check_plate(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        toucher: Option<&Toucher>,
        output: &mut TickOutput,
    ) {
        let old = signal::plate_signal(state).unwrap_or(0);
        let new = self.plate_strength(pos, state, toucher);
        if new != old {
            let changed = if is_class(state, BlockClass::WeightedPressurePlateBlock) {
                set(state, "power", &new.to_string())
            } else {
                set_flag(state, "powered", new > 0)
            };
            self.set_block_updated(pos, changed, UPDATE_CLIENTS, UPDATE_LIMIT, output);
            self.plate_updates_neighbours(pos, changed, output);
        }
        if new == 0 && old > 0 {
            play(pos, plate_sound(state, false), 1.0, 1.0, output);
        } else if new > 0 && old == 0 {
            play(pos, plate_sound(state, true), 1.0, 1.0, output);
        }
        if new > 0 {
            let block = Ticked::Block(tick_id(state));
            self.schedule_tick(pos, block, plate_delay(state), 0);
        }
    }

    /// `BasePressurePlateBlock.updateNeighbours`: the blocks around the plate, then
    /// those around the block below it.
    pub(super) fn plate_updates_neighbours(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        self.update_neighbours_at(pos, state, output);
        self.update_neighbours_at(relative(pos, Face::Down), state, output);
    }

    /// `getSignalStrength`: 15 for a plate anything stands on (a stone or polished
    /// blackstone one: anything living), and for a weighted plate its share of the
    /// entities on it against its most (`getEntityCount`).
    fn plate_strength(&self, pos: BlockPos, state: BlockState, toucher: Option<&Toucher>) -> u8 {
        let touch = moved(TOUCH, pos);
        let count = self.pressing(&touch, counts_only_living(state), toucher);
        if is_class(state, BlockClass::WeightedPressurePlateBlock) {
            let most = plate_weight(state);
            let pressing = count.min(most);
            if pressing == 0 {
                return 0;
            }
            let share = pressing as f32 / most as f32;
            return (share * f32::from(MAX_SIGNAL)).ceil() as u8;
        }
        if count > 0 { MAX_SIGNAL } else { 0 }
    }

    /// `TripWireBlock.checkPressed`: tripwire takes whether anything is on it, with
    /// every update, tells the hooks along it, and looks again later.
    fn tripwire_pressed(&mut self, pos: BlockPos, pressed: bool, output: &mut TickOutput) {
        let Some(state) = self.own_block(pos) else {
            return;
        };
        let was = flag(state, "powered");
        if pressed != was {
            let changed = set_flag(state, "powered", pressed);
            self.set_block(pos, changed, output);
            self.tripwire_update_source(pos, changed, output);
        }
        let tripwire = Ticked::Block(tick_id(state));
        if pressed {
            self.schedule_tick(pos, tripwire, TRIPWIRE_DELAY, 0);
        } else if was {
            self.schedule_tick(pos, tripwire, 1, 0);
        }
    }

    /// Whether the box of anything that does not ignore triggers overlaps tripwire
    /// `state` at `pos` (`TripWireBlock.checkPressed(level, pos)`).
    fn on_tripwire(&self, pos: BlockPos, state: BlockState) -> bool {
        let shape = if flag(state, "attached") {
            TRIPWIRE_ATTACHED
        } else {
            TRIPWIRE_LOOSE
        };
        self.pressing(&moved(shape, pos), false, None) > 0
    }

    /// How many entities that do not ignore triggers (`NO_SPECTATORS` and
    /// `!isIgnoringBlockTriggers`) have boxes that meet `area`, a box of a block: the
    /// region's players, then the entities of its own chunks, of the class
    /// `LivingEntity` only where `living_only` (players and armour stands until N7's
    /// mobs). `toucher`, in whose tick this counts, is counted where its walk has it.
    ///
    /// Step N6g adds here the entities of copies and the boxes of other regions'
    /// players sent across a line (`docs/groundwork/n6-plan.md`, section 6 item 6 and
    /// section 7 item 12).
    pub(super) fn pressing(
        &self,
        area: &[f64; 6],
        living_only: bool,
        toucher: Option<&Toucher>,
    ) -> usize {
        let players = self
            .players
            .values()
            .filter(|player| overlaps(&crate::placement::body(player.pose.position, false), area))
            .count();
        let bounds = Aabb::new([area[0], area[1], area[2]], [area[3], area[4], area[5]]);
        let others = self
            .entities_in(&bounds)
            .into_iter()
            .filter(|(key, _)| toucher.is_none_or(|toucher| toucher.key != *key))
            .filter_map(|(key, chunk)| self.entity_record(key, chunk))
            .filter(|record| !ignores_triggers(&record.data))
            .filter(|record| !living_only || i32::from(record.kind) == entity_types::ARMOR_STAND)
            .count();
        let touching = toucher.is_some_and(|toucher| {
            toucher.bounds.intersects(bounds) && (!living_only || toucher.living)
        });
        players + others + usize::from(touching)
    }

    /// `TripWireBlock.updateSource`: the hooks facing back along the line to the south
    /// and to the west, through tripwire only, work their state out again, with
    /// tripwire `state` at `pos`.
    pub(super) fn tripwire_update_source(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        for towards in [Face::South, Face::West] {
            for step in 1..HOOK_REACH {
                let at = along(pos, towards, step);
                let Some(found) = self.block(at) else {
                    break;
                };
                if is_class(found, BlockClass::TripWireHookBlock) {
                    if facing(found) == Some(towards.opposite()) {
                        self.hook_calculate_state(
                            at,
                            found,
                            false,
                            true,
                            step,
                            Some(state),
                            output,
                        );
                    }
                    break;
                }
                if !is_class(found, BlockClass::TripWireBlock) {
                    break;
                }
            }
        }
    }

    /// `TripWireHookBlock.calculateState`: a hook at `pos` of `state` looks along its
    /// line for the hook facing it, through armed tripwire, and both take whether the
    /// line holds and is pressed; then the tripwire along it whether it is attached.
    /// `wire_source` is the step at which tripwire told it, whose state is `wire_state`
    /// and which is looked at again ten ticks on.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn hook_calculate_state(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        being_destroyed: bool,
        can_update: bool,
        wire_source: i32,
        wire_state: Option<BlockState>,
        output: &mut TickOutput,
    ) {
        let Some(towards) = facing(state) else {
            return;
        };
        let was_attached = flag(state, "attached");
        let was_powered = flag(state, "powered");
        let hook_block = state;
        let mut attached = !being_destroyed;
        let mut powered = false;
        let mut receiver = 0;
        let mut wires: Vec<Option<BlockState>> = vec![None; HOOK_REACH as usize];
        for step in 1..HOOK_REACH {
            let at = along(pos, towards, step);
            // A line through a block the region cannot read is not judged, so that
            // nothing takes a block that is not there for air.
            let Some(mut found) = self.block(at) else {
                return;
            };
            if is_class(found, BlockClass::TripWireHookBlock) {
                if facing(found) == Some(towards.opposite()) {
                    receiver = step;
                }
                break;
            }
            if is_class(found, BlockClass::TripWireBlock) || step == wire_source {
                if step == wire_source {
                    found = wire_state.unwrap_or(found);
                }
                let armed = !flag(found, "disarmed");
                let wire_powered = flag(found, "powered");
                powered |= armed && wire_powered;
                wires[step as usize] = Some(found);
                if step == wire_source {
                    let block = Ticked::Block(tick_id(hook_block));
                    self.schedule_tick(pos, block, TRIPWIRE_DELAY, 0);
                    attached &= armed;
                }
            } else {
                wires[step as usize] = None;
                attached = false;
            }
        }
        attached &= receiver > 1;
        powered &= attached;
        let hook = set_flag(
            set_flag(clustine_data::blocks::TRIPWIRE_HOOK, "attached", attached),
            "powered",
            powered,
        );
        if receiver > 0 {
            let at = along(pos, towards, receiver);
            let back = towards.opposite();
            let other = set(hook, "facing", clustine_blocks::data::face_name(back));
            self.set_block(at, other, output);
            self.hook_notifies_neighbours(at, back, other, output);
            if !self
                .own_block(pos)
                .is_some_and(|now| is_class(now, BlockClass::TripWireHookBlock))
            {
                self.hook_removed(pos, hook, output);
                return;
            }
            self.hook_sounds(at, attached, powered, was_attached, was_powered, output);
        }
        self.hook_sounds(pos, attached, powered, was_attached, was_powered, output);
        if !being_destroyed {
            let own = set(hook, "facing", clustine_blocks::data::face_name(towards));
            self.set_block(pos, own, output);
            if can_update {
                self.hook_notifies_neighbours(pos, towards, own, output);
            }
        }
        if was_attached != attached {
            for step in 1..receiver {
                let at = along(pos, towards, step);
                let Some(wire) = wires[step as usize] else {
                    continue;
                };
                let still = self.own_block(at).is_some_and(|now| {
                    is_class(now, BlockClass::TripWireBlock)
                        || is_class(now, BlockClass::TripWireHookBlock)
                });
                if still {
                    let changed = if wire.property("attached").is_some() {
                        set_flag(wire, "attached", attached)
                    } else {
                        wire
                    };
                    self.set_block(at, changed, output);
                }
            }
        }
    }

    /// `TripWireHookBlock.notifyNeighbors`: the blocks around the hook, then those
    /// around the block it hangs on.
    fn hook_notifies_neighbours(
        &mut self,
        pos: BlockPos,
        towards: Face,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        self.update_neighbours_at(pos, state, output);
        self.update_neighbours_at(relative(pos, towards.opposite()), state, output);
    }

    /// `TripWireHookBlock.onRemoved`: a hook that held a line works the line out
    /// again without itself, and one that was powered tells the block it hung on.
    pub(super) fn hook_removed(&mut self, pos: BlockPos, old: BlockState, output: &mut TickOutput) {
        let attached = flag(old, "attached");
        let powered = flag(old, "powered");
        if attached || powered {
            self.hook_calculate_state(pos, old, true, false, -1, None, output);
        }
        if powered && let Some(towards) = facing(old) {
            self.hook_notifies_neighbours(pos, towards, old, output);
        }
    }

    /// `TripWireHookBlock.emitState`: a click as the line is pressed or let go, and a
    /// sound as it holds or breaks, heard by everyone near.
    fn hook_sounds(
        &mut self,
        pos: BlockPos,
        attached: bool,
        powered: bool,
        was_attached: bool,
        was_powered: bool,
        output: &mut TickOutput,
    ) {
        if powered && !was_powered {
            play(pos, sounds::BLOCK_TRIPWIRE_CLICK_ON, 0.4, 0.6, output);
        } else if !powered && was_powered {
            play(pos, sounds::BLOCK_TRIPWIRE_CLICK_OFF, 0.4, 0.5, output);
        } else if attached && !was_attached {
            play(pos, sounds::BLOCK_TRIPWIRE_ATTACH, 0.4, 0.7, output);
        } else if !attached && was_attached {
            let pitch = 1.2 / (self.next_draw().next_float() * 0.2 + 0.9);
            play(pos, sounds::BLOCK_TRIPWIRE_DETACH, 0.4, pitch, output);
        }
    }

    /// `BigDripleafBlock.setTiltAndScheduleTick`: the leaf takes `tilt`, with its sound
    /// if it has one, and its next tilt is due.
    fn tilt_dripleaf(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        tilt: &str,
        sound: Option<i32>,
        output: &mut TickOutput,
    ) {
        self.set_dripleaf_tilt(pos, state, tilt, output);
        if let Some(sound) = sound {
            self.dripleaf_sound(pos, sound, output);
        }
        let delay = match tilt {
            "unstable" => DRIPLEAF_UNSTABLE,
            "partial" => DRIPLEAF_PARTIAL,
            "full" => DRIPLEAF_FULL,
            _ => return,
        };
        self.schedule_tick(pos, Ticked::Block(tick_id(state)), delay, 0);
    }

    /// `BigDripleafBlock.resetTilt`: the leaf lies flat, and says so if it was not.
    pub(super) fn reset_dripleaf(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        self.set_dripleaf_tilt(pos, state, "none", output);
        if state.property("tilt") != Some("none") {
            self.dripleaf_sound(pos, sounds::BLOCK_BIG_DRIPLEAF_TILT_UP, output);
        }
    }

    /// `BigDripleafBlock.setTilt`: set with `UPDATE_CLIENTS` alone, which tells no
    /// neighbour.
    fn set_dripleaf_tilt(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        tilt: &str,
        output: &mut TickOutput,
    ) {
        let tilted = set(state, "tilt", tilt);
        self.set_block_updated(pos, tilted, UPDATE_CLIENTS, UPDATE_LIMIT, output);
    }

    /// `BigDripleafBlock.playTiltSound`: its pitch drawn between 0.8 and 1.2.
    fn dripleaf_sound(&mut self, pos: BlockPos, sound: i32, output: &mut TickOutput) {
        let pitch = self.next_draw().next_float() * 0.4 + 0.8;
        play(pos, sound, 1.0, pitch, output);
    }

    /// The `tick` of the blocks of this module. Returns whether `state` is one of
    /// them.
    pub(super) fn pressed_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) -> bool {
        if let Some(power) = signal::plate_signal(state) {
            // `BasePressurePlateBlock.tick`.
            if power > 0 {
                self.check_plate(pos, state, None, output);
            }
            return true;
        }
        if is_class(state, BlockClass::TripWireBlock) {
            // `TripWireBlock.tick`.
            if flag(state, "powered") {
                let pressed = self.on_tripwire(pos, state);
                self.tripwire_pressed(pos, pressed, output);
            }
            return true;
        }
        if is_class(state, BlockClass::TripWireHookBlock) {
            // `TripWireHookBlock.tick`.
            self.hook_calculate_state(pos, state, false, true, -1, None, output);
            return true;
        }
        if is_class(state, BlockClass::BigDripleafBlock) {
            // `BigDripleafBlock.tick`: power holds it flat; otherwise it tips on.
            if signal::has_neighbour_signal(&mut RegionView(self), pos) {
                self.reset_dripleaf(pos, state, output);
                return true;
            }
            let down = Some(sounds::BLOCK_BIG_DRIPLEAF_TILT_DOWN);
            match state.property("tilt") {
                Some("unstable") => self.tilt_dripleaf(pos, state, "partial", down, output),
                Some("partial") => self.tilt_dripleaf(pos, state, "full", down, output),
                Some("full") => self.reset_dripleaf(pos, state, output),
                _ => {}
            }
            return true;
        }
        false
    }

    /// `onPlace` of the blocks of this module, for `state` placed at `pos` in place of
    /// `old`: tripwire tells the hooks along it.
    pub(super) fn pressed_on_place(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        old: BlockState,
        output: &mut TickOutput,
    ) {
        if is_class(state, BlockClass::TripWireBlock) && !same_block(state, old) {
            self.tripwire_update_source(pos, state, output);
        }
    }

    /// `affectNeighborsAfterRemoval` of the blocks of this module: a pressed plate
    /// tells its neighbours as it goes, tripwire tells its hooks as if it were
    /// pressed, a hook lets go of its line. Nothing of this when a piston moved it.
    pub(super) fn pressed_after_removal(
        &mut self,
        pos: BlockPos,
        old: BlockState,
        moved_by_piston: bool,
        output: &mut TickOutput,
    ) {
        if moved_by_piston {
            return;
        }
        if let Some(power) = signal::plate_signal(old) {
            if power > 0 {
                self.plate_updates_neighbours(pos, old, output);
            }
        } else if is_class(old, BlockClass::TripWireBlock) {
            self.tripwire_update_source(pos, set_flag(old, "powered", true), output);
        } else if is_class(old, BlockClass::TripWireHookBlock) {
            self.hook_removed(pos, old, output);
        }
    }

    /// `TripWireHookBlock.setPlacedBy`: a hook placed by a player looks along its line.
    pub(super) fn placed_by_player(&mut self, pos: BlockPos, output: &mut TickOutput) {
        let Some(state) = self.own_block(pos) else {
            return;
        };
        if is_class(state, BlockClass::TripWireHookBlock) {
            self.hook_calculate_state(pos, state, false, false, -1, None, output);
        }
        self.diode_placed_by_player(pos, state);
        // `PistonBaseBlock.setPlacedBy`: a piston placed checks whether to move.
        if clustine_blocks::piston::is_piston_base(state) {
            self.piston_check(pos, state, output);
        }
    }
}

/// The block `steps` along `towards` from `pos`.
fn along(pos: BlockPos, towards: Face, steps: i32) -> BlockPos {
    let one = relative(BlockPos::new(0, 0, 0), towards);
    BlockPos::new(
        pos.x + one.x * steps,
        pos.y + one.y * steps,
        pos.z + one.z * steps,
    )
}

/// A box of a block, moved to the block at `pos`.
fn moved(shape: [f64; 6], pos: BlockPos) -> [f64; 6] {
    let (x, y, z) = (f64::from(pos.x), f64::from(pos.y), f64::from(pos.z));
    [
        shape[0] + x,
        shape[1] + y,
        shape[2] + z,
        shape[3] + x,
        shape[4] + y,
        shape[5] + z,
    ]
}

/// `AABB.intersects`: the boxes share more than a face.
fn overlaps(a: &[f64; 6], b: &[f64; 6]) -> bool {
    a[0] < b[3] && a[3] > b[0] && a[1] < b[4] && a[4] > b[1] && a[2] < b[5] && a[5] > b[2]
}

/// `Entity.isIgnoringBlockTriggers`: true only for an armour stand that is a marker.
fn ignores_triggers(data: &EntityData) -> bool {
    matches!(data, EntityData::ArmorStand { flags, .. } if flags & super::stands::MARKER != 0)
}

/// Whether the plate `state` is pressed only by living entities: the stone and the
/// polished blackstone plates, whose block sets are `MOBS` (`BlockSetType`'s static
/// initialiser); every wooden plate and both weighted ones are pressed by anything.
fn counts_only_living(state: BlockState) -> bool {
    same_block(state, blocks::STONE_PRESSURE_PLATE)
        || same_block(state, blocks::POLISHED_BLACKSTONE_PRESSURE_PLATE)
}

/// How long a plate stays pressed before it looks again: ten ticks for a weighted
/// plate, twenty for the rest (`getPressedTime`).
fn plate_delay(state: BlockState) -> i32 {
    if is_class(state, BlockClass::WeightedPressurePlateBlock) {
        10
    } else {
        20
    }
}

/// `WeightedPressurePlateBlock.maxWeight`: 15 for the light (gold) plate and 150 for
/// the heavy (iron) one, as the game's `Blocks` makes them.
fn plate_weight(state: BlockState) -> usize {
    if same_block(state, blocks::HEAVY_WEIGHTED_PRESSURE_PLATE) {
        150
    } else {
        15
    }
}

/// A plate's click as it goes down or comes up, by its block set
/// (`BlockSetType.pressurePlateClickOn` and `pressurePlateClickOff`): metal for iron
/// and gold, stone for stone and polished blackstone, and the woods'.
fn plate_sound(state: BlockState, on: bool) -> i32 {
    use sounds as s;
    let name = state.block().map_or("", |block| block.name);
    let (on_sound, off_sound) = if is_class(state, BlockClass::WeightedPressurePlateBlock) {
        (
            s::BLOCK_METAL_PRESSURE_PLATE_CLICK_ON,
            s::BLOCK_METAL_PRESSURE_PLATE_CLICK_OFF,
        )
    } else if name.contains("stone") {
        (
            s::BLOCK_STONE_PRESSURE_PLATE_CLICK_ON,
            s::BLOCK_STONE_PRESSURE_PLATE_CLICK_OFF,
        )
    } else if name.contains("cherry") {
        (
            s::BLOCK_CHERRY_WOOD_PRESSURE_PLATE_CLICK_ON,
            s::BLOCK_CHERRY_WOOD_PRESSURE_PLATE_CLICK_OFF,
        )
    } else if name.contains("bamboo") {
        (
            s::BLOCK_BAMBOO_WOOD_PRESSURE_PLATE_CLICK_ON,
            s::BLOCK_BAMBOO_WOOD_PRESSURE_PLATE_CLICK_OFF,
        )
    } else if name.contains("crimson") || name.contains("warped") {
        (
            s::BLOCK_NETHER_WOOD_PRESSURE_PLATE_CLICK_ON,
            s::BLOCK_NETHER_WOOD_PRESSURE_PLATE_CLICK_OFF,
        )
    } else {
        (
            s::BLOCK_WOODEN_PRESSURE_PLATE_CLICK_ON,
            s::BLOCK_WOODEN_PRESSURE_PLATE_CLICK_OFF,
        )
    };
    if on { on_sound } else { off_sound }
}

/// A block's sound at the middle of the block, heard by everyone near
/// (`playSound(null, …)`).
fn play(pos: BlockPos, sound: i32, volume: f32, pitch: f32, output: &mut TickOutput) {
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
        except: None,
    });
}
