//! Riding, for units without players (`docs/groundwork/n5-brief.md`, sections 1.1 to
//! 1.8; `docs/adr/0028-entities.md`, section 5, "Vehicles and riders";
//! `docs/groundwork/n5-plan.md`, step N5b): the game's `startRiding`, `addPassenger` and
//! `removePassenger` with its order of seats, the boarding cooldown, `stopRiding` and
//! `ejectPassengers`, the level's tick of passengers (the root, then each passenger's
//! `rideTick` in the order of seats, depth first), `positionRider` by the types'
//! attachment points, and the place a living passenger gets off at
//! (`getDismountLocationForPassenger`, `DismountHelper`).
//!
//! A unit, a root and every entity that rides it however indirectly, lives in its
//! root's chunk: each member is a record of that chunk, whatever its own position, and
//! when the root changes chunk all of them go `Gone` from the old and `Whole` into the
//! new in the same tick, or are sent across a line together ([`Region::relocate`]).
//! Links are keys and stay as they are wherever the unit goes. A player's seat is kept in
//! the order with its flag, but players are seated by a later step (the riders' step,
//! N5h): until then a unit with a player seat is held back at a line, as a full chunk
//! holds an entity back (`docs/groundwork/n5-plan.md`, section 8 item 3).

use clustine_blocks::data::{clockwise, in_tag, is_class};
use clustine_data::{BlockClass, BlockState, EntityTypeInfo, Face};
use clustine_noise::trig;
use clustine_world::{
    BlockPos, ChunkPos, Entity, EntityChange, EntityData, EntityField, EntityId, Seat,
};

use super::Region;
use super::colliders::root_among;
use super::items::View;
use crate::api::{RegionEvent, TickOutput};
use crate::physics::aabb::Aabb;
use crate::physics::collide::{Context, block_collisions};
use crate::physics::entities::{is_boat, is_chest_boat, is_minecart, is_raft};
use crate::physics::voxel::Placed;
use crate::physics::{self, Vec3, World};

/// The ticks an entity that got off waits before it may ride again (`removePassenger`).
pub(crate) const BOARDING_COOLDOWN: u16 = 60;

/// The game's `Mth.DEG_TO_RAD`, a float.
const DEGREES_TO_RADIANS: f32 = 0.017_453_292;

/// `Mth.SQRT_OF_TWO`, the float.
const SQRT_OF_TWO: f32 = std::f32::consts::SQRT_2;

/// How deep a tick of passengers goes at most: no unit in play is deeper, and a
/// malformed record must not recurse without end.
const DEEPEST: usize = 64;

/// What became of an entity whose tick left it at a position, as to its chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Relocated {
    /// It stays in its chunk: it is there, or a passenger, or held back at a line.
    Stays,
    /// It moved into another loaded chunk of the region's, with its unit.
    Moved(ChunkPos),
    /// It was sent across a line, with its unit.
    Sent,
}

/// A pose a living passenger may get off in: its size, and how far up and down from
/// the vehicle's block a minecart looks for room (`POSE_DISMOUNT_HEIGHTS`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DismountPose {
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) heights: &'static [i32],
}

/// `POSE_DISMOUNT_HEIGHTS` of `STANDING` (and `CROUCHING`).
pub(crate) const STANDING_HEIGHTS: &[i32] = &[0, 1, -1];

/// `Vec3.yRot(angle)`: turned about the vertical by `angle` radians, through the game's
/// table of sines, each float widened before its multiply.
pub(crate) fn y_rot([x, y, z]: Vec3, angle: f32) -> Vec3 {
    let c = f64::from(trig::cos(f64::from(angle)));
    let s = f64::from(trig::sin(f64::from(angle)));
    [x * c + z * s, y, z * c - x * s]
}

/// `EntityAttachments.transformPoint(point, yRot)`: the point turned as an entity with
/// that yaw turns it, `yRot(-yRot * DEG_TO_RAD)`, the product a float.
pub(crate) fn transform_point(point: Vec3, yaw: f32) -> Vec3 {
    y_rot(point, -yaw * DEGREES_TO_RADIANS)
}

/// `Entity.addPassenger`'s order: a player goes first where the first is not a player,
/// anyone else last.
pub(crate) fn add_passenger(passengers: &mut Vec<Seat>, seat: Seat) {
    let first_is_player = passengers.first().is_some_and(|first| first.player);
    if seat.player && !passengers.is_empty() && !first_is_player {
        passengers.insert(0, seat);
    } else {
        passengers.push(seat);
    }
}

/// `Direction.fromYRot(yRot)`: the horizontal direction an entity of that yaw faces,
/// from `Mth.floor(yRot / 90 + 0.5) & 3` (south, west, north, east).
pub(crate) fn direction_from_yaw(yaw: f32) -> Face {
    match physics::floor(f64::from(yaw) / 90.0 + 0.5) & 3 {
        0 => Face::South,
        1 => Face::West,
        2 => Face::North,
        _ => Face::East,
    }
}

/// A horizontal direction's steps along x and z.
fn steps(face: Face) -> (i32, i32) {
    let [x, _, z] = physics::piston::step(face);
    (x, z)
}

/// `DismountHelper.offsetsForDirection(direction)`: beside first, square to it, then
/// the corners behind and ahead, then straight behind and straight ahead.
pub(crate) fn offsets_for_direction(direction: Face) -> [(i32, i32); 8] {
    let cw = steps(clockwise(direction));
    let ccw = (-cw.0, -cw.1);
    let ahead = steps(direction);
    let back = (-ahead.0, -ahead.1);
    let sum = |a: (i32, i32), b: (i32, i32)| (a.0 + b.0, a.1 + b.1);
    [
        cw,
        ccw,
        sum(back, cw),
        sum(back, ccw),
        sum(ahead, cw),
        sum(ahead, ccw),
        back,
        ahead,
    ]
}

