//! Minecarts (`AbstractMinecart`, `OldMinecartBehavior` and the kinds;
//! `docs/groundwork/n5-brief.md`, section 3; step N5e of `docs/groundwork/n5-plan.md`):
//! a cart placed on a rail by its item, its tick along rails of every shape and off
//! them, powered rails pushing and braking, activator rails, carts pushing carts and a
//! moving empty cart picking up what it may carry, players pushing carts, the furnace
//! cart's fuel and push, container carts rolling by how full they are, and what a hit
//! or a use from any region does to a cart.
//!
//! The arithmetic is the game's (`crate::physics::minecart`, `crate::physics::body`):
//! a cart's positions, tick by tick, are the official server's for the same blocks, as
//! the old behaviour draws nothing. What a cart's tick does to other entities and to
//! blocks (a push, a rider let off, a detector rail pressed, a hit) is done on their
//! records as the game does it in the middle of the tick, the cart's own record put
//! where the cart is first, so that they read it as the game's would; the cart's own
//! fields are committed at the end of its tick, as an item's are.
//!
//! The hurt data and a furnace cart's fuel are counters: they fall in place and are
//! saved at the worker's checkpoints (`vehicles.rs`); a cart's place, its motion, its
//! yaw, whether it is flipped and a furnace cart's push are committed when they change.
//! What the game keeps of a cart only in memory (its block underfoot, a cobweb's hold,
//! whether it was in water at the end of its last tick, the height of a furnace cart's
//! push) the region keeps by key; after a take-in it is worked out again from the
//! record, as at the game's load.
//!
//! Named differences (`docs/groundwork/n5-plan.md`, sections 5 and 7):
//! - A hopper cart takes in what lies about it and what the container above it holds
//!   after its tick, and an activator rail switches it off (`hoppers.rs`, step N6e). A
//!   TNT cart primes and explodes as the game's (`tnt.rs`, step N6c).
//! - Carts tick in the order of their keys, so that three carts meeting push one
//!   another in another order than the game's entity list.
//! - Players are not in copies, and the region gives the physics no players' boxes yet
//!   (section 7, item 3): a cart passes through a player rather than stop against them.
//!   A player's box does push a cart of the region's own chunks, from the player's
//!   tick, and a moving empty cart is pushed back by a player it meets, but the player
//!   is not sent the nudge the game's cart gives them.
//! - A cart meets and pushes only the entities of the region's own chunks: one in a
//!   copy, across a line, is neither pushed nor picked up.
//! - A cart's fire ticks change nothing a server or a client sees (it never calls
//!   `baseTick`, the brief's finding 2): they are not kept, and a burning cart that rolls
//!   into water makes no sound of going out. Fire and lava still hurt it, and break it.
//! - A cart presses plates and tripwire where it is after its moves, not along them,
//!   as it meets them only there (`cart_effects_from_blocks`).
//! - A sprinting hit's push of a cart that survives it is not made: the hit carries no
//!   yaw, and every player is a creative one, whose hit breaks the cart at once.
//! - The rider's keys reach a cart through [`Region::rider_intent`], which knows no
//!   player's keys until the riders' step (N5h) keeps them.
//! - A boat a cart meets is picked up (`startRiding`); a boat pushed by a cart that
//!   carries something, or one that rides something, is pushed as `AbstractBoat.push`
//!   pushes it (`boats.rs`).
//! - `isInWater` after a take-in is what the cart's box is in then, where the game's
//!   load would read false until the cart's first fluid update.

use clustine_blocks::data::{flag, is_block, is_class, item_in_tag};
use clustine_data::{BlockClass, BlockState, EntityTypeInfo, blocks, entity_types, items, sounds};
use clustine_world::{
    BlockPos, ChunkPos, Contents, Double, Entity, EntityChange, EntityData, EntityField, EntityId,
    EntityUse, Float, Hurt, ItemStack, PlayerId,
};

use super::Region;
use super::entities::insecure_uuid;
use super::items::{CartMemory, Memory, View, tracked};
use super::riding::{Relocated, wrap_degrees};
use super::vehicles::container_size;
use crate::api::{RegionEvent, TickOutput, Tracked};
use crate::physics::aabb::Aabb;
use crate::physics::body::{Body, Bounce, Movement};
use crate::physics::entities::{is_minecart, is_pushable};
use crate::physics::item::{Inside, SOUND_SOURCE_NEUTRAL, check_inside_blocks};
use crate::physics::minecart::{
    self as rail, AIR_DRAG, GRAVITY, GRAVITY_IN_WATER, Pushing, Shape, clamp, horizontal_distance,
};
use crate::physics::{self, Vec3, World, fluids, horizontal_sqr, length_sqr};

/// The ticks a furnace cart burns on one piece of coal, and the most it holds.
const FUEL_PER_ITEM: i32 = 3600;
const MOST_FUEL: i32 = 32_000;

/// The cart a minecart item makes (`MinecartItem`'s type): of the five a player can
/// place. Spawner and command block carts have no item a player has.
pub(super) fn kind_of_item(item: i32) -> Option<i32> {
    match item {
        items::MINECART => Some(entity_types::MINECART),
        items::CHEST_MINECART => Some(entity_types::CHEST_MINECART),
        items::FURNACE_MINECART => Some(entity_types::FURNACE_MINECART),
        items::TNT_MINECART => Some(entity_types::TNT_MINECART),
        items::HOPPER_MINECART => Some(entity_types::HOPPER_MINECART),
        _ => None,
    }
}

/// A cart in its tick: its record's fields that the tick reads and writes, and what the
/// game keeps of it in memory.
#[derive(Debug, Clone)]
struct Cart {
    key: EntityId,
    kind: i32,
    body: Body,
    yaw: f32,
    pitch: f32,
    flipped: bool,
    fuel: i32,
    /// A furnace cart's push, its height among it (which the game does not save).
    push: Vec3,
    contents: Contents,
    /// `isInWater()`, as the end of its last tick left it.
    in_water: bool,
    /// Where it was and how it faced as its tick began (`xo`, `zo`, `yRotO`).
    start: Vec3,
    start_yaw: f32,
    removed: bool,
    needs_sync: bool,
    sync_velocity: bool,
}

impl Cart {
    fn is_furnace(&self) -> bool {
        self.kind == entity_types::FURNACE_MINECART
    }

    /// `getMaxSpeed(level)` (`OldMinecartBehavior`'s, and `MinecartFurnace`'s half or
    /// three quarters of it).
    fn max_speed(&self) -> f64 {
        let base = if self.in_water { 0.2 } else { 0.4 };
        if self.is_furnace() {
            base * if self.in_water { 0.75 } else { 0.5 }
        } else {
            base
        }
    }

    /// `getDefaultGravity()`.
    fn gravity(&self) -> f64 {
        if self.in_water {
            GRAVITY_IN_WATER
        } else {
            GRAVITY
        }
    }

