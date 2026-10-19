//! Falling blocks (`docs/groundwork/n4-plan.md`, step N4c; the brief's sections 6 and
//! 7): a block whose tick finds nothing under it becomes a `FallingBlockEntity`
//! (`FallingBlockEntity.fall`), which falls as the game's does and lands where it
//! comes to rest, or drops as its item where it cannot.
//!
//! The entity's tick is the game's whole: it has no `baseTick`, so it falls through
//! water and lava at full speed, is not pushed by them and does not burn; gravity, its
//! move, the blocks it touches (a cobweb's or powder snow's hold, slime, lit redstone
//! ore), and then, on the ground or in water as concrete powder, its landing. Fire,
//! lava and cactus, which hurt what can be hurt, do nothing to it that anyone sees (the
//! game sets a fire countdown no client shows, and marks it hurt, which only syncs its
//! motion): left out.
//!
//! Named differences: an anvil's fall hurts no entity (the damage of living things is
//! not ported), though it chips the anvil as the game's does, with a draw from the
//! entity's stream; a stalactite's fall hurts no entity either. The game keeps a block
//! entity's data with a falling block summoned with it (`TileEntityData`), which no
//! block that falls by a rule has; Clustine keeps none. Whether a falling block hurts
//! entities is not kept: an anvil does, as the game's `falling` sets it, and nothing
//! else chips. Where the region cannot keep the entity (its chunk is full or no key is
//! left), the block does not fall and stays where it is; where its chunk fills up while
//! the block gives way, it is lost.

use clustine_blocks::data::{has_water, in_tag, is_class};
use clustine_blocks::fluid::{FluidState, concrete_of};
use clustine_blocks::survive::can_survive;
use clustine_data::{BlockClass, BlockState, EntityTypeInfo, blocks, entity_types};
use clustine_world::{BlockPos, ChunkPos, Entity, EntityChange, EntityData, EntityField, EntityId};

use super::Region;
use super::entities::insecure_uuid;
use super::items::{Memory, View, tracked};
use super::riding::Relocated;
use super::updates::{RegionView, UPDATE_ALL, UPDATE_LIMIT, fluid_block};
use crate::api::{RegionEvent, TickOutput, Tracked};
use crate::physics::aabb::Aabb;
use crate::physics::body::{AIR_DRAG, Body, Movement, clips_shape};
use crate::physics::item::{Inside, check_inside_blocks, move_guarded, tick_movements};
use crate::physics::{self, World};

/// The game's level event of an anvil landing (`AnvilBlock.onLand`).
const ANVIL_LANDED: i32 = 1031;
/// The game's level event of an anvil breaking as it lands (`onBrokenAfterFall`).
const ANVIL_DESTROYED: i32 = 1029;
/// `PointedDripstoneBlock.getStalactiteLandingSound`.
const DRIPSTONE_LANDED: i32 = 1045;
/// `SulfurSpikeBlock.getStalactiteLandingSound`.
const SULFUR_SPIKE_LANDED: i32 = 1052;

/// `FallingBlock.isFree`: what a block falls into.
pub(crate) fn is_free(state: BlockState) -> bool {
    let props = state.props();
    props.is_air()
        || in_tag(state, "minecraft:fire")
        || props.is_liquid()
        || props.can_be_replaced()
}

/// `getDelayAfterPlace`: two ticks, five for the dragon egg.
pub(crate) fn delay_after_place(state: BlockState) -> i32 {
    if is_class(state, BlockClass::DragonEggBlock) {
        5
    } else {
        2
    }
}

/// `AnvilBlock.damage`: the next anvil down, facing the same way; none after a damaged
/// one.
fn damaged_anvil(state: BlockState) -> Option<BlockState> {
    let next = match state.block()?.name {
        "anvil" | "minecraft:anvil" => "minecraft:chipped_anvil",
        "chipped_anvil" | "minecraft:chipped_anvil" => "minecraft:damaged_anvil",
        _ => return None,
    };
    let next = BlockState::parse(next)?;
    match state.property("facing") {
        Some(facing) => next.with_property("facing", facing),
        None => Some(next),
    }
}

/// A falling block's part in the walk through the blocks it touches: a cobweb and
/// powder snow hold it on its next move, as `makeStuckInBlock` does; the plates,
/// tripwire and detector rails it meets are handed back to the region.
struct FallingInside<'a> {
    body: &'a mut Body,
    inside: Vec<BlockPos>,
}

