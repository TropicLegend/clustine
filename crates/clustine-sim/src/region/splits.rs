//! Mobs that part S9's types make where they stand (`docs/groundwork/n9-plan.md`,
//! section 8.4): a slime's split at its death, and the making of a mob by section 4.1's
//! rule that the silverfish of an infested block and the endermite of a pearl share.
//!
//! A mob is made in the region's own land as any is; in a chunk the region copies it is
//! made and sent to that chunk's holder as an arrival; where the region can read nothing
//! it is not made (section 4.1's rule).

use clustine_data::entity_types;
use clustine_world::{ChunkPos, Entity, EntityId};

use super::Region;
use crate::api::TickOutput;
use crate::mobs::Mob;
use crate::mobs::making::{construct, fresh_record};
use crate::mobs::types::cube_mob;
use crate::physics::Vec3;
use crate::region::Random;

/// What a maker of a mob does to it between its construction and its adding, given its
/// own stream: its `finalizeSpawn`, its size.
pub(super) type Prepare<'p> = &'p mut dyn FnMut(&mut Region, &mut Mob, &mut Random);

impl Region {
    /// A mob of the type `kind` made at `at` facing `yaw` and `pitch` by section 4.1's rule: under the
    /// region's next key, constructed of its own stream (its UUID, its constructor's yaw,
    /// what its type's constructor draws), `prepare` done to it, and added where it is, or
    /// sent to the holder of the chunk where the region copies it. Its key, or `None` and
    /// nothing made where the region can read nothing there, its chunk is full, or it has
    /// no key left.
    pub(super) fn make_mob(
        &mut self,
        kind: u16,
        at: Vec3,
        (yaw, pitch): (f32, f32),
        prepare: Prepare<'_>,
        output: &mut TickOutput,
    ) -> Option<EntityId> {
        let chunk = ChunkPos::containing(at[0], at[2]);
        let here = self.ticks_chunk(chunk);
        if !here && !self.readable(chunk) {
            return None;
        }
        let key = if here {
            self.take_entity_key(chunk)
        } else {
            self.take_key()
        }?;
        let mut own = self.entity_stream(key).random;
        let start = own.clone();
        let (uuid, mut mob) = construct(kind, &mut own, yaw);
        prepare(self, &mut mob, &mut own);
        let mut record: Entity = fresh_record(key, kind, uuid, at, yaw, &mob);
        record.pitch = pitch;
        if here {
            self.entity_drew(key, own.draws_since(&start));
            self.add_fresh_entity(record, output);
        } else {
            self.send_made(record, output);
        }
        Some(key)
    }

    /// `AbstractCubeMob.remove(KILLED)` of the dying mob `before` (its record as its last
    /// tick began), at `at` as that tick left it: for a cube larger than tiny, its split
    /// (`cube_mob::split`, of its own stream on from its tick), each new cube made by
    /// `convertTo(SPLIT_ON_DEATH)`: constructed of its own stream, with what
    /// `convertCommon` copies of the parent that Clustine keeps (its effects, its left
    /// hand, no AI, and persistence where the parent had it), its size set with full
    /// health, at the parent's feet set out and half a block up, facing its yaw.
    ///
    /// **Named**: the new cube's head and body face its yaw where the game leaves them at
    /// its constructor's; the parent's passenger and lead are not let go (no cube carries
    /// one in N9); a new cube in another region's land is made there by section 4.1's
    /// rule, and none where the region reads nothing.
    pub(crate) fn cube_split(
        &mut self,
        key: EntityId,
        before: &Entity,
        at: Vec3,
        output: &mut TickOutput,
    ) {
        if i32::from(before.kind) != entity_types::SLIME {
            return;
        }
        let Ok(parent) = Mob::decode(&before.data) else {
            return;
        };
        let mut draws = self.entity_stream(key).random;
        let start = draws.clone();
        let splits = cube_mob::split(before.kind, &parent, &mut draws);
        self.entity_drew(key, draws.draws_since(&start));
        let kind = before.kind;
        for split in splits {
            let place = [
                at[0] + f64::from(split.offset[0]),
                at[1] + 0.5,
                at[2] + f64::from(split.offset[1]),
            ];
            let mut prepare = |_: &mut Region, child: &mut Mob, _: &mut Random| {
                child.body.effects = parent.body.effects.clone();
                child.body.flags.left_handed = parent.body.flags.left_handed;
                child.body.flags.no_ai = parent.body.flags.no_ai;
                if parent.body.flags.persistent {
                    child.body.flags.persistent = true;
                }
                cube_mob::set_size(kind, child, split.size, true);
            };
            self.make_mob(kind, place, (split.yaw, 0.0), &mut prepare, output);
        }
    }
}

