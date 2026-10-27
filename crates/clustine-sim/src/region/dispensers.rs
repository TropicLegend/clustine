//! Dispensers and droppers (`DispenserBlock`, `DropperBlock`, `DispenserBlockEntity` and
//! the behaviours of `net.minecraft.core.dispenser`; `docs/groundwork/n6-brief.md`,
//! section 4; step N6d of `docs/groundwork/n6-plan.md`): the block fires four ticks after
//! a rising edge of its power, read beside it and beside the block above it, and its
//! tick draws a slot that holds something and runs that item's behaviour
//! (`behaviours.rs`); a dropper drops one item, or puts it into the container it faces.
//!
//! What the game draws from the level's random in a dispense (the slot, an item's speed
//! and spread, the discarded yaw of an armour stand, the splashes of a water bottle)
//! comes from the dispenser's own stream in the tick (`Random::of_block`, kept on as
//! `drops.rs` keeps it), in the game's order; what it draws from a new entity's own
//! random comes from the entity's key, as a thrown item's does. A primed TNT draws from
//! the stream of the block it is primed in, as `tnt.rs` primes any. The distributions
//! are the game's, the values not.
//!
//! The slots are the block entity's, read from its data (`containers.rs`) at the start
//! of a dispense and written back whole at its end, as the game's `setItem` does, which
//! tells the comparators reading it; a change of what it holds is a circuit at work
//! (`docs/groundwork/n6-plan.md`, section 10 item 5).
//!
//! At a line (section 10 items 1 and 7 of the plan): a behaviour that reads or changes
//! the block in front, or the entities in it, where that block is in a chunk the region
//! does not hold fails as an empty dispense fails, with its click (1001) and nothing
//! else, the item kept; a dropper facing a container there fails so too. Lending land
//! to dispensers at a pinned line is step N6g's. What only makes an entity there (an
//! item dropped, a TNT primed, an armour stand) makes it in this region and sends it as
//! an arrival, as a throw does.
//!
//! Named differences: a structure's loot table is cleared as the game unpacks it but
//! rolls nothing (decision 4); the game rule `tnt_explodes` is always true; a dropper
//! facing a
//! container block whose slots Clustine does not keep (a furnace, a brewing stand, a
//! composter, a crafter, a chiseled bookshelf, a decorated pot, a jukebox, a shelf)
//! keeps its item as it would facing a full one. The rest are the behaviours' own
//! (`behaviours.rs`).

use clustine_blocks::data::{facing, is_class, relative, tick_id};
use clustine_data::{BlockClass, BlockState, Face, entity_types, sounds};
use clustine_world::{BlockPos, ChunkPos, Entity, EntityData, EntityField, EntityId, Ticked};

use super::Region;
use super::containers::container_size;
use super::entities::insecure_uuid;
use super::random_ticks::Random;
use super::updates::{UPDATE_CLIENTS, UPDATE_LIMIT};
use crate::api::{ItemStack, RegionEvent, SOUND_SOURCE_BLOCKS, TickOutput};
use crate::physics::aabb::Aabb;

mod behaviours;
mod projectiles;
#[cfg(test)]
mod tests;

/// Mixed into a dispenser's stream for the draws of its dispenses.
const DISPENSING: u64 = 0x6469_7370_656e_7365;

/// `DispenserBlock.TRIGGER_DURATION`: the ticks from a rising edge to the dispense.
pub(crate) const TRIGGER_DURATION: i32 = 4;

/// `LevelEvent.SOUND_DISPENSER_DISPENSE`: the click of something dispensed.
pub(crate) const DISPENSED: i32 = 1000;
/// `LevelEvent.SOUND_DISPENSER_FAIL`: the click of nothing dispensed.
pub(crate) const FAILED: i32 = 1001;
/// `LevelEvent.PARTICLES_SHOOT_SMOKE`, whose data is the face the smoke goes out of.
pub(crate) const SHOOT_SMOKE: i32 = 2000;
/// `LevelEvent.PARTICLES_WAX_ON`: honeycomb's sparks on a block it waxed.
pub(crate) const WAX_ON: i32 = 3003;

/// `DefaultDispenseItemBehavior.DEFAULT_ACCURACY`: how widely a dropped item spreads.
const ACCURACY: i32 = 6;

/// The most any container slot holds (`Container.getMaxStackSize`'s 99), which a
/// dispenser does not change.
const CONTAINER_MAX: i32 = 99;

