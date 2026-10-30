//! Firework rockets: their construction, flight and explosion
//! (`docs/groundwork/n9-plan.md`, section 4.7; `FireworkRocketEntity`, n9 35–104 for the
//! construction, 226–543 for the tick, 488–572 for the explosion and the hits, 573–722
//! for the damage). A rocket used on a block, a crossbow and a dispenser make one
//! (`make_firework`).
//!
//! Its construction draws, after its UUID, two triangles for its motion across and the
//! lifetime's `nextInt(6)` and `nextInt(7)` (`10 · flight + …`, the flight one more than
//! the stack's `fireworks` flight duration); a shot rocket then draws its shot's six.
//! Each tick: `Projectile.tick` (its first tick's `projectile_shoot`, whether it has
//! left its owner, the fluids, the floor of the world); one not shot at an angle speeds
//! up by 1.15 across (1 after a horizontal collision) and 0.04 up; the hit test along
//! its motion; its move, through the blocks as any entity's, its motion kept; the hit;
//! its turn; its launch sound at life 0; and past its lifetime it explodes. It explodes
//! at an entity it hits, and at a block where it has explosions.
//!
//! The explosion (entity event 17, the `explode` game event): with `n` explosions, every
//! living entity within 5 of it with a clear line to its feet or its middle is hurt by
//! `fireworks(rocket, owner)` `(5 + 2n) · sqrt((5 - d) / 5)`; none without explosions.
//! Nothing is ever attached to it (nobody glides), so nothing takes the whole.
//!
//! Named differences: the block a rocket hits does not run `entityInside`, as no
//! projectile's blocks are (`applyEffectsFromBlocks`, both left out); a player, the
//! region's or another's, is creative and refuses the hurt; an armour stand is not hurt
//! (the hurt of a stand by an explosion of fireworks is not built); the horizontal
//! collision the next tick reads is kept in the region's memory of it
//! (`Memory::vertical_collision`'s place, the memory having no field of its own), which a
//! take-in forgets, as the game's load does.

use clustine_data::{EntityTypeInfo, ItemComponents, entity_types, sounds};
use clustine_world::components::{Component, Kind};
use clustine_world::projectiles::Firework;
use clustine_world::{ChunkPos, Double, Entity, EntityData, EntityId, Float, HurtBy, ItemStack};

use super::Region;
use super::entities::insecure_uuid;
use super::items::{Memory, View};
use super::projectiles::{Aim, UUID_DRAWS};
use super::thrown::{Flying, Struck};
use super::vibrations::{Context, Source, Vibration};
use crate::api::{RegionEvent, TickOutput};
use crate::physics::aabb::Aabb;
use crate::physics::body::{Bounce, NOT_LIVING};
use crate::physics::collide::Context as Collision;
use crate::physics::thrown::{self, Shot as Flight};
use crate::physics::{self, Vec3, fluids};

/// The entity event of a rocket's explosion.
const EXPLODES: u8 = 17;

/// `SoundSource.AMBIENT`, of the launch.
const SOUND_SOURCE_AMBIENT: i32 = 8;

/// A rocket's move: of no gravity of its own, and not living.
const MOVE: Bounce = Bounce {
    gravity: 0.0,
    ..NOT_LIVING
};

/// The flight duration and the number of explosions of the stack's `fireworks`: the
/// patch's, else the item's default (a rocket's flight of 1, no explosions), else none.
fn fireworks_of(stack: &ItemStack) -> Option<(i32, usize)> {
    if stack.components.removes(Kind::Fireworks) {
        return None;
    }
    if let Some(Component::Fireworks(fireworks)) = stack.components.get(Kind::Fireworks) {
        return Some((fireworks.flight_duration, fireworks.explosions.len()));
    }
    ItemComponents::of(stack.item)
        .fireworks_flight_duration
        .map(|flight| (flight, 0))
}

/// `getExplosions().size()`.
fn explosions_of(stack: &ItemStack) -> usize {
    fireworks_of(stack).map_or(0, |(_, explosions)| explosions)
}

