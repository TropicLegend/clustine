//! What a mob's type does as a seat on it is taken or let go (`addPassenger` and
//! `removePassenger`'s own parts; `docs/groundwork/n7j-riding-plan.md`, section 9 item
//! 1): a happy ghast's goggles and its still timeout. Every path that adds a seat to a
//! mob calls [`Region::passenger_adding`] before it does, and every path that takes one
//! out calls `passenger_removed` (`riding.rs`) once it has.

use clustine_world::{ChunkPos, Entity, EntityData, EntityId};

use super::Region;
use super::items::{View, tracked};
use crate::api::{RegionEvent, TickOutput};
use crate::mobs::Mob;
use crate::mobs::types;
use crate::physics::item::SOUND_SOURCE_NEUTRAL;

impl Region {
    /// `addPassenger(rider)`'s own part of the region's mob `vehicle`, before the seat is
    /// added: its seats as they are, but for one of `rider`'s it may still keep.
    pub(crate) fn passenger_adding(
        &mut self,
        vehicle: EntityId,
        rider: EntityId,
        output: &mut TickOutput,
    ) {
        let Some((chunk, record)) = self.find_entity(vehicle) else {
            return;
        };
        if !matches!(record.data, EntityData::Mob { .. }) {
            return;
        }
        let before = record.clone();
        let mut seats = before.clone();
        seats.riding.passengers.retain(|seat| seat.key != rider);
        let Ok(mut mob) = Mob::decode(&before.data) else {
            return;
        };
        let sound = {
            let view = View {
                region: self,
                home: chunk,
            };
            types::of(before.kind).passenger_added(&seats, &mut mob, &view)
        };
        self.mob_seats_changed(chunk, &before, &mob, sound, output);
    }

    /// `removePassenger`'s own part of the region's mob `key`, whose seats are as the
    /// seat's going left them.
    pub(crate) fn mob_passenger_removed(
        &mut self,
        key: EntityId,
        chunk: ChunkPos,
        output: &mut TickOutput,
    ) {
        let Some((_, record)) = self.find_entity(key) else {
            return;
        };
        let before = record.clone();
        let Ok(mut mob) = Mob::decode(&before.data) else {
            return;
        };
        let sound = types::of(before.kind).passenger_removed(&before, &mut mob);
        self.mob_seats_changed(chunk, &before, &mob, sound, output);
    }

    /// What watchers hear of a seat's going from `vehicle`, a mob leaving the world with
    /// its rider, whose record is kept as it was.
    pub(crate) fn mob_passenger_removed_heard(
        &mut self,
        vehicle: &Entity,
        output: &mut TickOutput,
    ) {
        let Ok(mut mob) = Mob::decode(&vehicle.data) else {
            return;
        };
        if let Some(sound) = types::of(vehicle.kind).passenger_removed(vehicle, &mut mob) {
            heard(vehicle, sound, output);
        }
    }

    /// `addPassenger`'s own part of `vehicle`, a mob of a unit coming back to the world
    /// with its rider and not in the region's chunks yet: its data as the part leaves it,
    /// where it changed. The players above it are those the region sees there, its own
    /// and the faces.
    pub(crate) fn mob_passenger_added_away(
        &mut self,
        vehicle: &Entity,
        output: &mut TickOutput,
    ) -> Option<EntityData> {
        if !matches!(vehicle.data, EntityData::Mob { .. }) {
            return None;
        }
        let mut mob = Mob::decode(&vehicle.data).ok()?;
        let sound = {
            let view = View {
                region: self,
                home: vehicle.chunk(),
            };
            types::of(vehicle.kind).passenger_added(vehicle, &mut mob, &view)
        };
        if let Some(sound) = sound {
            heard(vehicle, sound, output);
        }
        let data = mob.encode();
        (data != vehicle.data).then_some(data)
    }

    /// The mob `before` with its own part `mob` of a seat's change: committed and shown
    /// where it changed, and `sound` made at it.
    fn mob_seats_changed(
        &mut self,
        chunk: ChunkPos,
        before: &Entity,
        mob: &Mob,
        sound: Option<i32>,
        output: &mut TickOutput,
    ) {
        if let Some(sound) = sound {
            heard(before, sound, output);
        }
        let fields = mob.changes_from(&before.data);
        if fields.is_empty() {
            return;
        }
        self.commit_entity(chunk, before.key, fields, output);
        if let Some((_, after)) = self.find_entity(before.key) {
            let after = after.clone();
            let mut report = tracked(&after, 0, true);
            report.chunk = chunk;
            output.events.push(RegionEvent::EntityTracked(report));
            self.mob_shown_changes(before, &after, chunk, output);
        }
    }
}

/// `Entity.playSound(sound)`: at volume 1 and pitch 1, of the entity's own source.
fn heard(at: &Entity, sound: i32, output: &mut TickOutput) {
    output.events.push(RegionEvent::Sound {
        sound,
        source: SOUND_SOURCE_NEUTRAL,
        position: clustine_world::Vec3::new(at.position[0], at.position[1], at.position[2]),
        volume: 1.0,
        pitch: 1.0,
        except: None,
    });
}
