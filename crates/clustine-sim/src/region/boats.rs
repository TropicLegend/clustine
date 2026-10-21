//! Boats and rafts, with a chest or not (`AbstractBoat`, `Boat`, `Raft`,
//! `AbstractChestBoat`, `BoatItem`; `docs/groundwork/n5-brief.md`, section 4; step N5f
//! of `docs/groundwork/n5-plan.md`): a boat placed where the player's ray meets water or
//! a block, ridden by a click, driven by the client of the player in its first seat
//! through the game's check of each move, its paddles kept as that client says and
//! shown, and, while no player drives it, floated, carried by currents and bubble
//! columns, moved, and pushing what it meets; hit and broken as `VehicleEntity` is.
//!
//! The arithmetic of its tick is the game's (`crate::physics::boat`,
//! `crate::physics::vehicle_move`). What its tick does to other entities (a push, a
//! passenger let off) is done on their records as the game does it in the middle of the
//! tick, the boat's own record put where the boat is first; the boat's own fields are
//! committed at the end of its tick, as a cart's are.
//!
//! What is committed and what is counted: its place, its motion, its turn, whether it
//! is on the ground, its fire, its paddles and its status are committed when they
//! change; its hurt data, its bubble time, its ticks out of control, its turning and
//! the status of the tick before are counters, changed in place and saved at the worker's checkpoints
//! (`docs/groundwork/n5-plan.md`, section 7 item 7). What the game keeps only in memory
//! (`lastYd`, the paddles' places in their strokes, the moves a driver made since its
//! last tick) the region keeps by key ([`super::items::BoatMemory`]).
//!
//! Named differences and what could not be settled:
//! - The effects of the blocks a boat is in are applied as a cart's are, in the order
//!   the walk met the blocks, with fire's and lava's after the others, rather than by
//!   the game's collector step by step: fire ignites it and hurts it, lava ignites it
//!   and hurts it, water puts it out. Powder snow does not freeze it.
//! - Players are not in copies and the physics are given no players' boxes (plan,
//!   section 7 item 3): a boat nobody drives passes through a player rather than stop
//!   against them. A player the region knows pushes a boat and is pushed by one, but
//!   the player's client is not sent the nudge.
//! - A boat meets and pushes only the entities of the region's own chunks.
//! - Nothing living is picked up before N7: an armour stand is never pushable, and the
//!   game's rule for the living is kept for N7's mobs.
//! - A boat's custom name and the item's entity data are for later; its leash is
//!   `leashes.rs`'s.
//! - `isPickable` of the kinds a boat's ray may be stopped by is taken as true for all
//!   but items (**Inferred**: `Entity::isPickable` is the target of
//!   `EntitySelector.CAN_BE_PICKED`, which `javap -c` does not show).
//! - The server's `allow-flight` is false: Clustine has no such setting.
//! - The paddles' places in their strokes start again at a take-in, so a sound of a
//!   stroke may come early or late once at a line.

use clustine_blocks::data::{entity_in_tag, flag, has_lava, has_water, is_class};
use clustine_data::{BlockClass, BlockState, ENTITY_TYPES, EntityTypeInfo, ITEMS, sounds};
use clustine_noise::trig;
use clustine_world::{
    BlockPos, ChunkPos, Double, Entity, EntityChange, EntityData, EntityField, EntityId, EntityUse,
    Float, Hurt, PlayerId,
};

use super::Region;
use super::entities::insecure_uuid;
use super::items::{BoatMemory, Memory, View, tracked};
use super::leashes;
use super::random_ticks::Random;
use super::riding::Relocated;
use crate::api::{PlayerEvent, RegionEvent, TickOutput, Tracked, VehicleMove};
use crate::mobs::Mob;
use crate::physics::aabb::Aabb;
use crate::physics::boat::{self, Floating, NO_STATUS, OUT_OF_CONTROL, Status};
use crate::physics::body::{Body, Bounce, Movement};
use crate::physics::entities::{Mover, is_boat, is_chest_boat, is_pushable};
use crate::physics::item::{Inside, SOUND_SOURCE_NEUTRAL, check_inside_blocks, tick_movements};
use crate::physics::minecart::entity_push;
use crate::physics::vehicle_move::{self, Checked, Root, Verdict};
use crate::physics::{self, Vec3, World, fluids};
use crate::placement::ItemUse;
use crate::state::{PlayerRiding, VehicleCheck};

/// The ticks a bubble column's push takes to come (`setBubbleTime(60)`).
const BUBBLE_TIME: i32 = 60;

/// `0.3926991f`: an eighth of a turn of a paddle's stroke a tick (the float nearest a
/// sixteenth of a turn, the same bits as the library's `π / 8`).
const PADDLE_STEP: f32 = std::f32::consts::FRAC_PI_8;

/// `6.2831855f`, a whole stroke.
const STROKE: f32 = 6.283_185_5;

/// `0.7853981852531433`: the end of the part of a stroke a paddle's sound starts in.
const PADDLE_WINDOW: f64 = 0.785_398_185_253_143_3;

/// The reason a client is shown for being put out for holding a vehicle in the air.
pub(crate) const FLYING: &str = "multiplayer.disconnect.flying";

/// The reason for a vehicle's move that is not a number.
pub(crate) const INVALID_VEHICLE_MOVEMENT: &str = "multiplayer.disconnect.invalid_vehicle_movement";

/// The boat type an item makes (`BoatItem`'s type): the type of the item's own name.
/// **Inferred**: the items are registered with their types where the dump does not
/// show; each boat item has the name of its type (the brief's table 8.4).
pub(crate) fn boat_of_item(item: i32) -> Option<u16> {
    let name = ITEMS.get(usize::try_from(item).ok()?)?.name;
    let kind = ENTITY_TYPES.iter().position(|entity| *entity == name)?;
    let kind = u16::try_from(kind).ok()?;
    is_boat(kind).then_some(kind)
}

/// `getMaxPassengers()`: two, a chest boat's or a chest raft's one.
pub(crate) fn max_passengers(kind: u16) -> usize {
    if is_chest_boat(kind) { 1 } else { 2 }
}

/// `Entity.calculateViewVector(xRot, yRot)`, with the game's table of sines.
fn view_vector(yaw: f32, pitch: f32) -> Vec3 {
    let pitch = pitch * 0.017_453_292;
    let yaw = -yaw * 0.017_453_292;
    let yaw_cos = trig::cos(f64::from(yaw));
    let yaw_sin = trig::sin(f64::from(yaw));
    let pitch_cos = trig::cos(f64::from(pitch));
    let pitch_sin = trig::sin(f64::from(pitch));
    [
        f64::from(yaw_sin * pitch_cos),
        f64::from(-pitch_sin),
        f64::from(yaw_cos * pitch_cos),
    ]
}

/// The parts of a boat's record that its tick reads and writes beside its body.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Parts {
    paddles: [bool; 2],
    bubble_time: i32,
    out_of_control: f32,
    turning: f32,
    status: [u8; 2],
}

fn parts_of(data: &EntityData) -> Option<Parts> {
    match data {
        EntityData::Boat {
            paddles,
            bubble_time,
            out_of_control,
            turning,
            status,
            ..
        } => Some(Parts {
            paddles: *paddles,
            bubble_time: *bubble_time,
            out_of_control: out_of_control.0,
            turning: turning.0,
            status: *status,
        }),
        _ => None,
    }
}

/// `data` with the boat's parts `parts`.
fn with_parts(data: &EntityData, parts: Parts) -> EntityData {
    let mut data = data.clone();
    if let EntityData::Boat {
        paddles,
        bubble_time,
        out_of_control,
        turning,
        status,
        ..
    } = &mut data
    {
        *paddles = parts.paddles;
        *bubble_time = parts.bubble_time;
        *out_of_control = Float(parts.out_of_control);
        *turning = Float(parts.turning);
        *status = parts.status;
    }
    data
}

/// The parts of a boat's data that a tick commits when they change: its paddles, its
/// status and what it holds. Its hurt data, bubble time, ticks out of control, turning
/// and the status of the tick before (which only follows the status) are counters.
type CommittedParts<'a> = ([bool; 2], u8, &'a clustine_world::Contents);

fn committed_parts(data: &EntityData) -> Option<CommittedParts<'_>> {
    match data {
        EntityData::Boat {
            paddles,
            status,
            contents,
            ..
        } => Some((*paddles, status[0], contents)),
        _ => None,
    }
}

