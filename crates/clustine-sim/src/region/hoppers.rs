//! Hoppers and hopper carts (step N6e of `docs/groundwork/n6-plan.md`; the brief's
//! sections 5.1 to 5.9): a hopper's block, which redstone locks; its block entity's
//! ticker, which counts its cooldown down and moves an item every eight ticks; its
//! push into the container it faces and its pull from the container above it or of
//! the items lying in it; the item that falls into it, taken in the item's own tick;
//! and the hopper cart, which an activator rail switches off and which pulls as it
//! rolls.
//!
//! A hopper is a typed block entity ([`Timer`]): its cooldown, which the game saves as
//! `TransferCooldown` and which is written into its data whenever it changes, and the
//! tick it last ticked in (`tickedGameTime`), which the game does not save and which is
//! 0 after a load, as there. Its slots are in its data, as every container's are
//! (`containers.rs`), and a comparator reads them there.
//!
//! A move reads the containers it touches into slots of its own, does the game's
//! `addItem` on them, and writes back what changed: a double chest as one container of
//! 54 slots, a shulker box refusing a shulker box, an entity container's contents into
//! its record. What the game's `setChanged` tells, the comparators beside a container,
//! is told once for each container a move changed, after its data is written: the game
//! tells a chest's comparators at each `setItem` too, and a chest above a hopper that
//! cannot take its items at every try, which tells a comparator nothing it did not know.
//!
//! The container entities a hopper finds by a box (chest and hopper carts, chest boats
//! and rafts) are chosen among by a draw, as the game's `getEntityContainer` draws from
//! the level's random: here from a stream of the hopper's place and the tick (the
//! brief's section 10.5), or of a hopper cart's own.
//!
//! Named differences:
//! - A composter is a container to a hopper that nothing goes into or out of: a hopper
//!   does not compost nor take the bone meal until the composter's recipes come (decision
//!   8), and one under a composter takes no items lying above it, as the game's does not.
//!   So too a furnace, a blast furnace, a smoker, a brewing stand, a chiseled bookshelf,
//!   a decorated pot, a jukebox, a shelf and a crafter, whose slots Clustine does not
//!   keep yet.
//! - A loot table is not rolled (decision 4): a chest with one gives a hopper nothing,
//!   and loses the table when a hopper puts something into it.
//! - A hopper takes from and gives to the entities of the chunks the region ticks only.
//! - An item a hopper takes part of is told to the edges only where its stack changed;
//!   the game sends its stack again each time, unchanged or not.
//!
//! At a line (`docs/groundwork/n6-plan.md`, section 10 items 1 and 7): a hopper whose
//! container in front is in a chunk the region does not hold moves nothing into it, as
//! if that were a container nothing goes into ([`Region::hopper_reach`]); the block
//! above a hopper is in its own chunk. Lending that land to the hopper's region at a
//! pinned line, as a circuit's part has it lent (ADR-0027), is step N6g's, and starts
//! from [`Region::hopper_reach`]. A hopper whose slots change calls `circuit_changed`,
//! so that regions that follow players merge where hoppers work at their line.

use clustine_blocks::data::{facing, flag, in_tag, is_class, relative, same_block, set_flag};
use clustine_blocks::signal;
use clustine_data::{BlockClass, BlockState, ITEMS, blocks, entity_types};
use clustine_world::{BlockPos, ChunkPos, Contents, EntityData, EntityField, EntityId, ItemStack};

use super::Region;
use super::containers::{container_data, container_size, read_container, read_int, with_int};
use super::items::tracked;
use super::random_ticks::Random;
use super::updates::{RegionView, UPDATE_CLIENTS, UPDATE_LIMIT};
use crate::api::{RegionEvent, TickOutput};
use crate::physics::aabb::Aabb;
use crate::physics::item::{Item, max_stack_size};
use crate::physics::{self, Vec3};

/// `HopperBlockEntity.MOVE_ITEM_SPEED`: the ticks a hopper waits after it moved items.
const MOVE_ITEM_SPEED: i32 = 8;

/// `HopperBlockEntity.NO_COOLDOWN_TIME`: the cooldown of a hopper that has none saved.
const NO_COOLDOWN_TIME: i32 = -1;

/// The key the game saves a hopper's cooldown under.
const COOLDOWN: &str = "TransferCooldown";

/// `Container.getMaxStackSize()`: the most a container's slot holds of anything.
const CONTAINER_MAX: i32 = 99;

