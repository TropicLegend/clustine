//! The area effect cloud: its shrinking and its effects (`docs/groundwork/n9-plan.md`,
//! section 4.5; `AreaEffectCloud.serverTick`, n9 AreaEffectCloud 453–704, as D9 2.5 reads
//! it). A lingering potion makes one (`thrown.rs`).
//!
//! Its age is its `lived` (the game's `tickCount` in a tick is one more). Past its
//! duration after its wait it is gone; while it waits it does nothing; after, its radius
//! shrinks by its radius per tick each tick, and it is gone below half a block. Every
//! fifth tick it forgets the victims whose time has come and gives its effects, scaled by
//! its duration scale, to each living entity in its box within its radius across
//! (instant ones at half strength), each then a victim until its reapplication delay
//! has passed; each use takes its radius on use off the radius (and its duration on use
//! off the duration). No draws.
//!
//! The radius is derived (finding 53): the record holds it as of `radius_at`, the age
//! of its last use or its making, and each tick works it out by the game's float
//! additions of the ticks since that did not wait; so a cloud commits only when it is
//! used or forgets a victim, not every tick of its thirty seconds.
//!
//! Its targets are the hit test's (`projectiles.rs`' `targets`): the region's mobs and
//! players, those of its copies (their holders given `EntityUse::Effects`) and other
//! regions' players by their stay; its victims count them all, as in one region.
//!
//! Named differences: only mobs and players are living entities to it (an armour stand
//! is never affected anyway); `noneMatch(target::canBeAffected)` is taken as the game's
//! refusals by tag (`mobs/body/effects.rs`' `can_be_affected`, which is not this part's
//! to share: the same three tags are read here); a custom particle is not kept.

use clustine_blocks::data::entity_in_tag;
use clustine_world::components::EffectInstance;
use clustine_world::{ChunkPos, Entity, EntityData, Float, GivenEffects, projectiles::Cloud};

use super::Region;
use super::thrown::{Living, all_effects, has_effects, scaled};
use crate::api::TickOutput;
use crate::physics::aabb::Aabb;

/// `TIME_BETWEEN_APPLICATIONS`.
const BETWEEN_USES: u32 = 5;

/// `MIN_RADIUS`, below which it is gone, and `MAX_RADIUS`, which `setRadius` keeps it
/// within with nothing.
const MIN_RADIUS: f32 = 0.5;
const MAX_RADIUS: f32 = 32.0;

/// `EntityDimensions.scalable(radius · 2, 0.5)`: its height.
const HEIGHT: f32 = 0.5;

/// The cloud's radius at the age `age` (the game's `tickCount` of a tick), from what
/// the record holds: each tick after `radius_at` that did not wait adds the radius per
/// tick, the radius kept within nothing and 32 as `setRadius` keeps it; `None` where a
/// tick took it below half a block, which ends it.
pub(crate) fn radius_at(cloud: &Cloud, age: u32) -> Option<f32> {
    let mut radius = cloud.radius.0;
    let per_tick = cloud.radius_per_tick.0;
    if per_tick == 0.0 {
        return Some(radius);
    }
    let wait = u32::try_from(cloud.wait_time.max(0)).unwrap_or(0);
    let first = cloud.radius_at.saturating_add(1).max(wait);
    for _ in first..=age {
        radius += per_tick;
        if radius < MIN_RADIUS {
            return None;
        }
        radius = radius.clamp(0.0, MAX_RADIUS);
    }
    Some(radius)
}

/// `canBeAffected(instance)` of a living entity of the type `kind` (none for a
/// player): the tags' refusals.
fn can_be_affected(kind: Option<u16>, effect: &str) -> bool {
    let Some(kind) = kind.map(i32::from) else {
        return true;
    };
    if entity_in_tag(kind, "minecraft:immune_to_infested") {
        return effect != "minecraft:infested";
    }
    if entity_in_tag(kind, "minecraft:immune_to_oozing") {
        return effect != "minecraft:oozing";
    }
    if entity_in_tag(kind, "minecraft:ignores_poison_and_regen") {
        return effect != "minecraft:regeneration" && effect != "minecraft:poison";
    }
    true
}