/// The parts of a boat's data its clients are shown: the hurt data, the paddles and
/// the bubble time.
fn shown_parts(data: &EntityData) -> Option<(Hurt, [bool; 2], i32)> {
    match data {
        EntityData::Boat {
            hurt,
            paddles,
            bubble_time,
            ..
        } => Some((*hurt, *paddles, *bubble_time)),
        _ => None,
    }
}

/// `AbstractBoat.push(other)`'s test of whether it pushes at all: another boat only
/// while its bottom is below this one's top, anything else only while its bottom is no
/// higher than this one's.
fn pushes(this: &Aabb, other: &Aabb, other_is_boat: bool) -> bool {
    if other_is_boat {
        other.min[1] < this.max[1]
    } else {
        other.min[1] <= this.min[1]
    }
}

/// Whether the first seat of `vehicle` is a player's, seated or on the way: such a boat
/// is driven by that player's client (`getControllingPassenger`), and a seat kept for
/// one on the way keeps it still until they come (the plan, section 7 item 4).
fn driven(vehicle: &Entity) -> bool {
    vehicle
        .riding
        .passengers
        .first()
        .is_some_and(|seat| seat.player)
}

/// Whether the player whose entity is `player` drives `vehicle`: a boat whose first
/// seat is theirs and taken.
fn drives(vehicle: &Entity, player: EntityId) -> bool {
    is_boat(vehicle.kind)
        && vehicle
            .riding
            .passengers
            .first()
            .is_some_and(|seat| seat.player && seat.key == player && seat.pending.is_none())
}

/// The blocks a boat's box is in, as the walk of `checkInsideBlocks` meets them.
struct BoatInside<'a> {
    body: &'a Body,
    met: Vec<(BlockState, BlockPos, bool, bool)>,
}

impl Inside for BoatInside<'_> {
    fn body(&self) -> &Body {
        self.body
    }

    fn removed(&self) -> bool {
        false
    }

    fn met(&mut self, state: BlockState, pos: BlockPos, _step: i32, block: bool, fluid: bool) {
        self.met.push((state, pos, block, fluid));
    }
}

/// A boat in its tick: its body and what its tick keeps of it.
#[derive(Debug, Clone)]
struct Tick {
    key: EntityId,
    kind: u16,
    body: Body,
    yaw: f32,
    pitch: f32,
    fire_ticks: i32,
    parts: Parts,
    memory: BoatMemory,
    /// `isAboveBubbleColumn` and `bubbleColumnDirectionIsDown`, which its blocks set
    /// and its bubble column's tick reads in the same tick.
    above_bubble_column: bool,
    bubble_down: bool,
    /// `isInLava`, as the tick's fluid update found it.
    in_lava: bool,
    removed: bool,
    needs_sync: bool,
    sync_velocity: bool,
}

impl Tick {
    fn status(&self) -> Option<Status> {
        Status::of(self.parts.status[0])
    }

    /// `igniteForTicks`.
    fn ignite_for_ticks(&mut self, ticks: i32) {
        if self.fire_ticks < ticks {
            self.fire_ticks = ticks;
        }
    }
}

/// What a boat's move's bounce reads of it: its gravity, its air drag of 1, no bounce
/// of its own, not living.
fn boat_bounce() -> Bounce {
    Bounce {
        gravity: boat::GRAVITY,
        air_drag: boat::AIR_DRAG,
        own: 0.0,
        living: false,
        rails_keep_speed: false,
        max_up_step: 0.0,
    }
}

/// Whether a boat nobody drives may pick `entity` up, beside the room and width it
/// has: a `LivingEntity` not of `#minecraft:cannot_be_pushed_onto_boats`. No kind
/// before N7 is both living and pushable (an armour stand never is pushable), so none
/// is picked up; the tag is read for the kinds that will be.
fn living_pickable(entity: &Entity) -> bool {
    let living = matches!(entity.data, EntityData::ArmorStand { .. });
    living
        && !entity_in_tag(
            i32::from(entity.kind),
            "minecraft:cannot_be_pushed_onto_boats",
        )
}

/// The client of `id` is told where the vehicle it drives is (the clientbound
/// `move_vehicle`).
fn corrected(id: PlayerId, position: Vec3, yaw: f32, pitch: f32, output: &mut TickOutput) {
    output.player_events.push((
        id,
        PlayerEvent::VehicleCorrected {
            position: clustine_world::Vec3::new(position[0], position[1], position[2]),
            yaw,
            pitch,
        },
    ));
}

impl Region {
    /// `BoatItem.use`: a boat of the held item's kind where the player's ray from their
    /// eyes meets a block's outline or any fluid's surface, turned as the player is,
    /// unless an entity's box holds the player's eyes or the boat's box would meet a
    /// block's shape, a boat, a cart or a player. A creative player keeps the item
    /// (`ItemStack.consume`), and every player is one.
    pub(crate) fn place_boat(
        &mut self,
        used: &ItemUse,
        player: PlayerId,
        output: &mut TickOutput,
    ) -> bool {
        let Some(kind) = used.held.and_then(boat_of_item) else {
            return false;
        };
        let Ok(Some(hit)) = crate::placement::boat_hit(used, &super::Readable(self)) else {
            return false;
        };
        let eye = [
            used.feet.x,
            used.feet.y + if used.sneaking { 1.27 } else { 1.62 },
            used.feet.z,
        ];
        if self.eye_in_an_entity(eye, player) {
            return false;
        }
        let info = EntityTypeInfo::of(i32::from(kind));
        let bounds = Aabb::of_entity(hit, info.width, info.height);
        let chunk = ChunkPos::containing(hit[0], hit[2]);
        let view = View {
            region: self,
            home: chunk,
        };
        // The new boat is no entity yet: it meets every other.
        let mover = Mover {
            key: EntityId(u64::MAX),
            root: EntityId(u64::MAX),
            like_a_vehicle: true,
        };
        let context = crate::physics::collide::Context {
            bottom: hit[1],
            fall_distance: 0.0,
            falling_block: false,
            noclip: None,
        };
        if !crate::physics::collide::no_collision(&view, context, &mover, &bounds)
            || self.a_player_in(&bounds)
        {
            return false;
        }
        let here = self.ticks_chunk(chunk);
        let key = if here {
            self.take_entity_key(chunk)
        } else {
            self.take_key()
        };
        let Some(key) = key else {
            return false;
        };
        // `Entity.<init>`: the UUID's two longs from the new entity's own random.
        let mut own = self.entity_stream(key).random;
        let uuid = insecure_uuid(own.next_u64(), own.next_u64());
        let entity = Entity {
            key,
            kind,
            uuid,
            position: hit,
            motion: [0.0; 3],
            yaw: used.yaw,
            pitch: 0.0,
            on_ground: false,
            hops: 0,
            lived: 0,
            fresh_at: 0,
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
            data: EntityData::Boat {
                hurt: Hurt::default(),
                paddles: [false; 2],
                bubble_time: 0,
                out_of_control: Float(0.0),
                turning: Float(0.0),
                status: [NO_STATUS; 2],
                contents: Vec::new(),
            },
        };
        if here {
            self.entity_drew(key, 2);
            self.entity_memory.insert(key, Memory::default());
            self.add_fresh_entity(entity, output);
        } else {
            self.send_made(entity, output);
        }
        true
    }

    /// Whether the box of an entity that can be picked, or of a player the region
    /// knows other than `player`, holds the point `eye` (`BoatItem.use`'s refusal): any
    /// box that holds the eyes is within the box the game looks in.
    fn eye_in_an_entity(&self, eye: Vec3, player: PlayerId) -> bool {
        let low = ChunkPos::containing(eye[0] - 3.0, eye[2] - 3.0);
        let high = ChunkPos::containing(eye[0] + 3.0, eye[2] + 3.0);
        for x in low.x..=high.x {
            for z in low.z..=high.z {
                let Some(chunk) = self.readable_chunk(ChunkPos::new(x, z)) else {
                    continue;
                };
                let held = chunk.entities().iter().any(|entity| {
                    !matches!(entity.data, EntityData::Item { .. })
                        && self.entity_box(entity).contains(eye)
                });
                if held {
                    return true;
                }
            }
        }
        self.players.iter().any(|(id, other)| {
            let at = other.pose.position;
            *id != player && Aabb::of_entity([at.x, at.y, at.z], 0.6, 1.8).contains(eye)
        })
    }

    /// Whether the box of a player the region knows, who rides nothing, meets `bounds`
    /// grown by 1e-7, as `getEntityCollisions` grows it: a boat collides with players,
    /// who are pushable.
    fn a_player_in(&self, bounds: &Aabb) -> bool {
        let reach = bounds.inflate([1.0e-7; 3]);
        self.players.values().any(|player| {
            let at = player.pose.position;
            player.riding.is_none()
                && Aabb::of_entity([at.x, at.y, at.z], 0.6, 1.8).intersects(reach)
        })
    }