    /// What the cart's move reads of it: its gravity and air drag, no bounce of its own,
    /// and rails that keep its speed.
    fn move_rules(&self) -> Bounce {
        Bounce {
            gravity: self.gravity(),
            air_drag: AIR_DRAG,
            own: 0.0,
            living: false,
            rails_keep_speed: true,
            max_up_step: 0.0,
        }
    }

    fn pushing(&self) -> Pushing {
        Pushing {
            position: self.body.position,
            motion: self.body.motion,
            yaw: self.yaw,
            furnace: self.is_furnace(),
            needs_sync: false,
        }
    }

    fn pushed(&mut self, pushing: Pushing) {
        self.body.motion = pushing.motion;
        self.needs_sync |= pushing.needs_sync;
    }

    /// `applyNaturalSlowdown(v)` of the kind, carrying someone or not.
    fn natural_slowdown(&mut self, v: Vec3, carrying: bool) -> Vec3 {
        match self.kind {
            entity_types::CHEST_MINECART | entity_types::HOPPER_MINECART => {
                // `AbstractMinecartContainer.applyNaturalSlowdown`, in place of the
                // plain one: the fuller, the sooner it stops. No cart a player meets
                // has a loot table.
                let size = container_size(u16::try_from(self.kind).expect("a type's id"));
                let empty = 15 - i32::from(rail::container_signal(&self.contents, size));
                let mut factor: f32 = 0.98;
                factor += empty as f32 * 0.001;
                if self.in_water {
                    factor *= 0.95;
                }
                let factor = f64::from(factor);
                [v[0] * factor, v[1] * 0.0, v[2] * factor]
            }
            entity_types::FURNACE_MINECART => {
                let w = if length_sqr(self.push) > 1.0e-7 {
                    self.push = self.new_push_along(v);
                    let w = [
                        v[0] * 0.8 + self.push[0],
                        v[1] * 0.0 + self.push[1],
                        v[2] * 0.8 + self.push[2],
                    ];
                    if self.in_water {
                        physics::scale(w, 0.1)
                    } else {
                        w
                    }
                } else {
                    [v[0] * 0.98, v[1] * 0.0, v[2] * 0.98]
                };
                self.plain_slowdown(w, carrying)
            }
            _ => self.plain_slowdown(v, carrying),
        }
    }

    /// `AbstractMinecart.applyNaturalSlowdown(v)`.
    fn plain_slowdown(&self, v: Vec3, carrying: bool) -> Vec3 {
        let factor = if carrying { 0.997 } else { 0.96 };
        let slowed = [v[0] * factor, v[1] * 0.0, v[2] * factor];
        if self.in_water {
            physics::scale(slowed, 0.949_999_988_079_071)
        } else {
            slowed
        }
    }

    /// `MinecartFurnace.calculateNewPushAlong(v)`: the push turned along the motion,
    /// its length kept, once both are long enough.
    fn new_push_along(&self, v: Vec3) -> Vec3 {
        let push = self.push;
        if horizontal_sqr(push) > 1.0e-4 && horizontal_sqr(v) > 0.001 {
            // `push.projectedOn(v)`.
            let squared = length_sqr(v);
            let projected = if squared == 0.0 {
                v
            } else {
                let dot = push[0] * v[0] + push[1] * v[1] + push[2] * v[2];
                physics::scale(physics::scale(v, dot), 1.0 / squared)
            };
            return physics::scale(physics::normalize(projected), length_sqr(push).sqrt());
        }
        push
    }
}

/// The blocks a cart's box is in, as the walk of `checkInsideBlocks` meets them.
struct CartInside<'a> {
    body: &'a Body,
    met: Vec<(BlockState, BlockPos, bool, bool)>,
}

impl Inside for CartInside<'_> {
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

/// A hurt a block a cart is in queues after its effect: fire's after `FIRE_IGNITE` (1,
/// or 2 for soul fire), lava's after `LAVA_IGNITE`.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Queued {
    Fire(f32),
    Lava,
}

impl Region {
    /// `MinecartItem.useOn`: a cart of the held item's kind placed on the clicked
    /// block where it is of `#minecraft:rails`, at its middle and a sixteenth up, half
    /// a block higher on a slope, facing nowhere in particular (yaw 0). A creative
    /// player keeps the item (`ServerPlayerGameMode.useItemOn` gives its count back).
    /// Under the old behaviour carts may be placed into one another. The game spawns
    /// it as from a dispenser (`EntitySpawnReason.DISPENSER`), which changes nothing.
    /// False, and nothing placed, for any other item or block, where the chunk is full or
    /// no key is left.
    pub(crate) fn place_minecart(
        &mut self,
        click: &crate::placement::Click,
        player: PlayerId,
        output: &mut TickOutput,
    ) -> bool {
        let _ = player;
        let Some(kind) = click.held.and_then(kind_of_item) else {
            return false;
        };
        let Some(state) = self.block(click.position) else {
            return false;
        };
        if !rail::in_rails_tag(state) {
            return false;
        }
        let shape = if is_class(state, BlockClass::BaseRailBlock) {
            Shape::of(state).unwrap_or(Shape::NorthSouth)
        } else {
            Shape::NorthSouth
        };
        let lift = if shape.is_slope() { 0.5 } else { 0.0 };
        let at = click.position;
        let position = [
            f64::from(at.x) + 0.5,
            f64::from(at.y) + 0.0625 + lift,
            f64::from(at.z) + 0.5,
        ];
        let chunk = ChunkPos::containing(position[0], position[2]);
        let Some(key) = self.take_entity_key(chunk) else {
            return false;
        };
        // `Entity.<init>`: the UUID's two longs from the new entity's own random.
        let mut own = self.entity_stream(key).random;
        let uuid = insecure_uuid(own.next_u64(), own.next_u64());
        self.entity_drew(key, 2);
        let entity = Entity {
            key,
            kind: u16::try_from(kind).expect("entity type ids fit 16 bits"),
            uuid,
            position,
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
            data: EntityData::Minecart {
                hurt: Hurt::default(),
                display: None,
                display_offset: None,
                flipped: false,
                contents: Vec::new(),
                fuel: 0,
                push: [Double(0.0); 2],
                fuse: -1,
                enabled: true,
            },
        };
        // A new cart has never been in water: its first tick reads it so.
        self.entity_memory.insert(key, Memory::default());
        self.add_fresh_entity(entity, output);
        true
    }

