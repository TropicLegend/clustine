//! Going to water and out of it (the brief's 7.3.15): `TryFindLiquid`, `TryFindLand` and
//! `TryFindLandNearLiquid`, one-shots that search the Manhattan box about the mob and
//! wait between searches.
//!
//! **The wait** (`MutableLong next`): each call that finds it not yet time sets it again
//! from now, so a mob asked every tick waits for as long as it is asked (the bytecode's,
//! TryFindLiquid.txt 57–135). It is kept as the moment it was last set at rest and
//! whether every tick since has set it again ([`Wait`]), written only where that
//! changes (the plan, section 8 item 4).
//!
//! The search's order within one distance is **Inferred** (`BlockPos$3` is not dumped):
//! [`manhattan_ordered`]'s.

use clustine_blocks::data::{fluid_in_tag, is_block};
use clustine_data::{Face, SupportType, blocks};
use clustine_world::BlockPos;
use serde::{Deserialize, Serialize};

use super::super::{Behaviour, Cx, MemoryKind, Status, Tracker, Value};
use super::{HeldKept, KeptShot, centre, walk};
use crate::mobs::ai::goals::manhattan_ordered;
use crate::physics::collide::{Context, collision_shape};
use crate::physics::{self, World};

/// What a search waits on: the game time `next` holds while it rests, and whether each
/// tick since the one it was last asked at has asked it again, setting it again from
/// then.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Wait {
    pub(crate) next: i64,
    /// The wait is set again from each tick as it is asked, from this game time on.
    pub(crate) again_since: Option<i64>,
}

impl Wait {
    /// `next` as the tick at game time `now` reads it, before setting it.
    fn next(self, now: i64, wait: i64) -> i64 {
        match self.again_since {
            Some(_) => now - 1 + wait,
            None => self.next,
        }
    }

    /// The tick at `now` does not reach the wait: it rests as the last tick left it.
    fn pause(&mut self, now: i64, wait: i64) {
        if self.again_since.is_some() {
            *self = Self {
                next: now - 1 + wait,
                again_since: None,
            };
        }
    }

    /// `if (time < next) { next = time + wait; return true; }`, else none: whether it
    /// waits.
    fn waits(&mut self, now: i64, wait: i64) -> bool {
        if now >= self.next(now, wait) {
            return false;
        }
        if self.again_since.is_none() {
            self.again_since = Some(now);
        }
        true
    }

    /// `next = time + wait` after a search.
    fn searched(&mut self, now: i64, wait: i64) {
        *self = Self {
            next: now + wait,
            again_since: None,
        };
    }
}

/// The three searches' shared entry: no attack target, no walk target, a look target
/// registered.
const REQUIRED: [MemoryKind; 3] = [
    MemoryKind::AttackTarget,
    MemoryKind::WalkTarget,
    MemoryKind::LookTarget,
];

fn entry_holds(cx: &Cx<'_, '_, '_>) -> bool {
    cx.memories.check(MemoryKind::AttackTarget, Status::Absent)
        && cx.memories.check(MemoryKind::WalkTarget, Status::Absent)
        && cx
            .memories
            .check(MemoryKind::LookTarget, Status::Registered)
}

/// The fluid at the mob's block is of `tag`.
fn in_fluid(cx: &Cx<'_, '_, '_>, tag: &str) -> bool {
    fluid_in_tag(cx.ai.tick.world.state(cx.ai.block_position()), tag)
}

/// The positions of the box about the mob, by Manhattan distance, but its own column's.
fn around(cx: &Cx<'_, '_, '_>, range: i32) -> Vec<BlockPos> {
    let at = cx.ai.block_position();
    manhattan_ordered(at, range, range, range)
        .into_iter()
        .filter(|pos| pos.x != at.x || pos.z != at.z)
        .collect()
}

/// The collision shape at `pos`, as the mob's own context has it, is empty.
fn shape_empty(cx: &Cx<'_, '_, '_>, pos: BlockPos) -> bool {
    let context = Context {
        stands_on_lava: false,
        bottom: cx.ai.position()[1],
        fall_distance: cx.ai.tick.living.body.fall_distance,
        falling_block: false,
        noclip: None,
    };
    collision_shape(cx.ai.tick.world.state(pos), pos, context)
        .is_none_or(|(shape, _)| shape.boxes().is_empty())
}

/// The look at and walk to `pos` a search found.
fn go_to(cx: &mut Cx<'_, '_, '_>, pos: BlockPos, speed: f32, close_enough: i32) {
    cx.memories
        .set(MemoryKind::LookTarget, Value::Tracker(Tracker::Block(pos)));
    cx.memories.set(
        MemoryKind::WalkTarget,
        walk(Tracker::Block(pos), speed, close_enough),
    );
}

/// `TryFindLiquid.create(range, speed, fluid)`: out of the fluid, to the nearest block of
/// it with air above (else the first not within 1.5 of the mob); a wait of 22 while it
/// waits, 40 after a search.
pub(crate) struct TryFindLiquid {
    pub(crate) range: i32,
    pub(crate) speed: f32,
    /// The fluid tag (`#minecraft:water`).
    pub(crate) fluid: &'static str,
}

/// `TryFindLand.create(range, speed)`: in water, to the nearest dry block one can stand
/// on; a wait of 60.
pub(crate) struct TryFindLand {
    pub(crate) range: i32,
    pub(crate) speed: f32,
}

