//! Fireballs, large and small (`AbstractHurtingProjectile`, `Fireball`, `LargeFireball`,
//! `SmallFireball`; d javap; family E2 of P12, `docs/groundwork/p-plan.md`, sections
//! 18.3, 18.5 and 18.6): the ghast's and the blaze's.
//!
//! **The flight** (`AbstractHurtingProjectile.tick`, 0–152): its motion grows along
//! itself by its acceleration power and is kept by 0.95, by 0.8 in water
//! (`applyInertia`); the hit test along that motion (`ProjectileUtil
//! .getHitResultOnMoveVector`, `COLLIDER`), as the thrown items find hits; its turn a
//! fifth of the way towards its motion (`rotateTowardsMovement(0.2)`); `Projectile.tick`
//! (its first tick's `projectile_shoot`, whether it has left its owner, the fluids, the
//! floor of the world); it burns for a second, renewed every tick (`shouldBurn`); then
//! the hit. The trail's smoke and the bubbles in water are the client's: the server's
//! `addParticle` sends nothing.
//!
//! **Across a line** it goes as a thrown item does (`write_projectile`, `relocate`): its
//! owner by key, type and a player's UUID ([`ProjectileOwner`]), so that the region that
//! takes it still names who shot it. It is discarded where its owner is a mob of the
//! region's own that is dying (the game's `owner.isRemoved()`), and where its move would
//! take it into land no region is known to hold: neither loaded here, nor copied, nor
//! another region's (`!level.hasChunkAt`), so that one that misses does not load land
//! as it goes.
//!
//! **Deflection**: a player's hit on a large fireball, here or from across a line
//! (`EntityUse::Attack`; `Player.deflectProjectile`, n9 Player.txt 2390–2440), sends it
//! along their look (`ProjectileDeflection.AIM_DEFLECT`), owned by them with their UUID,
//! its power back to 0.1 (`onDeflection(true)`), its motion sent at once (`needsSync`),
//! and the attack's no-damage sound. Another projectile's hit on it does the same by
//! that projectile's owner (`Projectile.onRedirectProjectile`); an explosion makes the
//! explosion's cause its owner (`ServerExplosion.hurtEntities`, n6 425–463). A small
//! fireball is none of `#redirectable_projectile`: nothing picks it, and nothing sends
//! it back.
//!
//! **Impact**: the large fireball hurts what it hits by `fireball` 6 (`unattributed_
//! fireball` without an owner), here or across a line, then explodes at its power with
//! fire by `mob_griefing` (`Level.explode(…, MOB)`, its drops decaying) and is gone; the
//! small one sets what it hits alight for five seconds and hurts it by 5, taking the
//! fire back where the hurt is refused, or sets fire beside the block it hits (where its
//! owner is no mob, or `mob_griefing` allows), and is gone. Both burn, so the block hit
//! takes them as a burning projectile: TNT is primed (`projectile_hits.rs`) and an unlit
//! campfire lit.
//!
//! Named differences:
//! - An owner removed otherwise than by its death here (despawned, gone across a line
//!   and removed there) is not noticed: the fireball flies on until it hits or reaches
//!   land nobody holds. One whose owner dies here is gone at once, where the game's
//!   waits for the owner's removal twenty ticks after its death.
//! - Land a region holds that this one has never heard of counts as nobody's.
//! - The blocks it passes through (`applyEffectsFromBlocks`) and the world border are
//!   left out, as for the thrown items; `canHitEntity`'s `noPhysics` is not read (no
//!   entity of Clustine's is without physics but a vex in its tick).
//! - An explosion names only a player as its cause, so one of a mob's makes the
//!   fireball it pushes no one's (the game's names the mob); a projectile whose owner
//!   the region cannot see turns the fireball without changing its course (the game's
//!   `getOwner()` is null then too).
//! - The small fireball's fire on what is not a mob (a boat, a frame) is not set: the
//!   hurt of such a thing carries no fire.

use clustine_blocks::data::{flag, in_tag, set_flag};
use clustine_data::{BlockState, EntityTypeInfo, entity_types, items, sounds};
use clustine_world::{
    ChunkPos, Double, Entity, EntityData, EntityField, EntityId, EntityUse, Float, HurtBy,
    ItemStack, PlayerId,
    projectiles::{Fireball, ProjectileOwner},
};
use uuid::Uuid;

