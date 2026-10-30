//! The region's side of the water creatures (`crate::mobs::types`; the brief's sections
//! 6.6 to 6.9): a bucket of a fish poured (`MobBucketItem`), the schools a fish's tick
//! joins and leaves, a pufferfish's sting dealt to another mob, and an item a dolphin
//! takes.
//!
//! Named differences: a school's writes go only to fish of the region's own (a school
//! keeps to one region, `mobs/types/fish.rs`); a sting reaches only a mob the region
//! holds.

use clustine_world::{EntityData, EntityId, PlayerId};

use super::Region;
use super::dispensers::empty_sound;
use crate::api::{RegionEvent, TickOutput};
use crate::mobs::Mob;
use crate::mobs::body::hurt::{Causer, DamageSource, DamageType};
use crate::mobs::types::{self, fish, water};
use crate::physics::Vec3;
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::placement::ItemUse;

impl Region {
    /// `BucketItem.use` of a bucket of a mob by the region's player `player`: its water
    /// emptied as a water bucket's into the block before what the ray hits (or into the
    /// hit block where that takes water), nothing where water is there already, with the
    /// bucket's own sound, heard by all but the player (and no `FLUID_PLACE`: the mob
    /// bucket's sound replaces the bucket's, which made it); then its mob made at that
    /// block (`MobBucketItem.spawn`, `EntityType.create` with `BUCKET`: as an egg makes
    /// it, its `finalizeSpawn` drawn again but where its type's returns at once, an
    /// axolotl's), marked as from a bucket where its type keeps that. The creative
    /// player keeps the bucket. Whether it did anything.
    pub(crate) fn use_mob_bucket(
        &mut self,
        used: &ItemUse,
        player: PlayerId,
        output: &mut TickOutput,
    ) -> bool {
        let Some(item) = used.held else {
            return false;
        };
        let Some(kind) = water::poured_by(item) else {
            return false;
        };
        // A type no part has built makes nothing, as its egg does not.
        if !types::is_built(kind) {
            return false;
        }
        let Some((pos, changes)) = crate::placement::mob_bucket(used, &super::Readable(self))
        else {
            return false;
        };
        if !changes.is_empty() && !self.change_all(&changes, output) {
            return false;
        }
        // Water that evaporates fizzes instead of the bucket's sound, and the mob is made
        // all the same (`MobBucketItem.checkExtraContent`; `ice.rs`).
        if let Some(at) = crate::placement::evaporated(used, &super::Readable(self)) {
            self.evaporate_player_s_water(player, at, output);
        } else {
            output.events.push(RegionEvent::Sound {
                sound: empty_sound(item),
                source: SOUND_SOURCE_NEUTRAL,
                position: clustine_world::Vec3::new(
                    f64::from(pos.x) + 0.5,
                    f64::from(pos.y) + 0.5,
                    f64::from(pos.z) + 0.5,
                ),
                volume: 1.0,
                pitch: 1.0,
                except: Some(player),
            });
        }
        self.spawn_mob_with(
            kind,
            player,
            pos,
            (true, false),
            true,
            |mob| types::of(kind).poured_from_bucket(kind, mob),
            output,
        );
        true
    }

    /// The region's dolphin `taker` took the item entity `item` whole (`take(item,
    /// count)` and its `discard()`): shown as taken, then gone.
    pub(crate) fn item_taken_by_mob(
        &mut self,
        taker: EntityId,
        item: EntityId,
        count: i32,
        output: &mut TickOutput,
    ) {
        let Some((chunk, record)) = self.find_entity(item) else {
            return;
        };
        if !matches!(record.data, EntityData::Item { .. }) {
            return;
        }
        let hops = record.hops;
        output.events.push(RegionEvent::ItemTaken {
            item,
            collector: taker,
            count,
            chunk,
        });
        self.remove_entity(item, hops, chunk, output);
    }