    /// Whether the player whose entity is `player` drives the root `root`: a boat's
    /// first seated player, or a mob's whom its type lets steer it as they hold what they
    /// hold now (`getControllingPassenger`, `docs/groundwork/n7h-riding-plan.md`,
    /// section 3 item 2).
    pub(crate) fn drives_root(&self, root: &Entity, player: EntityId) -> bool {
        if is_boat(root.kind) {
            return drives(root, player);
        }
        let first = root.riding.passengers.first();
        if !first.is_some_and(|seat| seat.player && seat.key == player && seat.pending.is_none()) {
            return false;
        }
        Mob::decode(&root.data).is_ok_and(|mob| {
            matches!(
                self.ridden(root, &mob),
                crate::mobs::tick::Ridden::Driven { .. }
            )
        })
    }

    /// Whether the boat `key` is driven, as its record says now.
    fn is_driven(&self, key: EntityId) -> bool {
        self.find_entity(key)
            .is_some_and(|(_, record)| driven(record))
    }

    /// Whether the boat `key` carries anyone (`isVehicle`).
    fn boat_carries(&self, key: EntityId) -> bool {
        self.find_entity(key)
            .is_some_and(|(_, record)| !record.riding.passengers.is_empty())
    }

    /// The boat of `entity`, of the loaded chunk `chunk`, as its tick starts.
    fn boat_tick_of(&self, entity: &Entity, chunk: ChunkPos) -> Option<Tick> {
        let parts = parts_of(&entity.data)?;
        let info = EntityTypeInfo::of(i32::from(entity.kind));
        let memory = self.entity_memory.get(&entity.key);
        let body = Body {
            position: entity.position,
            motion: entity.motion,
            on_ground: entity.on_ground,
            fall_distance: entity.fall_distance,
            width: info.width,
            height: info.height,
            old_position: entity.position,
            horizontal_collision: false,
            vertical_collision: false,
            vertical_collision_below: memory
                .is_some_and(|memory| memory.boat.vertical_collision_below),
            supporting: memory.and_then(|memory| memory.supporting),
            on_ground_no_blocks: memory.is_some_and(|memory| memory.on_ground_no_blocks),
            stuck_speed: memory.map_or([0.0; 3], |memory| memory.stuck_speed),
            movements: memory.map_or_else(Vec::new, |memory| memory.boat.recorded.clone()),
            in_water: false,
            sync_position: false,
            falling_block: false,
            item: false,
            fell_on: None,
            noclip: None,
            mover: self.mover_of(entity, chunk),
            steps: None,
        };
        Some(Tick {
            key: entity.key,
            kind: entity.kind,
            body,
            yaw: entity.yaw,
            pitch: entity.pitch,
            fire_ticks: entity.fire_ticks,
            parts,
            memory: memory.map(|memory| memory.boat.clone()).unwrap_or_default(),
            above_bubble_column: false,
            bubble_down: false,
            in_lava: false,
            removed: false,
            needs_sync: false,
            sync_velocity: false,
        })
    }

    /// Puts the boat's place, motion, turn, fire and parts into its record in place, so
    /// that what its tick does to others and to itself through its record (a hit, a
    /// passenger let off) reads it where it is now; nothing is committed.
    fn sync_boat(&mut self, boat: &Tick) {
        let Some((chunk, record)) = self.find_entity(boat.key) else {
            return;
        };
        let data = with_parts(&record.data, boat.parts);
        let Some(loaded) = self.chunks.get_mut(&chunk) else {
            return;
        };
        let fields = vec![
            EntityField::Position(boat.body.position),
            EntityField::Motion(boat.body.motion),
            EntityField::Rotation {
                yaw: boat.yaw,
                pitch: boat.pitch,
            },
            EntityField::OnGround(boat.body.on_ground),
            EntityField::FireTicks(boat.fire_ticks),
            EntityField::Data(data),
        ];
        loaded.apply_entity(&EntityChange::Set(boat.key, fields));
    }

    /// The chunk the boat is in now, after something that may have moved or removed it
    /// (a passenger let off, a hit); `None`, and the boat marked gone, where it is gone.
    fn boat_chunk(&self, boat: &mut Tick) -> Option<ChunkPos> {
        let Some((chunk, _)) = self.find_entity(boat.key) else {
            boat.removed = true;
            return None;
        };
        Some(chunk)
    }

    /// `AbstractBoat.tick()` on the server of `entity`, in the loaded chunk `chunk`: its
    /// status and its time out of control, its passengers let off after sixty ticks
    /// under water, its wobble's fall, `baseTick` (riding, the water's push, fire), then,
    /// while no player drives it, its float and its own move, or, while one does, its
    /// motion and turning held at nothing; the blocks it is in, twice; its bubble
    /// column; its paddles' sounds; and what it pushes or picks up.
    pub(crate) fn tick_boat(&mut self, entity: Entity, chunk: ChunkPos, output: &mut TickOutput) {
        let key = entity.key;
        let Some(mut boat) = self.boat_tick_of(&entity, chunk) else {
            return;
        };
        let found = {
            let view = View {
                region: self,
                home: chunk,
            };
            boat::status(&view, &boat.body.bounds())
        };
        let old_status = boat.status();
        boat.parts.status = [found.status as u8, boat.parts.status[0]];
        boat.parts.out_of_control = if Status::is_under_water(Some(found.status)) {
            boat.parts.out_of_control + 1.0
        } else {
            0.0
        };
        self.sync_boat(&boat);
        if boat.parts.out_of_control >= OUT_OF_CONTROL {
            self.eject_passengers(key, output);
            if self.boat_chunk(&mut boat).is_none() {
                return;
            }
        }
        self.decay_hurt(key, output);
        // `baseTick`: an entity whose vehicle is gone gets off, then the fluids.
        self.base_tick_riding(key, output);
        let Some((chunk, record)) = self.find_entity(key) else {
            return;
        };
        boat.body.position = record.position;
        boat.body.old_position = record.position;
        let in_fluids = {
            let view = View {
                region: self,
                home: chunk,
            };
            fluids::update(&view, &boat.body.bounds())
        };
        boat.body.in_water = in_fluids.in_water();
        if in_fluids.in_water() {
            boat.body.fall_distance = 0.0;
        }
        boat.body.motion = fluids::push(&in_fluids, boat.body.motion);
        // `isInLava` is false in an entity's first tick, before any fluid update.
        boat.in_lava = entity.tick_count() > 0 && in_fluids.in_lava();
        if boat.fire_ticks > 0 {
            if boat.fire_ticks % 20 == 0 && !boat.in_lava {
                self.boat_hurt(&mut boat, 1.0, output);
                if boat.removed {
                    return;
                }
            }
            boat.fire_ticks -= 1;
        }
        if boat.in_lava {
            boat.body.fall_distance *= 0.5;
        }
        let min_y = self.chunks.get(&chunk).map_or(-64, |chunk| chunk.min_y());
        if boat.body.position[1] < f64::from(min_y - 64) {
            // `onBelowWorld` discards it, and a chest boat spills as it goes.
            self.remove_destroyed(key, output);
            return;
        }
        // `Leashable.tickLeash`, the end of `baseTick`.
        self.boat_tick_leash(&mut boat, chunk, output);
        if boat.removed {
            return;
        }
        if self.is_driven(key) {
            boat.body.motion = [0.0; 3];
            boat.parts.turning = 0.0;
        } else {
            boat.parts.paddles = [false; 2];
            self.float_and_move(&mut boat, chunk, found, old_status);
        }
        // `applyEffectsFromBlocks()` twice: the tick's moves, then where it began the
        // tick to where it is.
        for _ in 0..2 {
            let movements = tick_movements(&mut boat.body);
            self.boat_effects_from_blocks(&mut boat, chunk, &movements, output);
            if boat.removed {
                return;
            }
        }
        boat.body.movements.clear();
        self.tick_bubble_column(&mut boat, output);
        let Some(chunk) = self.boat_chunk(&mut boat) else {
            return;
        };
        self.paddle_sounds(&mut boat, chunk, output);
        self.boat_push_and_pick_up(&mut boat, output);
        if boat.removed || self.find_entity(key).is_none() {
            return;
        }
        self.write_boat(&entity, boat, output);
    }

