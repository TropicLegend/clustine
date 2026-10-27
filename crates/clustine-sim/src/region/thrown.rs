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

use clustine_data::{EntityTypeInfo, entity_types, items, sounds};
use clustine_world::{
    ChunkPos, Double, Entity, EntityChange, EntityData, EntityField, EntityId, EntityUse, Float,
    ItemStack, PlayerId, Thrower,
};

use super::Region;
use super::attached::Breaker;
use super::entities::insecure_uuid;
use super::items::{CartMemory, Memory, View, tracked};
use super::riding::Relocated;
use super::vibrations::{Context, Source, Vibration};
use crate::api::{RegionEvent, TickOutput, Tracked};
use crate::mobs::body::hurt::{Causer, DamageSource};
use crate::physics::aabb::Aabb;
use crate::physics::collide::Context as Collision;
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::physics::thrown::{self, Shot};
use crate::physics::{self, Vec3, fluids};
use crate::placement::ItemUse;

/// The entity event that shows a snowball's particles where it hit.
const HIT_PARTICLES: u8 = 3;

/// How far above a player's feet their eyes are, standing and crouching.
const STANDING_EYES: f32 = 1.62;
const CROUCHING_EYES: f32 = 1.27;

/// The box a player is given in the hit test, standing.
const PLAYER_WIDTH: f32 = 0.6;
const PLAYER_HEIGHT: f32 = 1.8;

/// How far below the world's floor an entity is gone (`checkBelowWorld`).
const BELOW_THE_WORLD: f64 = 64.0;

/// The draws of a projectile's birth: its UUID's two longs (`Entity.<init>`).
const UUID_DRAWS: u64 = 2;

/// The snowball's type, by id.
fn snowball_kind() -> u16 {
    u16::try_from(entity_types::SNOWBALL).expect("entity type ids fit 16 bits")
}

/// How a projectile is shot as it is made.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Aim {
    /// `shootFromRotation` of a shooter looking along `yaw` and `pitch`, moving by
    /// `known` and on the ground or not (a player's throw).
    Rotation {
        yaw: f32,
        pitch: f32,
        known: Vec3,
        on_ground: bool,
    },
    /// `shoot` along `direction` (a mob's shot).
    Direction(Vec3),
}

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

/// An entity a projectile may hit, as the hit test finds it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Target {
    key: EntityId,
    bounds: Aabb,
    /// The root of its unit (`getRootVehicle`), which an owner's own unit is told by.
    root: EntityId,
    /// `isPickable`, and `isAlive` beside it.
    pickable: bool,
    alive: bool,
}

/// `isPickable` of a record, and `isAlive`: items, orbs and thrown items are never
/// picked; a mob is alive while it has health, a marker stand is not picked; every other
/// kind is picked and alive.
fn pickable_and_alive(entity: &Entity) -> (bool, bool) {
    match &entity.data {
        EntityData::Item { .. } | EntityData::ExperienceOrb { .. } | EntityData::Thrown { .. } => {
            (false, true)
        }
        EntityData::Mob { .. } => {
            let alive = crate::mobs::Mob::decode(&entity.data)
                .is_ok_and(|mob| mob.body.vitals.health > 0.0);
            (true, alive)
        }
        EntityData::ArmorStand { flags, health, .. } => {
            (flags & super::stands::MARKER == 0, health.0 > 0.0)
        }
        _ => (true, true),
    }
}

