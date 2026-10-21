//! `TransportItemsBetweenContainers` as the copper golem has it (`CopperGolemAi`'s first
//! idle behaviour; `docs/groundwork/n7-brief.md`, sections 7.0.5 and 7.13.5): with an
//! empty hand it walks to the nearest copper chest within 32 blocks across and 8 up or
//! down that it has not visited, stands at it for 60 ticks with the lid open, and takes
//! up to 16 of its first stack; holding something, it walks to the nearest chest or
//! trapped chest and puts it in where the chest is empty or has that item. It queues
//! while another has the chest open, remembers the chests it visited (at most ten) and
//! could not reach (at most fifty) for 6000 ticks, and rests 140 ticks where it finds
//! none.
//!
//! What it does to the chest (opening it, closing it, the items moved) is the region's,
//! after its tick ([`Local::ContainerTake`] and [`Local::ContainerPut`],
//! `region/copper_golems.rs`), which also moves the items into or out of its hand, so
//! that a chest it may not write gives nothing and takes nothing. The chest it reads in
//! its tick is the one the region then writes: nothing comes between.
//!
//! What it keeps between ticks ([`Transporting`]): its target's block and the state it
//! had when found, whether it travels, queues or is at the chest, its interaction, and
//! the moment its count of ticks at the chest began (`ticksSinceReachingTarget`, a
//! counter the game raises on every tick at the chest, kept as a moment).
//!
//! At a line (the plan's section 3, item 9): the search reads the chests of the region's
//! land and of its copies alike, as the view has no way to tell them apart (named); the
//! region writes only its own chests, and one of a copy that a golem comes to is taken
//! for unreachable when it closes it, so that the golem passes it over for 6000 ticks.
//!
//! Named differences: the chunks of the search are walked by x then z, and a chunk's
//! chests by y, then z, then x, where the game's order follows hashes not dumped (it
//! decides only between chests at exactly one distance); the search reads every block of
//! its box, where the game reads the chunks' block entities; the target "has not changed"
//! while a chest with its block entity stands at its block (the game's is the same block
//! entity); no chest is locked (`BaseContainerBlockEntity.isLocked` needs a lock
//! component, which no stack carries) and no cat sits on one; another's having the
//! chest open is its openers' count as last counted; the navigation paths to targets
//! below the surface whenever the golem walks (the game's only while this behaviour
//! runs); a tick the golem's AI does not run (no AI) counts at the chest all the same.

use clustine_blocks::connect::is_conductor;
use clustine_blocks::data::{in_tag, is_block, is_class, relative};
use clustine_blocks::growth::chest_connected_direction;
use clustine_data::{BlockClass, BlockState, Face, blocks};
use clustine_world::{BlockPos, ChunkPos, ItemStack};
use serde::{Deserialize, Serialize};

use super::copper_golem::{self, ChestStep};
use crate::living::slots;
use crate::mobs::ai::navigation;
use crate::mobs::brain::behaviours::{bottom_centre, centre, walk};
use crate::mobs::brain::{Cx, Memories, MemoryKind, Status, Timed, Tracker, Value};
use crate::mobs::tick::Local;
use crate::mobs::view::MobWorld;
use crate::physics::aabb::Aabb;
use crate::physics::collide::{Context, collision_shape};
use crate::physics::{self, Vec3};
use crate::region::golem_chests;

/// `TARGET_INTERACTION_TIME`: the ticks at a chest before the items move.
pub(crate) const INTERACTION_TIME: u32 = 60;
/// `VISITED_POSITIONS_MEMORY_TIME`.
const VISITED_MEMORY_TIME: i64 = 6000;
/// `TRANSPORTED_ITEM_MAX_STACK_SIZE`.
pub(crate) const MOST_CARRIED: i32 = 16;
/// `MAX_VISITED_POSITIONS` and `MAX_UNREACHABLE_POSITIONS`.
const MOST_VISITED: usize = 10;
const MOST_UNREACHABLE: usize = 50;
/// `IDLE_COOLDOWN`: the rest after finding nothing.
pub(crate) const IDLE_COOLDOWN: i32 = 140;
/// `CLOSE_ENOUGH_TO_START_QUEUING_DISTANCE`, `..._INTERACTING_WITH_TARGET_DISTANCE`, its
/// `_PATH_END_DISTANCE` and `CLOSE_ENOUGH_TO_CONTINUE_INTERACTING_WITH_TARGET`.
const QUEUING_DISTANCE: f64 = 3.0;
const INTERACTING_DISTANCE: f64 = 0.5;
const INTERACTING_DISTANCE_AT_PATH_END: f64 = 1.0;
const CONTINUING_DISTANCE: f64 = 2.0;
/// The golem's search (`TRANSPORT_ITEM_HORIZONTAL_SEARCH_RADIUS` and its vertical), and
/// a passenger's (`PASSENGER_MOB_TARGET_SEARCH_DISTANCE`).
const HORIZONTAL_SEARCH: i32 = 32;
const VERTICAL_SEARCH: i32 = 8;
const PASSENGER_SEARCH: i32 = 1;
/// The golem's `speedModifier`.
const SPEED: f32 = 1.0;