    /// `Leashable.tickLeash(level, boat)` of the boat in its tick, in the chunk
    /// `chunk`: a saved leash looked for (`restoreLeashFromSave`); then, where it holds,
    /// its holder gone or out of reach drops it with a lead; beyond the snap distance it
    /// breaks with the sound of it; and otherwise it pulls; the boat turns by its swing
    /// either way, which then slows by the friction of what it stands on, of the liquid
    /// it is in, or of the air. The swing is kept in the record in place, as counters are,
    /// and committed with the boat's other fields.
    fn boat_tick_leash(&mut self, boat: &mut Tick, chunk: ChunkPos, output: &mut TickOutput) {
        let key = boat.key;
        let Some(leash) = self
            .find_entity(key)
            .and_then(|(_, record)| record.riding.leash.as_deref().copied())
        else {
            return;
        };
        if !boat.memory.leash.found {
            boat.memory.leash.ticks = boat.memory.leash.ticks.saturating_add(1);
            let ticks = boat.memory.leash.ticks;
            if !self.restore_leash(key, leash.holder, ticks, output) {
                return;
            }
            boat.memory.leash.found = true;
            // Leashed, it got off what it rode.
            let Some((_, record)) = self.find_entity(key) else {
                boat.removed = true;
                return;
            };
            boat.body.position = record.position;
        }
        let Some(held) = self.leash_holder(leash.holder) else {
            self.drop_leash(key, true, output);
            return;
        };
        let info = EntityTypeInfo::of(i32::from(boat.kind));
        let movement = match self.find_entity(key) {
            Some((_, record)) if self.is_driven(key) => {
                let mut now = record.clone();
                now.motion = boat.body.motion;
                self.record_movement(&now)
            }
            _ => boat.body.motion,
        };
        let leashee = leashes::End {
            position: boat.body.position,
            yaw: boat.yaw,
            width: info.width,
            height: info.height,
            movement,
        };
        let friction = self.angular_friction(boat, chunk);
        let mut momentum = leash.angular_momentum.0;
        // A boat is held by four (`supportQuadLeash`) where its holder holds by four, and
        // tells such a holder so (`notifyLeashHolder`).
        self.notify_leash_holder(held.key, true);
        let spring = self.spring_of(boat.kind, true, held.key);
        let snapped = leashes::leash_tick_with(
            &leashee,
            &held.end,
            &mut boat.body.motion,
            &mut boat.body.fall_distance,
            &mut boat.yaw,
            &mut momentum,
            friction,
            spring,
        )
        .snapped;
        if snapped {
            let [x, y, z] = held.end.position;
            output.events.push(RegionEvent::Sound {
                sound: sounds::ITEM_LEAD_BREAK,
                source: SOUND_SOURCE_NEUTRAL,
                position: clustine_world::Vec3::new(x, y, z),
                volume: 1.0,
                pitch: 1.0,
                except: None,
            });
            self.drop_leash(key, true, output);
            return;
        }
        if let Some((chunk, record)) = self.find_entity(key) {
            let mut riding = record.riding.clone();
            if let Some(kept) = riding.leash.as_deref_mut() {
                kept.angular_momentum = Double(momentum);
            }
            self.age_entity(chunk, key, vec![EntityField::Riding(riding)], output);
        }
    }

    /// `Leashable.angularFriction(boat)`: on the ground the friction of the block below
    /// that affects its movement, times 0.91; in a liquid 0.8; in the air 0.91.
    fn angular_friction(&self, boat: &Tick, chunk: ChunkPos) -> f32 {
        if boat.body.on_ground {
            let view = View {
                region: self,
                home: chunk,
            };
            let below = view.state(boat.body.below_for_movement(&view));
            return below.physics().map_or(0.6, |physics| physics.friction) * 0.91;
        }
        if boat.body.in_water || boat.in_lava {
            0.8
        } else {
            0.91
        }
    }

    /// `floatBoat()` and `move(SELF, motion)` of a boat nobody drives.
    fn float_and_move(
        &mut self,
        boat: &mut Tick,
        chunk: ChunkPos,
        found: boat::Found,
        old_status: Option<Status>,
    ) {
        let passenger = self
            .find_entity(boat.key)
            .is_some_and(|(_, record)| record.riding.vehicle.is_some());
        let view = View {
            region: self,
            home: chunk,
        };
        let mut floating = Floating {
            status: found.status,
            old_status,
            water_level: found.water_level,
            land_friction: found.land_friction.unwrap_or(0.0),
            turning: boat.parts.turning,
            last_yd: boat.memory.last_yd,
        };
        let mover = boat.body.mover;
        boat::float(&view, &mover, &mut boat.body, &mut floating);
        boat.parts.turning = floating.turning;
        boat.parts.status[0] = floating.status as u8;
        let vector = boat.body.motion;
        let mut root = Root {
            body: boat.body.clone(),
            yaw: boat.yaw,
            pitch: boat.pitch,
            gravity: boat::GRAVITY,
            bounce: boat_bounce(),
            passenger,
            last_yd: floating.last_yd,
            living: false,
            flying: false,
        };
        vehicle_move::move_boat(&view, &mut root, vector);
        boat.body = root.body;
        boat.memory.last_yd = root.last_yd;
    }

    /// `applyEffectsFromBlocks()` of the boat over `movements`: what it stands on, then
    /// each block the walk meets, as a cart's are applied (the module's named
    /// differences).
    fn boat_effects_from_blocks(
        &mut self,
        boat: &mut Tick,
        chunk: ChunkPos,
        movements: &[Movement],
        output: &mut TickOutput,
    ) {
        if boat.removed {
            return;
        }
        self.sync_boat(boat);
        let view = View {
            region: self,
            home: chunk,
        };
        let standing = boat.body.on_ground.then(|| {
            let on = boat.body.on_pos_legacy(&view);
            (on, view.state(on))
        });
        let mut inside = BoatInside {
            body: &boat.body,
            met: Vec::new(),
        };
        check_inside_blocks(&view, &mut inside, movements);
        let met = inside.met;
        if let Some((on, state)) = standing {
            // `Block.stepOn`.
            if is_class(state, BlockClass::SlimeBlock) {
                let up = boat.body.motion[1].abs();
                if up < 0.1 {
                    let factor = 0.4 + up * 0.2;
                    let [x, y, z] = boat.body.motion;
                    boat.body.motion = [x * factor, y * 1.0, z * factor];
                }
            } else {
                // Redstone ore lights, and a sculk sensor hears it (`vibrations.rs`).
                let source = super::vibrations::Source::of(boat.key);
                self.stood_on(on, source, boat.body.position, output);
            }
        }
        let mut fire_hurts = Vec::new();
        let mut lava = 0;
        let mut extinguish = false;
        let toucher = super::pressed::Toucher {
            key: boat.key,
            bounds: boat.body.bounds(),
            living: false,
        };
        for (state, pos, block, fluid) in met {
            if block {
                if crate::physics::item::handed_back(state) {
                    // A plate, tripwire or a detector rail, which counts the boat where
                    // it is (`pressed.rs`).
                    self.entity_touched(pos, &toucher, output);
                } else if is_class(state, BlockClass::BubbleColumnBlock) {
                    self.boat_in_bubble_column(boat, state, pos);
                } else if is_class(state, BlockClass::BaseFireBlock) {
                    let damage = if is_class(state, BlockClass::SoulFireBlock) {
                        2.0
                    } else {
                        1.0
                    };
                    fire_hurts.push(damage);
                } else if is_class(state, BlockClass::CactusBlock) {
                    self.boat_hurt(boat, 1.0, output);
                } else if is_class(state, BlockClass::WebBlock) {
                    boat.body.fall_distance = 0.0;
                    boat.body.stuck_speed = [0.25, f64::from(0.05_f32), 0.25];
                } else if is_class(state, BlockClass::PowderSnowBlock) {
                    let side = f64::from(0.9_f32);
                    boat.body.fall_distance = 0.0;
                    boat.body.stuck_speed = [side, 1.5, side];
                    extinguish = true;
                } else if is_class(state, BlockClass::LayeredCauldronBlock) {
                    extinguish = true;
                } else if is_class(state, BlockClass::LavaCauldronBlock) {
                    lava += 1;
                }
            }
            if fluid {
                if has_water(state) {
                    extinguish = true;
                } else if has_lava(state) {
                    lava += 1;
                }
            }
            if boat.removed {
                return;
            }
        }
        // `FIRE_IGNITE`, its hurts, then `LAVA_IGNITE`, its hurts, then `EXTINGUISH`.
        if !fire_hurts.is_empty() {
            if boat.fire_ticks < 0 {
                boat.fire_ticks += 1;
            }
            if boat.fire_ticks >= 0 {
                boat.ignite_for_ticks(160);
            }
            for damage in fire_hurts {
                self.boat_hurt(boat, damage, output);
                if boat.removed {
                    return;
                }
            }
        }
        if lava > 0 {
            boat.ignite_for_ticks(300);
            for _ in 0..lava {
                let position = boat.body.position;
                self.boat_hurt(boat, 4.0, output);
                // `lavaHurt`'s sound of burning, its pitch from the boat's own random.
                let [drawn] = self.boat_floats::<1>(boat.key);
                output.events.push(RegionEvent::Sound {
                    sound: sounds::ENTITY_GENERIC_BURN,
                    source: SOUND_SOURCE_NEUTRAL,
                    position: clustine_world::Vec3::new(position[0], position[1], position[2]),
                    volume: 0.4,
                    pitch: 2.0 + drawn * 0.4,
                    except: None,
                });
                if boat.removed {
                    return;
                }
            }
        }
        if extinguish {
            boat.fire_ticks = boat.fire_ticks.min(0);
        }
    }

