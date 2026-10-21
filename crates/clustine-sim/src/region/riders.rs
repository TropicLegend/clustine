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
use clustine_world::{ChunkPos, Entity, EntityData, EntityField, EntityId, PlayerId, Seat};

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
            lived: 0,
            fresh_at: 0,
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

    /// `handlePlayerCommand(START_RIDING_JUMP)` of the player `id`: with a charge above
    /// nothing, a horse they drive that can jump (`canJump`: saddled) begins its jump
    /// (`handleStartJump`: it may rear) and plays its jump sound (`playJumpSound`, 0.4 and
    /// 1, its kind's own); the jump itself is the rider's client's.
    pub(crate) fn riding_jump(&mut self, id: PlayerId, power: i32, output: &mut TickOutput) {
        if power <= 0 {
            return;
        }
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let Some(riding) = player.riding else {
            return;
        };
        let rider = player.entity_id;
        let Some((chunk, vehicle)) = self.find_entity(riding.vehicle) else {
            return;
        };
        let vehicle = vehicle.clone();
        if !self.drives_root(&vehicle, rider) {
            return;
        }
        if !crate::mobs::types::equine::is_equine(vehicle.kind) {
            self.mob_riding_jump(&vehicle, chunk, power, output);
            return;
        }
        let Ok(mut mob) = crate::mobs::Mob::decode(&vehicle.data) else {
            return;
        };
        if !crate::mobs::types::equine::riding::can_jump(&mob) {
            return;
        }
        crate::mobs::types::equine::riding::handle_start_jump(
            vehicle.kind,
            &mut mob,
            vehicle.lived,
        );
        self.commit_mob(&vehicle, chunk, &mob, output);
        let view = super::items::View {
            region: &*self,
            home: chunk,
        };
        let in_water = crate::physics::fluids::update(&view, &self.entity_box(&vehicle)).in_water();
        let [x, y, z] = vehicle.position;
        output.events.push(RegionEvent::Sound {
            sound: crate::mobs::types::equine::riding::jump_sound(vehicle.kind, in_water),
            source: crate::physics::item::SOUND_SOURCE_NEUTRAL,
            position: clustine_world::Vec3::new(x, y, z),
            volume: 0.4,
            pitch: 1.0,
            except: None,
        });
    }

    /// `FoodOnAStickItem.use` of the carrot on a stick `held` by the player `id`: the pig
    /// they drive boosts, unless it boosts already (`Pig.boost`, one draw of its own
    /// random); the stick takes no wear of a creative player, which every player is.
    /// Whether the use was this.
    pub(crate) fn steered_boost(
        &mut self,
        id: PlayerId,
        held: Option<i32>,
        output: &mut TickOutput,
    ) -> bool {
        if held != Some(clustine_data::items::CARROT_ON_A_STICK) {
            return false;
        }
        let Some(player) = self.players.get(&id) else {
            return false;
        };
        let Some(riding) = player.riding else {
            return false;
        };
        let rider = player.entity_id;
        // `getControlledVehicle`: the vehicle they ride, where they are its driver.
        let Some((chunk, vehicle)) = self.find_entity(riding.vehicle) else {
            return false;
        };
        let vehicle = vehicle.clone();
        if i32::from(vehicle.kind) != clustine_data::entity_types::PIG
            || !self.drives_root(&vehicle, rider)
        {
            return false;
        }
        let Ok(mut mob) = crate::mobs::Mob::decode(&vehicle.data) else {
            return false;
        };
        let mut stream = self.entity_stream(vehicle.key).random;
        let start = stream.clone();
        let boosted = crate::mobs::types::pig::boost(&mut mob, &mut stream);
        self.entity_drew(vehicle.key, stream.draws_since(&start));
        if boosted {
            self.commit_mob(&vehicle, chunk, &mob, output);
        }
        boosted
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
            // Another region's player, by a use sent across a line: a seat is granted,
            // which their region takes or declines (`granted.rs`).
            return match self.using_across {
                Some(across) => self.grant_seat(rider, vehicle, across, output),
                None => false,
            };
        };
        let player = &self.players[&id];
        // One on their way to their unit rides it already, and gets on nothing else
        // meanwhile (`docs/groundwork/n5-plan.md`, section 7 item 5).
        if player.keys.sneak
            || player
                .riding
                .is_some_and(|riding| riding.vehicle == vehicle || riding.pending.is_some())
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
        // `addPassenger`'s own part comes before the seat (a happy ghast's goggles).
        self.passenger_adding(vehicle, rider, output);
        let Some((chunk, record)) = self.find_entity(vehicle) else {
            return false;
        };
        let mut record = record.clone();
        // A seat it still had for them, of a ride they were let off from while it was
        // out of reach, is not a second one.
        record.riding.passengers.retain(|seat| seat.key != rider);
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
            passengers: record.riding.shown_passengers(),
        });
        // `entity_mount`, of the rider, at the vehicle (`vibrations.rs`).
        let context = super::vibrations::Context::of(self.player_source(id), None);
        let mount = super::vibrations::Vibration::ENTITY_MOUNT;
        self.vibrate(mount, record.position, context);
        let player = self.players.get_mut(&id).expect("found above");
        player.riding = Some(PlayerRiding {
            vehicle,
            chunk,
            check: Default::default(),
            pending: None,
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
        let game_time = i64::try_from(self.clock.game_time).unwrap_or(i64::MAX);
        let point = passenger_point(vehicle, index, super::mob_bodies::player_kind(), game_time);
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
        // `entity_dismount`, of the rider, at the vehicle (`vibrations.rs`).
        if let Some((_, record)) = &vehicle {
            let context = super::vibrations::Context::of(self.player_source(id), None);
            let dismount = super::vibrations::Vibration::ENTITY_DISMOUNT;
            self.vibrate(dismount, record.position, context);
        }
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
                passengers: record.riding.shown_passengers(),
            });
            self.passenger_removed(&record, chunk, entity, output);
            let record = self
                .find_entity(riding.vehicle)
                .map_or(record, |(_, record)| record.clone());
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

    /// The leave of the player `id`, whose stay `player` the region has just taken out:
    /// where they are seated (not on their way) in a unit of the region's whose seats,
    /// over every member, hold no player but them, the whole unit leaves the world with
    /// them, as the game's `PlayerList.remove` takes a root vehicle with exactly one
    /// player passenger (`docs/groundwork/n7h-logout-on-a-mount.md`, section 3): every
    /// member is gone from its chunk at its `hops`, its seat for the player kept, and the
    /// leave's note carries it with the place the vehicle would let them off at, where
    /// they stand if it is not put under them again. Whether it was taken.
    pub(super) fn take_unit_leaving(
        &mut self,
        id: PlayerId,
        player: &Player,
        output: &mut TickOutput,
    ) -> bool {
        let Some(riding) = player.riding else {
            return false;
        };
        if riding.pending.is_some() {
            return false;
        }
        let root = self.root_of(riding.vehicle);
        let Some((chunk, record)) = self.find_entity(root) else {
            return false;
        };
        let members = self.unit_records(chunk, record.clone());
        let mut players = members
            .iter()
            .flat_map(|member| &member.riding.passengers)
            .filter(|seat| seat.player);
        let alone = players
            .next()
            .is_some_and(|seat| seat.key == player.entity_id && seat.pending.is_none())
            && players.next().is_none();
        if !alone {
            return false;
        }
        let Some(note) = self.dismount_note(id, player) else {
            return false;
        };
        // The game saves the unit before the player's `stopRiding`, so the unit keeps
        // its records as they were, and only what watchers hear of the seat's going is
        // made (`docs/groundwork/n7j-riding-plan.md`, section 9 item 1).
        if let Some(vehicle) = members.iter().find(|member| member.key == riding.vehicle) {
            let mut left = vehicle.clone();
            left.riding
                .passengers
                .retain(|seat| seat.key != player.entity_id);
            self.mob_passenger_removed_heard(&left, output);
        }
        for member in &members {
            self.take_out(chunk, member, output);
        }
        let crate::api::StayNote::Has {
            player,
            entity,
            hops,
            place,
            mut face,
            let_go,
            ..
        } = note
        else {
            unreachable!("a dismount note is a `Has`")
        };
        // Their face goes with them; the vehicle is out of the world.
        face.vehicle = None;
        self.dismounted.push((
            id,
            crate::api::StayNote::Has {
                player,
                entity,
                hops,
                place,
                face,
                let_go,
                unit: Some(crate::api::Unit(members)),
            },
        ));
        true
    }

    /// Puts the unit `unit`, which the player `id` left the world on, back under their
    /// stay `entity` of the edge `edge`, which enters
    /// (`docs/groundwork/n7h-logout-on-a-mount.md`, section 5): it is taken out of their
    /// record (a `Took` in this tick's commit) and sent, every member at the `hops` it
    /// left with, to its root's chunk as an arrival, whose taker raises them. Where
    /// `seated`, the player's seat in it names the stay and is kept pending for it, and
    /// the stay's riding is returned, the stay waiting for its unit as one handed over
    /// before it (N5); else the seat is dropped and the unit comes back empty.
    pub(super) fn unit_returning(
        &mut self,
        id: PlayerId,
        entity: EntityId,
        edge: clustine_world::EdgeId,
        unit: crate::api::Unit,
        seated: bool,
        output: &mut TickOutput,
    ) -> Option<PlayerRiding> {
        let mut members = unit.0;
        let root = members.first()?.clone();
        let to = root.chunk();
        self.took.push(crate::api::StayNote::Took {
            player: id,
            root: root.key,
        });
        let mut returned = None;
        for member in &mut members {
            let Some(index) = member.riding.passengers.iter().position(|seat| seat.player) else {
                continue;
            };
            if !seated || returned.is_some() {
                member.riding.passengers.remove(index);
                continue;
            }
            // `startRiding(…, true, false)` at the re-entry runs `addPassenger`: the
            // vehicle's own part, as of its seats without the player's.
            let mut without = member.clone();
            without.riding.passengers.remove(index);
            if let Some(data) = self.mob_passenger_added_away(&without, output) {
                member.data = data;
            }
            let seat = &mut member.riding.passengers[index];
            seat.key = entity;
            seat.pending = Some(clustine_world::Pending {
                granted: false,
                left: PENDING_FOR,
                edge,
            });
            returned = Some(PlayerRiding {
                vehicle: member.key,
                chunk: to,
                check: Default::default(),
                pending: Some(PENDING_FOR),
            });
        }
        for member in members {
            output.arrivals.push((to, member));
        }
        returned
    }

    /// A member of a unit leaving the world with its player: gone from the chunk `from`
    /// as an entity removed, at its `hops`, and what the region kept of it beyond its
    /// record dropped, as a member handed on has it dropped.
    fn take_out(&mut self, from: ChunkPos, member: &Entity, output: &mut TickOutput) {
        let key = member.key;
        self.remove_entity(key, member.hops, from, output);
        self.entity_draws.remove(&key);
        self.aside_draws.remove(&key);
        self.piston_deltas.remove(&key);
    }

    /// The note of a rider who leaves the world, at the place their vehicle lets them off
    /// at, if the vehicle is here to say.
    fn dismount_note(&self, id: PlayerId, player: &Player) -> Option<crate::api::StayNote> {
        let riding = player.riding?;
        let (chunk, record) = self.find_entity(riding.vehicle)?;
        let at = self.dismount_location(
            record,
            chunk,
            &player.as_passenger(riding.vehicle),
            &PLAYER_POSES,
        );
        let mut place = player.place();
        place.pose.position = clustine_world::Vec3::new(at[0], at[1], at[2]);
        let mut face = player.face();
        face.position = at;
        face.vehicle = None;
        Some(crate::api::StayNote::Has {
            player: id,
            entity: player.entity_id,
            hops: player.hops,
            place,
            face,
            let_go: false,
            unit: None,
        })
    }

    /// Takes the player `player`, who is leaving the region, out of their vehicle's
    /// seats, so that it keeps no seat for nobody (`docs/groundwork/n5-plan.md`,
    /// section 7, item 6). Nothing is told to them: they are gone. One who leaves the
    /// world (`left` names them) has the place the vehicle lets them off at noted with
    /// the store, so that they come back beside it, not in a seat that is no longer
    /// theirs (section 8, item 6; ADR-0020, section 3, as amended).
    pub(super) fn unseat_leaving(
        &mut self,
        left: Option<PlayerId>,
        player: &Player,
        output: &mut TickOutput,
    ) {
        let Some(riding) = player.riding else {
            return;
        };
        if let Some(id) = left
            && let Some(note) = self.dismount_note(id, player)
        {
            self.dismounted.push((id, note));
        }
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
            passengers: record.riding.shown_passengers(),
        });
        self.passenger_removed(&record, chunk, player.entity_id, output);
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
            // One on their way to their unit is seated or given up on by
            // `tick_waiting_riders`, and neither gets off nor is put anywhere meanwhile.
            if riding.pending.is_some() {
                continue;
            }
            // A unit in a chunk the region has not loaded yet (after a restore, a merge
            // or a split, whose chunks come some ticks later, and whose holder a merged
            // region learns only then) is waited for: the rider stays seated, and is
            // neither put anywhere nor let off. Only in a chunk loaded here, or known to
            // be another region's, is a unit not found gone.
            if self.find_entity(riding.vehicle).is_none()
                && !self.chunks.contains_key(&riding.chunk)
                && !matches!(
                    self.land.knowledge(riding.chunk),
                    super::Knowledge::Foreign(_)
                )
            {
                continue;
            }
            let seated = self
                .find_entity(riding.vehicle)
                .is_some_and(|(_, vehicle)| {
                    vehicle
                        .riding
                        .passengers
                        .iter()
                        .any(|seat| seat.key == entity)
                });
            if sneaking || !seated || self.rider_under_water(id, riding.vehicle) {
                self.dismount_player(id, output);
            } else {
                self.seat_player(id, output);
            }
        }
    }

    /// `LivingEntity.baseTick`'s rule for a rider whose eyes are in water: off a
    /// vehicle of `#dismounts_underwater` (a pig, a horse), but in a bubble column. The
    /// eyes are a standing player's, 1.62 above their feet, in water where the block
    /// they are in holds water (**inferred**: the game's camera-height test of the
    /// fluid is taken as the block's).
    fn rider_under_water(&self, id: PlayerId, vehicle: EntityId) -> bool {
        let (Some(player), Some((_, vehicle))) = (self.players.get(&id), self.find_entity(vehicle))
        else {
            return false;
        };
        if !clustine_blocks::data::entity_in_tag(
            i32::from(vehicle.kind),
            "minecraft:dismounts_underwater",
        ) {
            return false;
        }
        let at = player.pose.position;
        let eyes = crate::physics::containing([at.x, at.y + 1.62, at.z]);
        let Some(state) = self.block(eyes) else {
            return false;
        };
        clustine_blocks::data::has_water(state)
            && !clustine_blocks::data::is_class(state, clustine_data::BlockClass::BubbleColumnBlock)
    }
}

