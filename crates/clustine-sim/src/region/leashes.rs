//! Leashes (`Leashable`, `Leashable$LeashData`, `Leashable$Wrench`, `LeadItem` and the
//! leash's part of `Entity.interact`; `docs/groundwork/n5-brief.md`, sections 0.8, 0.17,
//! 0.36 and 7; step N5g of `docs/groundwork/n5-plan.md`, with its section 7 item 3).
//! Boats are the only kind of N5 a leash holds; anything holds one: a player, a fence's
//! knot, another boat.
//!
//! A leash's holder is kept by key, or, for a knot, by its fence's position, as the
//! game saves it, so that a knot found or made again at the fence holds what was tied
//! there. The game keeps the holder as a live reference once it has found it and looks
//! it up again only after a load (`restoreLeashFromSave`). Here the region looks it up
//! every tick, among its own entities and players and the entities of its copies, and
//! remembers whether it has found it since the leashee was taken in
//! ([`LeashMemory`]): before, the leash is the game's delayed leash, which holds nothing
//! and is looked for for a hundred ticks; after, a holder not found is gone, and the
//! leash drops with a lead.
//!
//! The hundred ticks count the leashee's own ticks from its take-in, as the game's
//! `tickCount` counts them from its load and does not save them: they are kept in the
//! region's memory of the leashee, which starts again at every take-in (a load, a
//! restore, a merge's or a split's), as the game's count starts again at a load. An
//! arrival is no load: a boat sent across a line keeps its leash found, and is judged at
//! once where it comes.
//!
//! Named differences and what could not be settled:
//! - Players are not in copies (the plan, section 7 item 3): a holder that is another
//!   region's player, or an entity the region neither has nor sees in a copy, is out of
//!   reach, and the leash drops with a lead as one whose holder has gone does (without
//!   the sound of a snap). Where regions follow players, a held leash near a line counts
//!   as entities at work there ([`Region::leashes_at_line`]), so that the regions merge
//!   before it comes to that.
//! - A player's holding is by the key of their stay: one who comes back in a new stay
//!   does not find a boat whose chunk was saved with their leash, which drops after the
//!   hundred ticks (the game finds them by their UUID).
//! - What a binding, a cut or a knot's use moves are the region's own boats: a boat in
//!   another region's chunk held by a knot here, or by a boat here, is passed over, and
//!   a knot in another region's chunk is left there when its last leashee here lets go.
//!   A use by another region's player of a boat or a knot here ties nothing to that
//!   player, who holds nothing here; a lead used so does nothing, and shears cut.
//! - A leash restored by a fence in a chunk the region does not hold finds a knot there
//!   in the copy, and makes none: after the hundred ticks it drops.
//! - The game's rule `entityDrops` is taken as true, its default: Clustine keeps no
//!   rules.

use std::collections::BTreeSet;

use clustine_blocks::data::FENCES;
use clustine_data::{EntityTypeInfo, entity_types, items, sounds};
use clustine_world::{
    Attached, BlockPos, ChunkPos, Entity, EntityData, EntityField, EntityId, EntityUse, ItemStack,
    Leash, LeashHolder, PlayerId, RegionId,
};

use super::Region;
use super::entities::insecure_uuid;
use super::riding::y_rot;
use crate::api::{RegionEvent, TickOutput};
use crate::physics::aabb::Aabb;
use crate::physics::entities::is_boat;
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::physics::{self, Vec3};

/// `leashSnapDistance()`: beyond this between the boxes' centres a leash breaks.
pub(crate) const SNAP_DISTANCE: f64 = 12.0;

/// `leashElasticDistance()`: beyond this between the points of attachment it pulls.
const ELASTIC_DISTANCE: f64 = 6.0;

/// `ENTITY_ATTACHMENT_POINT` and `LEASHER_ATTACHMENT_POINT`, in parts of the width,
/// height and width: where a single leash is tied on the leashee and on its holder.
const ENTITY_ATTACHMENT_POINT: Vec3 = [0.0, 0.5, 0.5];
const LEASHER_ATTACHMENT_POINT: Vec3 = [0.0, 0.5, 0.0];

/// `AXIS_SPECIFIC_ELASTICITY`.
const AXIS_SPECIFIC_ELASTICITY: Vec3 = [0.8, 0.2, 0.8];

/// `TORSIONAL_ELASTICITY`.
const TORSIONAL_ELASTICITY: f64 = 10.0;

/// `STIFFNESS`: how much of the difference of the two ends' movements the pull adds.
const STIFFNESS: f64 = 0.11;