/// `Hopper.SUCK_AABB`, the box of `Block.column(16, 11, 32)`: where a hopper takes
/// items from, from eleven sixteenths up its own block to the top of the one above.
const SUCK_FLOOR: f64 = 11.0 / 16.0;
const SUCK_TOP: f64 = 2.0;

/// What the streams of a hopper's draws are for, mixed into them.
const PUSHES: u64 = 0x7075_7368_6573;
const PULLS: u64 = 0x0070_756c_6c73;

/// A hopper's block entity as the simulation keeps it beside its data: its cooldown
/// (`cooldownTime`) and the region's tick it last ticked in (`tickedGameTime`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Timer {
    pub(crate) cooldown: i32,
    pub(crate) ticked: u64,
}

impl Timer {
    /// A hopper's block entity loaded from `data`: its saved cooldown, or none; it has
    /// not ticked.
    pub(crate) fn loaded(data: &[u8]) -> Timer {
        Timer {
            cooldown: read_int(data, COOLDOWN).unwrap_or(NO_COOLDOWN_TIME),
            ticked: 0,
        }
    }
}

/// Where the stream of a draw among container entities comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Draw {
    /// A hopper's at this place, for this purpose.
    Block(BlockPos, u64),
    /// A hopper cart's own.
    Entity(EntityId),
}

/// What holds the slots of a container a hopper moves items into or out of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Holder {
    /// A container block, or the two halves of a double chest, the right one first.
    Blocks {
        first: BlockPos,
        second: Option<BlockPos>,
    },
    /// A chest or hopper cart, or a chest boat or raft, in its chunk.
    Entity { key: EntityId, chunk: ChunkPos },
    /// A container nothing goes into or out of here: one whose slots Clustine does not
    /// keep yet, or one in a chunk the region does not hold.
    Shut,
}

/// A container a hopper found, its slots read to be moved in and written back.
#[derive(Debug, Clone, PartialEq)]
struct Open {
    holder: Holder,
    /// A shulker box takes no shulker box through any face
    /// (`ShulkerBoxBlockEntity.canPlaceItemThroughFace`).
    refuses_boxes: bool,
    slots: Vec<Option<ItemStack>>,
    /// Whether a move put something into it while it was wholly empty, which hands a
    /// hopper's cooldown on.
    fed_empty: bool,
}

impl Open {
    fn shut() -> Open {
        Open {
            holder: Holder::Shut,
            refuses_boxes: false,
            slots: Vec::new(),
            fed_empty: false,
        }
    }

    /// `isFullContainer`: every slot holds as much as its item stacks to. One with no
    /// slots is full.
    fn is_full(&self) -> bool {
        self.slots.iter().all(|slot| match slot {
            Some(stack) if stack.count > 0 => stack.count >= max_stack_size(*stack),
            _ => false,
        })
    }

    /// `addItem(source, this, stack, face)`: what of `stack` does not go in.
    fn add(&mut self, stack: ItemStack) -> Option<ItemStack> {
        let (rest, fed_empty) = add_to(&mut self.slots, stack, self.refuses_boxes);
        self.fed_empty |= fed_empty;
        rest
    }
}

/// The stack in a slot, where there is one.
fn filled(slot: Option<ItemStack>) -> Option<ItemStack> {
    slot.filter(|stack| stack.count > 0)
}

/// `stack` less one, or nothing where it was one (`removeItem(slot, 1)` of what is
/// left in the slot).
fn less_one(stack: ItemStack) -> Option<ItemStack> {
    (stack.count > 1).then_some(ItemStack {
        count: stack.count - 1,
        ..stack
    })
}

/// Whether `item` is the item of a shulker box block.
fn is_shulker_box_item(item: i32) -> bool {
    usize::try_from(item)
        .ok()
        .and_then(|item| ITEMS.get(item))
        .and_then(|item| item.block)
        .is_some_and(|block| is_class(block, BlockClass::ShulkerBoxBlock))
}

