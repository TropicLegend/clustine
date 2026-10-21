//! Fleeing and staying afloat (the brief's 7.3.13 and 7.3.14): `AnimalPanic` and `Swim`.

use clustine_blocks::data::has_water;
use clustine_world::BlockPos;

use super::super::sensors::hurt_by;
use super::super::{Behaviour, Cx, HeldTimed, MemoryKind, Status, Timed, Value};
use super::walking::HALF_PI;
use super::{bottom_centre, walk_to_point};
use crate::mobs::ai::goals::{manhattan_ordered, view_vector};
use crate::mobs::ai::{controls, navigation, random_pos};
use crate::physics::{Vec3, World};

/// Where a panic runs to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum PanicTo {
    /// `LandRandomPos.getPos(mob, 5, 4)`.
    Land,
    /// `AirAndWaterRandomPos.getPos(mob, 5, 4, yOffset, view, π/2)` (`AnimalPanic(speed,
    /// yOffset)`).
    AirAndWater { y_offset: i32 },
}

fn nothing(_: &mut Cx<'_, '_, '_>) {}

fn always(_: &mut Cx<'_, '_, '_>) -> bool {
    true
}

fn panic_causes(_: &Cx<'_, '_, '_>) -> &'static str {
    "minecraft:panic_causes"
}

/// `AnimalPanic(speed[, yOffset | causes])` (7.3.13): while `hurt_by` is a damage of the
/// causes' tag, or `is_panicking` is set, 100 to 120 ticks of running to random places
/// (to water within 5 when burning), each chosen when the last is reached. It erases
/// `is_panicking` as it ends and not `hurt_by`, so that it starts again at once while the
/// hurt is remembered.
///
/// A type's own panic (the camel's, the armadillo's, the sniffer's) sets `may_start`,
/// asked after the panic's own conditions, `on_start`, done before the panic's own start,
/// and `causes`.
pub(crate) struct AnimalPanic {
    pub(crate) speed: f32,
    pub(crate) to: PanicTo,
    /// The damage type tag that panics it (`#panic_causes` by default).
    pub(crate) causes: fn(&Cx<'_, '_, '_>) -> &'static str,
    pub(crate) may_start: fn(&mut Cx<'_, '_, '_>) -> bool,
    pub(crate) on_start: fn(&mut Cx<'_, '_, '_>),
}

impl AnimalPanic {
    /// `AnimalPanic(speed)`.
    pub(crate) fn new(speed: f32) -> Self {
        Self {
            speed,
            to: PanicTo::Land,
            causes: panic_causes,
            may_start: always,
            on_start: nothing,
        }
    }

    /// `AnimalPanic(speed, yOffset)`.
    pub(crate) fn flying(speed: f32, y_offset: i32) -> Self {
        Self {
            to: PanicTo::AirAndWater { y_offset },
            ..Self::new(speed)
        }
    }

    /// The panic, boxed.
    pub(crate) fn boxed(self) -> Box<dyn Behaviour> {
        Box::new(HeldTimed::new(self))
    }
}

/// `new AnimalPanic(speed)`, boxed.
pub(crate) fn panic(speed: f32) -> Box<dyn Behaviour> {
    AnimalPanic::new(speed).boxed()
}

/// `AnimalPanic.lookForWater(level, entity)`: none where the mob's block has a collision
/// shape; else the first water by Manhattan distance within 5 across and 1 up or down,
/// for a mob two blocks wide one with water south, east and south-east of it too.
pub(crate) fn look_for_water(cx: &Cx<'_, '_, '_>) -> Option<BlockPos> {
    let at = cx.ai.block_position();
    let world = cx.ai.tick.world;
    if !world.state(at).voxel_collision_shape().is_empty() {
        return None;
    }
    let wide = cx.ai.width().ceil() as i32 == 2;
    let water = |pos: BlockPos| has_water(world.state(pos));
    manhattan_ordered(at, 5, 1, 5).into_iter().find(|&pos| {
        water(pos)
            && (!wide
                || (water(BlockPos::new(pos.x, pos.y, pos.z + 1))
                    && water(BlockPos::new(pos.x + 1, pos.y, pos.z))
                    && water(BlockPos::new(pos.x + 1, pos.y, pos.z + 1))))
    })
}

impl AnimalPanic {
    /// `getPanicPos(mob, level)`.
    fn panic_pos(&self, cx: &mut Cx<'_, '_, '_>) -> Option<Vec3> {
        if cx.ai.tick.living.is_on_fire()
            && let Some(water) = look_for_water(cx)
        {
            return Some(bottom_centre(water));
        }
        match self.to {
            PanicTo::Land => random_pos::land_pos(cx.ai, 5, 4),
            PanicTo::AirAndWater { y_offset } => {
                let view = view_vector(cx.ai.tick.pitch, cx.ai.tick.yaw);
                random_pos::air_and_water_pos(cx.ai, 5, 4, y_offset, view[0], view[2], HALF_PI)
            }
        }
    }
}

impl Timed for AnimalPanic {
    type State = ();

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[
            (MemoryKind::IsPanicking, Status::Registered),
            (MemoryKind::HurtBy, Status::Registered),
        ]
    }

    fn durations(&self) -> (i64, i64) {
        (100, 120)
    }

    fn extra_start(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) -> bool {
        let tag = (self.causes)(cx);
        let hurt = hurt_by(cx.memories).is_some_and(|kind| kind.is(tag));
        (hurt || cx.memories.check(MemoryKind::IsPanicking, Status::Present))
            && (self.may_start)(cx)
    }

    fn can_still_use(&self, _: &mut (), _: &mut Cx<'_, '_, '_>) -> bool {
        true
    }

    fn start(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        (self.on_start)(cx);
        cx.memories.set(MemoryKind::IsPanicking, Value::Bool(true));
        cx.memories.erase(MemoryKind::WalkTarget);
        navigation::stop(cx.ai);
    }

    fn tick(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        if navigation::is_done(cx.ai)
            && let Some(at) = self.panic_pos(cx)
        {
            cx.memories
                .set(MemoryKind::WalkTarget, walk_to_point(at, self.speed, 0));
        }
    }

    fn stop(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        cx.memories.erase(MemoryKind::IsPanicking);
    }
}

/// `Swim(chance)` (7.3.14): while deeper in water than its jump threshold, or in lava,
/// a jump each tick with the chance, of the mob's own draw. It runs 61 ticks and starts
/// again the tick after.
pub(crate) struct Swim {
    pub(crate) chance: f32,
}

/// `new Swim(chance)`, boxed.
pub(crate) fn swim(chance: f32) -> Box<dyn Behaviour> {
    Box::new(HeldTimed::new(Swim { chance }))
}

impl Swim {
    /// `isInFluidDeeperThan(getFluidJumpThreshold(), #water) || isInLava()`.
    fn afloat(cx: &Cx<'_, '_, '_>) -> bool {
        let threshold = if cx.ai.tick.living.eye_height < 0.4 {
            0.0
        } else {
            0.4
        };
        cx.ai.tick.living.fluids.water_height > threshold || cx.ai.in_lava()
    }
}

impl Timed for Swim {
    type State = ();

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[]
    }

    fn extra_start(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) -> bool {
        Self::afloat(cx)
    }

    fn can_still_use(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) -> bool {
        Self::afloat(cx)
    }

    fn tick(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        if cx.ai.tick.draws.next_float() < self.chance {
            controls::jump(cx.ai);
        }
    }
}
