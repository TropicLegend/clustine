//! The respawn, the credits and the way out of the End (`docs/groundwork/p-plan.md`,
//! section 19.7; the game's rules in `docs/groundwork/p-respawn-facts.md`).
//!
//! - **One path.** The respawn after a death (`Way::Respawn`), after the credits
//!   (`Way::Credits`) and out of the End by its portal (`Way::EndPortal`) is a travel to
//!   the player's bed or anchor where they have one, else to the world spawn in the
//!   overworld. The stay goes to the home of that dimension, or waits here where this
//!   region is that home.
//! - **Asked for.** A player in the credits, or dead, is a waiting stay that waits to be
//!   asked (`PerformRespawn`): out of the world, not ticked, shown to nobody. Every other
//!   input is counted and dropped. A leave keeps the travel with the place, so the next
//!   join goes to the respawn.
//! - **Only the holder checks.** A waiting respawn whose target is another region's
//!   chunk is let go to that region. The region that holds it checks the bed or anchor
//!   (`respawn_block.rs`) and places the player at its stand-up place, spending a
//!   charge for a death's respawn; where it is no respawn the travel is turned to the
//!   world spawn, and the respawn is forgotten when the player is placed (kept on the
//!   portal's way).
//! - **What the client is owed** goes with the placing (`Durable::Placed`'s
//!   `Respawned`), which the edge sends with the `Respawn`.
//!
//! Named: the world spawn is the spawn's block (`spawn_pose`), as a join's is, where the
//! game spreads both over `respawn_radius` with an unseeded random.

use clustine_blocks::data::is_block;
use clustine_data::blocks;
use clustine_world::{ChunkPos, Dimension, PlayerId, Travel, Way};

use super::Region;
use super::items::View;
use super::respawn_block::{RespawnBlock, respawn_block};
use super::updates::UPDATE_ALL;
use crate::api::{
    Durable, PlayerFace, PlayerInput, PlayerTransfer, RegionEvent, Respawned, TickOutput,
};
use crate::state::WaitingState;

/// How many times a waiting respawn is let go to the holder of its target before it is
/// placed at the world spawn, its respawn kept: a holder that changes faster than the
/// stay can follow does not keep it moving for good.
const FORWARDS_LIMIT: u8 = 16;

/// The `Respawn`'s keep of a respawn's way (`PlayerList.respawn`, 959–965; a portal's
/// change of level keeps everything).
fn keep_of(way: Way) -> u8 {
    match way {
        Way::Respawn => 0,
        Way::Credits => 1,
        _ => 3,
    }
}

impl Region {
    /// Whether this region is the home of `dimension`: of it, and holding its home
    /// chunk.
    pub(super) fn is_home_of(&self, dimension: Dimension) -> bool {
        self.config.dimension == dimension && self.land.holds(self.land.home)
    }

    /// The travel of a respawn by `way` of a player whose respawn is `respawn`: to the
    /// bed or anchor, or to the world spawn in the overworld
    /// (`findRespawnPositionAndUseSpawnBlock`).
    pub(super) fn respawn_travel(&self, respawn: Option<crate::api::Respawn>, way: Way) -> Travel {
        match respawn {
            Some(respawn) => Travel {
                to: respawn.dimension,
                target: respawn.pos,
                way,
            },
            None => Travel {
                to: Dimension::Overworld,
                target: self.config.world_spawn,
                way,
            },
        }
    }

    /// `EndPortalBlock.entityInside` for a player who has not seen the credits
    /// (`ServerPlayer.showEndCredits`): off what they ride, out of the world and shown
    /// to nobody, the credits seen, and their edge told to show them. Their stay waits
    /// here to be asked for the respawn after the credits.
    pub(super) fn show_end_credits(&mut self, id: PlayerId, output: &mut TickOutput) {
        if self
            .players
            .get(&id)
            .is_some_and(|player| player.riding.is_some())
        {
            self.dismount_player(id, output);
        }
        self.stop_sleep(id, true, false, output);
        self.menu_leaving(id, true, output);
        let Some(player) = self.players.remove(&id) else {
            return;
        };
        self.journal.players.insert(id);
        output.events.push(RegionEvent::EntityRemoved {
            entity: player.entity_id,
            chunk: player.chunk(),
        });
        let edge = player.edge;
        let entity = player.entity_id;
        let travel = self.respawn_travel(player.respawn, Way::Credits);
        let mut transfer = player.into_transfer(self.tick, travel.to);
        transfer.seen_credits = true;
        self.wait_to_be_asked(edge, id, transfer, travel);
        self.send(edge, Durable::WonGame { player: id, entity }, output);
    }

