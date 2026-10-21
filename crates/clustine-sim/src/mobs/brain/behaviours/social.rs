//! Behaviours towards others (the brief's 7.3.9 to 7.3.11): `AnimalMakeLove`,
//! `BabyFollowAdult` and `FollowTemptation`.

use clustine_world::{Entity, EntityId};

use super::super::sensors::{closer_than, find, find_closest_visible, visible_contains};
use super::super::{
    Behaviour, Cx, HeldShot, HeldTimed, MemoryKind, MemoryWrite, Shot, Status, Timed, Tracker,
    Value,
};
use super::{Speed, entity_of, tracking, walk};
use crate::mobs::Mob;
use crate::mobs::ai::sensing::Looked;
use crate::mobs::tick::Local;
use crate::mobs::types::{self, Mate, animal};
use crate::physics;

fn nothing(_: &mut Cx<'_, '_, '_>) {}

/// The record of the region's entity `key`, as the view has it, and whether it is a
/// copy.
fn record<'w>(cx: &Cx<'_, '_, 'w>, key: EntityId) -> Option<(&'w Entity, bool)> {
    let world = cx.ai.tick.world;
    world.entity(key).map(|seen| (seen.entity, seen.copied))
}

/// `isPanicking()` of another mob, from its record: its brain's `is_panicking` where it
/// has a brain, else whether a panic goal runs. A copy keeps no mind, so a mob across a
/// line is read as not panicking (named, as for the farm animals).
pub(crate) fn panicking(entity: &Entity) -> bool {
    let Some(provider) = types::of(entity.kind).brain() else {
        return crate::mobs::ai::panicking_record(entity);
    };
    let Ok(mob) = Mob::decode(&entity.data) else {
        return false;
    };
    super::super::memories_of(provider, &mob, entity.lived)
        .check(MemoryKind::IsPanicking, Status::Present)
}

/// `this.isInLove()` of the mob: in love, and not waiting on the answer of a request to
/// breed (the region's breeding protocol, `region/breeding.rs`).
fn in_love(cx: &Cx<'_, '_, '_>) -> bool {
    animal::animal_of(&cx.ai.tick.mob.state).in_love_for_others(cx.ai.tick.lived)
}

/// `this.canMate(partner)` of the mob's type, both in love, for the region's animal
/// `partner`.
fn can_mate(cx: &Cx<'_, '_, '_>, partner: &Entity) -> bool {
    if partner.key == cx.ai.tick.key || !in_love(cx) {
        return false;
    }
    let other = animal::animal_of(match &partner.data {
        clustine_world::EntityData::Mob { state, .. } => state,
        _ => return false,
    });
    if !other.in_love_for_others(partner.lived) {
        return false;
    }
    let Some((own, _)) = record(cx, cx.ai.tick.key) else {
        return false;
    };
    let Ok(partner_mob) = Mob::decode(&partner.data) else {
        return false;
    };
    let mut own = own.clone();
    own.lived = cx.ai.tick.lived;
    cx.ai.tick.mob_type.can_mate(
        Mate {
            entity: &own,
            mob: &cx.ai.tick.mob,
        },
        Mate {
            entity: partner,
            mob: &partner_mob,
        },
    )
}

/// What `BehaviorUtils.lockGazeAndWalkToEachOther(a, b, speed, closeEnough)` writes into
/// the one of the two that is `to`'s other: a look at `to`, a walk to it, and the look
/// again.
fn gaze_and_walk(to: EntityId, speed: f32, close_enough: i32) -> [MemoryWrite; 3] {
    let tracker = tracking(to, true);
    [
        MemoryWrite::Set(MemoryKind::LookTarget, Value::Tracker(tracker)),
        MemoryWrite::Set(MemoryKind::WalkTarget, walk(tracker, speed, close_enough)),
        MemoryWrite::Set(MemoryKind::LookTarget, Value::Tracker(tracker)),
    ]
}