impl Region {
    /// `new FireworkRocketEntity(level, stack, owner, x, y, z, shotAtAngle)`, and its
    /// `shoot` where it is shot: the record, whether its chunk is the region's to tick
    /// and the draws of its own stream; none where section 4.1's rule makes nothing.
    pub(crate) fn construct_firework(
        &mut self,
        stack: &ItemStack,
        at: Vec3,
        shot: Option<(Aim, f32, f32)>,
        shot_at_angle: bool,
        owner: Option<EntityId>,
    ) -> Option<(Entity, bool, u64)> {
        let chunk = ChunkPos::containing(at[0], at[2]);
        let here = self.ticks_chunk(chunk);
        if !here && !self.readable(chunk) {
            return None;
        }
        let key = if here {
            self.take_entity_key(chunk)
        } else {
            self.take_key()
        }?;
        let mut own = self.entity_stream(key).random;
        let start = own.clone();
        let uuid = insecure_uuid(own.next_u64(), own.next_u64());
        let flight = 1 + fireworks_of(stack).map_or(0, |(flight, _)| flight);
        let x = thrown::triangle(&mut own, 0.0, 0.002_297);
        let z = thrown::triangle(&mut own, 0.0, 0.002_297);
        let mut motion = [x, 0.05, z];
        let first = own.next_int(6) as i32;
        let lifetime = 10_i32
            .wrapping_mul(flight)
            .wrapping_add(first)
            .wrapping_add(own.next_int(7) as i32);
        let (mut yaw, mut pitch) = (0.0, 0.0);
        if let Some((aim, power, uncertainty)) = shot {
            let Flight {
                motion: shot,
                yaw: shot_yaw,
                pitch: shot_pitch,
            } = match aim {
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
            motion = shot;
            yaw = shot_yaw;
            pitch = shot_pitch;
        }
        let drawn = own.draws_since(&start);
        debug_assert!(drawn >= UUID_DRAWS + 4);
        let entity = Entity {
            portal: Default::default(),
            key,
            kind: u16::try_from(entity_types::FIREWORK_ROCKET)
                .expect("entity type ids fit 16 bits"),
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
            data: EntityData::Firework(Box::new(Firework {
                stack: stack.clone(),
                owner,
                left_owner: false,
                has_been_shot: false,
                life: 0,
                lifetime,
                shot_at_angle,
            })),
        };
        Some((entity, here, drawn))
    }

    /// The tick of the firework rocket `entity` of the loaded chunk `position`
    /// (`FireworkRocketEntity.tick`); it commits through `write_projectile`.
    pub(crate) fn tick_firework(
        &mut self,
        entity: Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let EntityData::Firework(ref rocket) = entity.data else {
            return;
        };
        let mut rocket = (**rocket).clone();
        let key = entity.key;
        let info = EntityTypeInfo::of(entity_types::FIREWORK_ROCKET);
        let owner_root = rocket.owner.and_then(|owner| self.unit_root(owner));
        // `Projectile.tick`.
        if !rocket.has_been_shot {
            self.shot_vibration(rocket.owner, owner_root, entity.position);
        }
        rocket.has_been_shot = true;
        rocket.left_owner = rocket.left_owner
            || self.has_left_owner(key, owner_root, entity.position, info, entity.motion);
        let view = View {
            region: self,
            home: position,
        };
        let bounds = Aabb::of_entity(entity.position, info.width, info.height);
        let found = fluids::update(&view, &bounds);
        let mut motion = fluids::push(&found, entity.motion);
        if self.below_the_world(position, entity.position) {
            self.remove_entity(key, entity.hops, position, output);
            return;
        }
        if !rocket.shot_at_angle {
            let across_wall = self
                .entity_memory
                .get(&key)
                .is_some_and(|memory| memory.vertical_collision);
            let h = if across_wall { 1.0 } else { 1.15 };
            motion = [
                motion[0] * h + 0.0,
                motion[1] * 1.0 + 0.04,
                motion[2] * h + 0.0,
            ];
        }
        let flying = Flying {
            key,
            width: info.width,
            height: info.height,
            owner_root,
            left_owner: rocket.left_owner,
            tick_count: entity.tick_count().saturating_add(1),
            fall_distance: entity.fall_distance,
        };
        let struck = self.hit_on_move(&flying, position, entity.position, motion);
        // `move(SELF, motion)`, then the motion as it was.
        let mut body = self.living_body(&entity, info.width, info.height, position);
        body.motion = motion;
        let view = View {
            region: self,
            home: position,
        };
        body.move_with(&view, false, motion, None, MOVE, &mut |_| {});
        let moved = Entity {
            position: body.position,
            motion,
            on_ground: body.on_ground,
            fall_distance: body.fall_distance,
            data: EntityData::Firework(Box::new(rocket.clone())),
            ..entity.clone()
        };
        let mut exploded = false;
        // A hit, a block's one from inside it too (a rocket used with the click's cursor
        // at a block's middle starts inside it), asks for it to be sent at once
        // (`needsSync`).
        let needs_sync = struck.is_some();
        if let Some(struck) = struck {
            exploded = self.rocket_struck(&moved, struck, output);
        }
        let [yaw, pitch] = thrown::update_rotation(entity.yaw, entity.pitch, motion);
        if rocket.life == 0 {
            output.events.push(RegionEvent::Sound {
                sound: sounds::ENTITY_FIREWORK_ROCKET_LAUNCH,
                source: SOUND_SOURCE_AMBIENT,
                position: clustine_world::Vec3::new(
                    body.position[0],
                    body.position[1],
                    body.position[2],
                ),
                volume: 3.0,
                pitch: 1.0,
                except: None,
            });
        }
        rocket.life = rocket.life.wrapping_add(1);
        if rocket.life > rocket.lifetime {
            // The game explodes it here even where its hit has, a second time.
            self.rocket_explodes(&moved, output);
            exploded = true;
        }
        if exploded {
            self.remove_entity(key, entity.hops, position, output);
            return;
        }
        self.entity_memory.insert(
            key,
            Memory {
                vertical_collision: body.horizontal_collision,
                supporting: body.supporting,
                on_ground_no_blocks: body.on_ground_no_blocks,
                stuck_speed: body.stuck_speed,
                ..Memory::default()
            },
        );
        let after = Entity {
            yaw,
            pitch,
            lived: entity.lived.saturating_add(1),
            data: EntityData::Firework(Box::new(rocket)),
            ..moved
        };
        self.write_projectile_synced(&entity, after, position, needs_sync, output);
    }

    /// `hitTargetOrDeflectSelf` of the rocket `rocket`, where it moved to, at what it
    /// struck: at an entity it explodes; at a block it explodes where it has explosions,
    /// then the block's hook. Whether it exploded.
    fn rocket_struck(&mut self, rocket: &Entity, struck: Struck, output: &mut TickOutput) -> bool {
        let key = rocket.key;
        match struck {
            Struck::Entity { at, .. } => {
                self.rocket_explodes(rocket, output);
                self.vibrate(
                    Vibration::PROJECTILE_LAND,
                    at,
                    Context::of(Some(Source::of(key)), None),
                );
                true
            }
            Struck::Block { hit, .. } => {
                let EntityData::Firework(firework) = &rocket.data else {
                    return false;
                };
                let explodes = explosions_of(&firework.stack) > 0;
                if explodes {
                    self.rocket_explodes(rocket, output);
                }
                let state = self.block(hit.pos);
                if let Some(state) = state {
                    self.projectile_hit(rocket, state, &hit, output);
                }
                self.vibrate_at(
                    Vibration::PROJECTILE_LAND,
                    hit.pos,
                    Context::of(Some(Source::of(key)), state),
                );
                explodes
            }
        }
    }

    /// `FireworkRocketEntity.explode`: its entity event, `explode` of its owner where it
    /// is, and its damage (`dealExplosionDamage`).
    pub(crate) fn rocket_explodes(&mut self, rocket: &Entity, output: &mut TickOutput) {
        let EntityData::Firework(firework) = &rocket.data else {
            return;
        };
        output.events.push(RegionEvent::EntityEvent {
            entity: rocket.key,
            chunk: rocket.chunk(),
            event: EXPLODES,
        });
        let owner = firework.owner;
        let owner_here = owner.and_then(|owner| self.unit_root(owner)).is_some();
        let source = owner.filter(|_| owner_here).map(Source::of);
        self.vibrate(
            Vibration::EXPLODE,
            rocket.position,
            Context::of(source, None),
        );
        let explosions = explosions_of(&firework.stack);
        if explosions == 0 {
            return;
        }
        let damage = 5.0_f32 + (explosions as i32).wrapping_mul(2) as f32;
        let info = EntityTypeInfo::of(entity_types::FIREWORK_ROCKET);
        let around = Aabb::of_entity(rocket.position, info.width, info.height).inflate([5.0; 3]);
        let cause = owner.and_then(|owner| self.causer_of(owner));
        let direct = clustine_world::Causer {
            key: rocket.key,
            kind: rocket.kind,
            position: rocket.position.map(Double),
            player: None,
            living: false,
        };
        for living in self.living_in(&around, rocket.key) {
            if physics::distance_sqr(rocket.position, living.position) > 25.0 {
                continue;
            }
            let height = living.target.bounds.max[1] - living.target.bounds.min[1];
            let clear = (0..2).any(|step| {
                let to = [
                    living.position[0],
                    living.position[1] + height * (0.5 * f64::from(step)),
                    living.position[2],
                ];
                let collision = Collision {
                    bottom: rocket.position[1],
                    fall_distance: rocket.fall_distance,
                    falling_block: false,
                    noclip: None,
                };
                let view = View {
                    region: self,
                    home: rocket.chunk(),
                };
                thrown::clip_blocks(&view, collision, rocket.position, to).is_none()
            });
            if !clear {
                continue;
            }
            // `distanceTo`, in floats.
            let [dx, dy, dz] =
                [0, 1, 2].map(|axis| (rocket.position[axis] - living.position[axis]) as f32);
            let distance = (dx * dx + dy * dy + dz * dz).sqrt();
            let amount = damage * ((5.0 - f64::from(distance)) / 5.0).sqrt() as f32;
            let hurt = HurtBy {
                damage_type: "minecraft:fireworks".to_owned(),
                cause: cause.clone(),
                direct: Some(direct.clone()),
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
            };
            self.give_hurt(&living, hurt, output);
        }
    }
}