    /// `BubbleColumnBlock.entityInside` for a boat whose box meets the column at `pos`:
    /// with nothing solid and no fluid above it, `AbstractBoat.onAboveBubbleColumn`
    /// (the bubble time set, and the draws of a splash the server does not show);
    /// otherwise `Entity.onInsideBubbleColumn`, a push up or down for a boat nobody
    /// drives.
    fn boat_in_bubble_column(&mut self, boat: &mut Tick, state: BlockState, pos: BlockPos) {
        let down = flag(state, "drag");
        let above = BlockPos::new(pos.x, pos.y + 1, pos.z);
        let over = self.block(above).unwrap_or(clustine_data::blocks::AIR);
        let open = over.voxel_collision_shape().is_empty() && over.props().fluid().is_none();
        if open {
            boat.above_bubble_column = true;
            boat.bubble_down = down;
            if boat.parts.bubble_time == 0 {
                boat.parts.bubble_time = BUBBLE_TIME;
            }
            if !Status::is_under_water(boat.status()) {
                // `random.nextInt(100) == 0`, then the sound's pitch and the splash's x
                // and z: all drawn, nothing shown by a server.
                let [chance] = self.boat_ints::<1>(boat.key, 100);
                if chance == 0 {
                    self.boat_floats::<3>(boat.key);
                }
            }
            return;
        }
        if self.is_driven(boat.key) {
            return;
        }
        let [x, y, z] = boat.body.motion;
        let y = if down {
            (y - 0.03).max(-0.3)
        } else {
            (y + 0.06).min(0.7)
        };
        boat.body.motion = [x, y, z];
        boat.body.fall_distance = 0.0;
    }

    /// `tickBubbleColumn()` on the server: a boat away from a column has its time
    /// cleared; one above one counts down, and at the end of the count a downward
    /// column lets its passengers off and pulls it down, an upward one throws it up,
    /// where nobody drives it.
    fn tick_bubble_column(&mut self, boat: &mut Tick, output: &mut TickOutput) {
        if !boat.above_bubble_column {
            boat.parts.bubble_time = 0;
        }
        let mut time = boat.parts.bubble_time;
        if time <= 0 {
            return;
        }
        time -= 1;
        boat.parts.bubble_time = time;
        if BUBBLE_TIME - time - 1 > 0 && time == 0 {
            boat.parts.bubble_time = 0;
            if boat.bubble_down {
                self.sync_boat(boat);
                self.eject_passengers(boat.key, output);
                if self.boat_chunk(boat).is_none() {
                    return;
                }
            }
            // The push of a driven boat is its driver's client's to make, told by these.
            if let Some(chunk) = self.boat_chunk(boat) {
                output.events.push(RegionEvent::EntityEvent {
                    entity: boat.key,
                    chunk,
                    event: if boat.bubble_down { 71 } else { 72 },
                });
            }
            // `handleBubbleColumnEffect`, where the boat is the server's to move.
            if !self.is_driven(boat.key) {
                let [x, y, z] = boat.body.motion;
                boat.body.motion = if boat.bubble_down {
                    [x + 0.0, y + -0.7, z + 0.0]
                } else {
                    [x, 0.6, z]
                };
            }
        }
        boat.above_bubble_column = false;
    }

    /// The paddles' part of the tick: a paddle a driver turns moves on by an eighth of
    /// a stroke, playing its sound near the start of each stroke on water or land
    /// (`getPaddleSound`, from the status worked out again); one not turned rests at the
    /// start.
    fn paddle_sounds(&mut self, boat: &mut Tick, chunk: ChunkPos, output: &mut TickOutput) {
        let driven = self.is_driven(boat.key);
        for paddle in 0..2 {
            if !(boat.parts.paddles[paddle] && driven) {
                boat.memory.paddle_positions[paddle] = 0.0;
                continue;
            }
            let at = boat.memory.paddle_positions[paddle];
            let in_window = f64::from(at % STROKE) <= PADDLE_WINDOW
                && f64::from((at + PADDLE_STEP) % STROKE) >= PADDLE_WINDOW;
            if in_window {
                let status = {
                    let view = View {
                        region: self,
                        home: chunk,
                    };
                    boat::status(&view, &boat.body.bounds()).status
                };
                let sound = match status {
                    Status::InWater | Status::UnderWater | Status::UnderFlowingWater => {
                        Some(sounds::ENTITY_BOAT_PADDLE_WATER)
                    }
                    Status::OnLand => Some(sounds::ENTITY_BOAT_PADDLE_LAND),
                    Status::InAir => None,
                };
                if let Some(sound) = sound {
                    let view = view_vector(boat.yaw, boat.pitch);
                    let (dx, dz) = if paddle == 1 {
                        (-view[2], view[0])
                    } else {
                        (view[2], -view[0])
                    };
                    let [drawn] = self.boat_floats::<1>(boat.key);
                    let [x, y, z] = boat.body.position;
                    output.events.push(RegionEvent::Sound {
                        sound,
                        source: SOUND_SOURCE_NEUTRAL,
                        position: clustine_world::Vec3::new(x + dx, y, z + dz),
                        volume: 1.0,
                        pitch: 0.8 + 0.4 * drawn,
                        except: None,
                    });
                }
            }
            boat.memory.paddle_positions[paddle] = at + PADDLE_STEP;
        }
    }

    /// The pushing and picking up of the boat's tick: what is pushable in its box grown
    /// by a fifth sideways and shrunk by a hundredth up and down, other than what it
    /// rides, is made to ride it where nobody drives it, it has room, the other rides
    /// nothing, is narrower, is living and not of `#cannot_be_pushed_onto_boats` (none
    /// is before N7); anything else is pushed. Players the region knows come last.
    fn boat_push_and_pick_up(&mut self, boat: &mut Tick, output: &mut TickOutput) {
        self.sync_boat(boat);
        let reach = boat.body.bounds().inflate([
            f64::from(0.2_f32),
            f64::from(-0.01_f32),
            f64::from(0.2_f32),
        ]);
        let met: Vec<(EntityId, ChunkPos)> = self
            .entities_in(&reach)
            .into_iter()
            .filter(|(key, at)| *key != boat.key && self.ticks_chunk(*at))
            .collect();
        let picking_up = !self.is_driven(boat.key);
        for (key, at) in met {
            let Some(other) = self.entity_record(key, at).cloned() else {
                continue;
            };
            // `EntitySelector.pushableBy`: no spectators, no teams.
            if !is_pushable(other.kind) {
                continue;
            }
            if other
                .riding
                .passengers
                .iter()
                .any(|seat| seat.key == boat.key)
            {
                continue;
            }
            let room = self.find_entity(boat.key).is_some_and(|(_, record)| {
                record.riding.passengers.len() < max_passengers(boat.kind)
            });
            let narrower = EntityTypeInfo::of(i32::from(other.kind)).width
                < EntityTypeInfo::of(i32::from(boat.kind)).width;
            if picking_up
                && room
                && other.riding.vehicle.is_none()
                && narrower
                && living_pickable(&other)
            {
                self.sync_boat(boat);
                self.start_riding(key, boat.key, false, output);
            } else {
                self.boat_pushes_entity(boat, &other, at, output);
            }
        }
        let players: Vec<Vec3> = self
            .players
            .values()
            .filter(|player| player.riding.is_none())
            .map(|player| {
                let feet = player.pose.position;
                [feet.x, feet.y, feet.z]
            })
            .filter(|feet| Aabb::of_entity(*feet, 0.6, 1.8).intersects(reach))
            .collect();
        for feet in players {
            let theirs = Aabb::of_entity(feet, 0.6, 1.8);
            if !pushes(&boat.body.bounds(), &theirs, false) || self.boat_carries(boat.key) {
                continue;
            }
            if let Some((back, _)) = entity_push(boat.body.position, feet) {
                boat.body.motion = physics::add(boat.body.motion, back);
                boat.needs_sync = true;
            }
        }
    }

