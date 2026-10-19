//! Entities pushed by pistons (`docs/groundwork/n4-plan.md`, step N4f): each tick of a
//! moving piston's block entity, before its progress moves on, the game's
//! `PistonMovingBlockEntity.moveCollidedEntities` pushes the entities in the way of the
//! block (a retracting piston's base pushing with its head) as far as the block goes,
//! and a hundredth more, a slime block launching what it pushes at a block a tick the
//! way it goes; and `moveStuckEntities` carries what stands on a honey block that moves
//! sideways. Each push is `Entity.move(MoverType.PISTON, …)`: the pistons of one tick
//! move an entity at most 0.51 along an axis (`limitPistonMovement`), the moving blocks
//! going the same way do not stop it, and the effects of the blocks on its way follow.
//!
//! A block-attached entity (a frame, a painting, a cushion, a knot) in the way is popped,
//! as any move of it breaks it (`BlockAttachedEntity.move`).
//!
//! The entities are the items and falling blocks of the region's chunks; a push commits
//! as their own moves do (a change of place, of being on the ground, commits the record
//! with its motion), one pushed into a chunk the region has not loaded is sent there
//! (`arrivals.rs`), and a push after their tick is reported to the edges as their moves
//! are. Players are pushed by
//! their own clients, which run the same moves on the moving pistons they are sent
//! (`pistons.rs`); the region leaves them alone.
//!
//! Named differences: an entity in a chunk the region holds but does not tick, or in
//! another region's chunk across a line, is not pushed (the game pushes whatever is
//! loaded about the block); the boxes a block pushes with are its shape's cells, one
//! each, where the game joins them into larger boxes, which pushes alike unless an
//! entity is already inside the block; a moved block whose shape shifts by its position
//! (pointed dripstone) collides without its shift; a moving piston in a copied chunk,
//! whose block entity the region does not keep, collides with nothing; and a falling
//! block's landing test (`Level.clip` with the collider's shapes) does not see moving
//! pistons. An entity pushed after its own tick is reported twice in the tick, and the
//! edge keeps the later report, as the game's `sendChanges` sees the entity after the
//! block entities.

use clustine_world::{BlockPos, ChunkPos, Entity, EntityData, EntityId};

use super::Region;
use super::attached::is_block_attached;
use super::block_entities::Moving;
use super::items::{Memory, View};
use crate::api::{TickOutput, Tracked};
use crate::physics::Vec3;
use crate::physics::aabb::Aabb;
use crate::physics::body::{Body, Movement};
use crate::physics::item::{Item, move_guarded_as};
use crate::physics::piston::{
    self, MovingBlock, PUSH_OFFSET, boxes_of, is_honey, is_slime, movement, movement_area,
};
use clustine_data::Face;

/// `AABB(0, top, 0, 1, 1.5000010000000001, 1)`'s top: how high above a honey block
/// what it carries may reach.
const STUCK_TOP: f64 = 1.500_001_000_000_000_1;

/// An entity as a piston moves it.
enum Pushed {
    Item(Box<Item>),
    Falling(Box<Body>),
}

impl Pushed {
    fn body(&self) -> &Body {
        match self {
            Pushed::Item(item) => &item.body,
            Pushed::Falling(body) => body,
        }
    }

    fn body_mut(&mut self) -> &mut Body {
        match self {
            Pushed::Item(item) => &mut item.body,
            Pushed::Falling(body) => body,
        }
    }
}

impl Region {
    /// The pushes of the moving piston at `pos`, whose block entity is `moving`, as its
    /// tick takes its progress on by half a block (`PistonMovingBlockEntity.tick`).
    pub(super) fn push_entities(
        &mut self,
        pos: BlockPos,
        moving: &Moving,
        output: &mut TickOutput,
    ) {
        let block = MovingBlock {
            moved: moving.moved,
            direction: moving.direction,
            extending: moving.extending,
            source: moving.source,
            progress: f32::from(moving.progress) / 2.0,
        };
        // `f = progress + 0.5f`, and the double of `f - progress`.
        let next = block.progress + 0.5;
        let d = f64::from(next - block.progress);
        self.move_collided_entities(pos, &block, d, output);
        self.move_stuck_entities(pos, &block, d, output);
    }