/// `HopperBlockEntity.addItem(source, target, stack, face)` on the slots `slots` of a
/// container that takes anything, but a shulker box where `refuses_boxes`, through any
/// face: each slot in order while some of `stack` is left, an empty one taking it
/// whole (to the container's most for it: `setItem` limits it, and the rest is lost,
/// as the game loses it), one of the same item as much as it has room for
/// (`tryMoveInItem`). What is left of `stack`, and whether some went into a slot while
/// the container was wholly empty.
fn add_to(
    slots: &mut [Option<ItemStack>],
    mut stack: ItemStack,
    refuses_boxes: bool,
) -> (Option<ItemStack>, bool) {
    if refuses_boxes && is_shulker_box_item(stack.item) {
        return (Some(stack), false);
    }
    let mut fed_empty = false;
    for slot in 0..slots.len() {
        if stack.count <= 0 {
            break;
        }
        let was_empty = slots.iter().all(|slot| filled(*slot).is_none());
        let moved = match filled(slots[slot]) {
            None => {
                let most = CONTAINER_MAX.min(max_stack_size(stack));
                slots[slot] = Some(ItemStack {
                    count: stack.count.min(most),
                    ..stack
                });
                stack.count = 0;
                true
            }
            // `canMergeItems`: no more than it stacks to, of the same item.
            Some(mut here) if here.item == stack.item && here.count <= max_stack_size(here) => {
                let room = max_stack_size(stack) - here.count;
                let taken = stack.count.min(room);
                if taken > 0 {
                    stack.count -= taken;
                    here.count += taken;
                    slots[slot] = Some(here);
                }
                taken > 0
            }
            Some(_) => false,
        };
        fed_empty |= moved && was_empty;
    }
    ((stack.count > 0).then_some(stack), fed_empty)
}

/// `HopperBlockEntity.inventoryFull`: every slot holds a stack as large as its item
/// stacks to.
fn inventory_full(slots: &[Option<ItemStack>]) -> bool {
    slots
        .iter()
        .all(|slot| filled(*slot).is_some_and(|stack| stack.count == max_stack_size(stack)))
}

/// The slots of a container entity's contents, `size` of them.
fn slots_of(contents: &Contents, size: usize) -> Vec<Option<ItemStack>> {
    let mut slots = vec![None; size];
    for (slot, stack) in contents {
        if let Some(place) = slots.get_mut(usize::from(*slot)) {
            *place = Some(*stack);
        }
    }
    slots
}

/// The contents of `slots`, as an entity's record keeps them.
fn contents_of(slots: &[Option<ItemStack>]) -> Contents {
    slots
        .iter()
        .enumerate()
        .filter_map(|(slot, stack)| Some((u8::try_from(slot).ok()?, filled(*stack)?)))
        .collect()
}

/// The centre of the block at `pos`.
fn centre(pos: BlockPos) -> Vec3 {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5,
        f64::from(pos.z) + 0.5,
    ]
}

/// The box a hopper at `at` (`getLevelX`, `getLevelY`, `getLevelZ`) takes items from
/// (`getItemsAtAndAbove`: `SUCK_AABB` moved by `at` less a half).
fn suck_box(at: Vec3) -> Aabb {
    Aabb::new(
        [at[0] - 0.5, at[1] - 0.5 + SUCK_FLOOR, at[2] - 0.5],
        [at[0] + 0.5, at[1] - 0.5 + SUCK_TOP, at[2] + 0.5],
    )
}

/// What takes items in: a hopper's block, with its place's centre, or a hopper cart at
/// its position half a block up; only a block is grid aligned.
#[derive(Debug, Clone, Copy)]
struct Sucker {
    at: Vec3,
    grid_aligned: bool,
    draw: Draw,
}

/// Where a hopper's items come from in a move: the container above it or the items
/// lying in it (`suckInItems`), or the one item that fell into it (`entityInside`).
enum Supply<'a> {
    Above,
    Item(&'a mut Item),
}

/// The container blocks whose slots a hopper cannot reach yet, but which the game's is
/// a container to: nothing goes into or out of them.
fn shut_to_hoppers(state: BlockState) -> bool {
    [
        BlockClass::ComposterBlock,
        BlockClass::AbstractFurnaceBlock,
        BlockClass::BrewingStandBlock,
        BlockClass::ChiseledBookShelfBlock,
        BlockClass::DecoratedPotBlock,
        BlockClass::JukeboxBlock,
        BlockClass::ShelfBlock,
        BlockClass::CrafterBlock,
    ]
    .into_iter()
    .any(|class| is_class(state, class))
}