use super::entities::insecure_uuid;
use super::explosions::Explosion;
use super::items::{CartMemory, Memory, View, tracked};
use super::projectiles::{TargetKind, UUID_DRAWS};
use super::thrown::{Flying, Hit, Struck};
use super::updates::{RegionView, UPDATE_ALL, UPDATE_LIMIT};
use super::vibrations::{Context, Source, Vibration};
use super::{Knowledge, Region};
use crate::api::{RegionEvent, SOUND_SOURCE_PLAYERS, TickOutput, Tracked};
use crate::mobs::types::ghast::view_vector;
use crate::physics::aabb::Aabb;
use crate::physics::thrown;
use crate::physics::{self, Vec3, fluids};

/// `AbstractHurtingProjectile.INITAL_ACCELERATION_POWER`.
pub(crate) const INITIAL_ACCELERATION_POWER: f64 = 0.1;

/// `getInertia()` and `getLiquidInertia()`.
const INERTIA: f32 = 0.95;
const LIQUID_INERTIA: f32 = 0.8;

/// `LargeFireball.onHitEntity`'s hurt, and `SmallFireball.onHitEntity`'s with its
/// `igniteForSeconds(5.0f)`.
const LARGE_DAMAGE: f32 = 6.0;
const SMALL_DAMAGE: f32 = 5.0;
const SMALL_FIRE_TICKS: i32 = 100;

/// `shouldBurn()`'s `igniteForSeconds(1.0f)`, renewed every tick.
const BURNING_TICKS: i32 = 20;

/// `mob_griefing`, on everywhere in Clustine: the large fireball's explosion breaks
/// blocks and sets fire, and a mob's small fireball sets fire.
const MOB_GRIEFING: bool = true;

/// `Block.UPDATE_IMMEDIATE`, of a campfire lit by a burning projectile
/// (`setBlock(pos, lit, 11)`).
const UPDATE_IMMEDIATE: u32 = 8;

/// `Mth.RAD_TO_DEG` widened, as `rotateTowardsMovement` multiplies by it.
const DEGREES: f64 = 57.295_776_367_187_5;

/// Whether a fireball of the entity type `kind` is of `#redirectable_projectile`, which
/// a hit sends back and the hit test can pick (`Projectile.isPickable`): the large one.
pub(crate) fn redirectable(kind: u16) -> bool {
    i32::from(kind) == entity_types::FIREBALL
}

/// `ProjectileUtil.rotateTowardsMovement(entity, 0.2f)` of a fireball that faced `yaw`
/// and `pitch` as its tick began (`yRotO`, `xRotO`) and moves by `motion`: its pitch and
/// yaw a fifth of the way from there towards its motion's, the old ones brought within
/// half a turn first (n7k ProjectileUtil.txt 587–721).
pub(crate) fn rotate_towards_movement(yaw: f32, pitch: f32, motion: Vec3) -> [f32; 2] {
    let [x, y, z] = motion;
    if x * x + y * y + z * z == 0.0 {
        return [yaw, pitch];
    }
    let horizontal = (x * x + z * z).sqrt();
    let to_yaw = (crate::mobs::ai::maths::atan2(z, x) * DEGREES) as f32 + 90.0;
    let to_pitch = (crate::mobs::ai::maths::atan2(horizontal, y) * DEGREES) as f32 - 90.0;
    let near = |mut old: f32, to: f32| {
        while to - old < -180.0 {
            old -= 360.0;
        }
        while to - old >= 180.0 {
            old += 360.0;
        }
        old
    };
    let pitch = near(pitch, to_pitch);
    let yaw = near(yaw, to_yaw);
    // `Mth.lerp(delta, start, end)`.
    [yaw + 0.2 * (to_yaw - yaw), pitch + 0.2 * (to_pitch - pitch)]
}

/// `applyInertia()`'s motion: grown along itself by `power` (`Vec3.normalize`, nothing
/// for a motion shorter than 1e-5), then kept by the air's or the water's inertia.
pub(crate) fn inertia(motion: Vec3, power: f64, in_water: bool) -> Vec3 {
    let kept = if in_water { LIQUID_INERTIA } else { INERTIA };
    let grown = physics::add(motion, physics::scale(physics::normalize(motion), power));
    physics::scale(grown, f64::from(kept))
}