    /// Keeps the stay of `transfer` waiting here to be asked for its respawn by
    /// `travel`, at the travel's target until it is placed.
    pub(super) fn wait_to_be_asked(
        &mut self,
        edge: clustine_world::EdgeId,
        id: PlayerId,
        mut transfer: PlayerTransfer,
        travel: Travel,
    ) {
        at_target(&mut transfer, travel);
        transfer.riding = None;
        // Their edge was told their entity, so the stay needs its join's attempt no
        // more (as at a portal, `travel.rs`).
        transfer.attempt = None;
        self.take_waiting_stay(edge, id, &transfer);
        if let Some(waiting) = self.waiting.get_mut(&id) {
            waiting.until_asked = true;
        }
    }

    /// An input of a stay that waits to be asked: `PerformRespawn` sends it on to its
    /// respawn; anything else is dropped (`handleMovePlayer` returns early while the
    /// game's player is in the credits). Whether the stay waits to be asked.
    pub(super) fn asked_input(&mut self, id: PlayerId, input: &PlayerInput) -> bool {
        let Some(waiting) = self.waiting.get_mut(&id) else {
            return false;
        };
        if !waiting.until_asked {
            return false;
        }
        if matches!(input, PlayerInput::PerformRespawn) {
            waiting.until_asked = false;
            waiting.since = self.tick;
            self.journal.waiting.insert(id);
            let travel = waiting.transfer.travel;
            if let Some(travel) = travel
                && !self.is_home_of(travel.to)
                && let Some(to) = self.home_of(travel.to)
            {
                self.let_waiting_go(id, to);
            }
        }
        true
    }

    /// Lets a waiting stay go to `to`, as a portal's traveller goes: with the face of
    /// nobody's sight, its travel unchanged.
    pub(super) fn let_waiting_go(&mut self, id: PlayerId, to: clustine_world::RegionId) {
        let Some(waiting) = self.waiting.remove(&id) else {
            return;
        };
        self.journal.waiting.insert(id);
        let transfer = waiting.transfer;
        let face = PlayerFace::of(
            &transfer.pose,
            false,
            transfer.flying,
            &transfer.inventory,
            transfer.selected_slot,
            None,
        );
        self.travelled
            .push(((waiting.edge, id, transfer, to), face));
    }