/// `TransportItemsBetweenContainers(1, copper chests, chests, 32, 8, …)` of the copper
/// golem, a `Behavior` of 60 ticks that never times out.
pub(crate) struct TransportItemsBetweenContainers;

/// `TransportItemState`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Way {
    #[default]
    Travelling,
    Queuing,
    Interacting,
}

/// `ContainerInteractionState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Reached {
    /// `PICKUP_ITEM`.
    Pickup,
    /// `PICKUP_NO_ITEM`.
    PickupNothing,
    /// `PLACE_ITEM`.
    Place,
    /// `PLACE_NO_ITEM`.
    PlaceNothing,
}

impl Reached {
    /// Whether it is a taking (the golem's empty hand at the chest).
    pub(crate) fn takes(self) -> bool {
        matches!(self, Reached::Pickup | Reached::PickupNothing)
    }
}

/// `TransportItemTarget` as kept: the chest's block, and its state when found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Target {
    pub(crate) pos: BlockPos,
    pub(crate) state: u16,
}

impl Target {
    fn state(self) -> BlockState {
        BlockState(self.state)
    }
}

/// What the behaviour keeps between ticks (its object's fields in the game).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Transporting {
    pub(crate) target: Option<Target>,
    pub(crate) way: Way,
    pub(crate) interaction: Option<Reached>,
    /// The `lived` of its first tick at the chest, from which `ticksSinceReachingTarget`
    /// counts: 1 then, one more each tick after; none for 0.
    pub(crate) at_chest_since: Option<u32>,
}

impl Transporting {
    /// `ticksSinceReachingTarget` at `now`, before that tick's count.
    pub(crate) fn ticks(&self, now: u32) -> u32 {
        self.at_chest_since
            .map_or(0, |since| now.saturating_sub(since))
    }
}

// ------------------------------------------------------------------------------------
// What the golem reads of chests.

/// The chests the search takes (`ChestBlockEntity`): a chest, a trapped chest, a copper
/// chest, of their block entity.
fn is_chest(world: &dyn MobWorld, pos: BlockPos, state: BlockState) -> bool {
    is_class(state, BlockClass::ChestBlock) && world.block_entity_data(pos).is_some()
}

/// `ChestBlock.isChestBlockedAt(level, pos)`: a redstone conductor above it (no cat
/// sits on a chest).
fn blocked_at(world: &dyn MobWorld, pos: BlockPos) -> bool {
    is_conductor(world.state(relative(pos, Face::Up)))
}

/// `ChestBlock.getConnectedBlockPos(pos, state)` where the chest is not single: the
/// block of its other half.
fn connected(pos: BlockPos, state: BlockState) -> Option<BlockPos> {
    if !matches!(state.property("type"), Some("left" | "right")) {
        return None;
    }
    chest_connected_direction(state).map(|towards| relative(pos, towards))
}

/// The halves of the chest at `pos` (`ChestBlock.combine`), the right (first) one
/// first, and whether either is blocked: as the region's containers have them, read
/// through the view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Halves {
    pub(crate) first: BlockPos,
    pub(crate) second: Option<BlockPos>,
    pub(crate) blocked: bool,
}

/// The halves of the chest at `pos`, where a chest with its block entity is there.
pub(crate) fn halves(world: &dyn MobWorld, pos: BlockPos) -> Option<Halves> {
    let state = world.block(pos)?;
    if !is_chest(world, pos, state) {
        return None;
    }
    let single = Halves {
        first: pos,
        second: None,
        blocked: blocked_at(world, pos),
    };
    let Some(partner) = connected(pos, state) else {
        return Some(single);
    };
    let kind = state.property("type");
    let joins = world.block(partner).is_some_and(|other| {
        clustine_blocks::data::same_block(other, state)
            && matches!(other.property("type"), Some("left" | "right"))
            && other.property("type") != kind
            && clustine_blocks::data::facing(other) == clustine_blocks::data::facing(state)
    }) && world.block_entity_data(partner).is_some();
    if !joins {
        return Some(single);
    }
    let (first, second) = if kind == Some("right") {
        (pos, partner)
    } else {
        (partner, pos)
    };
    Some(Halves {
        first,
        second: Some(second),
        blocked: single.blocked || blocked_at(world, partner),
    })
}

