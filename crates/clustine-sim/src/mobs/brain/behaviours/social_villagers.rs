//! The villager's social behaviours (`docs/groundwork/n10-plan.md`, section 7;
//! `n10-brief.md`, section 4, as `n10-dumps.md`, part B, corrects it): gossip and
//! reputation, trade between villagers, interactions, love, golems summoned, bells,
//! hiding, the hero's gifts and celebrating; and the villager's hooks that are part of
//! them (its trade's reputation event after its brain, its death's witnesses and points
//! of interest, a hurt's reputation event, a partner's birth).
//!
//! Every draw is the villager's stream's at the game's place (A28 section 4): the
//! level's on its behalf, its own, a thrown item's construction (the region's, as for
//! every item thrown).
//!
//! **Across a line**: what a villager reads of another is that one's record, which a
//! copy carries (its type state: gossips, food, `Shown`); what it writes into another
//! is `Local::Villager`, applied to the region's own at once and sent to the holder of
//! one only a copy shows (decision 4). What other villagers' brains hold is read from
//! their `Shown` (decision 3): when they last slept, whether a golem was seen, whether
//! they sleep.
//!
//! Named differences: offers a villager has not drawn yet are not drawn to be shown to a
//! player (`getOffers()` draws them: part T2's), so an unasked villager shows none; the
//! prices are not worked out again after a reputation event or a gossip about the
//! trading player (`updateSpecialPrices`, T2's); a partner across a line or in a copy is
//! not shown its entity events, and a child its event 12 at its birth (the region makes
//! it after the tick); a partner of a love without a bed does not digest its food (only
//! a birth writes into it, `VillagerUse::Born`); raids do not exist yet (N10d), so
//! `ReactToBell` always hides and celebrating never starts, and its rocket is not made.

#![allow(dead_code, reason = "part V1 names these in the villager's packages")]

use clustine_data::villagers::professions;
use clustine_data::{attributes, entity_types, items, sounds};
use clustine_world::poi::PoiType;
use clustine_world::{BlockPos, Entity, EntityId, Float, ItemStack, VillagerUse};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::super::sensors::{find, find_closest_visible, targetable, visible_contains};
use super::super::{
    Activity, Behaviour, Cx, HeldShot, HeldTimed, MemoryKind, Shot, Status, Timed, Tracker, Value,
};
use super::{HeldKept, KeptShot, entity_of, tracking, walk};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::ai::navigation;
use crate::mobs::ai::sensing::Looked;
use crate::mobs::body::Event;
use crate::mobs::body::hurt::{DamageSource, Struck};
use crate::mobs::tick::{Local, MobTick};
use crate::mobs::types::gossip::{Gossips, ReputationEvent};
use crate::mobs::types::villager::{self, VillagerState};
use crate::mobs::types::{self, Breeding, animal};
use crate::mobs::view::MobWorld;
use crate::physics::item::max_stack_size;
use crate::physics::{self, Vec3, World};
use crate::region::Random;
use crate::region::pois::Occupancy;
use crate::region::spawn_util::{self, Making, Strategy};

/// The villager's entity type.
const VILLAGER: u16 = entity_types::VILLAGER as u16;

/// How long gossip with one villager waits before the next (`Villager.gossip`).
const GOSSIP_COOLDOWN: i64 = 1200;
/// How long gossip lasts between its decays (`maybeDecayGossip`).
const GOSSIP_DECAY_INTERVAL: i64 = 24_000;
/// How long a summoned golem is remembered (`GolemSensor.golemDetected`).
const GOLEM_DETECTED_FOR: i64 = 599;
/// How long ago a villager may have slept to want a golem
/// (`golemSpawnConditionsMet`).
const SLEPT_FOR_GOLEM: i64 = 24_000;
/// `villager_food` points below which a villager is hungry, wants more food, may breed.
const HUNGRY: i32 = 12;
/// What a villager with excess food has (`hasExcessFood`).
const EXCESS_FOOD: i32 = 24;
/// `BehaviorUtils.throwItem`'s speed of a throw, a float widened.
const THROW_SPEED: f32 = 0.3;
/// `ItemEntity.setDefaultPickUpDelay()`.
const DEFAULT_PICK_UP_DELAY: u16 = 10;

// ------------------------------------------------------------------------------------
// What a villager reads of itself and of other villagers.

/// The villager whose record `entity` is, as its type state has it: none for another
/// type.
pub(crate) fn villager_of(entity: &Entity) -> Option<(animal::Animal, VillagerState)> {
    if entity.kind != VILLAGER {
        return None;
    }
    let mob = Mob::decode(&entity.data).ok()?;
    Some(villager::state_of(&mob))
}

/// The record of the entity `key` as the mob's view has it.
fn record<'w>(cx: &Cx<'_, '_, 'w>, key: EntityId) -> Option<&'w Entity> {
    let world: &'w dyn MobWorld = cx.ai.tick.world;
    world.entity(key).map(|seen| seen.entity)
}

/// `countFoodPointsInInventory()`: each stack of villager food by its nutrition.
pub(crate) fn food_points(own: &VillagerState) -> i32 {
    own.inventory
        .iter()
        .flatten()
        .filter_map(|stack| {
            villager::villager_food(stack.item).map(|nutrition| stack.count * nutrition)
        })
        .sum()
}

/// `Villager.canBreed()`: food and food points of at least 12, not asleep, of age 0.
pub(crate) fn can_breed(shared: &animal::Animal, own: &VillagerState, lived: u32) -> bool {
    i32::from(own.food_level) + food_points(own) >= HUNGRY
        && !own.shown.sleeping
        && shared.age(lived) == 0
}

/// `hasExcessFood()`.
fn has_excess_food(own: &VillagerState) -> bool {
    food_points(own) >= EXCESS_FOOD
}

/// `wantsMoreFood()`.
fn wants_more_food(own: &VillagerState) -> bool {
    food_points(own) < HUNGRY
}

/// `eatAndDigestFood()`: `eatUntilFull` (while hungry, each stack of villager food in
/// order eaten one by one until it is not, what was eaten taken from the stack), then
/// 12 of its food digested. The level is kept as the byte the game saves it as.
pub(crate) fn eat_and_digest(own: &mut VillagerState) {
    let mut level = i32::from(own.food_level);
    if level < HUNGRY {
        'slots: for slot in &mut own.inventory {
            let Some(stack) = slot.as_mut() else {
                continue;
            };
            let Some(nutrition) = villager::villager_food(stack.item) else {
                continue;
            };
            let mut eaten = 0;
            let mut left = stack.count;
            while left > 0 {
                level += nutrition;
                eaten += 1;
                if level >= HUNGRY {
                    remove_item(slot, eaten);
                    break 'slots;
                }
                left -= 1;
            }
            remove_item(slot, eaten);
        }
    }
    own.food_level = (level - HUNGRY) as i8;
}

/// `SimpleContainer.removeItem(slot, count)`: `count` taken from the slot's stack, the
/// slot empty where nothing is left.
fn remove_item(slot: &mut Option<ItemStack>, count: i32) {
    if let Some(stack) = slot.as_mut() {
        stack.count -= count.min(stack.count);
        if stack.count <= 0 {
            *slot = None;
        }
    }
}

/// `VillagerProfession.requestedItems()`: the farmer's wheat, seeds and bone meal, none
/// for any other.
fn requested_items(profession: u8) -> &'static [i32] {
    if i32::from(profession) == professions::FARMER {
        &[
            items::WHEAT,
            items::WHEAT_SEEDS,
            items::BEETROOT_SEEDS,
            items::BONE_MEAL,
        ]
    } else {
        &[]
    }
}

/// Whether the thing `looked` names is of the entity type `kind` (a player for the
/// player's type).
fn is_kind(looked: &Looked, kind: u16) -> bool {
    if i32::from(kind) == entity_types::PLAYER {
        looked.is_player()
    } else {
        looked.kind == Some(kind)
    }
}

/// `BehaviorUtils.targetIsValid(brain, memory, VILLAGER)`: the memory names a villager
/// that lives and is seen.
fn target_is_valid(cx: &mut Cx<'_, '_, '_>, memory: MemoryKind) -> bool {
    let Some(key) = entity_of(cx, memory) else {
        return false;
    };
    let Some(looked) = find(cx.ai, key) else {
        return false;
    };
    looked.kind == Some(VILLAGER) && looked.alive && visible_contains(cx, key)
}