/// What a fireball's tick reads of its record.
fn fireball_of(entity: &Entity) -> Option<Fireball> {
    match &entity.data {
        EntityData::Fireball(fireball) => Some((**fireball).clone()),
        _ => None,
    }
}

impl Region {
    /// A fireball's tick (`AbstractHurtingProjectile.tick`, the module's note).
    pub(crate) fn tick_fireball(
        &mut self,
        entity: Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let Some(fireball) = fireball_of(&entity) else {
            return;
        };
        let key = entity.key;
        let info = EntityTypeInfo::of(i32::from(entity.kind));
        // `owner != null && owner.isRemoved()`.
        if self.fireball_owner_gone(fireball.owner) {
            self.remove_entity(key, entity.hops, position, output);
            return;
        }
        let in_water = self
            .entity_memory
            .get(&key)
            .is_some_and(|memory| memory.cart.in_water);
        let motion = inertia(entity.motion, fireball.acceleration_power.0, in_water);
        let owner_key = fireball.owner.map(|owner| owner.key);
        let owner_root = owner_key.and_then(|owner| self.unit_root(owner));
        let flying = Flying {
            key,
            width: info.width,
            height: info.height,
            owner_root,
            left_owner: fireball.left_owner,
            tick_count: entity.tick_count().saturating_add(1),
            fall_distance: entity.fall_distance,
        };
        let struck = self.hit_on_move(&flying, position, entity.position, motion);
        let location = struck.map_or_else(
            || physics::add(entity.position, motion),
            |struck| struck.location(),
        );
        // `!level.hasChunkAt(blockPosition())`, asked where its move would leave it.
        if struck.is_none() && !self.land_is_held(ChunkPos::containing(location[0], location[2])) {
            self.remove_entity(key, entity.hops, position, output);
            return;
        }
        let [yaw, pitch] = rotate_towards_movement(entity.yaw, entity.pitch, motion);
        // `Projectile.tick`: the first tick's `projectile_shoot`, then whether it has left
        // its owner, from where it is now.
        if !fireball.has_been_shot {
            self.shot_vibration(owner_key, owner_root, location);
        }
        let left_owner =
            fireball.left_owner || self.has_left_owner(key, owner_root, location, info, motion);
        // `baseTick`: the fluids it is in and their push; its fire burning down (by four
        // for a type immune to fire), put out in water; below the world it is gone.
        let view = View {
            region: self,
            home: position,
        };
        let found = fluids::update(&view, &Aabb::of_entity(location, info.width, info.height));
        let motion = fluids::push(&found, motion);
        if self.below_the_world(position, location) {
            self.remove_entity(key, entity.hops, position, output);
            return;
        }
        let mut fire = entity.fire_ticks;
        if fire > 0 {
            fire -= if info.fire_immune { 4 } else { 1 };
        }
        if found.in_water() {
            fire = 0;
        }
        // `shouldBurn()`: `igniteForSeconds(1)`.
        let fire = fire.max(BURNING_TICKS);
        let after = Entity {
            position: location,
            motion,
            yaw,
            pitch,
            lived: entity.lived.saturating_add(1),
            fire_ticks: fire,
            data: EntityData::Fireball(Box::new(Fireball {
                left_owner,
                has_been_shot: true,
                ..fireball
            })),
            ..entity.clone()
        };
        if let Some(struck) = struck {
            // `isAlive()` and `hitTargetOrDeflectSelf`: no entity of Clustine's deflects
            // a projectile, and a block hit is never on air.
            self.fireball_struck(&after, struck, found.in_water(), output);
            self.remove_entity(key, entity.hops, position, output);
            return;
        }
        self.entity_memory.insert(
            key,
            Memory {
                cart: CartMemory {
                    in_water: found.in_water(),
                    ..CartMemory::default()
                },
                ..Memory::default()
            },
        );
        self.write_projectile(&entity, after, position, output);
        // Its fire is committed where it changed, as the record's other fields are.
        if fire != entity.fire_ticks
            && let Some((chunk, _)) = self.find_entity(key)
        {
            self.commit_entity(chunk, key, vec![EntityField::FireTicks(fire)], output);
        }
    }