/// The slots of the chest of `halves` (`CompoundContainer`: the first half's, then the
/// second's).
pub(crate) fn slots_of(world: &dyn MobWorld, halves: Halves) -> Vec<Option<ItemStack>> {
    let mut slots = Vec::new();
    for pos in [Some(halves.first), halves.second].into_iter().flatten() {
        let read = world
            .block(pos)
            .zip(world.block_entity_data(pos))
            .and_then(|(state, data)| golem_chests::slots(state, data));
        slots.extend(read.unwrap_or_default());
    }
    slots
}

/// `ChestBlock.getContainer(block, state, level, pos, false)` of the chest at `pos`: its
/// slots, none where it is blocked.
fn container(world: &dyn MobWorld, pos: BlockPos) -> Option<Vec<Option<ItemStack>>> {
    let halves = halves(world, pos)?;
    (!halves.blocked).then(|| slots_of(world, halves))
}

/// `Container.isEmpty()`.
fn empty(slots: &[Option<ItemStack>]) -> bool {
    slots.iter().flatten().all(|stack| stack.count <= 0)
}

/// `hasItemMatchingHandItem`: a slot of the hand's item (`ItemStack.isSameItem`; an
/// empty slot is air, which no hand holds).
fn has_hand_item(slots: &[Option<ItemStack>], hand: ItemStack) -> bool {
    slots
        .iter()
        .flatten()
        .any(|stack| stack.count > 0 && stack.item == hand.item)
}

/// `pickupItemFromContainer`: the first slot with anything, and how much of it is
/// taken (at most 16).
pub(crate) fn pickup(slots: &[Option<ItemStack>]) -> Option<(usize, i32)> {
    slots.iter().enumerate().find_map(|(index, slot)| {
        slot.filter(|stack| stack.count > 0)
            .map(|stack| (index, stack.count.min(MOST_CARRIED)))
    })
}

/// `addItemsToContainer(mob, container)`: `hand` into `slots` in order, the first empty
/// slot taking all that is left, a stack of the same item (and components) not full
/// topped up on the way; what is left of the hand (none for nothing).
pub(crate) fn add_items(slots: &mut [Option<ItemStack>], hand: ItemStack) -> Option<ItemStack> {
    let mut hand = hand;
    for slot in slots.iter_mut() {
        match slot {
            Some(stack) if stack.count > 0 => {
                let most = physics::item::max_stack_size(*stack);
                if stack.item != hand.item || stack.count >= most {
                    continue;
                }
                let room = most - stack.count;
                stack.count += room.min(hand.count);
                // The game takes the room from the hand, not what it moved: a hand with
                // less than the room is left below nothing, which is empty.
                hand.count -= room;
                if hand.count <= 0 {
                    return None;
                }
            }
            _ => {
                *slot = Some(hand);
                return None;
            }
        }
    }
    Some(hand)
}

/// `isWantedBlock(mob, state)`: with an empty hand a copper chest, else a chest or a
/// trapped chest.
fn wanted(hand_empty: bool, state: BlockState) -> bool {
    if hand_empty {
        in_tag(state, "minecraft:copper_chests")
    } else {
        is_block(state, blocks::CHEST) || is_block(state, blocks::TRAPPED_CHEST)
    }
}

/// The golem's main hand, if it holds anything.
fn hand(cx: &Cx<'_, '_, '_>) -> Option<ItemStack> {
    cx.ai.tick.mob.body.equipment.slots[usize::from(slots::MAIN_HAND)]
        .filter(|stack| stack.count > 0)
}

/// A memory of a set of blocks, as a sorted list; none for none.
fn positions(cx: &Cx<'_, '_, '_>, kind: MemoryKind) -> Vec<BlockPos> {
    positions_in(cx.memories, kind)
}

/// A memory of a set of blocks in `memories`, as a sorted list; none for none.
fn positions_in(memories: &Memories, kind: MemoryKind) -> Vec<BlockPos> {
    match memories.get(kind) {
        Some(Value::Blocks(blocks)) => blocks,
        _ => Vec::new(),
    }
}

