//! The shulker's bullet (`ShulkerBullet`, d javap ShulkerBullet.txt; family E8 of the
//! End's milestone, `docs/groundwork/p-plan.md`, sections 19.2, 19.5 and 19.6).
//!
//! **Its making** (the constructor, 0–71): owned by the shulker, at the middle of the
//! shulker's box, its UUID drawn, aimed at its target and given its first turn away from
//! the axis of the shulker's face (`selectNextMoveDirection`).
//!
//! **Its turns** (`selectNextMoveDirection`, 0–641): it aims at the block of the middle of
//! its target's height (the block below itself where it has none), at that block's middle
//! across and the half height up. Further than 2 from that block's middle, it lists the
//! steps that bring it nearer along each axis but the one it went along, where the block
//! that way is air (x, then y, then z, each the one way that is nearer); with none, a
//! direction drawn (`Direction.getRandom`, `nextInt(6)`), drawn again up to five times
//! while the block that way is not air; with some, one of them drawn (`nextInt(n)`). It
//! then wants the block's middle one step that way, or the target's point where it is
//! near, its wanted motion that way at 0.15 (`targetDelta`), and keeps its course for 10
//! to 50 ticks (`10 + nextInt(5) · 10`); its motion is sent at once (`needsSync`).
//!
//! **Its tick** (`tick`, 0–515): `Projectile.tick` first (its first tick's
//! `projectile_shoot`, whether it has left its owner, the fluids and their push, the floor
//! of the world). A target it cannot find is forgotten. While its target is alive (and no
//! spectator), its wanted motion grows by 1.025 (kept within ±1) and its motion goes a
//! fifth of the way to it; without one it falls by 0.04 a tick (`getDefaultGravity`). Its
//! hit test is along that motion (`getHitResultOnMoveVector`); it moves by it, turns half
//! the way towards it (`rotateTowardsMovement(0.5)`), and hits what it met. With a
//! target, its steps count down and at 0 it turns again; and it turns at once where the
//! block ahead is one an entity stands on, or where it has drawn level with its target's
//! block along the axis it goes.
//!
//! **Its hit** (`onHit`, `onHitEntity`, `onHitBlock`): an entity is hurt by
//! `mob_projectile` 4 caused by its owner, and where the hurt took a living thing
//! levitates for 200 ticks (a creative player takes no hurt, so never levitates: the
//! plan's 19.5 item 14); a block takes the hit (`onProjectileHit`), two `explosion`
//! particles and the hit sound are sent where the bullet is; `projectile_land` is heard;
//! and it is destroyed (`destroy`: gone, with `entity_damage` where it is).
//!
//! **Its hurt** (`hurtServer`): any hurt, a player's hit among them, plays its hurt sound,
//! sends fifteen `crit` particles and destroys it.
//!
//! **Across a line** it goes as every projectile does (`write_projectile`): its owner by
//! key, type and a player's UUID, and its target by key, which the region that takes it
//! finds among its own, its copies' and the players it sees.
//!
//! Named differences:
//! - Whether it has left its owner is asked again every tick from where it is, as its
//!   record keeps no `leftOwner`: one that comes back about its owner may not hit it.
//! - Its target is found where the region sees it (its land, its copies, its players and
//!   the faces of others'), where the game's finds it anywhere in the level; one beyond
//!   is forgotten, as the game's forgets one unloaded.
//! - A block the region cannot read is not air to its turns, and not one to stand on.
//! - The blocks it passes through (`applyEffectsFromBlocks`) and portals are left out, as
//!   for the other projectiles; its fire burns down as nothing.
//! - The trail of `end_rod` particles is the client's (`addParticle`): the server sends
//!   none.

use clustine_data::{BlockState, EntityTypeInfo, entity_types, sounds};
use clustine_world::{
    BlockPos, ChunkPos, Double, Entity, EntityData, EntityId, EntityUse, Float, HurtBy,
    ShulkerBullet, components::EffectInstance, projectiles::ProjectileOwner,
};