    /// Whether the fireball's owner is gone as far as the region can tell: a mob of its
    /// own that is dying.
    fn fireball_owner_gone(&self, owner: Option<ProjectileOwner>) -> bool {
        let Some(owner) = owner else {
            return false;
        };
        let Some((_, record)) = self.find_entity(owner.key) else {
            return false;
        };
        matches!(record.data, EntityData::Mob { .. })
            && crate::mobs::Mob::decode(&record.data).is_ok_and(|mob| mob.body.vitals.dying())
    }

    /// Whether some region is known to hold the chunk `chunk`: this one, which has it
    /// loaded, or another, whose copy it has or whom the store named.
    fn land_is_held(&self, chunk: ChunkPos) -> bool {
        self.chunks.contains_key(&chunk)
            || self.copies.contains_key(&chunk)
            || matches!(self.land.knowledge(chunk), Knowledge::Foreign(_))
    }

    /// `hitTargetOrDeflectSelf`'s `onHit` of the fireball `fireball`, as it is where it
    /// hit: `Projectile.onHit` (its kind's `onHitEntity` or `onHitBlock`, and
    /// `projectile_land`), then the large one's explosion. The caller removes it after.
    fn fireball_struck(
        &mut self,
        fireball: &Entity,
        struck: Struck,
        in_water: bool,
        output: &mut TickOutput,
    ) {
        let Some(data) = fireball_of(fireball) else {
            return;
        };
        let key = fireball.key;
        let large = redirectable(fireball.kind);
        match struck {
            Struck::Entity { target, kind, at } => {
                // `onRedirectProjectile` of a large fireball hit and its `onHitEntity`
                // are both the hurt: a fireball takes it as a redirect
                // (`fireball_hit`), and a face refuses every hurt.
                if kind != TargetKind::Face {
                    let hurt = if large {
                        self.fireball_hurt(fireball, &data, LARGE_DAMAGE, 0)
                    } else {
                        self.fireball_hurt(fireball, &data, SMALL_DAMAGE, SMALL_FIRE_TICKS)
                    };
                    self.hurt_target(target, kind, hurt, output);
                }
                self.vibrate(
                    Vibration::PROJECTILE_LAND,
                    at,
                    Context::of(Some(Source::of(key)), None),
                );
            }
            Struck::Block { hit, face } => {
                let state = self.block(hit.pos);
                if let Some(state) = state {
                    self.projectile_hit(fireball, state, &hit, output);
                    self.burning_projectile_on_campfire(hit.pos, state, fireball, output);
                }
                if !large {
                    self.small_fireball_sets_fire(&data, hit.pos, face, output);
                }
                self.vibrate_at(
                    Vibration::PROJECTILE_LAND,
                    hit.pos,
                    Context::of(Some(Source::of(key)), self.block(hit.pos)),
                );
            }
        }
        if large {
            self.fireball_explodes(fireball, &data, in_water, output);
        }
    }

    /// `damageSources().fireball(fireball, owner)`'s hurt of `amount`, with fire for
    /// `fire_before` ticks set before it and taken back where it is refused: `fireball`
    /// caused by the owner, `unattributed_fireball` where it has none, pushing along its
    /// motion.
    fn fireball_hurt(
        &self,
        fireball: &Entity,
        data: &Fireball,
        amount: f32,
        fire_before: i32,
    ) -> HurtBy {
        let cause = data.owner.map(|owner| clustine_world::Causer {
            key: owner.key,
            kind: owner.kind,
            position: self
                .thrower(owner.key)
                .map_or(fireball.position.map(Double), |thrower| thrower.from),
            player: owner.player,
            living: true,
        });
        let damage_type = if cause.is_some() {
            "minecraft:fireball"
        } else {
            "minecraft:unattributed_fireball"
        };
        HurtBy {
            damage_type: damage_type.to_owned(),
            cause,
            direct: Some(clustine_world::Causer {
                key: fireball.key,
                kind: fireball.kind,
                position: fireball.position.map(Double),
                player: None,
                living: false,
            }),
            position: None,
            projectile_motion: Some(fireball.motion.map(Double)),
            amount: Float(amount),
            knockback: None,
            fire_before,
            fire_after: 0,
            effects: Vec::new(),
            arrow: false,
            extras: Vec::new(),
            sound: None,
            hit: None,
        }
    }