/// `DismountHelper.isBlockFloorValid`.
pub(crate) fn is_floor_valid(height: f64) -> bool {
    !height.is_infinite() && height < 1.0
}

/// `VoxelShape.max(Y)` and `min(Y)` of a block's own shape: the top of its highest full
/// cell and the bottom of its lowest, `None` where it is empty.
fn shape_extent_y(shape: clustine_data::collisions::VoxelShape) -> Option<(f64, f64)> {
    use clustine_data::collisions::Axis;
    if shape.is_empty() {
        return None;
    }
    let (nx, ny, nz) = (
        shape.size(Axis::X),
        shape.size(Axis::Y),
        shape.size(Axis::Z),
    );
    let full_at = |y: usize| (0..nx).any(|x| (0..nz).any(|z| shape.is_full(x, y, z)));
    let low = (0..ny).find(|y| full_at(*y))?;
    let high = (0..ny).rev().find(|y| full_at(*y))?;
    Some((shape.coord(Axis::Y, low), shape.coord(Axis::Y, high + 1)))
}

/// The collision shape of a block with no entity in the context (the table's, which is
/// the empty context's answer), `None` where it is empty.
fn empty_context_shape(state: BlockState) -> Option<clustine_data::collisions::VoxelShape> {
    let shape = state.voxel_collision_shape();
    (!shape.is_empty()).then_some(shape)
}

/// `BlockGetter.getBlockFloorHeight(shape, below)`: the top of `shape`, or, where it is
/// empty, how far the block below's reaches up into this one, minus infinity where it
/// does not.
pub(crate) fn block_floor_height(
    here: Option<clustine_data::collisions::VoxelShape>,
    below: impl FnOnce() -> Option<clustine_data::collisions::VoxelShape>,
) -> f64 {
    if let Some((_, top)) = here.and_then(shape_extent_y) {
        return top;
    }
    match below().and_then(shape_extent_y) {
        Some((_, top)) if top >= 1.0 => top - 1.0,
        _ => f64::NEG_INFINITY,
    }
}

/// The boxes of a placed shape's full cells.
fn boxes_of(shape: &Placed) -> Vec<Aabb> {
    let size = [shape.size(0), shape.size(1), shape.size(2)];
    let mut boxes = Vec::new();
    for x in 0..size[0] {
        for y in 0..size[1] {
            for z in 0..size[2] {
                if shape.is_full(x as i64, y as i64, z as i64) {
                    boxes.push(Aabb {
                        min: [shape.coord(0, x), shape.coord(1, y), shape.coord(2, z)],
                        max: [
                            shape.coord(0, x + 1),
                            shape.coord(1, y + 1),
                            shape.coord(2, z + 1),
                        ],
                    });
                }
            }
        }
    }
    boxes
}

/// What is left of `from` once `cut` is taken out of it, as boxes.
fn subtract(from: Aabb, cut: &Aabb) -> Vec<Aabb> {
    if !from.intersects(*cut) {
        return vec![from];
    }
    let mut left = Vec::new();
    let mut rest = from;
    for axis in 0..3 {
        if cut.min[axis] > rest.min[axis] {
            let mut part = rest;
            part.max[axis] = cut.min[axis];
            left.push(part);
            rest.min[axis] = cut.min[axis];
        }
        if cut.max[axis] < rest.max[axis] {
            let mut part = rest;
            part.min[axis] = cut.max[axis];
            left.push(part);
            rest.max[axis] = cut.max[axis];
        }
    }
    left
}

/// The point of the boxes nearest `target` (`VoxelShape.closestPointTo`), the first
/// found where two are as near.
fn closest_point(boxes: &[Aabb], target: Vec3) -> Option<Vec3> {
    let mut best: Option<(f64, Vec3)> = None;
    for one in boxes {
        let point: Vec3 =
            std::array::from_fn(|axis| target[axis].clamp(one.min[axis], one.max[axis]));
        let distance = physics::distance_sqr(target, point);
        if best.is_none_or(|(nearest, _)| distance < nearest) {
            best = Some((distance, point));
        }
    }
    best.map(|(_, point)| point)
}

/// The dimensions a living passenger of `entity`'s kind gets off in, by the poses it
/// may take (`getDismountPoses`), or `None` for one that is not living, which stays
/// where it is when it gets off (`Entity.stopRiding` places nothing).
pub(crate) fn dismount_poses(entity: &Entity) -> Option<Vec<DismountPose>> {
    match &entity.data {
        EntityData::ArmorStand { flags, .. } => {
            let info = EntityTypeInfo::of(i32::from(entity.kind));
            // A small stand is half the size, a marker none (`ArmorStand.getDimensions`).
            let scale = if flags & 16 != 0 {
                0.0
            } else if flags & 1 != 0 {
                0.5
            } else {
                1.0
            };
            Some(vec![DismountPose {
                width: info.width * scale,
                height: info.height * scale,
                heights: STANDING_HEIGHTS,
            }])
        }
        _ => None,
    }
}

impl Region {
    /// The chunk of the entity `key` and its record, where a loaded chunk has it.
    pub(crate) fn find_entity(&self, key: EntityId) -> Option<(ChunkPos, &Entity)> {
        let chunk = *self.entity_chunks.get(&key)?;
        Some((chunk, self.entity_record(key, chunk)?))
    }

