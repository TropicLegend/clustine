//! What a mob's brain does to its region after its tick (`docs/groundwork/n7j-plan.md`,
//! section 4.1 item 5): another mob's memories written, a door opened or closed, a mob
//! hit here or across a line.

use clustine_blocks::data::{flag, set_flag};
use clustine_world::{BlockPos, Double, EntityId, EntityUse, Float};

use super::Region;
use super::redstone::play;
use super::updates::UPDATE_LIMIT;
use super::vibrations::{Context, Source, Vibration};
use crate::api::TickOutput;
use crate::mobs::brain::{self, MemoryWrite};
use crate::mobs::{Mob, types};
use crate::physics::Vec3;

/// `UPDATE_CLIENTS | UPDATE_IMMEDIATE`, the flags of `DoorBlock.setOpen`'s `setBlock`;
/// the second is the client's alone.
const DOOR_FLAGS: u32 = 10;

impl Region {
    /// `writes` made to the memories of the region's mob `key`, in order: nothing where
    /// it is not the region's own (a copy's mind is not here), not a mob, or of a type
    /// without a brain. Sensed memories are their sensors' alone, which no write of the
    /// game's to another mob touches: one here would be dropped with the record's.
    pub(crate) fn write_mob_memories(
        &mut self,
        key: EntityId,
        writes: Vec<MemoryWrite>,
        output: &mut TickOutput,
    ) {
        let Some((chunk, record)) = self.find_entity(key) else {
            return;
        };
        let Some(provider) = types::of(record.kind).brain() else {
            return;
        };
        let record = record.clone();
        let Ok(mut mob) = Mob::decode(&record.data) else {
            return;
        };
        let mut memories = brain::memories_of(provider, &mob, record.lived);
        for write in writes {
            write.apply(&mut memories);
        }
        brain::write_memories(provider, &mut mob, memories);
        let fields = mob.changes_from(&record.data);
        if !fields.is_empty() {
            self.commit_entity(chunk, key, fields, output);
        }
    }

    /// `DoorBlock.setOpen`'s side in the region, for the mob `key`: the door at `pos`
    /// opened or closed where it still is a door the other way, its sound at `pitch` and
    /// its game event. A door in another region's land is not opened (named, plan section
    /// 3 item 7): the region sets only its own blocks.
    pub(crate) fn mob_set_door_open(
        &mut self,
        key: EntityId,
        pos: BlockPos,
        open: bool,
        pitch: f32,
        output: &mut TickOutput,
    ) {
        let Some(state) = self.block(pos) else {
            return;
        };
        if state.property("hinge").is_none() || flag(state, "open") == open {
            return;
        }
        let changed = set_flag(state, "open", open);
        if !self.set_block_updated(pos, changed, DOOR_FLAGS, UPDATE_LIMIT, output) {
            return;
        }
        if let Some(sound) = crate::placement::use_sound(changed) {
            play(pos, sound, 1.0, pitch, None, output);
        }
        let event = if open {
            Vibration::BLOCK_OPEN
        } else {
            Vibration::BLOCK_CLOSE
        };
        self.vibrate_at(event, pos, Context::of(Some(Source::of(key)), None));
    }

    /// A mob's hit on the mob `target` (`Local::Hurt`): the region's own is hurt, and
    /// where that took, knocked back by `knockback` (strength and the direction it is
    /// pushed from) and `sound` heard at `from`. A mob only a copy shows is hit by its
    /// holder (`EntityUse::MobHit`, plan section 3 item 5); one beyond the copies is
    /// not there, and nothing happens.
    #[expect(
        clippy::too_many_arguments,
        reason = "the hit's parts, as the attacker's tick gives them"
    )]
    pub(crate) fn hit_by_mob(
        &mut self,
        attacker: EntityId,
        kind: u16,
        from: Vec3,
        target: EntityId,
        damage: f32,
        (knockback, fling): (Option<[f64; 3]>, Option<f32>),
        sound: Option<i32>,
        output: &mut TickOutput,
    ) {
        if self.find_entity(target).is_none() {
            let hit = EntityUse::MobHit {
                attacker,
                kind,
                from: from.map(Double),
                damage: Float(damage),
                knockback: knockback.map(|push| push.map(Double)),
                fling: fling.map(Float),
                sound,
            };
            self.send_on_entity(target, hit);
            return;
        }
        let took = self.hurt_by_mob(attacker, kind, from, target, damage, sound, output);
        if took && let Some([strength, x, z]) = knockback {
            self.knock_mob(target, strength, x, z, output);
        }
        // The fling comes after the hurt and its own knockback, only where it took.
        if took && let Some(up) = fling {
            self.fling_mob(target, up, output);
        }
    }
}
