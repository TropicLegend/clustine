//! What every projectile of N9 shares (`docs/groundwork/n9-plan.md`, section 4.1): its
//! making, by one function for every kind (`Projectile.spawnProjectile` and the
//! constructions its callers give), the rule for a making where the region does not
//! tick, the hit test's list of targets, and the commit of a projectile's tick. Each
//! kind's tick and hits are its own file's: the thrown items' `thrown.rs`, the arrows'
//! and the trident's `arrows.rs`, the spit's `spit.rs`, the rocket's `fireworks.rs`, the
//! cloud's `clouds.rs` and the fangs' `fangs.rs`.
//!
//! **A making where the region does not tick** (finding 23): what is made in the
//! region's own land is made there; in a chunk it copies, under a key of its own sent to
//! the chunk's holder (`take_key` and `send_made`), its own draws made by the maker; where
//! the region can read nothing, it is not made (named).
//!
//! **The targets** are the entities of the region's loaded and copied chunks, then its
//! own players, then the faces of other regions' players it keeps in land it holds or
//! copies: a face is a standing box (1.5 tall crouching) that can be hit and refuses
//! every hurt, as a creative player does, so that a projectile aimed past a player
//! across a line turns back from them as it would in one region. A face is told apart
//! by [`TargetKind::Face`], whose hit no projectile sends anywhere.

use clustine_data::entity_types;
use clustine_world::{
    ChunkPos, Difficulty, Double, Entity, EntityChange, EntityData, EntityField, EntityId, Fangs,
    ItemStack, Thrower, projectiles::Cloud,
};

use super::Region;
use super::entities::insecure_uuid;
use super::items::tracked;
use super::random_ticks::Random;
use super::riding::Relocated;
use crate::api::{RegionEvent, TickOutput};
use crate::mobs::tick::Shot;
use crate::physics::aabb::Aabb;
use crate::physics::thrown::{self, Shot as Flight};
use crate::physics::{self, Vec3};

/// The draws of a projectile's birth: its UUID's two longs (`Entity.<init>`).
pub(crate) const UUID_DRAWS: u64 = 2;

/// The draws of `shoot` and `shootFromRotation`: three gaussians, each two doubles.
const SHOOT_DRAWS: u64 = 6;

/// The box a player is given in the hit test, standing, and crouching.
const PLAYER_WIDTH: f32 = 0.6;
const PLAYER_HEIGHT: f32 = 1.8;
const CROUCHING_HEIGHT: f32 = 1.5;

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
    /// `shoot` along `direction` (a mob's shot, a dispenser's).
    Direction(Vec3),
}

/// A projectile to be made: its type, where, its record before the shot, and how it is
/// shot.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Making {
    pub(crate) kind: u16,
    pub(crate) at: Vec3,
    pub(crate) data: EntityData,
    pub(crate) aim: Aim,
    pub(crate) power: f32,
    pub(crate) uncertainty: f32,
}

/// Who works a hit on a target out: the region where it holds the entity, the holder
/// through an effect where the region sees it in a copy, the region for its own player,
/// and nobody for another region's player, whose face refuses every hurt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetKind {
    Here,
    Copy,
    Player,
    Face,
}

/// An entity a projectile may hit, as the hit test finds it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Target {
    pub(crate) key: EntityId,
    pub(crate) bounds: Aabb,
    /// The root of its unit (`getRootVehicle`), which an owner's own unit is told by.
    pub(crate) root: EntityId,
    /// `isPickable`, and `isAlive` beside it.
    pub(crate) pickable: bool,
    pub(crate) alive: bool,
    pub(crate) kind: TargetKind,
}