    /// The root of the unit the entity `key` is of: itself where it rides nothing.
    pub(crate) fn root_of(&self, key: EntityId) -> EntityId {
        self.entity_chunks
            .get(&key)
            .and_then(|chunk| self.chunks.get(chunk))
            .map_or(key, |chunk| root_among(chunk.entities(), key))
    }

    /// Sets `fields` of the entity `key` of the chunk `chunk`, committed.
    pub(crate) fn commit_entity(
        &mut self,
        chunk: ChunkPos,
        key: EntityId,
        fields: Vec<EntityField>,
        output: &mut TickOutput,
    ) {
        if fields.is_empty() {
            return;
        }
        let change = EntityChange::Set(key, fields);
        if let Some(loaded) = self.chunks.get_mut(&chunk) {
            loaded.apply_entity(&change);
        }
        output.entity_changes.push((chunk, change));
    }

    /// Sets `fields` of the entity `key` of the chunk `chunk` in place, as counters are:
    /// the chunk ages, and its record is saved as it is at the worker's checkpoints.
    pub(crate) fn age_entity(
        &mut self,
        chunk: ChunkPos,
        key: EntityId,
        fields: Vec<EntityField>,
        output: &mut TickOutput,
    ) {
        if let Some(loaded) = self.chunks.get_mut(&chunk) {
            loaded.apply_entity(&EntityChange::Set(key, fields));
        }
        output.entities_ageing.push(chunk);
    }

    /// The members of the unit whose root is `root`, as of `root` and the records of the
    /// chunk `from`: the root first, each passenger before its own passengers, in the
    /// order of seats (`getSelfAndPassengers`). Players are not records and are left out.
    pub(crate) fn unit_records(&self, from: ChunkPos, root: Entity) -> Vec<Entity> {
        let mut members = Vec::new();
        self.gather(from, root, &mut members);
        members
    }

    fn gather(&self, from: ChunkPos, entity: Entity, members: &mut Vec<Entity>) {
        let vehicle = entity.key;
        let seats: Vec<EntityId> = entity
            .riding
            .passengers
            .iter()
            .filter(|seat| !seat.player)
            .map(|seat| seat.key)
            .collect();
        members.push(entity);
        if members.len() > DEEPEST {
            return;
        }
        for key in seats {
            if members.iter().any(|member| member.key == key) {
                continue;
            }
            let Some(record) = self.entity_record(key, from) else {
                continue;
            };
            if record.riding.vehicle == Some(vehicle) {
                self.gather(from, record.clone(), members);
            }
        }
    }

    /// Whether a seat of the unit of `root` is a player's, seated or on the way.
    fn has_player_seat(&self, from: ChunkPos, root: &Entity) -> bool {
        self.unit_records(from, root.clone())
            .iter()
            .any(|member| member.riding.passengers.iter().any(|seat| seat.player))
    }

    /// Where the tick leaves the entity `after` of the chunk `from`, which was at
    /// `before` when its tick began (ADR-0028, section 5): a passenger stays in its
    /// unit's chunk wherever it sits; a root whose position is in another loaded chunk
    /// of the region's moves there with its unit, and one whose position is anywhere
    /// else is sent there with its unit. A unit with a player's seat is held back at the
    /// line, as the physics' guard holds an entity back from a full chunk: along each
    /// axis that would take it out it keeps its coordinate and loses its motion, until
    /// the riders' step hands riders over (`docs/groundwork/n5-plan.md`, section 8 item
    /// 3). Nothing of the entity is committed here but its going from `from`; the
    /// caller commits it where it stays.
    pub(crate) fn relocate(
        &mut self,
        from: ChunkPos,
        before: Vec3,
        after: &mut Entity,
        output: &mut TickOutput,
    ) -> Relocated {
        if after.riding.vehicle.is_some() {
            return Relocated::Stays;
        }
        let mut to = after.chunk();
        if to == from {
            return Relocated::Stays;
        }
        // A unit with a player in it goes as any other, its riders after it (`riders.rs`):
        // it is held back only where the riders cannot follow, into land the region
        // knows nothing of and does not hold, until it learns who holds it.
        if !self.chunks.contains_key(&to)
            && self.has_player_seat(from, after)
            && !self.riders_may_follow(to)
        {
            let may = |chunk: ChunkPos| chunk == from || self.chunks.contains_key(&chunk);
            let x_side = ChunkPos::containing(after.position[0], before[2]);
            if x_side != ChunkPos::containing(before[0], before[2]) && !may(x_side) {
                after.position[0] = before[0];
                after.motion[0] = 0.0;
            }
            let x = after.position[0];
            let z_side = ChunkPos::containing(x, after.position[2]);
            if z_side != ChunkPos::containing(x, before[2]) && !may(z_side) {
                after.position[2] = before[2];
                after.motion[2] = 0.0;
            }
            to = after.chunk();
            if to == from {
                return Relocated::Stays;
            }
        }
        if !self.chunks.contains_key(&to) {
            self.riders_follow(from, after, to, output);
            self.send_entity(from, after.clone(), output);
            return Relocated::Sent;
        }
        self.move_unit(from, to, after.clone(), output);
        Relocated::Moved(to)
    }

