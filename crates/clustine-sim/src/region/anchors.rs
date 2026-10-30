//! Respawn anchors (`RespawnAnchorBlock`; `docs/groundwork/p-brief.md`, section 5.4):
//! charged by glowstone, from a hand or a dispenser, in every dimension; used with a
//! charge, it sets the spawn where anchors work (the Nether) and explodes elsewhere.
//!
//! Named differences: an anchor used across a line by another region's player does
//! nothing, as a bed does (`sleep.rs`); every player is creative until survival, so the
//! glowstone a player charges with is kept; respawning at an anchor, which spends a
//! charge, comes with the respawn after a death (the End's milestone); where the region
//! has no entity key left, an anchor that explodes is removed without its explosion.

use clustine_blocks::data::{is_class, number, set};
use clustine_blocks::fluid::{FluidState, Kind};
use clustine_data::{BlockClass, BlockState, Face, blocks, items, sounds};
use clustine_world::{BlockPos, PlayerId};

use super::Region;
use super::vibrations::{Context, Vibration};
use crate::api::{Hand, PlayerEvent, Respawn, TickOutput};

/// The most charges an anchor holds (`RespawnAnchorBlock.MAX_CHARGES`).
const MAX_CHARGES: u8 = 4;

/// `Player.hasInfiniteMaterials`: every player is creative until survival.
const fn has_infinite_materials(_player: PlayerId) -> bool {
    true
}

/// `isWaterThatWouldFlow` (d RespawnAnchorBlock.txt 174–213) of `fluid`, whose block
/// below holds `below`: water that is a source, or flowing water of an amount of at
/// least 2 with no water under it.
fn water_that_would_flow(fluid: FluidState, below: FluidState) -> bool {
    if fluid.kind() != Some(Kind::Water) {
        return false;
    }
    if fluid.is_source() {
        return true;
    }
    if fluid.amount < 2 {
        return false;
    }
    below.kind() != Some(Kind::Water)
}

impl Region {
    /// `RespawnAnchorBlock.charge` (d RespawnAnchorBlock.txt 282–331) by nobody: a
    /// dispenser's. The anchor `state` at `pos` is not full.
    pub(super) fn charge_anchor(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        self.charge_anchor_by(pos, state, None, output);
    }

    /// `RespawnAnchorBlock.charge(entity, …)`: one charge more with every update, the
    /// game event `block_change` of `player` (if a player charged it) with the charged
    /// state, and its sound, heard by all.
    fn charge_anchor_by(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        player: Option<PlayerId>,
        output: &mut TickOutput,
    ) {
        let charge = number(state, "charges").unwrap_or(0);
        let charged = set(state, "charges", &(charge + 1).to_string());
        self.set_block(pos, charged, output);
        let source = player.and_then(|player| self.player_source(player));
        self.vibrate_at(
            Vibration::BLOCK_CHANGE,
            pos,
            Context::of(source, Some(charged)),
        );
        Region::block_sound(pos, sounds::BLOCK_RESPAWN_ANCHOR_CHARGE, 1.0, 1.0, output);
    }

    /// A player's click that the anchor at `pos` took (`placement::anchor::takes`):
    /// `useItemOn` with glowstone in the main hand where it is not full (one charge,
    /// the glowstone used unless the player is creative), else `useWithoutItem`
    /// (d RespawnAnchorBlock.txt 31–152): nothing for an empty anchor; where anchors
    /// work (`respawn_anchor_works`, the Nether's), the player's respawn set here
    /// unless it already is, with the message and the sound; anywhere else the anchor
    /// explodes.
    pub(super) fn use_anchor(
        &mut self,
        pos: BlockPos,
        face: Face,
        player: PlayerId,
        output: &mut TickOutput,
    ) {
        let _ = face;
        if !self.players.contains_key(&player) {
            return;
        }
        let Some(state) = self.own_block(pos) else {
            return;
        };
        if !is_class(state, BlockClass::RespawnAnchorBlock) {
            return;
        }
        let charges = number(state, "charges").unwrap_or(0);
        let held = self.in_hand(player, Hand::Main);
        if let Some(stack) = held.filter(|stack| stack.item == items::GLOWSTONE)
            && charges < MAX_CHARGES
        {
            self.charge_anchor_by(pos, state, Some(player), output);
            // `stack.consume(1, player)`.
            if !has_infinite_materials(player) {
                let left = (stack.count > 1).then(|| stack.with_count(stack.count - 1));
                self.set_selected_stack(player, left, output);
            }
            return;
        }
        if charges == 0 {
            return;
        }
        if self.config.dimension.rules().respawn_anchor_works {
            self.set_anchor_spawn(pos, player, output);
        } else {
            self.explode_anchor(pos, output);
        }
    }