impl Region {
    /// `HopperBlockEntity.pushItemsTick`, the hopper's ticker: its cooldown counts down
    /// and the tick is stamped; off its cooldown, it tries to move items, pulling from
    /// above. The cooldown is saved as it changes.
    pub(super) fn hopper_tick(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) {
        let tick = self.tick;
        let Some(timer) = self.hopper_timer_mut(pos) else {
            return;
        };
        timer.cooldown = timer.cooldown.saturating_sub(1);
        timer.ticked = tick;
        if timer.cooldown <= 0 {
            timer.cooldown = 0;
            self.try_move_items(pos, state, Supply::Above, output);
        }
        self.keep_block(pos, None, output);
    }

    /// `HopperBlockEntity.entityInside` of the hopper `state` at `pos` for the item
    /// `item`, whose walk met it in its tick: an item whose box meets the box the hopper
    /// takes from is taken in by a move of the hopper's, which pushes first.
    pub(super) fn hopper_entity_inside(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        item: &mut Item,
        output: &mut TickOutput,
    ) {
        if !self.ticks_chunk(pos.chunk()) || item.removed || item.stack.count <= 0 {
            return;
        }
        if !item.body.bounds().intersects(suck_box(centre(pos))) {
            return;
        }
        self.try_move_items(pos, state, Supply::Item(item), output);
    }

    /// `HopperBlockEntity.tryMoveItems`: a hopper off its cooldown and not locked by
    /// redstone pushes one item into the container it faces, if it holds any, then,
    /// unless it is full, takes from `supply`; having moved anything it waits eight
    /// ticks, and its comparators are told (`setChanged`). Whether it moved anything.
    fn try_move_items(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        supply: Supply<'_>,
        output: &mut TickOutput,
    ) -> bool {
        let Some(timer) = self.hopper_timer(pos) else {
            return false;
        };
        if timer.cooldown > 0 || !flag(state, "enabled") {
            return false;
        }
        let Some(container) = self.container_at(pos) else {
            return false;
        };
        let mut slots = container.slots;
        let mut moved = false;
        if slots.iter().any(|slot| filled(*slot).is_some()) {
            moved = self.eject_items(pos, state, timer, &mut slots, output);
        }
        if !inventory_full(&slots) {
            let took = match supply {
                Supply::Above => {
                    let sucker = Sucker {
                        at: centre(pos),
                        grid_aligned: true,
                        draw: Draw::Block(pos, PULLS),
                    };
                    self.suck_in_items(sucker, &mut slots, output)
                }
                Supply::Item(item) => take_from_item(&mut slots, item),
            };
            moved |= took;
        }
        if moved {
            if let Some(timer) = self.hopper_timer_mut(pos) {
                timer.cooldown = MOVE_ITEM_SPEED;
            }
            self.keep_block(pos, Some(&slots), output);
            self.update_neighbour_for_output_signal(pos, state, output);
            self.circuit_changed(pos.chunk());
        }
        moved
    }

    /// `HopperBlockEntity.ejectItems`: one item of the first of the hopper's slots that
    /// the container it faces takes, put in through the face towards the hopper; nothing
    /// into a container that is full. `timer` is the hopper's, which a hopper it feeds
    /// reads.
    fn eject_items(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        timer: Timer,
        slots: &mut [Option<ItemStack>],
        output: &mut TickOutput,
    ) -> bool {
        let Some(towards) = facing(state) else {
            return false;
        };
        let target = relative(pos, towards);
        let Some(mut open) = self.open_container(target, centre(target), Draw::Block(pos, PUSHES))
        else {
            return false;
        };
        if open.is_full() {
            return false;
        }
        for slot in slots.iter_mut() {
            let Some(stack) = filled(*slot) else {
                continue;
            };
            *slot = less_one(stack);
            if open.add(ItemStack { count: 1, ..stack }).is_none() {
                self.close_container(open, Some(timer), output);
                return true;
            }
            *slot = Some(stack);
        }
        false
    }

