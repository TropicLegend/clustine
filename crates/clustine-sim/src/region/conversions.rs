//! A mob turned into a mob of another type where it is (`Mob.convertTo` with
//! `ConversionType.SINGLE`; `docs/groundwork/n7k-brief.md`, section 11.5): a zombie
//! drowned into a drowned, a husk into a zombie, a skeleton frozen into a stray.

use clustine_world::EntityId;

use super::Region;
use crate::api::{RegionEvent, TickOutput};
use crate::mobs::Mob;
use crate::mobs::making::{construct, fresh_record};
use crate::mobs::types;

impl Region {
    /// `convertTo(into, single(mob, keep_equipment, _), after)` of the region's mob `key`:
    /// a new mob of the type `into`, constructed of its own stream under a new key, with
    /// what `ConversionType.SINGLE` and `convertCommon` copy of the old one; no
    /// `finalizeSpawn` and nothing else drawn but the old type's step after
    /// (`MobType::finalize_conversion`); added before the old one goes, so that clients
    /// are told of the new one first; the old one's vehicle ridden by the new one and its
    /// first passenger seated on it; the level event `sound` at the new mob's block.
    ///
    /// **Named**: the shared fire flag the game copies for a tick is not (the fire ticks
    /// are not copied either way), and a leash is not moved, as no mob that converts in
    /// N7k takes a lead.
    pub(crate) fn convert_mob_to(
        &mut self,
        key: EntityId,
        into: u16,
        keep_equipment: bool,
        sound: Option<i32>,
        output: &mut TickOutput,
    ) {
        let Some((_, old_record)) = self.find_entity(key).map(|(at, found)| (at, found.clone()))
        else {
            return;
        };
        let Ok(mut old) = Mob::decode(&old_record.data) else {
            return;
        };
        let vehicle = old_record.riding.vehicle;
        let passenger = old_record
            .riding
            .passengers
            .first()
            .filter(|seat| !seat.player)
            .map(|seat| seat.key);
        // The first passenger is taken off, and the old mob leaves its vehicle, before the
        // new one is placed, so that each is where it is let off.
        if let Some(passenger) = passenger {
            self.stop_riding(passenger, output);
        }
        if vehicle.is_some() {
            self.stop_riding(key, output);
        }
        let Some((chunk, old_record)) =
            self.find_entity(key).map(|(at, found)| (at, found.clone()))
        else {
            return;
        };
        let Some(new_key) = self.take_entity_key(old_record.chunk()) else {
            return;
        };
        let mut own = self.entity_stream(new_key).random;
        let start = own.clone();
        let (uuid, mut new) = construct(into, &mut own, old_record.yaw);
        let old_kind = old_record.kind;
        copy_single(&mut old, &mut new, keep_equipment);
        let old_type = types::of(old_kind);
        if old_type.is_baby(&old) {
            types::of(into).set_baby(&mut new, true);
        }
        old_type.finalize_conversion(into, &mut new, &mut own);
        // A zombie villager cured becomes the villager it was (`trading.rs`).
        self.cure_finished(&old_record, &old, into, &mut new, &mut own);
        self.entity_drew(new_key, own.draws_since(&start));
        let mut record = fresh_record(
            new_key,
            into,
            uuid,
            old_record.position,
            old_record.yaw,
            &new,
        );
        record.pitch = old_record.pitch;
        record.motion = old_record.motion;
        record.on_ground = old_record.on_ground;
        record.fall_distance = old_record.fall_distance;
        let block = crate::physics::containing(record.position);
        // The game plays the conversion's level event after `convertTo` added the new
        // mob, but it reaches clients at once, and the new mob only when the tracker
        // next runs: clients hear it first (`villagers_oracle.rs`).
        if let Some(kind) = sound {
            output.events.push(RegionEvent::LevelEvent {
                kind,
                position: block,
                data: 0,
            });
        }
        self.add_fresh_entity(record, output);
        if let Some(vehicle) = vehicle {
            self.start_riding(new_key, vehicle, true, output);
        }
        if let Some(passenger) = passenger {
            self.start_riding(passenger, new_key, true, output);
        }
        let hops = old_record.hops;
        self.unride_removed(key, true, output);
        self.remove_entity(key, hops, chunk, output);
    }
}

/// What `ConversionType.SINGLE` and `convertCommon` copy of the body from `old` to `new`
/// that Clustine keeps (K 11.5): the body's yaw, the hurt time and the player's memory of
/// a hurt, every slot of equipment with its drop chance where `keep_equipment` (and the
/// old one's emptied), its effects, the flags (no AI, the left hand, the loot pick-up, and persistence
/// only where the old one had it). Not its health, air, attributes or fire.
fn copy_single(old: &mut Mob, new: &mut Mob, keep_equipment: bool) {
    new.body.head.body_yaw = old.body.head.body_yaw;
    new.body.head.head_yaw = old.body.head.head_yaw;
    new.body.vitals.hurt_from = old.body.vitals.hurt_from;
    new.body.combat.last_hurt_by_player = old.body.combat.last_hurt_by_player;
    if keep_equipment {
        for slot in 0..old.body.equipment.slots.len() {
            if let Some(stack) = old.body.equipment.slots[slot].take() {
                new.body.equipment.slots[slot] = Some(stack);
                let chance = old.body.equipment.drop_chance(slot);
                let chances = new.body.equipment.drop_chances.get_or_insert([0.085; 8]);
                chances[slot] = chance;
            }
        }
    }
    // `convertCommon`: every active effect, a copy.
    new.body.effects = old.body.effects.clone();
    new.body.flags.no_ai = old.body.flags.no_ai;
    new.body.flags.left_handed = old.body.flags.left_handed;
    new.body.flags.picks_up_loot = old.body.flags.picks_up_loot;
    if old.body.flags.persistent {
        new.body.flags.persistent = true;
    }
}