/// `markVisitedBlockPosAsUnreachable(mob, level, pos)` of the golem's memories: the
/// chest out of its visits and into its unreachable chests, both for 6000 ticks; past
/// fifty of them, its rest instead (`enterCooldownAfterNoMatchingTargetFound`'s
/// memories), which it says, its target to be given up.
pub(crate) fn mark_unreachable_in(memories: &mut Memories, pos: BlockPos) -> bool {
    let mut visited = positions_in(memories, MemoryKind::VisitedBlockPositions);
    visited.retain(|visit| *visit != pos);
    let mut unreachable = positions_in(memories, MemoryKind::UnreachableTransportBlockPositions);
    if let Err(at) = unreachable.binary_search(&pos) {
        unreachable.insert(at, pos);
    }
    if unreachable.len() > MOST_UNREACHABLE {
        memories.set(
            MemoryKind::TransportItemsCooldownTicks,
            Value::Int(IDLE_COOLDOWN),
        );
        memories.erase(MemoryKind::VisitedBlockPositions);
        memories.erase(MemoryKind::UnreachableTransportBlockPositions);
        return true;
    }
    memories.set_with_expiry(
        MemoryKind::VisitedBlockPositions,
        Value::Blocks(visited),
        VISITED_MEMORY_TIME,
    );
    memories.set_with_expiry(
        MemoryKind::UnreachableTransportBlockPositions,
        Value::Blocks(unreachable),
        VISITED_MEMORY_TIME,
    );
    false
}

/// `Vec3i.distToCenterSqr(position)`.
fn dist_to_centre_sqr(pos: BlockPos, from: Vec3) -> f64 {
    physics::distance_sqr(centre(pos), from)
}

/// The block's collision shape's bounds (`getCollisionShape(level, pos).bounds()`), at
/// its block; none for an empty shape.
fn shape_bounds(state: BlockState, pos: BlockPos, context: Context) -> Option<Aabb> {
    let (shape, _) = collision_shape(state, pos, context)?;
    shape.boxes().into_iter().reduce(|a, b| Aabb {
        min: std::array::from_fn(|axis| a.min[axis].min(b.min[axis])),
        max: std::array::from_fn(|axis| a.max[axis].max(b.max[axis])),
    })
}

/// A block clip from `from` to `to` (`ClipContext(COLLIDER, NONE, mob)`): the first block
/// whose collision shape the segment meets, if any.
fn clip(world: &dyn MobWorld, from: Vec3, to: Vec3, context: Context) -> Option<BlockPos> {
    let mut hit = None;
    physics::walk::traverse(from, to, |pos| {
        let state = world.state(pos);
        let meets = collision_shape(state, pos, context).is_some_and(|(shape, _)| {
            shape
                .boxes()
                .iter()
                .any(|bounds| physics::body::clips_shape(bounds, from, to))
        });
        if meets {
            hit = Some(pos);
        }
        meets
    });
    hit
}

// ------------------------------------------------------------------------------------
// The behaviour.

impl TransportItemsBetweenContainers {
    /// The golem's collision context.
    fn context(cx: &Cx<'_, '_, '_>) -> Context {
        Context {
            bottom: cx.ai.position()[1],
            fall_distance: cx.ai.tick.living.body.fall_distance,
            falling_block: false,
            noclip: None,
        }
    }

    /// `setMiddleYPosition(mob, pos)`: raised by half its height.
    fn middle(cx: &Cx<'_, '_, '_>, pos: Vec3) -> Vec3 {
        let height = cx.ai.tick.living.body.bounds();
        [
            pos[0],
            pos[1] + (height.max[1] - height.min[1]) / 2.0,
            pos[2],
        ]
    }

    /// `getCenterPos(mob)`.
    fn centre_pos(cx: &Cx<'_, '_, '_>) -> Vec3 {
        Self::middle(cx, cx.ai.position())
    }

    /// `hasFinishedPath(mob)`: it has a path, done.
    fn finished_path(cx: &Cx<'_, '_, '_>) -> bool {
        cx.ai.tick.mob.body.path.followed.is_some() && navigation::is_done(cx.ai)
    }

    /// `getInteractionRange(mob)`: 1 at the end of its path, else 0.5.
    fn interaction_range(cx: &Cx<'_, '_, '_>) -> f64 {
        if Self::finished_path(cx) {
            INTERACTING_DISTANCE_AT_PATH_END
        } else {
            INTERACTING_DISTANCE
        }
    }

