//! Walking away from what a memory names (`SetWalkTargetAwayFrom`; a villager's panic,
//! `docs/groundwork/n7k-plan.md`, section 10).

use super::super::sensors::find;
use super::super::{Behaviour, Cx, HeldShot, MemoryKind, Shot, Status};
use super::{entity_of, position_of, walk_target, walk_to_point};
use crate::mobs::ai::random_pos;
use crate::physics;

/// `SetWalkTargetAwayFrom.entity(memory, speed, desiredDistance, overrideWalkTarget)`:
/// with the entity `memory` names nearer than `distance`, a walk at `speed` to a place
/// of `LandRandomPos.getPosAway(mob, 16, 7, its position)`, the first of ten tries that
/// finds one. It fires once its conditions hold, a place found or not. With an existing
/// walk target it starts only where `override_walk` is, and then not where that walk is
/// at the same speed and already away from the entity. An entity the region's view no
/// longer has is "unknown": nothing is done.
pub(crate) struct SetWalkTargetAwayFrom {
    /// The walk target, and the memory of what it walks away from.
    pub(crate) memories: [MemoryKind; 2],
    pub(crate) speed: f32,
    pub(crate) distance: i32,
    pub(crate) override_walk: bool,
}

/// [`SetWalkTargetAwayFrom`] of the entity `memory` names, boxed.
pub(crate) fn away_from(
    memory: MemoryKind,
    speed: f32,
    distance: i32,
    override_walk: bool,
) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(SetWalkTargetAwayFrom {
        memories: [MemoryKind::WalkTarget, memory],
        speed,
        distance,
        override_walk,
    }))
}

/// The ten tries of `LandRandomPos.getPosAway` the game's loop makes.
const TRIES: usize = 10;

impl Shot for SetWalkTargetAwayFrom {
    fn required(&self) -> &[MemoryKind] {
        &self.memories
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        let memory = self.memories[1];
        if !cx
            .memories
            .check(MemoryKind::WalkTarget, Status::Registered)
            || !cx.memories.check(memory, Status::Present)
        {
            return false;
        }
        let existing = walk_target(cx);
        if existing.is_some() && !self.override_walk {
            return false;
        }
        let Some(target) = entity_of(cx, memory)
            .and_then(|key| find(cx.ai, key))
            .map(|looked| looked.position)
        else {
            return false;
        };
        let position = cx.ai.position();
        let reach = f64::from(self.distance);
        if physics::distance_sqr(position, target) >= reach * reach {
            return false;
        }
        if let Some(walk) = existing
            && walk.speed.to_bits() == self.speed.to_bits()
            && let Some(to) = position_of(cx, &walk.to)
        {
            let along = physics::sub(to, position);
            let towards = physics::sub(target, position);
            let dot = along[0] * towards[0] + along[1] * towards[1] + along[2] * towards[2];
            if dot < 0.0 {
                return false;
            }
        }
        for _ in 0..TRIES {
            if let Some(at) = random_pos::land_pos_away(cx.ai, 16, 7, target) {
                cx.memories
                    .set(MemoryKind::WalkTarget, walk_to_point(at, self.speed, 0));
                break;
            }
        }
        true
    }
}