    /// `CampfireBlock.onProjectileHit` (n7 CampfireBlock.txt 459–501): a burning
    /// projectile lights an unlit campfire out of water (`setBlock(pos, lit, 11)`). Where
    /// the region cannot set it, it is set by the effect of a set, as any rule's.
    fn burning_projectile_on_campfire(
        &mut self,
        pos: clustine_world::BlockPos,
        state: BlockState,
        projectile: &Entity,
        output: &mut TickOutput,
    ) {
        if projectile.fire_ticks <= 0
            || !in_tag(state, "minecraft:campfires")
            || flag(state, "lit")
            || flag(state, "waterlogged")
        {
            return;
        }
        self.set_block_updated(
            pos,
            set_flag(state, "lit", true),
            UPDATE_ALL | UPDATE_IMMEDIATE,
            UPDATE_LIMIT,
            output,
        );
    }

    /// `SmallFireball.onHitBlock`'s own part (d SmallFireball.txt): where its owner is no
    /// mob, or `mob_griefing` allows, fire (`BaseFireBlock.getState`) in the empty block
    /// beside the side hit, set with every update; in another region's land, by the
    /// effect of a set.
    fn small_fireball_sets_fire(
        &mut self,
        data: &Fireball,
        pos: clustine_world::BlockPos,
        face: clustine_data::Face,
        output: &mut TickOutput,
    ) {
        let by_mob = data
            .owner
            .is_some_and(|owner| owner.player.is_none() && self.owner_is_mob(owner.key));
        if by_mob && !MOB_GRIEFING {
            return;
        }
        let [x, y, z] = thrown::unit_of(face).map(|value| value as i32);
        let beside = pos.offset(x, y, z);
        if !self
            .block(beside)
            .is_some_and(|state| state.props().is_air())
        {
            return;
        }
        let fire = clustine_blocks::shape::fire_state(&mut RegionView(self), beside);
        self.set_block_updated(beside, fire, UPDATE_ALL, UPDATE_LIMIT, output);
    }

    /// Whether the entity `key` is a mob as the region sees it, here or in a copy; an
    /// owner the region cannot see is taken for one, as only mobs shoot fireballs.
    fn owner_is_mob(&self, key: EntityId) -> bool {
        if let Some((_, record)) = self.find_entity(key) {
            return matches!(record.data, EntityData::Mob { .. });
        }
        if let Some((_, record)) = self.copied_entity(key) {
            return matches!(record.data, EntityData::Mob { .. });
        }
        self.player_with_entity(key).is_none()
    }

    /// `LargeFireball.onHit`'s `level.explode(this, x, y, z, explosionPower,
    /// mob_griefing, MOB)`: its power's radius, fire by `mob_griefing`, blocks broken
    /// with their drops decaying (`mob_explosion_drop_decay`), its cause the player who
    /// owns it where a player does; its draws from its own stream.
    fn fireball_explodes(
        &mut self,
        fireball: &Entity,
        data: &Fireball,
        in_water: bool,
        output: &mut TickOutput,
    ) {
        let key = fireball.key;
        let player = data.owner.and_then(|owner| owner.player);
        let explosion = Explosion {
            source: key,
            centre: fireball.position,
            radius: f32::from(data.explosion_power),
            owner: player,
            indirect: player,
            decay: true,
            rails_pass: false,
            source_in_water: in_water,
            fire: MOB_GRIEFING,
            hurts: true,
            resistance_at: None,
        };
        let mut random = self.entity_stream(key).random;
        let start = random.clone();
        self.explode(&explosion, &mut random, output);
        self.entity_drew(key, random.draws_since(&start));
    }