    /// `isWithinTargetDistance(distance, target, level, mob, pos)`: a box of the golem's
    /// size about `pos` meets the target's shape grown by `distance` across and 0.5 up
    /// and down.
    fn within(cx: &Cx<'_, '_, '_>, distance: f64, target: Target, pos: Vec3) -> bool {
        let own = cx.ai.tick.living.body.bounds();
        let half = [
            (own.max[0] - own.min[0]) / 2.0,
            (own.max[1] - own.min[1]) / 2.0,
            (own.max[2] - own.min[2]) / 2.0,
        ];
        let around = Aabb::new(
            [pos[0] - half[0], pos[1] - half[1], pos[2] - half[2]],
            [pos[0] + half[0], pos[1] + half[1], pos[2] + half[2]],
        );
        shape_bounds(target.state(), target.pos, Self::context(cx))
            .is_some_and(|shape| shape.inflate([distance, 0.5, distance]).intersects(around))
    }

    /// `canSeeAnyTargetSide(target, level, mob, pos)`: a clip from `pos` to a point half
    /// a block out of the chest's centre, the faces taken down, up, north, south, west,
    /// east, first meets the chest.
    fn sees_a_side(cx: &Cx<'_, '_, '_>, target: Target, from: Vec3) -> bool {
        let middle = centre(target.pos);
        let context = Self::context(cx);
        [
            Face::Down,
            Face::Up,
            Face::North,
            Face::South,
            Face::West,
            Face::East,
        ]
        .into_iter()
        .any(|face| {
            let [dx, dy, dz] = physics::piston::step(face);
            let to = [
                middle[0] + 0.5 * f64::from(dx),
                middle[1] + 0.5 * f64::from(dy),
                middle[2] + 0.5 * f64::from(dz),
            ];
            from != to && clip(cx.ai.tick.world, from, to, context) == Some(target.pos)
        })
    }

    /// `hasValidTravellingPath(level, target, mob)`: from the end of its path (or a path
    /// made now, or where it stands), within its interaction range and seeing a side of
    /// the chest; or with no path at all and out of range.
    fn valid_travelling_path(cx: &mut Cx<'_, '_, '_>, target: Target) -> bool {
        let end = match cx.ai.tick.mob.body.path.followed.as_ref() {
            Some(path) => Some(path.nodes.last().map(|node| node.pos())),
            None => navigation::create_path(cx.ai, target.pos, 0).map(|made| {
                made.path(cx.ai)
                    .and_then(|path| path.nodes.last().map(|node| node.pos()))
            }),
        };
        let has_path = end.is_some();
        let from = match end.flatten() {
            Some(node) => Self::middle(cx, bottom_centre(node)),
            None => Self::middle(cx, cx.ai.position()),
        };
        let within = Self::within(cx, Self::interaction_range(cx), target, from);
        (!has_path && !within) || (within && Self::sees_a_side(cx, target, from))
    }

    /// `hasValidTarget(level, mob)`: a target still wanted and unchanged, not blocked,
    /// and while it travels, with a way to it (else marked unreachable).
    fn has_valid_target(state: &mut Transporting, cx: &mut Cx<'_, '_, '_>) -> bool {
        let Some(target) = state.target else {
            return false;
        };
        let world = cx.ai.tick.world;
        let unchanged = world
            .block(target.pos)
            .is_some_and(|now| is_chest(world, target.pos, now));
        if !wanted(hand(cx).is_none(), target.state()) || !unchanged {
            return false;
        }
        if blocked_at(world, target.pos) {
            return false;
        }
        if state.way != Way::Travelling || Self::valid_travelling_path(cx, target) {
            return true;
        }
        Self::mark_unreachable(state, cx, target.pos);
        false
    }