/// `0.3f` widened: a spring's force where the leashee already moves along it.
const ALONG: f64 = 0.300_000_011_920_928_96;

/// `0.017453292f`.
const DEGREES_TO_RADIANS: f32 = 0.017_453_292;

/// Half the side of `leashableInArea`'s box (`AABB.ofSize(centre, 32, 32, 32)`).
const SEARCH: f64 = 16.0;

/// The ticks a saved leash is looked for after its leashee's take-in.
const RESTORE_FOR: u32 = 100;

/// `SoundSource.PLAYERS`, whose sounds shears' snip is.
const SOUND_SOURCE_PLAYERS: i32 = 7;

/// A knot's place above its fence block's corner (`LeashFenceKnotEntity`'s `OFFSET_Y`
/// and the block's middle).
const KNOT_OFFSET: Vec3 = [0.5, 0.375, 0.5];

/// What the region keeps of a leashee beyond its record: whether its holder has been
/// found since its take-in, and the ticks it has ticked since then while not (the
/// game's `tickCount`, which a restore reads).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct LeashMemory {
    pub(crate) found: bool,
    pub(crate) ticks: u32,
}

/// One end of a leash, as the pull reads it: where the entity stands, which way it
/// faces, its size, and its known movement (`getKnownMovement`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct End {
    pub(crate) position: Vec3,
    pub(crate) yaw: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) movement: Vec3,
}

impl End {
    /// The centre of its box (`getBoundingBox().getCenter()`).
    pub(crate) fn centre(&self) -> Vec3 {
        Aabb::of_entity(self.position, self.width, self.height).center()
    }
}

/// `Vec3.distanceTo`: the square root of the squares summed in order.
fn distance(from: Vec3, to: Vec3) -> f64 {
    let (dx, dy, dz) = (to[0] - from[0], to[1] - from[1], to[2] - from[2]);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// `Vec3.multiply(Vec3)`.
fn multiply(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] * b[0], a[1] * b[1], a[2] * b[2]]
}

/// `Vec3.dot`.
fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `leashDistanceTo(holder)`: from the holder's box's centre to the leashee's.
pub(crate) fn leash_distance(leashee: &End, holder: &End) -> f64 {
    distance(holder.centre(), leashee.centre())
}

/// `computeDampenedSpringInteraction(ph, pe, elastic, movement, offset)`: no wrench
/// where the two points are closer than `elastic`; else a force along the leash by how
/// far beyond it they are, its torque about the leashee's position, and the force cut to
/// three tenths where the leashee already moves along it.
fn spring(
    holder_point: Vec3,
    leashee_point: Vec3,
    movement: Vec3,
    offset: Vec3,
) -> Option<(Vec3, f64)> {
    let length = distance(leashee_point, holder_point);
    if length < ELASTIC_DISTANCE {
        return None;
    }
    let mut force = physics::scale(
        physics::normalize(physics::sub(holder_point, leashee_point)),
        length - ELASTIC_DISTANCE,
    );
    // `Wrench.torqueFromForce(offset, force)`.
    let torque = offset[2] * force[0] - offset[0] * force[2];
    if dot(movement, force) >= 0.0 {
        force = physics::scale(force, ALONG);
    }
    Some((force, torque))
}

/// `checkElasticInteractions(holder, data)` with the single points every N5 holder
/// uses: where the spring pulls, the wrench's torque turns the angular momentum by ten
/// times itself, and the leashee's motion gains the force by axis and a part of the
/// difference of the two movements. Whether it pulled.
pub(crate) fn elastic_pull(
    leashee: &End,
    holder: &End,
    motion: &mut Vec3,
    momentum: &mut f64,
) -> bool {
    // `computeElasticInteraction`: the leashee's own movement is the one its springs
    // read.
    let leashee_angle = leashee.yaw * DEGREES_TO_RADIANS;
    let leashee_size = [
        f64::from(leashee.width),
        f64::from(leashee.height),
        f64::from(leashee.width),
    ];
    let holder_angle = holder.yaw * DEGREES_TO_RADIANS;
    let holder_size = [
        f64::from(holder.width),
        f64::from(holder.height),
        f64::from(holder.width),
    ];
    let offset = y_rot(
        multiply(ENTITY_ATTACHMENT_POINT, leashee_size),
        -leashee_angle,
    );
    let leashee_point = physics::add(leashee.position, offset);
    let holder_offset = y_rot(
        multiply(LEASHER_ATTACHMENT_POINT, holder_size),
        -holder_angle,
    );
    let holder_point = physics::add(holder.position, holder_offset);
    let Some((force, torque)) = spring(holder_point, leashee_point, leashee.movement, offset)
    else {
        return false;
    };
    // `Wrench.accumulate` of the one wrench, summed from nothing, and `scale(1.0)`.
    let force = physics::scale([0.0 + force[0], 0.0 + force[1], 0.0 + force[2]], 1.0);
    let torque = (0.0 + torque) * 1.0;
    *momentum += TORSIONAL_ELASTICITY * torque;
    let relative = physics::sub(holder.movement, leashee.movement);
    let pull = physics::add(
        multiply(force, AXIS_SPECIFIC_ELASTICITY),
        physics::scale(relative, STIFFNESS),
    );
    *motion = physics::add(*motion, pull);
    true
}