    /// A player's hit on or use of a fireball, here or from across a line: a hit
    /// deflects a large one (`Player.attack`'s `REDIRECTABLE_PROJECTILE` branch,
    /// `deflectProjectile`: `deflect(AIM_DEFLECT, player, player, true, 1.0)`), along
    /// the look the attack carries, and plays the attack's no-damage sound where the
    /// player stands. A use does nothing.
    pub(crate) fn fireball_used(
        &mut self,
        entity: &Entity,
        position: ChunkPos,
        by: EntityUse,
        output: &mut TickOutput,
    ) {
        let EntityUse::Attack {
            attacker,
            from,
            look: [yaw, pitch],
            player,
            ..
        } = by
        else {
            return;
        };
        if !redirectable(entity.kind) {
            return;
        }
        let owner = ProjectileOwner {
            key: attacker,
            kind: super::mob_bodies::player_kind(),
            player,
        };
        let motion = view_vector(yaw.0, pitch.0);
        self.deflect_fireball(entity, position, Some(motion), Some(owner), output);
        output.events.push(RegionEvent::Sound {
            sound: sounds::ENTITY_PLAYER_ATTACK_NODAMAGE,
            source: SOUND_SOURCE_PLAYERS,
            position: clustine_world::Vec3::new(from[0].0, from[1].0, from[2].0),
            volume: 1.0,
            pitch: 1.0,
            except: None,
        });
    }

    /// `Projectile.deflect(AIM_DEFLECT, deflector, owner, true, 1.0)` of the region's
    /// fireball `entity` of the chunk `position`: its motion the deflector's look where
    /// the region knows it (`motion`), its owner `owner`, its acceleration power back to
    /// 0.1 (`onDeflection(true)`), committed and sent at once (`needsSync`).
    fn deflect_fireball(
        &mut self,
        entity: &Entity,
        position: ChunkPos,
        motion: Option<Vec3>,
        owner: Option<ProjectileOwner>,
        output: &mut TickOutput,
    ) {
        let Some(fireball) = fireball_of(entity) else {
            return;
        };
        let after = Entity {
            motion: motion.unwrap_or(entity.motion),
            data: EntityData::Fireball(Box::new(Fireball {
                owner,
                acceleration_power: Double(INITIAL_ACCELERATION_POWER),
                ..fireball
            })),
            ..entity.clone()
        };
        let mut fields = Vec::new();
        if after.motion.map(f64::to_bits) != entity.motion.map(f64::to_bits) {
            fields.push(EntityField::Motion(after.motion));
        }
        if after.data != entity.data {
            fields.push(EntityField::Data(after.data.clone()));
        }
        self.commit_entity(position, entity.key, fields, output);
        let mut report = tracked(&after, Tracked::NEEDS_SYNC, true);
        report.chunk = position;
        output.events.push(RegionEvent::EntityTracked(report));
    }

    /// What an explosion does to a fireball (`AbstractHurtingProjectile.hurtServer`, which
    /// refuses it and marks nothing; then `ServerExplosion.hurtEntities`' push, the
    /// caller's): a large one is owned by the explosion's cause, the player `owner`,
    /// after, or by nobody. The flags of its report: none, as nothing marks it hurt.
    pub(crate) fn fireball_blasted(
        &mut self,
        entity: &Entity,
        owner: Option<Uuid>,
        output: &mut TickOutput,
    ) -> u8 {
        if !redirectable(entity.kind) {
            return 0;
        }
        let Some((chunk, current)) = self.find_entity(entity.key) else {
            return 0;
        };
        let current = current.clone();
        let Some(fireball) = fireball_of(&current) else {
            return 0;
        };
        let owner = owner.and_then(|uuid| {
            self.player_key(uuid).map(|key| ProjectileOwner {
                key,
                kind: super::mob_bodies::player_kind(),
                player: Some(uuid),
            })
        });
        if fireball.owner != owner {
            let data = EntityData::Fireball(Box::new(Fireball { owner, ..fireball }));
            self.commit_entity(chunk, entity.key, vec![EntityField::Data(data)], output);
        }
        0
    }

    /// The entity of the player `uuid`, of the region or by their face.
    fn player_key(&self, uuid: Uuid) -> Option<EntityId> {
        let id = PlayerId(uuid);
        if let Some(player) = self.players.get(&id) {
            return Some(player.entity_id);
        }
        if let Some(kept) = self.faces.kept.get(&id) {
            return Some(kept.entity);
        }
        self.faces
            .world
            .iter()
            .flatten()
            .find(|player| player.player == id)
            .map(|player| player.entity)
    }