impl Region {
    /// `remove(KILLED)` of the region's mob `key` where it is dead or dying
    /// (`Local::RemoveDead`): as its death's end removes it, a cube's split and the effects
    /// at its removal first, but at once and without the smoke of `tickDeath`. Nothing for
    /// a mob alive, or not the region's.
    pub(crate) fn remove_if_dead(&mut self, key: EntityId, output: &mut TickOutput) {
        let Some((chunk, record)) = self
            .find_entity(key)
            .map(|(chunk, record)| (chunk, record.clone()))
        else {
            return;
        };
        let Ok(mob) = Mob::decode(&record.data) else {
            return;
        };
        if !mob.body.vitals.dying() {
            return;
        }
        self.cube_split(key, &record, record.position, output);
        let held = crate::mobs::body::held_effects(&mob.body.effects);
        self.effects_at_removal(key, record.position, &held, output);
        self.unride_removed(key, true, output);
        self.remove_entity(key, record.hops, chunk, output);
    }
}

#[cfg(test)]
mod tests {
    //! The region's side of part S9's makings (`docs/groundwork/n9-plan.md`, sections 4.4,
    //! 5.5 and 8.4): a slime's split at its death, an infested block's silverfish, a
    //! pearl's endermite and the chorus fruit's teleport, from the game's rules.

    use clustine_data::{blocks, entity_types};
    use clustine_world::{Entity, EntityChange, EntityId};

    use super::super::Region;
    use super::super::tnt::tests::{FLOOR, add_player, nothing, world};
    use crate::api::{PlayerEvent, RegionEvent, TickInputs, TickOutput};
    use crate::mobs::Mob;
    use crate::mobs::types::cube_mob;
    use crate::region::Random;

    const SEED: u64 = 0x5_1A1E;

    fn kind(id: i32) -> u16 {
        u16::try_from(id).expect("an entity type id")
    }

    /// The feet of a mob on the floor.
    const FEET: f64 = FLOOR as f64 + 1.0;

    /// Lays `entity` in the region's loaded chunk, as a load would have it.
    fn lay(region: &mut Region, entity: &Entity) {
        let chunk = entity.chunk();
        region
            .chunks
            .get_mut(&chunk)
            .expect("the chunk is loaded")
            .apply_entity(&EntityChange::Whole(entity.clone()));
        region.entity_chunks.insert(entity.key, chunk);
    }

    /// The region's loaded entities of the type `id`.
    fn of_kind(region: &Region, id: i32) -> Vec<Entity> {
        region
            .chunks
            .values()
            .flat_map(|chunk| chunk.entities().iter().cloned())
            .filter(|entity| i32::from(entity.kind) == id)
            .collect()
    }

    #[test]
    fn a_large_slime_at_the_end_of_its_death_splits_into_two_to_four_of_half_its_size() {
        let mut region = world(SEED, nothing);
        let slime = kind(entity_types::SLIME);
        let mut mob = Mob::fresh();
        cube_mob::set_size(slime, &mut mob, 4, true);
        mob.body.vitals.health = 0.0;
        mob.body.vitals.died_from = Some(0);
        let mut entity = crate::mobs::making::fresh_record(
            EntityId(500),
            slime,
            uuid::Uuid::from_u128(500),
            [0.5, FEET, 0.5],
            0.0,
            &mob,
        );
        entity.lived = 19;
        entity.on_ground = true;
        lay(&mut region, &entity);
        region.tick(&TickInputs::default());
        let slimes = of_kind(&region, entity_types::SLIME);
        assert!(
            region.find_entity(EntityId(500)).is_none(),
            "the parent is gone"
        );
        assert!((2..=4).contains(&slimes.len()), "{} made", slimes.len());
        for made in &slimes {
            let mob = Mob::decode(&made.data).expect("a mob");
            assert_eq!(cube_mob::size(&mob), 2);
            assert_eq!(mob.body.vitals.health, 4.0, "full health of its size");
            let [x, _, z] = made.position;
            assert!((x - 0.5).abs() <= 0.53 && (z - 0.5).abs() <= 0.53);
        }
    }