use super::Region;
use super::entities::insecure_uuid;
use super::items::{CartMemory, Memory, View};
use super::projectiles::TargetKind;
use super::thrown::{Flying, Struck, far_player_of};
use super::vibrations::{Context, Source, Vibration};
use crate::api::{RegionEvent, TickOutput};
use crate::mobs::types::shulker::{SHULKER, STEPS};
use crate::physics::aabb::Aabb;
use crate::physics::{self, Vec3, fluids};
use crate::region::Random;

/// `SPEED`: the length of its wanted motion.
const SPEED: f64 = 0.15;

/// `getDefaultGravity()`: its fall a tick without a target.
const GRAVITY: f64 = 0.04;

/// `onHitEntity`'s hurt, and the levitation that follows it.
const DAMAGE: f32 = 4.0;
const LEVITATION_TICKS: i32 = 200;

/// The ids of the particles it sends (`ParticleTypes`' order, n9 ParticleTypes): `crit`
/// where it is hurt, `explosion` where it hits a block.
const CRIT: i32 = 13;
const EXPLOSION: i32 = 30;

/// `SoundSource.HOSTILE` by its ordinal: the bullet's own sounds' source.
const HOSTILE: i32 = 5;

/// `Mth.RAD_TO_DEG` widened, as `rotateTowardsMovement` multiplies by it.
const DEGREES: f64 = 57.295_776_367_187_5;

/// A player's box, standing and crouching (`Player.STANDING_DIMENSIONS`, `CROUCHING`).
const PLAYER_HEIGHT: f32 = 1.8;
const CROUCHING_HEIGHT: f32 = 1.5;

/// The bullet's entity type.
fn bullet_kind() -> u16 {
    u16::try_from(entity_types::SHULKER_BULLET).expect("entity type ids fit 16 bits")
}

/// What a bullet reads of its target: its feet, its height (`getBbHeight`) and whether
/// it is alive (and no spectator).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Aimed {
    pub(crate) position: Vec3,
    pub(crate) height: f32,
    pub(crate) alive: bool,
}

/// The axis of the direction `direction` (by its 3D value): 0 for x, 1 for y, 2 for z.
fn axis_of(direction: u8) -> u8 {
    crate::mobs::types::shulker::axis_of(direction)
}

/// `ProjectileUtil.rotateTowardsMovement(entity, 0.5f)` of a bullet that faced `yaw`
/// and `pitch` as its tick began and moves by `motion`: half the way from there towards
/// its motion's, the old ones brought within half a turn first.
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
    [yaw + 0.5 * (to_yaw - yaw), pitch + 0.5 * (to_pitch - pitch)]
}

/// `BlockPos.closerToCenterThan(position, distance)`: the block's middle within
/// `distance` of `at`.
fn closer_to_centre_than(pos: BlockPos, at: Vec3, distance: f64) -> bool {
    let dx = f64::from(pos.x) + 0.5 - at[0];
    let dy = f64::from(pos.y) + 0.5 - at[1];
    let dz = f64::from(pos.z) + 0.5 - at[2];
    dx * dx + dy * dy + dz * dz < distance * distance
}

/// The course a turn sets: the direction, the wanted motion and the steps it keeps it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Course {
    pub(crate) direction: Option<u8>,
    pub(crate) target_delta: Vec3,
    pub(crate) steps: i32,
}