    /// The respawn of `player` set at the anchor at `pos` (d RespawnAnchorBlock.txt
    /// 85–144): `RespawnConfig(of(level, pos, 0, 0), forced = false)`, unless the one
    /// they have is at the same `GlobalPos`; then `block.minecraft.set_spawn` in their
    /// chat (`setRespawnPosition(…, true)`, d ServerPlayer.txt 4940–4957) and the
    /// anchor's `set_spawn` sound at its centre, heard by all.
    fn set_anchor_spawn(&mut self, pos: BlockPos, player: PlayerId, output: &mut TickOutput) {
        let dimension = self.config.dimension;
        let Some(state) = self.players.get_mut(&player) else {
            return;
        };
        if state
            .respawn
            .is_some_and(|old| old.pos == pos && old.dimension == dimension)
        {
            return;
        }
        state.respawn = Some(Respawn {
            pos,
            yaw: 0.0,
            pitch: 0.0,
            dimension,
        });
        self.journal.players.insert(player);
        output.player_events.push((
            player,
            PlayerEvent::Message {
                key: "block.minecraft.set_spawn".to_owned(),
                args: Vec::new(),
                overlay: false,
            },
        ));
        Region::block_sound(
            pos,
            sounds::BLOCK_RESPAWN_ANCHOR_SET_SPAWN,
            1.0,
            1.0,
            output,
        );
    }

    /// `RespawnAnchorBlock.explode` (d RespawnAnchorBlock.txt 215–270): the anchor
    /// removed with every update, then an explosion of no entity at its centre, power
    /// 5 with fire, hurting nobody until survival; where water that would flow is beside
    /// it horizontally, or water is above it, its own position resists as water does
    /// (`RespawnAnchorBlock$1`), which shields what is behind the water from the blast.
    fn explode_anchor(&mut self, pos: BlockPos, output: &mut TickOutput) {
        // `removeBlock(pos, false)`: an anchor holds no fluid, so it leaves air.
        self.set_block(pos, blocks::AIR, output);
        let fluid_at = |at: BlockPos| FluidState::of(self.block(at).unwrap_or(blocks::AIR));
        let beside = [(0, -1), (1, 0), (0, 1), (-1, 0)]
            .into_iter()
            .any(|(dx, dz)| {
                let at = pos.offset(dx, 0, dz);
                water_that_would_flow(fluid_at(at), fluid_at(at.offset(0, -1, 0)))
            });
        let above = fluid_at(pos.offset(0, 1, 0)).kind() == Some(Kind::Water);
        let resistance_at = (beside || above).then(|| {
            let water = super::explosions::resistance(blocks::WATER)
                .expect("water has a resistance to explosions");
            (pos, water)
        });
        let centre = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 0.5,
            f64::from(pos.z) + 0.5,
        ];
        self.bad_respawn_point_explosion(centre, resistance_at, output);
    }

    /// The stack in `player`'s selected hotbar slot set to `stack`, and their client
    /// told.
    fn set_selected_stack(
        &mut self,
        player: PlayerId,
        stack: Option<clustine_world::ItemStack>,
        output: &mut TickOutput,
    ) {
        let Some(state) = self.players.get_mut(&player) else {
            return;
        };
        let slot = usize::from(state.selected_slot);
        if !state.inventory.set(slot, stack.clone()) {
            return;
        }
        self.journal.players.insert(player);
        let slot = u8::try_from(slot).expect("a hotbar slot fits a byte");
        output
            .player_events
            .push((player, PlayerEvent::InventorySlot { slot, stack }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn water(amount: u8, falling: bool) -> FluidState {
        if amount == 8 && !falling {
            FluidState::source(Kind::Water, false)
        } else {
            FluidState::flowing(Kind::Water, amount, falling)
        }
    }

    #[test]
    fn a_source_of_water_would_flow_whatever_is_under_it() {
        assert!(water_that_would_flow(water(8, false), water(8, false)));
        assert!(water_that_would_flow(water(8, false), FluidState::EMPTY));
    }

    #[test]
    fn flowing_water_would_flow_from_an_amount_of_two_with_no_water_under_it() {
        assert!(!water_that_would_flow(water(1, false), FluidState::EMPTY));
        for amount in 2..8 {
            assert!(water_that_would_flow(
                water(amount, false),
                FluidState::EMPTY
            ));
            assert!(!water_that_would_flow(
                water(amount, false),
                water(8, false)
            ));
            assert!(!water_that_would_flow(water(amount, false), water(3, true)));
        }
    }

    #[test]
    fn lava_and_nothing_are_no_water_that_would_flow() {
        let lava = FluidState::source(Kind::Lava, false);
        assert!(!water_that_would_flow(lava, FluidState::EMPTY));
        assert!(!water_that_would_flow(FluidState::EMPTY, FluidState::EMPTY));
    }
}