/// `DispenserBlockEntity.getRandomSlot`: of the slots that hold something, each draws
/// `next_int(n)` with `n` counting up from 1, and the last to draw 0 is chosen (reservoir
/// sampling: each as likely). `None`, and no draw, where every slot is empty.
pub(crate) fn random_slot(
    slots: &[Option<ItemStack>],
    mut next_int: impl FnMut(u32) -> u32,
) -> Option<usize> {
    let mut chosen = None;
    let mut seen = 1;
    for (slot, stack) in slots.iter().enumerate() {
        if !holds(stack.as_ref()) {
            continue;
        }
        if next_int(seen) == 0 {
            chosen = Some(slot);
        }
        seen += 1;
    }
    chosen
}

/// Whether a slot holds anything (`!ItemStack.isEmpty()`).
fn holds(stack: Option<&ItemStack>) -> bool {
    stack.is_some_and(|stack| stack.count > 0 && stack.item != clustine_data::items::AIR)
}

/// `DispenserBlock.getDispensePosition`: seven tenths of a block out of the middle of
/// the dispenser at `pos`, towards `facing`.
pub(crate) fn dispense_position(pos: BlockPos, facing: Face) -> [f64; 3] {
    let [dx, dy, dz] = crate::physics::piston::step(facing);
    [
        f64::from(pos.x) + 0.5 + 0.7 * f64::from(dx),
        f64::from(pos.y) + 0.5 + 0.7 * f64::from(dy),
        f64::from(pos.z) + 0.5 + 0.7 * f64::from(dz),
    ]
}

/// Where `DefaultDispenseItemBehavior.spawnItem` puts the item it drops from `position`
/// going `facing`: an eighth of a block lower for a dispenser facing up or down, five
/// thirty-seconds for one facing sideways.
pub(crate) fn drop_position(position: [f64; 3], facing: Face) -> [f64; 3] {
    let lower = if matches!(facing, Face::Up | Face::Down) {
        0.125
    } else {
        0.156_25
    };
    [position[0], position[1] - lower, position[2]]
}

/// `RandomSource.triangle(mean, deviation)`: two draws, the second taken from the first.
fn triangle(mean: f64, deviation: f64, next_double: &mut impl FnMut() -> f64) -> f64 {
    let first = next_double();
    let second = next_double();
    mean + deviation * (first - second)
}

/// The motion `spawnItem` sets on an item it drops going `facing`, from seven level
/// draws: a speed of 0.2 to 0.3 along the facing, a fifth of a block a tick up, each
/// spread by a triangle of `0.0172275 * 6` (DefaultDispenseItemBehavior.txt 57–135).
pub(crate) fn drop_motion(facing: Face, next_double: &mut impl FnMut() -> f64) -> [f64; 3] {
    let [dx, _, dz] = crate::physics::piston::step(facing);
    let speed = next_double() * 0.1 + 0.2;
    let deviation = 0.017_227_5 * f64::from(ACCURACY);
    let x = triangle(f64::from(dx) * speed, deviation, next_double);
    let y = triangle(0.2, deviation, next_double);
    let z = triangle(f64::from(dz) * speed, deviation, next_double);
    [x, y, z]
}

/// `Direction.toYRot()`: south 0, west 90, north 180, east 270, and up and down 270,
/// as their two-dimensional value of -1 taken with `& 3` is east's.
pub(crate) fn y_rot(facing: Face) -> f32 {
    match facing {
        Face::South => 0.0,
        Face::West => 90.0,
        Face::North => 180.0,
        Face::East | Face::Up | Face::Down => 270.0,
    }
}

/// `DispenserBlockEntity.insertItem`: `stack` put into the first slots that are empty or
/// hold the same item with the same components, as much as each takes (at most 99 and the item's own most), until
/// none is left. What is left, if anything.
pub(crate) fn insert_item(slots: &mut [Option<ItemStack>], stack: ItemStack) -> Option<ItemStack> {
    let most = crate::physics::item::max_stack_size(&stack).min(CONTAINER_MAX);
    let mut left = stack.count;
    for place in slots.iter_mut() {
        if left <= 0 {
            break;
        }
        let held = place.as_ref().filter(|held| holds(Some(held)));
        if held.as_ref().is_some_and(|held| !held.same_kind(&stack)) {
            continue;
        }
        let already = held.map_or(0, |held| held.count);
        let moved = left.min(most - already);
        if moved > 0 {
            *place = Some(stack.with_count(already + moved));
            left -= moved;
        }
    }
    (left > 0).then_some(stack.with_count(left))
}

