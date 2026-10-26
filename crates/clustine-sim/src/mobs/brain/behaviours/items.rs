//! Going for an item the mob wants (`GoToWantedItem`, the brief's 7.10.3; a villager's
//! core package, `docs/groundwork/n7k-plan.md`, section 10).

use super::super::sensors::{closer_than, find};
use super::super::{Behaviour, Cx, HeldShot, MemoryKind, Shot, Status, Value};
use super::{entity_of, tracking, walk};

/// `GoToWantedItem.create(predicate, speed, overrideWalkTarget, maxDistToWalk)`: the
/// wanted item the mob sees (`nearest_visible_wanted_item`), nearer than `range`, while
/// it waits on no pick-up cooldown and may pick up (`canPickUpLoot`), looked at and walked
/// to. With `override_walk` false it starts only with no walk target. No draws. An item
/// the region's view no longer has is "unknown" and not gone after. **Named**: the world
/// border is never in the way (Clustine's has none).
pub(crate) struct GoToWantedItem {
    /// The predicate on the mob (`e -> true` for a villager's).
    pub(crate) may: fn(&Cx<'_, '_, '_>) -> bool,
    pub(crate) speed: f32,
    pub(crate) override_walk: bool,
    pub(crate) range: i32,
    /// `canPickUpLoot()`.
    pub(crate) can_pick_up: fn(&Cx<'_, '_, '_>) -> bool,
}

/// [`GoToWantedItem`], boxed.
pub(crate) fn go_to_wanted_item(item: GoToWantedItem) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(item))
}

impl Shot for GoToWantedItem {
    fn required(&self) -> &[MemoryKind] {
        &[
            MemoryKind::WalkTarget,
            MemoryKind::LookTarget,
            MemoryKind::NearestVisibleWantedItem,
            MemoryKind::ItemPickupCooldownTicks,
        ]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        let walk_status = if self.override_walk {
            Status::Registered
        } else {
            Status::Absent
        };
        if !cx.memories.check(MemoryKind::WalkTarget, walk_status)
            || !cx
                .memories
                .check(MemoryKind::LookTarget, Status::Registered)
            || !cx
                .memories
                .check(MemoryKind::NearestVisibleWantedItem, Status::Present)
            || !cx
                .memories
                .check(MemoryKind::ItemPickupCooldownTicks, Status::Registered)
        {
            return false;
        }
        let Some(item) =
            entity_of(cx, MemoryKind::NearestVisibleWantedItem).and_then(|key| find(cx.ai, key))
        else {
            return false;
        };
        if cx
            .memories
            .check(MemoryKind::ItemPickupCooldownTicks, Status::Present)
            || !(self.may)(cx)
            || !closer_than(cx.ai, &item, f64::from(self.range))
            || !(self.can_pick_up)(cx)
        {
            return false;
        }
        cx.memories.set(
            MemoryKind::LookTarget,
            Value::Tracker(tracking(item.key, true)),
        );
        cx.memories.set(
            MemoryKind::WalkTarget,
            walk(tracking(item.key, false), self.speed, 0),
        );
        true
    }
}