    /// The unit of `root` goes from the chunk `from` into the loaded chunk `to`: each
    /// member `Gone` from the one and `Whole` in the other, the root first.
    fn move_unit(&mut self, from: ChunkPos, to: ChunkPos, root: Entity, output: &mut TickOutput) {
        for member in self.unit_records(from, root) {
            let key = member.key;
            let gone = EntityChange::Gone(key);
            if let Some(chunk) = self.chunks.get_mut(&from) {
                chunk.apply_entity(&gone);
            }
            output.entity_changes.push((from, gone));
            let whole = EntityChange::Whole(member);
            if let Some(chunk) = self.chunks.get_mut(&to) {
                chunk.apply_entity(&whole);
            }
            output.entity_changes.push((to, whole));
            self.entity_chunks.insert(key, to);
        }
    }

    /// Settles the unit of the root `key` after its tick, which a kind's own tick may
    /// have left at a position in another chunk without moving it there itself: it
    /// began the tick at `before`. A root held back at a line commits where it is held.
    pub(crate) fn settle_unit(&mut self, key: EntityId, before: Vec3, output: &mut TickOutput) {
        let Some((chunk, root)) = self.find_entity(key) else {
            return;
        };
        // Most roots end their tick in their chunk, and nothing is to be done.
        if root.riding.vehicle.is_some() || root.chunk() == chunk {
            return;
        }
        let root = root.clone();
        let mut after = root.clone();
        if self.relocate(chunk, before, &mut after, output) == Relocated::Stays {
            let bits = |values: [f64; 3]| values.map(f64::to_bits);
            let mut fields = Vec::new();
            if bits(after.position) != bits(root.position) {
                fields.push(EntityField::Position(after.position));
            }
            if bits(after.motion) != bits(root.motion) {
                fields.push(EntityField::Motion(after.motion));
            }
            self.commit_entity(chunk, key, fields, output);
        }
    }

    /// Tells the edges who rides the vehicle `vehicle` of the chunk `chunk` now.
    fn report_passengers(&self, vehicle: &Entity, chunk: ChunkPos, output: &mut TickOutput) {
        output.events.push(RegionEvent::Passengers {
            vehicle: vehicle.key,
            chunk,
            hops: vehicle.hops,
            passengers: vehicle
                .riding
                .passengers
                .iter()
                .map(|seat| seat.key)
                .collect(),
        });
    }

    /// `canAddPassenger(rider)` of the vehicle `vehicle`: a boat takes two, a chest boat
    /// or a chest raft one, and none whose eye is under water; anything else one
    /// (minecarts, cushions). A seat kept for a player on their way counts.
    pub(crate) fn can_add_passenger(&self, vehicle: &Entity, chunk: ChunkPos) -> bool {
        let taken = vehicle.riding.passengers.len();
        if is_boat(vehicle.kind) {
            let most = if is_chest_boat(vehicle.kind) { 1 } else { 2 };
            return taken < most && !self.eye_in_water(vehicle, chunk);
        }
        taken == 0
    }

    /// `isEyeInFluid(#water)` of `entity`: water in the column of its block, among the
    /// cells of its fluid box, reaches its eyes. **Inferred**: the game keeps this from
    /// its last fluid update (`EntityFluidInteraction`, not in the dump) with the fluid's
    /// camera height, which is taken here as the fluid's height, worked out now.
    pub(crate) fn eye_in_water(&self, entity: &Entity, chunk: ChunkPos) -> bool {
        let info = EntityTypeInfo::of(i32::from(entity.kind));
        let view = View {
            region: self,
            home: chunk,
        };
        let bounds = self.entity_box(entity).deflate(0.001);
        let eye = entity.position[1] + f64::from(info.eye_height);
        let block = physics::containing(entity.position);
        let y0 = physics::floor(bounds.min[1]);
        let y1 = bounds.max[1].ceil() as i32 - 1;
        (y0..=y1).any(|y| {
            let pos = BlockPos::new(block.x, y, block.z);
            let state = view.state(pos);
            if !clustine_blocks::data::has_water(state) {
                return false;
            }
            let fluid = clustine_blocks::fluid::FluidState::of(state);
            let height = clustine_blocks::fluid::height(&mut physics::Rules(&view), fluid, pos);
            eye >= f64::from(y) && eye <= f64::from(y) + f64::from(height)
        })
    }

    /// `Entity.startRiding(vehicle, force, _)` of the entity `rider` (a player's own
    /// mount is the riders' step's): refused where it rides the vehicle already, where
    /// the vehicle rides it however indirectly, and, unless forced, where its boarding
    /// cooldown runs or the vehicle has no room. A rider that rides something else gets
    /// off it first. It takes its seat by `addPassenger`'s rule, and it and its own
    /// passengers join the vehicle's unit, in the root's chunk.
    pub(crate) fn start_riding(
        &mut self,
        rider: EntityId,
        vehicle: EntityId,
        force: bool,
        output: &mut TickOutput,
    ) -> bool {
        // An entity cannot ride itself; the game's cycle test does not ask, and nothing
        // in the game ever tries.
        if rider == vehicle {
            return false;
        }
        let (Some((_, rider_record)), Some((vehicle_chunk, vehicle_record))) =
            (self.find_entity(rider), self.find_entity(vehicle))
        else {
            return false;
        };
        if rider_record.riding.vehicle == Some(vehicle) {
            return false;
        }
        let mut up = vehicle_record.riding.vehicle;
        for _ in 0..DEEPEST {
            let Some(next) = up else {
                break;
            };
            if next == rider {
                return false;
            }
            up = self
                .find_entity(next)
                .and_then(|(_, record)| record.riding.vehicle);
        }
        if !force
            && (rider_record.riding.cooldown > 0
                || !self.can_add_passenger(vehicle_record, vehicle_chunk))
        {
            return false;
        }
        if rider_record.riding.vehicle.is_some() {
            self.stop_riding(rider, output);
        }
        let (Some((rider_chunk, rider_record)), Some((vehicle_chunk, vehicle_record))) =
            (self.find_entity(rider), self.find_entity(vehicle))
        else {
            return false;
        };
        let mut rider_record = rider_record.clone();
        let mut vehicle_record = vehicle_record.clone();
        rider_record.riding.vehicle = Some(vehicle);
        add_passenger(
            &mut vehicle_record.riding.passengers,
            Seat {
                key: rider,
                player: false,
                pending: None,
            },
        );
        self.commit_entity(
            vehicle_chunk,
            vehicle,
            vec![EntityField::Riding(vehicle_record.riding.clone())],
            output,
        );
        if rider_chunk == vehicle_chunk {
            self.commit_entity(
                rider_chunk,
                rider,
                vec![EntityField::Riding(rider_record.riding.clone())],
                output,
            );
        } else {
            self.move_unit(rider_chunk, vehicle_chunk, rider_record, output);
        }
        self.report_passengers(&vehicle_record, vehicle_chunk, output);
        true
    }

