//! Thrown items: the snowball, Clustine's first projectile (`docs/groundwork/n7k-plan.md`,
//! section 10; the brief's section 11.10): thrown by a player (`SnowballItem.use`) or by
//! a mob (`Projectile.spawnProjectile`, the snow golem's shot), its flight
//! (`ThrowableProjectile.tick`, the arithmetic in `crate::physics::thrown`), and its hit
//! (`Snowball.onHitEntity` and `onHit`): a hurt of nothing, or 3 to a blaze, whose
//! knockback pushes along the snowball's motion; its particles (entity event 3); and it
//! is gone.
//!
//! The hit test reads the blocks and entities of the region's loaded and copied chunks
//! and the region's players. An entity the region shows only in a copy is hit by its
//! holder, through an effect on it (`EntityUse::Thrown`), as a mob's hit is; the
//! snowball is gone in its own region. A snowball that crosses a line goes as any
//! entity does (`riding.rs`'s `relocate`) and flies on there.
//!
//! What the game keeps of it in memory only (`isInWater`, which its next tick's drag
//! reads) the region keeps by key, as primed TNT's (`items.rs`'s `CartMemory`); after a
//! take-in it starts dry, as at the game's load.
//!
//! Named differences:
//! - `SnowballItem` is not in the dumps: its use is inferred from what calls it
//!   (**Inferred**: the throw's sound at the player's feet, of the neutral source, at
//!   0.5 and a pitch of `0.4 / (nextFloat() · 0.4 + 0.8)` of the level's random, then
//!   `spawnProjectileFromRotation(Snowball::new, level, stack, player, 0, 1.5, 1)`, the
//!   stack taken but by a creative player, and no cooldown).
//! - A block's `onProjectileHit` (a target block's signal, a bell's ring) and the blocks
//!   a snowball passes through (`applyEffectsFromBlocks`: a tripwire, fire), the first
//!   tick's bubble column, a snowball's burning in lava and the world border wait for
//!   the projectiles of N9.
//! - Players are given the box of one standing, and only the region's own: another
//!   region's player is in no copy.
//! - The owner is known while the region has it, a player or an entity of its own
//!   chunks; one it does not have (gone, or across a line) is none, as the game's
//!   `getOwner()` is null for an owner its level does not have loaded.
//! - No blaze exists yet: every hit hurts by nothing.

use clustine_data::EntityTypeInfo;
use clustine_world::{ChunkPos, Double, Entity, EntityData, EntityId, EntityUse, Float, Thrower};

use super::Region;
use super::attached::Breaker;
use super::items::{CartMemory, Memory, View, tracked};
use super::projectiles::{Target, TargetKind};
use super::vibrations::{Context, Source, Vibration};
use crate::api::{RegionEvent, TickOutput, Tracked};
use crate::mobs::body::hurt::{Causer, DamageSource};
use crate::physics::aabb::Aabb;
use crate::physics::collide::Context as Collision;
use crate::physics::thrown;
use crate::physics::{self, Vec3, fluids};

/// The entity event that shows a snowball's particles where it hit.
const HIT_PARTICLES: u8 = 3;

/// How far below the world's floor an entity is gone (`checkBelowWorld`).
const BELOW_THE_WORLD: f64 = 64.0;

/// A projectile's hit on an entity, as the projectile's region works it out, and as an
/// effect brings it to the entity's holder.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Hit {
    pub(crate) projectile: EntityId,
    pub(crate) kind: u16,
    pub(crate) at: Vec3,
    pub(crate) motion: Vec3,
    pub(crate) damage: f32,
    pub(crate) owner: Option<Thrower>,
}

impl Hit {
    fn as_use(self) -> EntityUse {
        EntityUse::Thrown {
            projectile: self.projectile,
            kind: self.kind,
            at: self.at.map(Double),
            motion: self.motion.map(Double),
            damage: Float(self.damage),
            owner: self.owner,
        }
    }

    /// The hit an effect brings, if it is one.
    pub(crate) fn of_use(by: &EntityUse) -> Option<Self> {
        match *by {
            EntityUse::Thrown {
                projectile,
                kind,
                at,
                motion,
                damage,
                owner,
            } => Some(Self {
                projectile,
                kind,
                at: at.map(|value| value.0),
                motion: motion.map(|value| value.0),
                damage: damage.0,
                owner,
            }),
            _ => None,
        }
    }