/// `BehaviorUtils.lockGazeAndWalkToEachOther(villager, partner, speed, closeEnough)`:
/// each looks at the other and walks to it; the partner's half is a write into it.
fn lock_gaze(cx: &mut Cx<'_, '_, '_>, partner: EntityId, speed: f32, close_enough: i32) {
    let tracker = tracking(partner, true);
    cx.memories
        .set(MemoryKind::LookTarget, Value::Tracker(tracker));
    cx.memories
        .set(MemoryKind::WalkTarget, walk(tracker, speed, close_enough));
    let own = cx.ai.tick.key;
    cx.ai.tick.out.local.push(Local::Villager {
        key: partner,
        used: VillagerUse::Gaze {
            to: own,
            speed: Float(speed),
            close: close_enough,
        },
    });
}

/// `BehaviorUtils.throwItem(villager, stack, target)`: from a little below its eyes
/// towards `target` at 0.3, picked up after the default delay (the item's own
/// construction draws are the region's).
fn throw_item(cx: &mut Cx<'_, '_, '_>, stack: ItemStack, target: Vec3) {
    let position = cx.ai.position();
    let at = [
        position[0],
        cx.ai.eye_y() - f64::from(THROW_SPEED),
        position[2],
    ];
    let way = physics::normalize(physics::sub(target, position));
    let speed = f64::from(THROW_SPEED);
    cx.ai.tick.out.local.push(Local::Throw {
        stack,
        at,
        pick_up_delay: DEFAULT_PICK_UP_DELAY,
        motion: [way[0] * speed, way[1] * speed, way[2] * speed],
    });
}

/// `TradeWithVillager.throwHalfStack(villager, predicate, target)`: the first stack of
/// its inventory the predicate takes of more than half its most (half of it thrown) or
/// of more than 24 (the excess thrown), towards `target`.
fn throw_half_stack(cx: &mut Cx<'_, '_, '_>, take: impl Fn(&ItemStack) -> bool, target: Vec3) {
    let mut thrown = None;
    villager::update(&mut cx.ai.tick.mob, |own| {
        for stack in own.inventory.iter_mut().flatten() {
            if stack.count <= 0 || !take(stack) {
                continue;
            }
            let amount = if stack.count > max_stack_size(stack) / 2 {
                stack.count / 2
            } else if stack.count > 24 {
                stack.count - 24
            } else {
                continue;
            };
            stack.count -= amount;
            thrown = Some(stack.with_count(amount));
            break;
        }
        for slot in &mut own.inventory {
            if slot.as_ref().is_some_and(|stack| stack.count <= 0) {
                *slot = None;
            }
        }
    });
    if let Some(stack) = thrown {
        throw_item(cx, stack, target);
    }
}

// ------------------------------------------------------------------------------------
// Gossip and reputation.

/// `Villager.gossip(level, partner, time)`: unless either was stamped within the last
/// 1200 ticks, the villager takes gossip from the partner's by ten draws of its own
/// stream, both are stamped, and it asks for a golem (of five).
fn gossip(cx: &mut Cx<'_, '_, '_>, partner: &Entity, time: i64) {
    let Some((_, theirs)) = villager_of(partner) else {
        return;
    };
    let (_, own) = villager::state_of(&cx.ai.tick.mob);
    // Not saved by the game: nothing since a load is a time of 0.
    let cool = |last: Option<i64>| {
        let last = last.unwrap_or(0);
        !(time >= last && time < last + GOSSIP_COOLDOWN)
    };
    if !cool(own.last_gossip_time) || !cool(theirs.last_gossip_time) {
        return;
    }
    let draws = &mut *cx.ai.tick.draws;
    let mut picked = 0;
    villager::update(&mut cx.ai.tick.mob, |own| {
        picked = own.gossips.transfer_from(&theirs.gossips, draws, 10);
        own.last_gossip_time = Some(time);
    });
    cx.ai.tick.out.local.push(Local::Villager {
        key: partner.key,
        used: VillagerUse::Gossiped {
            at: u64::try_from(time).unwrap_or(0),
        },
    });
    spawn_golem_if_needed(cx, time, 5);
    // A gossip that passed something on works out the trading player's prices again
    // (`updateSpecialPrices`).
    if picked > 0
        && villager::state_of(&cx.ai.tick.mob)
            .1
            .trading_player
            .is_some()
    {
        cx.ai.tick.out.local.push(Local::PricesAgain);
    }
}

/// `Villager.onReputationEventFrom(event, entity)` of the villager `own` about the
/// entity of UUID `about`: its gossips, and whether `about` is the player it trades with,
/// whose prices the caller has worked out again (`updateSpecialPrices`,
/// [`Local::PricesAgain`]).
pub(crate) fn reputation_event(
    own: &mut VillagerState,
    event: ReputationEvent,
    about: Uuid,
) -> bool {
    own.gossips.reputation_event(event, about);
    own.trading_player.is_some_and(|player| player.0 == about)
}

/// `maybeDecayGossip()`, at the end of the villager's tick: its first stamps the time;
/// then every 24000 game ticks its gossip decays a day's worth.
pub(crate) fn maybe_decay_gossip(tick: &mut MobTick<'_>) {
    let time = tick.world.game_time();
    villager::update(&mut tick.mob, |own| {
        if own.last_gossip_decay == 0 {
            own.last_gossip_decay = time;
        } else if time >= own.last_gossip_decay + GOSSIP_DECAY_INTERVAL {
            own.gossips.decay();
            own.last_gossip_decay = time;
        }
    });
}

/// The gossips of the region's or a copy's villager `entity`, if it is one.
pub(crate) fn gossips_of(entity: &Entity) -> Option<Gossips> {
    villager_of(entity).map(|(_, own)| own.gossips)
}

// ------------------------------------------------------------------------------------
// Golems.

/// `wantsToSpawnGolem(time)` of another villager, from what it shows (decision 3): it
/// slept within the last 24000 game ticks and saw no golem lately.
fn shows_wanting_a_golem(own: &VillagerState, time: i64) -> bool {
    own.shown
        .last_slept
        .is_some_and(|slept| time - slept < SLEPT_FOR_GOLEM)
        && !own.shown.golem_detected
}

/// `wantsToSpawnGolem(time)` of the villager itself, from its brain.
fn wants_a_golem(cx: &Cx<'_, '_, '_>, time: i64) -> bool {
    let slept = matches!(
        cx.memories.get(MemoryKind::LastSlept),
        Some(Value::Long(slept)) if time - slept < SLEPT_FOR_GOLEM
    );
    slept
        && !cx
            .memories
            .check(MemoryKind::GolemDetectedRecently, Status::Present)
}