/// A dispense under way: the block's place, the way it faces, its slots as the dispense
/// has left them so far, the slot chosen, whose stack the behaviours change in place as
/// the game's change the slot's own object, and the stream of the level's draws.
pub(crate) struct Dispense {
    pub(crate) pos: BlockPos,
    pub(crate) facing: Face,
    pub(crate) slots: Vec<Option<ItemStack>>,
    pub(crate) slot: usize,
    pub(crate) random: Random,
}

impl Dispense {
    /// The block in front (`pos.relative(facing)`).
    pub(crate) fn front(&self) -> BlockPos {
        relative(self.pos, self.facing)
    }

    /// The stack being dispensed, as it is now.
    pub(crate) fn stack(&self) -> Option<ItemStack> {
        self.slots[self.slot]
            .as_ref()
            .filter(|stack| holds(Some(stack)))
            .cloned()
    }

    /// `stack.shrink(count)`.
    pub(crate) fn shrink(&mut self, count: i32) {
        if let Some(stack) = self.slots[self.slot].as_mut() {
            stack.count -= count;
        }
        if !holds(self.slots[self.slot].as_ref()) {
            self.slots[self.slot] = None;
        }
    }

    /// `stack.split(1)`: one of the stack, taken from it.
    pub(crate) fn split_one(&mut self) -> Option<ItemStack> {
        let one = self.stack().map(|stack| ItemStack { count: 1, ..stack })?;
        self.shrink(1);
        Some(one)
    }

    /// The stream of the level's draws as a closure of `nextDouble`.
    fn next_double(&mut self) -> impl FnMut() -> f64 + '_ {
        || self.random.next_double()
    }
}

/// What a dropper puts its item into (`HopperBlockEntity.getContainerAt`).
enum Into {
    /// Nothing: the item is dropped.
    Nothing,
    /// A container block, a double chest's two halves, the right one first.
    Block(Vec<BlockPos>),
    /// A container vehicle.
    Entity(EntityId, ChunkPos),
    /// A container block whose slots Clustine does not keep: the item stays.
    Unkept,
    /// A container in a chunk the region does not hold: the dropper fails.
    Across,
}

/// Whether `state` is a container block in the game whose slots Clustine does not keep.
fn is_unkept_container(state: BlockState) -> bool {
    [
        BlockClass::AbstractFurnaceBlock,
        BlockClass::BrewingStandBlock,
        BlockClass::ComposterBlock,
        BlockClass::CrafterBlock,
        BlockClass::ChiseledBookShelfBlock,
        BlockClass::DecoratedPotBlock,
        BlockClass::JukeboxBlock,
        BlockClass::ShelfBlock,
    ]
    .into_iter()
    .any(|class| is_class(state, class))
}

impl Region {
    /// `DispenserBlock.neighborChanged` of a dispenser or a dropper at `pos`: power beside
    /// it or beside the block above it (quasi-connectivity) that rises schedules its tick
    /// in four and sets it `triggered`, told to the clients alone (flags 2); power that
    /// goes clears it. Whether `state` is one. A dispenser placed beside power waits for
    /// an update, as the game's has no `onPlace`.
    pub(super) fn dispenser_neighbour_changed(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) -> bool {
        if !is_class(state, BlockClass::DispenserBlock) {
            return false;
        }
        let powered = {
            let mut view = super::updates::RegionView(self);
            clustine_blocks::signal::has_neighbour_signal(&mut view, pos)
                || clustine_blocks::signal::has_neighbour_signal(&mut view, relative(pos, Face::Up))
        };
        let triggered = clustine_blocks::data::flag(state, "triggered");
        if powered && !triggered {
            self.schedule_tick(pos, Ticked::Block(tick_id(state)), TRIGGER_DURATION, 0);
            let set = clustine_blocks::data::set_flag(state, "triggered", true);
            self.set_block_updated(pos, set, UPDATE_CLIENTS, UPDATE_LIMIT, output);
        } else if !powered && triggered {
            let set = clustine_blocks::data::set_flag(state, "triggered", false);
            self.set_block_updated(pos, set, UPDATE_CLIENTS, UPDATE_LIMIT, output);
        }
        true
    }

