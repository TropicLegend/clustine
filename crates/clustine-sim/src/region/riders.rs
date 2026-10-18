//! Players riding (`docs/groundwork/n5-plan.md`, step N5b, mine; `docs/groundwork/
//! n5-brief.md`, sections 0.11, 0.14, 0.15, 1.2 to 1.7): a player gets on a vehicle by
//! using it, sits at the vehicle's seat for their place among its passengers, moves
//! only their head while they ride, and gets off by sneaking, where the vehicle lets a
//! player off. Getting on and off are teleports in the game: the player's own client is
//! told where they are and whom the vehicle carries. A riding player's holding chunk is
//! their unit's.
//!
//! Riding across a line is the riders' step (N5h): until then a unit with a player in
//! it is held back at the line, and so is its rider with it.

use clustine_data::entity_types;
use clustine_world::{Entity, EntityData, EntityField, EntityId, PlayerId, Seat};

use super::riding::{DismountPose, STANDING_HEIGHTS, add_passenger, passenger_point};
use super::{Player, Region};
use crate::api::{PlayerEvent, Pose, RegionEvent, TickOutput};
use crate::state::PlayerRiding;

/// `POSE_DISMOUNT_HEIGHTS` of `SWIMMING`.
const SWIMMING_HEIGHTS: &[i32] = &[0, 1];

/// `Player.getDismountPoses()`: standing, crouching and swimming, each at the size the
/// player's type has in it (`Player.POSES`: 0.6 × 1.8, 0.6 × 1.5, 0.6 × 0.6).
const PLAYER_POSES: [DismountPose; 3] = [
    DismountPose {
        width: 0.6,
        height: 1.8,
        heights: STANDING_HEIGHTS,
    },
    DismountPose {
        width: 0.6,
        height: 1.5,
        heights: STANDING_HEIGHTS,
    },
    DismountPose {
        width: 0.6,
        height: 0.6,
        heights: SWIMMING_HEIGHTS,
    },
];

/// The player's vehicle point (`EntityAttachments`' `VEHICLE` of the player's type,
/// extracted): where a vehicle's seat point meets the player, above their feet.
fn player_vehicle_point() -> [f64; 3] {
    let info = clustine_data::EntityTypeInfo::of(entity_types::PLAYER);
    info.vehicle_points.first().copied().unwrap_or([0.0; 3])
}

impl Player {
    /// The player as an entity record, for the rules of getting off, which read a
    /// passenger's type, place and way.
    fn as_passenger(&self, vehicle: EntityId) -> Entity {
        let position = self.pose.position;
        Entity {
            key: self.entity_id,
            kind: entity_types::PLAYER as u16,
            uuid: uuid::Uuid::nil(),
            position: [position.x, position.y, position.z],
            motion: [0.0; 3],
            yaw: self.pose.yaw,
            pitch: self.pose.pitch,
            on_ground: self.pose.on_ground,
            hops: 0,
            age: 0,
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: clustine_world::Riding {
                vehicle: Some(vehicle),
                ..Default::default()
            },
            // Read for its kind only.
            data: EntityData::LeashKnot {
                attached: clustine_world::Attached {
                    block: clustine_world::BlockPos::new(0, 0, 0),
                    direction: 0,
                },
            },
        }
    }
}

impl Region {
    /// The player of this region whose stay has the entity `key`.
    pub(crate) fn player_with_entity(&self, key: EntityId) -> Option<PlayerId> {
        self.players
            .iter()
            .find(|(_, player)| player.entity_id == key)
            .map(|(id, _)| *id)
    }

    /// `startRiding(vehicle)` of the player whose entity is `rider`, after a use of the
    /// vehicle: whether they got on. A player who sneaks does not (`canRide`), nor one
    /// who rides it already; one who rides another vehicle gets off it first. The
    /// vehicle must be the region's and have room (`canAddPassenger`).
    pub(crate) fn mount_player(
        &mut self,
        rider: EntityId,
        vehicle: EntityId,
        output: &mut TickOutput,
    ) -> bool {
        let Some(id) = self.player_with_entity(rider) else {
            return false;
        };
        let player = &self.players[&id];
        if player.keys.sneak
            || player
                .riding
                .is_some_and(|riding| riding.vehicle == vehicle)
        {
            return false;
        }
        let Some((chunk, record)) = self.find_entity(vehicle) else {
            return false;
        };
        if !self.can_add_passenger(record, chunk) {
            return false;
        }
        if self.players[&id].riding.is_some() {
            self.dismount_player(id, output);
        }
        let Some((chunk, record)) = self.find_entity(vehicle) else {
            return false;
        };
        let mut record = record.clone();
        add_passenger(
            &mut record.riding.passengers,
            Seat {
                key: rider,
                player: true,
                pending: None,
            },
        );
        self.commit_entity(
            chunk,
            vehicle,
            vec![EntityField::Riding(record.riding.clone())],
            output,
        );
        output.events.push(RegionEvent::Passengers {
            vehicle,
            chunk,
            hops: record.hops,
            passengers: record
                .riding
                .passengers
                .iter()
                .map(|seat| seat.key)
                .collect(),
        });
        let player = self.players.get_mut(&id).expect("found above");
        player.riding = Some(PlayerRiding {
            vehicle,
            chunk,
            check: Default::default(),
        });
        self.journal.players.insert(id);
        self.seat_player(id, output);
        let pose = self.players[&id].pose;
        output.player_events.push((
            id,
            PlayerEvent::Rides {
                vehicle: Some(vehicle),
                pose,
            },
        ));
        true
    }

