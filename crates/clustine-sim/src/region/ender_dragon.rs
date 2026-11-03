//! What the ender dragon (`mobs/types/ender_dragon.rs`, family E9) asks of the region:
//! its fight told of its health and of its death (`end_fight.rs`), its death's global
//! level event and its experience, the sitting flame's cloud of dragon's breath and its
//! end; a hit on one of its parts judged by that part's box and hurt as that part
//! (`docs/groundwork/p-plan.md`, section 19.5 item 10); and the dragon's own part of a
//! crystal's destruction, which the fight asks for.

use clustine_data::entity_types;
use clustine_world::components::{EffectInstance, PotionContents};
use clustine_world::projectiles::Cloud;
use clustine_world::{BlockPos, Double, Entity, EntityData, EntityId, EntityUse, Float, HurtBy};
use uuid::Uuid;

use super::Region;
use crate::api::{GlobalLevelEvent, TickOutput};
use crate::mobs::body::held_effects;
use crate::mobs::body::hurt::{self, Causer, DamageSource, DamageType};
use crate::mobs::types::ender_dragon::{self, DRAGON, DragonLocal, parts};
use crate::physics::aabb::Aabb;

/// How long a mob that hurt a player is remembered (`LivingEntity.setLastHurtByMob`'s 100
/// ticks), from which when it hurt them is told.
const BLAME_TICKS: i64 = 100;

/// `AreaEffectCloud`'s `DEFAULT_WAIT_TIME` and `DEFAULT_REAPPLICATION_DELAY`, which no
/// breath of the dragon's changes.
const WAIT_TIME: i32 = 20;
const REAPPLICATION_DELAY: i32 = 20;

/// The sitting flame's cloud (`DragonSittingFlamingPhase`, 259–354): radius 5, 200 ticks,
/// instant damage (`new MobEffectInstance(INSTANT_DAMAGE)`: no time, level I).
const FLAME_RADIUS: f32 = 5.0;
const FLAME_DURATION: i32 = 200;

/// The fireball's cloud (`DragonFireball.onHit`): radius 3 growing to 7 over its 600
/// ticks, instant damage II for a tick.
const BALL_RADIUS: f32 = 3.0;
const BALL_DURATION: i32 = 600;

/// `setPotionDurationScale(0.25f)` of both.
const BREATH_DURATION_SCALE: f32 = 0.25;

/// The level event of the fireball's burst (`levelEvent(2006, pos, 1)`).
const BALL_BURST: i32 = 2006;

/// The breath's potion: instant damage of `amplifier` for `duration`, as a custom
/// effect of no potion (`addEffect`).
fn breath(amplifier: i32, duration: i32) -> PotionContents {
    PotionContents {
        potion: None,
        custom_colour: None,
        custom_effects: vec![EffectInstance {
            effect: "minecraft:instant_damage".to_owned(),
            amplifier,
            duration,
            ambient: false,
            visible: true,
            show_icon: true,
        }],
        custom_name: None,
    }
}

/// Whether `cloud` is one of the dragon's breath (its flame's or its fireball's), which
/// shows the particle `dragon_breath` (`setCustomParticle`): a potion of no name whose
/// only effect is instant damage, which nothing else makes.
pub fn is_breath(cloud: &Cloud) -> bool {
    cloud.potion.potion.is_none()
        && cloud.potion.custom_effects.len() == 1
        && cloud.potion.custom_effects[0].effect == "minecraft:instant_damage"
}

/// Whether `cloud` is a sitting flame's: its making's radius that never changes and its
/// 200 ticks.
fn is_flame(cloud: &Cloud) -> bool {
    is_breath(cloud)
        && cloud.duration == FLAME_DURATION
        && cloud.radius_per_tick.0 == 0.0
        && cloud.radius.0 == FLAME_RADIUS
}

impl Region {
    /// What the dragon `key`'s tick or hurt asked beyond the shared writes.
    pub(crate) fn dragon_local(
        &mut self,
        key: EntityId,
        local: DragonLocal,
        output: &mut TickOutput,
    ) {
        match local {
            DragonLocal::FightUpdated { health, max } => {
                self.fight_dragon_updated(key, health, max);
            }
            DragonLocal::FightKilled => self.fight_dragon_killed(key, output),
            DragonLocal::GlobalEvent { event, pos } => {
                let told = GlobalLevelEvent {
                    event,
                    dimension: self.config.dimension,
                    position: pos,
                    data: 0,
                };
                self.make_global_event(told, output);
            }
            DragonLocal::Award { at, amount } => {
                self.award_with_direction(at, [0.0; 3], amount, output);
            }
            DragonLocal::BreathCloud { at } => {
                let cloud = Cloud {
                    duration: FLAME_DURATION,
                    wait_time: WAIT_TIME,
                    reapplication_delay: REAPPLICATION_DELAY,
                    radius: Float(FLAME_RADIUS),
                    radius_at: 0,
                    radius_on_use: Float(0.0),
                    radius_per_tick: Float(0.0),
                    duration_on_use: 0,
                    potion: breath(0, 0),
                    duration_scale: Float(BREATH_DURATION_SCALE),
                    owner: Some(key),
                    victims: Vec::new(),
                };
                self.make_cloud(at, cloud, output);
            }
            DragonLocal::DiscardFlame => self.discard_flames(key, output),
            DragonLocal::PlayerHit {
                player,
                from,
                push,
                amount,
                fresh,
            } => self.dragon_hits_player(key, player, from, (push, amount, fresh), output),
        }
    }