/// `AnimalMakeLove(partnerType[, speed, closeEnough])` (7.3.9): an animal in love finds
/// the closest seen partner that may mate and does not panic, and for 110 ticks the two
/// look at and walk to each other; within 3 blocks, once the drawn moment has come
/// (60 to 109 ticks in), it breeds.
///
/// The partner's breed, look and walk targets are written by [`Local::SetMemories`],
/// which the region applies to its own mob (a mob in a copy is not written). The
/// breeding is the region's protocol's (`Local::BreedAsk`, `region/breeding.rs`): only
/// the higher key asks a partner that only a copy shows, and the asker is marked asked.
/// A partner the view no longer has is out of sight (`entityIsVisible` fails), the
/// game's answer (the plan, section 8 item 8).
pub(crate) struct AnimalMakeLove {
    /// The partner's entity type.
    pub(crate) partner: u16,
    pub(crate) speed: f32,
    pub(crate) close_enough: i32,
    /// What a type does before the start (the sniffer's `resetSniffing`).
    pub(crate) on_start: fn(&mut Cx<'_, '_, '_>),
}

impl AnimalMakeLove {
    /// `new AnimalMakeLove(partnerType)`: speed 1, close enough 2.
    pub(crate) fn new(partner: u16) -> Self {
        Self::at(partner, 1.0, 2)
    }

    /// `new AnimalMakeLove(partnerType, speed, closeEnough)`.
    pub(crate) fn at(partner: u16, speed: f32, close_enough: i32) -> Self {
        Self {
            partner,
            speed,
            close_enough,
            on_start: nothing,
        }
    }

    pub(crate) fn boxed(self) -> Box<dyn Behaviour> {
        Box::new(HeldTimed::new(self))
    }

    /// `findValidBreedPartner(animal)`.
    fn partner_of(&self, cx: &mut Cx<'_, '_, '_>) -> Option<Looked> {
        let kind = self.partner;
        find_closest_visible(cx, &mut |cx, looked| {
            looked.kind == Some(kind)
                && record(cx, looked.key)
                    .is_some_and(|(entity, _)| can_mate(cx, entity) && !panicking(entity))
        })
    }

    /// `lockGazeAndWalkToEachOther(animal, partner, speed, closeEnough)`: each looks at
    /// the other and walks to it.
    fn lock_gaze(&self, cx: &mut Cx<'_, '_, '_>, partner: EntityId) {
        let own = cx.ai.tick.key;
        for write in gaze_and_walk(partner, self.speed, self.close_enough) {
            write.apply(cx.memories);
        }
        cx.ai.tick.out.local.push(Local::SetMemories {
            key: partner,
            writes: gaze_and_walk(own, self.speed, self.close_enough).to_vec(),
        });
    }

    /// `spawnChildFromBreeding(level, partner)`, as the region's breeding protocol has
    /// it.
    fn breed(cx: &mut Cx<'_, '_, '_>, partner: EntityId, copied: bool) {
        if copied && cx.ai.tick.key < partner {
            // The lower key does nothing: the partner's region asks.
            return;
        }
        let now = cx.ai.tick.lived;
        let mut number = 0;
        animal::update(&mut cx.ai.tick.mob, |animal| {
            animal.requests = animal.requests.wrapping_add(1);
            number = animal.requests;
            animal.asked = Some(animal::Asked {
                partner,
                number,
                at: now,
            });
        });
        cx.ai
            .tick
            .out
            .local
            .push(Local::BreedAsk { partner, number });
    }
}

impl Timed for AnimalMakeLove {
    /// `spawnChildAtTime`, a game time.
    type State = i64;

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[
            (MemoryKind::VisibleMobs, Status::Present),
            (MemoryKind::BreedTarget, Status::Absent),
            (MemoryKind::WalkTarget, Status::Registered),
            (MemoryKind::LookTarget, Status::Registered),
            (MemoryKind::IsPanicking, Status::Absent),
        ]
    }

    fn durations(&self) -> (i64, i64) {
        (110, 110)
    }

    fn extra_start(&self, _: &mut i64, cx: &mut Cx<'_, '_, '_>) -> bool {
        in_love(cx) && self.partner_of(cx).is_some()
    }