/// `Villager.spawnGolemIfNeeded(level, time, minVillagerAmount)` (`n10-dumps.md`, part B,
/// correction 2): a villager that wants a golem, with at least `least` of the villagers
/// in its box grown by 10 wanting one (itself among them, at most five counted), tries
/// to summon one within 8 (`SpawnUtil.trySpawnMob(IRON_GOLEM, MOB_SUMMONED, …, 10, 8, 6,
/// LEGACY_IRON_GOLEM)`); a golem made, every villager in the box remembers it for 599
/// ticks. From gossip (5) and from the panic every hundredth game tick (3, part V1's
/// `VillagerPanicTrigger` calls it).
pub(crate) fn spawn_golem_if_needed(cx: &mut Cx<'_, '_, '_>, time: i64, least: usize) {
    if !wants_a_golem(cx, time) {
        return;
    }
    let own = cx.ai.tick.key;
    let bounds = cx.ai.tick.living.body.bounds().inflate([10.0; 3]);
    let world = cx.ai.tick.world;
    // `getEntitiesOfClass(Villager, box)`, itself among them, in the view's order.
    let nearby: Vec<(EntityId, bool)> = world
        .entities_in(&bounds)
        .iter()
        .filter(|seen| seen.entity.kind == VILLAGER)
        .map(|seen| {
            let wants = if seen.entity.key == own {
                true
            } else {
                villager_of(seen.entity)
                    .is_some_and(|(_, theirs)| shows_wanting_a_golem(&theirs, time))
            };
            (seen.entity.key, wants)
        })
        .collect();
    let mut agree = nearby.iter().filter(|(_, wants)| *wants).count().min(5);
    // The view has itself only as its record of the tick's start; a villager is in its
    // own box whatever its record says.
    if !nearby.iter().any(|(key, _)| *key == own) {
        agree = (agree + 1).min(5);
    }
    if agree < least {
        return;
    }
    let day = world.day();
    let at = cx.ai.block_position();
    let mut making = Making {
        world,
        draws: &mut *cx.ai.tick.draws,
        day,
        game_time: time,
    };
    let Some((golem, at)) = spawn_util::try_spawn_mob(
        &mut making,
        entity_types::IRON_GOLEM as u16,
        types::SpawnReason::MobSummoned,
        at,
        10,
        8,
        6,
        Strategy::LegacyIronGolem,
        false,
    ) else {
        return;
    };
    cx.ai.tick.out.local.push(Local::MakeMob {
        kind: entity_types::IRON_GOLEM as u16,
        mob: Box::new(golem),
        at,
    });
    cx.memories.set_with_expiry(
        MemoryKind::GolemDetectedRecently,
        Value::Bool(true),
        GOLEM_DETECTED_FOR,
    );
    for (key, _) in nearby {
        if key != own {
            cx.ai.tick.out.local.push(Local::Villager {
                key,
                used: VillagerUse::GolemDetected,
            });
        }
    }
}

// ------------------------------------------------------------------------------------
// The hooks.

/// `VillagerMakeLove`'s write into the partner at a birth, where the partner is held by
/// another region (`VillagerUse::Born`): it eats and digests its food, and its age
/// becomes 6000. `lived` is its record's.
pub(crate) fn born_as_partner(mob: &mut Mob, lived: u32) {
    villager::update(mob, eat_and_digest);
    animal::update(mob, |shared| shared.set_age(6000, lived));
}

/// After the villager's brain in `customServerAiStep`: a trade's `TRADE` reputation event
/// about the last player who traded, entity event 14, and the last trader forgotten.
pub(crate) fn after_brain(tick: &mut MobTick<'_>) {
    let mut traded = false;
    let mut prices = false;
    villager::update(&mut tick.mob, |own| {
        if let Some(player) = own.last_traded_player.take() {
            // The prices for the trading player are worked out again where it is they
            // (`updateSpecialPrices`), which sends them the offers as the trade left them.
            prices = reputation_event(own, ReputationEvent::Trade, player.0);
            traded = true;
        }
    });
    if prices {
        tick.out.local.push(Local::PricesAgain);
    }
    if traded {
        tick.out.events.push(Event::Entity(14));
    }
}

/// `Villager.die`: its witnesses (the villagers it sees) gossip `major_negative` about
/// what caused its death, and its points of interest are given back
/// (`tellWitnessesThatIWasMurdered`, `releaseAllPois`), by the region, which keeps what
/// it sees and reads the points ([`witnesses`], [`points_to_release`]).
pub(crate) fn died(struck: &mut Struck<'_>, source: &DamageSource) {
    let murderer = source.cause.as_ref().map(|cause| cause.key);
    struck.out.local.push(Local::VillagerDied { murderer });
}

/// `setLastHurtByMob(attacker)`'s `VILLAGER_HURT` reputation event about a living
/// attacker: written now about a player, whose UUID the source names; about a mob by the
/// region, which finds its UUID.
pub(crate) fn hurt(struck: &mut Struck<'_>, source: &DamageSource, took: bool) {
    let Some(cause) = source.cause.as_ref().filter(|cause| cause.living) else {
        return;
    };
    if !took || source.kind.is("minecraft:no_anger") {
        return;
    }
    match cause.player {
        Some(player) => {
            let mut prices = false;
            villager::update(struck.mob, |own| {
                prices =
                    reputation_event(own, ReputationEvent::VillagerHurt, Uuid::from_u128(player));
            });
            if prices {
                struck.out.local.push(Local::PricesAgain);
            }
        }
        None => struck.out.local.push(Local::Reputation {
            event: ReputationEvent::VillagerHurt,
            about: cause.key,
        }),
    }
}

/// The villagers that see the dying villager `dying` (`tellWitnessesThatIWasMurdered`):
/// those of its `visible_mobs` as it last sensed them that are villagers, alive, within
/// its follow range and in its line of sight now (`Sensor.isEntityTargetable`).
pub(crate) fn witnesses(
    world: &dyn MobWorld,
    dying: &Entity,
    visible: &[EntityId],
) -> Vec<EntityId> {
    let Ok(mob) = Mob::decode(&dying.data) else {
        return Vec::new();
    };
    let range = mob.body.attributes.value_worn(
        dying.kind,
        crate::mobs::body::attribute(attributes::FOLLOW_RANGE),
        &mob.body.equipment,
    );
    let from = world
        .entity(dying.key)
        .map(|seen| Looked::of_seen(&seen))
        .map_or(dying.position[1], |looked| looked.eye_y);
    let eyes = [dying.position[0], from, dying.position[2]];
    visible
        .iter()
        .filter_map(|key| world.entity(*key))
        .map(|seen| Looked::of_seen(&seen))
        .filter(|looked| {
            looked.kind == Some(VILLAGER)
                && looked.key != dying.key
                && looked.seen_by_anyone()
                && looked.distance_sqr(dying.position) <= range.max(2.0) * range.max(2.0)
                && line_of_sight(world, dying, eyes, looked)
        })
        .map(|looked| looked.key)
        .collect()
}

/// `LivingEntity.hasLineOfSight(target)` of `from`'s eyes at `eyes`: within 128 blocks,
/// with no block's collision shape in the way.
fn line_of_sight(world: &dyn MobWorld, from: &Entity, eyes: Vec3, target: &Looked) -> bool {
    use crate::physics::collide::{Context, collision_shape};
    let to = [target.position[0], target.eye_y, target.position[2]];
    if physics::distance_sqr(eyes, to).sqrt() > 128.0 {
        return false;
    }
    let context = Context {
        bottom: from.position[1],
        fall_distance: from.fall_distance,
        falling_block: false,
        noclip: None,
    };
    !physics::walk::traverse(eyes, to, |pos| {
        collision_shape(world.state(pos), pos, context).is_some_and(|(shape, _)| {
            shape
                .boxes()
                .iter()
                .any(|bounds| physics::body::clips_shape(bounds, eyes, to))
        })
    })
}

/// `releaseAllPois()` of the dying villager `dying`: each of its home, job site,
/// potential job site and meeting point whose point is still there and of the type the
/// memory holds (`Villager.POI_MEMORIES`), with that type.
pub(crate) fn points_to_release(world: &dyn MobWorld, dying: &Entity) -> Vec<(BlockPos, PoiType)> {
    let Some(provider) = types::of(dying.kind).brain() else {
        return Vec::new();
    };
    let Ok(mob) = Mob::decode(&dying.data) else {
        return Vec::new();
    };
    let profession = villager::state_of(&mob).1.data.profession;
    let memories = super::super::memories_of(provider, &mob, dying.lived);
    let held = PoiType::held_by(i32::from(profession));
    let tests: [(MemoryKind, &dyn Fn(PoiType) -> bool); 4] = [
        (MemoryKind::Home, &|kind| kind == PoiType::Home),
        (MemoryKind::JobSite, &|kind| Some(kind) == held),
        (MemoryKind::PotentialJobSite, &|kind| kind.is_job_site()),
        (MemoryKind::MeetingPoint, &|kind| kind == PoiType::Meeting),
    ];
    let mut released = Vec::new();
    for (memory, test) in tests {
        let Some(Value::Block(pos)) = memories.get(memory) else {
            continue;
        };
        if let Some(kind) = world.poi_type_at(pos).filter(|kind| test(*kind)) {
            released.push((pos, kind));
        }
    }
    released
}

// ------------------------------------------------------------------------------------
// `TradeWithVillager`.

