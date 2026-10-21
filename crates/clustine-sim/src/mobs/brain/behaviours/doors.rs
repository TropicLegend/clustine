//! `InteractWithDoor` (the brief's 7.3.21): a walker on a path opens the wooden and copper
//! doors at its last and next nodes, remembers them, and closes those it has passed
//! once it is three blocks away and no mob of its kind is on its way through.
//!
//! The doors to close are kept sorted by position, where the game keeps a `HashSet` of
//! `GlobalPos` whose order follows hashes not dumped: the order decides only which of two
//! doors is looked at first (the plan, section 2.7, named). A door in another region's
//! land is not opened (`Local::OpenDoor`'s). Another mob's path is read from its body,
//! which a copy carries (the plan, section 8 item 8).

use clustine_blocks::data::{flag, in_tag};
use clustine_world::{BlockPos, EntityId};
use serde::{Deserialize, Serialize};

use super::super::{Behaviour, Cx, MemoryKind, Status, Value};
use super::{Counter, HeldKept, KeptShot, centre};
use crate::mobs::Mob;
use crate::physics::{self, World};

/// The doors a mob opens and closes (`#mob_interactable_doors`, a `DoorBlock`).
fn is_door(state: clustine_data::BlockState) -> bool {
    in_tag(state, "minecraft:mob_interactable_doors") && state.property("hinge").is_some()
}

/// The closure's state: `lastNode` and `cooldown`, the cooldown a [`Counter`] lowered on
/// the ticks its next node is not the last one seen.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Doorway {
    pub(crate) last_node: Option<[i32; 3]>,
    pub(crate) cooldown: Counter,
}

/// `InteractWithDoor.create()`.
pub(crate) struct InteractWithDoor;

/// `InteractWithDoor.create()`, boxed.
pub(crate) fn interact_with_door() -> Box<dyn Behaviour> {
    Box::new(HeldKept::new(InteractWithDoor))
}

/// The previous and next node of a path, and whether it is under way.
struct Nodes {
    previous: BlockPos,
    next: BlockPos,
}

/// The path the body follows, its previous and next node; none where it has not
/// started or is done.
fn nodes_of(mob: &Mob) -> Option<Nodes> {
    let path = mob.body.path.followed.as_ref()?;
    let index = mob.body.progress.next as usize;
    if index == 0 || index >= path.nodes.len() {
        return None;
    }
    let at = |node: &crate::mobs::ai::pathfinder::PathNode| BlockPos::new(node.x, node.y, node.z);
    Some(Nodes {
        previous: at(&path.nodes[index - 1]),
        next: at(&path.nodes[index]),
    })
}

/// `rememberDoorToClose`: the door added to the set.
fn remember(doors: &mut Vec<BlockPos>, door: BlockPos) {
    if let Err(at) = doors.binary_search(&door) {
        doors.insert(at, door);
    }
}

impl KeptShot for InteractWithDoor {
    type State = Doorway;

    fn required(&self) -> &[MemoryKind] {
        &[MemoryKind::Path, MemoryKind::DoorsToClose, MemoryKind::Mobs]
    }

    fn trigger(&self, state: &mut Doorway, cx: &mut Cx<'_, '_, '_>) -> bool {
        let now = cx.ai.tick.lived;
        if !cx.memories.check(MemoryKind::Path, Status::Present)
            || !cx
                .memories
                .check(MemoryKind::DoorsToClose, Status::Registered)
            || !cx.memories.check(MemoryKind::Mobs, Status::Registered)
        {
            state.cooldown.pause(now);
            return false;
        }
        let Some(nodes) = nodes_of(&cx.ai.tick.mob) else {
            state.cooldown.pause(now);
            return false;
        };
        let next = [nodes.next.x, nodes.next.y, nodes.next.z];
        if state.last_node == Some(next) {
            state.cooldown = Counter::at_rest(20);
        } else {
            // `--cooldown > 0`: it waits while the cooldown lasts.
            let left = state.cooldown.at(now) - 1;
            if left > 0 {
                state.cooldown.lower(now);
                return false;
            }
            state.cooldown = Counter::at_rest(left);
        }
        state.last_node = Some(next);
        let world = cx.ai.tick.world;
        let mut doors: Option<Vec<BlockPos>> = match cx.memories.get(MemoryKind::DoorsToClose) {
            Some(Value::Blocks(doors)) => Some(doors),
            _ => None,
        };
        let previous = world.state(nodes.previous);
        if is_door(previous) {
            if !flag(previous, "open") {
                cx.ai.tick.set_door_open(nodes.previous, previous, true);
            }
            remember(doors.get_or_insert_with(Vec::new), nodes.previous);
        }
        let following = world.state(nodes.next);
        if is_door(following) && !flag(following, "open") {
            cx.ai.tick.set_door_open(nodes.next, following, true);
            remember(doors.get_or_insert_with(Vec::new), nodes.next);
        }
        if let Some(mut doors) = doors {
            close_passed(cx, &nodes, &mut doors);
            cx.memories
                .set(MemoryKind::DoorsToClose, Value::Blocks(doors));
        }
        true
    }
}

/// `closeDoorsThatIHaveOpenedOrPassedThrough`: each remembered door but the two nodes',
/// forgotten where it is three blocks or more away, no longer an open door, or (without
/// closing it) on the way of another mob of the kind within 2 of it; else closed and
/// forgotten.
fn close_passed(cx: &mut Cx<'_, '_, '_>, nodes: &Nodes, doors: &mut Vec<BlockPos>) {
    let world = cx.ai.tick.world;
    let position = cx.ai.position();
    let others: Vec<EntityId> = match cx.memories.get(MemoryKind::Mobs) {
        Some(Value::Entities(list)) => list,
        _ => Vec::new(),
    };
    let kind = cx.ai.tick.kind;
    let mut kept = Vec::new();
    for door in std::mem::take(doors) {
        if door == nodes.previous || door == nodes.next {
            kept.push(door);
            continue;
        }
        if physics::distance_sqr(centre(door), position) >= 3.0 * 3.0 {
            continue;
        }
        let state = world.state(door);
        if !is_door(state) || !flag(state, "open") {
            continue;
        }
        let passing = others.iter().any(|&key| {
            let Some(seen) = world.entity(key) else {
                return false;
            };
            if seen.entity.kind != kind
                || physics::distance_sqr(centre(door), seen.entity.position) >= 2.0 * 2.0
            {
                return false;
            }
            Mob::decode(&seen.entity.data)
                .ok()
                .and_then(|mob| nodes_of(&mob))
                .is_some_and(|nodes| nodes.previous == door || nodes.next == door)
        });
        if passing {
            continue;
        }
        cx.ai.tick.set_door_open(door, state, false);
    }
    *doors = kept;
}
