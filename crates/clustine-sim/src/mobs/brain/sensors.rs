//! The sensors the brains share (`ai.sensing`, the brief's 7.1.3 and 7.2): what each
//! requires, what its scan writes, and the readers of what it wrote that the behaviours
//! share (`NearestVisibleLivingEntities`, `Sensor.isEntityTargetable` and its kin).
//!
//! A scan reads the region's land, copies and faces and draws nothing. Its sensing again
//! after a take-in (the plan, section 8 item 1) writes only the memories the region keeps
//! beside the record ([`MemoryKind::sensed`]): the hurt-by sensor writes only `hurt_by`,
//! and a sensor that senses nothing of the kind writes nothing.
//!
//! Two forms the contract's [`Value`] lacks are carried in forms it has, here alone:
//!
//! - `visible_mobs` is never empty as a memory in the game (`NearestVisibleLivingEntities`
//!   is an object even for an empty list, and a behaviour whose entry asks it present
//!   starts on it), where an empty list erases a memory: an empty one is kept as
//!   [`Value::Unit`], which [`visible_mobs`] reads as an empty list.
//! - `hurt_by` holds a `DamageSource`: it is kept as its damage type's id in the registry
//!   the server sends ([`Value::Int`]), which is all its readers ask of it (its tags); the
//!   entity it names is `hurt_by_entity`'s.
//!
//! **Named**: whether an entity of `visible_mobs` is seen is decided at each asking with
//! the AI step's cache of lines of sight, where the game decides it at the first asking and
//! keeps it for the life of the memory (the brief's 7.1.1 finding 9); and the range that
//! test uses is the asking mob's own follow range, not the last scanner's in the server.

use clustine_blocks::data::{entity_in_tag, item_in_tag};
use clustine_data::{attributes, entity_types, synced_registry};
use clustine_world::{EntityData, EntityId, ItemStack};

use super::{Cx, Memories, MemoryKind, Sensor, Value};
use crate::mobs::Mob;
use crate::mobs::ai::Ai;
use crate::mobs::ai::sensing::{self, Conditions, Looked};
use crate::mobs::body::hurt::DamageType;
use crate::physics;
use crate::physics::aabb::Aabb;

// ------------------------------------------------------------------------------------
// What the sensors' memories hold, as the behaviours read them.

/// Whether a thing a mob sees is a `LivingEntity`: a mob, a player or an armour stand
/// (an item, an orb, a boat are not).
pub(crate) fn is_living(looked: &Looked) -> bool {
    looked.is_player() || looked.mob || looked.kind == Some(entity_types::ARMOR_STAND as u16)
}

/// The thing `key` names, an entity or a player, wherever the region's view has it; none
/// where it has it not, which its readers take as "unknown" (the plan, section 3 item 2).
pub(crate) fn find(ai: &Ai<'_, '_>, key: EntityId) -> Option<Looked> {
    sensing::find(ai, key)
}

/// The mob's follow range (`getAttributeValue(FOLLOW_RANGE)`).
pub(crate) fn follow_range(ai: &Ai<'_, '_>) -> f64 {
    ai.attribute(attributes::FOLLOW_RANGE)
}

/// `visible_mobs` as the nearest-living-entities sensor left it: its list, nearest first,
/// none where it is not there (see the module's comment for its empty form).
pub(crate) fn visible_mobs(memories: &Memories) -> Option<Vec<EntityId>> {
    match memories.get(MemoryKind::VisibleMobs)? {
        Value::Entities(list) => Some(list),
        _ => Some(Vec::new()),
    }
}

/// The attack target the mob remembers, if any.
fn attack_target(memories: &Memories) -> Option<EntityId> {
    match memories.get(MemoryKind::AttackTarget)? {
        Value::Entity(key) => Some(key),
        _ => None,
    }
}

/// `Sensor.isEntityTargetable(level, mob, target)`: the non-combat conditions of the
/// mob's follow range, ignoring invisibility for its attack target.
pub(crate) fn targetable(cx: &mut Cx<'_, '_, '_>, target: &Looked) -> bool {
    let mut conditions = Conditions::non_combat().range(follow_range(cx.ai));
    if attack_target(cx.memories) == Some(target.key) {
        conditions = conditions.ignore_invisibility();
    }
    conditions.test(cx.ai, target, &|_| true)
}