/// `TradeWithVillager` (a `Behavior` of 60 ticks, meet 3 and idle 3 in a gate erasing
/// `interaction_target`): with a villager it sees as its interaction target, both lock
/// their gaze and walk to each other; within √5 they lock again, it takes the partner's
/// gossip, and it throws half a stack of food (where it has excess and is a farmer or
/// the partner wants more), half its wheat above 32 (a farmer), and half a stack of what
/// the partner's profession asks for and its own does not.
struct TradeWithVillager;

/// What it keeps: the items it is willing to trade (`trades`), by id.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Trades {
    willing: Vec<i32>,
}

/// `TradeWithVillager` (meet 3, idle 3, in a gate erasing `interaction_target`).
pub(crate) fn trade_with_villager() -> Box<dyn Behaviour> {
    Box::new(HeldTimed::new(TradeWithVillager))
}

impl Timed for TradeWithVillager {
    type State = Trades;

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[
            (MemoryKind::InteractionTarget, Status::Present),
            (MemoryKind::VisibleMobs, Status::Present),
        ]
    }

    fn extra_start(&self, _: &mut Trades, cx: &mut Cx<'_, '_, '_>) -> bool {
        target_is_valid(cx, MemoryKind::InteractionTarget)
    }

    fn can_still_use(&self, _: &mut Trades, cx: &mut Cx<'_, '_, '_>) -> bool {
        target_is_valid(cx, MemoryKind::InteractionTarget)
    }

    fn start(&self, trades: &mut Trades, cx: &mut Cx<'_, '_, '_>) {
        let Some(partner) = entity_of(cx, MemoryKind::InteractionTarget) else {
            return;
        };
        lock_gaze(cx, partner, 0.5, 2);
        // `figureOutWhatIAmWillingToTrade`: what the partner's profession asks for and
        // its own does not.
        let own = villager::state_of(&cx.ai.tick.mob).1.data.profession;
        let theirs = record(cx, partner)
            .and_then(villager_of)
            .map_or(professions::NONE as u8, |(_, them)| them.data.profession);
        trades.willing = requested_items(theirs)
            .iter()
            .copied()
            .filter(|item| !requested_items(own).contains(item))
            .collect();
    }

    fn tick(&self, trades: &mut Trades, cx: &mut Cx<'_, '_, '_>) {
        let Some(key) = entity_of(cx, MemoryKind::InteractionTarget) else {
            return;
        };
        let Some(partner) = record(cx, key) else {
            return;
        };
        let target = partner.position;
        if physics::distance_sqr(cx.ai.position(), target) > 5.0 {
            return;
        }
        lock_gaze(cx, key, 0.5, 2);
        let time = cx.time;
        gossip(cx, partner, time);
        let (_, own) = villager::state_of(&cx.ai.tick.mob);
        let farmer = i32::from(own.data.profession) == professions::FARMER;
        let partner_wants = villager_of(partner).is_some_and(|(_, them)| wants_more_food(&them));
        if has_excess_food(&own) && (farmer || partner_wants) {
            throw_half_stack(
                cx,
                |stack| villager::villager_food(stack.item).is_some(),
                target,
            );
        }
        if farmer {
            let (_, own) = villager::state_of(&cx.ai.tick.mob);
            let wheat: i32 = own
                .inventory
                .iter()
                .flatten()
                .filter(|stack| stack.item == items::WHEAT)
                .map(|stack| stack.count)
                .sum();
            if wheat > max_stack_size(&ItemStack::new(items::WHEAT, 1)) / 2 {
                throw_half_stack(cx, |stack| stack.item == items::WHEAT, target);
            }
        }
        if !trades.willing.is_empty() {
            let (_, own) = villager::state_of(&cx.ai.tick.mob);
            let has_any = own
                .inventory
                .iter()
                .flatten()
                .any(|stack| stack.count > 0 && trades.willing.contains(&stack.item));
            if has_any {
                let willing = trades.willing.clone();
                throw_half_stack(cx, move |stack| willing.contains(&stack.item), target);
            }
        }
    }

    fn stop(&self, _: &mut Trades, cx: &mut Cx<'_, '_, '_>) {
        cx.memories.erase(MemoryKind::InteractionTarget);
    }
}

// ------------------------------------------------------------------------------------
// The interactions.

/// `SocializeAtBell.create()` (meet 2): one tick in a hundred (the level's draw), within
/// 4 of the meeting point's centre and seeing a villager, the closest seen villager
/// within √32 becomes its interaction target, looked at and walked to at 0.3.
struct SocializeAtBell;

/// `SocializeAtBell.create()` (meet 2).
pub(crate) fn socialize_at_bell() -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(SocializeAtBell))
}

/// `BlockPos.closerToCenterThan(position, distance)`: the block's centre strictly within
/// `distance` of the point.
fn closer_to_centre_than(pos: BlockPos, point: Vec3, distance: f64) -> bool {
    physics::distance_sqr(super::centre(pos), point) < distance * distance
}

/// `BlockPos.closerThan(other, distance)`: strictly within, block to block.
fn closer_than(a: BlockPos, b: BlockPos, distance: f64) -> bool {
    crate::region::pois::dist_sqr(a, b) < distance * distance
}

impl Shot for SocializeAtBell {
    fn required(&self) -> &[MemoryKind] {
        &[
            MemoryKind::WalkTarget,
            MemoryKind::LookTarget,
            MemoryKind::MeetingPoint,
            MemoryKind::VisibleMobs,
            MemoryKind::InteractionTarget,
        ]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx
            .memories
            .check(MemoryKind::WalkTarget, Status::Registered)
            || !cx
                .memories
                .check(MemoryKind::LookTarget, Status::Registered)
            || !cx.memories.check(MemoryKind::VisibleMobs, Status::Present)
            || !cx
                .memories
                .check(MemoryKind::InteractionTarget, Status::Absent)
        {
            return false;
        }
        let Some(Value::Block(meeting)) = cx.memories.get(MemoryKind::MeetingPoint) else {
            return false;
        };
        if cx.ai.tick.draws.next_int(100) != 0
            || !closer_to_centre_than(meeting, cx.ai.position(), 4.0)
            || find_closest_visible(cx, &mut |_, looked| looked.kind == Some(VILLAGER)).is_none()
        {
            return false;
        }
        let position = cx.ai.position();
        let found = find_closest_visible(cx, &mut |_, looked| {
            looked.kind == Some(VILLAGER) && looked.distance_sqr(position) <= 32.0
        });
        if let Some(other) = found {
            cx.memories
                .set(MemoryKind::InteractionTarget, Value::Entity(other.key));
            cx.memories.set(
                MemoryKind::LookTarget,
                Value::Tracker(tracking(other.key, true)),
            );
            cx.memories.set(
                MemoryKind::WalkTarget,
                walk(tracking(other.key, false), 0.3, 1),
            );
        }
        true
    }
}

/// `InteractWith.of(type, maxDistance, selfFilter, targetFilter, memory, speed,
/// closeEnough)`: where the villager itself passes `self_test` and it sees one of the
/// type that passes `target_test`, the closest such within `max_distance` becomes the
/// `memory` target, looked at and walked to.
struct InteractWith {
    kind: u16,
    max_distance: i32,
    required: [MemoryKind; 4],
    speed: f32,
    close: i32,
    self_test: fn(&Cx<'_, '_, '_>) -> bool,
    target_test: fn(&Cx<'_, '_, '_>, &Looked) -> bool,
}

/// `InteractWith.of(kind, maxDistance, memory, speed, close)`: an entity of the type
/// `kind` within `max_distance` becomes the `memory` target, walked to.
pub(crate) fn interact_with(
    kind: u16,
    max_distance: i32,
    memory: MemoryKind,
    speed: f32,
    close: i32,
) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(InteractWith {
        kind,
        max_distance,
        required: [
            memory,
            MemoryKind::LookTarget,
            MemoryKind::WalkTarget,
            MemoryKind::VisibleMobs,
        ],
        speed,
        close,
        self_test: |_| true,
        target_test: |_, _| true,
    }))
}

/// `Villager.canBreed()` of the villager itself.
fn self_can_breed(cx: &Cx<'_, '_, '_>) -> bool {
    let (shared, own) = villager::state_of(&cx.ai.tick.mob);
    can_breed(&shared, &own, cx.ai.tick.lived)
}

