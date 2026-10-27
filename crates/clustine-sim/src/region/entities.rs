//! Entities other than players (`docs/adr/0028-entities.md`, sections 2 and 4;
//! `docs/groundwork/n2-plan.md`, sections 7 and 11): they live in their chunks, the
//! region keeps which chunk each is in, they tick after the block events in ascending
//! key, and what a tick does to them goes into its output to be committed.
//!
//! The one entity so far is the item a player throws. It lies where it was thrown until
//! N4 brings physics: its tick only counts its age and its pick-up delay down, which
//! the region keeps in the chunk and commits nothing for, so that an item at rest costs
//! the log nothing. Its counters are kept as counts and not as ticks of the region, so
//! that what a chunk holds is always as of its last tick and nothing needs rebasing
//! when another region takes it in; while its chunk does not tick they stand still, as
//! the game's do. A crash loses what they counted since the item's last record: as if
//! its chunk had not been loaded for those ticks, a difference the plan names.

use clustine_data::{ITEMS, entity_types};
use clustine_noise::trig;
use clustine_world::{
    ChunkPos, Entity, EntityChange, EntityData, EntityField, EntityId, MAX_CHUNK_ENTITIES, PlayerId,
};
use uuid::Uuid;

use super::Region;
use crate::api::{EntityKind, EntityState, ItemStack, Pose, RegionEvent, TickOutput};

/// How far above a player's feet their eyes are, standing and crouching: the game's
/// `Avatar` dimensions, which it keeps as floats.
const STANDING_EYES: f32 = 1.62;
pub(super) const CROUCHING_EYES: f32 = 1.27;

/// How far below the eyes an item leaves from (`LivingEntity.createItemStackToDrop`).
const BELOW_THE_EYES: f64 = 0.300_000_011_920_928_96;

/// The ticks before a thrown item may be picked up.
const THROWN_PICK_UP_DELAY: u16 = 40;

/// The game's `Mth.DEG_TO_RAD`, a float.
const DEGREES_TO_RADIANS: f32 = 0.017_453_292;

/// How many draws a throw takes from the thrower's stream.
const THROW_DRAWS: usize = 4;

/// The motion the game gives an item a player throws, `LivingEntity.createItemStackToDrop`
/// without its scattering, from the thrower's yaw and pitch in degrees and the four
/// floats it draws from the thrower's random, in the order drawn: straight ahead at 0.3
/// and a little up, and off that in a direction of the first draw by up to 0.02 by the
/// second; the last two lift it or drop it a little. The yaw and the pitch go through
/// `Mth`'s table of sines and the direction through `StrictMath`'s sine and cosine (the
/// game's `Math`, which the plan takes as `StrictMath`, ADR-0028 section 4), each in the
/// game's widths and order.
pub(crate) fn throw_motion(yaw: f32, pitch: f32, draws: [f32; THROW_DRAWS]) -> [f64; 3] {
    let sin_pitch = trig::sin(f64::from(pitch * DEGREES_TO_RADIANS));
    let cos_pitch = trig::cos(f64::from(pitch * DEGREES_TO_RADIANS));
    let sin_yaw = trig::sin(f64::from(yaw * DEGREES_TO_RADIANS));
    let cos_yaw = trig::cos(f64::from(yaw * DEGREES_TO_RADIANS));
    let direction = f64::from(draws[0] * 6.283_185_5);
    let spread = f64::from(0.02 * draws[1]);
    [
        f64::from(-sin_yaw * cos_pitch * 0.3) + libm::cos(direction) * spread,
        f64::from(-sin_pitch * 0.3 + 0.1 + (draws[2] - draws[3]) * 0.1),
        f64::from(cos_yaw * cos_pitch * 0.3) + libm::sin(direction) * spread,
    ]
}

/// A UUID of version 4 from two draws, as the game's `Mth.createInsecureUUID` makes one
/// of two longs.
pub(crate) fn insecure_uuid(most: u64, least: u64) -> Uuid {
    Uuid::from_u64_pair(
        most & !0xF000 | 0x4000,
        least & 0x3FFF_FFFF_FFFF_FFFF | 0x8000_0000_0000_0000,
    )
}