/// `Sensor.isEntityAttackable(level, mob, target)`: the combat conditions likewise.
pub(crate) fn attackable(cx: &mut Cx<'_, '_, '_>, target: &Looked) -> bool {
    let mut conditions = Conditions::combat().range(follow_range(cx.ai));
    if attack_target(cx.memories) == Some(target.key) {
        conditions = conditions.ignore_invisibility();
    }
    conditions.test(cx.ai, target, &|_| true)
}

/// `Sensor.isEntityAttackableIgnoringLineOfSight(level, mob, target)`.
pub(crate) fn attackable_ignoring_line_of_sight(cx: &mut Cx<'_, '_, '_>, target: &Looked) -> bool {
    let mut conditions = Conditions::combat()
        .range(follow_range(cx.ai))
        .ignore_line_of_sight();
    if attack_target(cx.memories) == Some(target.key) {
        conditions = conditions.ignore_invisibility();
    }
    conditions.test(cx.ai, target, &|_| true)
}

/// `NearestVisibleLivingEntities.findClosest(predicate)`: the first of the list, nearest
/// first, that passes `predicate` and is seen; none where the memory is not there. An
/// entity of the list the view no longer has is passed over, as one out of sight.
pub(crate) fn find_closest_visible(
    cx: &mut Cx<'_, '_, '_>,
    predicate: &mut dyn FnMut(&mut Cx<'_, '_, '_>, &Looked) -> bool,
) -> Option<Looked> {
    let list = visible_mobs(cx.memories)?;
    for key in list {
        let Some(looked) = find(cx.ai, key) else {
            continue;
        };
        if predicate(cx, &looked) && targetable(cx, &looked) {
            return Some(looked);
        }
    }
    None
}

/// `NearestVisibleLivingEntities.contains(entity)`: in the list and seen.
pub(crate) fn visible_contains(cx: &mut Cx<'_, '_, '_>, key: EntityId) -> bool {
    let Some(list) = visible_mobs(cx.memories) else {
        return false;
    };
    if !list.contains(&key) {
        return false;
    }
    find(cx.ai, key).is_some_and(|looked| targetable(cx, &looked))
}

/// `isBaby()` of the thing `looked` names: its type's, from its record (a player never).
pub(crate) fn is_baby(ai: &Ai<'_, '_>, looked: &Looked) -> bool {
    let Some(kind) = looked.kind else {
        return false;
    };
    let Some(seen) = ai.tick.world.entity(looked.key) else {
        return false;
    };
    Mob::decode(&seen.entity.data).is_ok_and(|mob| crate::mobs::types::of(kind).is_baby(&mob))
}

/// The living things whose boxes meet `bounds` but the mob itself, alive
/// (`getEntitiesOfClass(LivingEntity, box, e -> e != mob && e.isAlive())`): the view's
/// entities in its order, then its players.
pub(crate) fn living_in(ai: &Ai<'_, '_>, bounds: Aabb) -> Vec<Looked> {
    let own = ai.tick.key;
    let mut found: Vec<Looked> = ai
        .tick
        .world
        .entities_in(&bounds)
        .iter()
        .map(Looked::of_seen)
        .filter(is_living)
        .collect();
    found.extend(
        sensing::players(ai)
            .into_iter()
            .filter(|player| player.bounds.intersects(bounds)),
    );
    found.retain(|looked| looked.key != own && looked.alive);
    found
}

/// The list sorted by `distanceToSqr` from the mob's feet, stably.
fn by_distance(ai: &Ai<'_, '_>, mut list: Vec<Looked>) -> Vec<Looked> {
    let from = ai.position();
    list.sort_by(|a, b| a.distance_sqr(from).total_cmp(&b.distance_sqr(from)));
    list
}

/// A list of things as a memory keeps them.
fn keys(list: &[Looked]) -> Value {
    Value::Entities(list.iter().map(|looked| looked.key).collect())
}

// ------------------------------------------------------------------------------------
// The sensors.

/// `NearestLivingEntitySensor` (`NEAREST_LIVING_ENTITIES`, the brief's 7.2.1): the living
/// things within the follow range of the mob's box on every axis, nearest first, as
/// `mobs`, and the same list as `visible_mobs`.
pub(crate) struct NearestLivingEntitySensor;

pub(crate) static NEAREST_LIVING_ENTITIES: NearestLivingEntitySensor = NearestLivingEntitySensor;

impl Sensor for NearestLivingEntitySensor {
    fn requires(&self) -> &'static [MemoryKind] {
        &[MemoryKind::Mobs, MemoryKind::VisibleMobs]
    }

    fn scan(&self, cx: &mut Cx<'_, '_, '_>) {
        let range = follow_range(cx.ai);
        let bounds = cx.ai.tick.living.body.bounds().inflate([range; 3]);
        let list = by_distance(cx.ai, living_in(cx.ai, bounds));
        cx.memories.set(MemoryKind::Mobs, keys(&list));
        let visible = if list.is_empty() {
            Value::Unit
        } else {
            keys(&list)
        };
        cx.memories.set(MemoryKind::VisibleMobs, visible);
    }
}