    /// `moveCollidedEntities`: the entities whose boxes meet the slab the block sweeps
    /// this tick are pushed as far as they reach into it, at most `d`, and a hundredth
    /// more; a slime block sets their motion along the way it goes to a block a tick,
    /// and a retracting base puts out of itself what it has drawn in.
    fn move_collided_entities(
        &mut self,
        pos: BlockPos,
        block: &MovingBlock,
        d: f64,
        output: &mut TickOutput,
    ) {
        let direction = block.movement_direction();
        let Some((boxes, bounds)) = boxes_of(block.collision_related_state()) else {
            return;
        };
        let offset = block.offset(pos);
        let swept = bounds.moved(offset);
        let reach = movement_area(swept, direction, d).minmax(swept);
        let found = self.pushable_in(&reach);
        if found.is_empty() {
            return;
        }
        let slime = is_slime(block.moved);
        let axis = piston::axis(direction);
        let along = f64::from(piston::step(direction)[axis]);
        for (record, chunk) in found {
            if is_block_attached(&record.data) {
                // `BlockAttachedEntity.move`: a push of any length pops it.
                let entity = self.entity_box(&record);
                let reached = boxes
                    .iter()
                    .map(|one| movement_area(one.moved(offset), direction, d))
                    .filter(|area| area.intersects(entity))
                    .map(|area| movement(area, direction, entity))
                    .fold(0.0_f64, f64::max);
                if reached > 0.0 {
                    let amount = reached.min(d) + PUSH_OFFSET;
                    let step = piston::step(direction).map(f64::from);
                    self.moved_attached(record.key, step.map(|along| along * amount), output);
                }
                continue;
            }
            let Some(mut pushed) = self.pushed_of(&record, chunk) else {
                continue;
            };
            // `canSimulateMovement` is true on the server for both.
            if slime {
                pushed.body_mut().motion[axis] = along;
            }
            let mut reached = 0.0_f64;
            for one in &boxes {
                let area = movement_area(one.moved(offset), direction, d);
                let entity = pushed.body().bounds();
                if !area.intersects(entity) {
                    continue;
                }
                reached = reached.max(movement(area, direction, entity));
                if reached >= d {
                    break;
                }
            }
            if reached > 0.0 {
                let amount = reached.min(d) + PUSH_OFFSET;
                self.move_by_piston(
                    &mut pushed,
                    record.key,
                    direction,
                    amount,
                    direction,
                    chunk,
                    output,
                );
                if !block.extending && block.source {
                    self.fix_within_piston_base(
                        pos,
                        &mut pushed,
                        record.key,
                        direction,
                        d,
                        chunk,
                        output,
                    );
                }
            }
            self.write_pushed(&record, pushed, chunk, output);
        }
    }