    /// The cart of `entity`, of the loaded chunk `chunk`, as its tick starts.
    fn cart_of(&self, entity: &Entity, chunk: ChunkPos) -> Option<Cart> {
        let EntityData::Minecart {
            flipped,
            fuel,
            push,
            contents,
            ..
        } = &entity.data
        else {
            return None;
        };
        let info = EntityTypeInfo::of(i32::from(entity.kind));
        let memory = self.entity_memory.get(&entity.key);
        let mut body = Body {
            position: entity.position,
            motion: entity.motion,
            on_ground: entity.on_ground,
            fall_distance: entity.fall_distance,
            width: info.width,
            height: info.height,
            old_position: entity.position,
            horizontal_collision: false,
            vertical_collision: false,
            vertical_collision_below: false,
            supporting: memory.and_then(|memory| memory.supporting),
            on_ground_no_blocks: memory.is_some_and(|memory| memory.on_ground_no_blocks),
            stuck_speed: memory.map_or([0.0; 3], |memory| memory.stuck_speed),
            movements: Vec::new(),
            in_water: false,
            sync_position: false,
            falling_block: false,
            item: false,
            fell_on: None,
            noclip: None,
            mover: self.mover_of(entity, chunk),
            steps: None,
        };
        let view = View {
            region: self,
            home: chunk,
        };
        if memory.is_none() && body.on_ground {
            body.check_supporting_block(&view, None);
        }
        // What the end of its last tick found, or, after a take-in, what its box is in
        // now.
        let in_water = match memory {
            Some(memory) => memory.cart.in_water,
            None => fluids::update(&view, &body.bounds()).in_water(),
        };
        body.in_water = in_water;
        let push_y = memory.map_or(0.0, |memory| memory.cart.push_y);
        Some(Cart {
            key: entity.key,
            kind: i32::from(entity.kind),
            body,
            yaw: entity.yaw,
            pitch: entity.pitch,
            flipped: *flipped,
            fuel: *fuel,
            push: [push[0].0, push_y, push[1].0],
            contents: contents.clone(),
            in_water,
            start: entity.position,
            start_yaw: entity.yaw,
            removed: false,
            needs_sync: false,
            sync_velocity: false,
        })
    }

    /// Puts the cart's place, motion and turn into its record in place, so that what
    /// its tick does to others (a rider let off, a detector rail, a hit that breaks it)
    /// reads the cart where it is now; nothing is committed, the end of its tick
    /// commits it.
    fn sync_cart(&mut self, cart: &Cart, chunk: ChunkPos) {
        let Some(loaded) = self.chunks.get_mut(&chunk) else {
            return;
        };
        let fields = vec![
            EntityField::Position(cart.body.position),
            EntityField::Motion(cart.body.motion),
            EntityField::Rotation {
                yaw: cart.yaw,
                pitch: cart.pitch,
            },
            EntityField::OnGround(cart.body.on_ground),
        ];
        loaded.apply_entity(&EntityChange::Set(cart.key, fields));
    }

    /// Whether the cart `key` carries anyone (`isVehicle`): a seat kept for a player on
    /// the way counts (`docs/groundwork/n5-plan.md`, section 7 item 2).
    fn carries(&self, key: EntityId) -> bool {
        self.find_entity(key)
            .is_some_and(|(_, record)| !record.riding.passengers.is_empty())
    }

    /// The move intent of the rider in the cart `key`'s first seat, where that is a
    /// player (`ServerPlayer.getLastClientMoveIntent`, [`rail::move_intent`]) from the
    /// keys they hold and their yaw: `None` where the first passenger is no player or
    /// there is none.
    pub(crate) fn rider_intent(&self, key: EntityId) -> Option<Vec3> {
        let (_, record) = self.find_entity(key)?;
        let first = record.riding.passengers.first()?;
        if !first.player {
            return None;
        }
        // A seat kept for a player on their way pushes nothing until they come.
        let Some(player) = self
            .player_with_entity(first.key)
            .and_then(|id| self.players.get(&id))
        else {
            return Some([0.0; 3]);
        };
        Some(rail::move_intent(&player.keys, player.pose.yaw))
    }

    /// `AbstractMinecart.tick()` of `entity`, in the loaded chunk `position`: the hurt
    /// data fall, a cart far below the world goes, the behaviour moves it, the fluids
    /// push it, lava hurts it; and a furnace cart burns its fuel. A cart never calls
    /// `baseTick`.
    pub(crate) fn tick_minecart(
        &mut self,
        entity: Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let key = entity.key;
        self.decay_hurt(key, output);
        let Some(mut cart) = self.cart_of(&entity, position) else {
            return;
        };
        let min_y = self
            .chunks
            .get(&position)
            .map_or(-64, |chunk| chunk.min_y());
        if cart.body.position[1] < f64::from(min_y - 64) {
            // `onBelowWorld` discards it, and a container spills as it goes.
            self.remove_destroyed(key, output);
            return;
        }
        // `handlePortal`: there are no portals yet.
        self.cart_behaviour(&mut cart, position, output);
        if !cart.removed {
            // `updateFluidInteraction`, after the behaviour, which read last tick's.
            let view = View {
                region: self,
                home: position,
            };
            let found = fluids::update(&view, &cart.body.bounds());
            cart.in_water = found.in_water();
            cart.body.in_water = cart.in_water;
            if found.in_water() {
                cart.body.fall_distance = 0.0;
            }
            cart.body.motion = fluids::push(&found, cart.body.motion);
            if found.in_lava() {
                // `lavaIgnite` sets fire ticks, which a cart never counts down; then
                // `lavaHurt`.
                self.cart_lava_hurt(&mut cart, position, output);
                cart.body.fall_distance *= 0.5;
            }
        }
        if !cart.removed && cart.kind == entity_types::TNT_MINECART {
            // `MinecartTNT.tick` after `super.tick()`: its fuse, and its explosion.
            self.sync_cart(&cart, position);
            let moving = super::tnt::Moving {
                position: cart.body.position,
                motion: cart.body.motion,
                horizontal_collision: cart.body.horizontal_collision,
                in_water: cart.in_water,
            };
            if self.tnt_cart_tick(key, moving, output) {
                cart.removed = true;
            }
        }
        if !cart.removed && cart.kind == entity_types::HOPPER_MINECART {
            // `MinecartHopper.tick` after `super.tick()`: `tryConsumeItems`.
            self.sync_cart(&cart, position);
            self.hopper_cart_consume(
                key,
                position,
                cart.body.position,
                cart.body.bounds(),
                output,
            );
        }
        if !cart.removed && cart.is_furnace() {
            // `MinecartFurnace.tick` after `super.tick()`.
            if cart.fuel > 0 {
                cart.fuel -= 1;
            }
            if cart.fuel <= 0 {
                cart.push = [0.0; 3];
            }
            if cart.fuel > 0 {
                // `nextInt(4)` for a smoke particle the server does not send: drawn,
                // so that the cart's stream stays the game's in count.
                self.entity_drew(key, 1);
            }
        }
        self.write_cart(&entity, cart, position, output);
    }