/// `AgeableMob.canBreed()` of a villager seen, from its record: `Villager.canBreed()`.
fn target_can_breed(cx: &Cx<'_, '_, '_>, looked: &Looked) -> bool {
    record(cx, looked.key).is_some_and(|entity| {
        villager_of(entity).is_some_and(|(shared, own)| can_breed(&shared, &own, entity.lived))
    })
}

/// `InteractWith.of(VILLAGER, 8, Villager::canBreed, AgeableMob::canBreed, breed_target,
/// speed, close)` (idle 2): a partner for love.
pub(crate) fn interact_with_breedable(
    max_distance: i32,
    speed: f32,
    close: i32,
) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(InteractWith {
        kind: VILLAGER,
        max_distance,
        required: [
            MemoryKind::BreedTarget,
            MemoryKind::LookTarget,
            MemoryKind::WalkTarget,
            MemoryKind::VisibleMobs,
        ],
        speed,
        close,
        self_test: self_can_breed,
        target_test: target_can_breed,
    }))
}

impl Shot for InteractWith {
    fn required(&self) -> &[MemoryKind] {
        &self.required
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        let memory = self.required[0];
        if !cx.memories.check(memory, Status::Registered)
            || !cx
                .memories
                .check(MemoryKind::LookTarget, Status::Registered)
            || !cx.memories.check(MemoryKind::WalkTarget, Status::Absent)
            || !cx.memories.check(MemoryKind::VisibleMobs, Status::Present)
        {
            return false;
        }
        if !(self.self_test)(cx) {
            return false;
        }
        let (kind, target_test) = (self.kind, self.target_test);
        let seen = find_closest_visible(cx, &mut |cx, looked| {
            is_kind(looked, kind) && target_test(cx, looked)
        });
        if seen.is_none() {
            return false;
        }
        let position = cx.ai.position();
        let most = f64::from(self.max_distance) * f64::from(self.max_distance);
        let found = find_closest_visible(cx, &mut |cx, looked| {
            looked.distance_sqr(position) <= most
                && is_kind(looked, kind)
                && target_test(cx, looked)
        });
        if let Some(other) = found {
            cx.memories.set(memory, Value::Entity(other.key));
            cx.memories.set(
                MemoryKind::LookTarget,
                Value::Tracker(tracking(other.key, true)),
            );
            cx.memories.set(
                MemoryKind::WalkTarget,
                walk(tracking(other.key, false), self.speed, self.close),
            );
        }
        true
    }
}

/// `SetLookAndInteract.create(type, maxDistance)`: with no interaction target, the
/// closest seen of the type within `max_distance` becomes it, and is looked at.
struct SetLookAndInteract {
    kind: u16,
    max_distance: i32,
}

/// `SetLookAndInteract.create(kind, maxDistance)`.
pub(crate) fn set_look_and_interact(kind: u16, max_distance: i32) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(SetLookAndInteract { kind, max_distance }))
}

impl Shot for SetLookAndInteract {
    fn required(&self) -> &[MemoryKind] {
        &[
            MemoryKind::LookTarget,
            MemoryKind::InteractionTarget,
            MemoryKind::VisibleMobs,
        ]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx
            .memories
            .check(MemoryKind::LookTarget, Status::Registered)
            || !cx
                .memories
                .check(MemoryKind::InteractionTarget, Status::Absent)
            || !cx.memories.check(MemoryKind::VisibleMobs, Status::Present)
        {
            return false;
        }
        let position = cx.ai.position();
        let most = f64::from(self.max_distance * self.max_distance);
        let kind = self.kind;
        let Some(other) = find_closest_visible(cx, &mut |_, looked| {
            looked.distance_sqr(position) <= most && is_kind(looked, kind)
        }) else {
            return false;
        };
        cx.memories
            .set(MemoryKind::InteractionTarget, Value::Entity(other.key));
        cx.memories.set(
            MemoryKind::LookTarget,
            Value::Tracker(tracking(other.key, true)),
        );
        true
    }
}

// ------------------------------------------------------------------------------------
// Trading with players.

/// `ShowTradesToPlayer(minDuration, maxDuration)`: an adult with a player as its
/// interaction target within √17 looks at them; while the item in their main hand is
/// the cost of one of its offers in stock, it holds that offer's result, cycling every
/// 40 ticks through each such offer (an equipment change clients see), for up to 900
/// ticks, else for up to 40 more.
struct ShowTradesToPlayer {
    min: i64,
    max: i64,
}

/// What it keeps (`playerItemStack`, `displayItems`, `cycleCounter`, `displayIndex`,
/// `lookTime`): the player's item by id, none before the first look.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Showing {
    player_item: Option<i32>,
    display: Vec<ItemStack>,
    cycle: i32,
    index: i32,
    look_time: i32,
}

/// `ShowTradesToPlayer(minDuration, maxDuration)`.
pub(crate) fn show_trades_to_player(min: i32, max: i32) -> Box<dyn Behaviour> {
    Box::new(HeldTimed::new(ShowTradesToPlayer {
        min: i64::from(min),
        max: i64::from(max),
    }))
}

/// The main hand of the villager set to `stack`, of the drop chance `chance`
/// (`setItemSlot`, `setDropChance`).
fn hold(mob: &mut Mob, stack: Option<ItemStack>, chance: f32) {
    let hand = usize::from(slots::MAIN_HAND);
    mob.body.equipment.slots[hand] = stack;
    let mut chances = mob.body.equipment.drop_chances.unwrap_or([0.085; 8]);
    chances[hand] = chance;
    mob.body.equipment.drop_chances = (chances != [0.085; 8]).then_some(chances);
}

impl ShowTradesToPlayer {
    /// The player of its interaction target, where it is one the view has.
    fn target(cx: &Cx<'_, '_, '_>) -> Option<Looked> {
        let key = entity_of(cx, MemoryKind::InteractionTarget)?;
        find(cx.ai, key)
    }

    /// `lookAtTarget(villager)`: its interaction target looked at.
    fn look_at_target(cx: &mut Cx<'_, '_, '_>) -> Option<Looked> {
        let target = Self::target(cx)?;
        cx.memories.set(
            MemoryKind::LookTarget,
            Value::Tracker(tracking(target.key, true)),
        );
        Some(target)
    }
}

impl Timed for ShowTradesToPlayer {
    type State = Showing;

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[(MemoryKind::InteractionTarget, Status::Present)]
    }

    fn durations(&self) -> (i64, i64) {
        (self.min, self.max)
    }

    fn extra_start(&self, _: &mut Showing, cx: &mut Cx<'_, '_, '_>) -> bool {
        let Some(target) = Self::target(cx) else {
            return false;
        };
        target.is_player()
            && !cx.ai.tick.mob.body.vitals.dying()
            && target.alive
            && !animal::is_baby(&cx.ai.tick.mob)
            && target.distance_sqr(cx.ai.position()) <= 17.0
    }

    fn can_still_use(&self, showing: &mut Showing, cx: &mut Cx<'_, '_, '_>) -> bool {
        self.extra_start(showing, cx)
            && showing.look_time > 0
            && cx
                .memories
                .check(MemoryKind::InteractionTarget, Status::Present)
    }

    fn start(&self, showing: &mut Showing, cx: &mut Cx<'_, '_, '_>) {
        let _ = Self::look_at_target(cx);
        showing.cycle = 0;
        showing.index = 0;
        showing.look_time = 40;
    }

    fn tick(&self, showing: &mut Showing, cx: &mut Cx<'_, '_, '_>) {
        let Some(target) = Self::look_at_target(cx) else {
            return;
        };
        // `findItemsToDisplay`.
        let held = target
            .main_hand
            .as_ref()
            .filter(|stack| stack.count > 0)
            .map_or(items::AIR, |stack| stack.item);
        let changed = showing.player_item != Some(held);
        if changed {
            showing.player_item = Some(held);
            showing.display.clear();
        }
        if changed && held != items::AIR {
            let (_, own) = villager::state_of(&cx.ai.tick.mob);
            for offer in own.offers.iter().flat_map(|offers| offers.0.iter()) {
                let matches = offer.cost_a.item == held
                    || offer.cost_b.as_ref().is_some_and(|cost| cost.item == held);
                if !offer.is_out_of_stock() && matches {
                    showing.display.push(offer.assemble());
                }
            }
            if let Some(first) = showing.display.first().cloned() {
                showing.look_time = 900;
                hold(&mut cx.ai.tick.mob, Some(first), 0.0);
            }
        }
        if showing.display.is_empty() {
            hold(&mut cx.ai.tick.mob, None, 0.085);
            showing.look_time = showing.look_time.min(40);
        } else if showing.display.len() >= 2 {
            // `displayCyclingItems`.
            showing.cycle += 1;
            if showing.cycle >= 40 {
                showing.index += 1;
                showing.cycle = 0;
                if showing.index > showing.display.len() as i32 - 1 {
                    showing.index = 0;
                }
                let shown = showing.display[showing.index as usize].clone();
                hold(&mut cx.ai.tick.mob, Some(shown), 0.0);
            }
        }
        showing.look_time -= 1;
    }

    fn stop(&self, showing: &mut Showing, cx: &mut Cx<'_, '_, '_>) {
        cx.memories.erase(MemoryKind::InteractionTarget);
        hold(&mut cx.ai.tick.mob, None, 0.085);
        showing.player_item = None;
    }
}

