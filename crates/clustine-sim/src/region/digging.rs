//! A player's dig (`docs/groundwork/v-plan.md`, sections 2.6 and 4.5, step V2): the
//! game's `ServerPlayerGameMode.handleBlockBreakAction`, its `tick` and its
//! `destroyBlock` for a player who does not build instantly, read from n5
//! `ServerPlayerGameMode` (the offsets below are its bytecode's).
//!
//! - **The count.** The game counts each player's ticks (`gameTicks`, one more at the
//!   start of each `tick`, 0 for a new `ServerPlayerGameMode`). Clustine keeps it as the
//!   region's tick less an origin, so that a state changes only when the dig does: the
//!   inputs of a tick read the count of the tick before, as the game's packets are
//!   handled between two ticks, and the dig's tick reads the tick's own. A transfer
//!   carries the count, a merge moves the origin onto the survivor's ticks, and a login,
//!   a respawn and a way out of the End begin it again (`PlayerList.respawn` makes a new
//!   `ServerPlayer`); a change of level by a portal keeps it, as the game keeps its
//!   player. Named: the ticks a hand-over spends on the way (where the next region takes
//!   the player in later than the tick after) are not counted, as the attack strength's
//!   and the time since rest's are not; a dig across such a hand-over is that much
//!   slower.
//! - **The actions** (@0-700): beyond reach (+1) only an abort of a dig under way is
//!   taken, above the level's top the client is shown the block. START destroys at once
//!   for a player who builds instantly (the creative break, as before), shows the block
//!   to one whose mode restricts block actions (adventure, spectator), else begins the
//!   dig at the count: the block's `attack`, and its progress `p`; where `p` is 1 or
//!   more the block breaks at once (an insta-mine), else the dig's stage is sent to the
//!   others. STOP at the dig's block breaks it where `p` times the ticks since the start
//!   plus one is 0.7 or more, else leaves a **delayed destroy** that the tick finishes;
//!   at another block it does nothing. ABORT stops the dig and takes its cracks away.
//!   CHANGE_DESTROY_DIRECTION turns the face the level events name. Every action is
//!   acknowledged after it acted (`handlePlayerAction @299-307`).
//! - **The tick** (@0-197), after the inputs with the player's other parts: a delayed
//!   destroy goes on to its break by the game's quirk of counting the **start** as the
//!   ticks (`incrementDestroyProgress(state, pos, delayedTickStart)`, so that it
//!   finishes at once for any player who has played more than a moment); a dig sends
//!   its stage where it changed and the level event 2020 every fourth tick, 2019 the
//!   others, with the face. The progress is the destroy speed **now** times the ticks
//!   plus one, never summed: a tool changed in mid-dig acts on the whole dig.
//! - **The break** (`destroyBlock @0-273`): the held item may forbid it (a debug stick),
//!   the block goes (`playerWillDestroy`, `removeBlock`), the held tool wears where the
//!   block's hardness is not 0 (`ItemStack.mineBlock`, whether or not anything was
//!   removed or the tool is right; shears wherever the block is not in `#fire`,
//!   `ShearsItem.mineBlock`), and only where the block was removed with the right tool
//!   its `playerDestroy` drops it by its table with the tool and spawns what comes
//!   after (an ore's experience, an infested block's silverfish), with the stack as it
//!   was before the wear.
//!
//! Named differences: spawn protection and `mayInteract` (no spawn protection here);
//! `EnchantmentHelper.onHitBlock` (no enchantment of the game acts on a hit block
//! without a target entity); a dragon egg's teleport on `attack`; adventure's
//! `can_break` (no stack carries it, so an adventure player breaks nothing); a debug
//! stick's own interaction; the statistics (`BLOCK_MINED`, `ITEM_USED`) and the
//! exhaustion of 0.005 (V4); a tripwire cut with shears, a beehive's bees and a turtle
//! egg's eggs when broken by a player (their `playerWillDestroy` and `playerDestroy`).
//!
//! **Across a line** (V2c): the player's region judges the dig on its copy of the block
//! (the chunks around its viewers), as it judges a click. A survival player's break of a
//! block another region holds is split where the game's knowledge is: the player's
//! region asks `canDestroyBlock` and the mode, angers the piglins near the player (who
//! are its own or its copies'), wears the held tool by the block of its copy, and sends
//! a [`RemoteStep::BreakWith`] with the stack as it was; the holder breaks the block if
//! it is still the same block, and drops it with that stack. A creative player's stays a
//! `RemoteStep::Break`. A delayed destroy's step carries the highest action the player's
//! region handled in the tick, which it then leaves to the holder's answer to
//! acknowledge, so that the client keeps its guess until the block is gone; START's
//! `attack` goes as a [`RemoteStep::Attack`] that nobody answers. Named: where the
//! holder's block changed on the way (another player broke it), the game would wear the
//! tool by what is there; here by the copy. The choice is
//! [`Region::break_for_player`]'s alone.