    /// `OldMinecartBehavior.tick()` on the server.
    fn cart_behaviour(&mut self, cart: &mut Cart, chunk: ChunkPos, output: &mut TickOutput) {
        cart.body.motion[1] += -cart.gravity();
        let view = View {
            region: self,
            home: chunk,
        };
        let pos = rail::rail_block_or_below(&view, cart.body.position);
        let state = view.state(pos);
        if rail::is_rail(state) {
            self.move_along_track(cart, pos, state, chunk, output);
            if !cart.removed && is_block(state, blocks::ACTIVATOR_RAIL) {
                self.activate_minecart(cart, flag(state, "powered"), chunk, output);
            }
        } else {
            self.come_off_track(cart, chunk, output);
        }
        self.cart_effects_from_blocks(cart, chunk, output);
        if cart.removed {
            return;
        }
        cart.pitch = 0.0;
        // Old place less new: the cart faces back along its motion.
        let dx = cart.start[0] - cart.body.position[0];
        let dz = cart.start[2] - cart.body.position[2];
        if dx * dx + dz * dz > 0.001 {
            let yaw = (rail::atan2(dz, dx) * 180.0 / std::f64::consts::PI) as f32;
            if yaw.is_finite() {
                cart.yaw = yaw;
            }
            if cart.flipped {
                cart.yaw += 180.0;
            }
        }
        let turned = f64::from(wrap_degrees(cart.yaw - cart.start_yaw));
        if !(-170.0..170.0).contains(&turned) {
            cart.yaw += 180.0;
            cart.flipped = !cart.flipped;
        }
        cart.pitch %= 360.0;
        cart.yaw %= 360.0;
        self.push_and_pick_up(cart, chunk, output);
    }

    /// `AbstractMinecart.move(SELF, vector)` under the old behaviour: the entity's move,
    /// then the effects of the blocks it ends in.
    fn cart_move(
        &mut self,
        cart: &mut Cart,
        vector: Vec3,
        chunk: ChunkPos,
        output: &mut TickOutput,
    ) {
        let view = View {
            region: self,
            home: chunk,
        };
        let rules = cart.move_rules();
        cart.body
            .move_with(&view, false, vector, None, rules, &mut |_| {});
        cart.body.movements.clear();
        // `MinecartTNT.causeFallDamage`, from the block it lands on: a fall of three
        // or more explodes it at once.
        if cart.kind == entity_types::TNT_MINECART
            && let Some((fallen, on)) = cart.body.fell_on
            && let Some(fallen) = super::tnt::fall_onto(view.state(on), fallen)
            && fallen >= 3.0
        {
            self.sync_cart(cart, chunk);
            let moving = super::tnt::Moving {
                position: cart.body.position,
                motion: cart.body.motion,
                horizontal_collision: cart.body.horizontal_collision,
                in_water: cart.in_water,
            };
            let speed = fallen / 10.0;
            self.tnt_cart_fell(cart.key, moving, speed * speed, output);
            cart.removed = true;
            return;
        }
        self.cart_effects_from_blocks(cart, chunk, output);
    }

    /// `OldMinecartBehavior.moveAlongTrack(level)` on the rail `state` at `pos`.
    fn move_along_track(
        &mut self,
        cart: &mut Cart,
        pos: BlockPos,
        state: BlockState,
        chunk: ChunkPos,
        output: &mut TickOutput,
    ) {
        let Some(shape) = Shape::of(state) else {
            return;
        };
        cart.body.fall_distance = 0.0;
        let [x, y_start, z] = cart.body.position;
        let before = rail::get_pos(
            &View {
                region: self,
                home: chunk,
            },
            [x, y_start, z],
        );
        let mut y = f64::from(pos.y);
        let mut powered = false;
        let mut braking = false;
        if is_block(state, blocks::POWERED_RAIL) {
            powered = flag(state, "powered");
            braking = !powered;
        }
        let mut slope = 0.007_812_5;
        if cart.in_water {
            slope *= 0.2;
        }
        // Down a slope, always.
        let downhill = match shape {
            Shape::AscendingEast => Some([-slope, 0.0, 0.0]),
            Shape::AscendingWest => Some([slope, 0.0, 0.0]),
            Shape::AscendingNorth => Some([0.0, 0.0, slope]),
            Shape::AscendingSouth => Some([0.0, 0.0, -slope]),
            _ => None,
        };
        if let Some(by) = downhill {
            cart.body.motion = physics::add(cart.body.motion, by);
            y += 1.0;
        }
        // The whole horizontal speed, up to 2, turned onto the rail's line.
        let v = cart.body.motion;
        let (first, second) = shape.exits();
        let mut ex = f64::from(second[0] - first[0]);
        let mut ez = f64::from(second[2] - first[2]);
        let length = (ex * ex + ez * ez).sqrt();
        let dot = v[0] * ex + v[2] * ez;
        if dot < 0.0 {
            ex = -ex;
            ez = -ez;
        }
        let speed = 2.0_f64.min(horizontal_distance(v));
        cart.body.motion = [speed * ex / length, v[1], speed * ez / length];
        // The rider's push, which lifts the brake.
        if let Some(intent) = self.rider_intent(cart.key)
            && rail::rider_push(&mut cart.body.motion, intent)
        {
            braking = false;
        }
        if braking {
            let m = cart.body.motion;
            cart.body.motion = if horizontal_distance(m) < 0.03 {
                [0.0; 3]
            } else {
                [m[0] * 0.5, m[1] * 0.0, m[2] * 0.5]
            };
        }
        // Onto the rail's line.
        let (bx, bz) = (f64::from(pos.x), f64::from(pos.z));
        let x0 = bx + 0.5 + f64::from(first[0]) * 0.5;
        let z0 = bz + 0.5 + f64::from(first[2]) * 0.5;
        let x1 = bx + 0.5 + f64::from(second[0]) * 0.5;
        let z1 = bz + 0.5 + f64::from(second[2]) * 0.5;
        let ex = x1 - x0;
        let ez = z1 - z0;
        let t = if ex == 0.0 {
            z - bz
        } else if ez == 0.0 {
            x - bx
        } else {
            let dx = x - x0;
            let dz = z - z0;
            (dx * ex + dz * ez) * 2.0
        };
        cart.body.position = [x0 + ex * t, y, z0 + ez * t];
        // The move, along x and z only: the height comes from the snaps.
        let k = if self.carries(cart.key) { 0.75 } else { 1.0 };
        let most = cart.max_speed();
        let v = cart.body.motion;
        let vector = [
            clamp(k * v[0], -most, most),
            0.0,
            clamp(k * v[2], -most, most),
        ];
        self.cart_move(cart, vector, chunk, output);
        if cart.removed {
            return;
        }
        // Into the next block of a slope, or down from its low end.
        let [px, py, pz] = cart.body.position;
        let enters = |exit: [i32; 3]| {
            physics::floor(px).wrapping_sub(pos.x) == exit[0]
                && physics::floor(pz).wrapping_sub(pos.z) == exit[2]
        };
        if first[1] != 0 && enters(first) {
            cart.body.position = [px, py + f64::from(first[1]), pz];
        } else if second[1] != 0 && enters(second) {
            cart.body.position = [px, py + f64::from(second[1]), pz];
        }
        let carrying = self.carries(cart.key);
        cart.body.motion = cart.natural_slowdown(cart.body.motion, carrying);
        // Speed from the height lost or gained.
        let after = rail::get_pos(
            &View {
                region: self,
                home: chunk,
            },
            cart.body.position,
        );
        if let (Some(after), Some(before)) = (after, before) {
            let gained = (before[1] - after[1]) * 0.05;
            let v = cart.body.motion;
            let h = horizontal_distance(v);
            if h > 0.0 {
                cart.body.motion = [
                    v[0] * ((h + gained) / h),
                    v[1] * 1.0,
                    v[2] * ((h + gained) / h),
                ];
            }
            cart.body.position = [cart.body.position[0], after[1], cart.body.position[2]];
        }
        // Into a new block, the whole speed along it.
        let nx = physics::floor(cart.body.position[0]);
        let nz = physics::floor(cart.body.position[2]);
        if nx != pos.x || nz != pos.z {
            let v = cart.body.motion;
            let h = horizontal_distance(v);
            cart.body.motion = [
                h * f64::from(nx.wrapping_sub(pos.x)),
                v[1],
                h * f64::from(nz.wrapping_sub(pos.z)),
            ];
        }
        if !powered {
            return;
        }
        let v = cart.body.motion;
        let h = horizontal_distance(v);
        if h > 0.01 {
            cart.body.motion = [v[0] + v[0] / h * 0.06, v[1] + 0.0, v[2] + v[2] / h * 0.06];
            return;
        }
        // From rest: kicked away from a conductor at one end of a straight rail; a
        // powered rail on a slope gives a cart at rest nothing.
        let view = View {
            region: self,
            home: chunk,
        };
        let conducts = |dx: i32, dz: i32| {
            let at = BlockPos::new(pos.x + dx, pos.y, pos.z + dz);
            clustine_blocks::connect::is_conductor(view.state(at))
        };
        let (mut sx, mut sz) = (v[0], v[2]);
        match shape {
            Shape::EastWest => {
                if conducts(-1, 0) {
                    sx = 0.02;
                } else if conducts(1, 0) {
                    sx = -0.02;
                }
            }
            Shape::NorthSouth => {
                if conducts(0, -1) {
                    sz = 0.02;
                } else if conducts(0, 1) {
                    sz = -0.02;
                }
            }
            _ => return,
        }
        cart.body.motion = [sx, v[1], sz];
    }