/// `PlayerSensor` (`NEAREST_PLAYERS`, 7.2.2): the players not spectating within the
/// follow range (a sphere), nearest first; the first of them seen; those seen that it may
/// attack (none in creative), and the first of those.
pub(crate) struct PlayerSensor;

pub(crate) static NEAREST_PLAYERS: PlayerSensor = PlayerSensor;

impl Sensor for PlayerSensor {
    fn requires(&self) -> &'static [MemoryKind] {
        &[
            MemoryKind::NearestPlayers,
            MemoryKind::NearestVisiblePlayer,
            MemoryKind::NearestVisibleTargetablePlayer,
            MemoryKind::NearestVisibleTargetablePlayers,
        ]
    }

    fn scan(&self, cx: &mut Cx<'_, '_, '_>) {
        let range = follow_range(cx.ai);
        let from = cx.ai.position();
        let near: Vec<Looked> = sensing::players(cx.ai)
            .into_iter()
            .filter(|player| !player.spectator && player.distance_sqr(from) < range * range)
            .collect();
        let near = by_distance(cx.ai, near);
        cx.memories.set(MemoryKind::NearestPlayers, keys(&near));
        let mut visible = Vec::new();
        for player in near {
            if targetable(cx, &player) {
                visible.push(player);
            }
        }
        match visible.first() {
            Some(first) => cx
                .memories
                .set(MemoryKind::NearestVisiblePlayer, Value::Entity(first.key)),
            None => cx.memories.erase(MemoryKind::NearestVisiblePlayer),
        }
        let mut attackable_ones = Vec::new();
        for player in visible {
            if attackable(cx, &player) {
                attackable_ones.push(player);
            }
        }
        cx.memories.set(
            MemoryKind::NearestVisibleTargetablePlayers,
            keys(&attackable_ones),
        );
        match attackable_ones.first() {
            Some(first) => cx.memories.set(
                MemoryKind::NearestVisibleTargetablePlayer,
                Value::Entity(first.key),
            ),
            None => cx
                .memories
                .erase(MemoryKind::NearestVisibleTargetablePlayer),
        }
    }
}

/// The damage a mob last took, as `getLastDamageSource()` gives it: its type, and the
/// living thing that caused it, if one did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LastDamage {
    pub(crate) kind: DamageType,
    pub(crate) cause: Option<EntityId>,
}

/// `getLastDamageSource()` as the body keeps it (`LastDamage`): what last hurt the
/// mob, of whatever cause (fire, lava and a cactus too), within the 40 game ticks the
/// game gives a source for.
pub(crate) fn remembered_damage(cx: &Cx<'_, '_, '_>) -> Option<LastDamage> {
    let (id, cause) = cx.ai.tick.mob.body.last_damage.source(cx.time)?;
    Some(LastDamage {
        kind: DamageType::of_id(id)?,
        cause,
    })
}