/// How long a seat is kept for a player on their way to it, and a player waits for
/// their unit, in ticks (ADR-0028, section 5: "within 100 ticks").
pub(crate) const PENDING_FOR: u16 = 100;

impl Region {
    /// Whether the riders of a unit sent into the chunk `to` can follow it: the region
    /// knows who holds the chunk, another region or itself (a chunk of its own not
    /// loaded, which its arrival comes back to). Into land it knows nothing of, or has
    /// asked for, the unit is held back at the line until it knows.
    pub(crate) fn riders_may_follow(&self, to: ChunkPos) -> bool {
        matches!(
            self.land.knowledge(to),
            super::Knowledge::Foreign(_) | super::Knowledge::Held
        )
    }

    /// Before the unit of the root `root` (as its tick leaves it) of the chunk `from` is
    /// sent into the chunk `to`: each of the region's players seated in it is on their
    /// way with it, their seat kept for them and their holding chunk the unit's new one,
    /// so that the end of the tick hands them on to its holder in the commit that sends
    /// the unit (`docs/groundwork/n5-plan.md`, section 4 and section 8 item 8); a seat
    /// kept already for a player not come yet leaves a forwarding entry, so that they
    /// follow when they come.
    pub(crate) fn riders_follow(
        &mut self,
        from: ChunkPos,
        root: &mut Entity,
        to: ChunkPos,
        output: &mut TickOutput,
    ) {
        let members = self.unit_records(from, root.clone());
        for member in members {
            if !member.riding.passengers.iter().any(|seat| seat.player) {
                continue;
            }
            let mut riding = member.riding.clone();
            for seat in &mut riding.passengers {
                if !seat.player {
                    continue;
                }
                if seat.pending.is_some() {
                    self.forwarded.insert(member.key, (to, PENDING_FOR));
                    continue;
                }
                let Some(id) = self.player_with_entity(seat.key) else {
                    continue;
                };
                let player = self.players.get_mut(&id).expect("found above");
                seat.pending = Some(clustine_world::Pending {
                    granted: false,
                    left: PENDING_FOR,
                    edge: player.edge,
                });
                if let Some(rides) = player.riding.as_mut() {
                    rides.chunk = to;
                    rides.pending = Some(PENDING_FOR);
                }
                self.journal.players.insert(id);
            }
            if member.key == root.key {
                root.riding = riding;
            } else {
                self.commit_entity(from, member.key, vec![EntityField::Riding(riding)], output);
            }
        }
    }