/// What a leash's tick does to its leashee (`tickLeash` after its holder is found):
/// beyond the snap distance it breaks (`true`, for the caller to drop it); beyond the
/// elastic distance less both widths it pulls, and a pull stops a fall from counting
/// beyond a block (`onElasticLeashPull`); and then, broken or not, the leashee turns by
/// the angular momentum, which `friction` then slows.
pub(crate) fn leash_tick(
    leashee: &End,
    holder: &End,
    motion: &mut Vec3,
    fall_distance: &mut f64,
    yaw: &mut f32,
    momentum: &mut f64,
    friction: f32,
) -> bool {
    let length = leash_distance(leashee, holder);
    let snapped = length > SNAP_DISTANCE;
    if !snapped
        && length > ELASTIC_DISTANCE - f64::from(holder.width) - f64::from(leashee.width)
        && elastic_pull(leashee, holder, motion, momentum)
    {
        // `checkFallDistanceAccumulation`.
        if motion[1] > -0.5 && *fall_distance > 1.0 {
            *fall_distance = 1.0;
        }
    }
    let turned = (f64::from(*yaw) - *momentum) as f32;
    if turned.is_finite() {
        *yaw = turned;
    }
    *momentum *= f64::from(friction);
    snapped
}

/// Who holds a leash, found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Held {
    /// The holder's key: a player's stay, a knot, a boat.
    pub(crate) key: EntityId,
    pub(crate) end: End,
    /// The region's player who holds it, if one does.
    pub(crate) player: Option<PlayerId>,
}

/// Whether `record` is a knot.
fn is_knot(record: &Entity) -> bool {
    matches!(record.data, EntityData::LeashKnot { .. })
}

/// The leash of `record`, if any.
fn leash_of(record: &Entity) -> Option<Leash> {
    record.riding.leash.as_deref().copied()
}

/// The box `leashableInArea` looks in about `centre`.
fn search_box(centre: Vec3) -> Aabb {
    Aabb::new(
        [centre[0] - SEARCH, centre[1] - SEARCH, centre[2] - SEARCH],
        [centre[0] + SEARCH, centre[1] + SEARCH, centre[2] + SEARCH],
    )
}

impl Region {
    /// Whether the leash of the region's entity `key` has found its holder since its
    /// take-in (`LeashData.leashHolder` is set).
    pub(crate) fn leash_found(&self, key: EntityId) -> bool {
        self.entity_memory
            .get(&key)
            .is_some_and(|memory| memory.boat.leash.found)
    }

    /// Notes that the leash of `key` has found its holder.
    pub(crate) fn note_leash_found(&mut self, key: EntityId) {
        self.entity_memory.entry(key).or_default().boat.leash.found = true;
    }

    /// The known movement of the player `id` (`ServerPlayer.getKnownMovement`): that of
    /// a vehicle they ride and do not drive, else what their last move this tick moved
    /// them by, none if they made none.
    pub(crate) fn player_movement(&self, id: PlayerId) -> Vec3 {
        let Some(player) = self.players.get(&id) else {
            return [0.0; 3];
        };
        if let Some(riding) = player.riding
            && let Some((_, vehicle)) = self.find_entity(riding.vehicle)
            && self.driver_of(vehicle) != Some(id)
        {
            return self.record_movement(vehicle);
        }
        self.known_movements.get(&id).copied().unwrap_or([0.0; 3])
    }

    /// The region's player who drives `vehicle` (`getControllingPassenger`, a player):
    /// a boat's first seat, taken.
    fn driver_of(&self, vehicle: &Entity) -> Option<PlayerId> {
        if !is_boat(vehicle.kind) {
            return None;
        }
        let seat = vehicle.riding.passengers.first()?;
        if !seat.player || seat.pending.is_some() {
            return None;
        }
        self.player_with_entity(seat.key)
    }

