//! Getting on a vehicle across a line (`docs/groundwork/n7j-mount-across-a-line.md`, with
//! its review in section 8): a player clicks a vehicle their region shows only in a copy;
//! the click is a use on the vehicle's holder, which grants them a kept seat, hidden,
//! and tells their region; their region hands them over to ride it, as a crossing does,
//! or declines; the vehicle's region seats them as it seats one who crossed, and the
//! first seating is a mount.

use clustine_world::{
    ChunkPos, EdgeId, Effect, Effected, EntityField, EntityId, EntityUse, Pending, Seat,
};

use super::Region;
use super::riding::add_passenger;
use super::uses::{CREATIVE_ENTITY_REACH, User, distance_to_sqr};
use crate::api::{PlayerEvent, RegionEvent, TickOutput};
use crate::state::PlayerRiding;

/// How long a granted seat waits for its player: two effects and a hand-over take a few
/// ticks, and one that never comes holds a seat no longer than this (section 8 item 7).
pub(crate) const GRANTED_FOR: u16 = 40;

/// The use across a line the region is taking: the player's edge and where they stood.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Across {
    pub(crate) edge: EdgeId,
    pub(crate) from: [f64; 3],
}

impl Region {
    /// The holder's side, for a use across a line that ends in a mount of `vehicle` by
    /// the player `rider`, who is another region's: a seat kept for them, granted and
    /// hidden, and their region told. Nothing where the vehicle has no room, where a seat
    /// already names them (a second click: section 8 item 3), or where their region is
    /// not one the effect can reach. Whether a seat was granted.
    pub(crate) fn grant_seat(
        &mut self,
        rider: EntityId,
        vehicle: EntityId,
        across: Across,
        output: &mut TickOutput,
    ) -> bool {
        let Some((chunk, record)) = self.find_entity(vehicle) else {
            return false;
        };
        if record
            .riding
            .passengers
            .iter()
            .any(|seat| seat.key == rider)
            || !self.can_add_passenger(record, chunk)
        {
            return false;
        }
        let at = crate::physics::containing(across.from);
        if !self.sends_to(at.chunk()) {
            return false;
        }
        let root = self.root_of(vehicle);
        let Some((root_chunk, _)) = self.find_entity(root) else {
            return false;
        };
        let mut record = record.clone();
        add_passenger(
            &mut record.riding.passengers,
            Seat {
                key: rider,
                player: true,
                pending: Some(Pending {
                    left: GRANTED_FOR,
                    edge: across.edge,
                    granted: true,
                }),
            },
        );
        self.commit_entity(
            chunk,
            vehicle,
            vec![EntityField::Riding(record.riding.clone())],
            output,
        );
        self.made.push(Effect {
            at,
            what: Effected::Seated {
                player: rider,
                vehicle,
                chunk: root_chunk,
            },
        });
        true
    }

    /// The player's region's side of a seat granted to its player `player` on `vehicle`,
    /// whose root's chunk is `chunk` (section 8 items 3, 4 and 6). Ignored where they ride
    /// it or wait for it already; declined where they are not to get on now; where the
    /// region holds the vehicle itself, they get on at once; else they ride it, on their
    /// way, and are handed over to its region at the end of the tick.
    pub(crate) fn take_seated(
        &mut self,
        player: EntityId,
        vehicle: EntityId,
        chunk: ChunkPos,
        output: &mut TickOutput,
    ) {
        let Some(id) = self.player_with_entity(player) else {
            return;
        };
        let Some(rider) = self.players.get(&id) else {
            return;
        };
        if rider.riding.is_some_and(|riding| riding.vehicle == vehicle) {
            return;
        }
        let user = User {
            feet: [
                rider.pose.position.x,
                rider.pose.position.y,
                rider.pose.position.z,
            ],
            sneaking: rider.keys.sneak,
        };
        let waiting = rider.riding.is_some_and(|riding| riding.pending.is_some());
        let busy = rider.sleeping.is_some() || rider.menu.is_some() || waiting;
        // Within reach of what the region sees of it, with a block's slack for the ticks
        // the grant took.
        let reach = (CREATIVE_ENTITY_REACH + 1.0) * (CREATIVE_ENTITY_REACH + 1.0);
        let seen = self
            .find_entity(vehicle)
            .map(|(_, record)| record.clone())
            .or_else(|| self.copied_entity(vehicle).map(|(_, record)| record));
        let near = seen
            .as_ref()
            .is_some_and(|record| distance_to_sqr(self.entity_box(record), user.eyes()) < reach);
        if busy || !near {
            self.decline_seat(player, vehicle);
            return;
        }
        // Held here now (merged, lent, or come into the region): an ordinary mount,
        // which replaces the kept seat.
        if self.find_entity(vehicle).is_some() {
            if !self.mount_player(player, vehicle, output) {
                self.decline_seat(player, vehicle);
            }
            return;
        }
        if self.land.knowledge(chunk).holder().is_none() {
            self.decline_seat(player, vehicle);
            return;
        }
        // One who rides another vehicle gets off it first, as `mount_player` does.
        if self.players[&id].riding.is_some() {
            self.dismount_player(id, output);
        }
        let player = self.players.get_mut(&id).expect("found above");
        player.riding = Some(PlayerRiding {
            vehicle,
            chunk,
            check: Default::default(),
            pending: Some(super::riders::PENDING_FOR),
        });
        self.journal.players.insert(id);
    }