    /// `damageSources().thrown(projectile, owner)`.
    fn source(&self) -> DamageSource {
        let direct = Causer {
            key: self.projectile,
            kind: self.kind,
            position: self.at,
            player: None,
            living: false,
        };
        let owner = self.owner.map(|owner| Causer {
            key: owner.key,
            kind: owner.kind,
            position: owner.from.map(|value| value.0),
            player: owner.player.map(|uuid| uuid.as_u128()),
            living: true,
        });
        DamageSource::thrown(direct, self.motion, owner)
    }

    /// Whether a player threw it: every player is a creative one.
    fn by_creative(&self) -> bool {
        self.owner.is_some_and(|owner| owner.player.is_some())
    }
}

impl Region {
    /// The tick of the thrown item `entity` of the loaded chunk `position`
    /// (`ThrowableProjectile.tick`): gravity, then drag, then the hit test along its
    /// move, which puts it where it hit or where its move takes it; its turn towards
    /// its motion; `Projectile.tick` (its first tick's `projectile_shoot`, whether it
    /// has left its owner, the fluids it is in and their push, the floor of the world);
    /// and then the hit.
    pub(crate) fn tick_thrown(
        &mut self,
        entity: Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let EntityData::Thrown {
            ref stack,
            owner,
            left_owner,
            has_been_shot,
        } = entity.data
        else {
            return;
        };
        let key = entity.key;
        let info = EntityTypeInfo::of(i32::from(entity.kind));
        let in_water = self
            .entity_memory
            .get(&key)
            .is_some_and(|memory| memory.cart.in_water);
        // `applyGravity`, then `applyInertia`.
        let mut motion = [
            entity.motion[0] + 0.0,
            entity.motion[1] + -thrown::GRAVITY,
            entity.motion[2] + 0.0,
        ];
        let drag = if in_water {
            thrown::WATER_DRAG
        } else {
            thrown::AIR_DRAG
        };
        motion = physics::scale(motion, f64::from(drag));
        // `ProjectileUtil.getHitResultOnMoveVector(this, canHitEntity)`.
        let owner_root = owner.and_then(|owner| self.unit_root(owner));
        let can_hit = |target: &Target| {
            target.pickable
                && target.alive
                && (left_owner || owner_root.is_none_or(|root| target.root != root))
        };
        let from = entity.position;
        let to = physics::add(from, motion);
        let collision = Collision {
            bottom: from[1],
            fall_distance: entity.fall_distance,
            falling_block: false,
            noclip: None,
        };
        let view = View {
            region: self,
            home: position,
        };
        let block = thrown::clip_blocks(&view, collision, from, to);
        let end = block.map_or(to, |hit| hit.location);
        let search = Aabb::of_entity(from, info.width, info.height)
            .expand_towards(motion)
            .inflate([1.0; 3]);
        let margin = f64::from(thrown::margin(entity.tick_count().saturating_add(1)));
        let mut nearest = f64::MAX;
        let mut struck: Option<(EntityId, TargetKind, Vec3)> = None;
        for target in self.targets(&search, key) {
            if !can_hit(&target) {
                continue;
            }
            let Some(at) = target.bounds.inflate([margin; 3]).clip(from, end) else {
                continue;
            };
            let distance = physics::distance_sqr(from, at);
            if distance < nearest {
                nearest = distance;
                struck = Some((target.key, target.kind, at));
            }
        }
        let location = match (struck, block) {
            (Some((_, _, at)), _) => at,
            (None, Some(hit)) => hit.location,
            (None, None) => to,
        };
        let [yaw, pitch] = thrown::update_rotation(entity.yaw, entity.pitch, motion);
        // `Projectile.tick`: the first tick's `projectile_shoot`, of the owner, where the
        // projectile is now.
        if !has_been_shot {
            let source = owner.filter(|_| owner_root.is_some()).map(Source::of);
            self.vibrate(
                Vibration::PROJECTILE_SHOOT,
                location,
                Context::of(source, None),
            );
        }
        // `checkLeftOwner`, from where it is now.
        let left_owner = left_owner || {
            let reach = Aabb::of_entity(location, info.width, info.height)
                .expand_towards(motion)
                .inflate([1.0; 3]);
            owner_root.is_none_or(|root| {
                !self
                    .targets(&reach, key)
                    .iter()
                    .any(|target| target.pickable && target.root == root)
            })
        };
        // `baseTick`: the fluids it is in, and their push; below the world it is gone.
        let view = View {
            region: self,
            home: position,
        };
        let found = fluids::update(&view, &Aabb::of_entity(location, info.width, info.height));
        motion = fluids::push(&found, motion);
        let min_y = self
            .chunks
            .get(&position)
            .map_or(-64, |chunk| chunk.min_y());
        if location[1] < f64::from(min_y) - BELOW_THE_WORLD {
            self.remove_entity(key, entity.hops, position, output);
            return;
        }
        let after = Entity {
            position: location,
            motion,
            yaw,
            pitch,
            lived: entity.lived.saturating_add(1),
            data: EntityData::Thrown {
                stack: stack.clone(),
                owner,
                left_owner,
                has_been_shot: true,
            },
            ..entity.clone()
        };
        if struck.is_some() || block.is_some() {
            // `hitTargetOrDeflectSelf`: no entity of Clustine's deflects, and a block hit
            // on air is none (a hit is never on air: the clip finds collision shapes).
            match struck {
                Some((target, target_kind, at)) => {
                    let hit = Hit {
                        projectile: key,
                        kind: entity.kind,
                        at: location,
                        motion,
                        damage: 0.0,
                        owner: owner.and_then(|owner| self.thrower(owner)),
                    };
                    // Another region's player, a face here, is creative and takes
                    // nothing (`docs/groundwork/n9-plan.md`, section 4.1).
                    if target_kind != TargetKind::Face {
                        self.thrown_hit(target, hit, output);
                    }
                    self.vibrate(
                        Vibration::PROJECTILE_LAND,
                        at,
                        Context::of(Some(Source::of(key)), None),
                    );
                }
                None => {
                    if let Some(hit) = block {
                        let state = self.block(hit.pos);
                        if let Some(state) = state {
                            self.projectile_hit(&after, state, &hit, output);
                        }
                        self.vibrate_at(
                            Vibration::PROJECTILE_LAND,
                            hit.pos,
                            Context::of(Some(Source::of(key)), state),
                        );
                    }
                }
            }
            // `Snowball.onHit`: its particles, then `discard()`.
            output.events.push(RegionEvent::EntityEvent {
                entity: key,
                chunk: position,
                event: HIT_PARTICLES,
            });
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
    }

    /// `onHitEntity` of a projectile's hit `hit` on the entity or player `target`: done
    /// here where the region has the entity; a player is creative and takes nothing; an
    /// entity the region sees only in a copy is hit by its holder.
    fn thrown_hit(&mut self, target: EntityId, hit: Hit, output: &mut TickOutput) {
        if self.find_entity(target).is_some() {
            self.thrown_hit_here(target, hit, output);
            return;
        }
        if self.player_with_entity(target).is_some() {
            // A creative player is invulnerable to everything that a snowball deals.
            return;
        }
        self.send_on_entity(target, hit.as_use());
    }

    /// `target.hurt(thrown(projectile, owner), damage)` of the region's entity `target`,
    /// as its kind takes it: a mob by its body's hurt, pushed along the projectile's
    /// motion; a vehicle wobbles, or a creative player's throw takes it away; a hanging
    /// thing breaks, or a frame gives up its item; a falling block is only marked hurt;
    /// an armour stand is not hurt by a throw (`#can_break_armor_stand`), nor primed TNT,
    /// an item or an orb (none of which a projectile hits).
    pub(crate) fn thrown_hit_here(&mut self, target: EntityId, hit: Hit, output: &mut TickOutput) {
        let Some((chunk, record)) = self.find_entity(target) else {
            return;
        };
        let record = record.clone();
        let creative = hit.by_creative();
        match &record.data {
            EntityData::Mob { .. } => {
                self.hurt_mob(target, &hit.source(), hit.damage, output);
            }
            EntityData::Minecart { .. } | EntityData::Boat { .. } => {
                self.hurt_vehicle(target, hit.damage, creative, output);
            }
            EntityData::ItemFrame { .. } => self.hurt_frame(&record, creative, output),
            EntityData::Painting { .. }
            | EntityData::Cushion { .. }
            | EntityData::LeashKnot { .. } => {
                let player = hit.owner.and_then(|owner| owner.player);
                self.hurt_attached(target, Breaker::of_owner(player), output);
            }
            EntityData::FallingBlock { .. } => {
                // `markHurt`: its motion is sent.
                let mut report = tracked(&record, Tracked::SYNC_VELOCITY, false);
                report.chunk = chunk;
                output.events.push(RegionEvent::EntityTracked(report));
            }
            EntityData::ArmorStand { .. }
            | EntityData::PrimedTnt { .. }
            | EntityData::Item { .. }
            | EntityData::ExperienceOrb { .. }
            | EntityData::Thrown { .. }
            | EntityData::Arrow(_)
            | EntityData::Spit(_)
            | EntityData::Firework(_)
            | EntityData::Cloud(_)
            | EntityData::Fangs(_) => {}
        }
    }
}