    /// `DispenserBlock.tick` and the dropper's: the dispense, whatever the power is by
    /// now. Whether `state` is a dispenser or a dropper.
    pub(super) fn dispenser_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) -> bool {
        if !is_class(state, BlockClass::DispenserBlock) {
            return false;
        }
        if self.own_block_entity(pos) {
            if is_class(state, BlockClass::DropperBlock) {
                self.drop_from(pos, state, output);
            } else {
                self.dispense_from(pos, state, output);
            }
        }
        true
    }

    /// Whether the block at `pos` has its block entity in a chunk the region holds, which
    /// a dispense needs (the game logs and does nothing without one).
    fn own_block_entity(&self, pos: BlockPos) -> bool {
        let (x, z) = pos.in_chunk();
        self.chunks.get(&pos.chunk()).is_some_and(|chunk| {
            chunk
                .block_entities()
                .contains_key(&clustine_world::InChunk::new(x, pos.y, z))
        })
    }

    /// The level's stream of the dispenser at `pos` in this tick, taken.
    fn take_dispenser_stream(&mut self, pos: BlockPos) -> Random {
        self.block_streams
            .remove(&pos)
            .unwrap_or_else(|| Random::of_block(self.config.seed, pos, self.tick, DISPENSING))
    }

    /// `DispenserBlock.dispenseFrom(level, state, pos)`: a slot drawn, and the
    /// behaviour of its item run on it, which plays the click and the smoke; an empty
    /// dispenser clicks its failure. The slot is set to what the behaviour gives back.
    fn dispense_from(&mut self, pos: BlockPos, state: BlockState, output: &mut TickOutput) {
        let Some(mut container) = self.container_at(pos) else {
            return;
        };
        let unpacked = container.loot;
        container.unpack();
        let mut random = self.take_dispenser_stream(pos);
        let Some(slot) = random_slot(&container.slots, |bound| random.next_int(bound)) else {
            self.block_streams.insert(pos, random);
            level_event(FAILED, pos, 0, output);
            // `block_activate` of the dispenser as it is (`vibrations.rs`).
            let context = super::vibrations::Context::of(None, self.own_block(pos));
            self.vibrate_at(super::vibrations::Vibration::BLOCK_ACTIVATE, pos, context);
            if unpacked {
                self.set_container(pos, container, output);
            }
            return;
        };
        let mut dispense = Dispense {
            pos,
            facing: facing(state).unwrap_or(Face::North),
            slots: container.slots.clone(),
            slot,
            random,
        };
        let left = self.run_behaviour(&mut dispense, output);
        dispense.slots[slot] = left.filter(|stack| holds(Some(stack)));
        self.block_streams.insert(pos, dispense.random);
        let changed = dispense.slots != container.slots;
        container.slots = dispense.slots;
        self.set_container(pos, container, output);
        if changed {
            self.circuit_changed(pos.chunk());
        }
    }

    /// `DropperBlock.dispenseFrom`: a slot drawn as a dispenser draws it, and one of its
    /// item put into the container in front, silently, the slot keeping it where it does
    /// not go in; with no container there, dropped as a dispenser drops it. An empty
    /// dropper clicks its failure. A dropper never uses the item's own behaviour.
    fn drop_from(&mut self, pos: BlockPos, state: BlockState, output: &mut TickOutput) {
        let Some(mut container) = self.container_at(pos) else {
            return;
        };
        let unpacked = container.loot;
        container.unpack();
        let mut random = self.take_dispenser_stream(pos);
        let Some(slot) = random_slot(&container.slots, |bound| random.next_int(bound)) else {
            self.block_streams.insert(pos, random);
            level_event(FAILED, pos, 0, output);
            if unpacked {
                self.set_container(pos, container, output);
            }
            return;
        };
        // The front is read from the block as it is now; the drop goes the way of the
        // tick's state, as the game's `BlockSource` holds it.
        let now = self.own_block(pos).and_then(facing);
        let source = facing(state).unwrap_or(Face::North);
        let front = relative(pos, now.unwrap_or(source));
        let mut dispense = Dispense {
            pos,
            facing: source,
            slots: container.slots.clone(),
            slot,
            random,
        };
        let item = dispense.stack().map(|stack| stack.item);
        let into = self.dropper_target(front, &mut dispense.random);
        match (into, item) {
            (_, None) => {}
            (Into::Nothing, Some(_)) => {
                let left = self.default_dispense(&mut dispense, output);
                dispense.slots[slot] = left;
            }
            (Into::Across, Some(_)) => level_event(FAILED, pos, 0, output),
            (Into::Unkept, Some(_)) => {}
            (Into::Block(halves), Some(item)) => {
                if self.insert_one_into_blocks(&halves, item, output) {
                    dispense.shrink(1);
                }
            }
            (Into::Entity(key, chunk), Some(item)) => {
                if self.insert_one_into_entity(key, chunk, item, output) {
                    dispense.shrink(1);
                }
            }
        }
        self.block_streams.insert(pos, dispense.random);
        let changed = dispense.slots != container.slots;
        container.slots = dispense.slots;
        self.set_container(pos, container, output);
        if changed {
            self.circuit_changed(pos.chunk());
        }
    }

    /// `HopperBlockEntity.getContainerAt(level, front)` as a dropper asks it: the
    /// container block there (a chest with the chest it is double with, whether or not
    /// it is blocked), else one of the living container vehicles whose boxes meet the
    /// block, drawn by `nextInt(count)` where there are several.
    fn dropper_target(&self, front: BlockPos, random: &mut Random) -> Into {
        let state = self.block(front);
        let is_container = state
            .is_some_and(|state| container_size(state).is_some() || is_unkept_container(state));
        if !self.chunks.contains_key(&front.chunk()) {
            return if is_container {
                Into::Across
            } else {
                Into::Nothing
            };
        }
        if let Some(state) = state {
            if let Some(halves) = self.chest_halves(front) {
                return Into::Block(std::iter::once(halves.first).chain(halves.second).collect());
            }
            if container_size(state).is_some() && self.container_at(front).is_some() {
                return Into::Block(vec![front]);
            }
            if is_unkept_container(state) {
                return Into::Unkept;
            }
        }
        let vehicles: Vec<(EntityId, ChunkPos)> = self
            .entities_in(&Aabb::of_block(front))
            .into_iter()
            .filter(|(key, chunk)| {
                self.entity_record(*key, *chunk)
                    .is_some_and(|record| super::vehicles::container_size(record.kind) > 0)
            })
            .collect();
        match vehicles.len() {
            0 => Into::Nothing,
            1 => Into::Entity(vehicles[0].0, vehicles[0].1),
            count => {
                let bound = u32::try_from(count).unwrap_or(u32::MAX);
                let (key, chunk) = vehicles[random.next_int(bound) as usize];
                Into::Entity(key, chunk)
            }
        }
    }

    /// `HopperBlockEntity.addItem` of one `item` into the container block of `halves`:
    /// the first slot, the right half's before the left's, that is empty or holds the same
    /// item below its most, takes it; a shulker box takes no shulker box. Whether one
    /// took it. One item put by a dropper follows the same rules as the hoppers' whole
    /// `addItem` (`hoppers.rs`), which this need not share.
    fn insert_one_into_blocks(
        &mut self,
        halves: &[BlockPos],
        item: i32,
        output: &mut TickOutput,
    ) -> bool {
        for pos in halves {
            let Some(state) = self.own_block(*pos) else {
                continue;
            };
            if is_class(state, BlockClass::ShulkerBoxBlock) && is_shulker_box_item(item) {
                return false;
            }
            let Some(mut container) = self.container_at(*pos) else {
                continue;
            };
            if container.loot {
                // `addItem` asks the slots, which unpacks the table first.
                container.unpack();
            }
            let was_empty = container.slots.iter().all(Option::is_none);
            if insert_one(&mut container.slots, item) {
                self.set_container(*pos, container, output);
                if was_empty && is_class(state, BlockClass::HopperBlock) {
                    self.hopper_fed_from_outside(*pos, output);
                }
                // `CompoundContainer.setChanged` tells both halves: the comparators of the
                // other read the change too.
                for other in halves.iter().filter(|other| *other != pos) {
                    if let Some(unchanged) = self.container_at(*other) {
                        self.set_container(*other, unchanged, output);
                    }
                }
                return true;
            }
        }
        false
    }

    /// `HopperBlockEntity.addItem` of one `item` into the container vehicle `key`.
    /// Whether it took it.
    fn insert_one_into_entity(
        &mut self,
        key: EntityId,
        chunk: ChunkPos,
        item: i32,
        output: &mut TickOutput,
    ) -> bool {
        let Some(record) = self.entity_record(key, chunk) else {
            return false;
        };
        let size = super::vehicles::container_size(record.kind);
        let mut data = record.data.clone();
        let (EntityData::Minecart { contents, .. } | EntityData::Boat { contents, .. }) = &mut data
        else {
            return false;
        };
        let mut slots = vec![None; size];
        for (slot, stack) in contents.iter() {
            if let Some(place) = slots.get_mut(usize::from(*slot)) {
                *place = Some(stack.clone());
            }
        }
        if !insert_one(&mut slots, item) {
            return false;
        }
        *contents = slots
            .iter()
            .enumerate()
            .filter_map(|(slot, stack)| Some((u8::try_from(slot).ok()?, stack.clone()?)))
            .collect();
        self.commit_entity(chunk, key, vec![EntityField::Data(data)], output);
        true
    }

    /// `DefaultDispenseItemBehavior.dispense`: one of the stack dropped, then the click
    /// and the smoke. What the slot is left with.
    pub(crate) fn default_dispense(
        &mut self,
        dispense: &mut Dispense,
        output: &mut TickOutput,
    ) -> Option<ItemStack> {
        let left = self.default_execute(dispense, output);
        self.click_and_smoke(dispense, DISPENSED, output);
        left
    }

    /// `DefaultDispenseItemBehavior.execute`: one of the stack dropped out of the front.
    /// What the slot is left with.
    pub(crate) fn default_execute(
        &mut self,
        dispense: &mut Dispense,
        output: &mut TickOutput,
    ) -> Option<ItemStack> {
        if let Some(one) = dispense.split_one() {
            self.spawn_dispensed(dispense, one, output);
        }
        dispense.stack()
    }

    /// `DefaultDispenseItemBehavior.playSound` (or a behaviour's own, `click`) and
    /// `playAnimation`: the click, then smoke out of the face the dispenser faces.
    pub(crate) fn click_and_smoke(&self, dispense: &Dispense, click: i32, output: &mut TickOutput) {
        level_event(click, dispense.pos, 0, output);
        level_event(SHOOT_SMOKE, dispense.pos, dispense.facing as i32, output);
    }

    /// `DefaultDispenseItemBehavior.spawnItem(level, stack, 6, facing, position)`: the
    /// item at the dispense position a little lowered, with the constructor's own draws,
    /// and then the motion of seven level draws, no pick-up delay (the constructor's 0).
    /// Into a chunk the region does not have loaded it is sent as an arrival, as a throw
    /// is; into one that is full, or with no key left, it is lost, as a drop is.
    pub(crate) fn spawn_dispensed(
        &mut self,
        dispense: &mut Dispense,
        stack: ItemStack,
        output: &mut TickOutput,
    ) {
        let position = drop_position(
            dispense_position(dispense.pos, dispense.facing),
            dispense.facing,
        );
        let chunk = ChunkPos::containing(position[0], position[2]);
        if self.chunks.contains_key(&chunk) {
            // The constructor's draws are the item's own; the level's come after.
            let Some(key) = self.take_entity_key(chunk) else {
                let _ = drop_motion(dispense.facing, &mut dispense.next_double());
                return;
            };
            let motion = drop_motion(dispense.facing, &mut dispense.next_double());
            let entity = self.new_item(key, position, stack, motion);
            self.entity_drew(key, 6);
            self.add_fresh_entity(entity, output);
            return;
        }
        let motion = drop_motion(dispense.facing, &mut dispense.next_double());
        let Some(key) = self.take_key() else {
            return;
        };
        let entity = self.new_item(key, position, stack, motion);
        self.send_made(entity, output);
    }

    /// `new ItemEntity(level, x, y, z, stack)` under `key`, its motion then set to
    /// `motion`: its UUID, the bob of its model, its yaw and the constructor's motion
    /// drawn from its own stream, the last two turned over.
    fn new_item(
        &self,
        key: EntityId,
        position: [f64; 3],
        stack: ItemStack,
        motion: [f64; 3],
    ) -> Entity {
        let mut own = self.entity_stream(key).random;
        let uuid = insecure_uuid(own.next_u64(), own.next_u64());
        let _bob = own.next_float();
        let yaw = own.next_float() * 360.0;
        let _motion = [own.next_double(), own.next_double()];
        Entity {
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
                pick_up_delay: 0,
                thrower: None,
                health: 5,
                age_offset: 0,
            },
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
        }
    }

    /// `DefaultDispenseItemBehavior.consumeWithRemainder`: one of the stack used, and
    /// `remainder` (an empty bucket, a filled bottle) in its place where it was the last;
    /// otherwise put into the dispenser's slots where it fits, and what does not fit
    /// dropped with a click and smoke of its own. What the slot is left with.
    pub(crate) fn consume_with_remainder(
        &mut self,
        dispense: &mut Dispense,
        remainder: ItemStack,
        output: &mut TickOutput,
    ) -> Option<ItemStack> {
        dispense.shrink(1);
        if dispense.stack().is_none() {
            return Some(remainder);
        }
        if let Some(left) = insert_item(&mut dispense.slots, remainder) {
            self.spawn_dispensed(dispense, left, output);
            self.click_and_smoke(dispense, DISPENSED, output);
        }
        dispense.stack()
    }

    /// `Level.playSound(null, pos, …)` of a block's sound at the middle of the block at
    /// `pos`, from the blocks, heard by everyone near.
    pub(crate) fn block_sound(
        pos: BlockPos,
        sound: i32,
        volume: f32,
        pitch: f32,
        output: &mut TickOutput,
    ) {
        output.events.push(RegionEvent::Sound {
            sound,
            source: SOUND_SOURCE_BLOCKS,
            position: clustine_world::Vec3::new(
                f64::from(pos.x) + 0.5,
                f64::from(pos.y) + 0.5,
                f64::from(pos.z) + 0.5,
            ),
            volume,
            pitch,
            except: None,
        });
    }
}