    fn start(&self, spawn_at: &mut i64, cx: &mut Cx<'_, '_, '_>) {
        (self.on_start)(cx);
        let Some(partner) = self.partner_of(cx) else {
            return;
        };
        let own = cx.ai.tick.key;
        cx.memories
            .set(MemoryKind::BreedTarget, Value::Entity(partner.key));
        cx.ai.tick.out.local.push(Local::SetMemories {
            key: partner.key,
            writes: vec![MemoryWrite::Set(
                MemoryKind::BreedTarget,
                Value::Entity(own),
            )],
        });
        self.lock_gaze(cx, partner.key);
        *spawn_at = cx.time + 60 + i64::from(cx.ai.tick.draws.next_int(50));
    }

    fn can_still_use(&self, spawn_at: &mut i64, cx: &mut Cx<'_, '_, '_>) -> bool {
        let Some(partner) = entity_of(cx, MemoryKind::BreedTarget) else {
            return false;
        };
        let Some((entity, _)) = record(cx, partner) else {
            return false;
        };
        if entity.kind != self.partner {
            return false;
        }
        let alive = find(cx.ai, partner).is_some_and(|looked| looked.alive);
        alive
            && can_mate(cx, entity)
            && visible_contains(cx, partner)
            && cx.time <= *spawn_at
            && !cx.memories.check(MemoryKind::IsPanicking, Status::Present)
            && !panicking(entity)
    }

    fn tick(&self, spawn_at: &mut i64, cx: &mut Cx<'_, '_, '_>) {
        let Some(partner) = entity_of(cx, MemoryKind::BreedTarget) else {
            return;
        };
        let Some(looked) = find(cx.ai, partner) else {
            return;
        };
        let copied = looked.copied;
        self.lock_gaze(cx, partner);
        if !closer_than(cx.ai, &looked, 3.0) {
            return;
        }
        if cx.time >= *spawn_at {
            Self::breed(cx, partner, copied);
            cx.memories.erase(MemoryKind::BreedTarget);
            cx.ai.tick.out.local.push(Local::SetMemories {
                key: partner,
                writes: vec![MemoryWrite::Erase(MemoryKind::BreedTarget)],
            });
        }
    }

    fn stop(&self, spawn_at: &mut i64, cx: &mut Cx<'_, '_, '_>) {
        cx.memories.erase(MemoryKind::BreedTarget);
        cx.memories.erase(MemoryKind::WalkTarget);
        cx.memories.erase(MemoryKind::LookTarget);
        *spawn_at = 0;
    }
}

/// `BabyFollowAdult.create(range, speed[, memory, eyes])` (7.3.10): a baby farther from
/// the adult `memory` names than the range's least and within its most and one, looks at
/// it and walks to within the least less one. An adult the view has not is not followed
/// this tick. No draws.
pub(crate) struct BabyFollowAdult {
    pub(crate) range: (i32, i32),
    pub(crate) speed: Speed,
    required: [MemoryKind; 3],
    pub(crate) eyes: bool,
}

/// `BabyFollowAdult.create(range, speed)`: the nearest visible adult, at the feet.
pub(crate) fn follow_adult(range: (i32, i32), speed: f32) -> Box<dyn Behaviour> {
    follow_adult_by(
        range,
        Speed::Fixed(speed),
        MemoryKind::NearestVisibleAdult,
        false,
    )
}

/// `BabyFollowAdult.create(range, speedFn, memory, eyes)`.
pub(crate) fn follow_adult_by(
    range: (i32, i32),
    speed: Speed,
    memory: MemoryKind,
    eyes: bool,
) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(BabyFollowAdult {
        range,
        speed,
        required: [memory, MemoryKind::LookTarget, MemoryKind::WalkTarget],
        eyes,
    }))
}