    /// `Entity.getKnownMovement()` of `record`: its driver's where a player drives it,
    /// else its motion.
    pub(crate) fn record_movement(&self, record: &Entity) -> Vec3 {
        match self.driver_of(record) {
            Some(driver) => self
                .known_movements
                .get(&driver)
                .copied()
                .unwrap_or([0.0; 3]),
            None => record.motion,
        }
    }

    /// `record` as an end of a leash.
    pub(crate) fn record_end(&self, record: &Entity) -> End {
        let info = EntityTypeInfo::of(i32::from(record.kind));
        End {
            position: record.position,
            yaw: record.yaw,
            width: info.width,
            height: info.height,
            movement: self.record_movement(record),
        }
    }

    /// The region's player `id` as an end of a leash: 0.6 wide, 1.8 high, 1.5 while they
    /// crouch (sneaking and not flying, `Player.updatePlayerPose`).
    fn player_end(&self, id: PlayerId) -> Option<End> {
        let player = self.players.get(&id)?;
        let at = player.pose.position;
        let crouching = player.keys.sneak && !player.flying && player.riding.is_none();
        Some(End {
            position: [at.x, at.y, at.z],
            yaw: player.pose.yaw,
            width: 0.6,
            height: if crouching { 1.5 } else { 1.8 },
            movement: self.player_movement(id),
        })
    }

    /// The entity `key` wherever the region can see it: its own, its player, or a
    /// copy's.
    pub(crate) fn entity_end(&self, key: EntityId) -> Option<Held> {
        if let Some((_, record)) = self.find_entity(key) {
            return Some(Held {
                key,
                end: self.record_end(record),
                player: None,
            });
        }
        if let Some(id) = self.player_with_entity(key) {
            return Some(Held {
                key,
                end: self.player_end(id)?,
                player: Some(id),
            });
        }
        let (_, record) = self.copied_entity(key)?;
        Some(Held {
            key,
            end: self.record_end(&record),
            player: None,
        })
    }

    /// The knot at the fence `fence` (`LeashFenceKnotEntity.getKnot`), in the region's
    /// chunk there or its copy: the first whose block it is, by key.
    pub(crate) fn knot_at(&self, fence: BlockPos) -> Option<Entity> {
        let chunk = self.readable_chunk(fence.chunk())?;
        chunk
            .entities()
            .iter()
            .find(|entity| matches!(entity.data, EntityData::LeashKnot { attached } if attached.block == fence))
            .cloned()
    }

    /// The key of whatever holds a leash held by `holder` now, if the region sees it:
    /// for a fence, its knot.
    pub(crate) fn holder_key(&self, holder: LeashHolder) -> Option<EntityId> {
        match holder {
            LeashHolder::Entity(key) => Some(key),
            LeashHolder::Fence(fence) => self.knot_at(fence).map(|knot| knot.key),
        }
    }

    /// The holder `holder`, found where the region sees it (`canInteractWithLevel`):
    /// `None` where it is gone or out of reach.
    pub(crate) fn leash_holder(&self, holder: LeashHolder) -> Option<Held> {
        match holder {
            LeashHolder::Entity(key) => self.entity_end(key),
            LeashHolder::Fence(fence) => {
                let knot = self.knot_at(fence)?;
                Some(Held {
                    key: knot.key,
                    end: self.record_end(&knot),
                    player: None,
                })
            }
        }
    }

    /// `canHaveALeashAttachedTo(holder)` of the region's entity `key`: not itself, and
    /// no further than the snap distance from it.
    fn can_leash_to(&self, key: EntityId, holder: EntityId) -> bool {
        if key == holder {
            return false;
        }
        let (Some(leashee), Some(held)) = (self.entity_end(key), self.entity_end(holder)) else {
            return false;
        };
        leash_distance(&leashee.end, &held.end) <= SNAP_DISTANCE
    }

    /// `leashableInArea(centre, l -> l.getLeashHolder() == holder)` among the region's
    /// own entities: those in the box of 32 about `centre` whose leash has found the
    /// holder `holder`, in the order of the game's lookups.
    pub(crate) fn leashed_to(&self, holder: EntityId, centre: Vec3) -> Vec<EntityId> {
        self.entities_in(&search_box(centre))
            .into_iter()
            .filter(|(key, chunk)| {
                self.entity_record(*key, *chunk)
                    .and_then(leash_of)
                    .is_some_and(|leash| {
                        self.leash_found(*key) && self.holder_key(leash.holder) == Some(holder)
                    })
            })
            .map(|(key, _)| key)
            .collect()
    }