/// `LookAndFollowTradingPlayerSink(speed)` (core 3, a `Behavior` that never times
/// out): while it lives, out of water, not hurt lately, with its trading player within
/// 4, it walks to within 2 of them and looks at them.
struct LookAndFollowTradingPlayer {
    speed: f32,
}

/// `LookAndFollowTradingPlayerSink(speed)` (core 3).
pub(crate) fn look_and_follow_trading_player(speed: f32) -> Box<dyn Behaviour> {
    Box::new(HeldTimed::new(LookAndFollowTradingPlayer { speed }))
}

impl LookAndFollowTradingPlayer {
    /// `getTradingPlayer()`, where the view has them.
    fn trading_player(cx: &Cx<'_, '_, '_>) -> Option<Looked> {
        let player = villager::state_of(&cx.ai.tick.mob).1.trading_player?;
        let seen = cx
            .ai
            .tick
            .world
            .players()
            .into_iter()
            .find(|seen| seen.uuid == player.0.as_u128())?;
        Some(Looked::of_player(&seen))
    }

    /// `followPlayer(villager)`.
    fn follow(&self, cx: &mut Cx<'_, '_, '_>) {
        let Some(player) = Self::trading_player(cx) else {
            return;
        };
        cx.memories.set(
            MemoryKind::WalkTarget,
            walk(tracking(player.key, false), self.speed, 2),
        );
        cx.memories.set(
            MemoryKind::LookTarget,
            Value::Tracker(tracking(player.key, true)),
        );
    }
}

/// `wasHurtRecently()`: its hurt time, 10 at a hurt, not yet run out.
fn hurt_recently(tick: &MobTick<'_>) -> bool {
    let now = crate::mobs::body::moment(tick);
    tick.mob
        .body
        .vitals
        .hurt_from
        .is_some_and(|from| i64::from(now) - i64::from(from) < 10)
}

impl Timed for LookAndFollowTradingPlayer {
    type State = ();

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[
            (MemoryKind::WalkTarget, Status::Registered),
            (MemoryKind::LookTarget, Status::Registered),
        ]
    }

    fn durations(&self) -> (i64, i64) {
        (i64::from(i32::MAX), i64::from(i32::MAX))
    }

    fn timed_out(&self, _: &(), _: &Cx<'_, '_, '_>, _: i64) -> bool {
        false
    }

    fn extra_start(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) -> bool {
        let Some(player) = Self::trading_player(cx) else {
            return false;
        };
        !cx.ai.tick.mob.body.vitals.dying()
            && !cx.ai.in_water()
            && !hurt_recently(cx.ai.tick)
            && player.distance_sqr(cx.ai.position()) <= 16.0
    }

    fn can_still_use(&self, state: &mut (), cx: &mut Cx<'_, '_, '_>) -> bool {
        self.extra_start(state, cx)
    }

    fn start(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        self.follow(cx);
    }

    fn tick(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        self.follow(cx);
    }

    fn stop(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        cx.memories.erase(MemoryKind::WalkTarget);
        cx.memories.erase(MemoryKind::LookTarget);
    }
}

// ------------------------------------------------------------------------------------
// Love.

/// `VillagerMakeLove` (a `Behavior` of 350 ticks, idle 3 in a gate erasing
/// `breed_target`): with a villager it sees as its breed target, both able to breed,
/// both lock their gaze and walk to each other and show hearts (entity event 18); the
/// birth is 275 to 324 ticks on (its own draw). Within √5, before the birth one tick in
/// 35 (its own draw) both show hearts again (12); at the birth both digest their food
/// and, with a free bed within 48 it can path to, a baby is born there (its own draw of
/// the type, both parents of age 6000) and given the bed, or without one both are angry
/// (13).
struct VillagerMakeLove;

/// `VillagerMakeLove` (idle 3, in a gate erasing `breed_target`).
pub(crate) fn villager_make_love() -> Box<dyn Behaviour> {
    Box::new(HeldTimed::new(VillagerMakeLove))
}

impl VillagerMakeLove {
    /// `isBreedingPossible(villager)`: its breed target a villager that lives and is
    /// seen, and both able to breed.
    fn possible(cx: &mut Cx<'_, '_, '_>) -> bool {
        let Some(key) = entity_of(cx, MemoryKind::BreedTarget) else {
            return false;
        };
        let Some(partner) = record(cx, key).filter(|entity| entity.kind == VILLAGER) else {
            return false;
        };
        if !target_is_valid(cx, MemoryKind::BreedTarget) || !self_can_breed(cx) {
            return false;
        }
        villager_of(partner).is_some_and(|(shared, own)| can_breed(&shared, &own, partner.lived))
    }

    /// An entity event of the partner `key` and then its own.
    fn events(cx: &mut Cx<'_, '_, '_>, partner: EntityId, event: u8) {
        cx.ai.tick.out.local.push(Local::EntityEvent {
            key: partner,
            event,
        });
        cx.ai.tick.out.events.push(Event::Entity(event));
    }

    /// `takeVacantBed(level, villager)`: the first free home within 48 of its block, in
    /// the query's order, that its navigation finds a path to within the point's valid
    /// range (a search for each until one reaches), its ticket taken.
    fn take_vacant_bed(cx: &mut Cx<'_, '_, '_>) -> Option<BlockPos> {
        let world = cx.ai.tick.world;
        let at = cx.ai.block_position();
        let beds = world.pois_in_range(at, 48, &|kind| kind == PoiType::Home, Occupancy::HasSpace);
        let bed = beds.into_iter().find(|bed| {
            let reach = i32::from(bed.kind.valid_range());
            navigation::create_path(cx.ai, bed.pos, reach)
                .is_some_and(|made| made.path(cx.ai).is_some_and(|path| path.reached))
        })?;
        cx.ai.tick.out.local.push(Local::TakePoi { pos: bed.pos });
        Some(bed.pos)
    }

    /// `tryToGiveBirth(level, villager, partner)`.
    fn give_birth(cx: &mut Cx<'_, '_, '_>, partner: &Entity) {
        let Some(bed) = Self::take_vacant_bed(cx) else {
            Self::events(cx, partner.key, 13);
            return;
        };
        let Ok(partner_mob) = Mob::decode(&partner.data) else {
            cx.ai.tick.out.local.push(Local::ReleasePoi {
                pos: bed,
                kind: PoiType::Home,
            });
            return;
        };
        // `breed`: `getBreedOffspring`, its own draw.
        let biome = cx.ai.tick.world.biome(cx.ai.block_position());
        let parent = cx.ai.tick.mob.clone();
        let mut breeding = Breeding {
            this: &parent,
            partner: &partner_mob,
            partner_kind: VILLAGER,
            child_kind: None,
            draws: &mut *cx.ai.tick.draws,
            biome,
            egg_user: None,
        };
        let Some(mut child) = types::of(VILLAGER).breed_offspring(&mut breeding) else {
            cx.ai.tick.out.local.push(Local::ReleasePoi {
                pos: bed,
                kind: PoiType::Home,
            });
            return;
        };
        let lived = cx.ai.tick.lived;
        animal::update(&mut cx.ai.tick.mob, |shared| shared.set_age(6000, lived));
        cx.ai.tick.out.local.push(Local::Villager {
            key: partner.key,
            used: VillagerUse::Born,
        });
        animal::made_baby(&mut child);
        // `giveBedToChild`: its home the bed.
        if let Some(provider) = types::of(VILLAGER).brain() {
            let mut memories = super::super::memories_of(provider, &child, 0);
            memories.set(MemoryKind::Home, Value::Block(bed));
            super::super::write_memories(provider, &mut child, memories);
        }
        cx.ai.tick.out.local.push(Local::MakeMob {
            kind: VILLAGER,
            mob: Box::new(child),
            at: cx.ai.position(),
        });
    }
}

