//! The travel's source side (step P9 of `docs/groundwork/p-plan.md`, sections 16.2,
//! 16.7 and 16.8): what a Nether portal does in the tick its clock fires
//! (`NetherPortalBlock.getPortalDestination` and `Entity.teleport`, cut where the game
//! would go on in the destination's level). The traveller leaves the region in that
//! tick and is in the destination's hands from its commit: a player let go to the
//! destination dimension's home with their travel, an entity filed with the store for
//! the target's chunk. Both are in the tick's commit, so that the traveller is gone
//! from here and on its way there, or neither.
//!
//! The target is worked out here, from the source's blocks (the brief's section 2.2,
//! items 1, 2 and 6): the destination places the traveller by it, its portal's
//! rectangle and the traveller's place in the one it left.

use clustine_blocks::data::is_block;
use clustine_blocks::portal_forcer::{self, DEFAULT_BORDER};
use clustine_data::blocks;
use clustine_world::{
    BlockPos, ChunkPos, Dimension, Double, Entity, EntityChange, EntityId, LevelChunk, PlayerId,
    PortalKind, Travel, Traveller, Way,
};

use super::Region;
use super::portals::PortalUser;
use crate::api::{PlayerFace, RegionEvent, TickOutput};

/// A player's size standing (`Player.STANDING_DIMENSIONS`), which the game measures
/// their place in a portal by.
const PLAYER_SIZE: (f32, f32) = (0.6, 1.8);

/// `NetherPortalBlock.getPortalDestination`'s destination: the overworld from the
/// Nether, the Nether from anywhere else a portal can be lit.
fn nether_portal_destination(from: Dimension) -> Dimension {
    match from {
        Dimension::Nether => Dimension::Overworld,
        _ => Dimension::Nether,
    }
}

/// The block under the End's spawn point (`ServerLevel.END_SPAWN_POINT`, (100, 50, 0)),
/// on which the platform is made: a traveller into the End is placed there (the brief,
/// section 2.4).
pub(super) const END_PLATFORM: BlockPos = BlockPos::new(100, 49, 0);

impl Region {
    /// What a portal does as its clock fires for `who`, in the portal of `kind` it
    /// entered at `entry`: the cooldown is set already. A Nether portal sends them on,
    /// and the End's portal into the End (`EndPortalBlock.getPortalDestination`). Out
    /// of the End, which goes to the world spawn or a player's respawn, is P10c's, and
    /// gateways are P10e's: until then they send nobody.
    pub(super) fn travel_fired(
        &mut self,
        who: PortalUser,
        kind: PortalKind,
        entry: BlockPos,
        output: &mut TickOutput,
    ) {
        let to = match kind {
            PortalKind::Nether => nether_portal_destination(self.config.dimension),
            PortalKind::End if self.config.dimension != Dimension::End => Dimension::End,
            // Out of the End: a player to their respawn (`respawning.rs`), an entity to
            // the top of the world spawn's column.
            PortalKind::End => {
                match who {
                    PortalUser::Player(id) => self.end_portal_out(id, output),
                    PortalUser::Entity(key) => {
                        self.entity_travels(key, kind, Dimension::Overworld, entry, output);
                    }
                }
                return;
            }
            PortalKind::Gateway => return,
        };
        let Some(home) = self.home_of(to) else {
            // A world without the dimension's home: nowhere to go.
            return;
        };
        match who {
            PortalUser::Player(id) => self.player_travels(id, kind, to, home, entry, output),
            PortalUser::Entity(key) => self.entity_travels(key, kind, to, entry, output),
        }
    }

    /// The travel through the portal of `kind` at `entry` to `to` of a traveller at
    /// `position` of `size`: a Nether portal's by its rectangle, the End's to the
    /// platform.
    fn travel_of(
        &self,
        kind: PortalKind,
        to: Dimension,
        entry: BlockPos,
        position: [f64; 3],
        size: (f32, f32),
    ) -> Travel {
        match kind {
            PortalKind::End if to == Dimension::End => Travel {
                to,
                target: END_PLATFORM,
                way: Way::EndPortal,
            },
            // `EndPortalBlock.getPortalDestination` out of the End for an entity: the
            // world spawn, placed on the top of its column (`arriving.rs`).
            PortalKind::End => Travel {
                to,
                target: self.config.world_spawn,
                way: Way::EndPortal,
            },
            PortalKind::Nether | PortalKind::Gateway => {
                self.nether_travel(to, entry, position, size)
            }
        }
    }

    /// The region that holds the home chunk of `dimension`, which a traveller into it is
    /// sent to (the plan, section 16.1).
    pub(super) fn home_of(&self, dimension: Dimension) -> Option<clustine_world::RegionId> {
        (self.config.homes.iter())
            .find(|(of, _)| *of == dimension)
            .map(|(_, region)| *region)
    }