    /// `stopRiding()` of the entity `rider`: it leaves its vehicle's seats, waits the
    /// boarding cooldown before it rides again, and, if living, is put where the vehicle
    /// lets it off (`LivingEntity.dismountVehicle`). It is its own root then, and goes
    /// to the chunk its position is in.
    pub(crate) fn stop_riding(&mut self, rider: EntityId, output: &mut TickOutput) {
        self.leave_vehicle(rider, false, output);
    }

    /// `stopRiding()` of `rider`, whose vehicle is being removed when `vehicle_removed`.
    fn leave_vehicle(&mut self, rider: EntityId, vehicle_removed: bool, output: &mut TickOutput) {
        let Some((chunk, record)) = self.find_entity(rider) else {
            return;
        };
        let Some(vehicle) = record.riding.vehicle else {
            return;
        };
        let mut record = record.clone();
        let before = record.position;
        record.riding.vehicle = None;
        // `removePassenger` sets it on the passenger, whatever became of the vehicle.
        record.riding.cooldown = BOARDING_COOLDOWN;
        let vehicle_record = self
            .find_entity(vehicle)
            .map(|(chunk, entity)| (chunk, entity.clone()));
        if let Some((vehicle_chunk, mut vehicle_record)) = vehicle_record.clone() {
            vehicle_record
                .riding
                .passengers
                .retain(|seat| seat.key != rider);
            self.commit_entity(
                vehicle_chunk,
                vehicle,
                vec![EntityField::Riding(vehicle_record.riding.clone())],
                output,
            );
            self.report_passengers(&vehicle_record, vehicle_chunk, output);
            self.passenger_removed(&vehicle_record, vehicle_chunk, rider, output);
        }
        let mut fields = vec![EntityField::Riding(record.riding.clone())];
        if let Some(poses) = dismount_poses(&record) {
            let at = match &vehicle_record {
                Some((vehicle_chunk, vehicle_record)) if !vehicle_removed => {
                    self.dismount_location(vehicle_record, *vehicle_chunk, &record, &poses)
                }
                _ => self.free_dismount(vehicle_record.as_ref().map(|(_, v)| v), chunk, &record),
            };
            if at.map(f64::to_bits) != record.position.map(f64::to_bits) {
                record.position = at;
                fields.push(EntityField::Position(at));
            }
        }
        self.commit_entity(chunk, rider, fields, output);
        // `teleportPassengers`: its own passengers are put on it where it is now.
        if record.position.map(f64::to_bits) != before.map(f64::to_bits) {
            let seats: Vec<EntityId> = record
                .riding
                .passengers
                .iter()
                .filter(|seat| !seat.player)
                .map(|seat| seat.key)
                .collect();
            for seat in seats {
                self.position_rider(rider, seat, output);
            }
        }
        self.settle_unit(rider, before, output);
    }

    /// What `Entity.baseTick` does of riding, for the kinds that call it (boats, armour
    /// stands): an entity whose vehicle is gone gets off, and its boarding cooldown
    /// counts down, kept in place as counters are.
    pub(crate) fn base_tick_riding(&mut self, key: EntityId, output: &mut TickOutput) {
        let Some((_, record)) = self.find_entity(key) else {
            return;
        };
        if let Some(vehicle) = record.riding.vehicle
            && self.find_entity(vehicle).is_none()
        {
            self.stop_riding(key, output);
        }
        let Some((chunk, record)) = self.find_entity(key) else {
            return;
        };
        if record.riding.cooldown > 0 {
            let mut riding = record.riding.clone();
            riding.cooldown -= 1;
            self.age_entity(chunk, key, vec![EntityField::Riding(riding)], output);
        }
    }

    /// `removePassenger(passenger)`'s own part of the kind of `vehicle`.
    fn passenger_removed(
        &mut self,
        vehicle: &Entity,
        chunk: ChunkPos,
        passenger: EntityId,
        output: &mut TickOutput,
    ) {
        let _ = (chunk, passenger);
        if let EntityData::Cushion { .. } = vehicle.data {
            self.cushion_passenger_removed(vehicle, output);
        }
    }