    /// `HopperBlockEntity.suckInItems` into `slots`, the hopper's or the hopper cart's:
    /// from the container above, one item of the first slot it gives up, and nothing
    /// else even where it gives none; without one, unless a full block stands above a
    /// hopper's block (one of `#minecraft:does_not_block_hoppers` does not), the items
    /// lying in its box, the first that goes in wholly. Whether it took anything.
    fn suck_in_items(
        &mut self,
        sucker: Sucker,
        slots: &mut [Option<ItemStack>],
        output: &mut TickOutput,
    ) -> bool {
        let [x, y, z] = sucker.at;
        let up = [x, y + 1.0, z];
        let above = physics::containing(up);
        if let Some(mut source) = self.open_container(above, up, sucker.draw) {
            for slot in 0..source.slots.len() {
                let Some(stack) = filled(source.slots[slot]) else {
                    continue;
                };
                source.slots[slot] = less_one(stack);
                let (rest, _) = add_to(slots, ItemStack { count: 1, ..stack }, false);
                if rest.is_none() {
                    self.close_container(source, None, output);
                    return true;
                }
                source.slots[slot] = Some(stack);
            }
            return false;
        }
        let above_state = self.block(above).unwrap_or(blocks::AIR);
        let blocked = sucker.grid_aligned
            && above_state.props().collision_is_full_block()
            && !in_tag(above_state, "minecraft:does_not_block_hoppers");
        if blocked {
            return false;
        }
        for (key, chunk) in self.items_in(&suck_box(sucker.at)) {
            if self.take_item_record(key, chunk, slots, output) {
                return true;
            }
        }
        false
    }

    /// The items of the chunks the region ticks whose boxes meet `bounds`, with a stack,
    /// in the order of the game's lookups (`ENTITY_STILL_ALIVE`).
    fn items_in(&self, bounds: &Aabb) -> Vec<(EntityId, ChunkPos)> {
        self.entities_in(bounds)
            .into_iter()
            .filter(|(key, chunk)| {
                self.ticks_chunk(*chunk)
                    && self.entity_record(*key, *chunk).is_some_and(|record| {
                        matches!(record.data, EntityData::Item { stack, .. } if stack.count > 0)
                    })
            })
            .collect()
    }

    /// `HopperBlockEntity.addItem(container, itemEntity)` of the item `key` of the chunk
    /// `chunk` into `slots`: all of its stack that goes in; an item that went wholly is
    /// gone, one that went in part keeps what is left. Whether it went wholly.
    fn take_item_record(
        &mut self,
        key: EntityId,
        chunk: ChunkPos,
        slots: &mut [Option<ItemStack>],
        output: &mut TickOutput,
    ) -> bool {
        let Some(record) = self.entity_record(key, chunk).cloned() else {
            return false;
        };
        let EntityData::Item {
            stack,
            pick_up_delay,
            thrower,
            health,
        } = record.data
        else {
            return false;
        };
        let (rest, _) = add_to(slots, stack, false);
        let Some(rest) = rest else {
            self.remove_entity(key, record.hops, chunk, output);
            return true;
        };
        if rest != stack {
            let data = EntityData::Item {
                stack: rest,
                pick_up_delay,
                thrower,
                health,
            };
            self.commit_entity(chunk, key, vec![EntityField::Data(data.clone())], output);
            let mut after = record;
            after.data = data;
            let mut report = tracked(&after, 0, true);
            report.chunk = chunk;
            output.events.push(RegionEvent::EntityTracked(report));
        }
        false
    }

    /// The container at `pos` as a hopper finds it (`getContainerAt` with `at` the
    /// point it looks around): a container block's (a chest as the double chest it is
    /// half of, whatever blocks it), else one of the container entities whose boxes meet
    /// the block's box about `at`, chosen by a draw of `draw`'s stream; `None` where
    /// there is neither.
    fn open_container(&mut self, pos: BlockPos, at: Vec3, draw: Draw) -> Option<Open> {
        if !self.hopper_reach(pos) {
            return Some(Open::shut());
        }
        let state = self.own_block(pos)?;
        if shut_to_hoppers(state) {
            return Some(Open::shut());
        }
        if is_class(state, BlockClass::ChestBlock) {
            if let Some(halves) = self.chest_halves(pos) {
                if halves
                    .second
                    .is_some_and(|second| !self.hopper_reach(second))
                {
                    return Some(Open::shut());
                }
                let mut slots = self.container_at(halves.first)?.slots;
                if let Some(second) = halves.second {
                    slots.extend(self.container_at(second)?.slots);
                }
                return Some(Open {
                    holder: Holder::Blocks {
                        first: halves.first,
                        second: halves.second,
                    },
                    refuses_boxes: false,
                    slots,
                    fed_empty: false,
                });
            }
        } else if let Some(container) = self.container_at(pos) {
            return Some(Open {
                holder: Holder::Blocks {
                    first: pos,
                    second: None,
                },
                refuses_boxes: is_class(state, BlockClass::ShulkerBoxBlock),
                slots: container.slots,
                fed_empty: false,
            });
        }
        // `getEntityContainer`: the box of a block about `at`.
        let bounds = Aabb::new(
            [at[0] - 0.5, at[1] - 0.5, at[2] - 0.5],
            [at[0] + 0.5, at[1] + 0.5, at[2] + 0.5],
        );
        let found: Vec<(EntityId, ChunkPos, Vec<Option<ItemStack>>)> = self
            .entities_in(&bounds)
            .into_iter()
            .filter(|(_, chunk)| self.ticks_chunk(*chunk))
            .filter_map(|(key, chunk)| {
                let record = self.entity_record(key, chunk)?;
                let size = super::vehicles::container_size(record.kind);
                let contents = super::vehicles::contents_of(&record.data)?;
                (size > 0).then(|| (key, chunk, slots_of(contents, size)))
            })
            .collect();
        if found.is_empty() {
            return None;
        }
        let chosen = self.draw_index(draw, found.len());
        let (key, chunk, slots) = found.into_iter().nth(chosen)?;
        Some(Open {
            holder: Holder::Entity { key, chunk },
            refuses_boxes: false,
            slots,
            fed_empty: false,
        })
    }