    /// `SeatDeclined` to the vehicle `vehicle` of the region whose copy shows it.
    fn decline_seat(&mut self, player: EntityId, vehicle: EntityId) {
        self.send_on_entity(vehicle, EntityUse::SeatDeclined { player });
    }

    /// The vehicle's region's side of a decline: the seat granted to `player` on
    /// `vehicle` goes, and no other (section 8 item 3). Never shown, it is not reported.
    pub(crate) fn seat_declined(
        &mut self,
        vehicle: EntityId,
        player: EntityId,
        output: &mut TickOutput,
    ) {
        let Some((chunk, record)) = self.find_entity(vehicle) else {
            return;
        };
        let granted =
            |seat: &Seat| seat.key == player && seat.pending.is_some_and(|pending| pending.granted);
        if !record.riding.passengers.iter().any(granted) {
            return;
        }
        let mut record = record.clone();
        record.riding.passengers.retain(|seat| !granted(seat));
        self.commit_entity(
            chunk,
            vehicle,
            vec![EntityField::Riding(record.riding)],
            output,
        );
    }

    /// The first seating of a player on a seat granted them across a line, as
    /// `mount_player` mounts one: the vehicle's own part before the seat is shown (a happy
    /// ghast's goggles), the seat shown, `entity_mount`, and the player told they ride;
    /// a horse or a camel turns them to its head (`doPlayerRide`).
    pub(crate) fn first_seating(
        &mut self,
        id: clustine_world::PlayerId,
        vehicle: EntityId,
        output: &mut TickOutput,
    ) {
        let Some(rider) = self.players.get(&id).map(|player| player.entity_id) else {
            return;
        };
        self.passenger_adding(vehicle, rider, output);
        let Some((chunk, record)) = self.find_entity(vehicle) else {
            return;
        };
        let record = record.clone();
        output.events.push(RegionEvent::Passengers {
            vehicle,
            chunk,
            hops: record.hops,
            passengers: record.riding.shown_passengers(),
        });
        let context = super::vibrations::Context::of(self.player_source(id), None);
        self.vibrate(
            super::vibrations::Vibration::ENTITY_MOUNT,
            record.position,
            context,
        );
        let turns = crate::mobs::types::equine::is_equine(record.kind)
            || matches!(
                i32::from(record.kind),
                clustine_data::entity_types::CAMEL | clustine_data::entity_types::CAMEL_HUSK
            );
        if turns {
            let head = crate::mobs::Mob::decode(&record.data)
                .map_or(record.yaw, |mob| mob.body.head.head_yaw);
            if let Some(player) = self.players.get_mut(&id) {
                player.pose.yaw = head;
                player.pose.pitch = record.pitch;
            }
        }
        let pose = self.players[&id].pose;
        output.player_events.push((
            id,
            PlayerEvent::Rides {
                vehicle: Some(vehicle),
                pose,
            },
        ));
    }
}