/// `selectNextMoveDirection(axis, target)` of a bullet at `at` (the module's comment),
/// drawing from its own stream; `empty` is `isEmptyBlock`.
pub(crate) fn select_next_move_direction(
    at: Vec3,
    axis: Option<u8>,
    target: Option<&Aimed>,
    empty: &dyn Fn(BlockPos) -> bool,
    own: &mut Random,
) -> Course {
    let here = physics::containing(at);
    let (aim, half) = match target {
        None => (BlockPos::new(here.x, here.y - 1, here.z), 0.5),
        Some(target) => {
            let half = f64::from(target.height) * 0.5;
            let [x, y, z] = target.position;
            (physics::containing([x, y + half, z]), half)
        }
    };
    let mut wanted = [
        f64::from(aim.x) + 0.5,
        f64::from(aim.y) + half,
        f64::from(aim.z) + 0.5,
    ];
    let mut direction = None;
    if !closer_to_centre_than(aim, at, 2.0) {
        let mut nearer: Vec<u8> = Vec::new();
        // x (east 5, west 4), then y (up 1, down 0), then z (south 3, north 2).
        let axes: [(u8, i32, i32, u8, u8); 3] = [
            (0, here.x, aim.x, 5, 4),
            (1, here.y, aim.y, 1, 0),
            (2, here.z, aim.z, 3, 2),
        ];
        for (along, from, to, towards_more, towards_less) in axes {
            if axis == Some(along) {
                continue;
            }
            if from < to && empty(relative(here, towards_more)) {
                nearer.push(towards_more);
            } else if from > to && empty(relative(here, towards_less)) {
                nearer.push(towards_less);
            }
        }
        let mut chosen = own.next_int(6) as u8;
        if nearer.is_empty() {
            let mut tries = 5;
            while !empty(relative(here, chosen)) && tries > 0 {
                chosen = own.next_int(6) as u8;
                tries -= 1;
            }
        } else {
            chosen = nearer[own.next_int(nearer.len() as u32) as usize];
        }
        let [x, y, z] = STEPS[usize::from(chosen)].map(f64::from);
        wanted = [at[0] + x, at[1] + y, at[2] + z];
        direction = Some(chosen);
    }
    let delta = [wanted[0] - at[0], wanted[1] - at[1], wanted[2] - at[2]];
    let length = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
    let target_delta = if length == 0.0 {
        [0.0; 3]
    } else {
        delta.map(|along| along / length * SPEED)
    };
    let steps = 10 + own.next_int(5) as i32 * 10;
    Course {
        direction,
        target_delta,
        steps,
    }
}

/// The block one step from `pos` towards the direction `direction`.
fn relative(pos: BlockPos, direction: u8) -> BlockPos {
    crate::mobs::types::shulker::relative(pos, direction)
}

/// `Mth.clamp(value * 1.025, -1, 1)`.
fn grown(value: f64) -> f64 {
    (value * 1.025).clamp(-1.0, 1.0)
}

/// What a bullet's tick reads of its record.
fn bullet_of(entity: &Entity) -> Option<ShulkerBullet> {
    match &entity.data {
        EntityData::ShulkerBullet(bullet) => Some((**bullet).clone()),
        _ => None,
    }
}

impl Region {
    /// `isEmptyBlock(pos)` as a bullet's turn asks it: air; a block the region cannot
    /// read is not (the module's note).
    fn bullet_sees_air(&self, pos: BlockPos) -> bool {
        self.block(pos).is_some_and(|state| state.props().is_air())
    }

    /// `loadedAndEntityCanStandOn(pos, bullet)`: a block the region reads whose
    /// collision shape covers its top.
    fn bullet_can_stand_on(&self, pos: BlockPos) -> bool {
        self.block(pos).is_some_and(|state: BlockState| {
            let collision: Vec<[f64; 6]> = state.props().collision_shape().boxes().collect();
            clustine_data::faces::face_full(&collision, clustine_data::Face::Up)
        })
    }

    /// The target `key` of a bullet as `EntityReference.getEntity` finds it, where the
    /// region sees it: an entity of its own or of its copies, its own player, or another
    /// region's player by their face.
    pub(crate) fn bullet_target(&self, key: EntityId) -> Option<Aimed> {
        let of_record = |record: &Entity| {
            let info = EntityTypeInfo::of(i32::from(record.kind));
            let height =
                crate::mobs::types::own_size(record).map_or(info.height, |size| size.height);
            let alive = match &record.data {
                EntityData::Mob { .. } => {
                    crate::mobs::Mob::decode(&record.data).is_ok_and(|mob| !mob.body.vitals.dying())
                }
                _ => true,
            };
            Aimed {
                position: record.position,
                height,
                alive,
            }
        };
        if let Some((_, record)) = self.find_entity(key) {
            return Some(of_record(record));
        }
        if let Some((_, record)) = self.copied_entity(key) {
            return Some(of_record(&record));
        }
        if let Some(player) = self
            .player_with_entity(key)
            .and_then(|id| self.players.get(&id))
        {
            let at = player.pose.position;
            return Some(Aimed {
                position: [at.x, at.y, at.z],
                height: if player.keys.sneak {
                    CROUCHING_HEIGHT
                } else {
                    PLAYER_HEIGHT
                },
                alive: !player.dead,
            });
        }
        if let Some(kept) = self.faces.kept.values().find(|kept| kept.entity == key) {
            return Some(Aimed {
                position: kept.face.position,
                height: if kept.face.sneaking {
                    CROUCHING_HEIGHT
                } else {
                    PLAYER_HEIGHT
                },
                alive: kept.face.alive,
            });
        }
        far_player_of(self, key).map(|(_, position, _)| Aimed {
            position,
            height: PLAYER_HEIGHT,
            alive: true,
        })
    }