    /// Puts the riding player `id` at their seat: the vehicle's passenger point for
    /// their place, less the player's own vehicle point (`positionRider`). A player who
    /// rides turns their head as they like: the vehicle does not turn them, but a boat
    /// keeps them within 105 degrees of its way, which their client does too.
    fn seat_player(&mut self, id: PlayerId, output: &mut TickOutput) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let Some(riding) = player.riding else {
            return;
        };
        let Some((chunk, vehicle)) = self.find_entity(riding.vehicle) else {
            return;
        };
        let Some(index) = vehicle
            .riding
            .passengers
            .iter()
            .position(|seat| seat.key == player.entity_id)
        else {
            return;
        };
        let point = passenger_point(vehicle, index);
        let attach = player_vehicle_point();
        let at: [f64; 3] =
            std::array::from_fn(|axis| (vehicle.position[axis] + point[axis]) - attach[axis]);
        let player = self.players.get_mut(&id).expect("found above");
        player.riding = Some(PlayerRiding { chunk, ..riding });
        let position = clustine_world::Vec3::new(at[0], at[1], at[2]);
        if position != player.pose.position {
            let from = player.chunk();
            player.pose.position = position;
            player.moved_from.get_or_insert(from);
            self.journal.players.insert(id);
        }
        let _ = output;
    }

    /// `stopRiding()` of the riding player `id`, who is put where the vehicle lets a
    /// player off, or where they are if it is gone, and told so as by a teleport.
    pub(crate) fn dismount_player(&mut self, id: PlayerId, output: &mut TickOutput) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let Some(riding) = player.riding else {
            return;
        };
        let passenger = player.as_passenger(riding.vehicle);
        let entity = player.entity_id;
        let vehicle = self
            .find_entity(riding.vehicle)
            .map(|(chunk, record)| (chunk, record.clone()));
        let mut at = passenger.position;
        if let Some((chunk, mut record)) = vehicle {
            record.riding.passengers.retain(|seat| seat.key != entity);
            self.commit_entity(
                chunk,
                riding.vehicle,
                vec![EntityField::Riding(record.riding.clone())],
                output,
            );
            output.events.push(RegionEvent::Passengers {
                vehicle: riding.vehicle,
                chunk,
                hops: record.hops,
                passengers: record
                    .riding
                    .passengers
                    .iter()
                    .map(|seat| seat.key)
                    .collect(),
            });
            at = self.dismount_location(&record, chunk, &passenger, &PLAYER_POSES);
        }
        let player = self.players.get_mut(&id).expect("found above");
        let from = player.chunk();
        player.riding = None;
        player.pose.position = clustine_world::Vec3::new(at[0], at[1], at[2]);
        player.moved_from.get_or_insert(from);
        self.journal.players.insert(id);
        let pose: Pose = player.pose;
        output.player_events.push((
            id,
            PlayerEvent::Rides {
                vehicle: None,
                pose,
            },
        ));
    }

    /// Takes the player `player`, who is leaving the region, out of their vehicle's
    /// seats, so that it keeps no seat for nobody (`docs/groundwork/n5-plan.md`,
    /// section 7, item 6). Nothing is told to them: they are gone.
    pub(super) fn unseat_leaving(&mut self, player: &Player, output: &mut TickOutput) {
        let Some(riding) = player.riding else {
            return;
        };
        let Some((chunk, record)) = self.find_entity(riding.vehicle) else {
            return;
        };
        let mut record = record.clone();
        let before = record.riding.passengers.len();
        record
            .riding
            .passengers
            .retain(|seat| seat.key != player.entity_id);
        if record.riding.passengers.len() == before {
            return;
        }
        self.commit_entity(
            chunk,
            riding.vehicle,
            vec![EntityField::Riding(record.riding.clone())],
            output,
        );
        output.events.push(RegionEvent::Passengers {
            vehicle: riding.vehicle,
            chunk,
            hops: record.hops,
            passengers: record
                .riding
                .passengers
                .iter()
                .map(|seat| seat.key)
                .collect(),
        });
    }

    /// The riding players' part of the entities' phase, after the vehicles have ticked
    /// (`ServerLevel.tickPassenger` → `Player.rideTick`): one who sneaks gets off, one
    /// whose vehicle is gone or no longer seats them gets off where they are, and the
    /// rest are put at their seats.
    pub(crate) fn tick_riding_players(&mut self, output: &mut TickOutput) {
        let riding: Vec<(PlayerId, PlayerRiding, EntityId, bool)> = self
            .players
            .iter()
            .filter_map(|(id, player)| {
                Some((*id, player.riding?, player.entity_id, player.keys.sneak))
            })
            .collect();
        for (id, riding, entity, sneaking) in riding {
            let seated = self
                .find_entity(riding.vehicle)
                .is_some_and(|(_, vehicle)| {
                    vehicle
                        .riding
                        .passengers
                        .iter()
                        .any(|seat| seat.key == entity)
                });
            if sneaking || !seated {
                self.dismount_player(id, output);
            } else {
                self.seat_player(id, output);
            }
        }
    }
}