    /// `fixEntityWithinPistonBase`: an entity a retracting piston's base has drawn in,
    /// and that is in the base's block by as much as it is in the block at all, is put
    /// out the way the piston faces.
    #[allow(
        clippy::too_many_arguments,
        reason = "the game's arguments and the region's"
    )]
    fn fix_within_piston_base(
        &mut self,
        pos: BlockPos,
        pushed: &mut Pushed,
        key: EntityId,
        direction: Face,
        d: f64,
        chunk: ChunkPos,
        output: &mut TickOutput,
    ) {
        let entity = pushed.body().bounds();
        let block = Aabb::of_block(pos);
        if !entity.intersects(block) {
            return;
        }
        let out = direction.opposite();
        let mut whole = movement(block, out, entity) + PUSH_OFFSET;
        let within = movement(block, out, entity.intersect(block)) + PUSH_OFFSET;
        if (whole - within).abs() < PUSH_OFFSET {
            whole = whole.min(d) + PUSH_OFFSET;
            self.move_by_piston(pushed, key, direction, whole, out, chunk, output);
        }
    }

    /// `moveStuckEntities`: a honey block going sideways carries `d` along what stands
    /// on it: on the ground, and on it or over it.
    fn move_stuck_entities(
        &mut self,
        pos: BlockPos,
        block: &MovingBlock,
        d: f64,
        output: &mut TickOutput,
    ) {
        if !is_honey(block.moved) {
            return;
        }
        let direction = block.movement_direction();
        if piston::axis(direction) == 1 {
            return;
        }
        let Some((_, bounds)) = boxes_of(block.moved) else {
            return;
        };
        let area =
            Aabb::new([0.0, bounds.max[1], 0.0], [1.0, STUCK_TOP, 1.0]).moved(block.offset(pos));
        // The game finds them all before it moves any.
        let stuck: Vec<(Entity, ChunkPos, Pushed)> = self
            .pushable_in(&area)
            .into_iter()
            .filter_map(|(record, chunk)| {
                let pushed = self.pushed_of(&record, chunk)?;
                let body = pushed.body();
                let [x, _, z] = body.position;
                let over =
                    x >= area.min[0] && x <= area.max[0] && z >= area.min[2] && z <= area.max[2];
                (body.on_ground && (body.supporting == Some(pos) || over))
                    .then_some((record, chunk, pushed))
            })
            .collect();
        for (record, chunk, mut pushed) in stuck {
            self.move_by_piston(
                &mut pushed,
                record.key,
                direction,
                d,
                direction,
                chunk,
                output,
            );
            self.write_pushed(&record, pushed, chunk, output);
        }
    }

    /// `Entity.move(MoverType.SHULKER_BOX, by)` of what the region ticks whose boxes meet
    /// `bounds`, as a shulker box's lid opening moves it (`shulker_boxes.rs`): a plain
    /// move, with none of a piston's limits, of items and falling blocks, and a
    /// block-attached entity popped as any move of it pops it. What the blocks along
    /// the way would do to what moves is not done.
    pub(super) fn push_by_shulker_box(&mut self, bounds: &Aabb, by: Vec3, output: &mut TickOutput) {
        for (record, chunk) in self.pushable_in(bounds) {
            if is_block_attached(&record.data) {
                self.moved_attached(record.key, by, output);
                continue;
            }
            let Some(mut pushed) = self.pushed_of(&record, chunk) else {
                continue;
            };
            let no_physics = matches!(&pushed, Pushed::Item(item) if item.no_physics);
            let view = View {
                region: self,
                home: chunk,
            };
            let body = pushed.body_mut();
            move_guarded_as(body, &view, no_physics, by, None);
            body.movements.clear();
            self.write_pushed(&record, pushed, chunk, output);
        }
    }

    /// The records of the items and falling blocks of the chunks the region ticks whose
    /// boxes meet `bounds`, in the order of the game's lookups, each with its chunk.
    fn pushable_in(&self, bounds: &Aabb) -> Vec<(Entity, ChunkPos)> {
        self.entities_in(bounds)
            .into_iter()
            .filter(|(_, chunk)| self.ticks_chunk(*chunk))
            .filter_map(|(key, chunk)| Some((self.entity_record(key, chunk)?.clone(), chunk)))
            .collect()
    }

    /// The entity of `record` in the chunk `chunk`, as a piston finds it.
    fn pushed_of(&self, record: &Entity, chunk: ChunkPos) -> Option<Pushed> {
        match record.data {
            EntityData::Item { .. } => Some(Pushed::Item(Box::new(self.item_of(record, chunk)?))),
            EntityData::FallingBlock { .. } => {
                Some(Pushed::Falling(Box::new(self.falling_body(record, chunk))))
            }
            _ => None,
        }
    }

    /// `moveEntityByPiston(noclip, entity, amount, towards)`: `move(PISTON, …)` by
    /// `amount` towards `towards`, the moving blocks going `noclip` letting it through,
    /// then the effects of the blocks on its way.
    #[allow(
        clippy::too_many_arguments,
        reason = "the game's arguments and the region's"
    )]
    fn move_by_piston(
        &mut self,
        pushed: &mut Pushed,
        key: EntityId,
        noclip: Face,
        amount: f64,
        towards: Face,
        chunk: ChunkPos,
        output: &mut TickOutput,
    ) {
        let [x, y, z] = piston::step(towards);
        let vector = [
            amount * f64::from(x),
            amount * f64::from(y),
            amount * f64::from(z),
        ];
        let mut deltas = self.piston_deltas.get(&key).copied().unwrap_or_default();
        match pushed {
            Pushed::Item(item) => {
                let mut stream = self.entity_stream(key);
                let view = View {
                    region: self,
                    home: chunk,
                };
                item.pushed_by_piston(&view, &mut stream, noclip, vector, &mut deltas);
                if stream.used > 0 {
                    *self.entity_draws.entry(key).or_default() += stream.used;
                }
                self.item_side_effects(item, output);
            }
            Pushed::Falling(body) => {
                body.noclip = Some(noclip);
                let from = body.position;
                let view = View {
                    region: self,
                    home: chunk,
                };
                move_guarded_as(body, &view, false, vector, Some(&mut deltas));
                let movement = Movement {
                    from,
                    to: body.position,
                    asked: None,
                };
                self.falling_effects(body, &[movement], chunk, output);
                body.movements.clear();
                body.noclip = None;
            }
        }
        self.piston_deltas.insert(key, deltas);
    }

    /// What a push leaves of the entity of `before`, committed as its own moves are.
    fn write_pushed(
        &mut self,
        before: &Entity,
        pushed: Pushed,
        chunk: ChunkPos,
        output: &mut TickOutput,
    ) {
        match pushed {
            Pushed::Item(item) => self.write_item(before, *item, chunk, output),
            Pushed::Falling(body) => {
                let after = Entity {
                    position: body.position,
                    motion: body.motion,
                    on_ground: body.on_ground,
                    fall_distance: body.fall_distance,
                    ..before.clone()
                };
                self.entity_memory.insert(
                    before.key,
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
                if body.horizontal_collision || body.vertical_collision {
                    flags |= Tracked::PRECISE;
                }
                self.write_falling_block(before, after, flags, chunk, output);
            }
        }
    }
}