    /// `AbstractMinecart.comeOffTrack(level)`: no faster than the top speed along x and
    /// z, half as fast on the ground, then the move, and the air's drag off the ground.
    fn come_off_track(&mut self, cart: &mut Cart, chunk: ChunkPos, output: &mut TickOutput) {
        let most = cart.max_speed();
        let v = cart.body.motion;
        cart.body.motion = [clamp(v[0], -most, most), v[1], clamp(v[2], -most, most)];
        if cart.body.on_ground {
            cart.body.motion = physics::scale(cart.body.motion, 0.5);
        }
        let vector = cart.body.motion;
        self.cart_move(cart, vector, chunk, output);
        if !cart.removed && !cart.body.on_ground {
            cart.body.motion = physics::scale(cart.body.motion, f64::from(AIR_DRAG));
        }
    }

    /// `activateMinecart(level, x, y, z, powered)` of a cart on an activator rail, every
    /// tick it is on one: a plain cart on a powered one lets its riders off, and rocks,
    /// its damage set to 50, each time its wobble has run down; a TNT cart on a powered
    /// one is primed if it is not yet (`MinecartTNT.activateMinecart`); a hopper cart is
    /// switched off on a powered one and on on one that is not
    /// (`MinecartHopper.activateMinecart`, `hoppers.rs`). Chest and furnace carts do
    /// nothing.
    fn activate_minecart(
        &mut self,
        cart: &mut Cart,
        powered: bool,
        chunk: ChunkPos,
        output: &mut TickOutput,
    ) {
        if cart.kind == entity_types::HOPPER_MINECART {
            self.hopper_cart_activated(cart.key, chunk, powered);
            return;
        }
        if cart.kind == entity_types::TNT_MINECART && powered {
            self.sync_cart(cart, chunk);
            if self.tnt_cart_fuse(cart.key).is_some_and(|fuse| fuse < 0) {
                self.prime_tnt_cart(cart.key, None, output);
            }
            return;
        }
        if cart.kind != entity_types::MINECART || !powered {
            return;
        }
        self.sync_cart(cart, chunk);
        if self.carries(cart.key) {
            self.eject_passengers(cart.key, output);
        }
        let Some((at, record)) = self.find_entity(cart.key) else {
            cart.removed = true;
            return;
        };
        let EntityData::Minecart { hurt, .. } = &record.data else {
            return;
        };
        if hurt.time != 0 {
            return;
        }
        let shaken = Hurt {
            time: 10,
            direction: -hurt.direction,
            damage: Float(50.0),
        };
        let mut data = record.data.clone();
        if let EntityData::Minecart { hurt, .. } = &mut data {
            *hurt = shaken;
        }
        let hops = record.hops;
        self.commit_entity(at, cart.key, vec![EntityField::Data(data.clone())], output);
        output.events.push(RegionEvent::DataChanged {
            entity: cart.key,
            chunk: at,
            hops,
            data,
        });
        // `markHurt`.
        cart.sync_velocity = true;
    }

    /// `applyEffectsFromBlocks()` of a cart under the old behaviour: what it stands on,
    /// then the blocks its box is in where it is (one movement from its place to
    /// itself), as the game applies them: a detector rail is pressed and a cactus hurts
    /// at once, a cobweb or powder snow holds it for its next move, and fire's and
    /// lava's hurts come after.
    fn cart_effects_from_blocks(
        &mut self,
        cart: &mut Cart,
        chunk: ChunkPos,
        output: &mut TickOutput,
    ) {
        if cart.removed {
            return;
        }
        self.sync_cart(cart, chunk);
        let view = View {
            region: self,
            home: chunk,
        };
        let standing = cart.body.on_ground.then(|| {
            let on = cart.body.on_pos_legacy(&view);
            (on, view.state(on))
        });
        let here = cart.body.position;
        let mut inside = CartInside {
            body: &cart.body,
            met: Vec::new(),
        };
        check_inside_blocks(
            &view,
            &mut inside,
            &[Movement {
                from: here,
                to: here,
                asked: None,
            }],
        );
        let met = inside.met;
        if let Some((on, state)) = standing {
            // `Block.stepOn`.
            if is_class(state, BlockClass::SlimeBlock) {
                let up = cart.body.motion[1].abs();
                if up < 0.1 {
                    let factor = 0.4 + up * 0.2;
                    let [x, y, z] = cart.body.motion;
                    cart.body.motion = [x * factor, y * 1.0, z * factor];
                }
            } else {
                // Redstone ore lights, and a sculk sensor hears it (`vibrations.rs`).
                let source = super::vibrations::Source::of(cart.key);
                self.stood_on(on, source, cart.body.position, output);
            }
        }
        let mut queued = Vec::new();
        let toucher = super::pressed::Toucher {
            key: cart.key,
            bounds: cart.body.bounds(),
            living: false,
        };
        for (state, pos, block, fluid) in met {
            if block {
                if is_class(state, BlockClass::BasePressurePlateBlock)
                    || is_class(state, BlockClass::TripWireBlock)
                {
                    // `entityInside` of a plate or tripwire, which counts the cart where
                    // it is (`pressed.rs`).
                    self.entity_touched(pos, &toucher, output);
                } else if is_class(state, BlockClass::DetectorRailBlock) {
                    // `DetectorRailBlock.entityInside`: any entity, but it looks for
                    // carts.
                    if self.ticks_chunk(pos.chunk()) && !flag(state, "powered") {
                        self.detector_rail_check(pos, state, output);
                    }
                } else if is_class(state, BlockClass::BaseFireBlock) {
                    let damage = if is_class(state, BlockClass::SoulFireBlock) {
                        2.0
                    } else {
                        1.0
                    };
                    queued.push(Queued::Fire(damage));
                } else if is_class(state, BlockClass::CactusBlock) {
                    self.cart_hurt(cart, 1.0, false, chunk, output);
                } else if is_class(state, BlockClass::WebBlock) {
                    cart.body.fall_distance = 0.0;
                    cart.body.stuck_speed = [0.25, f64::from(0.05_f32), 0.25];
                } else if is_class(state, BlockClass::PowderSnowBlock) {
                    let side = f64::from(0.9_f32);
                    cart.body.fall_distance = 0.0;
                    cart.body.stuck_speed = [side, 1.5, side];
                } else if is_class(state, BlockClass::LavaCauldronBlock) {
                    queued.push(Queued::Lava);
                }
            }
            if fluid && clustine_blocks::data::has_lava(state) {
                queued.push(Queued::Lava);
            }
            if cart.removed {
                return;
            }
        }
        // `FIRE_IGNITE`'s hurts before `LAVA_IGNITE`'s, the game's order of effects;
        // within each, in the order the blocks queued them.
        queued.sort_by_key(|hurt| matches!(hurt, Queued::Lava));
        for hurt in queued {
            match hurt {
                Queued::Fire(amount) => {
                    self.cart_hurt(cart, amount, true, chunk, output);
                }
                Queued::Lava => self.cart_lava_hurt(cart, chunk, output),
            }
            if cart.removed {
                return;
            }
        }
    }