/// Whether `item` places a shulker box (`Block.byItem(item) instanceof ShulkerBoxBlock`),
/// which a shulker box does not take (`canPlaceItemThroughFace`).
pub(crate) fn is_shulker_box_item(item: i32) -> bool {
    usize::try_from(item)
        .ok()
        .and_then(|index| clustine_data::ITEMS.get(index))
        .and_then(|info| info.places)
        .is_some_and(|block| {
            is_class(
                clustine_data::BLOCKS[usize::from(block)].default_state,
                BlockClass::ShulkerBoxBlock,
            )
        })
}

/// `HopperBlockEntity.tryMoveInItem` of one `item` over `slots` in order: into the first
/// that is empty, or that holds the same item with the same components (none) below the
/// item's own most. Whether it went in.
fn insert_one(slots: &mut [Option<ItemStack>], item: i32) -> bool {
    let one = ItemStack::new(item, 1);
    let most = crate::physics::item::max_stack_size(&one);
    for place in slots.iter_mut() {
        match place {
            Some(held) if holds(Some(held)) => {
                if held.same_kind(&one) && held.count < most {
                    held.count += 1;
                    return true;
                }
            }
            _ => {
                *place = Some(one);
                return true;
            }
        }
    }
    false
}

/// `ServerLevel.levelEvent(kind, pos, data)`: what clients within 64 blocks show of it.
pub(crate) fn level_event(kind: i32, position: BlockPos, data: i32, output: &mut TickOutput) {
    output.events.push(RegionEvent::LevelEvent {
        kind,
        position,
        data,
    });
}

/// The sound of a bucket of `item` emptied (`BucketItem.playEmptySound`, a mob bucket's
/// own): lava's for lava, a fish's, an axolotl's or a tadpole's for theirs, else water's.
pub(crate) fn empty_sound(item: i32) -> i32 {
    use clustine_data::items;
    match item {
        items::LAVA_BUCKET => sounds::ITEM_BUCKET_EMPTY_LAVA,
        items::SALMON_BUCKET
        | items::COD_BUCKET
        | items::PUFFERFISH_BUCKET
        | items::TROPICAL_FISH_BUCKET => sounds::ITEM_BUCKET_EMPTY_FISH,
        items::AXOLOTL_BUCKET => sounds::ITEM_BUCKET_EMPTY_AXOLOTL,
        items::TADPOLE_BUCKET => sounds::ITEM_BUCKET_EMPTY_TADPOLE,
        _ => sounds::ITEM_BUCKET_EMPTY,
    }
}