    /// Where the entity or player `key` looks (`getLookAngle`, of its yaw and pitch),
    /// where the region sees them: its players, its entities and those of its copies,
    /// and other regions' players by their faces.
    fn look_of(&self, key: EntityId) -> Option<Vec3> {
        if let Some(id) = self.player_with_entity(key)
            && let Some(player) = self.players.get(&id)
        {
            return Some(view_vector(player.pose.yaw, player.pose.pitch));
        }
        if let Some((_, record)) = self.find_entity(key) {
            return Some(view_vector(record.yaw, record.pitch));
        }
        if let Some((_, record)) = self.copied_entity(key) {
            return Some(view_vector(record.yaw, record.pitch));
        }
        self.faces
            .kept
            .values()
            .find(|kept| kept.entity == key)
            .map(|kept| view_vector(kept.face.yaw, kept.face.pitch))
    }

    /// A fireball of the entity type `kind` that the mob `owner` shoots from `at` along
    /// `direction` (`Local::ShootFireball`; `new LargeFireball(level, mob, direction,
    /// power)` or `new SmallFireball(level, mob, direction)`, then `setPos(at)` and
    /// `addFreshEntity`): its UUID its only draws, its motion the direction made one long
    /// at the power 0.1 (`assignDirectionalMovement`), turned as its owner is, showing a
    /// fire charge, owned by the mob. Made in the region's own land, sent to the holder
    /// of a chunk it copies, not made where it can read nothing (section 4.1's rule).
    pub(crate) fn shoot_fireball(
        &mut self,
        owner: EntityId,
        kind: u16,
        at: crate::physics::Vec3,
        direction: crate::physics::Vec3,
        explosion_power: u8,
        output: &mut TickOutput,
    ) {
        let chunk = ChunkPos::containing(at[0], at[2]);
        let here = self.ticks_chunk(chunk);
        if !here && !self.readable(chunk) {
            return;
        }
        let Some(key) = (if here {
            self.take_entity_key(chunk)
        } else {
            self.take_key()
        }) else {
            return;
        };
        let mut own = self.entity_stream(key).random;
        let uuid = insecure_uuid(own.next_u64(), own.next_u64());
        let (owner_kind, yaw, pitch) = self
            .find_entity(owner)
            .map_or((0, 0.0, 0.0), |(_, record)| {
                (record.kind, record.yaw, record.pitch)
            });
        let data = Fireball {
            item: ItemStack::new(items::FIRE_CHARGE, 1),
            owner: Some(ProjectileOwner {
                key: owner,
                kind: owner_kind,
                player: None,
            }),
            left_owner: false,
            has_been_shot: false,
            acceleration_power: Double(INITIAL_ACCELERATION_POWER),
            explosion_power: if redirectable(kind) {
                explosion_power
            } else {
                0
            },
        };
        let motion = physics::scale(physics::normalize(direction), INITIAL_ACCELERATION_POWER);
        let entity = Entity {
            portal: Default::default(),
            key,
            kind,
            uuid,
            position: at,
            motion,
            yaw,
            pitch,
            on_ground: false,
            hops: 0,
            lived: 0,
            fresh_at: 0,
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
            data: EntityData::Fireball(Box::new(data)),
        };
        if here {
            self.entity_drew(key, UUID_DRAWS);
            self.add_fresh_entity(entity, output);
        } else {
            self.send_made(entity, output);
        }
    }

    /// A thrown item's or another projectile's hit on a fireball
    /// (`Projectile.onRedirectProjectile`): a large one is deflected along the look of
    /// the hitting projectile's owner where the region sees them, owned by that owner
    /// (or by nobody), its power back to 0.1. The hurt itself does nothing to it
    /// (`hurtServer` refuses it).
    pub(crate) fn fireball_hit(&mut self, entity: &Entity, hit: Hit, output: &mut TickOutput) {
        if !redirectable(entity.kind) {
            return;
        }
        let Some((chunk, current)) = self.find_entity(entity.key) else {
            return;
        };
        let current = current.clone();
        let motion = hit.owner.and_then(|owner| self.look_of(owner.key));
        let owner = hit.owner.map(|owner| ProjectileOwner {
            key: owner.key,
            kind: owner.kind,
            player: owner.player,
        });
        self.deflect_fireball(&current, chunk, motion, owner, output);
    }
}

#[cfg(test)]
#[path = "fireballs_tests.rs"]
mod tests;