    /// `hurtServer` of the cart by what is not a player (fire, a cactus, lava): its
    /// wobble and damage, broken above 40, dropping its item; a TNT cart is primed by a
    /// source that is fire (`ignites`), and by any that breaks it as it moves. Whether
    /// it is still there.
    fn cart_hurt(
        &mut self,
        cart: &mut Cart,
        amount: f32,
        ignites: bool,
        chunk: ChunkPos,
        output: &mut TickOutput,
    ) -> bool {
        self.sync_cart(cart, chunk);
        // Fire, lava and a cactus have no entity that caused them.
        let igniter = ignites.then_some(None);
        self.hurt_vehicle_by(cart.key, amount, false, igniter, output);
        if self.find_entity(cart.key).is_none() {
            cart.removed = true;
            return false;
        }
        cart.sync_velocity = true;
        true
    }

    /// `Entity.lavaHurt()`: a hurt of 4, and the sound of burning, its pitch drawn from
    /// the cart's own random, each time it is hurt (which is every time: a vehicle's
    /// `hurtServer` answers true while it is there).
    fn cart_lava_hurt(&mut self, cart: &mut Cart, chunk: ChunkPos, output: &mut TickOutput) {
        let position = cart.body.position;
        self.cart_hurt(cart, 4.0, true, chunk, output);
        let mut stream = self.entity_stream(cart.key).random;
        self.entity_drew(cart.key, 1);
        let pitch = 2.0 + stream.next_float() * 0.4;
        output.events.push(RegionEvent::Sound {
            sound: sounds::ENTITY_GENERIC_BURN,
            source: SOUND_SOURCE_NEUTRAL,
            position: clustine_world::Vec3::new(position[0], position[1], position[2]),
            volume: 0.4,
            pitch,
            except: None,
        });
    }

    /// `OldMinecartBehavior.pushAndPickupEntities()`: a plain cart moving at a tenth of
    /// a block a tick or more makes what it meets that is pushable, and no player or
    /// cart, ride it, while it carries nobody, and pushes the rest; any other cart
    /// pushes only the carts it meets. An armour stand is never pushable, so never
    /// picked up; a boat is.
    fn push_and_pick_up(&mut self, cart: &mut Cart, chunk: ChunkPos, output: &mut TickOutput) {
        self.sync_cart(cart, chunk);
        let reach = f64::from(0.2_f32);
        let bounds = cart.body.bounds().inflate([reach, 0.0, reach]);
        let met: Vec<(EntityId, ChunkPos)> = self
            .entities_in(&bounds)
            .into_iter()
            .filter(|(key, at)| *key != cart.key && self.ticks_chunk(*at))
            .collect();
        let picking_up =
            cart.kind == entity_types::MINECART && horizontal_sqr(cart.body.motion) >= 0.01;
        if !picking_up {
            for (key, at) in met {
                let Some(other) = self.entity_record(key, at).cloned() else {
                    continue;
                };
                let rides_it = other.riding.vehicle == Some(cart.key);
                if !rides_it && is_minecart(other.kind) {
                    self.pushed_by_cart(cart, &other, at, output);
                }
            }
            return;
        }
        for (key, at) in met {
            let Some(other) = self.entity_record(key, at).cloned() else {
                continue;
            };
            // `EntitySelector.pushableBy`: no teams, and no spectators among entities.
            if !is_pushable(other.kind) {
                continue;
            }
            let passenger = other.riding.vehicle.is_some();
            if is_minecart(other.kind) || self.carries(cart.key) || passenger {
                self.pushed_by_cart(cart, &other, at, output);
            } else {
                self.sync_cart(cart, chunk);
                self.start_riding(key, cart.key, false, output);
            }
        }
        // A player it meets, last (the game's list has players among the entities by
        // their sections), is pushed and pushes it back (`LivingEntity.push`).
        let players: Vec<Vec3> = self
            .players
            .values()
            .map(|player| {
                let feet = player.pose.position;
                [feet.x, feet.y, feet.z]
            })
            .filter(|feet| Aabb::of_entity(*feet, 0.6, 1.8).intersects(bounds))
            .collect();
        for feet in players {
            if self.carries(cart.key) {
                break;
            }
            if let Some((_, back)) = rail::entity_push(feet, cart.body.position) {
                let mut pushing = cart.pushing();
                pushing.motion = physics::add(pushing.motion, back);
                pushing.needs_sync = true;
                cart.pushed(pushing);
            }
        }
    }