/// `HurtBySensor` (`HURT_BY`, 7.2.3): the last damage as `hurt_by` (its type's id, see
/// the module's comment) and its living cause as `hurt_by_entity`, which only death or
/// another level erases. Where the view has `hurt_by_entity` in neither the region nor
/// its copies, it is kept: "unknown" is not dead (the plan, section 8 item 8), so that a
/// mob hit by a player who walks over a line remembers them.
pub(crate) struct HurtBySensor {
    /// `getLastDamageSource()`.
    pub(crate) last_damage: fn(&Cx<'_, '_, '_>) -> Option<LastDamage>,
}

pub(crate) static HURT_BY: HurtBySensor = HurtBySensor {
    last_damage: remembered_damage,
};

impl HurtBySensor {
    fn hurt_by(&self, cx: &mut Cx<'_, '_, '_>) -> Option<LastDamage> {
        let last = (self.last_damage)(cx);
        match last.and_then(|last| last.kind.id()) {
            Some(id) => cx.memories.set(MemoryKind::HurtBy, Value::Int(id)),
            None => cx.memories.erase(MemoryKind::HurtBy),
        }
        last
    }
}

impl Sensor for HurtBySensor {
    fn requires(&self) -> &'static [MemoryKind] {
        &[MemoryKind::HurtBy, MemoryKind::HurtByEntity]
    }

    fn scan(&self, cx: &mut Cx<'_, '_, '_>) {
        if let Some(cause) = self.hurt_by(cx).and_then(|last| last.cause) {
            cx.memories
                .set(MemoryKind::HurtByEntity, Value::Entity(cause));
        }
        if let Some(Value::Entity(key)) = cx.memories.get(MemoryKind::HurtByEntity)
            && find(cx.ai, key).is_some_and(|looked| !looked.alive)
        {
            cx.memories.erase(MemoryKind::HurtByEntity);
        }
    }

    fn resense(&self, cx: &mut Cx<'_, '_, '_>) {
        self.hurt_by(cx);
    }
}

/// The damage type `hurt_by` holds, if it holds one.
pub(crate) fn hurt_by(memories: &Memories) -> Option<DamageType> {
    let Value::Int(id) = memories.get(MemoryKind::HurtBy)? else {
        return None;
    };
    let registry = synced_registry("minecraft:damage_type")?;
    let name = *registry.entries.get(usize::try_from(id).ok()?)?;
    Some(DamageType(name))
}

/// `AdultSensor` and `AdultSensorAnyType` (`NEAREST_ADULT`, `NEAREST_ADULT_ANY_TYPE`,
/// 7.2.4): of the `visible_mobs` this tick's earlier scan left, the closest adult of the
/// mob's own type (or of `#followable_friendly_mobs`); nothing where the list is not
/// there.
pub(crate) struct AdultSensor {
    pub(crate) any_type: bool,
}

pub(crate) static NEAREST_ADULT: AdultSensor = AdultSensor { any_type: false };
pub(crate) static NEAREST_ADULT_ANY_TYPE: AdultSensor = AdultSensor { any_type: true };

impl Sensor for AdultSensor {
    fn requires(&self) -> &'static [MemoryKind] {
        &[MemoryKind::NearestVisibleAdult, MemoryKind::VisibleMobs]
    }

    fn scan(&self, cx: &mut Cx<'_, '_, '_>) {
        if visible_mobs(cx.memories).is_none() {
            return;
        }
        let own = cx.ai.tick.kind;
        let any_type = self.any_type;
        let adult = find_closest_visible(cx, &mut |cx, looked| {
            let Some(kind) = looked.kind else {
                return false;
            };
            let kind_fits = if any_type {
                entity_in_tag(i32::from(kind), "minecraft:followable_friendly_mobs")
            } else {
                kind == own
            };
            kind_fits && !is_baby(cx.ai, looked)
        });
        match adult {
            Some(adult) => cx
                .memories
                .set(MemoryKind::NearestVisibleAdult, Value::Entity(adult.key)),
            None => cx.memories.erase(MemoryKind::NearestVisibleAdult),
        }
    }
}