    /// Whether anything still holds by the knot `knot` (`leashableLeashedTo(this)` is
    /// not empty): a leashee of the region's that has found it, or one of a copy's.
    fn knot_holds(&self, knot: &Entity) -> bool {
        let EntityData::LeashKnot { attached } = knot.data else {
            return false;
        };
        let centre = self.entity_box(knot).center();
        if !self.leashed_to(knot.key, centre).is_empty() {
            return true;
        }
        let bounds = search_box(centre);
        let low = ChunkPos::containing(bounds.min[0], bounds.min[2]);
        let high = ChunkPos::containing(bounds.max[0], bounds.max[2]);
        (low.x..=high.x).any(|x| {
            (low.z..=high.z).any(|z| {
                self.copies.get(&ChunkPos::new(x, z)).is_some_and(|copy| {
                    copy.entities().iter().any(|entity| {
                        leash_of(entity).is_some_and(|leash| {
                            leash.holder == LeashHolder::Fence(attached.block)
                                || leash.holder == LeashHolder::Entity(knot.key)
                        }) && self.entity_box(entity).intersects(bounds)
                    })
                })
            })
        })
    }

    /// `notifyLeasheeRemoved` of the holder `holder`: a knot of the region's that holds
    /// nothing any more discards itself, silently.
    fn leashee_removed(&mut self, holder: EntityId, output: &mut TickOutput) {
        let Some((chunk, record)) = self.find_entity(holder) else {
            return;
        };
        if !is_knot(record) {
            return;
        }
        let record = record.clone();
        if !self.knot_holds(&record) {
            self.remove_entity(record.key, record.hops, chunk, output);
        }
    }

    /// `setLeashedTo(holder, true)` of the region's entity `key`, the holder found as
    /// `holder_key`: the leash holds by `holder` now, keeping its swing where it had
    /// one; a knot let go of that holds nothing more goes; clients are told; and the
    /// leashee gets off what it rides (a leashed entity cannot ride).
    pub(crate) fn set_leashed_to(
        &mut self,
        key: EntityId,
        holder: LeashHolder,
        holder_key: EntityId,
        output: &mut TickOutput,
    ) {
        if holder_key == key {
            return;
        }
        let Some((chunk, record)) = self.find_entity(key) else {
            return;
        };
        let old = leash_of(record);
        let old_holder = old
            .filter(|_| self.leash_found(key))
            .and_then(|leash| self.holder_key(leash.holder));
        let mut riding = record.riding.clone();
        let rides = riding.vehicle.is_some();
        riding.leash = Some(Box::new(Leash {
            holder,
            angular_momentum: old.map(|leash| leash.angular_momentum).unwrap_or_default(),
        }));
        self.commit_entity(chunk, key, vec![EntityField::Riding(riding)], output);
        self.note_leash_found(key);
        if let Some(old_holder) = old_holder
            && old_holder != holder_key
        {
            self.leashee_removed(old_holder, output);
        }
        output.events.push(RegionEvent::Leashed {
            entity: key,
            chunk,
            holder: Some(holder_key),
        });
        if rides {
            self.stop_riding(key, output);
        }
    }

    /// `dropLeash` (with a lead, `item`) or `removeLeash` (without) of the region's
    /// entity `key`, where its leash holds: the leash goes, the lead is dropped at the
    /// leashee's feet, clients are told, and the holder is told it lost a leashee.
    /// Whether there was a leash to drop.
    pub(crate) fn drop_leash(
        &mut self,
        key: EntityId,
        item: bool,
        output: &mut TickOutput,
    ) -> bool {
        let Some((chunk, record)) = self.find_entity(key) else {
            return false;
        };
        let Some(leash) = leash_of(record) else {
            return false;
        };
        if !self.leash_found(key) {
            return false;
        }
        let old_holder = self.holder_key(leash.holder);
        let position = record.position;
        let mut riding = record.riding.clone();
        riding.leash = None;
        self.commit_entity(chunk, key, vec![EntityField::Riding(riding)], output);
        if item {
            self.drop_lead(position, output);
        }
        let chunk = self.find_entity(key).map_or(chunk, |(chunk, _)| chunk);
        output.events.push(RegionEvent::Leashed {
            entity: key,
            chunk,
            holder: None,
        });
        if let Some(old_holder) = old_holder {
            self.leashee_removed(old_holder, output);
        }
        true
    }

    /// `spawnAtLocation(level, Items.LEAD)` at `position`.
    fn drop_lead(&mut self, position: Vec3, output: &mut TickOutput) {
        self.spawn_item(
            position,
            ItemStack {
                item: items::LEAD,
                count: 1,
            },
            output,
        );
    }