    /// `level.random.nextInt(count)` for the choice among container entities, from the
    /// stream `draw` names.
    fn draw_index(&mut self, draw: Draw, count: usize) -> usize {
        let bound = u32::try_from(count).unwrap_or(u32::MAX);
        let drawn = match draw {
            Draw::Block(pos, purpose) => {
                Random::of_block(self.config.seed, pos, self.tick, purpose).next_int(bound)
            }
            Draw::Entity(key) => {
                let already = self.entity_draws.get(&key).copied().unwrap_or(0);
                let mut random = Random::of_entity(self.config.seed, key, self.tick);
                for _ in 0..already {
                    random.next_u64();
                }
                let start = random.clone();
                let drawn = random.next_int(bound);
                *self.entity_draws.entry(key).or_default() += random.draws_since(&start);
                drawn
            }
        };
        usize::try_from(drawn).unwrap_or(0)
    }

    /// Writes back what a move changed of the container `open`, and tells its
    /// comparators (`setChanged`); a hopper fed while it was empty and not on a cooldown
    /// of its own above eight takes eight, or seven where it ticked already in the tick
    /// of `feeder`, the hopper that fed it (`tryMoveInItem`), so that it moves the item
    /// on at the same tick whichever of the two ticked first.
    fn close_container(&mut self, open: Open, feeder: Option<Timer>, output: &mut TickOutput) {
        match open.holder {
            Holder::Shut => {}
            Holder::Blocks { first, second } => {
                let halves: Vec<BlockPos> = std::iter::once(first).chain(second).collect();
                let mut start = 0;
                for &half in &halves {
                    let size = self
                        .own_block(half)
                        .and_then(container_size)
                        .unwrap_or_default();
                    let part = open.slots.get(start..start + size).unwrap_or_default();
                    start += size;
                    if open.fed_empty
                        && let Some(timer) = self.hopper_timer_mut(half)
                        && timer.cooldown <= MOVE_ITEM_SPEED
                    {
                        let ticked_first =
                            feeder.is_some_and(|feeder| timer.ticked >= feeder.ticked);
                        timer.cooldown = MOVE_ITEM_SPEED - i32::from(ticked_first);
                    }
                    self.keep_block(half, Some(part), output);
                }
                for half in halves {
                    if let Some(state) = self.own_block(half) {
                        self.update_neighbour_for_output_signal(half, state, output);
                    }
                    self.circuit_changed(half.chunk());
                }
            }
            Holder::Entity { key, chunk } => {
                let Some(record) = self.entity_record(key, chunk) else {
                    return;
                };
                let contents = contents_of(&open.slots);
                let mut data = record.data.clone();
                match &mut data {
                    EntityData::Minecart { contents: was, .. }
                    | EntityData::Boat { contents: was, .. } => {
                        if *was == contents {
                            return;
                        }
                        *was = contents;
                    }
                    _ => return,
                }
                // A cart's `setChanged` does nothing.
                self.commit_entity(chunk, key, vec![EntityField::Data(data)], output);
            }
        }
    }