use clustine_blocks::data::{flag, in_tag, is_class, same_block};
use clustine_data::{BlockClass, BlockState, ItemComponents, blocks, items};
use clustine_world::{BlockPos, ItemStack, PlayerId};

use super::Region;
use super::breaking::{Digger, has_correct_tool_for_drops};
use super::vibrations::Source;
use crate::api::{
    DestroyAction, Durable, Face, PlayerEvent, RegionEvent, RemoteAction, RemoteStep, TickOutput,
};
use crate::mobs::body::effects::Effect;

/// The level events a dig plays every fourth tick and on the others (`tick` @149-165),
/// which clients show as the cracking block's particles and sound.
const DIG_EVERY_FOURTH: i32 = 2020;
const DIG: i32 = 2019;

/// What a STOP needs of the progress to break (@603, `0.7f`).
const STOP_BREAKS_AT: f32 = 0.7;

/// The player's `gameTicks` at the end of the region's tick `tick`, of the origin
/// `origin`: the game's `int`, which wraps.
pub(super) fn game_ticks(origin: i64, tick: u64) -> i32 {
    (i128::from(tick) - i128::from(origin)) as i32
}

/// The origin that makes the count `game_ticks` at the end of the tick before `tick`:
/// what the inputs of `tick` read, as a player who arrives or begins in `tick` has it.
pub(super) fn origin_after(tick: u64, game_ticks: i32) -> i64 {
    i64::try_from(i128::from(tick) - 1 - i128::from(game_ticks)).unwrap_or(i64::MIN)
}

/// `Direction.ordinal` of a face, which the level events of a dig carry.
fn ordinal(face: Face) -> i32 {
    match face {
        Face::Bottom => 0,
        Face::Top => 1,
        Face::North => 2,
        Face::South => 3,
        Face::West => 4,
        Face::East => 5,
    }
}

/// `Item.canDestroyBlock` and its overrides for a player who does not build instantly:
/// a tool's `can_destroy_blocks_in_creative` asks only in creative, so only the debug
/// stick refuses (`DebugStickItem.canDestroyBlock`, whose own interaction is named in
/// the module).
fn can_destroy_block(held: Option<&ItemStack>) -> bool {
    held.is_none_or(|stack| stack.item != items::DEBUG_STICK)
}

/// Where a player's break was done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Broke {
    /// Here: `destroyBlock`'s answer, false where the block was kept.
    Here(bool),
    /// By the region that holds the block, which acknowledges the action.
    Across,
    /// Not at all: the player has gone.
    NotDone,
}

impl Region {
    /// The player `id`'s `gameTicks` as the inputs of this tick read it: the count the
    /// tick before ended with.
    fn input_game_ticks(&self, id: PlayerId) -> Option<i32> {
        let player = self.players.get(&id)?;
        Some(game_ticks(player.game_origin, self.tick.saturating_sub(1)))
    }

    /// `ServerPlayerGameMode.handleBlockBreakAction` of the player `id`, then the
    /// acknowledgement of `sequence` unless the region that holds the block gives it.
    pub(super) fn destroy_input(
        &mut self,
        id: PlayerId,
        action: DestroyAction,
        position: BlockPos,
        face: Face,
        sequence: i32,
        output: &mut TickOutput,
    ) {
        if self.block_break_action(id, action, position, face, sequence, output)
            && let Some(player) = self.players.get_mut(&id)
        {
            player.acknowledge(sequence);
        }
    }