    /// `getTransportTarget(level, mob)`: the nearest chest by its centre, of the chunks
    /// within the search's reach that the view has, whose block is in the search's box
    /// and which it may pick.
    fn transport_target(cx: &Cx<'_, '_, '_>) -> Option<Target> {
        let world = cx.ai.tick.world;
        let at = cx.ai.block_position();
        let from = cx.ai.position();
        let passenger = cx.ai.tick.vehicle.is_some();
        let (across, up) = if passenger {
            (PASSENGER_SEARCH, PASSENGER_SEARCH)
        } else {
            (HORIZONTAL_SEARCH, VERTICAL_SEARCH)
        };
        let visited = positions(cx, MemoryKind::VisitedBlockPositions);
        let unreachable = positions(cx, MemoryKind::UnreachableTransportBlockPositions);
        let hand_empty = hand(cx).is_none();
        let home = at.chunk();
        let reach = across.div_euclid(16) + 1;
        let mut best: Option<Target> = None;
        let mut best_distance = f64::from(f32::MAX);
        for chunk_x in home.x - reach..=home.x + reach {
            for chunk_z in home.z - reach..=home.z + reach {
                let chunk = ChunkPos::new(chunk_x, chunk_z);
                if !world.has_chunk(chunk) {
                    continue;
                }
                let x_range =
                    (chunk_x * 16).max(at.x - across)..=(chunk_x * 16 + 15).min(at.x + across);
                let z_range =
                    (chunk_z * 16).max(at.z - across)..=(chunk_z * 16 + 15).min(at.z + across);
                for y in at.y - up..=at.y + up {
                    for z in z_range.clone() {
                        for x in x_range.clone() {
                            let pos = BlockPos::new(x, y, z);
                            let Some(state) = world.block(pos) else {
                                continue;
                            };
                            if !is_chest(world, pos, state) {
                                continue;
                            }
                            let distance = dist_to_centre_sqr(pos, from);
                            if distance < best_distance
                                && valid_to_pick(
                                    world,
                                    pos,
                                    state,
                                    hand_empty,
                                    &visited,
                                    &unreachable,
                                )
                            {
                                best = Some(Target {
                                    pos,
                                    state: state.0,
                                });
                                best_distance = distance;
                            }
                        }
                    }
                }
            }
        }
        best
    }

    /// `updateInvalidTarget(level, mob)`: an invalid target given up for the nearest
    /// one, which it travels to and has visited, or, with none, its rest; whether it
    /// changed.
    fn update_invalid_target(state: &mut Transporting, cx: &mut Cx<'_, '_, '_>) -> bool {
        if Self::has_valid_target(state, cx) {
            return false;
        }
        Self::stop_targeting(state, cx);
        match Self::transport_target(cx) {
            Some(found) => {
                state.target = Some(found);
                Self::on_start_travelling(state, cx);
                Self::set_visited(state, cx, found.pos);
            }
            None => Self::enter_cooldown(state, cx),
        }
        true
    }

    /// `isAnotherMobInteractingWithTarget(target, level)`: either half of the chest has
    /// someone at it (`shouldQueueForTarget`: its openers).
    fn another_at(cx: &Cx<'_, '_, '_>, target: Target) -> bool {
        let world = cx.ai.tick.world;
        connected_targets(world, target.pos, target.state())
            .into_iter()
            .any(|pos| {
                world
                    .block_entity_data(pos)
                    .is_some_and(|data| golem_chests::openers(data) > 0)
            })
    }

    /// `walkTowardsTarget(mob)`: a look at and a walk to the chest's block.
    fn walk_towards(state: &Transporting, cx: &mut Cx<'_, '_, '_>) {
        let Some(target) = state.target else {
            return;
        };
        let tracker = Tracker::Block(target.pos);
        cx.memories
            .set(MemoryKind::LookTarget, Value::Tracker(tracker));
        cx.memories
            .set(MemoryKind::WalkTarget, walk(tracker, SPEED, 0));
    }

    /// `stopInPlace(mob)`: its path dropped, its inputs and its motion across at nothing.
    fn stop_in_place(cx: &mut Cx<'_, '_, '_>) {
        navigation::stop(cx.ai);
        let inputs = &mut cx.ai.tick.mob.body.inputs;
        inputs.xxa = 0.0;
        inputs.yya = 0.0;
        // `Mob.setSpeed` sets its forward input too.
        inputs.speed = 0.0;
        inputs.zza = 0.0;
        let [_, y, _] = cx.ai.tick.living.body.motion;
        cx.ai.tick.living.body.motion = [0.0, y, 0.0];
    }

    /// `onQueuingForTarget`: on its way again once nobody is at the chest.
    fn on_queuing(state: &mut Transporting, cx: &mut Cx<'_, '_, '_>, target: Target) {
        if !Self::another_at(cx, target) {
            state.way = Way::Travelling;
            Self::walk_towards(state, cx);
        }
    }