    #[test]
    fn an_infested_block_broken_makes_a_silverfish_at_its_bottom_middle() {
        let mut region = world(SEED, nothing);
        let mut output = TickOutput::default();
        let pos = clustine_world::BlockPos::new(3, FLOOR, 4);
        region.infested_broken(pos, blocks::STONE, &mut output);
        assert!(of_kind(&region, entity_types::SILVERFISH).is_empty());
        region.infested_broken(pos, blocks::INFESTED_STONE, &mut output);
        let made = of_kind(&region, entity_types::SILVERFISH);
        assert_eq!(made.len(), 1);
        assert_eq!(made[0].position, [3.5, f64::from(FLOOR), 4.5]);
        assert!(output.events.iter().any(|event| matches!(
            event,
            RegionEvent::EntityEvent { entity, event: 20, .. } if *entity == made[0].key
        )));
    }

    #[test]
    fn a_silverfish_waking_a_friend_breaks_its_block_and_the_friend_comes_out() {
        let at = clustine_world::BlockPos::new(2, FLOOR + 1, 2);
        let mut region = world(SEED, move |pos| {
            (pos == at).then_some(blocks::INFESTED_STONE)
        });
        let mut output = TickOutput::default();
        region.mob_breaks_block(at, &mut output);
        assert_eq!(region.block(at), Some(blocks::AIR));
        assert_eq!(of_kind(&region, entity_types::SILVERFISH).len(), 1);
    }

    #[test]
    fn a_pearl_s_endermite_is_made_at_its_thrower_s_feet_with_their_look() {
        let mut region = world(SEED, nothing);
        let (id, _) = add_player(&mut region, 1, [2.5, FEET, 2.5], false);
        if let Some(player) = region.players.get_mut(&id) {
            player.pose.yaw = 45.0;
            player.pose.pitch = 10.0;
        }
        let mut output = TickOutput::default();
        region.pearl_endermite(id, &mut output);
        let made = of_kind(&region, entity_types::ENDERMITE);
        assert_eq!(made.len(), 1);
        assert_eq!(made[0].position, [2.5, FEET, 2.5]);
        assert_eq!((made[0].yaw, made[0].pitch), (45.0, 10.0));
    }

    #[test]
    fn the_chorus_fruit_tries_up_to_sixteen_places_and_moves_its_eater_to_the_first_that_lands() {
        let mut region = world(SEED, nothing);
        let (id, _) = add_player(&mut region, 1, [0.5, FEET, 0.5], false);
        let mut random = Random::of_entity(3, EntityId(1), 0);
        let mut replay = random.clone();
        let mut output = TickOutput::default();
        let moved = region.chorus_teleport(id, 16.0, &mut random, &mut output);
        // Each try three doubles; a place lands where it is at or above the floor's top
        // (the floor is one layer of stone over air).
        let mut expected = None;
        for _ in 0..16 {
            let x = 0.5 + (replay.next_double() - 0.5) * 16.0;
            let y = (FEET + (replay.next_double() - 0.5) * 16.0).clamp(-64.0, 319.0);
            let z = 0.5 + (replay.next_double() - 0.5) * 16.0;
            if y >= FEET {
                expected = Some([x, y - (y.floor() - FEET), z]);
                break;
            }
        }
        assert_eq!(random, replay, "the tries' draws, no more");
        assert_eq!(moved, expected.is_some());
        let pose = region.players.get(&id).expect("the player").pose;
        if let Some([x, y, z]) = expected {
            assert_eq!(
                [pose.position.x, pose.position.y, pose.position.z],
                [x, y, z]
            );
            assert!(output.player_events.iter().any(|(to, event)| *to == id
                && matches!(event, PlayerEvent::Teleported { .. })));
        }
    }
}