    /// The action itself: whether it is acknowledged here.
    fn block_break_action(
        &mut self,
        id: PlayerId,
        action: DestroyAction,
        position: BlockPos,
        face: Face,
        sequence: i32,
        output: &mut TickOutput,
    ) -> bool {
        let Some(player) = self.players.get(&id) else {
            return false;
        };
        let destroying = player.dig.is_destroying;
        // @0-43: beyond reach only an abort of a dig under way is taken.
        if !player.can_reach(position) {
            if destroying && action == DestroyAction::Abort {
                self.abort_destroy_block(id, position, output);
            }
            return true;
        }
        // @44-90: above the level's top the client is shown the block.
        let rules = self.config.dimension.rules();
        if position.y > rules.min_y + rules.height as i32 - 1 {
            output.player_events.push((
                id,
                PlayerEvent::BlockShown {
                    position,
                    state: blocks::AIR,
                },
            ));
            return true;
        }
        match action {
            DestroyAction::Start => self.start_destroying(id, position, face, sequence, output),
            // @515-526.
            DestroyAction::ChangeDirection => {
                if let Some(player) = self.players.get_mut(&id) {
                    player.dig.direction = face;
                }
                true
            }
            DestroyAction::Stop => self.stop_destroying(id, position, sequence, output),
            // @686-697.
            DestroyAction::Abort => {
                self.abort_destroy_block(id, position, output);
                true
            }
        }
    }

    /// START (@91-511).
    fn start_destroying(
        &mut self,
        id: PlayerId,
        position: BlockPos,
        face: Face,
        sequence: i32,
        output: &mut TickOutput,
    ) -> bool {
        let Some(player) = self.players.get(&id) else {
            return false;
        };
        let abilities = player.mode.current.abilities();
        // @193-216: a player who builds instantly destroys at once.
        if abilities.instabuild {
            return self.destroy_and_ack(id, position, Some(sequence), output);
        }
        // @217-273, `Player.blockActionRestricted`: adventure's `can_break` is not
        // built (named), so a mode that may not build breaks nothing.
        if !abilities.may_build {
            self.show_block(id, position, output);
            return true;
        }
        let Some(count) = self.input_game_ticks(id) else {
            return false;
        };
        // @274-279: the start is set whatever the block.
        if let Some(player) = self.players.get_mut(&id) {
            player.dig.start = count;
        }
        let state = self.block(position).unwrap_or(blocks::AIR);
        let air = state.props().is_air();
        let mut progress = 1.0;
        if !air {
            // `EnchantmentHelper.onHitBlock` does nothing here (named); then the block's
            // `attack` and the progress.
            self.attack_block(id, position, state, output);
            progress = self.destroy_progress(id, state);
        }
        // @373-398: an insta-mine.
        if !air && progress >= 1.0 {
            return self.destroy_and_ack(id, position, Some(sequence), output);
        }
        // @401-448: a dig under way that the client gave up for this one, which the
        // client broke at once where the server did not: it is shown the old block.
        let Some(player) = self.players.get(&id) else {
            return false;
        };
        if player.dig.is_destroying {
            let old = player.dig.destroy_pos;
            self.show_block(id, old, output);
        }
        let stage = (progress * 10.0) as i32;
        if let Some(player) = self.players.get_mut(&id) {
            player.dig.is_destroying = true;
            player.dig.destroy_pos = position;
            player.dig.direction = face;
        }
        self.destroy_block_progress(id, position, stage, output);
        if let Some(player) = self.players.get_mut(&id) {
            player.dig.last_sent = stage;
        }
        true
    }