    /// `restoreLeashFromSave` of the region's entity `key`, whose leash holds by
    /// `holder` and has not found it since the take-in, in its `ticks`-th tick since:
    /// by key among what the region sees, or by fence, through the knot there or one
    /// made there where the region holds the fence's chunk. Whether it found the
    /// holder. One not found by the 101st tick is let go of with a lead dropped, and no
    /// client told: none was told of the leash either.
    pub(crate) fn restore_leash(
        &mut self,
        key: EntityId,
        holder: LeashHolder,
        ticks: u32,
        output: &mut TickOutput,
    ) -> bool {
        let found = match holder {
            LeashHolder::Entity(holder_key) => self.entity_end(holder_key).map(|held| held.key),
            LeashHolder::Fence(fence) => match self.knot_at(fence) {
                Some(knot) => Some(knot.key),
                None => self.make_knot(fence, output),
            },
        };
        if let Some(holder_key) = found {
            self.set_leashed_to(key, holder, holder_key, output);
            return true;
        }
        if ticks > RESTORE_FOR
            && let Some((chunk, record)) = self.find_entity(key)
        {
            let position = record.position;
            let mut riding = record.riding.clone();
            riding.leash = None;
            self.commit_entity(chunk, key, vec![EntityField::Riding(riding)], output);
            self.drop_lead(position, output);
        }
        false
    }

    /// `LeashFenceKnotEntity.createKnot(level, fence)`: a knot at the fence, in the
    /// region's loaded chunk there, its UUID from its own random; `None` where the chunk
    /// is not the region's loaded one, is full, or no key is left.
    pub(crate) fn make_knot(
        &mut self,
        fence: BlockPos,
        output: &mut TickOutput,
    ) -> Option<EntityId> {
        let chunk = fence.chunk();
        let key = self.take_entity_key(chunk)?;
        let mut own = self.entity_stream(key).random;
        let uuid = insecure_uuid(own.next_u64(), own.next_u64());
        self.entity_drew(key, 2);
        let corner = [f64::from(fence.x), f64::from(fence.y), f64::from(fence.z)];
        let knot = Entity {
            key,
            kind: u16::try_from(entity_types::LEASH_KNOT).expect("entity type ids fit 16 bits"),
            uuid,
            position: physics::add(corner, KNOT_OFFSET),
            motion: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            hops: 0,
            lived: 0,
            fresh_at: 0,
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
            data: EntityData::LeashKnot {
                attached: Attached {
                    block: fence,
                    direction: 0,
                },
            },
        };
        self.add_fresh_entity(knot, output);
        Some(key)
    }

    /// `Entity.playSound(sound, 1, 1)` of an entity at `position`, neutral, heard by all.
    pub(crate) fn leash_sound(&self, sound: i32, position: Vec3, output: &mut TickOutput) {
        output.events.push(RegionEvent::Sound {
            sound,
            source: SOUND_SOURCE_NEUTRAL,
            position: clustine_world::Vec3::new(position[0], position[1], position[2]),
            volume: 1.0,
            pitch: 1.0,
            except: None,
        });
    }

    /// `LeadItem.bindPlayerMobs(player, level, fence)`: the boats the player `player`
    /// holds within the box of 32 about the fence's middle are tied to the knot there,
    /// found or made, each within the snap distance of it; the knot plays its sound.
    /// Where none could be tied, a knot made for it goes again, sent and gone. Whether
    /// any was tied.
    pub(crate) fn bind_player_mobs(
        &mut self,
        player: PlayerId,
        fence: BlockPos,
        output: &mut TickOutput,
    ) -> bool {
        let Some(holder) = self.players.get(&player).map(|player| player.entity_id) else {
            return false;
        };
        let centre = [
            f64::from(fence.x) + 0.5,
            f64::from(fence.y) + 0.5,
            f64::from(fence.z) + 0.5,
        ];
        let held = self.leashed_to(holder, centre);
        if held.is_empty() {
            return false;
        }
        let found = self.knot_at(fence).map(|knot| knot.key);
        let Some(knot) = found.or_else(|| self.make_knot(fence, output)) else {
            return false;
        };
        let mut any = false;
        for key in held {
            if self.can_leash_to(key, knot) {
                self.set_leashed_to(key, LeashHolder::Fence(fence), knot, output);
                any = true;
            }
        }
        let Some((chunk, record)) = self
            .find_entity(knot)
            .map(|(chunk, record)| (chunk, record.clone()))
        else {
            return any;
        };
        if any {
            // `playPlacementSound`.
            self.leash_sound(sounds::ITEM_LEAD_TIED, record.position, output);
            return true;
        }
        if found.is_none() {
            self.remove_entity(knot, record.hops, chunk, output);
        }
        false
    }