    /// Writes the block entity of the container block at `pos` as it now is: `slots`
    /// where given (a loot table, which the game would have unpacked to read them, gone)
    /// and a hopper's cooldown; nothing where that is what its data says already. The
    /// comparators are not told: the caller tells them as the game's `setChanged` does.
    fn keep_block(
        &mut self,
        pos: BlockPos,
        slots: Option<&[Option<ItemStack>]>,
        output: &mut TickOutput,
    ) {
        let Some(state) = self.own_block(pos) else {
            return;
        };
        let Some(size) = container_size(state) else {
            return;
        };
        let Some(before) = self.own_block_entity_data(pos) else {
            return;
        };
        let cooldown = self.hopper_timer(pos).map(|timer| timer.cooldown);
        let mut container = read_container(size, before);
        let slots_changed = slots.is_some_and(|slots| {
            slots.len() == size && (container.slots != slots || container.loot)
        });
        let cooldown_changed =
            cooldown.is_some_and(|cooldown| read_int(before, COOLDOWN) != Some(cooldown));
        if !slots_changed && !cooldown_changed {
            return;
        }
        if slots_changed && let Some(slots) = slots {
            container.slots = slots.to_vec();
            container.unpack();
        }
        let mut data = container_data(state, &container, before);
        if let Some(cooldown) = cooldown {
            data = with_int(&data, COOLDOWN, cooldown);
        }
        if data.as_slice() != before {
            self.set_block_entity_data_as(pos, data, false, output);
        }
    }

    /// Whether a hopper of the region may move items into or out of the block at
    /// `pos`, and the entities about it: only where the region holds its chunk. Step
    /// N6g lends a hopper at a pinned line the land in front of it, as a circuit's part
    /// is lent what it reaches (ADR-0027; `docs/groundwork/n6-plan.md`, section 6 item
    /// 4 and section 10 item 1): its reach is the block it faces and the block above it.
    pub(super) fn hopper_reach(&self, pos: BlockPos) -> bool {
        self.chunks.contains_key(&pos.chunk())
    }

    /// `HopperBlock.onPlace`: a hopper placed where no hopper was takes whether power
    /// locks it.
    pub(super) fn hopper_on_place(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        old: BlockState,
        output: &mut TickOutput,
    ) {
        if is_class(state, BlockClass::HopperBlock) && !same_block(state, old) {
            self.check_hopper_power(pos, state, output);
        }
    }

    /// `HopperBlock.neighborChanged`, whatever changed: whether power locks it. Whether
    /// `state` is a hopper.
    pub(super) fn hopper_neighbour_changed(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        output: &mut TickOutput,
    ) -> bool {
        if !is_class(state, BlockClass::HopperBlock) {
            return false;
        }
        self.check_hopper_power(pos, state, output);
        true
    }

    /// `HopperBlock.checkPoweredState`: `enabled` where no signal reaches the hopper,
    /// set for clients only (flag 2), with no sound and no update.
    fn check_hopper_power(&mut self, pos: BlockPos, state: BlockState, output: &mut TickOutput) {
        let on = !signal::has_neighbour_signal(&mut RegionView(self), pos);
        if flag(state, "enabled") != on {
            let changed = set_flag(state, "enabled", on);
            self.set_block_updated(pos, changed, UPDATE_CLIENTS, UPDATE_LIMIT, output);
        }
    }

    /// `MinecartHopper.activateMinecart`: a hopper cart on an activator rail is switched
    /// off while the rail is powered and on while it is not, every tick it is on one.
    /// Set in the record in place: the end of the cart's tick commits it.
    pub(super) fn hopper_cart_activated(&mut self, key: EntityId, chunk: ChunkPos, powered: bool) {
        let Some(record) = self.entity_record(key, chunk) else {
            return;
        };
        let mut data = record.data.clone();
        let EntityData::Minecart { enabled, .. } = &mut data else {
            return;
        };
        if *enabled == !powered {
            return;
        }
        *enabled = !powered;
        if let Some(loaded) = self.chunks.get_mut(&chunk) {
            loaded.apply_entity(&clustine_world::EntityChange::Set(
                key,
                vec![EntityField::Data(data)],
            ));
        }
    }