    /// `AbstractBoat.push(other)` in the boat's own tick: where it pushes at all and the
    /// two are not of one unit, `Entity.push`: each that carries nobody and is pushable
    /// gets a twentieth of the way apart, the boat in its tick, the other in its record.
    fn boat_pushes_entity(
        &mut self,
        boat: &mut Tick,
        other: &Entity,
        at: ChunkPos,
        output: &mut TickOutput,
    ) {
        let theirs = self.entity_box(other);
        if !pushes(&boat.body.bounds(), &theirs, is_boat(other.kind)) {
            return;
        }
        if self.root_of(other.key) == self.root_of(boat.key) {
            return;
        }
        let Some((to_boat, to_other)) = entity_push(boat.body.position, other.position) else {
            return;
        };
        if !self.boat_carries(boat.key) {
            boat.body.motion = physics::add(boat.body.motion, to_boat);
            boat.needs_sync = true;
        }
        if other.riding.passengers.is_empty() && is_pushable(other.kind) {
            self.nudge(other, at, to_other, output);
        }
    }

    /// `push(x, y, z)` of the entity `record` of the chunk `at`: its motion grown by
    /// `by`, committed, and sent at once (`needsSync`).
    pub(crate) fn nudge(
        &mut self,
        record: &Entity,
        at: ChunkPos,
        by: Vec3,
        output: &mut TickOutput,
    ) {
        if !by.iter().all(|component| component.is_finite()) {
            return;
        }
        let motion = physics::add(record.motion, by);
        self.commit_entity(at, record.key, vec![EntityField::Motion(motion)], output);
        let mut after = record.clone();
        after.motion = motion;
        let mut report = tracked(&after, Tracked::NEEDS_SYNC, false);
        report.chunk = at;
        output.events.push(RegionEvent::EntityTracked(report));
    }

    /// `AbstractBoat.push(cart)` of the boat `boat` of the chunk `at` by the cart
    /// `cart`, of box `cart_bounds`, met in the cart's tick
    /// (`OldMinecartBehavior.pushAndPickupEntities`): where the cart's bottom is no
    /// higher than the boat's and they are not of one unit, the boat is pushed away
    /// where it carries nobody, and the push the cart is to be given back is returned
    /// where the cart carries nobody.
    pub(crate) fn boat_pushed_by_cart(
        &mut self,
        boat: &Entity,
        at: ChunkPos,
        cart: EntityId,
        cart_position: Vec3,
        cart_bounds: &Aabb,
        output: &mut TickOutput,
    ) -> Option<Vec3> {
        if !pushes(&self.entity_box(boat), cart_bounds, false) {
            return None;
        }
        if self.root_of(boat.key) == self.root_of(cart) {
            return None;
        }
        let (to_boat, to_cart) = entity_push(boat.position, cart_position)?;
        if boat.riding.passengers.is_empty() {
            self.nudge(boat, at, to_boat, output);
        }
        let cart_carries = self
            .find_entity(cart)
            .is_some_and(|(_, record)| !record.riding.passengers.is_empty());
        (!cart_carries).then_some(to_cart)
    }

    /// The players' push of the boats their boxes meet (`LivingEntity.pushEntities` in
    /// the player's own tick, after the carts': `AbstractBoat.push(player)`), in the
    /// order of the players. What the boat gives the player back is the client's.
    pub(crate) fn players_push_boats(&mut self, output: &mut TickOutput) {
        let players: Vec<Vec3> = self
            .players
            .values()
            .filter(|player| player.riding.is_none())
            .map(|player| {
                let feet = player.pose.position;
                [feet.x, feet.y, feet.z]
            })
            .collect();
        for feet in players {
            let theirs = Aabb::of_entity(feet, 0.6, 1.8);
            for (key, at) in self.entities_in(&theirs) {
                if !self.ticks_chunk(at) {
                    continue;
                }
                let Some(record) = self.entity_record(key, at).cloned() else {
                    continue;
                };
                if !is_boat(record.kind) || !record.riding.passengers.is_empty() {
                    continue;
                }
                if !pushes(&self.entity_box(&record), &theirs, false) {
                    continue;
                }
                if let Some((to_boat, _)) = entity_push(record.position, feet) {
                    self.nudge(&record, at, to_boat, output);
                }
            }
        }
    }

    /// `hurtServer` of the boat by what is not a player (fire, a cactus, lava): its
    /// wobble and damage, broken above 40, dropping its item.
    fn boat_hurt(&mut self, boat: &mut Tick, amount: f32, output: &mut TickOutput) {
        self.sync_boat(boat);
        self.hurt_vehicle(boat.key, amount, false, output);
        if self.find_entity(boat.key).is_none() {
            boat.removed = true;
            return;
        }
        boat.sync_velocity = true;
    }

    /// `N` floats from the boat's own random, on from what it drew in the tick.
    fn boat_floats<const N: usize>(&mut self, key: EntityId) -> [f32; N] {
        let mut random = self.boat_random(key, N as u64);
        std::array::from_fn(|_| random.next_float())
    }

    /// `N` of `nextInt(bound)` from the boat's own random.
    fn boat_ints<const N: usize>(&mut self, key: EntityId, bound: u32) -> [u32; N] {
        let mut random = self.boat_random(key, N as u64);
        std::array::from_fn(|_| random.next_int(bound))
    }

    /// The boat's stream in this tick, on from what it drew, counting `taking` more.
    fn boat_random(&mut self, key: EntityId, taking: u64) -> Random {
        let random = self.entity_stream(key).random;
        self.entity_drew(key, taking);
        random
    }

    /// What the boat's tick leaves: its record, its commit, its memory, and the edges'
    /// report.
    fn write_boat(&mut self, before: &Entity, boat: Tick, output: &mut TickOutput) {
        let key = before.key;
        let Some((chunk, current)) = self.find_entity(key) else {
            return;
        };
        let current = current.clone();
        let data = with_parts(&current.data, boat.parts);
        let mut memory = self.entity_memory.remove(&key).unwrap_or_default();
        memory.supporting = boat.body.supporting;
        memory.on_ground_no_blocks = boat.body.on_ground_no_blocks;
        memory.stuck_speed = boat.body.stuck_speed;
        memory.boat = BoatMemory {
            recorded: Vec::new(),
            vertical_collision_below: boat.body.vertical_collision_below,
            ..boat.memory
        };
        self.entity_memory.insert(key, memory);
        let mut after = Entity {
            position: boat.body.position,
            motion: boat.body.motion,
            yaw: boat.yaw,
            pitch: boat.pitch,
            on_ground: boat.body.on_ground,
            fall_distance: boat.body.fall_distance,
            fire_ticks: boat.fire_ticks,
            data,
            ..current
        };
        let to = match self.relocate(chunk, before.position, &mut after, output) {
            Relocated::Sent => return,
            Relocated::Moved(to) => to,
            Relocated::Stays => chunk,
        };
        let bits = |values: [f64; 3]| values.map(f64::to_bits);
        let moved = bits(after.position) != bits(before.position);
        let rotated = after.yaw.to_bits() != before.yaw.to_bits()
            || after.pitch.to_bits() != before.pitch.to_bits();
        let motion = bits(after.motion) != bits(before.motion);
        let state_changed = committed_parts(&after.data) != committed_parts(&before.data);
        let shown_changed = shown_parts(&after.data) != shown_parts(&before.data);
        let fire = after.fire_ticks != before.fire_ticks;
        let committed = moved
            || rotated
            || motion
            || after.on_ground != before.on_ground
            || state_changed
            || fire;
        if to == chunk {
            if committed {
                let mut fields = Vec::new();
                if moved {
                    fields.push(EntityField::Position(after.position));
                }
                if motion {
                    fields.push(EntityField::Motion(after.motion));
                }
                if rotated {
                    fields.push(EntityField::Rotation {
                        yaw: after.yaw,
                        pitch: after.pitch,
                    });
                }
                if after.on_ground != before.on_ground {
                    fields.push(EntityField::OnGround(after.on_ground));
                }
                if after.fall_distance.to_bits() != before.fall_distance.to_bits() {
                    fields.push(EntityField::FallDistance(after.fall_distance));
                }
                if fire {
                    fields.push(EntityField::FireTicks(after.fire_ticks));
                }
                if state_changed {
                    fields.push(EntityField::Data(after.data.clone()));
                }
                // A leash's swing, kept in place as the tick went, goes with the rest.
                if after.riding != before.riding {
                    fields.push(EntityField::Riding(after.riding.clone()));
                }
                output
                    .entity_changes
                    .push((chunk, EntityChange::Set(key, fields)));
            }
            if let Some(loaded) = self.chunks.get_mut(&chunk) {
                loaded.apply_entity(&EntityChange::Whole(after.clone()));
            }
        }
        output.entities_ageing.push(to);
        let mut flags = 0;
        if boat.body.horizontal_collision {
            flags |= Tracked::HORIZONTAL_COLLISION;
        }
        if boat.body.vertical_collision {
            flags |= Tracked::VERTICAL_COLLISION;
        }
        if boat.needs_sync {
            flags |= Tracked::NEEDS_SYNC;
        }
        if boat.body.sync_position {
            flags |= Tracked::SYNC_POSITION;
        }
        if boat.sync_velocity {
            flags |= Tracked::SYNC_VELOCITY;
        }
        if after.fire_ticks > 0 {
            flags |= Tracked::ON_FIRE;
        }
        let synced = Tracked::NEEDS_SYNC | Tracked::SYNC_POSITION | Tracked::SYNC_VELOCITY;
        if committed || to != chunk || flags & synced != 0 || shown_changed {
            let mut report = tracked(&after, flags, shown_changed || state_changed);
            report.chunk = to;
            output.events.push(RegionEvent::EntityTracked(report));
        }
    }