    /// The dragon `key`'s wings or head on the player whose stay is `player`
    /// (`EnderDragon.knockBack` 1425–1533, `hurt` 1534–1570): the region's own pushed
    /// by `push` (`Entity.push`, the motion the server keeps for them, which a hurt's
    /// `markHurt` tells their client), then hurt by `amount` of `mobAttack(dragon)` from
    /// `from`, where `fresh` only if no mob hurt them in their last two ticks; a player
    /// of another region hurt by their holder ([`Region::strike`]).
    fn dragon_hits_player(
        &mut self,
        key: EntityId,
        player: EntityId,
        from: [f64; 3],
        (push, amount, fresh): (Option<[f64; 3]>, Option<f32>, bool),
        output: &mut TickOutput,
    ) {
        let own = self.player_with_entity(player);
        if let (Some(id), Some(push)) = (own, push)
            && push.iter().all(|component| component.is_finite())
            && let Some(state) = self.players.get_mut(&id)
        {
            state.vitals.motion = crate::physics::add(state.vitals.motion, push);
            self.journal.players.insert(id);
        }
        let Some(amount) = amount else {
            return;
        };
        // `getLastHurtByMobTimestamp() < tickCount − 2`: the hurt that set who hurt them
        // last was at `until − 100`.
        let tick = i64::try_from(self.tick).unwrap_or(i64::MAX);
        let hurt_lately = own
            .and_then(|id| self.players.get(&id))
            .and_then(|state| state.vitals.last_hurt_by.as_ref())
            .is_some_and(|by| by.until - BLAME_TICKS >= tick - 2);
        if fresh && hurt_lately {
            return;
        }
        let dragon = clustine_world::Causer {
            key,
            kind: DRAGON,
            position: from.map(Double),
            player: None,
            living: true,
            name: None,
        };
        let hurt = HurtBy {
            damage_type: "minecraft:mob_attack".to_owned(),
            cause: Some(dragon.clone()),
            direct: Some(dragon),
            position: None,
            projectile_motion: None,
            amount: Float(amount),
            knockback: None,
            fire_before: 0,
            fire_after: 0,
            effects: Vec::new(),
            arrow: false,
            extras: Vec::new(),
            sound: None,
            hit: None,
            weapon: None,
        };
        self.strike(player, Box::new(hurt), output);
    }

    /// The sitting flames of the dragon `owner` the region holds, discarded.
    fn discard_flames(&mut self, owner: EntityId, output: &mut TickOutput) {
        let flames: Vec<(EntityId, u32, clustine_world::ChunkPos)> = self
            .chunks
            .iter()
            .flat_map(|(chunk, loaded)| {
                loaded
                    .entities()
                    .iter()
                    .filter_map(move |entity| match &entity.data {
                        EntityData::Cloud(cloud)
                            if cloud.owner == Some(owner) && is_flame(cloud) =>
                        {
                            Some((entity.key, entity.hops, *chunk))
                        }
                        _ => None,
                    })
            })
            .collect();
        for (key, hops, chunk) in flames {
            self.remove_entity(key, hops, chunk, output);
        }
    }

    /// `DragonFireball.onHit`'s burst (d DragonFireball.txt 30–300) of the fireball
    /// `fireball`, owned by `owner`: a cloud of breath where it is, or at the first living
    /// thing within 4 of it in its box grown by 4 across and 2 up and down; level event
    /// 2006 where it burst. The caller discards it.
    pub(crate) fn dragon_fireball_bursts(
        &mut self,
        fireball: &Entity,
        owner: Option<EntityId>,
        output: &mut TickOutput,
    ) {
        let info = clustine_data::EntityTypeInfo::of(i32::from(fireball.kind));
        let bounds =
            Aabb::of_entity(fireball.position, info.width, info.height).inflate([4.0, 2.0, 4.0]);
        let at = self
            .living_in(&bounds, fireball.key)
            .into_iter()
            .map(|living| living.position)
            .find(|position| crate::physics::distance_sqr(*position, fireball.position) < 16.0)
            .unwrap_or(fireball.position);
        let cloud = Cloud {
            duration: BALL_DURATION,
            wait_time: WAIT_TIME,
            reapplication_delay: REAPPLICATION_DELAY,
            radius: Float(BALL_RADIUS),
            radius_at: 0,
            radius_on_use: Float(0.0),
            radius_per_tick: Float((7.0 - BALL_RADIUS) / BALL_DURATION as f32),
            duration_on_use: 0,
            potion: breath(1, 1),
            duration_scale: Float(BREATH_DURATION_SCALE),
            owner,
            victims: Vec::new(),
        };
        output.events.push(crate::api::RegionEvent::LevelEvent {
            kind: BALL_BURST,
            position: crate::physics::containing(fireball.position),
            data: 1,
        });
        self.make_cloud(at, cloud, output);
    }