    /// `dropAllLeashConnections(player)` of the region's entity `key`, by shears: its
    /// own leash and every leash it holds among the region's boats drop, each with a
    /// lead, and the shears snip where anything was cut. Whether anything was.
    pub(crate) fn cut_leashes(&mut self, key: EntityId, output: &mut TickOutput) -> bool {
        let Some((_, record)) = self.find_entity(key) else {
            return false;
        };
        let record = record.clone();
        let held = self.leashed_to(key, self.entity_box(&record).center());
        let mut cut = false;
        if leash_of(&record).is_some() && self.leash_found(key) {
            cut |= self.drop_leash(key, true, output);
        }
        for leashee in held {
            cut |= self.drop_leash(leashee, true, output);
        }
        if cut {
            let block = physics::containing(record.position);
            output.events.push(RegionEvent::Sound {
                sound: sounds::ITEM_SHEARS_SNIP,
                source: SOUND_SOURCE_PLAYERS,
                position: clustine_world::Vec3::new(
                    f64::from(block.x) + 0.5,
                    f64::from(block.y) + 0.5,
                    f64::from(block.z) + 0.5,
                ),
                volume: 1.0,
                pitch: 1.0,
                except: None,
            });
        }
        cut
    }

    /// The leash's part of `Entity.interact` of the boat `entity` used by `by`, which
    /// comes before the boat's own use: whether it took the use. In order: a sneaking
    /// player's leashed boats are tied to this one; shears cut every leash it is in; its
    /// holder lets it go (a creative player keeps the lead, as every player is); a lead
    /// in hand ties it to the player, the leash it had dropping a lead, within the snap
    /// distance, the lead kept (`Player.interactOn` gives a creative player's back).
    pub(crate) fn leash_used(
        &mut self,
        entity: &Entity,
        by: EntityUse,
        output: &mut TickOutput,
    ) -> bool {
        let EntityUse::Interact {
            player,
            held,
            sneaking,
            ..
        } = by
        else {
            return false;
        };
        let key = entity.key;
        let Some((_, record)) = self.find_entity(key) else {
            return false;
        };
        let record = record.clone();
        let ours = self.player_with_entity(player);
        if sneaking && ours.is_some() {
            let tied: Vec<EntityId> = self
                .leashed_to(player, self.entity_box(&record).center())
                .into_iter()
                .filter(|leashee| self.can_leash_to(*leashee, key))
                .collect();
            for leashee in &tied {
                self.set_leashed_to(*leashee, LeashHolder::Entity(key), key, output);
            }
            if !tied.is_empty() {
                self.leash_sound(sounds::ITEM_LEAD_TIED, record.position, output);
                return true;
            }
        }
        let in_hand = held.map(|stack| stack.item);
        if in_hand == Some(items::SHEARS) && self.cut_leashes(key, output) {
            return true;
        }
        let leashed = leash_of(&record).is_some() && self.leash_found(key);
        let holder = leash_of(&record)
            .filter(|_| leashed)
            .and_then(|leash| self.holder_key(leash.holder));
        if holder == Some(player) {
            self.drop_leash(key, false, output);
            self.leash_sound(sounds::ITEM_LEAD_UNTIED, record.position, output);
            return true;
        }
        let held_by_player = holder.is_some_and(|holder| self.player_with_entity(holder).is_some());
        if in_hand != Some(items::LEAD) || held_by_player {
            return false;
        }
        // Another region's player holds nothing here: the lead does nothing.
        if ours.is_none() {
            return true;
        }
        if !self.can_leash_to(key, player) {
            return false;
        }
        if leashed {
            self.drop_leash(key, true, output);
        }
        self.set_leashed_to(key, LeashHolder::Entity(player), player, output);
        self.leash_sound(sounds::ITEM_LEAD_TIED, record.position, output);
        true
    }