    /// `onTravelToTarget`: within 3 of a chest someone has open, it queues; within its
    /// interaction range, it begins its interaction; else it walks on.
    fn on_travel(state: &mut Transporting, cx: &mut Cx<'_, '_, '_>, target: Target) {
        let middle = Self::centre_pos(cx);
        if Self::within(cx, QUEUING_DISTANCE, target, middle) && Self::another_at(cx, target) {
            Self::stop_in_place(cx);
            state.way = Way::Queuing;
        } else if Self::within(cx, Self::interaction_range(cx), target, middle) {
            let slots = chest_slots(cx.ai.tick.world, target.pos);
            state.interaction = Some(match hand(cx) {
                None if !empty(&slots) => Reached::Pickup,
                None => Reached::PickupNothing,
                Some(held) if empty(&slots) || has_hand_item(&slots, held) => Reached::Place,
                Some(_) => Reached::PlaceNothing,
            });
            state.way = Way::Interacting;
        } else {
            Self::walk_towards(state, cx);
        }
    }

    /// `onReachedTarget`: within 2 of the chest, one tick more at it, the look and the
    /// golem's own doing of the tick; at the 60th the items move, and it travels again.
    fn on_reached(state: &mut Transporting, cx: &mut Cx<'_, '_, '_>, target: Target) {
        let middle = Self::centre_pos(cx);
        if !Self::within(cx, CONTINUING_DISTANCE, target, middle) {
            Self::on_start_travelling(state, cx);
            return;
        }
        let now = cx.ai.tick.lived;
        state.at_chest_since.get_or_insert(now);
        let ticks = state.ticks(now) + 1;
        // `onTargetInteraction`.
        cx.memories.set(
            MemoryKind::LookTarget,
            Value::Tracker(Tracker::Block(target.pos)),
        );
        Self::stop_in_place(cx);
        if let Some(interaction) = state.interaction {
            copper_golem::at_chest(cx, target.pos, interaction, ticks);
        }
        if ticks >= INTERACTION_TIME {
            Self::interact(state, cx, target);
            Self::on_start_travelling(state, cx);
        }
    }

    /// `doReachedTargetInteraction` at the 60th tick, of the chest as it is now: an
    /// empty hand takes from a chest with anything, a full one puts into a chest that
    /// is empty or has its item; else the golem gives the chest up.
    fn interact(state: &mut Transporting, cx: &mut Cx<'_, '_, '_>, target: Target) {
        let slots = chest_slots(cx.ai.tick.world, target.pos);
        match hand(cx) {
            None if !empty(&slots) => {
                // `pickUpItems`: the region moves the stack into its hand.
                cx.ai.tick.out.local.push(Local::ContainerTake {
                    chest: target.pos,
                    step: ChestStep::Move,
                });
                Self::clear_memories_after_found(state, cx);
            }
            Some(held) if empty(&slots) || has_hand_item(&slots, held) => {
                // `putDownItem`: the region moves the stack out of its hand.
                let mut slots = slots;
                let rest = add_items(&mut slots, held);
                cx.ai.tick.out.local.push(Local::ContainerPut {
                    chest: target.pos,
                    step: ChestStep::Move,
                });
                if rest.is_none() {
                    Self::clear_memories_after_found(state, cx);
                } else {
                    Self::stop_targeting(state, cx);
                }
            }
            _ => Self::stop_targeting(state, cx),
        }
    }

    /// `setVisitedBlockPos(mob, level, pos)`: its visits and this one, for 6000 ticks;
    /// past ten, its rest.
    fn set_visited(state: &mut Transporting, cx: &mut Cx<'_, '_, '_>, pos: BlockPos) {
        let mut visited = positions(cx, MemoryKind::VisitedBlockPositions);
        if let Err(at) = visited.binary_search(&pos) {
            visited.insert(at, pos);
        }
        if visited.len() > MOST_VISITED {
            Self::enter_cooldown(state, cx);
        } else {
            cx.memories.set_with_expiry(
                MemoryKind::VisitedBlockPositions,
                Value::Blocks(visited),
                VISITED_MEMORY_TIME,
            );
        }
    }

    /// `markVisitedBlockPosAsUnreachable(mob, level, pos)` ([`mark_unreachable_in`]),
    /// its target given up where that is its rest.
    fn mark_unreachable(state: &mut Transporting, cx: &mut Cx<'_, '_, '_>, pos: BlockPos) {
        if mark_unreachable_in(cx.memories, pos) {
            Self::stop_targeting(state, cx);
        }
    }

    /// `stopTargetingCurrentTarget(mob)`.
    fn stop_targeting(state: &mut Transporting, cx: &mut Cx<'_, '_, '_>) {
        state.at_chest_since = None;
        state.target = None;
        navigation::stop(cx.ai);
        cx.memories.erase(MemoryKind::WalkTarget);
    }