/// `TemptingSensor` (`FOOD_TEMPTATIONS` and the frog's and nautilus's, 7.2.5): the
/// nearest player within the tempt range (ignoring line of sight) holding what tempts
/// the mob in either hand, and not riding it. A type's `FOOD_TEMPTATIONS`
/// (`TemptingSensor.forAnimal`, `Animal.isFood`) is a static of its own with its food as
/// `tempts`.
pub(crate) struct TemptingSensor {
    /// What tempts the mob (`Animal.isFood` of its type, or a tag).
    pub(crate) tempts: fn(&Mob, &ItemStack) -> bool,
}

/// `FROG_TEMPTATIONS`: `#frog_food`.
pub(crate) static FROG_TEMPTATIONS: TemptingSensor = TemptingSensor {
    tempts: |_, stack| item_in_tag(stack.item, "minecraft:frog_food"),
};

/// `NAUTILUS_TEMPTATIONS`: `#nautilus_food`.
pub(crate) static NAUTILUS_TEMPTATIONS: TemptingSensor = TemptingSensor {
    tempts: |_, stack| item_in_tag(stack.item, "minecraft:nautilus_food"),
};

impl Sensor for TemptingSensor {
    fn requires(&self) -> &'static [MemoryKind] {
        &[MemoryKind::TemptingPlayer]
    }

    fn scan(&self, cx: &mut Cx<'_, '_, '_>) {
        // The range is the attribute narrowed to a float and widened again.
        let range = f64::from(cx.ai.attribute(attributes::TEMPT_RANGE) as f32);
        let conditions = Conditions::non_combat().ignore_line_of_sight().range(range);
        let own = cx.ai.tick.key;
        let mut tempting = Vec::new();
        for player in sensing::players(cx.ai) {
            if player.spectator || !conditions.test(cx.ai, &player, &|_| true) {
                continue;
            }
            let holds = |stack: Option<&ItemStack>| {
                stack.is_some_and(|stack| stack.count > 0 && (self.tempts)(&cx.ai.tick.mob, stack))
            };
            if !(holds(player.main_hand.as_ref()) || holds(player.off_hand.as_ref())) {
                continue;
            }
            if player.vehicle == Some(own) {
                continue;
            }
            tempting.push(player);
        }
        let tempting = by_distance(cx.ai, tempting);
        match tempting.first() {
            Some(player) => cx
                .memories
                .set(MemoryKind::TemptingPlayer, Value::Entity(player.key)),
            None => cx.memories.erase(MemoryKind::TemptingPlayer),
        }
    }
}

/// `IsInWaterSensor` (`IS_IN_WATER`, 7.2.6).
pub(crate) struct IsInWaterSensor;

pub(crate) static IS_IN_WATER: IsInWaterSensor = IsInWaterSensor;

impl Sensor for IsInWaterSensor {
    fn requires(&self) -> &'static [MemoryKind] {
        &[MemoryKind::IsInWater]
    }

    fn scan(&self, cx: &mut Cx<'_, '_, '_>) {
        if cx.ai.in_water() {
            cx.memories.set(MemoryKind::IsInWater, Value::Unit);
        } else {
            cx.memories.erase(MemoryKind::IsInWater);
        }
    }
}

/// `NearestItemSensor` (`NEAREST_ITEMS`, 7.2.7): of the items in the mob's box grown by
/// 32, 16 and 32, nearest first, the first the mob wants (`wantsToPickUp`), within 32
/// blocks and in its line of sight. **Named**: only the items of the region's land and
/// copies are seen (the plan, section 3 item 1).
pub(crate) struct NearestItemSensor {
    /// `wantsToPickUp(level, stack)`: the type's.
    pub(crate) wants: fn(&Ai<'_, '_>, ItemStack) -> bool,
}