impl Region {
    /// `SnowballItem.use` of the player `player`'s held snowball in the air: the throw's
    /// sound, then the snowball made at their eyes and shot along their look with their
    /// movement (`spawnProjectileFromRotation`); a creative player keeps the stack, and
    /// a snowball has no cooldown. Whether the held item was a snowball.
    pub(crate) fn throw_snowball(
        &mut self,
        used: &ItemUse,
        player: PlayerId,
        output: &mut TickOutput,
    ) -> bool {
        if used.held != Some(items::SNOWBALL) {
            return false;
        }
        let Some(thrower) = self.players.get(&player) else {
            return false;
        };
        let feet = thrower.pose.position;
        let on_ground = thrower.pose.on_ground;
        let owner = thrower.entity_id;
        let eyes = if used.sneaking && !thrower.flying {
            CROUCHING_EYES
        } else {
            STANDING_EYES
        };
        // `level.playSound(null, x, y, z, SNOWBALL_THROW, NEUTRAL, 0.5, pitch)`, the
        // pitch of the level's random, heard by everyone.
        let mut random = self.next_draw();
        let pitch = 0.4 / (random.next_float() * 0.4 + 0.8);
        output.events.push(RegionEvent::Sound {
            sound: sounds::ENTITY_SNOWBALL_THROW,
            source: SOUND_SOURCE_NEUTRAL,
            position: feet,
            volume: 0.5,
            pitch,
            except: None,
        });
        let at = [
            feet.x,
            feet.y + f64::from(eyes) - thrown::BELOW_THE_EYES,
            feet.z,
        ];
        let aim = Aim::Rotation {
            yaw: used.yaw,
            pitch: used.pitch,
            known: self.player_movement(player),
            on_ground,
        };
        let stack = ItemStack::new(items::SNOWBALL, 1);
        self.make_thrown(
            owner,
            stack,
            at,
            aim,
            thrown::PLAYER_POWER,
            thrown::PLAYER_UNCERTAINTY,
            output,
        );
        true
    }