    /// The box a use `by` of `entity` is judged by (`ServerGamePacketListenerImpl`'s
    /// reach, of what the client hit): the part's where a hit names a part of a dragon,
    /// else the entity's own.
    pub(crate) fn used_box(&self, entity: &Entity, by: &EntityUse) -> Aabb {
        if let EntityUse::Attack {
            part: Some(part), ..
        } = by
            && let Some(boxes) = ender_dragon::part_boxes(entity)
        {
            return boxes[usize::from((*part).min(parts::COUNT - 1))];
        }
        self.entity_box(entity)
    }

    /// `hurtServer(source, amount)` of the region's mob `key` between its ticks, at its
    /// part `part` where it is a dragon and the hit named one (`EnderDragonPart
    /// .hurtServer` → `EnderDragon.hurt(part, …)`), else as [`Region::hurt_mob`]. Whether
    /// it took the hurt.
    pub(crate) fn hurt_mob_at(
        &mut self,
        key: EntityId,
        part: Option<u8>,
        source: &DamageSource,
        amount: f32,
        output: &mut TickOutput,
    ) -> bool {
        let dragon = self
            .find_entity(key)
            .is_some_and(|(_, record)| record.kind == DRAGON);
        let Some(part) = part.filter(|_| dragon) else {
            return self.hurt_mob(key, source, amount, output);
        };
        let hurt = self.struck_between_ticks(key, output, |struck| {
            ender_dragon::hit_part(struck.mob, part);
            let took = hurt::hurt_server(struck, source, amount);
            took.then(|| (struck.position, held_effects(&struck.mob.body.effects)))
        });
        let Some(Some((at, held))) = hurt else {
            return false;
        };
        self.effects_at_hurt(key, at, &held, output);
        true
    }

    /// `EnderDragon.onCrystalDestroyed(level, crystal, pos, source)` (2854–2906) of the
    /// dragon `dragon`, which the fight asks for where the crystal `crystal` at `pos` was
    /// destroyed by what `player` names (`source.getEntity()`, a player's UUID and key):
    /// where it was the dragon's nearest crystal, 10 of `explosion(crystal, player)` to
    /// its head. **Named**: the holding pattern's strafe at that player
    /// (`DragonHoldingPatternPhase.onCrystalDestroyed`, where `canAttack` lets it: one
    /// who is not creative), or at the nearest player within 64 of the crystal where no
    /// player destroyed it, is not made: the dragon flies on in its pattern, and strafes
    /// a player only as its own phases choose one.
    pub(crate) fn dragon_crystal_destroyed(
        &mut self,
        dragon: EntityId,
        crystal: EntityId,
        pos: BlockPos,
        player: Option<(EntityId, Uuid)>,
        output: &mut TickOutput,
    ) {
        let Some((_, record)) = self.find_entity(dragon) else {
            return;
        };
        let Ok(mob) = crate::mobs::Mob::decode(&record.data) else {
            return;
        };
        if record.kind != DRAGON || ender_dragon::state_of(&mob).nearest_crystal != Some(crystal) {
            return;
        }
        let at = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        ];
        let direct = Causer {
            name: None,
            key: crystal,
            kind: entity_types::END_CRYSTAL as u16,
            position: at,
            player: None,
            living: false,
        };
        let cause = player.map(|(key, uuid)| Causer {
            name: None,
            key,
            kind: super::mob_bodies::player_kind(),
            position: at,
            player: Some(uuid.as_u128()),
            living: true,
        });
        let kind = if cause.is_some() {
            "minecraft:player_explosion"
        } else {
            "minecraft:explosion"
        };
        let source = DamageSource {
            kind: DamageType::named(kind).unwrap_or(DamageType::MAGIC),
            cause,
            direct: Some(direct),
            position: None,
            projectile_motion: None,
            clean_water: false,
        };
        self.hurt_mob_at(dragon, Some(parts::HEAD), &source, 10.0, output);
    }
}

#[cfg(test)]
#[path = "ender_dragon_tests.rs"]
mod tests;