    /// `new ShulkerBullet(level, shulker, target, axis)` and `addFreshEntity`
    /// (`Local::ShootShulkerBullet`): owned by the shulker `owner`, at `at`, its UUID and
    /// then its first turn (away from the axis `axis`) drawn from its own stream. Made in
    /// the region's own land, sent to the holder of a chunk it copies, not made where it
    /// can read nothing (`projectiles.rs`' rule).
    pub(crate) fn shoot_shulker_bullet(
        &mut self,
        owner: EntityId,
        target: EntityId,
        axis: u8,
        at: Vec3,
        output: &mut TickOutput,
    ) {
        let chunk = ChunkPos::containing(at[0], at[2]);
        let here = self.ticks_chunk(chunk);
        if !here && !self.readable(chunk) {
            return;
        }
        let key = if here {
            self.take_entity_key(chunk)
        } else {
            self.take_key()
        };
        let Some(key) = key else {
            return;
        };
        let mut own = self.entity_stream(key).random;
        let start = own.clone();
        let uuid = insecure_uuid(own.next_u64(), own.next_u64());
        let aimed = self.bullet_target(target);
        let course = select_next_move_direction(
            at,
            Some(axis),
            aimed.as_ref(),
            &|pos| self.bullet_sees_air(pos),
            &mut own,
        );
        let owner_kind = self
            .find_entity(owner)
            .map_or(SHULKER, |(_, record)| record.kind);
        let entity = Entity {
            portal: Default::default(),
            key,
            kind: bullet_kind(),
            uuid,
            position: at,
            motion: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            hops: 0,
            lived: 0,
            fresh_at: 0,
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
            data: EntityData::ShulkerBullet(Box::new(ShulkerBullet {
                owner: Some(ProjectileOwner {
                    key: owner,
                    kind: owner_kind,
                    player: None,
                }),
                target: Some(target),
                steps: course.steps,
                direction: course.direction,
                target_delta: course.target_delta.map(Double),
            })),
        };
        if here {
            self.entity_drew(key, own.draws_since(&start));
            self.add_fresh_entity(entity, output);
        } else {
            self.send_made(entity, output);
        }
    }