    /// `ejectPassengers()` of `vehicle`: each passenger gets off, the last first.
    pub(crate) fn eject_passengers(&mut self, vehicle: EntityId, output: &mut TickOutput) {
        for _ in 0..DEEPEST {
            let Some((chunk, record)) = self.find_entity(vehicle) else {
                return;
            };
            let Some(last) = record.riding.passengers.last().cloned() else {
                return;
            };
            let rides = !last.player
                && self
                    .find_entity(last.key)
                    .is_some_and(|(_, passenger)| passenger.riding.vehicle == Some(vehicle));
            if rides {
                self.stop_riding(last.key, output);
            } else {
                // A player's seat, or one whose passenger is gone: it is given up. A
                // seated player gets off by the riders' step, which is to replace this.
                let mut record = record.clone();
                record.riding.passengers.pop();
                self.commit_entity(
                    chunk,
                    vehicle,
                    vec![EntityField::Riding(record.riding.clone())],
                    output,
                );
                self.report_passengers(&record, chunk, output);
            }
        }
    }

    /// `setRemoved(reason)`'s riding: the entity `key` gets off what it rides where the
    /// reason destroys it, and every passenger gets off it, as from a vehicle that is
    /// gone. The caller removes its record afterwards.
    pub(crate) fn unride_removed(
        &mut self,
        key: EntityId,
        destroyed: bool,
        output: &mut TickOutput,
    ) {
        if destroyed {
            self.stop_riding(key, output);
        }
        for _ in 0..DEEPEST {
            let Some((chunk, record)) = self.find_entity(key) else {
                return;
            };
            let Some(seat) = record.riding.passengers.first().cloned() else {
                return;
            };
            let rides = !seat.player
                && self
                    .find_entity(seat.key)
                    .is_some_and(|(_, passenger)| passenger.riding.vehicle == Some(key));
            if rides {
                self.leave_vehicle(seat.key, true, output);
            } else {
                let mut record = record.clone();
                record.riding.passengers.remove(0);
                self.commit_entity(
                    chunk,
                    key,
                    vec![EntityField::Riding(record.riding.clone())],
                    output,
                );
            }
        }
    }

    /// Whether the vehicle `vehicle` holds `passenger` among its seats.
    pub(crate) fn holds_passenger(&self, vehicle: EntityId, passenger: EntityId) -> bool {
        self.find_entity(vehicle).is_some_and(|(_, record)| {
            record
                .riding
                .passengers
                .iter()
                .any(|seat| seat.key == passenger)
        })
    }

    /// The passengers' part of the level's tick of the unit's member `vehicle`
    /// (`ServerLevel.tickNonPassenger` and `tickPassenger`): each passenger in the order
    /// of seats, whose `rideTick` zeroes its motion, ticks it as its kind ticks, and puts
    /// it on its vehicle; then its own passengers, depth first. One that no longer rides
    /// this vehicle gets off. Players tick in their own regions' turn.
    pub(crate) fn tick_passengers(&mut self, vehicle: EntityId, output: &mut TickOutput) {
        self.tick_passengers_of(vehicle, 0, output);
    }

    fn tick_passengers_of(&mut self, vehicle: EntityId, depth: usize, output: &mut TickOutput) {
        if depth > DEEPEST {
            return;
        }
        // The seats kept for players on their way count down with the vehicle's ticks.
        self.count_pending_seats(vehicle, output);
        let Some((_, record)) = self.find_entity(vehicle) else {
            return;
        };
        let seats: Vec<EntityId> = record
            .riding
            .passengers
            .iter()
            .filter(|seat| !seat.player)
            .map(|seat| seat.key)
            .collect();
        for passenger in seats {
            let Some((chunk, record)) = self.find_entity(passenger) else {
                continue;
            };
            if record.riding.vehicle != Some(vehicle) {
                self.stop_riding(passenger, output);
                continue;
            }
            // `rideTick`: the motion zeroed, the entity's own tick, then its seat.
            if record
                .motion
                .iter()
                .any(|component| component.to_bits() != 0)
            {
                self.commit_entity(
                    chunk,
                    passenger,
                    vec![EntityField::Motion([0.0; 3])],
                    output,
                );
            }
            let Some(record) = self.entity_record(passenger, chunk).cloned() else {
                continue;
            };
            self.tick_entity(record, chunk, output);
            let still = self
                .find_entity(passenger)
                .is_some_and(|(_, record)| record.riding.vehicle == Some(vehicle));
            if still {
                self.position_rider(vehicle, passenger, output);
                self.tick_passengers_of(passenger, depth + 1, output);
            }
        }
    }

    /// `positionRider(passenger)` of `vehicle`: the passenger is put at the vehicle's
    /// position and its passenger point for the passenger's seat, less the passenger's
    /// own vehicle point (`getPassengerRidingPosition` less `getVehicleAttachmentPoint`);
    /// a boat turns a rider with it and keeps it facing within 105 degrees of its own
    /// way (`AbstractBoat.positionRider`, `clampRotation`).
    pub(crate) fn position_rider(
        &mut self,
        vehicle: EntityId,
        passenger: EntityId,
        output: &mut TickOutput,
    ) {
        let (Some((_, carrier)), Some((chunk, rider))) =
            (self.find_entity(vehicle), self.find_entity(passenger))
        else {
            return;
        };
        let Some(index) = carrier
            .riding
            .passengers
            .iter()
            .position(|seat| seat.key == passenger)
        else {
            return;
        };
        let point = passenger_point(carrier, index);
        let attach = vehicle_point(rider);
        let at: Vec3 =
            std::array::from_fn(|axis| (carrier.position[axis] + point[axis]) - attach[axis]);
        let mut yaw = rider.yaw;
        if let EntityData::Boat { turning, .. } = &carrier.data {
            // Every passenger the server moves is the server's to turn: no player is
            // among these (`isLocalInstanceAuthoritative`); a breeze, which
            // `#can_turn_in_boats` exempts, comes with N7.
            yaw += turning.0;
            let wrapped = wrap_degrees(yaw - carrier.yaw);
            let clamped = wrapped.clamp(-105.0, 105.0);
            yaw += clamped - wrapped;
        }
        let mut fields = Vec::new();
        if at.map(f64::to_bits) != rider.position.map(f64::to_bits) {
            fields.push(EntityField::Position(at));
        }
        if yaw.to_bits() != rider.yaw.to_bits() {
            fields.push(EntityField::Rotation {
                yaw,
                pitch: rider.pitch,
            });
        }
        self.commit_entity(chunk, passenger, fields, output);
    }