    /// `new Snowball(level, owner, stack)`, shot by `aim` at `power` and `uncertainty`,
    /// and `addFreshEntity` (`Projectile.spawnProjectile`): at `at`, under the region's
    /// next key, its UUID and then its shot drawn from its own stream. Into a chunk the
    /// region does not have loaded and ticking it is sent as an arrival, as a thrown item
    /// is. False, and nothing made, where its chunk is full or no key is left.
    #[expect(
        clippy::too_many_arguments,
        reason = "the projectile's parts, as `spawnProjectile`'s callers give them"
    )]
    pub(crate) fn make_thrown(
        &mut self,
        owner: EntityId,
        stack: ItemStack,
        at: Vec3,
        aim: Aim,
        power: f32,
        uncertainty: f32,
        output: &mut TickOutput,
    ) -> bool {
        let chunk = ChunkPos::containing(at[0], at[2]);
        let here = self.ticks_chunk(chunk);
        let key = if here {
            self.take_entity_key(chunk)
        } else {
            self.take_key()
        };
        let Some(key) = key else {
            return false;
        };
        let mut own = self.entity_stream(key).random;
        let start = own.clone();
        let uuid = insecure_uuid(own.next_u64(), own.next_u64());
        let Shot { motion, yaw, pitch } = match aim {
            Aim::Rotation {
                yaw,
                pitch,
                known,
                on_ground,
            } => thrown::shoot_from_rotation(
                yaw,
                pitch,
                power,
                uncertainty,
                known,
                on_ground,
                &mut own,
            ),
            Aim::Direction(direction) => thrown::shoot(direction, power, uncertainty, &mut own),
        };
        let entity = Entity {
            key,
            kind: snowball_kind(),
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
            data: EntityData::Thrown {
                // `setItem` keeps one of the stack.
                stack: ItemStack { count: 1, ..stack },
                owner: Some(owner),
                left_owner: false,
                has_been_shot: false,
            },
        };
        debug_assert_eq!(own.draws_since(&start), UUID_DRAWS + 6);
        if here {
            self.entity_drew(key, own.draws_since(&start));
            self.add_fresh_entity(entity, output);
        } else {
            self.send_made(entity, output);
        }
        true
    }

    /// The root of the unit of the entity or player `key`, where the region has it: a
    /// player's is their vehicle's root, or their own entity.
    fn unit_root(&self, key: EntityId) -> Option<EntityId> {
        if let Some(id) = self.player_with_entity(key) {
            let player = self.players.get(&id)?;
            return Some(
                player
                    .riding
                    .map_or(player.entity_id, |riding| self.root_of(riding.vehicle)),
            );
        }
        self.find_entity(key).map(|_| self.root_of(key))
    }

    /// Who threw a projectile, as a hurt names its cause, where the region has them.
    fn thrower(&self, owner: EntityId) -> Option<Thrower> {
        if let Some(id) = self.player_with_entity(owner) {
            let feet = self.players.get(&id)?.pose.position;
            return Some(Thrower {
                key: owner,
                kind: super::mob_bodies::player_kind(),
                from: [Double(feet.x), Double(feet.y), Double(feet.z)],
                player: Some(id.0),
            });
        }
        let (_, record) = self.find_entity(owner)?;
        Some(Thrower {
            key: owner,
            kind: record.kind,
            from: record.position.map(Double),
            player: None,
        })
    }

    /// The entities of the region's loaded and copied chunks and its players whose boxes
    /// meet `bounds`, but `except`, in the order of the chunks and their sections, the
    /// players last.
    fn targets(&self, bounds: &Aabb, except: EntityId) -> Vec<Target> {
        let low = ChunkPos::containing(bounds.min[0] - 2.0, bounds.min[2] - 2.0);
        let high = ChunkPos::containing(bounds.max[0] + 2.0, bounds.max[2] + 2.0);
        let mut found = Vec::new();
        for x in low.x..=high.x {
            for z in low.z..=high.z {
                let Some(chunk) = self.readable_chunk(ChunkPos::new(x, z)) else {
                    continue;
                };
                let records = chunk.entities();
                let mut here: Vec<(i32, Target)> = records
                    .iter()
                    .filter(|entity| entity.key != except)
                    .filter_map(|entity| {
                        let own = self.entity_box(entity);
                        own.intersects(*bounds).then(|| {
                            let (pickable, alive) = pickable_and_alive(entity);
                            let target = Target {
                                key: entity.key,
                                bounds: own,
                                root: super::colliders::root_among(records, entity.key),
                                pickable,
                                alive,
                            };
                            (physics::floor(entity.position[1]) >> 4, target)
                        })
                    })
                    .collect();
                here.sort_by_key(|(section, target)| (*section, target.key));
                found.extend(here.into_iter().map(|(_, target)| target));
            }
        }
        for player in self.players.values() {
            let feet = player.pose.position;
            let own = Aabb::of_entity([feet.x, feet.y, feet.z], PLAYER_WIDTH, PLAYER_HEIGHT);
            if !own.intersects(*bounds) {
                continue;
            }
            let root = player
                .riding
                .map_or(player.entity_id, |riding| self.root_of(riding.vehicle));
            found.push(Target {
                key: player.entity_id,
                bounds: own,
                root,
                pickable: true,
                alive: true,
            });
        }
        found
    }

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
        let mut struck: Option<(EntityId, Vec3)> = None;
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
                struck = Some((target.key, at));
            }
        }
        let location = match (struck, block) {
            (Some((_, at)), _) => at,
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
                Some((target, at)) => {
                    let hit = Hit {
                        projectile: key,
                        kind: entity.kind,
                        at: location,
                        motion,
                        damage: 0.0,
                        owner: owner.and_then(|owner| self.thrower(owner)),
                    };
                    self.thrown_hit(target, hit, output);
                    self.vibrate(
                        Vibration::PROJECTILE_LAND,
                        at,
                        Context::of(Some(Source::of(key)), None),
                    );
                }
                None => {
                    if let Some(hit) = block {
                        let state = self.block(hit.pos);
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
        self.write_thrown(&entity, after, position, output);
    }

    /// What a thrown item's tick leaves: where it goes (`relocate`), its commit and the
    /// edges' report. It moves every tick it flies, so every such tick commits.
    fn write_thrown(
        &mut self,
        before: &Entity,
        after: Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let key = before.key;
        let mut after = after;
        let to = match self.relocate(position, before.position, &mut after, output) {
            Relocated::Sent => return,
            Relocated::Moved(to) => to,
            Relocated::Stays => position,
        };
        if to == position {
            let bits = |values: [f64; 3]| values.map(f64::to_bits);
            let mut fields = Vec::new();
            if bits(after.position) != bits(before.position) {
                fields.push(EntityField::Position(after.position));
            }
            if bits(after.motion) != bits(before.motion) {
                fields.push(EntityField::Motion(after.motion));
            }
            if after.yaw.to_bits() != before.yaw.to_bits()
                || after.pitch.to_bits() != before.pitch.to_bits()
            {
                fields.push(EntityField::Rotation {
                    yaw: after.yaw,
                    pitch: after.pitch,
                });
            }
            if after.data != before.data {
                fields.push(EntityField::Data(after.data.clone()));
            }
            if !fields.is_empty() {
                output
                    .entity_changes
                    .push((position, EntityChange::Set(key, fields)));
            }
            if let Some(chunk) = self.chunks.get_mut(&position) {
                chunk.apply_entity(&EntityChange::Whole(after.clone()));
            }
        }
        output.entities_ageing.push(to);
        let mut report = tracked(&after, 0, true);
        report.chunk = to;
        output.events.push(RegionEvent::EntityTracked(report));
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