    /// `MinecartHopper.tryConsumeItems` after the cart's tick, its record where the cart
    /// is: a hopper cart that is switched on takes from the container above it or the
    /// items in its box above (`HopperBlockEntity.suckInItems`, which no block above
    /// stops), and failing that from the items about its own box, the first that goes
    /// in wholly. What it holds is set in its record in place, which the end of its
    /// tick commits; no cooldown, and at most once a tick.
    pub(super) fn hopper_cart_consume(
        &mut self,
        key: EntityId,
        chunk: ChunkPos,
        position: Vec3,
        bounds: Aabb,
        output: &mut TickOutput,
    ) {
        let Some(record) = self.entity_record(key, chunk) else {
            return;
        };
        if i32::from(record.kind) != entity_types::HOPPER_MINECART {
            return;
        }
        let EntityData::Minecart {
            contents, enabled, ..
        } = &record.data
        else {
            return;
        };
        if !*enabled {
            return;
        }
        let size = super::vehicles::container_size(record.kind);
        let mut slots = slots_of(contents, size);
        let sucker = Sucker {
            at: [position[0], position[1] + 0.5, position[2]],
            grid_aligned: false,
            draw: Draw::Entity(key),
        };
        if !self.suck_in_items(sucker, &mut slots, output) {
            for (item, at) in self.items_in(&bounds.inflate([0.25, 0.0, 0.25])) {
                if self.take_item_record(item, at, &mut slots, output) {
                    break;
                }
            }
        }
        let Some(record) = self.entity_record(key, chunk) else {
            return;
        };
        let mut data = record.data.clone();
        if let EntityData::Minecart { contents, .. } = &mut data {
            let now = contents_of(&slots);
            if *contents == now {
                return;
            }
            *contents = now;
        }
        if let Some(loaded) = self.chunks.get_mut(&chunk) {
            loaded.apply_entity(&clustine_world::EntityChange::Set(
                key,
                vec![EntityField::Data(data)],
            ));
        }
    }
}

/// `HopperBlockEntity.addItem(container, itemEntity)` of the item in its tick, `item`:
/// all of its stack that goes into `slots`; an item that went wholly is gone, one that
/// went in part keeps what is left. Whether it went wholly.
fn take_from_item(slots: &mut [Option<ItemStack>], item: &mut Item) -> bool {
    let (rest, _) = add_to(slots, item.stack, false);
    match rest {
        None => {
            item.removed = true;
            true
        }
        Some(rest) => {
            item.stack = rest;
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clustine_data::items;

    fn stack(item: i32, count: i32) -> Option<ItemStack> {
        Some(ItemStack { item, count })
    }

    #[test]
    fn an_item_goes_onto_a_stack_of_its_own_before_an_empty_slot_only_in_slot_order() {
        // `tryMoveInItem` goes slot by slot: an empty slot before the stack takes it.
        let mut slots = vec![None, stack(items::STONE, 3), None];
        let (rest, fed) = add_to(
            &mut slots,
            ItemStack {
                item: items::STONE,
                count: 1,
            },
            false,
        );
        assert_eq!(rest, None);
        assert!(!fed, "the container held something");
        assert_eq!(
            slots,
            [stack(items::STONE, 1), stack(items::STONE, 3), None]
        );
    }

    #[test]
    fn a_stack_fills_what_room_there_is_and_the_rest_is_left() {
        // A stone stacks to 64: 60 and 10 make 64 and 6 left, which the next slot takes.
        let mut slots = vec![stack(items::STONE, 60), stack(items::DIRT, 1)];
        let (rest, _) = add_to(
            &mut slots,
            ItemStack {
                item: items::STONE,
                count: 10,
            },
            false,
        );
        assert_eq!(rest, stack(items::STONE, 6));
        assert_eq!(slots[0], stack(items::STONE, 64));
    }

    #[test]
    fn a_shulker_box_takes_no_shulker_box_and_anything_else() {
        let mut slots = vec![None; 27];
        let boxed = ItemStack {
            item: items::RED_SHULKER_BOX,
            count: 1,
        };
        assert_eq!(add_to(&mut slots, boxed, true), (Some(boxed), false));
        let stone = ItemStack {
            item: items::STONE,
            count: 1,
        };
        assert_eq!(add_to(&mut slots, stone, true), (None, true));
        // Any other container takes a shulker box.
        assert_eq!(add_to(&mut [None; 5], boxed, false).0, None);
    }

    #[test]
    fn a_hopper_is_full_only_when_every_slot_holds_all_it_stacks_to() {
        let mut slots = vec![stack(items::STONE, 64); 5];
        assert!(inventory_full(&slots));
        slots[4] = stack(items::ENDER_PEARL, 15);
        assert!(!inventory_full(&slots));
        slots[4] = stack(items::ENDER_PEARL, 16);
        assert!(inventory_full(&slots));
        slots[2] = None;
        assert!(!inventory_full(&slots));
    }
}