    /// `LeashFenceKnotEntity.interact` of the knot `knot` used by `by`: shears cut what
    /// it holds; otherwise the boats the player holds move to it, or, where none does
    /// and the player does not sneak, the boats it holds move to the player; the knot
    /// plays its sound where any moved.
    pub(crate) fn knot_interact(&mut self, knot: &Entity, by: EntityUse, output: &mut TickOutput) {
        let EntityUse::Interact {
            player,
            held,
            sneaking,
            ..
        } = by
        else {
            return;
        };
        let EntityData::LeashKnot { attached } = knot.data else {
            return;
        };
        if held.is_some_and(|stack| stack.item == items::SHEARS)
            && self.cut_leashes(knot.key, output)
        {
            return;
        }
        let Some(id) = self.player_with_entity(player) else {
            return;
        };
        let Some(standing) = self.player_end(id) else {
            return;
        };
        let mut moved = false;
        for leashee in self.leashed_to(player, standing.centre()) {
            if self.can_leash_to(leashee, knot.key) {
                self.set_leashed_to(
                    leashee,
                    LeashHolder::Fence(attached.block),
                    knot.key,
                    output,
                );
                moved = true;
            }
        }
        let mut moved_back = false;
        if !moved && !sneaking {
            let centre = self.entity_box(knot).center();
            for leashee in self.leashed_to(knot.key, centre) {
                if self.can_leash_to(leashee, player) {
                    self.set_leashed_to(leashee, LeashHolder::Entity(player), player, output);
                    moved_back = true;
                }
            }
        }
        if moved || moved_back {
            self.leash_sound(sounds::ITEM_LEAD_TIED, knot.position, output);
        }
    }

    /// Whether `state` is a fence a leash ties to (`#minecraft:fences`).
    pub(crate) fn is_fence(state: clustine_data::BlockState) -> bool {
        FENCES.contains(state)
    }

    /// Where held leashes are near a line (the plan, section 7 item 3): for each leash
    /// of the region's entities, the chunks of other regions within the snap distance
    /// of its leashee, each with its region, so that regions that follow players merge
    /// where a holder may step across.
    pub(crate) fn leashes_at_line(&self) -> BTreeSet<(ChunkPos, Option<RegionId>)> {
        let mut at = BTreeSet::new();
        for chunk in self.chunks.values() {
            for entity in chunk.entities() {
                if entity.riding.leash.is_none() {
                    continue;
                }
                let [x, _, z] = entity.position;
                let low = ChunkPos::containing(x - SNAP_DISTANCE, z - SNAP_DISTANCE);
                let high = ChunkPos::containing(x + SNAP_DISTANCE, z + SNAP_DISTANCE);
                for chunk_x in low.x..=high.x {
                    for chunk_z in low.z..=high.z {
                        let near = ChunkPos::new(chunk_x, chunk_z);
                        if let Some(other) = self.land.knowledge(near).holder() {
                            at.insert((near, Some(other)));
                        }
                    }
                }
            }
        }
        at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn end(position: Vec3, yaw: f32, width: f32, height: f32, movement: Vec3) -> End {
        End {
            position,
            yaw,
            width,
            height,
            movement,
        }
    }

    /// Within the elastic distance less both widths nothing pulls, but the leashee
    /// still turns by its swing, which the friction slows.
    #[test]
    fn a_short_leash_only_turns_the_leashee_by_its_swing() {
        let boat = end([0.0, 0.0, 0.0], 10.0, 1.375, 0.5625, [0.0; 3]);
        let player = end([3.0, 0.0, 0.0], 0.0, 0.6, 1.8, [0.0; 3]);
        let mut motion = [0.0; 3];
        let mut fall = 2.0;
        let mut yaw = 10.0;
        let mut momentum = 2.0;
        let snapped = leash_tick(
            &boat,
            &player,
            &mut motion,
            &mut fall,
            &mut yaw,
            &mut momentum,
            0.91,
        );
        assert!(!snapped);
        assert_eq!(motion, [0.0; 3]);
        assert_eq!(fall, 2.0);
        assert_eq!(yaw, 8.0);
        assert_eq!(momentum.to_bits(), (2.0 * f64::from(0.91_f32)).to_bits());
    }

    /// Beyond twelve between the boxes' centres it breaks, and the leashee still turns.
    #[test]
    fn a_leash_longer_than_twelve_snaps() {
        let boat = end([0.0, 0.0, 0.0], 0.0, 1.375, 0.5625, [0.0; 3]);
        let player = end([12.5, 0.0, 0.0], 0.0, 0.6, 1.8, [0.0; 3]);
        let mut motion = [0.0; 3];
        let mut fall = 0.0;
        let mut yaw = 0.0;
        let mut momentum = 1.0;
        assert!(leash_tick(
            &boat,
            &player,
            &mut motion,
            &mut fall,
            &mut yaw,
            &mut momentum,
            0.8
        ));
        assert_eq!(motion, [0.0; 3]);
        assert_eq!(yaw, -1.0);
    }
}