impl Inside for FallingInside<'_> {
    fn body(&self) -> &Body {
        self.body
    }

    fn removed(&self) -> bool {
        false
    }

    fn met(&mut self, state: BlockState, pos: BlockPos, _step: i32, block: bool, _fluid: bool) {
        if !block {
            return;
        }
        if crate::physics::item::handed_back(state) {
            self.inside.push(pos);
        }
        let hold = if is_class(state, BlockClass::WebBlock) {
            [0.25, f64::from(0.05_f32), 0.25]
        } else if is_class(state, BlockClass::PowderSnowBlock) {
            let side = f64::from(0.9_f32);
            [side, 1.5, side]
        } else {
            return;
        };
        self.body.fall_distance = 0.0;
        self.body.stuck_speed = hold;
    }
}

/// `Level.clip` with `ClipContext.Block.COLLIDER` and `Fluid.SOURCE_ONLY` from `from` to
/// `to`: the first block on the way whose collision or source fluid the segment meets.
fn clip<W: World + ?Sized>(
    world: &W,
    body: &Body,
    from: [f64; 3],
    to: [f64; 3],
) -> Option<BlockPos> {
    let mut found = None;
    let context = body.context();
    physics::walk::traverse(from, to, |pos| {
        let state = world.state(pos);
        let mut boxes: Vec<Aabb> = physics::collide::collision_shape(state, pos, context)
            .map(|(shape, _)| shape.boxes())
            .unwrap_or_default();
        let fluid = FluidState::of(state);
        if fluid.is_source() {
            let height = clustine_blocks::fluid::height(&mut physics::Rules(world), fluid, pos);
            let base = Aabb::of_block(pos);
            boxes.push(Aabb::new(
                base.min,
                [base.max[0], base.min[1] + f64::from(height), base.max[2]],
            ));
        }
        if boxes.iter().any(|shape| clips_shape(shape, from, to)) {
            found = Some(pos);
            return true;
        }
        false
    });
    found
}

/// What becomes of a falling block in its tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    /// It falls on.
    Falls,
    /// It is gone: landed, dropped or broken.
    Gone,
}