    /// A player has come in on their way to their unit (`PlayerTransfer::riding`), or a
    /// unit has come with a seat kept for a player who is here: seats them where the
    /// unit is here and keeps their seat, gets them off at once where the unit is here
    /// and has no seat for them, sends them on where the region sent the unit on, and
    /// otherwise lets them wait.
    pub(crate) fn rider_arrived(&mut self, id: PlayerId, output: &mut TickOutput) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let Some(riding) = player.riding else {
            return;
        };
        if riding.pending.is_none() {
            return;
        }
        let entity = player.entity_id;
        let Some((chunk, vehicle)) = self.find_entity(riding.vehicle) else {
            if let Some((to, _)) = self.forwarded.get(&riding.vehicle).copied() {
                // Sent on: the player's holding chunk is where it went, whose holder the
                // end of the tick hands them on to.
                let player = self.players.get_mut(&id).expect("found above");
                if let Some(rides) = player.riding.as_mut() {
                    rides.chunk = to;
                }
                self.journal.players.insert(id);
            }
            return;
        };
        let Some(index) = vehicle
            .riding
            .passengers
            .iter()
            .position(|seat| seat.key == entity)
        else {
            // Their seat is gone: they get off where they are, as one whose vehicle
            // did not come.
            self.give_up_waiting(id, output);
            return;
        };
        let mut record = vehicle.clone();
        let granted = record.riding.passengers[index]
            .pending
            .is_some_and(|pending| pending.granted);
        if record.riding.passengers[index].pending.take().is_some() {
            self.commit_entity(
                chunk,
                record.key,
                vec![EntityField::Riding(record.riding.clone())],
                output,
            );
        }
        let player = self.players.get_mut(&id).expect("found above");
        let kept = std::mem::take(&mut player.kept);
        if let Some(rides) = player.riding.as_mut() {
            rides.chunk = chunk;
            rides.pending = None;
        }
        self.journal.players.insert(id);
        self.seat_player(id, output);
        // A seat granted across a line is taken for the first time: a mount, which the
        // player's client and the vehicle's watchers learn of now.
        if granted {
            self.first_seating(id, riding.vehicle, output);
        }
        // The moves their client sent meanwhile, each as one client tick, the check of
        // the first counting from where the vehicle is now, as `tickPlayer` would seed it
        // (`docs/groundwork/n5-plan.md`, section 7 item 4).
        if !kept.is_empty() {
            self.seed_vehicle_check(id, riding.vehicle);
            // Twenty kept are as many as are kept: some may have been let go before
            // them, and then the first is taken as the game takes a driver's first
            // move, without the "too quickly" test (section 8 item 10).
            let some_dropped = kept.len() >= crate::state::KEPT_MOVES;
            for (index, moved) in kept.into_iter().enumerate() {
                // Each as one client tick: the check counts from where the last left
                // the vehicle, as the end of a player's tick would seed it.
                if index > 0 {
                    self.seed_vehicle_check(id, riding.vehicle);
                }
                self.move_vehicle_tested(id, moved, !(some_dropped && index == 0), output);
            }
        }
    }

    /// Seeds the check of the player `id`'s moves of their vehicle `vehicle` from where
    /// its root is now, as `tickPlayer` does at the end of a driver's tick.
    fn seed_vehicle_check(&mut self, id: PlayerId, vehicle: EntityId) {
        let root = self.root_of(vehicle);
        let Some(position) = self.find_entity(root).map(|(_, root)| root.position) else {
            return;
        };
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        if let Some(rides) = player.riding.as_mut() {
            rides.check = crate::physics::vehicle_move::seeded(&rides.check, position);
        }
    }

    /// The player `id`, whose unit did not come in time or kept no seat for them, gets
    /// off where they are and is never seated in it later; their client is told so.
    fn give_up_waiting(&mut self, id: PlayerId, output: &mut TickOutput) {
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        player.riding = None;
        player.kept.clear();
        self.journal.players.insert(id);
        let pose = player.pose;
        output.player_events.push((
            id,
            PlayerEvent::Rides {
                vehicle: None,
                pose,
            },
        ));
    }

    /// A move of their vehicle by the client of a player who waits for it: kept, the
    /// oldest let go beyond [`crate::state::KEPT_MOVES`].
    /// **Named**: a move kept so is judged at the seating by who controls the vehicle
    /// then, not when it came; a rider who puts their carrot on a stick away in the one
    /// to three ticks of a crossing has those moves taken as a passenger's
    /// (`docs/groundwork/n7h-riding-plan.md`, section 10 item 7: what is kept would have
    /// to change the transfer's format).
    pub(crate) fn keep_vehicle_move(&mut self, id: PlayerId, moved: crate::api::VehicleMove) {
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        if player.kept.len() >= crate::state::KEPT_MOVES {
            player.kept.remove(0);
        }
        player.kept.push(moved);
        self.journal.players.insert(id);
    }

    /// Counts down the seats of `vehicle` kept for players on their way, and lets go of
    /// one whose time is up: its player gets off where they are, wherever they are.
    pub(crate) fn count_pending_seats(&mut self, vehicle: EntityId, output: &mut TickOutput) {
        let Some((chunk, record)) = self.find_entity(vehicle) else {
            return;
        };
        if !record
            .riding
            .passengers
            .iter()
            .any(|seat| seat.pending.is_some())
        {
            return;
        }
        let mut record = record.clone();
        // A seat kept for a player who is here and does not wait for this vehicle (who
        // came riding nothing, or another, or gave up waiting) goes at once: they are
        // never seated in it later.
        let here_not_waiting: Vec<bool> = record
            .riding
            .passengers
            .iter()
            .map(|seat| {
                seat.pending.is_some()
                    && self
                        .player_with_entity(seat.key)
                        .and_then(|id| self.players.get(&id))
                        .is_some_and(|player| {
                            player.riding.is_none_or(|riding| {
                                riding.vehicle != vehicle || riding.pending.is_none()
                            })
                        })
            })
            .collect();
        for (seat, gone) in record.riding.passengers.iter_mut().zip(here_not_waiting) {
            if let Some(pending) = seat.pending.as_mut() {
                pending.left = if gone {
                    0
                } else {
                    pending.left.saturating_sub(1)
                };
            }
        }
        let shown_before = record.riding.shown_passengers().len();
        record
            .riding
            .passengers
            .retain(|seat| seat.pending.is_none_or(|pending| pending.left > 0));
        // A granted seat never shown goes unreported, and is no seat's going to the
        // vehicle's type (`docs/groundwork/n7j-mount-across-a-line.md`, section 8).
        let dropped = record.riding.shown_passengers().len() != shown_before;
        self.commit_entity(
            chunk,
            vehicle,
            vec![EntityField::Riding(record.riding.clone())],
            output,
        );
        if dropped {
            output.events.push(RegionEvent::Passengers {
                vehicle,
                chunk,
                hops: record.hops,
                passengers: record.riding.shown_passengers(),
            });
            // Once for all that lapsed: what a type does at a seat's going is the same
            // for one or several (the happy ghast's).
            self.passenger_removed(&record, chunk, EntityId(0), output);
        }
    }

    /// Lets go of every seat kept for the player of the stay `entity`, whose hand-over
    /// was discarded: they do not come now (ADR-0028, section 5).
    pub(crate) fn drop_pending_seat_of_player(
        &mut self,
        entity: EntityId,
        output: &mut TickOutput,
    ) {
        self.drop_pending_seats(|seat| seat.key == entity, output);
    }

    /// Lets go of every seat kept for a player of the edge `edge`, which has gone: none
    /// of them comes now (`docs/groundwork/n5-plan.md`, section 8 item 9).
    pub(crate) fn drop_pending_seats_of(
        &mut self,
        edge: clustine_world::EdgeId,
        output: &mut TickOutput,
    ) {
        self.drop_pending_seats(
            |seat| seat.pending.is_some_and(|pending| pending.edge == edge),
            output,
        );
    }

    /// Lets go of every kept seat that `goes` says of, in the region's loaded chunks.
    fn drop_pending_seats(&mut self, goes: impl Fn(&Seat) -> bool, output: &mut TickOutput) {
        let goes = |seat: &Seat| seat.pending.is_some() && goes(seat);
        let units: Vec<(EntityId, ChunkPos)> = self
            .chunks
            .iter()
            .flat_map(|(position, chunk)| {
                chunk
                    .entities()
                    .iter()
                    .filter(|entity| entity.riding.passengers.iter().any(&goes))
                    .map(|entity| (entity.key, *position))
                    .collect::<Vec<_>>()
            })
            .collect();
        for (key, chunk) in units {
            let Some(mut record) = self.entity_record(key, chunk).cloned() else {
                continue;
            };
            let shown_before = record.riding.shown_passengers().len();
            record.riding.passengers.retain(|seat| !goes(seat));
            let shown_gone = record.riding.shown_passengers().len() != shown_before;
            self.commit_entity(
                chunk,
                key,
                vec![EntityField::Riding(record.riding.clone())],
                output,
            );
            output.events.push(RegionEvent::Passengers {
                vehicle: key,
                chunk,
                hops: record.hops,
                passengers: record.riding.shown_passengers(),
            });
            if shown_gone {
                self.passenger_removed(&record, chunk, EntityId(0), output);
            }
        }
    }

    /// The waiting players' and the forwarding entries' part of the tick: a player whose
    /// unit has come is seated, one who has waited [`PENDING_FOR`] ticks gets off where
    /// they are, and an entry whose time is up goes.
    pub(crate) fn tick_waiting_riders(&mut self, output: &mut TickOutput) {
        // One whose unit went to another region's land leaves at the end of the tick and
        // waits there, not here.
        let waiting: Vec<PlayerId> = self
            .players
            .iter()
            .filter(|(_, player)| {
                player.riding.is_some_and(|riding| {
                    riding.pending.is_some()
                        && !matches!(
                            self.land.knowledge(riding.chunk),
                            super::Knowledge::Foreign(_)
                        )
                })
            })
            .map(|(id, _)| *id)
            .collect();
        for id in waiting {
            self.rider_arrived(id, output);
            let Some(player) = self.players.get_mut(&id) else {
                continue;
            };
            let Some(rides) = player.riding.as_mut() else {
                continue;
            };
            let Some(left) = rides.pending.as_mut() else {
                continue;
            };
            *left = left.saturating_sub(1);
            if *left == 0 {
                self.give_up_waiting(id, output);
            } else {
                self.journal.players.insert(id);
            }
        }
        self.forwarded.retain(|_, (_, left)| {
            *left = left.saturating_sub(1);
            *left > 0
        });
    }
}