    /// `e.push(cart)` of the entity `other` of the chunk `at`, met by `cart`: another
    /// cart pushes and is pushed (`AbstractMinecart.push`, `pushOtherMinecart`); a boat
    /// is pushed as `AbstractBoat.push` pushes it, and pushes the cart back where the
    /// cart carries nobody (`boats.rs`).
    fn pushed_by_cart(
        &mut self,
        cart: &mut Cart,
        other: &Entity,
        at: ChunkPos,
        output: &mut TickOutput,
    ) {
        if crate::physics::entities::is_boat(other.kind) {
            self.sync_cart(cart, at_of(self, cart.key).unwrap_or(at));
            let bounds = cart.body.bounds();
            if let Some(back) =
                self.boat_pushed_by_cart(other, at, cart.key, cart.body.position, &bounds, output)
            {
                let mut pushing = cart.pushing();
                pushing.motion = physics::add(pushing.motion, back);
                pushing.needs_sync = true;
                cart.pushed(pushing);
            }
            return;
        }
        if !is_minecart(other.kind) {
            return;
        }
        // `hasPassenger(cart)`: a cart that carries this one is not pushed by it.
        if other
            .riding
            .passengers
            .iter()
            .any(|seat| seat.key == cart.key)
        {
            return;
        }
        let mut this = Pushing {
            position: other.position,
            motion: other.motion,
            yaw: other.yaw,
            furnace: i32::from(other.kind) == entity_types::FURNACE_MINECART,
            needs_sync: false,
        };
        let mut me = cart.pushing();
        rail::push_carts(&mut this, &mut me);
        cart.pushed(me);
        self.set_pushed_motion(other, at, this, output);
    }

    /// The motion a push gave the cart `record` of the chunk `at`, committed and, as
    /// `needsSync`, sent.
    fn set_pushed_motion(
        &mut self,
        record: &Entity,
        at: ChunkPos,
        pushing: Pushing,
        output: &mut TickOutput,
    ) {
        let same = pushing.motion.map(f64::to_bits) == record.motion.map(f64::to_bits);
        if same && !pushing.needs_sync {
            return;
        }
        self.commit_entity(
            at,
            record.key,
            vec![EntityField::Motion(pushing.motion)],
            output,
        );
        let mut after = record.clone();
        after.motion = pushing.motion;
        let flags = if pushing.needs_sync {
            Tracked::NEEDS_SYNC
        } else {
            0
        };
        let mut report = tracked(&after, flags, false);
        report.chunk = at;
        output.events.push(RegionEvent::EntityTracked(report));
    }

    /// `AbstractMinecart.push(other)` of the cart `key` by an entity at `other` that is
    /// not a cart: an armour stand's `pushEntities` (the stands' step calls this), a
    /// player's own. The cart is pushed away; the push the other is to be given, a
    /// quarter of the cart's the other way, is returned. Nothing for a cart the region
    /// does not tick, or one that carries the other (`rider`).
    pub(crate) fn push_minecart(
        &mut self,
        key: EntityId,
        other: Vec3,
        rider: Option<EntityId>,
        output: &mut TickOutput,
    ) -> Option<Vec3> {
        let (at, record) = self.find_entity(key)?;
        if !self.ticks_chunk(at) || !is_minecart(record.kind) {
            return None;
        }
        if let Some(rider) = rider
            && record
                .riding
                .passengers
                .iter()
                .any(|seat| seat.key == rider)
        {
            return None;
        }
        let record = record.clone();
        let mut pushing = Pushing {
            position: record.position,
            motion: record.motion,
            yaw: record.yaw,
            furnace: i32::from(record.kind) == entity_types::FURNACE_MINECART,
            needs_sync: false,
        };
        let back = rail::push_cart_by(&mut pushing, other)?;
        self.set_pushed_motion(&record, at, pushing, output);
        Some(back)
    }

    /// The players' push of the carts their boxes meet (`LivingEntity.pushEntities` in
    /// the player's own tick, which the game runs after its levels), in the order of the
    /// players: each cart is pushed away, as `AbstractMinecart.push(player)` pushes it.
    /// What the cart gives the player back is the client's own to work out. The game's
    /// cramming draw (more than 23 pushable entities in one box) is not made.
    pub(crate) fn players_push_minecarts(&mut self, output: &mut TickOutput) {
        let players: Vec<(EntityId, Vec3)> = self
            .players
            .values()
            .map(|player| {
                let feet = player.pose.position;
                (player.entity_id, [feet.x, feet.y, feet.z])
            })
            .collect();
        for (player, feet) in players {
            let bounds = Aabb::of_entity(feet, 0.6, 1.8);
            for (key, at) in self.entities_in(&bounds) {
                let is_cart = self
                    .entity_record(key, at)
                    .is_some_and(|record| is_minecart(record.kind));
                if is_cart {
                    self.push_minecart(key, feet, Some(player), output);
                }
            }
        }
    }