impl Shot for BabyFollowAdult {
    fn required(&self) -> &[MemoryKind] {
        &self.required
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        let memory = self.required[0];
        if !cx.memories.check(memory, Status::Present)
            || !cx
                .memories
                .check(MemoryKind::LookTarget, Status::Registered)
            || !cx.memories.check(MemoryKind::WalkTarget, Status::Absent)
        {
            return false;
        }
        if !cx.ai.tick.mob_type.is_baby(&cx.ai.tick.mob) {
            return false;
        }
        let Some(adult) = entity_of(cx, memory).and_then(|key| find(cx.ai, key)) else {
            return false;
        };
        let (least, most) = self.range;
        if !closer_than(cx.ai, &adult, f64::from(most + 1))
            || closer_than(cx.ai, &adult, f64::from(least))
        {
            return false;
        }
        let to = Tracker::Entity {
            key: adult.key,
            eyes: self.eyes,
            eyes_block: self.eyes,
        };
        let speed = self.speed.of(cx);
        let looked = Tracker::Entity {
            key: adult.key,
            eyes: true,
            eyes_block: self.eyes,
        };
        cx.memories
            .set(MemoryKind::LookTarget, Value::Tracker(looked));
        cx.memories
            .set(MemoryKind::WalkTarget, walk(to, speed, least - 1));
        true
    }
}

fn close_enough_default(_: &Cx<'_, '_, '_>) -> f64 {
    2.5
}

/// `FollowTemptation(speed[, closeEnough[, lookInTheEyes]])` (7.3.11): while a player
/// tempts it, with no breed target and no panic, a look at them and a walk to within
/// 2, erased while within `close_enough`; as it ends, a cooldown of 100 and the walk and
/// look forgotten. It never times out. A tempting player the view has not is not
/// followed this tick.
pub(crate) struct FollowTemptation {
    pub(crate) speed: Speed,
    pub(crate) close_enough: fn(&Cx<'_, '_, '_>) -> f64,
    pub(crate) look_in_the_eyes: bool,
    /// What a type does as it starts (the sniffer's `resetSniffing`).
    pub(crate) on_start: fn(&mut Cx<'_, '_, '_>),
}

/// `FollowTemptation.TEMPTATION_COOLDOWN`.
pub(crate) const TEMPTATION_COOLDOWN: i32 = 100;

impl FollowTemptation {
    /// `new FollowTemptation(speed)`.
    pub(crate) fn new(speed: Speed) -> Self {
        Self {
            speed,
            close_enough: close_enough_default,
            look_in_the_eyes: false,
            on_start: nothing,
        }
    }

    pub(crate) fn boxed(self) -> Box<dyn Behaviour> {
        Box::new(HeldTimed::new(self))
    }
}

impl Timed for FollowTemptation {
    type State = ();

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[
            (MemoryKind::LookTarget, Status::Registered),
            (MemoryKind::WalkTarget, Status::Registered),
            (MemoryKind::TemptationCooldownTicks, Status::Absent),
            (MemoryKind::TemptingPlayer, Status::Present),
            (MemoryKind::BreedTarget, Status::Absent),
            (MemoryKind::IsPanicking, Status::Absent),
        ]
    }

    fn timed_out(&self, _: &(), _: &Cx<'_, '_, '_>, _: i64) -> bool {
        false
    }

    fn can_still_use(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) -> bool {
        cx.memories
            .check(MemoryKind::TemptingPlayer, Status::Present)
            && !cx.memories.check(MemoryKind::BreedTarget, Status::Present)
            && !cx.memories.check(MemoryKind::IsPanicking, Status::Present)
    }

    fn start(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        (self.on_start)(cx);
    }

    fn tick(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        let Some(player) =
            entity_of(cx, MemoryKind::TemptingPlayer).and_then(|key| find(cx.ai, key))
        else {
            return;
        };
        cx.memories.set(
            MemoryKind::LookTarget,
            Value::Tracker(tracking(player.key, true)),
        );
        let close_enough = (self.close_enough)(cx);
        if physics::distance_sqr(cx.ai.position(), player.position) < close_enough * close_enough {
            cx.memories.erase(MemoryKind::WalkTarget);
        } else {
            let to = Tracker::Entity {
                key: player.key,
                eyes: self.look_in_the_eyes,
                eyes_block: self.look_in_the_eyes,
            };
            let speed = self.speed.of(cx);
            cx.memories.set(MemoryKind::WalkTarget, walk(to, speed, 2));
        }
    }

    fn stop(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        cx.memories.set(
            MemoryKind::TemptationCooldownTicks,
            Value::Int(TEMPTATION_COOLDOWN),
        );
        cx.memories.erase(MemoryKind::WalkTarget);
        cx.memories.erase(MemoryKind::LookTarget);
    }
}