    /// The respawn of a player through the End's portal out of the End, who has seen the
    /// credits (`EndPortalBlock.getPortalDestination` with
    /// `findRespawnPositionAndUseSpawnBlock(false, …)`): the bed or anchor, its charge
    /// kept, else the world spawn. They leave as a portal's traveller does.
    pub(super) fn end_portal_out(&mut self, id: PlayerId, output: &mut TickOutput) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        if player.riding.is_some() {
            return;
        }
        let travel = self.respawn_travel(player.respawn, Way::EndPortal);
        let Some(home) = self.home_of(travel.to) else {
            return;
        };
        self.stop_sleep(id, true, false, output);
        self.menu_leaving(id, true, output);
        let Some(player) = self.players.remove(&id) else {
            return;
        };
        self.journal.players.insert(id);
        let chunk = player.chunk();
        let edge = player.edge;
        let mut transfer = player.into_transfer(self.tick, travel.to);
        let face = PlayerFace {
            sprinting: transfer.keys.sprint,
            ..PlayerFace::of(
                &transfer.pose,
                transfer.keys.sneak,
                transfer.flying,
                &transfer.inventory,
                transfer.selected_slot,
                None,
            )
        };
        output.events.push(RegionEvent::EntityRemoved {
            entity: transfer.entity_id,
            chunk,
        });
        at_target(&mut transfer, travel);
        transfer.riding = None;
        transfer.attempt = None;
        self.travelled.push(((edge, id, transfer, home), face));
    }

    /// A waiting respawn, once the chunks about its target are settled: let go to the
    /// holder of its target, turned to the world spawn, or placed here.
    pub(super) fn place_respawn(&mut self, id: PlayerId, output: &mut TickOutput) {
        let Some(waiting) = self.waiting.get(&id) else {
            return;
        };
        let Some(mut travel) = waiting.transfer.travel else {
            return;
        };
        let respawn = waiting.transfer.respawn;
        let forwards = waiting.forwards;
        let target = ChunkPos::new(travel.target.x >> 4, travel.target.z >> 4);
        // Only the holder of the target's chunk checks the block and spends a charge: a
        // copy is another region's chunk.
        if travel.to == self.config.dimension
            && let Some(holder) = self.land.knowledge(target).holder()
            && forwards < FORWARDS_LIMIT
        {
            if let Some(waiting) = self.waiting.get_mut(&id) {
                waiting.forwards += 1;
            }
            self.let_waiting_go(id, holder);
            return;
        }
        let at_world_spawn =
            travel.to == Dimension::Overworld && travel.target == self.config.world_spawn;
        let ours = respawn
            .filter(|respawn| (respawn.dimension, respawn.pos) == (travel.to, travel.target));
        let mut failed = false;
        let mut anchor_sound = None;
        let mut place = None;
        if let Some(respawn) = ours
            && travel.to == self.config.dimension
            && forwards < FORWARDS_LIMIT
        {
            let view = View {
                region: self,
                home: target,
            };
            let use_spawn_block = travel.way == Way::Respawn;
            match respawn_block(&view, self.config.dimension, &respawn, use_spawn_block) {
                RespawnBlock::Placed {
                    position,
                    yaw,
                    pitch,
                    spent,
                } => {
                    if let Some((at, state)) = spent {
                        self.set_for_travel(at, state, UPDATE_ALL, output);
                    }
                    // `PlayerList.respawn` 1057–1107: a death's respawn at a block that
                    // is still an anchor plays its depletion to the player.
                    if use_spawn_block
                        && self
                            .block(respawn.pos)
                            .is_some_and(|state| is_block(state, blocks::RESPAWN_ANCHOR))
                    {
                        anchor_sound = Some(respawn.pos);
                    }
                    place = Some((position, yaw, pitch));
                }
                RespawnBlock::Missing => failed = true,
            }
        }
        if place.is_none() && !at_world_spawn {
            // No bed or anchor to wake at: the world spawn in the overworld, placed by
            // the overworld's home.
            travel = Travel {
                to: Dimension::Overworld,
                target: self.config.world_spawn,
                way: travel.way,
            };
            if let Some(waiting) = self.waiting.get_mut(&id) {
                at_target(&mut waiting.transfer, travel);
                waiting.since = self.tick;
                self.journal.waiting.insert(id);
            }
            if !self.is_home_of(Dimension::Overworld) {
                // The overworld's home sees the travel is not the respawn's and owes the
                // event: nothing more is carried.
                if let Some(home) = self.home_of(Dimension::Overworld) {
                    self.let_waiting_go(id, home);
                }
                return;
            }
        }
        let (position, yaw, pitch) = match place {
            Some(place) => place,
            None => {
                let pose = self.spawn_pose();
                (
                    [pose.position.x, pose.position.y, pose.position.z],
                    pose.yaw,
                    pose.pitch,
                )
            }
        };
        // The event is owed where a respawn was set and is not where the player wakes
        // (section 19.7, the review's sixth point), unless the stay gave up following its
        // target's holder.
        let no_respawn_block = respawn.is_some()
            && forwards < FORWARDS_LIMIT
            && (failed || ours.is_none() || place.is_none());
        let forget = no_respawn_block && travel.way != Way::EndPortal;
        if let Some(waiting) = self.waiting.get_mut(&id)
            && forget
        {
            waiting.transfer.respawn = None;
        }
        let respawned = Respawned {
            keep: keep_of(travel.way),
            no_respawn_block,
            anchor_sound,
        };
        self.put_stay(id, position, yaw, Some(pitch), Some(respawned), output);
    }
}

/// Puts `transfer` at `travel`'s target, the block's bottom centre, with its travel:
/// where the store keeps the stay until a region places it.
fn at_target(transfer: &mut PlayerTransfer, travel: Travel) {
    transfer.dimension = travel.to;
    transfer.pose.position.x = f64::from(travel.target.x) + 0.5;
    transfer.pose.position.y = f64::from(travel.target.y);
    transfer.pose.position.z = f64::from(travel.target.z) + 0.5;
    transfer.travel = Some(travel);
}

/// Whether a waiting stay waits for its respawn rather than for its portal.
pub(super) fn waits_for_respawn(waiting: &WaitingState) -> bool {
    waiting
        .transfer
        .travel
        .is_some_and(|travel| match travel.way {
            Way::Respawn | Way::Credits => true,
            Way::EndPortal => travel.to != Dimension::End,
            _ => false,
        })
}

#[cfg(test)]
#[path = "respawning_tests.rs"]
mod tests;