/// The state an edge is shown of `entity`.
pub(crate) fn state_of(entity: &Entity) -> EntityState {
    EntityState {
        entity: entity.key,
        kind: EntityKind::Other {
            kind: entity.kind,
            uuid: entity.uuid,
            motion: entity.motion,
            hops: entity.hops,
            data: entity.data.clone(),
            riding: entity.riding.clone(),
        },
        pose: Pose {
            position: clustine_world::Vec3::new(
                entity.position[0],
                entity.position[1],
                entity.position[2],
            ),
            yaw: entity.yaw,
            pitch: entity.pitch,
            on_ground: entity.on_ground,
        },
    }
}

/// What the region's tick reads and writes of one thing a player threw.
pub(crate) struct Thrown {
    pub(crate) player: PlayerId,
    pub(crate) stack: ItemStack,
    pub(crate) crouching: bool,
}

impl Region {
    /// The entities other than players of the loaded chunks, in ascending key.
    pub(crate) fn other_entities(&self) -> impl Iterator<Item = EntityState> + '_ {
        self.entity_chunks.iter().filter_map(|(key, chunk)| {
            let chunk = self.chunks.get(chunk)?;
            let entities = chunk.entities();
            let at = entities
                .binary_search_by_key(key, |entity| entity.key)
                .ok()?;
            Some(state_of(&entities[at]))
        })
    }

    /// The entity other than a player with the key `key`, if a loaded chunk has it.
    pub(crate) fn other_entity(&self, key: EntityId) -> Option<EntityState> {
        let chunk = self.chunks.get(self.entity_chunks.get(&key)?)?;
        let entities = chunk.entities();
        let at = entities
            .binary_search_by_key(&key, |entity| entity.key)
            .ok()?;
        Some(state_of(&entities[at]))
    }

    /// Takes in the entities of the chunk at `position`, loaded: they are indexed, and,
    /// unless `continued`, taken in by this region, which raises each one's changes of
    /// holder and commits it whole (ADR-0028, section 6). Every load from the store is
    /// a take-in, a restore's among them, and so is the load of a warm chunk after a
    /// merge or a split: a region that a merge or a split made is what a restore makes
    /// of the store's record (ADR-0014), and cannot tell the one from the other. The
    /// plan would have a merge raise nothing; that it does is one record of each of
    /// its entities more, and the edge takes the higher word as it would the equal.
    /// What continues is a region made with its chunks loaded
    /// ([`Region::restore_loaded`]).
    pub(crate) fn take_up_entities(
        &mut self,
        position: ChunkPos,
        continued: bool,
        mut output: Option<&mut TickOutput>,
    ) {
        let Some(chunk) = self.chunks.get_mut(&position) else {
            return;
        };
        // A seat of a player the region does not have and keeps for nobody is no
        // seat: they left, or are another region's (`docs/groundwork/n5-plan.md`,
        // section 7 item 5 and section 8 item 1). Nor is one of a player here who
        // does not ride this entity: they were let off while it was out of reach.
        // Kept seats with time left stay.
        let riding: std::collections::BTreeMap<EntityId, Option<EntityId>> = self
            .players
            .values()
            .map(|player| (player.entity_id, player.riding.map(|riding| riding.vehicle)))
            .collect();
        let phantom = |entity: EntityId, seat: &clustine_world::Seat| {
            seat.player && seat.pending.is_none() && riding.get(&seat.key) != Some(&Some(entity))
        };
        let unseated: Vec<Entity> = chunk
            .entities()
            .iter()
            .filter(|entity| {
                entity
                    .riding
                    .passengers
                    .iter()
                    .any(|seat| phantom(entity.key, seat))
            })
            .map(|entity| {
                let mut entity = entity.clone();
                let key = entity.key;
                entity.riding.passengers.retain(|seat| !phantom(key, seat));
                entity
            })
            .collect();
        for entity in &unseated {
            chunk.apply_entity(&EntityChange::Set(
                entity.key,
                vec![EntityField::Riding(entity.riding.clone())],
            ));
        }
        let keys: Vec<EntityId> = chunk.entities().iter().map(|entity| entity.key).collect();
        if let Some(output) = output.as_deref_mut() {
            for entity in unseated {
                output.entity_changes.push((
                    position,
                    EntityChange::Set(entity.key, vec![EntityField::Riding(entity.riding)]),
                ));
            }
        }
        if !continued && let Some(output) = output {
            let game_time = i64::try_from(self.clock.game_time).unwrap_or(i64::MAX);
            let raised: Vec<Entity> = chunk
                .entities()
                .iter()
                .map(|entity| {
                    let mut raised = Entity {
                        hops: entity.hops.saturating_add(1),
                        ..entity.clone()
                    };
                    // Taken for the first time: generation made it, and no take has raised
                    // it yet (`docs/groundwork/n7l-plan.md`, section 2.5).
                    if entity.key.is_generated()
                        && entity.hops == 0
                        && let Ok(mut mob) = crate::mobs::Mob::decode(&entity.data)
                    {
                        crate::mobs::types::of(entity.kind).first_take(&mut mob, game_time);
                        raised.data = mob.encode();
                    }
                    raised
                })
                .collect();
            for entity in raised {
                let change = EntityChange::Whole(entity);
                chunk.apply_entity(&change);
                output.entity_changes.push((position, change));
            }
        }
        for key in keys {
            self.entity_chunks.insert(key, position);
            // What the game keeps in memory starts again at a take-in.
            self.entity_memory.remove(&key);
        }
    }

    /// A player's hit on, or use of, the entity `key`, made in another region and done
    /// here, where the entity is (`docs/groundwork/n5-plan.md`, decision 10): each kind
    /// answers it in its own hook, which the step that brings the kind fills; one gone by
    /// now is passed over.
    pub(crate) fn entity_used(
        &mut self,
        key: EntityId,
        by: clustine_world::EntityUse,
        output: &mut TickOutput,
    ) {
        // An explosion's hit across a line is the same for every kind as a hit from
        // within (`explosions.rs`).
        if let clustine_world::EntityUse::Blasted {
            damage,
            push,
            blast,
        } = by
        {
            self.take_blasted(key, damage, push, blast, output);
            return;
        }
        // A thrown item's hit across a line is taken as one from within, by the hit
        // entity's kind (`thrown.rs`).
        if let Some(hit) = super::thrown::Hit::of_use(&by) {
            self.thrown_hit_here(key, hit, output);
            return;
        }
        // The decline of a seat granted across a line is the same for every vehicle
        // (`granted.rs`).
        if let clustine_world::EntityUse::SeatDeclined { player } = by {
            self.seat_declined(key, player, output);
            return;
        }
        let Some((chunk, entity)) = self.find_entity(key) else {
            return;
        };
        let entity = entity.clone();
        // A use by another region's player, sent across a line: a mount it ends in is
        // theirs to take (`granted.rs`).
        let across = match by {
            clustine_world::EntityUse::Interact {
                player,
                from,
                edge: Some(edge),
                ..
            } if self.player_with_entity(player).is_none() => Some(super::granted::Across {
                edge,
                from: from.map(|value| value.0),
            }),
            _ => None,
        };
        let outer = std::mem::replace(&mut self.using_across, across);
        match entity.data {
            EntityData::Minecart { .. } => self.minecart_used(&entity, chunk, by, output),
            EntityData::Boat { .. } => self.boat_used(&entity, chunk, by, output),
            EntityData::ArmorStand { .. } => self.armor_stand_used(&entity, chunk, by, output),
            EntityData::ItemFrame { .. } | EntityData::Painting { .. } => {
                self.hanging_used(&entity, chunk, by, output);
            }
            EntityData::Cushion { .. } => self.cushion_used(&entity, chunk, by, output),
            EntityData::LeashKnot { .. } => self.knot_used(&entity, chunk, by, output),
            // A mob's uses and hits are its body's (`mob_bodies.rs`).
            EntityData::Mob { .. } => self.mob_used(&entity, chunk, by, output),
            EntityData::Item { .. }
            | EntityData::FallingBlock { .. }
            | EntityData::PrimedTnt { .. }
            | EntityData::ExperienceOrb { .. }
            | EntityData::Thrown { .. }
            | EntityData::Arrow(_)
            | EntityData::Spit(_)
            | EntityData::Firework(_)
            | EntityData::Cloud(_)
            | EntityData::Fangs(_) => {}
        }
        self.using_across = outer;
    }

    /// The tick of `entity`, of the loaded chunk `position`, as its kind ticks: an item
    /// as the game's (`items.rs`), a falling block (`falling.rs`), and each of N5's
    /// kinds in its own hook, which the step that brings it fills.
    pub(crate) fn tick_entity(
        &mut self,
        entity: Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let before = self.ticking.replace((entity.key, entity.lived));
        match &entity.data {
            EntityData::Item { .. } => self.tick_item(entity, position, output),
            EntityData::FallingBlock { .. } => {
                self.tick_falling_block(entity, position, output);
            }
            EntityData::Minecart { .. } => self.tick_minecart(entity, position, output),
            EntityData::Boat { .. } => self.tick_boat(entity, position, output),
            EntityData::ArmorStand { .. } => self.tick_armor_stand(entity, position, output),
            EntityData::ItemFrame { .. }
            | EntityData::Painting { .. }
            | EntityData::Cushion { .. }
            | EntityData::LeashKnot { .. } => self.tick_block_attached(entity, position, output),
            EntityData::PrimedTnt { .. } => self.tick_primed_tnt(entity, position, output),
            EntityData::Mob { .. } => self.tick_mob(entity, position, output),
            EntityData::ExperienceOrb { .. } => self.tick_orb(entity, position, output),
            EntityData::Thrown { .. } => self.tick_thrown(entity, position, output),
            EntityData::Arrow(_) => self.tick_arrow(entity, position, output),
            EntityData::Spit(_) => self.tick_spit(entity, position, output),
            EntityData::Firework(_) => self.tick_firework(entity, position, output),
            EntityData::Cloud(_) => self.tick_cloud(entity, position, output),
            EntityData::Fangs(_) => self.tick_fangs(entity, position, output),
        }
        self.ticking = before;
    }

    /// Forgets the entities of the chunk at `position`, which is no longer loaded.
    pub(crate) fn let_go_entities(&mut self, position: ChunkPos) {
        let memory = &mut self.entity_memory;
        self.entity_chunks.retain(|key, chunk| {
            let kept = *chunk != position;
            if !kept {
                memory.remove(key);
            }
            kept
        });
    }

    /// Applies what later commits did to the entities of copied chunks. A whole entity
    /// outside the chunk it is for is passed over: the holder's chunk would not have it.
    /// A passenger is its root's chunk's, wherever it sits.
    pub(crate) fn update_copied_entities(&mut self, updates: &[(ChunkPos, Vec<EntityChange>)]) {
        for (position, changes) in updates {
            let Some(copy) = self.copies.get_mut(position) else {
                continue;
            };
            for change in changes {
                if let EntityChange::Whole(entity) = change
                    && entity.riding.vehicle.is_none()
                    && entity.chunk() != *position
                {
                    continue;
                }
                copy.apply_entity(change);
            }
        }
    }

    /// The entities' phase of the tick (ADR-0028, section 2, as the plan of N4 amends
    /// it): every entity whose chunk ticks, in ascending key, those added before the
    /// phase among them; one added in the phase waits for the next tick, as the game's
    /// list of entities being ticked takes no new ones. A unit ticks as the game's level
    /// ticks it (`ServerLevel.tickNonPassenger`, `riding.rs`): a passenger its vehicle
    /// still holds waits for its vehicle, one it does not gets off and ticks on its own;
    /// the root ticks as its kind ticks, then its passengers, and then the unit goes
    /// where its root's position says.
    pub(crate) fn run_entities(&mut self, output: &mut TickOutput) {
        let ticking: Vec<EntityId> = self
            .entity_chunks
            .iter()
            .filter(|(_, chunk)| self.ticks_chunk(**chunk))
            .map(|(key, _)| *key)
            .collect();
        for key in ticking {
            // An item another merged into in this tick is gone.
            let Some(&position) = self.entity_chunks.get(&key) else {
                continue;
            };
            let Some(chunk) = self.chunks.get_mut(&position) else {
                continue;
            };
            let entities = chunk.entities();
            let Ok(at) = entities.binary_search_by_key(&key, |entity| entity.key) else {
                continue;
            };
            let entity = entities[at].clone();
            if let Some(vehicle) = entity.riding.vehicle {
                if self.holds_passenger(vehicle, key) {
                    continue;
                }
                self.stop_riding(key, output);
            }
            // Getting off may have moved it.
            let Some((position, entity)) = self
                .find_entity(key)
                .map(|(position, entity)| (position, entity.clone()))
            else {
                continue;
            };
            if !self.ticks_chunk(position) {
                continue;
            }
            let start = entity.position;
            self.tick_entity(entity, position, output);
            if self.entity_chunks.contains_key(&key) {
                self.tick_passengers(key, output);
                self.settle_unit(key, start, output);
            }
        }
        // What took another in by a merge has its counters committed as the tick leaves
        // them, where it is still in the chunk it merged in: a record's counters are as of
        // its tick (`docs/groundwork/n2-plan.md`, section 11).
        let mut merged = std::mem::take(&mut self.merged_into);
        merged.sort();
        merged.dedup();
        for (position, key) in merged {
            if self.entity_chunks.get(&key) != Some(&position) {
                continue;
            }
            let Some(entity) = self.chunks.get(&position).and_then(|chunk| {
                let entities = chunk.entities();
                entities
                    .binary_search_by_key(&key, |entity| entity.key)
                    .ok()
                    .map(|at| entities[at].clone())
            }) else {
                continue;
            };
            let fields = vec![
                EntityField::Lived(entity.lived),
                EntityField::Data(entity.data.clone()),
            ];
            output
                .entity_changes
                .push((position, EntityChange::Set(key, fields)));
        }
        // The riding players after the vehicles they ride, those on their way to theirs
        // first, whom a unit taken in this tick may seat.
        self.tick_waiting_riders(output);
        self.tick_riding_players(output);
        output.entities_ageing.sort();
        output.entities_ageing.dedup();
    }

    /// `count` floats from the stream of the entity `key` now, on from what it has
    /// drawn in the tick already ([`Region::entity_stream`]).
    fn draw_floats<const N: usize>(&mut self, key: EntityId) -> [f32; N] {
        let mut random = self.entity_stream(key).random;
        self.entity_drew(key, N as u64);
        std::array::from_fn(|_| random.next_float())
    }

    /// The region's next key for an entity that is to go into the loaded chunk
    /// `chunk`, taken; `None`, and nothing taken, where the chunk is not loaded or is
    /// full ([`MAX_CHUNK_ENTITIES`]), or the region has no key left (ADR-0028,
    /// section 1).
    pub(crate) fn take_entity_key(&mut self, chunk: ChunkPos) -> Option<EntityId> {
        let room = self
            .chunks
            .get(&chunk)
            .is_some_and(|loaded| loaded.entities().len() < MAX_CHUNK_ENTITIES);
        if !room {
            return None;
        }
        self.take_key()
    }

    /// The region's next key, taken; `None`, and nothing taken, where it has none left.
    /// An entity sent as it is made needs no room where it goes: an arrival is never
    /// refused.
    pub(super) fn take_key(&mut self) -> Option<EntityId> {
        let key = self.next_entity_id;
        if !self.entity_ids.contains(key) {
            return None;
        }
        self.next_entity_id = EntityId(key.0 + 1);
        self.journal.next_entity_id = true;
        Some(key)
    }

    /// `Level.addFreshEntity`: `entity` goes into its loaded chunk, whole, and the
    /// edges are told it is there. An entity added before the entities' phase ticks in
    /// that phase; one added in it waits for the next tick ([`Region::run_entities`]).
    pub(crate) fn add_fresh_entity(&mut self, entity: Entity, output: &mut TickOutput) {
        let chunk = entity.chunk();
        output
            .events
            .push(RegionEvent::EntitySpawned(state_of(&entity)));
        self.entity_chunks.insert(entity.key, chunk);
        let change = EntityChange::Whole(entity);
        if let Some(loaded) = self.chunks.get_mut(&chunk) {
            loaded.apply_entity(&change);
        }
        output.entity_changes.push((chunk, change));
    }

    /// A player throws `stack`, as the game's `LivingEntity.drop` and
    /// `createItemStackToDrop` do: an item at their x and z and a little below their
    /// eyes, with the motion of [`throw_motion`], the pick-up delay of a throw and the
    /// player as its thrower, under the region's next key; and their arm swings for
    /// everyone else. Into a chunk the region does not have loaded and ticking (one
    /// across a line the player has just crossed, or one being lent), the item is sent
    /// there as an arrival as it is made, with no `hops` and no room asked for, and
    /// appears where it is taken (`docs/groundwork/n3-plan.md`, section 5, G2). False,
    /// and nothing made, where its chunk ticks and is full ([`MAX_CHUNK_ENTITIES`]), or
    /// the region has no key left (ADR-0028, section 1).
    pub(crate) fn throw(&mut self, thrown: Thrown, output: &mut TickOutput) -> bool {
        let Thrown {
            player,
            stack,
            crouching,
        } = thrown;
        let Some(thrower) = self.players.get(&player) else {
            return false;
        };
        // Only stacks of items that exist are thrown.
        let known =
            stack.count > 0 && usize::try_from(stack.item).is_ok_and(|item| item < ITEMS.len());
        if !known {
            return false;
        }
        let pose = thrower.pose;
        let thrower_key = thrower.entity_id;
        // A player who flies does not crouch, whatever the key says.
        let eyes = if crouching && !thrower.flying {
            CROUCHING_EYES
        } else {
            STANDING_EYES
        };
        let position = [
            pose.position.x,
            pose.position.y + f64::from(eyes) - BELOW_THE_EYES,
            pose.position.z,
        ];
        let chunk = ChunkPos::containing(position[0], position[2]);
        let here = self.ticks_chunk(chunk);
        let key = if here {
            self.take_entity_key(chunk)
        } else {
            self.take_key()
        };
        let Some(key) = key else {
            return false;
        };

        let motion = throw_motion(pose.yaw, pose.pitch, self.draw_floats(thrower_key));
        // The item's own draws at its birth: its UUID (the game's two longs), the bob
        // of its model, which only clients show, and its yaw (`ItemEntity`'s
        // constructors).
        let mut own = self.entity_stream(key).random;
        let uuid = insecure_uuid(own.next_u64(), own.next_u64());
        let _bob = own.next_float();
        let yaw = own.next_float() * 360.0;
        let entity = Entity {
            key,
            kind: u16::try_from(entity_types::ITEM).expect("entity type ids fit 16 bits"),
            uuid,
            position,
            motion,
            yaw,
            pitch: 0.0,
            on_ground: false,
            hops: 0,
            lived: 0,
            fresh_at: 0,
            data: EntityData::Item {
                stack,
                pick_up_delay: THROWN_PICK_UP_DELAY,
                thrower: Some(player.0),
                health: 5,
                age_offset: 0,
            },
            fall_distance: 0.0,
            // The game's field starts at 0.
            fire_ticks: 0,
            riding: Default::default(),
        };
        // `LivingEntity.drop` swings the main hand with `SwingAnimation.DEFAULT`.
        self.swing_arm(player, false, clustine_data::Swing::DEFAULT, false, output);
        if here {
            self.entity_drew(key, 4);
            self.add_fresh_entity(entity, output);
        } else {
            self.send_made(entity, output);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The game's sine table at an angle, worked out here as the game fills the table
    /// (`Mth.SIN`, the sine of each 65,536th of a turn) and looks it up, without the
    /// table of the noise crate.
    fn table_sin(radians: f32) -> f32 {
        let index = (f64::from(radians) * 10_430.378_350_470_453) as i64 & 0xFFFF;
        libm::sin(index as f64 / 10_430.378_350_470_453) as f32
    }

    fn table_cos(radians: f32) -> f32 {
        let index = (f64::from(radians) * 10_430.378_350_470_453 + 16_384.0) as i64 & 0xFFFF;
        libm::sin(index as f64 / 10_430.378_350_470_453) as f32
    }

    #[test]
    fn an_item_thrown_looking_ahead_flies_ahead_at_three_tenths_scattered_by_its_draws() {
        // Looking along positive z, level: the sine of 0 is 0 and its cosine 1, so the
        // throw is 0.3 along z and 0.1 up, and the draws put it off that.
        let draws = [0.25, 0.5, 0.75, 0.25];
        let motion = throw_motion(0.0, 0.0, draws);
        let direction = f64::from(0.25_f32 * 6.283_185_5_f32);
        let spread = f64::from(0.02_f32 * 0.5_f32);
        assert_eq!(
            motion[0].to_bits(),
            (0.0 + libm::cos(direction) * spread).to_bits()
        );
        let lift = 0.0_f32 * 0.3 + 0.1 + (0.75_f32 - 0.25) * 0.1;
        assert_eq!(motion[1].to_bits(), f64::from(lift).to_bits());
        assert_eq!(
            motion[2].to_bits(),
            (f64::from(0.3_f32) + libm::sin(direction) * spread).to_bits()
        );
        // The first draw's quarter turn scatters it along z too: 0.31 ahead.
        assert!((motion[0] - 0.0).abs() < 1e-6, "{motion:?}");
        assert!((motion[2] - 0.31).abs() < 1e-6, "{motion:?}");
    }

    #[test]
    fn an_item_thrown_to_the_side_and_upwards_follows_the_game_s_sine_table() {
        // Facing east (yaw -90, towards positive x) and looking up at 45 degrees: each
        // of the game's steps by hand.
        let (yaw, pitch) = (-90.0_f32, -45.0_f32);
        let draws = [0.0, 1.0, 0.0, 0.0];
        let sin_pitch = table_sin(pitch * 0.017_453_292);
        let cos_pitch = table_cos(pitch * 0.017_453_292);
        let sin_yaw = table_sin(yaw * 0.017_453_292);
        let cos_yaw = table_cos(yaw * 0.017_453_292);
        // The first draw 0 points the scatter along x, by the second's 0.02.
        let expected = [
            f64::from(-sin_yaw * cos_pitch * 0.3_f32) + 1.0 * f64::from(0.02_f32),
            f64::from(-sin_pitch * 0.3_f32 + 0.1_f32 + 0.0 * 0.1_f32),
            f64::from(cos_yaw * cos_pitch * 0.3_f32) + 0.0,
        ];
        let motion = throw_motion(yaw, pitch, draws);
        assert_eq!(motion.map(f64::to_bits), expected.map(f64::to_bits));
        // East and up: 0.21 along x and the scatter's 0.02, 0.31 up, nothing along z.
        assert!((motion[0] - 0.232).abs() < 1e-3, "{motion:?}");
        assert!((motion[1] - 0.312).abs() < 1e-3, "{motion:?}");
        assert!(motion[2].abs() < 1e-6, "{motion:?}");
    }

    #[test]
    fn an_insecure_uuid_is_of_version_4_and_the_ietf_variant() {
        let uuid = insecure_uuid(u64::MAX, u64::MAX);
        assert_eq!(uuid.get_version_num(), 4);
        assert_eq!(uuid.get_variant(), uuid::Variant::RFC4122);
        assert_eq!(
            insecure_uuid(0, 0),
            Uuid::from_u64_pair(0x4000, 0x8000_0000_0000_0000)
        );
    }
}