/// `isPickable` of a record, and `isAlive`: items, orbs, projectiles, clouds and fangs
/// are never picked; a mob is alive while it has health, a marker stand is not picked;
/// every other kind is picked and alive.
pub(crate) fn pickable_and_alive(entity: &Entity) -> (bool, bool) {
    match &entity.data {
        EntityData::Item { .. }
        | EntityData::ExperienceOrb { .. }
        | EntityData::Thrown { .. }
        | EntityData::Arrow(_)
        | EntityData::Spit(_)
        | EntityData::Firework(_)
        | EntityData::Cloud(_)
        | EntityData::Fangs(_) => (false, true),
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

/// The record a mob's or a dispenser's shot `shot` is made with: a thrown item for the
/// throwables, an arrow for the arrows and the trident; `None` for a kind no shot
/// makes. Owned by `owner`.
pub(crate) fn shot_record(shot: &Shot, owner: Option<EntityId>) -> Option<EntityData> {
    let kind = i32::from(shot.kind);
    if kind == entity_types::ARROW
        || kind == entity_types::SPECTRAL_ARROW
        || kind == entity_types::TRIDENT
    {
        let of_kind = if kind == entity_types::SPECTRAL_ARROW {
            clustine_world::ArrowKind::Spectral {
                glowing_duration: 200,
            }
        } else if kind == entity_types::TRIDENT {
            clustine_world::ArrowKind::Trident {
                dealt_damage: false,
                returning: 0,
            }
        } else {
            clustine_world::ArrowKind::Arrow
        };
        // `AbstractArrow.<init>`: an arrow of intangible ammunition (a creative player's
        // bow) may be picked up only by a creative player.
        let pickup = if shot
            .stack
            .components
            .get(clustine_world::components::Kind::IntangibleProjectile)
            .is_some()
        {
            clustine_world::Pickup::CreativeOnly
        } else {
            clustine_world::Pickup::Disallowed
        };
        return Some(EntityData::Arrow(Box::new(clustine_world::Arrow {
            pickup,
            pickup_stack: shot.stack.with_count(1),
            weapon: shot.weapon.clone(),
            owner,
            left_owner: false,
            has_been_shot: false,
            base_damage: Double(2.0),
            crit: false,
            pierce_level: 0,
            pierced: Vec::new(),
            // `getDefaultHitGroundSoundEvent`: the trident's own.
            hit_sound: if kind == entity_types::TRIDENT {
                "minecraft:item.trident.hit_ground"
            } else {
                "minecraft:entity.arrow.hit"
            }
            .to_owned(),
            in_ground: None,
            shake_until: 0,
            of_kind,
        })));
    }
    let throwable = [
        entity_types::SNOWBALL,
        entity_types::EGG,
        entity_types::SPLASH_POTION,
        entity_types::LINGERING_POTION,
        entity_types::EXPERIENCE_BOTTLE,
        entity_types::ENDER_PEARL,
    ];
    throwable.contains(&kind).then(|| EntityData::Thrown {
        // `setItem` keeps one of the stack.
        stack: shot.stack.with_count(1),
        owner,
        left_owner: false,
        has_been_shot: false,
    })
}

impl Region {
    /// `Projectile.spawnProjectile`: `making`'s projectile under the region's next key,
    /// its UUID drawn from its own stream, then its construction's draws (`construct`,
    /// given the stream and the record, which an arrow from a mob draws its damage's
    /// triangle in), then its shot (`shoot` or `shootFromRotation`), and added. Made in
    /// the region's own land, sent to the holder of a chunk it copies, not made where it
    /// can read nothing (section 4.1's rule). The key, or `None` where nothing was made
    /// (its chunk full, no key left, nowhere to make it).
    pub(crate) fn make_projectile(
        &mut self,
        making: Making,
        construct: impl FnOnce(&mut Random, &mut EntityData),
        output: &mut TickOutput,
    ) -> Option<EntityId> {
        let Making {
            kind,
            at,
            mut data,
            aim,
            power,
            uncertainty,
        } = making;
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
        let constructed = own.clone();
        construct(&mut own, &mut data);
        let construction = own.draws_since(&constructed);
        let Flight { motion, yaw, pitch } = match aim {
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
        debug_assert_eq!(
            own.draws_since(&start),
            UUID_DRAWS + construction + SHOOT_DRAWS
        );
        let entity = Entity {
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
            data,
        };
        self.add_made(entity, here, own.draws_since(&start), output);
        Some(key)
    }

    /// The mob `key`'s shot `shot` (`Local::Shoot`), made as `spawnProjectile` makes it,
    /// an arrow from a mob drawing its damage first.
    pub(crate) fn mob_shoots(&mut self, key: EntityId, shot: &Shot, output: &mut TickOutput) {
        let Some(data) = shot_record(shot, Some(key)) else {
            return;
        };
        let making = Making {
            kind: shot.kind,
            at: shot.at,
            data,
            aim: Aim::Direction(shot.direction),
            power: shot.power,
            uncertainty: shot.uncertainty,
        };
        let from_mob = shot.from_mob;
        self.make_projectile(
            making,
            |own, data| {
                if let Some(power) = from_mob {
                    Region::damage_from_mob(power, own, data);
                }
            },
            output,
        );
    }

    /// A made entity added where it was made, its draws noted, or sent to its holder.
    fn add_made(&mut self, entity: Entity, here: bool, drawn: u64, output: &mut TickOutput) {
        if here {
            self.entity_drew(entity.key, drawn);
            self.add_fresh_entity(entity, output);
        } else {
            self.send_made(entity, output);
        }
    }

    /// An entity of no shot made at `at` facing `yaw`, its UUID its only draws, by
    /// section 4.1's rule: the cloud's and the fangs' making.
    fn make_still(
        &mut self,
        kind: i32,
        at: Vec3,
        yaw: f32,
        data: EntityData,
        output: &mut TickOutput,
    ) -> Option<EntityId> {
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
        let uuid = insecure_uuid(own.next_u64(), own.next_u64());
        let entity = Entity {
            key,
            kind: u16::try_from(kind).expect("entity type ids fit 16 bits"),
            uuid,
            position: at,
            motion: [0.0; 3],
            yaw,
            pitch: 0.0,
            on_ground: false,
            hops: 0,
            lived: 0,
            fresh_at: 0,
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
            data,
        };
        self.add_made(entity, here, UUID_DRAWS, output);
        Some(key)
    }

    /// `new EvokerFangs(level, x, y, z, yaw, warmup, owner)` and `addFreshEntity`
    /// (section 4.8).
    #[expect(dead_code, reason = "the evoker (S8) makes it")]
    pub(crate) fn make_fangs(
        &mut self,
        at: Vec3,
        yaw: f32,
        warmup: i32,
        owner: Option<EntityId>,
        output: &mut TickOutput,
    ) -> Option<EntityId> {
        let data = EntityData::Fangs(Fangs { owner, warmup });
        self.make_still(entity_types::EVOKER_FANGS, at, yaw, data, output)
    }

    /// `new AreaEffectCloud(level, x, y, z)` with `cloud` set, and `addFreshEntity`
    /// (section 4.5: a lingering potion's, made by `thrown.rs`).
    #[expect(dead_code, reason = "the lingering potion (S3) makes it")]
    pub(crate) fn make_cloud(
        &mut self,
        at: Vec3,
        cloud: Cloud,
        output: &mut TickOutput,
    ) -> Option<EntityId> {
        let data = EntityData::Cloud(Box::new(cloud));
        self.make_still(entity_types::AREA_EFFECT_CLOUD, at, 0.0, data, output)
    }

    /// `new FireworkRocketEntity(level, stack, …)` and `addFreshEntity` (section 4.7),
    /// made by a rocket used on a block (`items/rockets.rs`), a crossbow and a
    /// dispenser. `fireworks.rs`' to build: its construction draws two triangles and
    /// the lifetime's two ints, which the rocket's own tick reads. `shot` is the aim,
    /// power and uncertainty of a rocket that is shot (a crossbow's, a dispenser's),
    /// none for one used on a block, which flies up. Nothing is made until it is.
    #[expect(
        clippy::unused_self,
        reason = "fireworks.rs builds it on the region's making"
    )]
    pub(crate) fn make_firework(
        &mut self,
        _stack: &ItemStack,
        _at: Vec3,
        _shot: Option<(Aim, f32, f32)>,
        _shot_at_angle: bool,
        _owner: Option<EntityId>,
        _output: &mut TickOutput,
    ) -> Option<EntityId> {
        None
    }

    /// `AbstractArrow.setBaseDamageFromMob(power)`, in an arrow's construction: its base
    /// damage `power · 2 + triangle(difficulty · 0.11, 0.57425)`, drawn from its own
    /// stream after its UUID (D9 1.1).
    pub(crate) fn damage_from_mob(power: f32, own: &mut Random, data: &mut EntityData) {
        let EntityData::Arrow(arrow) = data else {
            return;
        };
        let difficulty = f64::from(Difficulty::WORLD.id());
        arrow.base_damage =
            Double(f64::from(power) * 2.0 + thrown::triangle(own, difficulty * 0.11, 0.574_25));
    }

    /// The root of the unit of the entity or player `key`, where the region has it: a
    /// player's is their vehicle's root, or their own entity.
    pub(crate) fn unit_root(&self, key: EntityId) -> Option<EntityId> {
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

    /// Who shot a projectile, as a hurt names its cause, where the region has them.
    pub(crate) fn thrower(&self, owner: EntityId) -> Option<Thrower> {
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

    /// The entities of the region's loaded and copied chunks, its players and the faces
    /// of other regions' players whose boxes meet `bounds`, but `except`: in the order
    /// of the chunks and their sections, then the players by key, then the faces by
    /// player.
    pub(crate) fn targets(&self, bounds: &Aabb, except: EntityId) -> Vec<Target> {
        let low = ChunkPos::containing(bounds.min[0] - 2.0, bounds.min[2] - 2.0);
        let high = ChunkPos::containing(bounds.max[0] + 2.0, bounds.max[2] + 2.0);
        let mut found = Vec::new();
        for x in low.x..=high.x {
            for z in low.z..=high.z {
                let position = ChunkPos::new(x, z);
                let Some(chunk) = self.readable_chunk(position) else {
                    continue;
                };
                let kind = if self.chunks.contains_key(&position) {
                    TargetKind::Here
                } else {
                    TargetKind::Copy
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
                                kind,
                            };
                            (physics::floor(entity.position[1]) >> 4, target)
                        })
                    })
                    .collect();
                here.sort_by_key(|(section, target)| (*section, target.key));
                found.extend(here.into_iter().map(|(_, target)| target));
            }
        }
        let mut players: Vec<Target> = Vec::new();
        for player in self.players.values() {
            let feet = player.pose.position;
            let own = Aabb::of_entity([feet.x, feet.y, feet.z], PLAYER_WIDTH, PLAYER_HEIGHT);
            if !own.intersects(*bounds) || player.entity_id == except {
                continue;
            }
            let root = player
                .riding
                .map_or(player.entity_id, |riding| self.root_of(riding.vehicle));
            players.push(Target {
                key: player.entity_id,
                bounds: own,
                root,
                pickable: true,
                alive: true,
                kind: TargetKind::Player,
            });
        }
        players.sort_by_key(|target| target.key);
        found.extend(players);
        let holds = |chunk| self.land.holds(chunk) || self.copies.contains_key(&chunk);
        for (id, kept) in self.faces.seen(holds) {
            if self.players.contains_key(id) || kept.entity == except || !kept.face.alive {
                continue;
            }
            let height = if kept.face.sneaking {
                CROUCHING_HEIGHT
            } else {
                PLAYER_HEIGHT
            };
            let own = Aabb::of_entity(kept.face.position, PLAYER_WIDTH, height);
            if !own.intersects(*bounds) {
                continue;
            }
            found.push(Target {
                key: kept.entity,
                bounds: own,
                root: kept.face.vehicle.unwrap_or(kept.entity),
                pickable: true,
                alive: true,
                kind: TargetKind::Face,
            });
        }
        found
    }

    /// What a projectile's tick leaves: where it goes (`relocate`), its commit and the
    /// edges' report. One that moves commits every tick it moves.
    pub(crate) fn write_projectile(
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
}