impl Region {
    /// `FallingBlockEntity.fall(level, pos, state)`: a falling block of `state` (not
    /// waterlogged) at the centre of the block's floor, still, under the region's next
    /// key with its UUID from its own stream; the block gives way to its fluid with
    /// every update, and then the entity is added. The edges are told of the entity
    /// before the block's change, as the game sends `AddEntity` at once and the tick's
    /// changes of blocks after. `cancel_drop` is `disableDrop`, of suspicious sand and
    /// gravel, which never land or drop. False, and nothing changed, where the region
    /// cannot keep the entity.
    pub(crate) fn fall(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        cancel_drop: bool,
        output: &mut TickOutput,
    ) -> bool {
        let falling = if state.property("waterlogged").is_some() {
            state.with_property("waterlogged", "false").unwrap_or(state)
        } else {
            state
        };
        let position = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        ];
        let chunk = ChunkPos::containing(position[0], position[2]);
        let Some(key) = self.take_entity_key(chunk) else {
            return false;
        };
        let mut own = self.entity_stream(key).random;
        let uuid = insecure_uuid(own.next_u64(), own.next_u64());
        self.entity_drew(key, 2);
        let entity = Entity {
            key,
            kind: u16::try_from(entity_types::FALLING_BLOCK).expect("entity type ids fit 16 bits"),
            uuid,
            position,
            motion: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            hops: 0,
            lived: 0,
            fresh_at: 0,
            data: EntityData::FallingBlock {
                state: falling.0,
                time: 0,
                drop_item: true,
                cancel_drop,
            },
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
        };
        let shown_at = output.events.len();
        self.set_block_updated(pos, fluid_block(state), UPDATE_ALL, UPDATE_LIMIT, output);
        let room = self
            .chunks
            .get(&chunk)
            .is_some_and(|loaded| loaded.entities().len() < clustine_world::MAX_CHUNK_ENTITIES);
        if !room {
            return true;
        }
        self.add_fresh_entity(entity, output);
        // Shown before the air it leaves: the spawn the adding reported last goes back.
        if let Some(spawned) = output.events.pop() {
            output.events.insert(shown_at, spawned);
        }
        true
    }

    /// Whether the block at `pos` has room to fall into below and is within the world
    /// (`FallingBlock.tick`'s test).
    pub(crate) fn may_fall(&self, pos: BlockPos) -> bool {
        let Some(min_y) = self
            .chunks
            .get(&pos.chunk())
            .map(clustine_world::Chunk::min_y)
        else {
            return false;
        };
        let below = BlockPos::new(pos.x, pos.y - 1, pos.z);
        self.block(below).is_some_and(is_free) && pos.y >= min_y
    }

    /// `SpeleothemBlock.spawnFallingStalactite`: from `pos` down, every block of the
    /// stalactite falls, to its tip.
    pub(crate) fn fall_stalactite(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        let mut at = pos;
        let mut current = state;
        while is_stalactite(current) {
            self.fall(at, current, false, output);
            if matches!(current.property("thickness"), Some("tip" | "tip_merge")) {
                break;
            }
            at = BlockPos::new(at.x, at.y - 1, at.z);
            current = self.block(at).unwrap_or(blocks::AIR);
        }
    }

    /// The body of the falling block `entity` in the loaded chunk `position`, as its
    /// record and the region's memory of it have it.
    pub(super) fn falling_body(&self, entity: &Entity, position: ChunkPos) -> Body {
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
            falling_block: true,
            item: false,
            fell_on: None,
            noclip: None,
            mover: self.mover_of(entity, position),
        };
        if memory.is_none() && body.on_ground {
            let view = View {
                region: self,
                home: position,
            };
            body.check_supporting_block(&view, None);
        }
        body
    }

    /// `applyEffectsFromBlocks` of a falling block or of primed TNT, the entity `key`,
    /// whose moves were `movements`: slime under it slows it, lit redstone ore under it
    /// lights, a cobweb or powder snow on its way holds it, and the plates and tripwire
    /// it meets count it where it is (`pressed.rs`).
    pub(super) fn falling_effects(
        &mut self,
        key: EntityId,
        body: &mut Body,
        movements: &[Movement],
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        // What its move posted of a landing (`vibrations.rs`).
        let source = super::vibrations::Source::of(key);
        if let Some((_, on)) = body.fell_on {
            self.entity_hit_ground(source, body.position, on, body.supporting);
        }
        let view = View {
            region: self,
            home: position,
        };
        let mut ore = None;
        if body.on_ground {
            let on = body.on_pos_legacy(&view);
            let under = view.state(on);
            if is_class(under, BlockClass::SlimeBlock) {
                let up = body.motion[1].abs();
                if up < 0.1 {
                    let factor = 0.4 + up * 0.2;
                    let [x, y, z] = body.motion;
                    body.motion = [x * factor, y * 1.0, z * factor];
                }
            } else if is_class(under, BlockClass::RedStoneOreBlock)
                || is_class(under, BlockClass::SculkSensorBlock)
            {
                ore = Some(on);
            }
        }
        let mut inside = FallingInside {
            body,
            inside: Vec::new(),
        };
        check_inside_blocks(&view, &mut inside, movements);
        let met = inside.inside;
        if let Some(ore) = ore {
            self.stood_on(ore, source, body.position, output);
        }
        let toucher = super::pressed::Toucher {
            key,
            bounds: body.bounds(),
            living: false,
        };
        for pos in met {
            self.entity_touched(pos, &toucher, output);
        }
    }

    /// The tick of the falling block `entity`, in the loaded chunk `position`
    /// (`FallingBlockEntity.tick`).
    pub(crate) fn tick_falling_block(
        &mut self,
        entity: Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let EntityData::FallingBlock {
            state,
            time,
            drop_item,
            cancel_drop,
        } = entity.data
        else {
            return;
        };
        let key = entity.key;
        let mut block_state = BlockState(state);
        if block_state.props().is_air() {
            self.remove_entity(key, entity.hops, position, output);
            return;
        }
        // The block as the tick began, which a drop is the item of.
        let block = block_state;
        let time = time.wrapping_add(1);
        let mut cancel_drop = cancel_drop;
        let mut body = self.falling_body(&entity, position);
        let view = View {
            region: self,
            home: position,
        };
        body.apply_gravity();
        let motion = body.motion;
        move_guarded(&mut body, &view, false, motion);
        // An anvil's fall chips it (`causeFallDamage`, from `Block.fallOn` of what it
        // lands on).
        let mut drawn = 0;
        if let Some((fallen, on)) = body.fell_on
            && in_tag(block_state, "minecraft:anvil")
        {
            let under = view.state(on);
            let fallen = if is_class(under, BlockClass::PowderSnowBlock) {
                None
            } else if is_class(under, BlockClass::PointedDripstoneBlock)
                && under.property("vertical_direction") == Some("up")
                && under.property("thickness") == Some("tip")
            {
                Some(fallen + 2.5)
            } else {
                Some(fallen)
            };
            if let Some(fallen) = fallen {
                let ticks = (fallen - 1.0).ceil() as i32;
                let damage = ((ticks as f32 * 2.0).floor() as i32).min(40);
                if ticks >= 0 && damage > 0 {
                    let mut random = self.entity_stream(key).random;
                    drawn += 1;
                    if random.next_float() < 0.05 + ticks as f32 * 0.05 {
                        match damaged_anvil(block_state) {
                            Some(next) => block_state = next,
                            None => cancel_drop = true,
                        }
                    }
                }
            }
        }
        self.entity_drew(key, drawn);
        // `applyEffectsFromBlocks`.
        let movements = tick_movements(&mut body);
        self.falling_effects(key, &mut body, &movements, position, output);
        // A move into a chunk the region does not have loaded ends the tick here: the
        // block is sent, and lands, where it lands, in the region it went to
        // (`arrivals.rs`).
        let to = ChunkPos::containing(body.position[0], body.position[2]);
        if to != position && !self.chunks.contains_key(&to) {
            let after = Entity {
                position: body.position,
                motion: physics::scale(body.motion, f64::from(AIR_DRAG)),
                on_ground: body.on_ground,
                fall_distance: body.fall_distance,
                lived: entity.lived.saturating_add(1),
                data: EntityData::FallingBlock {
                    state: block_state.0,
                    time,
                    drop_item,
                    cancel_drop,
                },
                ..entity.clone()
            };
            self.write_falling_block(&entity, after, 0, position, output);
            return;
        }

        let end = self.after_move(
            &entity,
            &mut body,
            &mut block_state,
            block,
            time,
            drop_item,
            cancel_drop,
            position,
            output,
        );
        body.motion = physics::scale(body.motion, f64::from(AIR_DRAG));
        if end == End::Gone {
            return;
        }
        let after = Entity {
            position: body.position,
            motion: body.motion,
            on_ground: body.on_ground,
            fall_distance: body.fall_distance,
            lived: entity.lived.saturating_add(1),
            data: EntityData::FallingBlock {
                state: block_state.0,
                time,
                drop_item,
                cancel_drop,
            },
            ..entity.clone()
        };
        self.entity_memory.insert(
            key,
            Memory {
                supporting: body.supporting,
                on_ground_no_blocks: body.on_ground_no_blocks,
                last_movements: Vec::new(),
                stuck_speed: body.stuck_speed,
                no_physics: false,
                ticks_since_check: 0,
                cart: Default::default(),
                boat: Default::default(),
            },
        );
        let mut flags = 0;
        if body.horizontal_collision {
            flags |= Tracked::HORIZONTAL_COLLISION;
        }
        if body.vertical_collision {
            flags |= Tracked::VERTICAL_COLLISION;
        }
        // `setRequiresPrecisePosition(horizontalCollision || verticalCollision)`.
        if body.horizontal_collision || body.vertical_collision {
            flags |= Tracked::PRECISE;
        }
        self.write_falling_block(&entity, after, flags, position, output);
    }

    /// The server's part of the tick, after the move: landing on the ground or, as
    /// concrete powder, in water; or the drop of a block that has fallen too long.
    #[allow(clippy::too_many_arguments, reason = "the game's fields of the entity")]
    fn after_move(
        &mut self,
        entity: &Entity,
        body: &mut Body,
        block_state: &mut BlockState,
        block: BlockState,
        time: i32,
        drop_item: bool,
        cancel_drop: bool,
        position: ChunkPos,
        output: &mut TickOutput,
    ) -> End {
        let mut pos = body.block_position();
        let concrete = concrete_of(*block_state).is_some();
        let mut in_water = concrete && self.block(pos).is_some_and(has_water);
        let speed = physics::length_sqr(body.motion);
        if concrete && speed > 1.0 {
            let view = View {
                region: self,
                home: position,
            };
            if let Some(hit) = clip(&view, body, body.old_position, body.position)
                && has_water(view.state(hit))
            {
                pos = hit;
                in_water = true;
            }
        }
        let gone = |region: &mut Self, output: &mut TickOutput| {
            region.remove_entity(entity.key, entity.hops, position, output);
        };
        if body.on_ground || in_water {
            let here = self.block(pos).unwrap_or(blocks::AIR);
            let [x, y, z] = body.motion;
            body.motion = [x * 0.7, y * -0.5, z * 0.7];
            if is_class(here, BlockClass::MovingPistonBlock) {
                // It waits on the moving block, and lands next tick.
                return End::Falls;
            }
            if cancel_drop {
                gone(self, output);
                self.broken_after_fall(block, *block_state, pos, body, output);
                return End::Gone;
            }
            let replaceable = crate::placement::can_be_replaced_empty_handed(here);
            let below = self
                .block(BlockPos::new(pos.x, pos.y - 1, pos.z))
                .unwrap_or(blocks::AIR);
            let below_free = is_free(below) && (!concrete || !in_water);
            let survives = can_survive(*block_state, pos, &mut RegionView(self)) && !below_free;
            if replaceable && survives {
                if block_state.property("waterlogged").is_some() && {
                    let fluid = FluidState::of(here);
                    fluid.is_source() && has_water(here)
                } {
                    *block_state = block_state
                        .with_property("waterlogged", "true")
                        .unwrap_or(*block_state);
                }
                if self.set_block_updated(pos, *block_state, UPDATE_ALL, UPDATE_LIMIT, output) {
                    let shown = self.block(pos).unwrap_or(blocks::AIR);
                    output.events.push(RegionEvent::TransientBlock {
                        position: pos,
                        state: shown.0,
                    });
                    gone(self, output);
                    self.on_land(*block_state, pos, here, output);
                    return End::Gone;
                }
                if drop_item {
                    gone(self, output);
                    self.broken_after_fall(block, *block_state, pos, body, output);
                    self.drop_falling_block(block, body.position, output);
                    return End::Gone;
                }
                return End::Falls;
            }
            gone(self, output);
            if drop_item {
                self.broken_after_fall(block, *block_state, pos, body, output);
                self.drop_falling_block(block, body.position, output);
            }
            return End::Gone;
        }
        let (min_y, max_y) = self.chunks.get(&position).map_or((-64, 319), |chunk| {
            let height = i32::try_from(chunk.height()).unwrap_or(384);
            (chunk.min_y(), chunk.min_y() + height - 1)
        });
        if (time > 100 && (pos.y <= min_y || pos.y > max_y)) || time > 600 {
            if drop_item {
                self.drop_falling_block(block, body.position, output);
            }
            gone(self, output);
            return End::Gone;
        }
        End::Falls
    }

    /// `Fallable.onLand`: concrete powder hardens where water is, an anvil clangs.
    fn on_land(
        &mut self,
        state: BlockState,
        pos: BlockPos,
        replaced: BlockState,
        output: &mut TickOutput,
    ) {
        if let Some(concrete) = concrete_of(state) {
            if clustine_blocks::fluid::powder_hardens(&mut RegionView(self), pos, replaced) {
                self.set_block(pos, concrete, output);
            }
        } else if in_tag(state, "minecraft:anvil") {
            output.events.push(RegionEvent::LevelEvent {
                kind: ANVIL_LANDED,
                position: pos,
                data: 0,
            });
        }
    }

    /// `callOnBrokenAfterFall`: what clients are shown of a falling block that breaks
    /// where it lands, by its block as the tick began.
    fn broken_after_fall(
        &mut self,
        block: BlockState,
        state: BlockState,
        pos: BlockPos,
        body: &Body,
        output: &mut TickOutput,
    ) {
        if in_tag(block, "minecraft:anvil") {
            output.events.push(RegionEvent::LevelEvent {
                kind: ANVIL_DESTROYED,
                position: pos,
                data: 0,
            });
        } else if is_class(block, BlockClass::PointedDripstoneBlock) {
            output.events.push(RegionEvent::LevelEvent {
                kind: DRIPSTONE_LANDED,
                position: pos,
                data: 0,
            });
        } else if is_class(block, BlockClass::SpeleothemBlock) {
            output.events.push(RegionEvent::LevelEvent {
                kind: SULFUR_SPIKE_LANDED,
                position: pos,
                data: 0,
            });
        } else if is_class(block, BlockClass::BrushableBlock) {
            // At the centre of its box, of its state now.
            let bounds = body.bounds();
            let centre = [
                (bounds.min[0] + bounds.max[0]) / 2.0,
                (bounds.min[1] + bounds.max[1]) / 2.0,
                (bounds.min[2] + bounds.max[2]) / 2.0,
            ];
            output.events.push(RegionEvent::BlockDestroyed {
                position: physics::containing(centre),
                state,
                breaker: None,
            });
        }
    }

    /// `spawnAtLocation(level, block)`: one item of the block, where the entity is.
    fn drop_falling_block(&mut self, block: BlockState, at: [f64; 3], output: &mut TickOutput) {
        if let Some(item) = super::drops::item_of_block(block) {
            let stack = clustine_world::ItemStack { item, count: 1 };
            self.spawn_item(at, stack, output);
        }
    }

    /// What the falling block's tick leaves: its record, its commit, the edges' report.
    /// Its motion and its time commit only with a change of its position, its being on
    /// the ground or its block; its chunk ages ([`TickOutput::entities_ageing`]).
    pub(super) fn write_falling_block(
        &mut self,
        before: &Entity,
        after: Entity,
        flags: u8,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let key = before.key;
        let mut after = after;
        let to = match self.relocate(position, before.position, &mut after, output) {
            Relocated::Sent => return,
            Relocated::Moved(to) => to,
            Relocated::Stays => position,
        };
        let bits = |values: [f64; 3]| values.map(f64::to_bits);
        let moved = bits(after.position) != bits(before.position);
        let block_of = |data: &EntityData| match data {
            EntityData::FallingBlock {
                state,
                drop_item,
                cancel_drop,
                ..
            } => Some((*state, *drop_item, *cancel_drop)),
            _ => None,
        };
        let committed = moved
            || after.on_ground != before.on_ground
            || block_of(&after.data) != block_of(&before.data);
        if to == position {
            if committed {
                let mut fields = Vec::new();
                if moved {
                    fields.push(EntityField::Position(after.position));
                }
                if bits(after.motion) != bits(before.motion) {
                    fields.push(EntityField::Motion(after.motion));
                }
                if after.on_ground != before.on_ground {
                    fields.push(EntityField::OnGround(after.on_ground));
                }
                if after.fall_distance.to_bits() != before.fall_distance.to_bits() {
                    fields.push(EntityField::FallDistance(after.fall_distance));
                }
                if after.data != before.data {
                    fields.push(EntityField::Data(after.data.clone()));
                }
                output
                    .entity_changes
                    .push((position, EntityChange::Set(key, fields)));
            }
            if let Some(chunk) = self.chunks.get_mut(&position) {
                chunk.apply_entity(&EntityChange::Whole(after.clone()));
            }
        }
        output.entities_ageing.push(to);
        let changed_block = block_of(&after.data) != block_of(&before.data);
        if committed || to != position {
            let mut report = tracked(&after, flags, changed_block);
            // A passenger is its unit's chunk's, wherever it sits.
            report.chunk = to;
            output.events.push(RegionEvent::EntityTracked(report));
        }
    }
}