    /// STOP (@529-683).
    fn stop_destroying(
        &mut self,
        id: PlayerId,
        position: BlockPos,
        sequence: i32,
        output: &mut TickOutput,
    ) -> bool {
        let Some(count) = self.input_game_ticks(id) else {
            return false;
        };
        let Some(dig) = self.players.get(&id).map(|player| player.dig) else {
            return false;
        };
        // A STOP at another block only logs (@538-544).
        if position != dig.destroy_pos {
            return true;
        }
        let ticks = count.wrapping_sub(dig.start);
        let state = self.block(position).unwrap_or(blocks::AIR);
        if state.props().is_air() {
            return true;
        }
        let progress = self.destroy_progress(id, state) * ticks.wrapping_add(1) as f32;
        if progress >= STOP_BREAKS_AT {
            if let Some(player) = self.players.get_mut(&id) {
                player.dig.is_destroying = false;
            }
            self.destroy_block_progress(id, position, -1, output);
            return self.destroy_and_ack(id, position, Some(sequence), output);
        }
        // @642-669: too early, a delayed destroy, unless one is waiting already (and then
        // the dig goes on, as the game's does).
        if !dig.has_delayed
            && let Some(player) = self.players.get_mut(&id)
        {
            player.dig.is_destroying = false;
            player.dig.has_delayed = true;
            player.dig.delayed_pos = position;
            player.dig.delayed_start = dig.start;
        }
        true
    }

    /// `abortDestroyBlock` (@0-87): the dig stops, and the others are told its cracks
    /// are gone, at the dig's block too where the client names another.
    fn abort_destroy_block(&mut self, id: PlayerId, position: BlockPos, output: &mut TickOutput) {
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        player.dig.is_destroying = false;
        let destroying = player.dig.destroy_pos;
        if destroying != position {
            self.destroy_block_progress(id, destroying, -1, output);
        }
        self.destroy_block_progress(id, position, -1, output);
    }

    /// `destroyAndAck` (@0-53): the break, and the block shown to the client where it was
    /// kept. Whether the action is acknowledged here.
    fn destroy_and_ack(
        &mut self,
        id: PlayerId,
        position: BlockPos,
        sequence: Option<i32>,
        output: &mut TickOutput,
    ) -> bool {
        match self.break_for_player(id, position, sequence, output) {
            Broke::Here(true) => true,
            Broke::Here(false) => {
                self.show_block(id, position, output);
                true
            }
            Broke::Across => false,
            Broke::NotDone => true,
        }
    }

    /// Where the break of the block at `position` for the player `id` is done: here where
    /// the region holds the block, else by the region that does, as a step that
    /// acknowledges `sequence`, the action's. A break that answers no action (a delayed
    /// destroy) passes on the highest action handled in the tick instead, if there is
    /// one. The one place that chooses.
    pub(super) fn break_for_player(
        &mut self,
        id: PlayerId,
        position: BlockPos,
        sequence: Option<i32>,
        output: &mut TickOutput,
    ) -> Broke {
        if self.land.holds(position.chunk()) {
            return Broke::Here(self.destroy_block_as(id, position, output));
        }
        let Some(player) = self.players.get(&id) else {
            return Broke::NotDone;
        };
        let held = player.held();
        let sneaking = player.keys.sneak;
        let abilities = player.mode.current.abilities();
        let step = if abilities.instabuild {
            RemoteStep::Break { position }
        } else {
            // What `destroyBlock` asks before anything changes: the block is shown and
            // the action acknowledged here.
            let Some(expected) = self.block(position) else {
                return Broke::Here(false);
            };
            if !can_destroy_block(held.as_ref()) || !abilities.may_build {
                return Broke::Here(false);
            }
            // `Block.playerWillDestroy`'s piglins, near the player and so here.
            if in_tag(expected, "minecraft:guarded_by_piglins") {
                self.anger_nearby_piglins(id, false, output);
            }
            self.mine_block(id, expected, output);
            RemoteStep::BreakWith {
                position,
                expected,
                tool: held,
                sneaking,
            }
        };
        let sequence = match sequence {
            Some(sequence) => sequence,
            // The client was told nothing of this break, but it guessed it at its STOP:
            // what the tick handled is acknowledged when the holder has answered.
            None => self
                .players
                .get_mut(&id)
                .and_then(|player| player.handled_sequence.take())
                .unwrap_or(RemoteAction::UNANSWERED),
        };
        self.send_remote(id, sequence, step, output);
        Broke::Across
    }