    /// What the cart's tick leaves: its record, its commit, and the edges' report.
    fn write_cart(
        &mut self,
        before: &Entity,
        cart: Cart,
        chunk: ChunkPos,
        output: &mut TickOutput,
    ) {
        let key = before.key;
        if cart.removed {
            return;
        }
        let Some(current) = self.entity_record(key, chunk).cloned() else {
            return;
        };
        let mut data = current.data.clone();
        if let EntityData::Minecart {
            flipped,
            fuel,
            push,
            ..
        } = &mut data
        {
            *flipped = cart.flipped;
            *fuel = cart.fuel;
            *push = [Double(cart.push[0]), Double(cart.push[2])];
        }
        // A TNT cart's ignition, which its hits in this tick may have set.
        let ignition = self
            .entity_memory
            .get(&key)
            .and_then(|memory| memory.cart.ignition);
        self.entity_memory.insert(
            key,
            Memory {
                supporting: cart.body.supporting,
                on_ground_no_blocks: cart.body.on_ground_no_blocks,
                stuck_speed: cart.body.stuck_speed,
                cart: CartMemory {
                    in_water: cart.in_water,
                    push_y: cart.push[1],
                    ignition,
                },
                ..Memory::default()
            },
        );
        let mut after = Entity {
            position: cart.body.position,
            motion: cart.body.motion,
            yaw: cart.yaw,
            pitch: cart.pitch,
            on_ground: cart.body.on_ground,
            fall_distance: cart.body.fall_distance,
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
        let committed =
            moved || rotated || motion || after.on_ground != before.on_ground || state_changed;
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
                if state_changed {
                    fields.push(EntityField::Data(after.data.clone()));
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
        if cart.body.horizontal_collision {
            flags |= Tracked::HORIZONTAL_COLLISION;
        }
        if cart.body.vertical_collision {
            flags |= Tracked::VERTICAL_COLLISION;
        }
        if cart.needs_sync {
            flags |= Tracked::NEEDS_SYNC;
        }
        if cart.body.sync_position {
            flags |= Tracked::SYNC_POSITION;
        }
        if cart.sync_velocity {
            flags |= Tracked::SYNC_VELOCITY;
        }
        let synced = Tracked::NEEDS_SYNC | Tracked::SYNC_POSITION | Tracked::SYNC_VELOCITY;
        if committed || to != chunk || flags & synced != 0 || shown_changed {
            let mut report = tracked(&after, flags, shown_changed || state_changed);
            report.chunk = to;
            output.events.push(RegionEvent::EntityTracked(report));
        }
    }

    /// A use of a cart by a player of this region or another: a hit hurts it as
    /// `VehicleEntity` is hurt (a creative player's breaks it at once, a container
    /// spilling what it holds); a furnace cart takes coal or charcoal (`addFuel`), its
    /// push pointing from the player to the cart, as long as they are apart; a chest or
    /// hopper cart opens its menu (`interactWithContainerVehicle`, `menu_play.rs`), and
    /// riding a plain cart is a mount, the riders' step's.
    pub(crate) fn minecart_used(
        &mut self,
        entity: &Entity,
        position: ChunkPos,
        by: EntityUse,
        output: &mut TickOutput,
    ) {
        match by {
            EntityUse::Attack {
                creative, damage, ..
            } => {
                self.hurt_vehicle(entity.key, damage.0, creative, output);
            }
            EntityUse::Interact {
                player,
                held,
                from,
                sneaking,
                ..
            } => {
                // `Minecart.interact`: the plain cart is ridden, by a player who does not
                // sneak, where nobody rides it.
                if i32::from(entity.kind) == entity_types::MINECART && !sneaking {
                    self.mount_player(player, entity.key, output);
                }
                if i32::from(entity.kind) == entity_types::FURNACE_MINECART {
                    let from = from.map(|component| component.0);
                    self.add_fuel(entity, position, held, from, output);
                }
                if super::menu_play::is_container_vehicle(entity.kind)
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

    /// `MinecartFurnace.addFuel(playerPosition, stack)`: 3600 ticks for a piece of
    /// `#minecraft:furnace_minecart_fuel`, but never above 32000, and the push from the
    /// player to the cart, not made of length one. A creative player keeps the coal
    /// (`ItemStack.consume`).
    fn add_fuel(
        &mut self,
        entity: &Entity,
        chunk: ChunkPos,
        held: Option<ItemStack>,
        from: Vec3,
        output: &mut TickOutput,
    ) {
        let Some(held) = held else {
            return;
        };
        if !item_in_tag(held.item, "minecraft:furnace_minecart_fuel") {
            return;
        }
        let EntityData::Minecart { fuel, .. } = &entity.data else {
            return;
        };
        let fuelled = fuel.saturating_add(FUEL_PER_ITEM);
        if fuelled > MOST_FUEL {
            return;
        }
        let mut data = entity.data.clone();
        if let EntityData::Minecart { fuel, push, .. } = &mut data {
            *fuel = fuelled;
            if fuelled > 0 {
                let away = physics::sub(entity.position, from);
                *push = [Double(away[0]), Double(away[2])];
            }
        }
        if let Some(memory) = self.entity_memory.get_mut(&entity.key) {
            memory.cart.push_y = 0.0;
        }
        self.commit_entity(
            chunk,
            entity.key,
            vec![EntityField::Data(data.clone())],
            output,
        );
        let mut after = entity.clone();
        after.data = data;
        let mut report = tracked(&after, 0, true);
        report.chunk = chunk;
        output.events.push(RegionEvent::EntityTracked(report));
    }

    /// `DetectorRailBlock.getAnalogOutputSignal` of the detector rail `state` at `pos`,
    /// as a comparator reads it: 0 unless it is powered, and then
    /// `AbstractContainerMenu.getRedstoneSignalFromContainer` of the first chest or
    /// hopper cart in its search box. **Inferred**: `getEntitiesOfClass` gives them by
    /// chunk, then section, then insertion, for which keys stand. A command block cart's
    /// success count, which the game looks for first, never arises: none is placeable.
    pub(crate) fn detector_rail_output(&self, pos: BlockPos, state: BlockState) -> u8 {
        if !flag(state, "powered") {
            return 0;
        }
        self.minecarts_meeting(&detector_search_box(pos))
            .into_iter()
            .find_map(|cart| {
                let size = container_size(cart.kind);
                match &cart.data {
                    EntityData::Minecart { contents, .. } if size > 0 => {
                        Some(rail::container_signal(contents, size))
                    }
                    _ => None,
                }
            })
            .unwrap_or(0)
    }

    /// The carts of the loaded and copied chunks whose boxes meet `bounds`, by chunk,
    /// then section, then key.
    pub(crate) fn minecarts_meeting(&self, bounds: &Aabb) -> Vec<Entity> {
        let low = ChunkPos::containing(bounds.min[0] - 2.0, bounds.min[2] - 2.0);
        let high = ChunkPos::containing(bounds.max[0] + 2.0, bounds.max[2] + 2.0);
        let mut found = Vec::new();
        for x in low.x..=high.x {
            for z in low.z..=high.z {
                let position = ChunkPos::new(x, z);
                let Some(chunk) = self
                    .chunks
                    .get(&position)
                    .or_else(|| self.copies.get(&position))
                else {
                    continue;
                };
                let mut here: Vec<(i32, &Entity)> = chunk
                    .entities()
                    .iter()
                    .filter(|entity| {
                        is_minecart(entity.kind) && self.entity_box(entity).intersects(*bounds)
                    })
                    .map(|entity| (physics::floor(entity.position[1]) >> 4, entity))
                    .collect();
                here.sort_by_key(|(section, entity)| (*section, entity.key));
                found.extend(here.into_iter().map(|(_, entity)| entity.clone()));
            }
        }
        found
    }
}

/// The chunk the entity `key` is in, where a loaded chunk has it.
fn at_of(region: &Region, key: EntityId) -> Option<ChunkPos> {
    region.find_entity(key).map(|(chunk, _)| chunk)
}

/// `DetectorRailBlock.getSearchBB(pos)`: the middle of the block, a fifth in from its
/// sides and its top.
pub(crate) fn detector_search_box(pos: BlockPos) -> Aabb {
    Aabb::new(
        [
            f64::from(pos.x) + 0.2,
            f64::from(pos.y),
            f64::from(pos.z) + 0.2,
        ],
        [
            f64::from(pos.x.wrapping_add(1)) - 0.2,
            f64::from(pos.y.wrapping_add(1)) - 0.2,
            f64::from(pos.z.wrapping_add(1)) - 0.2,
        ],
    )
}

/// What of a cart's data a commit carries: what it shows, whether it is flipped, what
/// it holds, a furnace cart's push, whether a TNT cart is primed and whether a hopper
/// cart is switched on.
type CommittedParts<'a> = (
    Option<u16>,
    Option<i32>,
    bool,
    &'a Contents,
    [Double; 2],
    bool,
    bool,
);

/// The parts of a cart's data that a tick commits when they change: all but its hurt
/// data, its fuel and a TNT cart's fuse once it burns, which are counters.
fn committed_parts(data: &EntityData) -> Option<CommittedParts<'_>> {
    match data {
        EntityData::Minecart {
            display,
            display_offset,
            flipped,
            contents,
            push,
            fuse,
            enabled,
            ..
        } => Some((
            *display,
            *display_offset,
            *flipped,
            contents,
            *push,
            *fuse >= 0,
            *enabled,
        )),
        _ => None,
    }
}

/// The parts of a cart's data that its clients are shown: the hurt data, the block
/// shown and where, and whether a furnace cart burns.
fn shown_parts(data: &EntityData) -> Option<(Hurt, Option<u16>, Option<i32>, bool)> {
    match data {
        EntityData::Minecart {
            hurt,
            display,
            display_offset,
            fuel,
            ..
        } => Some((*hurt, *display, *display_offset, *fuel > 0)),
        _ => None,
    }
}