    /// `getDismountLocationForPassenger(passenger)` of `vehicle`, in the chunk `chunk`,
    /// for a living passenger that may take `poses`: a minecart's room beside it, a
    /// boat's escape to the side, or else on top of the vehicle's box.
    pub(crate) fn dismount_location(
        &self,
        vehicle: &Entity,
        chunk: ChunkPos,
        passenger: &Entity,
        poses: &[DismountPose],
    ) -> Vec3 {
        let on_top = [
            vehicle.position[0],
            self.entity_box(vehicle).max[1],
            vehicle.position[2],
        ];
        let view = View {
            region: self,
            home: chunk,
        };
        // `LivingEntity.dismountVehicle`: a vehicle in a portal lets off as a gone one.
        if in_tag(
            view.state(physics::containing(vehicle.position)),
            "minecraft:portals",
        ) {
            return self.free_dismount(Some(vehicle), chunk, passenger);
        }
        let context = Context {
            bottom: passenger.position[1],
            fall_distance: passenger.fall_distance,
            falling_block: false,
            noclip: None,
        };
        let fits = |bounds: &Aabb| block_collisions(&view, context, bounds).is_empty();
        if is_minecart(vehicle.kind) {
            let flipped = matches!(vehicle.data, EntityData::Minecart { flipped: true, .. });
            let facing = direction_from_yaw(vehicle.yaw);
            // `OldMinecartBehavior.getMotionDirection`.
            let direction = if flipped {
                clockwise(facing.opposite())
            } else {
                clockwise(facing)
            };
            let base = physics::containing(vehicle.position);
            for pose in poses {
                let half = f64::from(pose.width.min(1.0) / 2.0);
                let bounds = Aabb::new([-half, 0.0, -half], [half, f64::from(pose.height), half]);
                for dy in pose.heights {
                    for (dx, dz) in offsets_for_direction(direction) {
                        let pos = BlockPos::new(base.x + dx, base.y + dy, base.z + dz);
                        let below = BlockPos::new(pos.x, pos.y - 1, pos.z);
                        let floor = block_floor_height(non_climbable(view.state(pos)), || {
                            non_climbable(view.state(below))
                        });
                        if !is_floor_valid(floor) {
                            continue;
                        }
                        let at = [
                            f64::from(pos.x) + 0.5,
                            f64::from(pos.y) + floor,
                            f64::from(pos.z) + 0.5,
                        ];
                        if fits(&bounds.moved(at)) {
                            return at;
                        }
                    }
                }
            }
            // The game then looks for headroom above the cart only to choose the pose,
            // which a passenger that is not a player does not have to choose.
            return on_top;
        }
        if is_boat(vehicle.kind) {
            let info = EntityTypeInfo::of(i32::from(vehicle.kind));
            let rider = EntityTypeInfo::of(i32::from(passenger.kind));
            let width = poses.first().map_or(rider.width, |pose| pose.width);
            let escape = escape_vector(
                f64::from(info.width * SQRT_OF_TWO),
                f64::from(width),
                passenger.yaw,
            );
            let x = vehicle.position[0] + escape[0];
            let z = vehicle.position[2] + escape[2];
            let at = physics::containing([x, on_top[1], z]);
            let below = BlockPos::new(at.x, at.y - 1, at.z);
            if !clustine_blocks::data::fluid_in_tag(view.state(below), "minecraft:water") {
                let floor_at = |pos: BlockPos| {
                    let under = BlockPos::new(pos.x, pos.y - 1, pos.z);
                    block_floor_height(empty_context_shape(view.state(pos)), || {
                        empty_context_shape(view.state(under))
                    })
                };
                let mut candidates = Vec::new();
                let here = floor_at(at);
                if is_floor_valid(here) {
                    candidates.push([x, f64::from(at.y) + here, z]);
                }
                let lower = floor_at(below);
                if is_floor_valid(lower) {
                    candidates.push([x, f64::from(below.y) + lower, z]);
                }
                for pose in poses {
                    // `getLocalBoundsForPose`: the half width a float.
                    let half = f64::from(pose.width / 2.0);
                    let bounds =
                        Aabb::new([-half, 0.0, -half], [half, f64::from(pose.height), half]);
                    for candidate in &candidates {
                        if fits(&bounds.moved(*candidate)) {
                            return *candidate;
                        }
                    }
                }
            }
            return on_top;
        }
        on_top
    }