    /// `clearMemoriesAfterMatchingTargetFound(mob)`.
    fn clear_memories_after_found(state: &mut Transporting, cx: &mut Cx<'_, '_, '_>) {
        Self::stop_targeting(state, cx);
        cx.memories.erase(MemoryKind::VisitedBlockPositions);
        cx.memories
            .erase(MemoryKind::UnreachableTransportBlockPositions);
    }

    /// `enterCooldownAfterNoMatchingTargetFound(mob)`: its rest of 140 (counted down by
    /// its core), its memories of chests forgotten.
    fn enter_cooldown(state: &mut Transporting, cx: &mut Cx<'_, '_, '_>) {
        Self::stop_targeting(state, cx);
        cx.memories.set(
            MemoryKind::TransportItemsCooldownTicks,
            Value::Int(IDLE_COOLDOWN),
        );
        cx.memories.erase(MemoryKind::VisitedBlockPositions);
        cx.memories
            .erase(MemoryKind::UnreachableTransportBlockPositions);
    }

    /// `onStartTravelling(mob)`: the golem's own (`onTravelling`), and it travels with
    /// no interaction and no tick at the chest.
    fn on_start_travelling(state: &mut Transporting, cx: &mut Cx<'_, '_, '_>) {
        copper_golem::on_travelling(cx);
        state.way = Way::Travelling;
        state.interaction = None;
        state.at_chest_since = None;
    }
}

/// The slots of the chest at `pos` however it is now, blocked or not (the target's
/// container, which the game found once and keeps).
fn chest_slots(world: &dyn MobWorld, pos: BlockPos) -> Vec<Option<ItemStack>> {
    halves(world, pos).map_or_else(Vec::new, |halves| slots_of(world, halves))
}

/// `getConnectedTargets(target, level)`: the chest's block, and its other half where it
/// is not single and the other is a chest it could take (`tryCreatePossibleTarget`).
fn connected_targets(world: &dyn MobWorld, pos: BlockPos, state: BlockState) -> Vec<BlockPos> {
    let mut targets = vec![pos];
    if let Some(partner) = connected(pos, state)
        && container(world, partner).is_some()
    {
        targets.push(partner);
    }
    targets
}

/// `isTargetValidToPick`: a chest whose container it can take (not blocked), of the kind
/// it wants, of which neither half is among its visits or its unreachable chests.
fn valid_to_pick(
    world: &dyn MobWorld,
    pos: BlockPos,
    state: BlockState,
    hand_empty: bool,
    visited: &[BlockPos],
    unreachable: &[BlockPos],
) -> bool {
    container(world, pos).is_some()
        && wanted(hand_empty, state)
        && !connected_targets(world, pos, state)
            .iter()
            .any(|half| visited.contains(half) || unreachable.contains(half))
}

impl Timed for TransportItemsBetweenContainers {
    type State = Transporting;

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[
            (MemoryKind::VisitedBlockPositions, Status::Registered),
            (
                MemoryKind::UnreachableTransportBlockPositions,
                Status::Registered,
            ),
            (MemoryKind::TransportItemsCooldownTicks, Status::Absent),
            (MemoryKind::IsPanicking, Status::Absent),
        ]
    }

    /// Not leashed.
    fn extra_start(&self, _: &mut Transporting, cx: &mut Cx<'_, '_, '_>) -> bool {
        !cx.ai.tick.leashed
    }

    /// No rest, no panic, no lead.
    fn can_still_use(&self, _: &mut Transporting, cx: &mut Cx<'_, '_, '_>) -> bool {
        cx.memories
            .check(MemoryKind::TransportItemsCooldownTicks, Status::Absent)
            && !cx.memories.check(MemoryKind::IsPanicking, Status::Present)
            && !cx.ai.tick.leashed
    }

    fn timed_out(&self, _: &Transporting, _: &Cx<'_, '_, '_>, _: i64) -> bool {
        false
    }

    fn tick(&self, state: &mut Transporting, cx: &mut Cx<'_, '_, '_>) {
        let changed = Self::update_invalid_target(state, cx);
        let Some(target) = state.target else {
            // `stop`, called from its tick: its status stays running until the next.
            self.stop(state, cx);
            return;
        };
        if changed {
            return;
        }
        if state.way == Way::Queuing {
            Self::on_queuing(state, cx, target);
        }
        if state.way == Way::Travelling {
            Self::on_travel(state, cx, target);
        }
        if state.way == Way::Interacting {
            Self::on_reached(state, cx, target);
        }
    }

    fn stop(&self, state: &mut Transporting, cx: &mut Cx<'_, '_, '_>) {
        Self::on_start_travelling(state, cx);
    }
}