    /// `take(item, count)` of `count` of the region's item `item` by the mob `taker`: shown
    /// as taken, and the rest left lying, or the item gone where none is left.
    pub(crate) fn item_partly_taken_by_mob(
        &mut self,
        taker: EntityId,
        item: EntityId,
        count: i32,
        output: &mut TickOutput,
    ) {
        let Some((chunk, record)) = self.find_entity(item) else {
            return;
        };
        let EntityData::Item { ref stack, .. } = record.data else {
            return;
        };
        if count >= stack.count {
            self.item_taken_by_mob(taker, item, count, output);
            return;
        }
        let mut left = record.clone();
        if let EntityData::Item { stack, .. } = &mut left.data {
            stack.count -= count;
        }
        output.events.push(RegionEvent::ItemTaken {
            item,
            collector: taker,
            count,
            chunk,
        });
        let data = left.data.clone();
        self.commit_entity(
            chunk,
            item,
            vec![clustine_world::EntityField::Data(data)],
            output,
        );
        output
            .events
            .push(RegionEvent::EntityTracked(super::items::tracked(
                &left, 0, true,
            )));
    }

    /// The fish of the region's own among `followers` follow `leader`, which gains
    /// `leader_gains` in its school where the region holds it (`startFollowing` of each,
    /// written by another fish's gathering).
    pub(crate) fn join_school(
        &mut self,
        leader: EntityId,
        followers: &[EntityId],
        leader_gains: i32,
        output: &mut TickOutput,
    ) {
        for &follower in followers {
            self.change_fish(follower, output, |kind, mob| {
                fish::start_following(kind, mob, leader);
            });
        }
        if leader_gains != 0 {
            self.change_fish(leader, output, |kind, mob| {
                fish::school_grew(kind, mob, leader_gains);
            });
        }
    }

    /// A follower left the school of the region's fish `leader` (`stopFollowing`'s
    /// write): one fewer in it.
    pub(crate) fn leave_school(&mut self, leader: EntityId, output: &mut TickOutput) {
        self.change_fish(leader, output, |kind, mob| fish::school_grew(kind, mob, -1));
    }

    /// Changes the fish state of the region's fish `key` between ticks, and commits it.
    fn change_fish(
        &mut self,
        key: EntityId,
        output: &mut TickOutput,
        change: impl FnOnce(u16, &mut Mob),
    ) {
        let Some((chunk, record)) = self.find_entity(key).map(|(at, found)| (at, found.clone()))
        else {
            return;
        };
        if fish::Species::of(record.kind).is_none() {
            return;
        }
        let Ok(mut mob) = Mob::decode(&record.data) else {
            return;
        };
        change(record.kind, &mut mob);
        self.commit_mob(&record, chunk, &mob, output);
    }

    /// `hurtServer(source, amount)` of the region's mob `key` by the mob `attacker` of
    /// the type `kind` standing at `from`, of `damage` (`mobAttack(attacker)`, or a
    /// guardian's `thorns(attacker)`), and where it took, `sound` at `from` (a
    /// pufferfish's sting).
    pub(crate) fn hurt_by_mob(
        &mut self,
        (attacker, kind, from): (EntityId, u16, Vec3),
        key: EntityId,
        (damage, amount): (DamageType, f32),
        sound: Option<i32>,
        output: &mut TickOutput,
    ) -> bool {
        if self.find_entity(key).is_none() {
            return false;
        }
        let source = DamageSource::by(
            damage,
            Causer {
                key: attacker,
                kind,
                position: from,
                player: None,
                living: true,
            },
        );
        let took = self.hurt_mob(key, &source, amount, output);
        if took && let Some(sound) = sound {
            output.events.push(RegionEvent::Sound {
                sound,
                source: SOUND_SOURCE_NEUTRAL,
                position: clustine_world::Vec3::new(from[0], from[1], from[2]),
                volume: 1.0,
                pitch: 1.0,
                except: None,
            });
        }
        took
    }
}