    /// Where `LivingEntity.dismountVehicle` lets a living passenger off a vehicle that is
    /// gone (or in a portal): at the higher of the two's heights, moved to the nearest
    /// place its box is free of blocks (`CollisionGetter.findFreePosition`).
    /// **Inferred**: `VoxelShape.closestPointTo` is not in the dump; it is taken as the
    /// nearest point of the free part's boxes.
    fn free_dismount(&self, vehicle: Option<&Entity>, chunk: ChunkPos, passenger: &Entity) -> Vec3 {
        let bounds = self.entity_box(passenger);
        let (width, height) = (bounds.size()[0], bounds.size()[1]);
        let y = vehicle.map_or(passenger.position[1], |vehicle| {
            passenger.position[1].max(vehicle.position[1])
        });
        let at = [passenger.position[0], y, passenger.position[2]];
        if width > 4.0 || height > 4.0 {
            return at;
        }
        let half = height / 2.0;
        let centre = [at[0], at[1] + half, at[2]];
        // `AABB.ofSize(centre, w, h, w)`.
        let shape = Aabb::new(
            [
                centre[0] - width / 2.0,
                centre[1] - height / 2.0,
                centre[2] - width / 2.0,
            ],
            [
                centre[0] + width / 2.0,
                centre[1] + height / 2.0,
                centre[2] + width / 2.0,
            ],
        );
        let view = View {
            region: self,
            home: chunk,
        };
        let context = Context {
            bottom: passenger.position[1],
            fall_distance: passenger.fall_distance,
            falling_block: false,
            noclip: None,
        };
        let search = shape.inflate([width, height, width]);
        let mut free = vec![shape];
        for found in block_collisions(&view, context, &search) {
            for obstacle in boxes_of(&found.shape) {
                let grown = obstacle.inflate([width / 2.0, height / 2.0, width / 2.0]);
                free = free
                    .into_iter()
                    .flat_map(|part| subtract(part, &grown))
                    .collect();
            }
        }
        match closest_point(&free, centre) {
            Some(point) => [point[0], point[1] - half, point[2]],
            None => at,
        }
    }
}

/// `Mth.wrapDegrees(float)`.
pub(crate) fn wrap_degrees(degrees: f32) -> f32 {
    let mut wrapped = degrees % 360.0;
    if wrapped >= 180.0 {
        wrapped -= 360.0;
    }
    if wrapped < -180.0 {
        wrapped += 360.0;
    }
    wrapped
}

/// `Entity.getCollisionHorizontalEscapeVector(vehicleWidth, passengerWidth, yaw)`: far
/// enough to the side, along the passenger's facing, that the two boxes no longer meet.
pub(crate) fn escape_vector(vehicle_width: f64, passenger_width: f64, yaw: f32) -> Vec3 {
    let d = (vehicle_width + passenger_width + 9.999_999_747_378_752e-6) / 2.0;
    let s = -trig::sin(f64::from(yaw * DEGREES_TO_RADIANS));
    let c = trig::cos(f64::from(yaw * DEGREES_TO_RADIANS));
    let m = s.abs().max(c.abs());
    [
        f64::from(s) * d / f64::from(m),
        0.0,
        f64::from(c) * d / f64::from(m),
    ]
}

/// `DismountHelper.nonClimbableShape`: the block's collision shape, but none for a
/// climbable block or an open trapdoor.
pub(crate) fn non_climbable(state: BlockState) -> Option<clustine_data::collisions::VoxelShape> {
    let open_trapdoor =
        is_class(state, BlockClass::TrapDoorBlock) && clustine_blocks::data::flag(state, "open");
    if in_tag(state, "minecraft:climbable") || open_trapdoor {
        return None;
    }
    empty_context_shape(state)
}

/// `getPassengerAttachmentPoint(passenger, dimensions, 1.0f)` of `vehicle` for the seat
/// `index`, turned as the vehicle faces: a boat's own seats (`AbstractBoat`, the
/// brief's 4.6), otherwise the type's points, the last for any seat beyond them
/// (`EntityAttachments.getClamped`).
pub(crate) fn passenger_point(vehicle: &Entity, index: usize) -> Vec3 {
    let info = EntityTypeInfo::of(i32::from(vehicle.kind));
    if is_boat(vehicle.kind) {
        let mut along: f32 = if is_chest_boat(vehicle.kind) {
            0.15
        } else {
            0.0
        };
        if vehicle.riding.passengers.len() > 1 {
            along = if index == 0 { 0.2 } else { -0.6 };
            // An `Animal` sits 0.2 further forward: none rides a boat before N7.
        }
        let height = if is_raft(vehicle.kind) {
            f64::from(info.height * 0.888_888_9)
        } else {
            f64::from(info.height / 3.0)
        };
        return y_rot(
            [0.0, height, f64::from(along)],
            -vehicle.yaw * DEGREES_TO_RADIANS,
        );
    }
    let point = match info.passenger_points {
        [] => [0.0, f64::from(info.height), 0.0],
        points => points[index.min(points.len() - 1)],
    };
    transform_point(scaled(vehicle, point), vehicle.yaw)
}

/// `getVehicleAttachmentPoint(vehicle)` of `passenger`: its type's first vehicle point,
/// turned as it faces.
pub(crate) fn vehicle_point(passenger: &Entity) -> Vec3 {
    let info = EntityTypeInfo::of(i32::from(passenger.kind));
    let point = info.vehicle_points.first().copied().unwrap_or([0.0; 3]);
    transform_point(scaled(passenger, point), passenger.yaw)
}

/// An attachment point as a small armour stand's size scales it (`EntityDimensions.scale`
/// scales the attachments with the box). **Inferred**: the stand's small size is its
/// baby size, half, as `getDimensions` reads it.
fn scaled(entity: &Entity, point: Vec3) -> Vec3 {
    match &entity.data {
        EntityData::ArmorStand { flags, .. } if flags & 1 != 0 => {
            point.map(|component| component * f64::from(0.5_f32))
        }
        _ => point,
    }
}