/// `SpeleothemBlock.isStalactite`: a speleothem pointing down.
pub(crate) fn is_stalactite(state: BlockState) -> bool {
    in_tag(state, "minecraft:speleothems") && state.property("vertical_direction") == Some("down")
}

/// `SpeleothemBlock.isStalagmite`: a speleothem pointing up.
pub(crate) fn is_stalagmite(state: BlockState) -> bool {
    in_tag(state, "minecraft:speleothems") && state.property("vertical_direction") == Some("up")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_anvil_chips_then_is_damaged_then_breaks_keeping_its_facing() {
        let anvil = BlockState::parse("minecraft:anvil[facing=east]").unwrap();
        let chipped = damaged_anvil(anvil).unwrap();
        assert_eq!(
            chipped,
            BlockState::parse("minecraft:chipped_anvil[facing=east]").unwrap()
        );
        let damaged = damaged_anvil(chipped).unwrap();
        assert_eq!(
            damaged,
            BlockState::parse("minecraft:damaged_anvil[facing=east]").unwrap()
        );
        assert_eq!(damaged_anvil(damaged), None);
    }

    #[test]
    fn a_dragon_egg_waits_five_ticks_and_other_falling_blocks_two() {
        assert_eq!(delay_after_place(blocks::DRAGON_EGG), 5);
        assert_eq!(delay_after_place(blocks::SAND), 2);
        assert_eq!(delay_after_place(blocks::ANVIL), 2);
    }

    #[test]
    fn a_block_falls_into_air_fire_liquids_and_what_can_be_replaced() {
        for free in [
            blocks::AIR,
            blocks::FIRE,
            blocks::WATER,
            blocks::LAVA,
            blocks::SHORT_GRASS,
        ] {
            assert!(is_free(free), "{free:?}");
        }
        for held in [blocks::STONE, blocks::TORCH, blocks::STONE_SLAB] {
            assert!(!is_free(held), "{held:?}");
        }
    }
}