    /// Sends `step` of the player `id`'s action `sequence` to the region believed to hold
    /// the block it concerns, through the player's edge.
    fn send_remote(
        &mut self,
        id: PlayerId,
        sequence: i32,
        step: RemoteStep,
        output: &mut TickOutput,
    ) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let (edge, entity) = (player.edge, player.entity_id);
        let to = self.land.knowledge(step.concerns().chunk()).holder();
        let action = RemoteAction {
            player: id,
            entity,
            sequence,
            step,
        };
        self.send(edge, Durable::Remote { action, to }, output);
    }

    /// `ItemStack.mineBlock` of what the player `id` holds, on breaking `state`: a tool
    /// with a `damage_per_block` above 0 wears by it where the block's hardness is not 0
    /// (`Item.mineBlock` @0-61), shears wherever the block is not in `#fire`
    /// (`ShearsItem.mineBlock` @0-59).
    fn mine_block(&mut self, id: PlayerId, state: BlockState, output: &mut TickOutput) {
        let Some(held) = self.players.get(&id).and_then(|player| player.held()) else {
            return;
        };
        let Some(tool) = ItemComponents::of(held.item).tool.as_ref() else {
            return;
        };
        let wears = if held.item == items::SHEARS {
            !in_tag(state, "minecraft:fire")
        } else {
            state
                .breaking()
                .is_some_and(|breaking| breaking.hardness != 0.0)
        };
        if wears && tool.damage_per_block > 0 {
            let amount = i32::try_from(tool.damage_per_block).unwrap_or(i32::MAX);
            self.hurt_and_break_in_hand(id, crate::api::Hand::Main, amount, output);
        }
    }

    /// A survival player's break of another region, [`RemoteStep::BreakWith`]: the part
    /// of `destroyBlock` that needs the block, by the player whose entity `source` names.
    pub(super) fn break_from_afar(
        &mut self,
        id: PlayerId,
        source: Source,
        position: BlockPos,
        expected: BlockState,
        tool: Option<&ItemStack>,
        output: &mut TickOutput,
    ) {
        let Some(state) = self.own_block(position) else {
            return;
        };
        if !same_block(state, expected) {
            return;
        }
        let correct = has_correct_tool_for_drops(tool, state);
        self.player_will_destroy_from(
            position,
            state,
            Some(id),
            Some(source),
            Some((tool, correct)),
            output,
        );
        let removed = self.set_block_updated(
            position,
            super::updates::fluid_block(state),
            super::updates::UPDATE_ALL,
            super::updates::UPDATE_LIMIT,
            output,
        );
        if removed && correct {
            self.player_destroy(position, state, tool, output);
        }
    }

    /// `ServerPlayerGameMode.destroyBlock` (@0-273) of the block at `position`, which
    /// the region holds, by the player `id`: a creative player's break as it was, a
    /// survival player's as the module says.
    fn destroy_block_as(
        &mut self,
        id: PlayerId,
        position: BlockPos,
        output: &mut TickOutput,
    ) -> bool {
        let Some(player) = self.players.get(&id) else {
            return false;
        };
        let held = player.held();
        let abilities = player.mode.current.abilities();
        if abilities.instabuild {
            self.break_by_player(position, Some(id), output);
            return true;
        }
        let Some(state) = self.own_block(position) else {
            return false;
        };
        if !can_destroy_block(held.as_ref()) {
            return false;
        }
        // The game-master blocks (command, structure, jigsaw and test blocks) cannot be
        // broken without the permission: their hardness of -1 keeps a survival player
        // from ever getting here. @83-103: `blockActionRestricted`, asked at START.
        if !abilities.may_build {
            return false;
        }
        let correct = has_correct_tool_for_drops(held.as_ref(), state);
        // @104-119: `playerWillDestroy`, with the tool.
        self.player_will_destroy(
            position,
            state,
            Some(id),
            Some((held.as_ref(), correct)),
            output,
        );
        // @121-130: `removeBlock`, to the block's fluid. `Block.destroy` does nothing
        // that Clustine has.
        let removed = self.set_block_updated(
            position,
            super::updates::fluid_block(state),
            super::updates::UPDATE_ALL,
            super::updates::UPDATE_LIMIT,
            output,
        );
        // @210-241: the stack as it was is the tool of the drops; the held one wears.
        self.mine_block(id, state, output);
        // @244-269: `playerDestroy` only where the block was removed with the right tool.
        if removed && correct {
            self.player_destroy(position, state, held.as_ref(), output);
        }
        true
    }

    /// `Block.playerDestroy` of `state` broken at `position` with `tool`, and the
    /// overrides Clustine has: the drops with the tool and what comes after them; a tall
    /// plant's (`DoublePlantBlock.playerDestroy`, dropping as air) dropped already, and
    /// ice melts to water without Silk Touch (`IceBlock.playerDestroy`, n8 @15-90).
    fn player_destroy(
        &mut self,
        position: BlockPos,
        state: BlockState,
        tool: Option<&ItemStack>,
        output: &mut TickOutput,
    ) {
        if !is_class(state, BlockClass::DoublePlantBlock) {
            self.drop_resources_by_player(state, position, tool, output);
        }
        let prevents_melting = tool.is_some_and(|tool| {
            crate::items::enchantments::level(tool, "minecraft:silk_touch") > 0
        });
        self.ice_broken_by_player(position, state, !prevents_melting, output);
    }

    /// `BlockState.attack` of the block at `position` by the player `id` (START @340-351):
    /// a note block plays (`NoteBlock.attack`), redstone ore lights
    /// (`RedStoneOreBlock.attack`). A dragon egg's teleport is named. A block another
    /// region holds is attacked there, if the copy says it is one that does something.
    fn attack_block(
        &mut self,
        id: PlayerId,
        position: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        if self.land.holds(position.chunk()) {
            self.attack_here(position, state, output);
        } else if is_class(state, BlockClass::NoteBlock)
            || is_class(state, BlockClass::RedStoneOreBlock)
        {
            let step = RemoteStep::Attack { position };
            self.send_remote(id, RemoteAction::UNANSWERED, step, output);
        }
    }

    /// The attack of `state`, a block the region holds at `position`.
    pub(super) fn attack_here(
        &mut self,
        position: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        if is_class(state, BlockClass::NoteBlock) {
            self.play_note(position, state);
        } else if is_class(state, BlockClass::RedStoneOreBlock) && !flag(state, "lit") {
            self.light_redstone_ore(position, state, output);
        }
    }

    /// `BlockBehaviour.getDestroyProgress` of `state` for the player `id` as they are now:
    /// what they hold, their attributes, their effects, their eyes and their feet.
    pub(super) fn destroy_progress(&self, id: PlayerId, state: BlockState) -> f32 {
        let Some(player) = self.players.get(&id) else {
            return 0.0;
        };
        let held = player.held();
        let attribute = |attribute: i32| self.player_attribute(id, attribute);
        let dig_speed = [Effect::HASTE, Effect::CONDUIT_POWER]
            .into_iter()
            .filter_map(|effect| player.effects.amplifier(effect))
            .max();
        let digger = Digger {
            held: held.as_ref(),
            mining_efficiency: attribute(clustine_data::attributes::MINING_EFFICIENCY),
            dig_speed_amplifier: dig_speed.map(i32::from),
            mining_fatigue: player
                .effects
                .amplifier(Effect::MINING_FATIGUE)
                .map(i32::from),
            block_break_speed: attribute(clustine_data::attributes::BLOCK_BREAK_SPEED),
            submerged_mining_speed: player
                .eyes
                .in_water()
                .then(|| attribute(clustine_data::attributes::SUBMERGED_MINING_SPEED)),
            on_ground: player.pose.on_ground,
        };
        digger.destroy_progress(state)
    }

    /// `incrementDestroyProgress` (@0-65): the progress of `ticks` (plus one) at the
    /// speed of now, and its stage sent where it changed.
    fn increment_destroy_progress(
        &mut self,
        id: PlayerId,
        state: BlockState,
        position: BlockPos,
        ticks: i32,
        output: &mut TickOutput,
    ) -> f32 {
        let progress = self.destroy_progress(id, state) * ticks.wrapping_add(1) as f32;
        let stage = (progress * 10.0) as i32;
        let Some(player) = self.players.get_mut(&id) else {
            return progress;
        };
        if stage != player.dig.last_sent {
            player.dig.last_sent = stage;
            self.journal.players.insert(id);
            self.destroy_block_progress(id, position, stage, output);
        }
        progress
    }

    /// `ServerLevel.destroyBlockProgress(id, pos, stage)`: the others see the cracks.
    fn destroy_block_progress(
        &self,
        id: PlayerId,
        position: BlockPos,
        stage: i32,
        output: &mut TickOutput,
    ) {
        if let Some(player) = self.players.get(&id) {
            let event = RegionEvent::BlockDestruction {
                breaker: player.entity_id,
                position,
                stage,
            };
            output.events.push(self.seen_near(id, position, event));
        }
    }

    /// `event` of the player `id`'s dig at `position`: as it is where the region holds
    /// the block, else filed under the player's chunk, since the edges watch the block's
    /// chunk at the region that holds it and would not hear of it from here.
    fn seen_near(&self, id: PlayerId, position: BlockPos, event: RegionEvent) -> RegionEvent {
        match self.players.get(&id) {
            Some(player) if !self.land.holds(position.chunk()) => RegionEvent::Near {
                chunk: player.chunk(),
                event: Box::new(event),
            },
            _ => event,
        }
    }

    /// The block at `position` shown to the player `id` alone
    /// (`ClientboundBlockUpdatePacket`), as far as the region can read it.
    fn show_block(&self, id: PlayerId, position: BlockPos, output: &mut TickOutput) {
        if let Some(state) = self.block(position) {
            output
                .player_events
                .push((id, PlayerEvent::BlockShown { position, state }));
        }
    }

    /// `ServerPlayerGameMode.tick` (@0-197) of every player, after the inputs.
    pub(super) fn dig_tick(&mut self, output: &mut TickOutput) {
        let ids: Vec<PlayerId> = self.players.keys().copied().collect();
        for id in ids {
            let Some(player) = self.players.get(&id) else {
                continue;
            };
            let count = game_ticks(player.game_origin, self.tick);
            let dig = player.dig;
            if dig.has_delayed {
                let state = self.block(dig.delayed_pos).unwrap_or(blocks::AIR);
                if state.props().is_air() {
                    self.change_dig(id, |dig| dig.has_delayed = false);
                    continue;
                }
                // The game's quirk: the start as the ticks.
                let progress = self.increment_destroy_progress(
                    id,
                    state,
                    dig.delayed_pos,
                    dig.delayed_start,
                    output,
                );
                if progress >= 1.0 {
                    self.change_dig(id, |dig| dig.has_delayed = false);
                    self.break_for_player(id, dig.delayed_pos, None, output);
                }
            } else if dig.is_destroying {
                let state = self.block(dig.destroy_pos).unwrap_or(blocks::AIR);
                if state.props().is_air() {
                    self.destroy_block_progress(id, dig.destroy_pos, -1, output);
                    self.change_dig(id, |dig| {
                        dig.last_sent = -1;
                        dig.is_destroying = false;
                    });
                    continue;
                }
                let ticks = count.wrapping_sub(dig.start);
                let kind = if ticks % 4 == 0 {
                    DIG_EVERY_FOURTH
                } else {
                    DIG
                };
                self.increment_destroy_progress(id, state, dig.destroy_pos, ticks, output);
                let event = RegionEvent::LevelEvent {
                    kind,
                    position: dig.destroy_pos,
                    data: ordinal(dig.direction),
                };
                output
                    .events
                    .push(self.seen_near(id, dig.destroy_pos, event));
            }
        }
    }

    /// Changes the dig of the player `id`, whose state then changed.
    fn change_dig(&mut self, id: PlayerId, change: impl FnOnce(&mut crate::state::Dig)) {
        if let Some(player) = self.players.get_mut(&id) {
            change(&mut player.dig);
            self.journal.players.insert(id);
        }
    }
}