impl Timed for VillagerMakeLove {
    /// `birthTimestamp`, a game time.
    type State = i64;

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[
            (MemoryKind::BreedTarget, Status::Present),
            (MemoryKind::VisibleMobs, Status::Present),
        ]
    }

    fn durations(&self) -> (i64, i64) {
        (350, 350)
    }

    fn extra_start(&self, _: &mut i64, cx: &mut Cx<'_, '_, '_>) -> bool {
        Self::possible(cx)
    }

    fn can_still_use(&self, birth: &mut i64, cx: &mut Cx<'_, '_, '_>) -> bool {
        cx.time <= *birth && Self::possible(cx)
    }

    fn start(&self, birth: &mut i64, cx: &mut Cx<'_, '_, '_>) {
        let Some(partner) = entity_of(cx, MemoryKind::BreedTarget) else {
            return;
        };
        lock_gaze(cx, partner, 0.5, 2);
        Self::events(cx, partner, 18);
        *birth = cx.time + 275 + i64::from(cx.ai.tick.draws.next_int(50));
    }

    fn tick(&self, birth: &mut i64, cx: &mut Cx<'_, '_, '_>) {
        let Some(key) = entity_of(cx, MemoryKind::BreedTarget) else {
            return;
        };
        let Some(partner) = record(cx, key) else {
            return;
        };
        if physics::distance_sqr(cx.ai.position(), partner.position) > 5.0 {
            return;
        }
        lock_gaze(cx, key, 0.5, 2);
        if cx.time >= *birth {
            villager::update(&mut cx.ai.tick.mob, eat_and_digest);
            // The partner's digestion rides on the birth's write (`VillagerUse::Born`).
            Self::give_birth(cx, partner);
        } else if cx.ai.tick.draws.next_int(35) == 0 {
            Self::events(cx, key, 12);
        }
    }

    fn stop(&self, birth: &mut i64, cx: &mut Cx<'_, '_, '_>) {
        cx.memories.erase(MemoryKind::BreedTarget);
        *birth = 0;
    }
}

// ------------------------------------------------------------------------------------
// The Hero of the Village.

/// `GiveGiftToHero(duration)`: while a Hero of the Village is the nearest player it
/// sees, its wait of 600 counts down (only on those ticks); once out, it looks at the
/// hero and walks to within 5 of them, and twenty ticks after its start throws them a
/// gift from its table (a baby's, its profession's or the unemployed's); then it waits
/// 600 to 6600 (the level's draw) again.
struct GiveGiftToHero {
    duration: i64,
}

/// What it keeps (`timeUntilNextGift`, `giftGivenDuringThisRun`, `timeSinceStart`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Gifting {
    until_next: i32,
    given: bool,
    since: i64,
}

impl Default for Gifting {
    fn default() -> Self {
        Self {
            until_next: 600,
            given: false,
            since: 0,
        }
    }
}

/// `GiveGiftToHero(duration)`.
pub(crate) fn give_gift_to_hero(duration: i32) -> Box<dyn Behaviour> {
    Box::new(HeldTimed::new(GiveGiftToHero {
        duration: i64::from(duration),
    }))
}

impl GiveGiftToHero {
    /// `getNearestTargetableHero(villager)`: the nearest player it sees, where they are
    /// a Hero of the Village.
    fn hero(cx: &Cx<'_, '_, '_>) -> Option<Looked> {
        let key = entity_of(cx, MemoryKind::NearestVisiblePlayer)?;
        let player = find(cx.ai, key)?;
        cx.ai
            .tick
            .world
            .player_omens(key)
            .hero
            .is_some()
            .then_some(player)
    }

    /// `throwGift(level, villager, hero)`: each stack its gift table gives (drawn from
    /// its stream, as the loot of its kind is) thrown at the hero.
    fn throw_gift(cx: &mut Cx<'_, '_, '_>, hero: &Looked) {
        let (shared, own) = villager::state_of(&cx.ai.tick.mob);
        let profession = clustine_data::villagers::PROFESSION_NAMES
            .get(usize::from(own.data.profession))
            .copied()
            .unwrap_or("minecraft:none");
        let table = clustine_data::loot::hero_gift_table(profession, shared.is_baby());
        let gifts = crate::mobs::body::loot::named_drops(
            table.name,
            VILLAGER,
            shared.is_baby(),
            crate::mobs::body::loot::Facts::default(),
            cx.ai.tick.draws,
        );
        for gift in gifts.into_iter().filter(|stack| stack.count > 0) {
            throw_item(cx, gift, hero.position);
        }
    }
}

impl Timed for GiveGiftToHero {
    type State = Gifting;

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[
            (MemoryKind::WalkTarget, Status::Registered),
            (MemoryKind::LookTarget, Status::Registered),
            (MemoryKind::InteractionTarget, Status::Registered),
            (MemoryKind::NearestVisiblePlayer, Status::Present),
        ]
    }

    fn durations(&self) -> (i64, i64) {
        (self.duration, self.duration)
    }

    fn extra_start(&self, gifting: &mut Gifting, cx: &mut Cx<'_, '_, '_>) -> bool {
        if Self::hero(cx).is_none() {
            return false;
        }
        if gifting.until_next > 0 {
            gifting.until_next -= 1;
            return false;
        }
        true
    }

    fn can_still_use(&self, gifting: &mut Gifting, cx: &mut Cx<'_, '_, '_>) -> bool {
        Self::hero(cx).is_some() && !gifting.given
    }

    fn start(&self, gifting: &mut Gifting, cx: &mut Cx<'_, '_, '_>) {
        gifting.given = false;
        gifting.since = cx.time;
        let Some(hero) = Self::hero(cx) else {
            return;
        };
        cx.memories
            .set(MemoryKind::InteractionTarget, Value::Entity(hero.key));
        cx.memories.set(
            MemoryKind::LookTarget,
            Value::Tracker(tracking(hero.key, true)),
        );
    }

    fn tick(&self, gifting: &mut Gifting, cx: &mut Cx<'_, '_, '_>) {
        let Some(hero) = Self::hero(cx) else {
            return;
        };
        cx.memories.set(
            MemoryKind::LookTarget,
            Value::Tracker(tracking(hero.key, true)),
        );
        if closer_than(cx.ai.block_position(), hero.block_position(), 5.0) {
            if cx.time - gifting.since > 20 {
                Self::throw_gift(cx, &hero);
                gifting.given = true;
            }
        } else {
            let tracker = tracking(hero.key, true);
            cx.memories
                .set(MemoryKind::LookTarget, Value::Tracker(tracker));
            cx.memories
                .set(MemoryKind::WalkTarget, walk(tracker, 0.5, 5));
        }
    }

    fn stop(&self, gifting: &mut Gifting, cx: &mut Cx<'_, '_, '_>) {
        gifting.until_next = 600 + cx.ai.tick.draws.next_int(6001) as i32;
        cx.memories.erase(MemoryKind::InteractionTarget);
        cx.memories.erase(MemoryKind::WalkTarget);
        cx.memories.erase(MemoryKind::LookTarget);
    }
}

// ------------------------------------------------------------------------------------
// Bells, hiding and raids.

/// `Raids.getRaidAt(pos)`: no raid exists before N10d, so none is near (its rule draws
/// nothing).
fn raid_at(_: &Cx<'_, '_, '_>, _: BlockPos) -> Option<()> {
    None
}

/// `RingBell.create()` (pre-raid 0): one tick in twenty (the level's draw, above 0.95),
/// within 3 of its meeting point, a bell there is rung.
struct RingBell;