    /// The tick of a shulker's bullet (`ShulkerBullet.tick`), as the module's comment
    /// says; it commits through `write_projectile`.
    pub(crate) fn tick_shulker_bullet(
        &mut self,
        entity: Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let Some(mut bullet) = bullet_of(&entity) else {
            return;
        };
        let key = entity.key;
        let info = EntityTypeInfo::of(i32::from(entity.kind));
        let owner_key = bullet.owner.map(|owner| owner.key);
        let owner_root = owner_key.and_then(|owner| self.unit_root(owner));
        // `Projectile.tick`: the first tick's `projectile_shoot`, whether it has left its
        // owner, then `baseTick`: the fluids and their push, and the floor of the world.
        if entity.lived == 0 {
            self.shot_vibration(owner_key, owner_root, entity.position);
        }
        let left_owner = self.has_left_owner(key, owner_root, entity.position, info, entity.motion);
        let view = View {
            region: self,
            home: position,
        };
        let found = fluids::update(
            &view,
            &Aabb::of_entity(entity.position, info.width, info.height),
        );
        let mut motion = fluids::push(&found, entity.motion);
        if self.below_the_world(position, entity.position) {
            self.remove_entity(key, entity.hops, position, output);
            return;
        }
        // Its target, forgotten where it is not found.
        let target = bullet.target.and_then(|target| self.bullet_target(target));
        if target.is_none() {
            bullet.target = None;
        }
        if target.is_some_and(|target| target.alive) {
            let wanted = bullet.target_delta.map(|value| grown(value.0));
            bullet.target_delta = wanted.map(Double);
            motion = [
                motion[0] + (wanted[0] - motion[0]) * 0.2,
                motion[1] + (wanted[1] - motion[1]) * 0.2,
                motion[2] + (wanted[2] - motion[2]) * 0.2,
            ];
        } else {
            motion[1] -= GRAVITY;
        }
        let flying = Flying {
            key,
            width: info.width,
            height: info.height,
            owner_root,
            left_owner,
            tick_count: entity.tick_count().saturating_add(1),
            fall_distance: entity.fall_distance,
        };
        let struck = self.hit_on_move(&flying, position, entity.position, motion);
        let moved = physics::add(entity.position, motion);
        let [yaw, pitch] = rotate_towards_movement(entity.yaw, entity.pitch, motion);
        let mut after = Entity {
            position: moved,
            motion,
            yaw,
            pitch,
            lived: entity.lived.saturating_add(1),
            data: EntityData::ShulkerBullet(Box::new(bullet.clone())),
            ..entity.clone()
        };
        if let Some(struck) = struck {
            self.shulker_bullet_struck(&after, &bullet, struck, output);
            self.destroy_bullet(&after, position, output);
            return;
        }
        // Its turns, with a target.
        let mut needs_sync = false;
        if let Some(target) = target {
            let mut own = self.entity_stream(key).random;
            let start = own.clone();
            let empty = |pos: BlockPos| self.bullet_sees_air(pos);
            let turn = |bullet: &mut ShulkerBullet, axis: Option<u8>, own: &mut Random| {
                let course = select_next_move_direction(moved, axis, Some(&target), &empty, own);
                bullet.direction = course.direction;
                bullet.target_delta = course.target_delta.map(Double);
                bullet.steps = course.steps;
            };
            if bullet.steps > 0 {
                bullet.steps -= 1;
                if bullet.steps == 0 {
                    let axis = bullet.direction.map(axis_of);
                    turn(&mut bullet, axis, &mut own);
                    needs_sync = true;
                }
            }
            if let Some(direction) = bullet.direction {
                let block = physics::containing(moved);
                let axis = axis_of(direction);
                let level = {
                    let aim = physics::containing(target.position);
                    match axis {
                        0 => block.x == aim.x,
                        2 => block.z == aim.z,
                        _ => block.y == aim.y,
                    }
                };
                if self.bullet_can_stand_on(relative(block, direction)) || level {
                    turn(&mut bullet, Some(axis), &mut own);
                    needs_sync = true;
                }
            }
            let drawn = own.draws_since(&start);
            if drawn > 0 {
                self.entity_drew(key, drawn);
            }
            after.data = EntityData::ShulkerBullet(Box::new(bullet));
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
        self.write_projectile_synced(&entity, after, position, needs_sync, output);
    }

    /// `hitTargetOrDeflectSelf`'s `onHit` of the bullet `bullet` (its record `after`, as
    /// it is where its move took it) at what it struck: its `onHitEntity` or
    /// `onHitBlock`, and `projectile_land`. The caller destroys it after.
    fn shulker_bullet_struck(
        &mut self,
        after: &Entity,
        bullet: &ShulkerBullet,
        struck: Struck,
        output: &mut TickOutput,
    ) {
        let key = after.key;
        match struck {
            Struck::Entity { target, kind, at } => {
                // A face is another region's player, creative: it takes no hurt.
                if kind != TargetKind::Face {
                    let hurt = self.bullet_hurt(after, bullet);
                    self.hurt_target(target, kind, hurt, output);
                }
                self.vibrate(
                    Vibration::PROJECTILE_LAND,
                    at,
                    Context::of(Some(Source::of(key)), None),
                );
            }
            Struck::Block { hit, .. } => {
                let state = self.block(hit.pos);
                if let Some(state) = state {
                    self.projectile_hit(after, state, &hit, output);
                }
                // `sendParticles(EXPLOSION, x, y, z, 2, 0.2, 0.2, 0.2, 0)` and the hit
                // sound, where the bullet is.
                output.events.push(RegionEvent::Particles {
                    particle: EXPLOSION,
                    position: after.position,
                    spread: [0.2; 3],
                    speed: 0.0,
                    count: 2,
                    alternative: false,
                });
                output.events.push(RegionEvent::Sound {
                    sound: sounds::ENTITY_SHULKER_BULLET_HIT,
                    source: HOSTILE,
                    position: clustine_world::Vec3::new(
                        after.position[0],
                        after.position[1],
                        after.position[2],
                    ),
                    volume: 1.0,
                    pitch: 1.0,
                    except: None,
                });
                self.vibrate_at(
                    Vibration::PROJECTILE_LAND,
                    hit.pos,
                    Context::of(Some(Source::of(key)), self.block(hit.pos)),
                );
            }
        }
    }

    /// `damageSources().mobProjectile(bullet, owner)`'s hurt of 4, pushing along its
    /// motion, with levitation for 200 ticks where it takes (`addEffect`).
    fn bullet_hurt(&self, bullet: &Entity, data: &ShulkerBullet) -> HurtBy {
        let cause = data.owner.map(|owner| clustine_world::Causer {
            key: owner.key,
            kind: owner.kind,
            position: self
                .thrower(owner.key)
                .map_or(bullet.position.map(Double), |thrower| thrower.from),
            player: owner.player,
            living: true,
        });
        HurtBy {
            damage_type: "minecraft:mob_projectile".to_owned(),
            cause,
            direct: Some(clustine_world::Causer {
                key: bullet.key,
                kind: bullet.kind,
                position: bullet.position.map(Double),
                player: None,
                living: false,
            }),
            position: None,
            projectile_motion: Some(bullet.motion.map(Double)),
            amount: Float(DAMAGE),
            knockback: None,
            fire_before: 0,
            fire_after: 0,
            effects: vec![EffectInstance {
                effect: "minecraft:levitation".to_owned(),
                amplifier: 0,
                duration: LEVITATION_TICKS,
                ambient: false,
                visible: true,
                show_icon: true,
            }],
            arrow: false,
            extras: Vec::new(),
            sound: None,
            hit: None,
        }
    }

    /// `destroy()`: the bullet `record` of the chunk `chunk` gone, and `entity_damage`
    /// heard where it is.
    fn destroy_bullet(&mut self, record: &Entity, chunk: ChunkPos, output: &mut TickOutput) {
        self.remove_entity(record.key, record.hops, chunk, output);
        if let Some(event) = Vibration::named("minecraft:entity_damage") {
            self.vibrate(
                event,
                record.position,
                Context::of(Some(Source::of(record.key)), None),
            );
        }
    }

    /// A shulker's bullet hurt by anything, which destroys it (`ShulkerBullet.hurtServer`):
    /// its hurt sound and fifteen `crit` particles where it is, then gone.
    pub(crate) fn shulker_bullet_hurt(&mut self, record: &Entity, output: &mut TickOutput) {
        let Some((chunk, current)) = self.find_entity(record.key) else {
            return;
        };
        let current = current.clone();
        let at = current.position;
        output.events.push(RegionEvent::Sound {
            sound: sounds::ENTITY_SHULKER_BULLET_HURT,
            source: HOSTILE,
            position: clustine_world::Vec3::new(at[0], at[1], at[2]),
            volume: 1.0,
            pitch: 1.0,
            except: None,
        });
        output.events.push(RegionEvent::Particles {
            particle: CRIT,
            position: at,
            spread: [0.2; 3],
            speed: 0.0,
            count: 15,
            alternative: false,
        });
        self.destroy_bullet(&current, chunk, output);
    }

    /// A player's hit on a bullet, here or from across a line, or a mob's
    /// (`Player.attack`'s `hurtOrSimulate`, `Mob.doHurtTarget`): its hurt. A use does
    /// nothing.
    pub(crate) fn shulker_bullet_used(
        &mut self,
        entity: &Entity,
        by: EntityUse,
        output: &mut TickOutput,
    ) {
        if matches!(by, EntityUse::Attack { .. } | EntityUse::MobHit { .. }) {
            self.shulker_bullet_hurt(entity, output);
        }
    }
}

#[cfg(test)]
#[path = "shulker_bullet_tests.rs"]
mod tests;