    /// The travel of a traveller at `position`, of `size`, through the Nether portal
    /// whose block at `entry` it was marked inside, to `to`: the target, the source
    /// portal's axis (none where the block is a portal no more, which the game takes as
    /// X) and its place in that portal (`getDimensionTransitionFromExit`).
    fn nether_travel(
        &self,
        to: Dimension,
        entry: BlockPos,
        position: [f64; 3],
        size: (f32, f32),
    ) -> Travel {
        // `DimensionType.getTeleportationScale`: the source's coordinate scale over the
        // destination's.
        let scale = self.config.dimension.rules().coordinate_scale / to.rules().coordinate_scale;
        let target = portal_forcer::clamp_to_bounds(
            DEFAULT_BORDER,
            position[0] * scale,
            position[1],
            position[2] * scale,
        );
        let read = |at: BlockPos| self.block(at);
        let axis = (self.block(entry))
            .filter(|state| is_block(*state, blocks::NETHER_PORTAL))
            .map(clustine_blocks::portal::axis_of);
        let (_, relative) = portal_forcer::exit_relative(entry, position, size, &read);
        Travel {
            to,
            target,
            way: Way::NetherPortal {
                axis,
                relative: relative.map(Double),
            },
        }
    }

    /// A player's portal fires: they leave the region in this tick as a departing
    /// player does, let go to `home` with their travel and their place at the target in
    /// the destination's coordinates; and everyone watching here sees them removed where
    /// they stood (the plan, section 16.2, D2). Their menu closes as at a leave (D6).
    fn player_travels(
        &mut self,
        id: PlayerId,
        kind: PortalKind,
        to: Dimension,
        home: clustine_world::RegionId,
        entry: BlockPos,
        output: &mut TickOutput,
    ) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        // No vehicle with a player in it goes until step P9b; P8 runs no clock for a
        // rider, so a player who fires rides nothing.
        if player.riding.is_some() {
            return;
        }
        let feet = player.pose.position;
        let travel = self.travel_of(kind, to, entry, [feet.x, feet.y, feet.z], PLAYER_SIZE);
        self.stop_sleep(id, true, false, output);
        self.menu_leaving(id, true, output);
        let Some(player) = self.players.remove(&id) else {
            return;
        };
        self.journal.players.insert(id);
        let chunk = player.chunk();
        let edge = player.edge;
        let mut transfer = player.into_transfer(self.tick, to);
        // The face the store keeps is the one the source's watchers last saw, at the
        // portal (D1, section 16.8).
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
        // At the target until a region places them: the block's bottom centre, the
        // look kept (section 16.2, step 1).
        transfer.pose.position.x = f64::from(travel.target.x) + 0.5;
        transfer.pose.position.y = f64::from(travel.target.y);
        transfer.pose.position.z = f64::from(travel.target.z) + 0.5;
        transfer.riding = None;
        transfer.travel = Some(travel);
        // Their edge was told their entity when they entered here, so the stay needs
        // the attempt of its join no more; and without it the destination knows that
        // the edge has them in the world, to finish the change with `Placed`
        // (`arriving.rs`), where a join's stay that was never in it keeps it.
        transfer.attempt = None;
        self.travelled.push(((edge, id, transfer, home), face));
    }

    /// An entity's portal fires: with its riders, none of them a player, it leaves the
    /// region, each reported gone by a portal, and is filed for the target's chunk in
    /// the destination with one hand-over more (the plan, section 16.2). A unit with a
    /// player in it does not fire until step P9b (D8): its cooldown is kept.
    fn entity_travels(
        &mut self,
        key: EntityId,
        kind: PortalKind,
        to: Dimension,
        entry: BlockPos,
        output: &mut TickOutput,
    ) {
        let Some((chunk, root)) = self
            .find_entity(key)
            .map(|(at, entity)| (at, entity.clone()))
        else {
            return;
        };
        if root.riding.vehicle.is_some() {
            return;
        }
        let members = self.unit_records(chunk, root.clone());
        let ridden = |entity: &Entity| entity.riding.passengers.iter().any(|seat| seat.player);
        if members.iter().any(ridden) {
            return;
        }
        let size = self.entity_box(&root).size();
        let size = (size[0] as f32, size[1] as f32);
        let travel = self.travel_of(kind, to, entry, root.position, size);
        let mut entities = Vec::with_capacity(members.len());
        for mut member in members {
            let at = self.find_entity(member.key).map_or(chunk, |(at, _)| at);
            self.travel_away(&member, at, output);
            member.hops = member.hops.saturating_add(1);
            entities.push(member);
        }
        let target = travel.target;
        output.travellers.push(Traveller {
            travel,
            chunk: LevelChunk::new(to, ChunkPos::new(target.x >> 4, target.z >> 4)),
            entities,
        });
    }

    /// Takes one member of a unit that goes through a portal out of the region, as a
    /// member sent across a line is, and reports it gone by a portal.
    fn travel_away(&mut self, entity: &Entity, from: ChunkPos, output: &mut TickOutput) {
        let key = entity.key;
        let gone = EntityChange::Gone(key);
        if let Some(chunk) = self.chunks.get_mut(&from) {
            chunk.apply_entity(&gone);
        }
        output.entity_changes.push((from, gone));
        self.entity_chunks.remove(&key);
        self.entity_memory.remove(&key);
        self.entity_draws.remove(&key);
        self.aside_draws.remove(&key);
        self.piston_deltas.remove(&key);
        output.events.push(RegionEvent::EntityGone {
            entity: key,
            chunk: from,
            hops: entity.hops,
            travelled: true,
        });
    }
}

#[cfg(test)]
#[path = "travel_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "end_travel_tests.rs"]
mod end_travel_tests;

#[cfg(test)]
#[path = "travel_spec_tests.rs"]
mod spec_tests;