/// `RingBell.create()` (pre-raid 0).
pub(crate) fn ring_bell() -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(RingBell))
}

impl Shot for RingBell {
    fn required(&self) -> &[MemoryKind] {
        &[MemoryKind::MeetingPoint]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        let Some(Value::Block(meeting)) = cx.memories.get(MemoryKind::MeetingPoint) else {
            return false;
        };
        if cx.ai.tick.draws.next_float() <= 0.95 {
            return false;
        }
        if closer_than(meeting, cx.ai.block_position(), 3.0)
            && clustine_blocks::data::is_block(
                cx.ai.tick.world.state(meeting),
                clustine_data::blocks::BELL,
            )
        {
            cx.ai.tick.out.local.push(Local::RingBell { pos: meeting });
        }
        true
    }
}

/// `ReactToBell.create()` (core 0): having heard a bell, with no raid near, it hides
/// (`setActiveActivityIfPossible(HIDE)`).
struct ReactToBell;

/// `ReactToBell.create()` (core 0).
pub(crate) fn react_to_bell() -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(ReactToBell))
}

impl Shot for ReactToBell {
    fn required(&self) -> &[MemoryKind] {
        &[MemoryKind::HeardBellTime]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx
            .memories
            .check(MemoryKind::HeardBellTime, Status::Present)
        {
            return false;
        }
        if raid_at(cx, cx.ai.block_position()).is_none() {
            cx.memories.requested.push(Activity::Hide);
        }
        true
    }
}

/// `SetHiddenState.create(seconds, close)` (hide 0): hidden, it counts the ticks it is
/// within `close` of its hiding place; past `seconds` of them, or 300 ticks after the
/// bell, it forgets the bell and its hiding place and takes the schedule's activity
/// again.
struct SetHiddenState {
    ticks: i32,
    close: i32,
}

/// `SetHiddenState.create(seconds, close)` (hide 0).
pub(crate) fn set_hidden_state(seconds: i32, close: i32) -> Box<dyn Behaviour> {
    Box::new(HeldKept::new(SetHiddenState {
        ticks: seconds * 20,
        close,
    }))
}

/// `SetHiddenState.HIDE_TIMEOUT`.
const HIDE_TIMEOUT: i64 = 300;

impl KeptShot for SetHiddenState {
    /// Its `MutableInt`: the ticks spent at the hiding place.
    type State = i32;

    fn required(&self) -> &[MemoryKind] {
        &[MemoryKind::HidingPlace, MemoryKind::HeardBellTime]
    }

    fn trigger(&self, hidden: &mut i32, cx: &mut Cx<'_, '_, '_>) -> bool {
        let Some(Value::Block(place)) = cx.memories.get(MemoryKind::HidingPlace) else {
            return false;
        };
        let Some(Value::Long(heard)) = cx.memories.get(MemoryKind::HeardBellTime) else {
            return false;
        };
        let timed_out = heard + HIDE_TIMEOUT <= cx.time;
        if *hidden > self.ticks || timed_out {
            cx.memories.erase(MemoryKind::HeardBellTime);
            cx.memories.erase(MemoryKind::HidingPlace);
            villager::update_from_schedule(cx);
            *hidden = 0;
            return true;
        }
        if closer_than(place, cx.ai.block_position(), f64::from(self.close)) {
            *hidden += 1;
        }
        true
    }
}

/// `LocateHidingPlace.create(radius, speed, close)`: with no walk target, a home within
/// `close + 1` (the first in the query's order, kept only if within `close` of its
/// centre), else a home within `radius` at random (its own stream's shuffle), else its
/// own home becomes its hiding place, its path, look, breed and interaction targets
/// forgotten, and walked to unless it is within `close`.
struct LocateHidingPlace {
    radius: i32,
    speed: f32,
    close: i32,
}

/// `LocateHidingPlace.create(radius, speed, close)`.
pub(crate) fn locate_hiding_place(radius: i32, speed: f32, close: i32) -> Box<dyn Behaviour> {
    Box::new(HeldShot::new(LocateHidingPlace {
        radius,
        speed,
        close,
    }))
}

/// `Util.toShuffledList(list, random)`'s shuffle: one `nextInt(i)` for `i` from the
/// count down to 2, swapping into place.
fn shuffle<T>(list: &mut [T], draws: &mut Random) {
    for i in (2..=list.len()).rev() {
        let j = draws.next_int(i as u32) as usize;
        list.swap(i - 1, j);
    }
}

impl Shot for LocateHidingPlace {
    fn required(&self) -> &[MemoryKind] {
        &[
            MemoryKind::WalkTarget,
            MemoryKind::Home,
            MemoryKind::HidingPlace,
            MemoryKind::Path,
            MemoryKind::LookTarget,
            MemoryKind::BreedTarget,
            MemoryKind::InteractionTarget,
        ]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx.memories.check(MemoryKind::WalkTarget, Status::Absent) {
            return false;
        }
        let world = cx.ai.tick.world;
        let at = cx.ai.block_position();
        let position = cx.ai.position();
        let home = |kind| kind == PoiType::Home;
        let close = f64::from(self.close);
        let near = world
            .pois_in_range(at, self.close + 1, &home, Occupancy::Any)
            .into_iter()
            .next()
            .map(|record| record.pos)
            .filter(|pos| closer_to_centre_than(*pos, position, close));
        let place = near
            .or_else(|| {
                let mut homes = world.pois_in_range(at, self.radius, &home, Occupancy::Any);
                shuffle(&mut homes, cx.ai.tick.draws);
                homes.first().map(|record| record.pos)
            })
            .or_else(|| match cx.memories.get(MemoryKind::Home) {
                Some(Value::Block(pos)) => Some(pos),
                _ => None,
            });
        if let Some(place) = place {
            for kind in [
                MemoryKind::Path,
                MemoryKind::LookTarget,
                MemoryKind::BreedTarget,
                MemoryKind::InteractionTarget,
            ] {
                cx.memories.erase(kind);
            }
            cx.memories
                .set(MemoryKind::HidingPlace, Value::Block(place));
            if !closer_to_centre_than(place, position, close) {
                cx.memories.set(
                    MemoryKind::WalkTarget,
                    walk(Tracker::Block(place), self.speed, self.close),
                );
            }
        }
        true
    }
}

/// `CelebrateVillagersSurvivedRaid(minDuration, maxDuration)` (raid 0): where a raid it
/// is in was won and nothing is above it, it cheers (one tick in 100, its own draw) and
/// sets off a rocket of one colour (one in 200 under the open sky); at its end the
/// schedule's activity again. No raid exists before N10d: it never starts.
struct CelebrateVillagersSurvivedRaid {
    min: i64,
    max: i64,
}

/// `CelebrateVillagersSurvivedRaid(min, max)` (raid 0).
pub(crate) fn celebrate_villagers_survived_raid(min: i32, max: i32) -> Box<dyn Behaviour> {
    Box::new(HeldTimed::new(CelebrateVillagersSurvivedRaid {
        min: i64::from(min),
        max: i64::from(max),
    }))
}

impl Timed for CelebrateVillagersSurvivedRaid {
    type State = ();

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[]
    }

    fn durations(&self) -> (i64, i64) {
        (self.min, self.max)
    }

    fn extra_start(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) -> bool {
        // `getRaidAt(pos)` won and `hasNoBlocksAbove`: no raid exists yet.
        raid_at(cx, cx.ai.block_position()).is_some()
    }

    fn can_still_use(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) -> bool {
        raid_at(cx, cx.ai.block_position()).is_some()
    }

    fn tick(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        if cx.ai.tick.draws.next_int(100) == 0 {
            let baby = animal::is_baby(&cx.ai.tick.mob);
            let pitch = crate::mobs::body::hurt::voice_pitch(cx.ai.tick.draws, baby);
            cx.ai.tick.out.events.push(Event::Sound {
                sound: sounds::ENTITY_VILLAGER_CELEBRATE,
                volume: 1.0,
                pitch,
            });
        }
        // The rocket of one in 200 under the open sky is made with raids (N10d).
    }

    fn stop(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        villager::update_from_schedule(cx);
    }
}