    /// A use of a boat by a player of this region or another: a hit hurts it as
    /// `VehicleEntity` is hurt (a creative player's breaks it at once, a chest boat
    /// spilling what it holds); a click of a player who does not sneak seats them while
    /// it has not been under water sixty ticks (`AbstractBoat.interact`), a chest
    /// boat's too (the plan, section 7 item 11), whose chest opens instead to a player
    /// who sneaks or finds no room (`menu_play.rs`).
    /// Riding a boat from across a line is a mount, the riders' step's (N5h).
    pub(crate) fn boat_used(
        &mut self,
        entity: &Entity,
        position: ChunkPos,
        by: EntityUse,
        output: &mut TickOutput,
    ) {
        let _ = position;
        match by {
            EntityUse::Attack {
                creative, damage, ..
            } => {
                self.hurt_vehicle(entity.key, damage.0, creative, output);
            }
            EntityUse::Interact {
                player, sneaking, ..
            } => {
                // `Entity.interact`'s leash comes first (`VehicleEntity` keeps it).
                if self.leash_used(entity, by, output) {
                    return;
                }
                let out_of_control =
                    parts_of(&entity.data).map_or(0.0, |parts| parts.out_of_control);
                // As it was before anyone got on by this use.
                let full = !self.can_add_passenger(entity, position);
                if !sneaking && out_of_control < OUT_OF_CONTROL {
                    self.mount_player(player, entity.key, output);
                }
                // `ChestBoat.interact`: its chest opens to a player who sneaks, or who
                // could not get on for want of room (`interactWithContainerVehicle`).
                if super::menu_play::is_container_vehicle(entity.kind)
                    && (sneaking || full)
                    && let Some(id) = self.player_with_entity(player)
                {
                    self.use_container_vehicle(entity.key, id, output);
                }
            }
            // An explosion's hit across a line is step N6g's (`docs/groundwork/n6-plan.md`).
            // Only animals breed.
            EntityUse::Blasted { .. }
            | EntityUse::BreedAsk { .. }
            | EntityUse::MobHit { .. }
            | EntityUse::BreedAnswer { .. } => {}
        }
    }

    /// `handlePaddleBoat`: the paddles of the boat the player drives are as their client
    /// says, committed and shown where they changed. Nothing is answered.
    pub(crate) fn paddle_boat(
        &mut self,
        id: PlayerId,
        left: bool,
        right: bool,
        output: &mut TickOutput,
    ) {
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let Some(riding) = player.riding else {
            return;
        };
        let entity = player.entity_id;
        let Some((chunk, vehicle)) = self.find_entity(riding.vehicle) else {
            return;
        };
        if !drives(vehicle, entity) {
            return;
        }
        let Some(mut parts) = parts_of(&vehicle.data) else {
            return;
        };
        if parts.paddles == [left, right] {
            return;
        }
        parts.paddles = [left, right];
        let mut after = vehicle.clone();
        after.data = with_parts(&vehicle.data, parts);
        self.commit_entity(
            chunk,
            after.key,
            vec![EntityField::Data(after.data.clone())],
            output,
        );
        let mut report = tracked(&after, 0, true);
        report.chunk = chunk;
        output.events.push(RegionEvent::EntityTracked(report));
    }

    /// `handleMoveVehicle` of a move the player's client made of their root: checked as
    /// the game checks it where they drive it, and where they ride it without driving,
    /// put back now and then (`physics::vehicle_move`). A move that is not a number puts
    /// the player out; one from a player who rides nothing is passed over.
    pub(crate) fn move_vehicle(
        &mut self,
        id: PlayerId,
        moved: VehicleMove,
        output: &mut TickOutput,
    ) {
        self.move_vehicle_tested(id, moved, true, output);
    }

    /// [`Region::move_vehicle`], with the "too quickly" test made only if `test_speed`:
    /// a move kept for a player who waited, after others were let go, is not.
    pub(crate) fn move_vehicle_tested(
        &mut self,
        id: PlayerId,
        moved: VehicleMove,
        test_speed: bool,
        output: &mut TickOutput,
    ) {
        if vehicle_move::invalid(&moved) {
            output.player_events.push((
                id,
                PlayerEvent::Kicked {
                    reason: INVALID_VEHICLE_MOVEMENT.to_owned(),
                },
            ));
            return;
        }
        let Some(player) = self.players.get(&id) else {
            return;
        };
        let Some(riding) = player.riding else {
            return;
        };
        let entity = player.entity_id;
        let root_key = self.root_of(riding.vehicle);
        let Some((chunk, root)) = self.find_entity(root_key) else {
            return;
        };
        let root = root.clone();
        if !self.drives_root(&root, entity) {
            let (check, correction) = vehicle_move::passenger_move(
                &riding.check,
                root.position,
                root.yaw,
                root.pitch,
                &moved,
            );
            self.set_check(id, check);
            if let Some((position, yaw, pitch)) = correction {
                corrected(id, position, yaw, pitch, output);
            }
            return;
        }
        let Some(Checked {
            verdict,
            root: checked,
            check,
            mut correction,
        }) = self.check_root_move(&root, chunk, &riding, &moved, test_speed)
        else {
            return;
        };
        self.set_check(id, check);
        if matches!(verdict, Verdict::Ignored | Verdict::TooQuickly) {
            if let Some((position, yaw, pitch)) = correction {
                corrected(id, position, yaw, pitch, output);
            }
            return;
        }
        // `handlePlayerKnownMovement`: what the move moved the root by is the
        // driver's known movement, which a leash's pull reads.
        self.known_movements
            .insert(id, physics::sub(checked.body.position, root.position));
        let mut memory = self.entity_memory.remove(&root.key).unwrap_or_default();
        memory.supporting = checked.body.supporting;
        memory.on_ground_no_blocks = checked.body.on_ground_no_blocks;
        memory.stuck_speed = checked.body.stuck_speed;
        memory.boat.recorded = checked.body.movements.clone();
        memory.boat.vertical_collision_below = checked.body.vertical_collision_below;
        memory.boat.last_yd = checked.last_yd;
        self.entity_memory.insert(root.key, memory);
        let fell = checked.body.fell_on;
        let mut after = Entity {
            position: checked.body.position,
            motion: checked.body.motion,
            yaw: checked.yaw,
            pitch: checked.pitch,
            on_ground: checked.body.on_ground,
            fall_distance: checked.body.fall_distance,
            ..root.clone()
        };
        let to = match self.relocate(chunk, root.position, &mut after, output) {
            Relocated::Sent => return,
            Relocated::Moved(to) => to,
            Relocated::Stays => chunk,
        };
        if verdict == Verdict::Accepted
            && after.position.map(f64::to_bits) != checked.body.position.map(f64::to_bits)
        {
            // Held back at a line until the riders' step hands riders over: the client
            // is told where the boat is.
            correction = Some((after.position, after.yaw, after.pitch));
        }
        if to == chunk {
            self.commit_moved_root(&root, &after, chunk, output);
        }
        // A living root's landing hurts it as its own would (`causeFallDamage`), after
        // its fall is passed on to its riders (`propagateFallToPassengers`), which hurts
        // none of Clustine's players: they are all creative.
        if !is_boat(root.kind)
            && let Some((distance, on)) = fell
        {
            self.living_root_landed(after.key, distance, on, output);
        }
        let mut report = tracked(&after, 0, false);
        report.chunk = to;
        output.events.push(RegionEvent::EntityTracked(report));
        if let Some((position, yaw, pitch)) = correction {
            corrected(id, position, yaw, pitch, output);
        }
    }