impl Sensor for NearestItemSensor {
    fn requires(&self) -> &'static [MemoryKind] {
        &[MemoryKind::NearestVisibleWantedItem]
    }

    fn scan(&self, cx: &mut Cx<'_, '_, '_>) {
        let bounds = cx.ai.tick.living.body.bounds().inflate([32.0, 16.0, 32.0]);
        let from = cx.ai.position();
        let mut items: Vec<(Looked, ItemStack)> = cx
            .ai
            .tick
            .world
            .entities_in(&bounds)
            .iter()
            .filter_map(|seen| match &seen.entity.data {
                EntityData::Item { stack, .. } => Some((Looked::of_seen(seen), stack.clone())),
                _ => None,
            })
            .collect();
        items.sort_by(|a, b| a.0.distance_sqr(from).total_cmp(&b.0.distance_sqr(from)));
        let mut wanted = None;
        for (item, stack) in items {
            if (self.wants)(cx.ai, stack)
                && item.distance_sqr(from) < 32.0 * 32.0
                && sensing::has_line_of_sight(cx.ai, &item)
            {
                wanted = Some(item.key);
                break;
            }
        }
        match wanted {
            Some(key) => cx
                .memories
                .set(MemoryKind::NearestVisibleWantedItem, Value::Entity(key)),
            None => cx.memories.erase(MemoryKind::NearestVisibleWantedItem),
        }
    }
}

/// `DummySensor` (`DUMMY`, 7.2.8): it senses nothing, but draws its start delay.
pub(crate) struct DummySensor;

pub(crate) static DUMMY: DummySensor = DummySensor;

impl Sensor for DummySensor {
    fn requires(&self) -> &'static [MemoryKind] {
        &[]
    }

    fn scan(&self, _: &mut Cx<'_, '_, '_>) {}
}

/// The distance squared between two block positions (`Vec3i.distSqr`).
pub(crate) fn block_distance_sqr(a: clustine_world::BlockPos, b: clustine_world::BlockPos) -> f64 {
    let (dx, dy, dz) = (
        f64::from(a.x - b.x),
        f64::from(a.y - b.y),
        f64::from(a.z - b.z),
    );
    dx * dx + dy * dy + dz * dz
}

/// `closerThan(entity, distance)`: the feet nearer than `distance`.
pub(crate) fn closer_than(ai: &Ai<'_, '_>, looked: &Looked, distance: f64) -> bool {
    physics::distance_sqr(ai.position(), looked.position) < distance * distance
}