impl Region {
    /// The tick of the cloud `entity` of the loaded chunk `position`
    /// (`AreaEffectCloud.tick`); it commits through `write_projectile`.
    pub(crate) fn tick_cloud(
        &mut self,
        entity: Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let EntityData::Cloud(ref cloud) = entity.data else {
            return;
        };
        let mut cloud = (**cloud).clone();
        let key = entity.key;
        let age = entity.lived.saturating_add(1);
        let gone = |region: &mut Region, output: &mut TickOutput| {
            region.remove_entity(key, entity.hops, position, output);
        };
        // Past its duration after its wait (an infinite one, -1, never).
        let since = i64::from(age) - i64::from(cloud.wait_time);
        if cloud.duration != -1 && since >= i64::from(cloud.duration) {
            gone(self, output);
            return;
        }
        let waiting = i64::from(age) < i64::from(cloud.wait_time);
        if waiting {
            self.age_cloud(&entity, cloud, position, output);
            return;
        }
        let Some(mut radius) = radius_at(&cloud, age) else {
            gone(self, output);
            return;
        };
        if !age.is_multiple_of(BETWEEN_USES) {
            self.age_cloud(&entity, cloud, position, output);
            return;
        }
        cloud.victims.retain(|&(_, until)| until > age);
        if !has_effects(&cloud.potion) {
            cloud.victims.clear();
            self.age_cloud(&entity, cloud, position, output);
            return;
        }
        let effects: Vec<EffectInstance> = all_effects(&cloud.potion)
            .iter()
            .map(|instance| scaled(instance, cloud.duration_scale.0))
            .collect();
        let width = radius * 2.0;
        let bounds = Aabb::of_entity(entity.position, width, HEIGHT);
        let source = clustine_world::Causer {
            key,
            kind: entity.kind,
            position: entity.position.map(clustine_world::Double),
            player: None,
            living: false,
        };
        let owner = cloud.owner.and_then(|owner| self.causer_of(owner));
        let mut used = false;
        for living in self.living_in(&bounds, key) {
            let target = living.target.key;
            if cloud.victims.iter().any(|&(victim, _)| victim == target) {
                continue;
            }
            // `isAffectedByPotions`, and some effect it can be given.
            if !living.target.alive
                || !effects
                    .iter()
                    .any(|instance| can_be_affected(living.kind, &instance.effect))
            {
                continue;
            }
            let dx = living.position[0] - entity.position[0];
            let dz = living.position[2] - entity.position[2];
            if dx * dx + dz * dz > f64::from(radius * radius) {
                continue;
            }
            let delay = u32::try_from(cloud.reapplication_delay.max(0)).unwrap_or(0);
            let until = age.saturating_add(delay);
            match cloud
                .victims
                .binary_search_by_key(&target, |&(victim, _)| victim)
            {
                Ok(at) => cloud.victims[at].1 = until,
                Err(at) => cloud.victims.insert(at, (target, until)),
            }
            self.give_cloud_effects(&living, &effects, &source, owner.as_ref(), output);
            used = true;
            if cloud.radius_on_use.0 != 0.0 {
                radius += cloud.radius_on_use.0;
                if radius < MIN_RADIUS {
                    gone(self, output);
                    return;
                }
                radius = radius.clamp(0.0, MAX_RADIUS);
            }
            if cloud.duration_on_use != 0 && cloud.duration != -1 {
                cloud.duration = cloud.duration.saturating_add(cloud.duration_on_use);
                if cloud.duration <= 0 {
                    gone(self, output);
                    return;
                }
            }
        }
        if used {
            cloud.radius = Float(radius);
            cloud.radius_at = age;
        }
        self.age_cloud(&entity, cloud, position, output);
    }

    /// The cloud's effects `effects` given to `living`: each as a copy of the cloud's
    /// (`new MobEffectInstance(instance)`), the instant ones at half strength.
    fn give_cloud_effects(
        &mut self,
        living: &Living,
        effects: &[EffectInstance],
        source: &clustine_world::Causer,
        owner: Option<&clustine_world::Causer>,
        output: &mut TickOutput,
    ) {
        let given = GivenEffects {
            instances: effects.to_vec(),
            instant_factor: Float(0.5),
            source: Some(source.clone()),
            owner: owner.cloned(),
            water: false,
        };
        self.give_effects(living, given, output);
    }

    /// The cloud `entity` a tick older, with `cloud` as its tick left it: committed only
    /// where that changed.
    fn age_cloud(
        &mut self,
        entity: &Entity,
        cloud: Cloud,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let after = Entity {
            lived: entity.lived.saturating_add(1),
            data: EntityData::Cloud(Box::new(cloud)),
            ..entity.clone()
        };
        self.write_projectile(entity, after, position, output);
    }
}