    /// `LivingEntity.causeFallDamage` of the region's mob `key`, which a move of its
    /// driver's ended a fall of `distance` on the block at `on` with: its sounds, and its
    /// hurt between its ticks, of which it may die.
    fn living_root_landed(
        &mut self,
        key: EntityId,
        distance: f64,
        on: BlockPos,
        output: &mut TickOutput,
    ) {
        let Some((chunk, record)) = self.find_entity(key) else {
            return;
        };
        let record = record.clone();
        let info = EntityTypeInfo::of(i32::from(record.kind));
        let (width, height, eye) = crate::mobs::types::own_size(&record)
            .map_or((info.width, info.height, info.eye_height), |size| {
                (size.width, size.height, size.eye_height)
            });
        let body = self.living_body(&record, width, height, chunk);
        let kind = crate::living::Kind::Mob {
            kind: record.kind,
            hurts: Vec::new(),
        };
        let mut living = crate::living::Living::new(body, kind, eye);
        // Its own safe fall and multiplier: a horse's 6 and a half.
        if let Ok(mob) = Mob::decode(&record.data) {
            living.attributes =
                crate::mobs::body::living_attributes(record.kind, &mob.body.attributes).0;
            living.landing = crate::mobs::body::landing(record.kind, &mob);
            living.fall_damage_immune = clustine_blocks::data::entity_in_tag(
                i32::from(record.kind),
                "minecraft:fall_damage_immune",
            );
        }
        let view = View {
            region: self,
            home: chunk,
        };
        living.landed(&view, distance, on);
        let hurts = match &mut living.kind {
            crate::living::Kind::Mob { hurts, .. } => std::mem::take(hurts),
            crate::living::Kind::ArmorStand { .. } => Vec::new(),
        };
        self.living_aftermath(key, &mut living, output);
        for (damage, amount) in hurts {
            let source = crate::mobs::body::hurt::DamageSource::of(
                crate::mobs::body::hurt::DamageType::from(damage),
            );
            self.hurt_mob(key, &source, amount, output);
        }
    }

    /// The check of one move of the living root `root` (a ridden pig) by the player who
    /// drives it: its body as its tick would begin, its gravity, and its step raised to a
    /// block while a player drives it (`LivingEntity.maxUpStep`).
    fn check_living_root_move(
        &self,
        root: &Entity,
        chunk: ChunkPos,
        riding: &PlayerRiding,
        moved: &VehicleMove,
        test_speed: bool,
    ) -> Option<Checked> {
        let info = EntityTypeInfo::of(i32::from(root.kind));
        let mob = Mob::decode(&root.data).ok()?;
        let (width, height) = crate::mobs::types::own_size(root)
            .map_or((info.width, info.height), |size| (size.width, size.height));
        let mut body = self.living_body(root, width, height, chunk);
        body.movements = self
            .entity_memory
            .get(&root.key)
            .map_or_else(Vec::new, |memory| memory.boat.recorded.clone());
        // Its own attributes: a horse's step and gravity are its type's.
        let (attributes, step) =
            crate::mobs::body::living_attributes(root.kind, &mob.body.attributes);
        let bounce = Bounce {
            gravity: attributes.gravity,
            air_drag: 0.98,
            own: attributes.bounciness,
            living: true,
            rails_keep_speed: false,
            max_up_step: step.max(1.0),
        };
        let view = View {
            region: self,
            home: chunk,
        };
        let root = Root {
            body,
            yaw: root.yaw,
            pitch: root.pitch,
            gravity: attributes.gravity,
            bounce,
            passenger: root.riding.vehicle.is_some(),
            last_yd: 0.0,
            living: true,
            flying: crate::mobs::types::of(root.kind).flying_vehicle(&mob),
        };
        Some(vehicle_move::check_move(
            &view,
            &riding.check,
            root,
            moved,
            false,
            test_speed,
        ))
    }

    /// The check of one move of the root `root` of the chunk `chunk` by the player whose
    /// riding is `riding`, as `physics::vehicle_move` makes it.
    fn check_root_move(
        &self,
        root: &Entity,
        chunk: ChunkPos,
        riding: &PlayerRiding,
        moved: &VehicleMove,
        test_speed: bool,
    ) -> Option<Checked> {
        let view = View {
            region: self,
            home: chunk,
        };
        if !is_boat(root.kind) {
            return self.check_living_root_move(root, chunk, riding, moved, test_speed);
        }
        let tick = self.boat_tick_of(root, chunk)?;
        let root = Root {
            body: tick.body,
            yaw: tick.yaw,
            pitch: tick.pitch,
            gravity: boat::GRAVITY,
            bounce: boat_bounce(),
            passenger: root.riding.vehicle.is_some(),
            last_yd: tick.memory.last_yd,
            living: false,
            flying: false,
        };
        Some(vehicle_move::check_move(
            &view,
            &riding.check,
            root,
            moved,
            false,
            test_speed,
        ))
    }

    /// Commits what a driver's move did to the root `before`, now `after`, of `chunk`.
    fn commit_moved_root(
        &mut self,
        before: &Entity,
        after: &Entity,
        chunk: ChunkPos,
        output: &mut TickOutput,
    ) {
        let bits = |values: [f64; 3]| values.map(f64::to_bits);
        let mut fields = Vec::new();
        if bits(after.position) != bits(before.position) {
            fields.push(EntityField::Position(after.position));
        }
        if bits(after.motion) != bits(before.motion) {
            fields.push(EntityField::Motion(after.motion));
        }
        if after.yaw.to_bits() != before.yaw.to_bits()
            || after.pitch.to_bits() != before.pitch.to_bits()
        {
            fields.push(EntityField::Rotation {
                yaw: after.yaw,
                pitch: after.pitch,
            });
        }
        if after.on_ground != before.on_ground {
            fields.push(EntityField::OnGround(after.on_ground));
        }
        if after.fall_distance.to_bits() != before.fall_distance.to_bits() {
            fields.push(EntityField::FallDistance(after.fall_distance));
        }
        self.commit_entity(chunk, before.key, fields, output);
    }

    /// Sets the move check's state of the riding player `id`.
    fn set_check(&mut self, id: PlayerId, check: VehicleCheck) {
        let Some(player) = self.players.get_mut(&id) else {
            return;
        };
        let Some(riding) = player.riding.as_mut() else {
            return;
        };
        if riding.check != check {
            riding.check = check;
            self.journal.players.insert(id);
        }
    }

    /// The vehicle's part of the end of each riding player's tick (`tickPlayer`, after
    /// the entities and the players' own ticks): one who drives a boat has their next
    /// moves checked from where it is now, and is put out once they have held it in the
    /// air longer than its gravity allows (160 ticks); one who does not is cleared.
    pub(crate) fn tick_drivers(&mut self, output: &mut TickOutput) {
        let riders: Vec<(PlayerId, PlayerRiding, EntityId)> = self
            .players
            .iter()
            .filter_map(|(id, player)| Some((*id, player.riding?, player.entity_id)))
            .collect();
        for (id, riding, entity) in riders {
            let root_key = self.root_of(riding.vehicle);
            let driving = self
                .find_entity(root_key)
                .filter(|(_, root)| self.drives_root(root, entity))
                .map(|(_, root)| {
                    // A living root's gravity, a pig's 0.08: 80 ticks in the air at most.
                    let gravity = if is_boat(root.kind) {
                        boat::GRAVITY
                    } else {
                        crate::living::Attributes::LIVING.gravity
                    };
                    (root.position, gravity)
                });
            let (check, kicked) = vehicle_move::end_of_tick(&riding.check, driving, false);
            self.set_check(id, check);
            if kicked {
                output.player_events.push((
                    id,
                    PlayerEvent::Kicked {
                        reason: FLYING.to_owned(),
                    },
                ));
            }
        }
    }
}