/// `TryFindLandNearLiquid.create(range, speed, fluid)`: out of the fluid, to the nearest
/// free block on something solid beside air over the fluid; a wait of 40.
pub(crate) struct TryFindLandNearLiquid {
    pub(crate) range: i32,
    pub(crate) speed: f32,
    pub(crate) fluid: &'static str,
}

/// `TryFindLiquid`, boxed.
pub(crate) fn find_liquid(range: i32, speed: f32, fluid: &'static str) -> Box<dyn Behaviour> {
    Box::new(HeldKept::new(TryFindLiquid {
        range,
        speed,
        fluid,
    }))
}

/// `TryFindLand`, boxed.
pub(crate) fn find_land(range: i32, speed: f32) -> Box<dyn Behaviour> {
    Box::new(HeldKept::new(TryFindLand { range, speed }))
}

/// `TryFindLandNearLiquid`, boxed.
pub(crate) fn find_land_near_liquid(
    range: i32,
    speed: f32,
    fluid: &'static str,
) -> Box<dyn Behaviour> {
    Box::new(HeldKept::new(TryFindLandNearLiquid {
        range,
        speed,
        fluid,
    }))
}

impl KeptShot for TryFindLiquid {
    type State = Wait;

    fn left(&self, wait: &mut Wait, _: u32, time: i64) {
        // As of the tick after the last it was asked at.
        wait.pause(time + 1, 20 + 2);
    }

    fn required(&self) -> &[MemoryKind] {
        &REQUIRED
    }

    fn trigger(&self, wait: &mut Wait, cx: &mut Cx<'_, '_, '_>) -> bool {
        const WAIT: i64 = 20 + 2;
        let now = cx.time;
        if !entry_holds(cx) || in_fluid(cx, self.fluid) {
            wait.pause(now, WAIT);
            return false;
        }
        if wait.waits(now, WAIT) {
            return true;
        }
        let world = cx.ai.tick.world;
        let position = cx.ai.position();
        let mut found = None;
        for pos in around(cx, self.range) {
            let state = world.state(pos);
            let liquid = is_block(state, blocks::WATER) || is_block(state, blocks::LAVA);
            if !liquid || !fluid_in_tag(state, self.fluid) {
                continue;
            }
            let above = BlockPos::new(pos.x, pos.y + 1, pos.z);
            if world.state(above).props().is_air() {
                found = Some(pos);
                break;
            }
            if found.is_none() && physics::distance_sqr(centre(pos), position) >= 1.5 * 1.5 {
                found = Some(pos);
            }
        }
        if let Some(pos) = found {
            go_to(cx, pos, self.speed, 0);
        }
        wait.searched(now, 40);
        true
    }
}

impl KeptShot for TryFindLand {
    type State = Wait;

    fn left(&self, wait: &mut Wait, _: u32, time: i64) {
        // As of the tick after the last it was asked at.
        wait.pause(time + 1, 60);
    }

    fn required(&self) -> &[MemoryKind] {
        &REQUIRED
    }

    fn trigger(&self, wait: &mut Wait, cx: &mut Cx<'_, '_, '_>) -> bool {
        const WAIT: i64 = 60;
        let now = cx.time;
        if !entry_holds(cx) || !in_fluid(cx, "minecraft:water") {
            wait.pause(now, WAIT);
            return false;
        }
        if wait.waits(now, WAIT) {
            return true;
        }
        let world = cx.ai.tick.world;
        let found = around(cx, self.range).into_iter().find(|&pos| {
            let below = BlockPos::new(pos.x, pos.y - 1, pos.z);
            world.state(pos).props().fluid().is_none()
                && shape_empty(cx, pos)
                && world
                    .state(below)
                    .props()
                    .is_face_sturdy(Face::Up, SupportType::Full)
        });
        if let Some(pos) = found {
            go_to(cx, pos, self.speed, 1);
        }
        wait.searched(now, WAIT);
        true
    }
}

impl KeptShot for TryFindLandNearLiquid {
    type State = Wait;

    fn left(&self, wait: &mut Wait, _: u32, time: i64) {
        // As of the tick after the last it was asked at.
        wait.pause(time + 1, 40);
    }

    fn required(&self) -> &[MemoryKind] {
        &REQUIRED
    }

    fn trigger(&self, wait: &mut Wait, cx: &mut Cx<'_, '_, '_>) -> bool {
        const WAIT: i64 = 40;
        let now = cx.time;
        if !entry_holds(cx) || in_fluid(cx, self.fluid) {
            wait.pause(now, WAIT);
            return false;
        }
        if wait.waits(now, WAIT) {
            return true;
        }
        let world = cx.ai.tick.world;
        let found = around(cx, self.range).into_iter().find(|&pos| {
            let below = BlockPos::new(pos.x, pos.y - 1, pos.z);
            if !shape_empty(cx, pos) || shape_empty(cx, below) {
                return false;
            }
            // `Direction.Plane.HORIZONTAL`: north, east, south, west.
            [(0, -1), (1, 0), (0, 1), (-1, 0)].iter().any(|&(dx, dz)| {
                let side = BlockPos::new(pos.x + dx, pos.y, pos.z + dz);
                let under = BlockPos::new(side.x, side.y - 1, side.z);
                world.state(side).props().is_air() && fluid_in_tag(world.state(under), self.fluid)
            })
        });
        if let Some(pos) = found {
            go_to(cx, pos, self.speed, 0);
        }
        wait.searched(now, WAIT);
        true
    }
}