/// `MobSensor(scanRate, mobTest, readyTest, toSet, ttl)` (the armadillo's
/// `ARMADILLO_SCARE_DETECTED`, the brief's 7.2 and 7.8): not ready, the memory is erased;
/// ready, it is set with its expiry where a thing of this tick's `mobs` passes the test.
/// It requires `mobs` alone, as the game's (the memory it sets is registered by what
/// reads it). What it writes the region keeps in the record, so it senses nothing again
/// after a take-in (the plan, section 8 item 1).
pub(crate) struct MobSensor {
    pub(crate) rate: u32,
    pub(crate) mob_test: fn(&Cx<'_, '_, '_>, &Looked) -> bool,
    pub(crate) ready: fn(&Cx<'_, '_, '_>) -> bool,
    pub(crate) to_set: MemoryKind,
    pub(crate) ttl: i64,
}

impl Sensor for MobSensor {
    fn rate(&self) -> u32 {
        self.rate
    }

    fn requires(&self) -> &'static [MemoryKind] {
        &[MemoryKind::Mobs]
    }

    fn scan(&self, cx: &mut Cx<'_, '_, '_>) {
        if !(self.ready)(cx) {
            cx.memories.erase(self.to_set);
            return;
        }
        let Some(Value::Entities(list)) = cx.memories.get(MemoryKind::Mobs) else {
            return;
        };
        let detected = list
            .into_iter()
            .filter_map(|key| find(cx.ai, key))
            .any(|looked| (self.mob_test)(cx, &looked));
        if detected {
            cx.memories
                .set_with_expiry(self.to_set, Value::Bool(true), self.ttl);
        }
    }

    fn resense(&self, _: &mut Cx<'_, '_, '_>) {}
}

/// `VillagerHostilesSensor.ACCEPTABLE_DISTANCE_FROM_HOSTILES`: the types a villager
/// fears, each with how near it must be.
const VILLAGER_HOSTILES_DISTANCES: [(i32, f32); 11] = [
    (entity_types::DROWNED, 8.0),
    (entity_types::EVOKER, 12.0),
    (entity_types::HUSK, 8.0),
    (entity_types::ILLUSIONER, 12.0),
    (entity_types::PILLAGER, 15.0),
    (entity_types::RAVAGER, 12.0),
    (entity_types::VEX, 8.0),
    (entity_types::VINDICATOR, 10.0),
    (entity_types::ZOGLIN, 10.0),
    (entity_types::ZOMBIE, 8.0),
    (entity_types::ZOMBIE_VILLAGER, 8.0),
];

/// `VillagerHostilesSensor` (`VILLAGER_HOSTILES`, a `NearestVisibleLivingEntitySensor`):
/// of `visible_mobs`, the closest seen of a type the villager fears within that type's
/// distance (`distanceToSqr <= d * d`, the square a float's), as `nearest_hostile`, or
/// none. What it writes the region keeps beside the record and senses again after a
/// take-in.
pub(crate) struct VillagerHostilesSensor;

pub(crate) static VILLAGER_HOSTILES: VillagerHostilesSensor = VillagerHostilesSensor;

impl Sensor for VillagerHostilesSensor {
    fn requires(&self) -> &'static [MemoryKind] {
        &[MemoryKind::NearestHostile, MemoryKind::VisibleMobs]
    }

    fn scan(&self, cx: &mut Cx<'_, '_, '_>) {
        if visible_mobs(cx.memories).is_none() {
            // `getVisibleEntities` empty: `setMemory(…, Optional.empty())`.
            cx.memories.erase(MemoryKind::NearestHostile);
            return;
        }
        let from = cx.ai.position();
        let hostile = find_closest_visible(cx, &mut |_, looked| {
            let Some(kind) = looked.kind else {
                return false;
            };
            VILLAGER_HOSTILES_DISTANCES
                .iter()
                .find(|(hostile, _)| *hostile == i32::from(kind))
                .is_some_and(|(_, near)| looked.distance_sqr(from) <= f64::from(near * near))
        });
        match hostile {
            Some(looked) => cx
                .memories
                .set(MemoryKind::NearestHostile, Value::Entity(looked.key)),
            None => cx.memories.erase(MemoryKind::NearestHostile),
        }
    }
}

/// `NearestBedSensor` (`NEAREST_BED`, a scan every 20 ticks) without points of
/// interest: a baby's scan draws `nextInt(20)` of the level's random for its next
/// search (`lastUpdate`), finds no bed and so writes nothing; an adult's does nothing.
/// The one sensor whose scan draws; its sensing again after a take-in draws nothing.
pub(crate) struct NearestBedSensor;

pub(crate) static NEAREST_BED: NearestBedSensor = NearestBedSensor;

impl Sensor for NearestBedSensor {
    fn requires(&self) -> &'static [MemoryKind] {
        &[MemoryKind::NearestBed]
    }

    fn scan(&self, cx: &mut Cx<'_, '_, '_>) {
        let tick = &*cx.ai.tick;
        if tick.mob_type.is_baby(&tick.mob) {
            cx.ai.tick.draws.next_int(20);
        }
    }

    fn resense(&self, _: &mut Cx<'_, '_, '_>) {}
}

/// A sensor whose scan finds nothing while there are no points of interest, nor
/// anything that reads what it would write (`VILLAGER_BABIES`, `SECONDARY_POIS`,
/// `GOLEM_DETECTED`): its memories registered and its start delay drawn at its rate, as
/// the game's.
pub(crate) struct InertSensor {
    pub(crate) rate: u32,
    pub(crate) requires: &'static [MemoryKind],
}

impl Sensor for InertSensor {
    fn rate(&self) -> u32 {
        self.rate
    }

    fn requires(&self) -> &'static [MemoryKind] {
        self.requires
    }

    fn scan(&self, _: &mut Cx<'_, '_, '_>) {}
}
