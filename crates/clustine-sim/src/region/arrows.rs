//! Arrows, spectral arrows and tridents (`docs/groundwork/n9-plan.md`, sections 4.2 and
//! 4.3; part S2's): their tick in D9 1.1's order, `stepMoveAndHit` and its hits, the
//! ground, `applyEffectsFromBlocks`, pick-up, the trident's loyalty and return. The
//! arithmetic is `crate::physics::arrows`'.
//!
//! **The tick** (n9 AbstractArrow 284–583): the motion `v` read first; the stuck test;
//! the fire put out in water; in the ground, the fall where the block it stuck in
//! changed and nothing holds it (three own draws), else the despawn at 1200 ticks, the
//! blocks its box is in, and no base tick; in flight, water's inertia before the move,
//! the turn towards `v`, `checkLeftOwner`, the clip and `stepMoveAndHit` along `v`, the
//! air's drag, gravity unless the step put it in the ground, then `Projectile.tick`
//! (the first tick's `projectile_shoot`, the base tick: the fluids and their push, the
//! fire counted down, the floor of the world). A trident's own part runs first
//! (n9 ThrownTrident 105–257).
//!
//! **Derived counters** (A28 section 3): the time in the ground, the despawn's `life` and
//! the shake are worked out from the `lived` the arrow landed at, so that an arrow lying
//! in the ground commits nothing. `inGroundTime` and `life` both count the ticks in the
//! ground since it landed (both are reset by `startFalling`, and `life` by nothing
//! else), so both are `lived − since`. A trident returning with no physics is the one
//! whose return count is above 0.
//!
//! **Hits** (n9 AbstractArrow 938–1110): every entity along the step, sorted by the
//! distance to its position, each in turn until the arrow is gone; a piercing arrow
//! looks again from where it is. An entity the region holds is hurt here
//! (`entity_effects.rs`' `take_hurt`); the region's own players and other regions'
//! faces are creative and refuse every hurt, and the arrow turns back from them; an
//! entity seen in a copy is hurt by its holder (`EntityUse::Hurt`) and the arrow is gone,
//! but at an enderman that rides nothing, which refuses and leaves it flying on.
//!
//! **Pick-up** is the players' (`Player.touch` → `playerTouch`): the region's own players
//! take arrows of its own chunks in their reach, in the order of their ids
//! ([`Region::players_touch_arrows`], which the region's tick calls after the items').
//!
//! Named differences:
//! - The arrow count of a living target (data 12) is not kept: no body part holds it
//!   yet (the lead's in N9f), and [`Region::count_arrow`] does nothing until one does.
//! - Punch's push of a target across a line is not carried: the hurt has no field for
//!   a push (`HurtBy.knockback` is a knockback's).
//! - An arrow's own hurt by fire or burning (`Projectile.hurtServer`, which only marks it
//!   hurt) and its splash into water (`doWaterSplashEffect`, its sound's draws) are left
//!   out; so are the enchantments' block hits and post-attack effects, which no
//!   enchantment of N9's has for a bow.
//! - The hit sound across a line is played at the target by its holder; here, at the
//!   arrow as the game does.
//! - The owner's `setLastHurtMob` is kept for a player of the region only: a mob keeps
//!   no last hurt mob.
//! - An owner the region does not have is none, as the game's `getOwner()` is null for
//!   an owner its level has not loaded: an arrow of such an owner can be picked up by
//!   anyone; a returning trident reads its owner's eyes from the faces and the world's
//!   players where the region has neither (section 4.3).
//! - A hopper's `entityInside` takes items only, and an arrow none.

use clustine_blocks::data::{entity_in_tag, has_lava, has_water, is_class};
use clustine_data::{
    BlockClass, EntityTypeInfo, SOUND_EVENTS, blocks, entity_types, items, sounds,
};
use clustine_world::components::{Component, EffectInstance, Kind};
use clustine_world::{
    Arrow, ArrowKind, BlockPos, Causer, ChunkPos, Double, Entity, EntityChange, EntityData,
    EntityField, EntityId, EntityUse, Float, HurtBy, InGround, ItemStack, Pickup,
};

use super::Region;
use super::items::{CartMemory, Memory, View};
use super::projectiles::{Target, TargetKind};
use super::random_ticks::Random;
use super::vibrations::{Context, Source, Vibration};
use crate::api::{PlayerEvent, RegionEvent, TickOutput};
use crate::items::enchantments;
use crate::physics::aabb::Aabb;
use crate::physics::arrows::{self as maths, InsideEffect};
use crate::physics::body::{Body, Movement};
use crate::physics::collide::{self, Context as Collision};
use crate::physics::entities::Mover;
use crate::physics::item::{Inside, SOUND_SOURCE_NEUTRAL, check_inside_blocks, handed_back};
use crate::physics::thrown::{self, BlockHit};
use crate::physics::{self, Vec3, fluids};

#[cfg(test)]
#[path = "arrows_n9_tests.rs"]
mod arrows_n9_tests;

/// How far below the world's floor an entity is gone (`checkBelowWorld`).
const BELOW_THE_WORLD: f64 = 64.0;

/// The entity event of a tipped arrow that has lain long enough in the ground: its
/// client's coloured particles (`Arrow.handleEntityEvent`).
const TIPPED_FADED: u8 = 0;

/// The eyes of a player standing, and crouching, above their feet.
const PLAYER_EYES: f64 = 1.62;
const CROUCHING_EYES: f64 = 1.27;

/// What `spawnAtLocation(stack, 0.1f)` raises a dropped stack by.
const DROP_RAISE: f64 = 0.100_000_001_490_116_12;

/// The sound an arrow's hits play by default (`SoundEvents.ARROW_HIT`).
const ARROW_HIT: &str = "minecraft:entity.arrow.hit";

/// An arrow as its tick works on it.
struct Flight {
    key: EntityId,
    kind: u16,
    hops: u32,
    chunk: ChunkPos,
    position: Vec3,
    motion: Vec3,
    yaw: f32,
    pitch: f32,
    fire: i32,
    /// `tickCount` in this tick, which the hit test's margin grows with.
    tick_count: u32,
    /// The `lived` its tick ends at.
    lived: u32,
    arrow: Arrow,
    /// Its own stream, on from what it has drawn in this tick.
    own: Random,
    /// `wasTouchingWater`, as its last base tick left it.
    in_water: bool,
    removed: bool,
    /// Whether a hit in this tick asks for it to be sent at once (`needsSync`).
    needs_sync: bool,
}

impl Flight {
    fn trident(&self) -> bool {
        i32::from(self.kind) == entity_types::TRIDENT
    }

    /// `isNoPhysics`: a trident on its way back.
    fn no_physics(&self) -> bool {
        matches!(self.arrow.of_kind, ArrowKind::Trident { returning, .. } if returning > 0)
    }

    fn bounds(&self) -> Aabb {
        super::projectile_hits::box_of(self.kind, self.position)
    }

    /// The arrow as an entity, where its tick has it (what a block hit hands on).
    fn entity(&self, base: &Entity) -> Entity {
        Entity {
            position: self.position,
            motion: self.motion,
            yaw: self.yaw,
            pitch: self.pitch,
            fire_ticks: self.fire,
            lived: self.lived,
            data: EntityData::Arrow(Box::new(self.arrow.clone())),
            ..base.clone()
        }
    }

    /// The arrow as a hurt names what dealt it.
    fn causer(&self) -> Causer {
        Causer {
            key: self.key,
            kind: self.kind,
            position: self.position.map(Double),
            player: None,
            living: false,
        }
    }
}

/// An entity along an arrow's step, and where the step meets it.
#[derive(Debug, Clone, Copy)]
struct Struck {
    target: Target,
    at: Vec3,
    /// Its position (`Entity.position()`), which the hits are sorted by.
    position: Vec3,
}

/// What an arrow's walk through the blocks met: each block's state and place, the step
/// it was met in, and whether its shape and its fluid were met.
struct ArrowInside<'a> {
    body: &'a Body,
    met: Vec<(clustine_data::BlockState, BlockPos, i32, bool, bool)>,
}

impl Inside for ArrowInside<'_> {
    fn body(&self) -> &Body {
        self.body
    }

    fn removed(&self) -> bool {
        false
    }

    fn met(
        &mut self,
        state: clustine_data::BlockState,
        pos: BlockPos,
        step: i32,
        block: bool,
        fluid: bool,
    ) {
        self.met.push((state, pos, step, block, fluid));
    }
}

/// An owner of a returning trident, as the region sees it: where its eyes are, whether
/// it is alive, whether it is a player, and its width.
struct Seen {
    eyes: Vec3,
    alive: bool,
    player: bool,
    width: f32,
}

/// The id of the sound `name`, the arrow's hit where the game has none of that name.
fn sound_id(name: &str) -> i32 {
    SOUND_EVENTS
        .iter()
        .position(|known| *known == name)
        .and_then(|at| i32::try_from(at).ok())
        .unwrap_or(sounds::ENTITY_ARROW_HIT)
}

/// `MobEffectInstance.withScaledDuration(scale)`: an endless or ended effect as it is,
/// any other at `max(floor(duration · scale), 1)`.
fn scaled(mut instance: EffectInstance, scale: f32) -> EffectInstance {
    if instance.duration != -1 && instance.duration != 0 {
        instance.duration = ((instance.duration as f32 * scale).floor() as i32).max(1);
    }
    instance
}

/// What `Arrow.doPostHurtEffects` gives: the effects of the pickup stack's
/// `potion_contents`, its potion's and then its own, each scaled by the stack's
/// `potion_duration_scale` (the item's default where the stack has none, 1 without).
pub(crate) fn potion_effects(stack: &ItemStack) -> Vec<EffectInstance> {
    let Some(Component::PotionContents(contents)) = stack.components.get(Kind::PotionContents)
    else {
        return Vec::new();
    };
    let scale = match stack.components.get(Kind::PotionDurationScale) {
        Some(Component::PotionDurationScale(Float(scale))) => *scale,
        _ if stack.components.removes(Kind::PotionDurationScale) => 1.0,
        _ => clustine_data::ItemComponents::of(stack.item)
            .potion_duration_scale
            .unwrap_or(1.0),
    };
    let mut effects = Vec::new();
    if let Some(potion) = contents
        .potion
        .as_deref()
        .and_then(clustine_data::potions::Potion::named)
    {
        for effect in potion.effects {
            effects.push(scaled(
                EffectInstance {
                    effect: effect.effect.to_owned(),
                    amplifier: effect.amplifier,
                    duration: effect.duration,
                    ambient: effect.ambient,
                    visible: effect.visible,
                    show_icon: effect.show_icon,
                },
                scale,
            ));
        }
    }
    for effect in &contents.custom_effects {
        effects.push(scaled(effect.clone(), scale));
    }
    effects
}

/// Whether a stack's `potion_contents` holds anything (`!PotionContents.EMPTY`).
fn has_potion(stack: &ItemStack) -> bool {
    matches!(
        stack.components.get(Kind::PotionContents),
        Some(Component::PotionContents(contents)) if *contents != clustine_world::components::PotionContents::default()
    )
}

impl Region {
    /// The tick of the arrow or trident `entity` of the loaded chunk `position`
    /// (`AbstractArrow.tick`, `ThrownTrident.tick`); it commits through
    /// `write_projectile`.
    pub(crate) fn tick_arrow(
        &mut self,
        entity: Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let EntityData::Arrow(record) = &entity.data else {
            return;
        };
        let key = entity.key;
        let stream = self.entity_stream(key);
        let start = stream.random.clone();
        let mut flight = Flight {
            key,
            kind: entity.kind,
            hops: entity.hops,
            chunk: position,
            position: entity.position,
            motion: entity.motion,
            yaw: entity.yaw,
            pitch: entity.pitch,
            fire: entity.fire_ticks,
            tick_count: entity.tick_count().saturating_add(1),
            lived: entity.lived.saturating_add(1),
            arrow: (**record).clone(),
            own: stream.random,
            in_water: self
                .entity_memory
                .get(&key)
                .is_some_and(|memory| memory.cart.in_water),
            removed: false,
            needs_sync: false,
        };
        self.arrow_tick(&entity, &mut flight, output);
        self.entity_drew(key, flight.own.draws_since(&start));
        if flight.removed {
            self.entity_memory.remove(&key);
            self.remove_entity(key, flight.hops, position, output);
            return;
        }
        self.entity_memory.insert(
            key,
            Memory {
                cart: CartMemory {
                    in_water: flight.in_water,
                    ..CartMemory::default()
                },
                no_physics: flight.no_physics(),
                ..Memory::default()
            },
        );
        let after = flight.entity(&entity);
        // `write_projectile` commits a move, a turn and the record: the fire, which a
        // projectile's tick changes too, is committed here where the arrow stays.
        if after.fire_ticks != entity.fire_ticks && after.chunk() == position {
            output.entity_changes.push((
                position,
                EntityChange::Set(key, vec![EntityField::FireTicks(after.fire_ticks)]),
            ));
        }
        self.write_projectile_synced(&entity, after, position, flight.needs_sync, output);
    }

    /// `AbstractArrow.tick`, with `ThrownTrident.tick`'s part first.
    fn arrow_tick(&mut self, entity: &Entity, flight: &mut Flight, output: &mut TickOutput) {
        let started = entity.lived;
        if flight.trident() {
            let in_ground_time = flight
                .arrow
                .in_ground
                .map_or(0, |ground| started.saturating_sub(ground.since));
            self.trident_before(flight, in_ground_time, output);
            if flight.removed {
                return;
            }
        }
        let no_physics = flight.no_physics();
        let v = flight.motion;
        let at = physics::containing(flight.position);
        let state = self.block(at).unwrap_or(blocks::AIR);
        // The stuck test: inside its block's collision shape, it is in the ground.
        if !no_physics && !state.props().is_air() {
            let context = Collision {
                bottom: flight.position[1],
                fall_distance: entity.fall_distance,
                falling_block: false,
                noclip: None,
            };
            let inside = collide::collision_shape(state, at, context).is_some_and(|(shape, _)| {
                shape
                    .boxes()
                    .iter()
                    .any(|bounds| bounds.contains(flight.position))
            });
            if inside {
                flight.motion = [0.0; 3];
                if flight.arrow.in_ground.is_none() {
                    // It counts this tick in the ground already; it hit no block to
                    // fall from, so the block it is in stands for the one it stuck in
                    // (`shouldFall` holds it as long as that is there).
                    flight.arrow.in_ground = Some(InGround {
                        state: state.0,
                        since: started,
                    });
                }
            }
        }
        // `isInWaterOrRain()`: no rain falls in Clustine.
        if flight.in_water {
            flight.fire = flight.fire.min(0);
        }
        if let Some(ground) = flight.arrow.in_ground
            && !no_physics
        {
            self.arrow_in_ground(flight, ground, state, output);
            return;
        }
        self.arrow_flies(entity, flight, v, no_physics, output);
    }

    /// The tick of an arrow in the ground, that stuck in `ground` and whose block is
    /// `state` now.
    fn arrow_in_ground(
        &mut self,
        flight: &mut Flight,
        ground: InGround,
        state: clustine_data::BlockState,
        output: &mut TickOutput,
    ) {
        let in_ground = flight.lived.saturating_sub(ground.since);
        if state.0 != ground.state && self.arrow_should_fall(flight) {
            // `startFalling`.
            let draws = [
                flight.own.next_float(),
                flight.own.next_float(),
                flight.own.next_float(),
            ];
            flight.motion = maths::start_falling(flight.motion, draws);
            flight.arrow.in_ground = None;
            // Its `inGroundTime` stays until its next tick, whose trident's part reads it
            // first: it would have dealt its damage then.
            if let ArrowKind::Trident { dealt_damage, .. } = &mut flight.arrow.of_kind
                && in_ground > maths::TRIDENT_SETTLES
            {
                *dealt_damage = true;
            }
        } else {
            // `tickDespawn`: never for a trident of loyalty that may be picked up.
            let kept = flight.trident()
                && flight.arrow.pickup == Pickup::Allowed
                && self.trident_loyalty(&flight.arrow) > 0;
            if !kept && in_ground >= maths::DESPAWN {
                flight.removed = true;
                return;
            }
        }
        let here = flight.position;
        self.arrow_effects_from_blocks(flight, here, here, output);
        if flight.removed {
            return;
        }
        // `Arrow.tick`: a tipped arrow long in the ground is a plain one.
        if i32::from(flight.kind) == entity_types::ARROW
            && let Some(ground) = flight.arrow.in_ground
        {
            let time = flight.lived.saturating_sub(ground.since);
            if time != 0 && has_potion(&flight.arrow.pickup_stack) && time >= maths::TIPPED_FADES {
                output.events.push(RegionEvent::EntityEvent {
                    entity: flight.key,
                    chunk: flight.chunk,
                    event: TIPPED_FADED,
                });
                flight.arrow.pickup_stack = ItemStack::new(items::ARROW, 1);
            }
        }
    }

    /// The tick of an arrow in flight, which moved by `v` as its tick began.
    fn arrow_flies(
        &mut self,
        entity: &Entity,
        flight: &mut Flight,
        v: Vec3,
        no_physics: bool,
        output: &mut TickOutput,
    ) {
        let from = flight.position;
        if flight.in_water {
            let inertia = if flight.trident() {
                maths::TRIDENT_WATER_INERTIA
            } else {
                maths::WATER_INERTIA
            };
            flight.motion = maths::inertia(flight.motion, inertia);
        }
        [flight.yaw, flight.pitch] = maths::turn(flight.yaw, flight.pitch, v, no_physics);
        // `checkLeftOwner`, once in the tick: `Projectile.tick`'s later call finds it
        // checked.
        if !flight.arrow.left_owner {
            flight.arrow.left_owner = self.arrow_left_owner(flight);
        }
        let to = physics::add(from, v);
        if no_physics {
            flight.position = to;
            self.arrow_effects_from_blocks(flight, from, to, output);
        } else {
            let collision = Collision {
                bottom: from[1],
                fall_distance: entity.fall_distance,
                falling_block: false,
                noclip: None,
            };
            let view = View {
                region: self,
                home: flight.chunk,
            };
            let block = thrown::clip_blocks(&view, collision, from, to);
            self.step_move_and_hit(flight, block, to, output);
        }
        if flight.removed {
            return;
        }
        if !flight.in_water {
            flight.motion = maths::inertia(flight.motion, maths::AIR_DRAG);
        }
        if !no_physics && flight.arrow.in_ground.is_none() {
            flight.motion = maths::gravity(flight.motion);
        }
        // `Projectile.tick`: its first tick's `projectile_shoot`, of its owner.
        if !flight.arrow.has_been_shot {
            let owner = flight
                .arrow
                .owner
                .filter(|owner| self.unit_root(*owner).is_some());
            self.vibrate(
                Vibration::PROJECTILE_SHOOT,
                flight.position,
                Context::of(owner.map(Source::of), None),
            );
            flight.arrow.has_been_shot = true;
        }
        // `baseTick`: the fluids it is in and their push, its fire counted down, and
        // below the world it is gone.
        let view = View {
            region: self,
            home: flight.chunk,
        };
        let found = fluids::update(&view, &flight.bounds());
        flight.motion = fluids::push(&found, flight.motion);
        flight.in_water = found.in_water();
        if flight.fire > 0 {
            flight.fire -= 1;
        }
        let min_y = self
            .chunks
            .get(&flight.chunk)
            .map_or(-64, |chunk| chunk.min_y());
        if flight.position[1] < f64::from(min_y) - BELOW_THE_WORLD {
            flight.removed = true;
        }
    }

    /// `stepMoveAndHit(blockHit)`: the entities along the step from where the arrow is
    /// to `block`'s place (or `to` for none), hit in order of their distance, then, with
    /// none, the block. Either hit asks for the arrow to be sent at once (`needsSync`),
    /// so that clients see a refused arrow turn back, or one land, when it does.
    fn step_move_and_hit(
        &mut self,
        flight: &mut Flight,
        block: Option<BlockHit>,
        to: Vec3,
        output: &mut TickOutput,
    ) {
        let end_of_step = block.map_or(to, |hit| hit.location);
        loop {
            if flight.removed {
                return;
            }
            let start = flight.position;
            let hits = self.arrow_hits(flight, start, end_of_step);
            let end = hits.first().map_or(end_of_step, |hit| hit.at);
            flight.position = end;
            self.arrow_effects_from_blocks(flight, start, end, output);
            if hits.is_empty() {
                if !flight.removed
                    && let Some(hit) = block
                {
                    self.arrow_hit_block(flight, hit, output);
                    flight.needs_sync = true;
                }
                return;
            }
            if flight.removed {
                return;
            }
            // `hitTargetsOrDeflectSelf`: no entity of Clustine's deflects, so each is hit
            // until the arrow is gone (a refusal's turn back is the arrow's own).
            for hit in hits {
                self.arrow_hit_entity(flight, hit, output);
                if flight.removed {
                    break;
                }
            }
            flight.needs_sync = true;
            if flight.arrow.pierce_level == 0 {
                return;
            }
        }
    }

    /// `findHitEntities(from, to)`: every entity the step from `from` to `to` meets
    /// (`getManyEntityHitResult(…, false, false)`), or for a trident the nearest, and none
    /// once it has dealt its damage (`findHitEntity`); sorted by the distance from `from`
    /// to each one's position.
    fn arrow_hits(&self, flight: &Flight, from: Vec3, to: Vec3) -> Vec<Struck> {
        let search = flight
            .bounds()
            .expand_towards(flight.motion)
            .inflate([1.0; 3]);
        let margin = f64::from(thrown::margin(flight.tick_count));
        let owner_root = flight.arrow.owner.and_then(|owner| self.unit_root(owner));
        let can_hit = |target: &Target| {
            target.pickable
                && target.alive
                && (flight.arrow.left_owner || owner_root.is_none_or(|root| target.root != root))
                && !flight.arrow.pierced.contains(&target.key)
        };
        let targets: Vec<Target> = self
            .targets(&search, flight.key)
            .into_iter()
            .filter(can_hit)
            .collect();
        let mut hits = Vec::new();
        if let ArrowKind::Trident { dealt_damage, .. } = flight.arrow.of_kind {
            if dealt_damage {
                return hits;
            }
            // `getEntityHitResult`: the nearest whose box, grown by the margin, the step
            // meets.
            let mut nearest = f64::MAX;
            for target in targets {
                let Some(at) = target.bounds.inflate([margin; 3]).clip(from, to) else {
                    continue;
                };
                let distance = physics::distance_sqr(from, at);
                if distance < nearest {
                    nearest = distance;
                    hits = vec![Struck {
                        target,
                        at,
                        position: self.target_position(&target),
                    }];
                }
            }
            return hits;
        }
        let view = View {
            region: self,
            home: flight.chunk,
        };
        for target in targets {
            let at = if let Some(at) = target.bounds.clip(from, to) {
                Some(at)
            } else if margin > 0.0 {
                // The grown box, and from where the step enters it a clear line to the
                // box's middle that enters the box itself.
                target
                    .bounds
                    .inflate([margin; 3])
                    .clip(from, to)
                    .and_then(|entry| {
                        let middle = target.bounds.center();
                        let context = Collision {
                            bottom: entry[1],
                            fall_distance: 0.0,
                            falling_block: false,
                            noclip: None,
                        };
                        let towards = thrown::clip_blocks(&view, context, entry, middle)
                            .map_or(middle, |hit| hit.location);
                        target.bounds.clip(entry, towards).map(|_| entry)
                    })
            } else {
                None
            };
            if let Some(at) = at {
                hits.push(Struck {
                    target,
                    at,
                    position: self.target_position(&target),
                });
            }
        }
        // `ArrayList.sort` is stable: ties keep the lookup's order.
        hits.sort_by(|a, b| {
            physics::distance_sqr(from, a.position)
                .total_cmp(&physics::distance_sqr(from, b.position))
        });
        hits
    }

    /// Where a target of the hit test stands (`Entity.position()`).
    fn target_position(&self, target: &Target) -> Vec3 {
        match target.kind {
            TargetKind::Here => self
                .find_entity(target.key)
                .map(|(_, record)| record.position),
            TargetKind::Copy => self
                .copied_entity(target.key)
                .map(|(_, record)| record.position),
            TargetKind::Player => self.player_with_entity(target.key).and_then(|id| {
                let feet = self.players.get(&id)?.pose.position;
                Some([feet.x, feet.y, feet.z])
            }),
            TargetKind::Face => self
                .faces
                .kept
                .values()
                .find(|kept| kept.entity == target.key)
                .map(|kept| kept.face.position),
        }
        .unwrap_or_else(|| {
            let bounds = target.bounds;
            [
                (bounds.min[0] + bounds.max[0]) / 2.0,
                bounds.min[1],
                (bounds.min[2] + bounds.max[2]) / 2.0,
            ]
        })
    }

    /// `Projectile.isOutsideOwnerCollisionRange`: no part of the owner's unit meets the
    /// arrow's box grown along its motion and by a block.
    fn arrow_left_owner(&self, flight: &Flight) -> bool {
        let Some(root) = flight.arrow.owner.and_then(|owner| self.unit_root(owner)) else {
            return true;
        };
        let reach = flight
            .bounds()
            .expand_towards(flight.motion)
            .inflate([1.0; 3]);
        !self
            .targets(&reach, flight.key)
            .iter()
            .any(|target| target.pickable && target.root == root)
    }

    /// `shouldFall`: nothing collides with a box of 0.06 about the arrow's position.
    fn arrow_should_fall(&self, flight: &Flight) -> bool {
        let bounds = Aabb::new(flight.position, flight.position).inflate([maths::FALL_MARGIN; 3]);
        let view = View {
            region: self,
            home: flight.chunk,
        };
        let context = Collision {
            bottom: bounds.min[1],
            fall_distance: 0.0,
            falling_block: false,
            noclip: None,
        };
        let mover = Mover {
            key: flight.key,
            root: flight.key,
            like_a_vehicle: false,
            happy_ghast: false,
        };
        collide::no_collision(&view, context, &mover, &bounds)
    }

    /// `applyEffectsFromBlocks` along the arrow's way from `from` to `to`: each block
    /// its box meets, a plate, tripwire or a detector rail counting it, a wooden button
    /// pressed, fire and lava setting it alight and water putting it out; then the
    /// sound of a fire put out, and a fire that did not burn forgotten.
    fn arrow_effects_from_blocks(
        &mut self,
        flight: &mut Flight,
        from: Vec3,
        to: Vec3,
        output: &mut TickOutput,
    ) {
        if flight.removed {
            return;
        }
        let info = EntityTypeInfo::of(i32::from(flight.kind));
        let body = Body {
            position: to,
            motion: flight.motion,
            on_ground: false,
            fall_distance: 0.0,
            width: info.width,
            height: info.height,
            old_position: from,
            horizontal_collision: false,
            vertical_collision: false,
            vertical_collision_below: false,
            supporting: None,
            on_ground_no_blocks: false,
            stuck_speed: [0.0; 3],
            movements: Vec::new(),
            in_water: flight.in_water,
            sync_position: false,
            falling_block: false,
            item: false,
            fell_on: None,
            noclip: None,
            mover: Mover {
                key: flight.key,
                root: flight.key,
                like_a_vehicle: false,
                happy_ghast: false,
            },
            steps: None,
        };
        let met = {
            let view = View {
                region: self,
                home: flight.chunk,
            };
            let mut inside = ArrowInside {
                body: &body,
                met: Vec::new(),
            };
            check_inside_blocks(
                &view,
                &mut inside,
                &[Movement {
                    from,
                    to,
                    asked: None,
                }],
            );
            inside.met
        };
        let own_box = body.bounds();
        let toucher = super::pressed::Toucher {
            key: flight.key,
            bounds: own_box,
            living: false,
        };
        let mut effects = Vec::new();
        for (state, pos, step, block, fluid) in met {
            if block {
                if is_class(state, BlockClass::ButtonBlock) {
                    self.arrow_inside_button(pos, state, (flight.key, own_box), output);
                } else if handed_back(state) && !is_class(state, BlockClass::HopperBlock) {
                    self.entity_touched(pos, &toucher, output);
                } else if is_class(state, BlockClass::BaseFireBlock) {
                    effects.push((step, InsideEffect::FireIgnite));
                } else if is_class(state, BlockClass::PowderSnowBlock)
                    || is_class(state, BlockClass::LayeredCauldronBlock)
                {
                    effects.push((step, InsideEffect::Extinguish));
                } else if is_class(state, BlockClass::LavaCauldronBlock) {
                    effects.push((step, InsideEffect::LavaIgnite));
                }
            }
            if fluid {
                if has_water(state) {
                    effects.push((step, InsideEffect::Extinguish));
                } else if has_lava(state) {
                    effects.push((step, InsideEffect::LavaIgnite));
                }
            }
        }
        let before = flight.fire;
        for effect in maths::inside_effects(&effects) {
            flight.fire = maths::after_inside(flight.fire, effect);
        }
        if before > 0 && flight.fire <= 0 {
            // `playEntityOnFireExtinguishedSound`, its pitch of two own draws.
            let first = flight.own.next_float();
            let second = flight.own.next_float();
            let pitch = 1.6 + (first - second) * 0.4;
            self.arrow_sound(
                flight,
                sounds::ENTITY_GENERIC_EXTINGUISH_FIRE,
                0.7,
                pitch,
                output,
            );
        }
        let ignited = flight.fire > before;
        if flight.fire <= 0 && !ignited {
            flight.fire = 0;
        }
    }

    /// `Entity.playSound` of the arrow, where it is.
    fn arrow_sound(
        &self,
        flight: &Flight,
        sound: i32,
        volume: f32,
        pitch: f32,
        output: &mut TickOutput,
    ) {
        let [x, y, z] = flight.position;
        output.events.push(RegionEvent::Sound {
            sound,
            source: SOUND_SOURCE_NEUTRAL,
            position: clustine_world::Vec3::new(x, y, z),
            volume,
            pitch,
            except: None,
        });
    }

    /// `onHitBlock` and `Projectile.onHit` of a block hit: the block's own hook, the
    /// arrow moved back along its motion and stopped, its sound, in the ground, its
    /// shake, crit and pierce off, and `projectile_land` at the block.
    fn arrow_hit_block(&mut self, flight: &mut Flight, hit: BlockHit, output: &mut TickOutput) {
        let state = self.block(hit.pos).unwrap_or(blocks::AIR);
        if let Some((_, base)) = self.find_entity(flight.key) {
            let now = flight.entity(&base.clone());
            self.projectile_hit(&now, state, &hit, output);
        }
        flight.position = maths::move_back(flight.position, flight.motion);
        flight.motion = [0.0; 3];
        let pitch = maths::hit_pitch(flight.own.next_float());
        let sound = sound_id(&flight.arrow.hit_sound);
        self.arrow_sound(flight, sound, 1.0, pitch, output);
        flight.arrow.in_ground = Some(InGround {
            state: state.0,
            since: flight.lived,
        });
        flight.arrow.shake_until = flight.lived.saturating_add(maths::SHAKE_TIME);
        flight.arrow.crit = false;
        flight.arrow.pierce_level = 0;
        ARROW_HIT.clone_into(&mut flight.arrow.hit_sound);
        flight.arrow.pierced.clear();
        let after = self.block(hit.pos);
        self.vibrate_at(
            Vibration::PROJECTILE_LAND,
            hit.pos,
            Context::of(Some(Source::of(flight.key)), after),
        );
    }

    /// The hurt an arrow of `flight` deals `amount` by, as it crosses a line: of the
    /// type `kind`, dealt by the arrow, caused by its owner where the region has them
    /// (else by the arrow itself), pushing along the arrow's motion.
    fn arrow_hurt(&self, flight: &Flight, damage_type: &str, amount: f32) -> HurtBy {
        let direct = flight.causer();
        let cause = flight
            .arrow
            .owner
            .and_then(|owner| self.thrower(owner))
            .map_or_else(
                || direct.clone(),
                |owner| Causer {
                    key: owner.key,
                    kind: owner.kind,
                    position: owner.from,
                    player: owner.player,
                    living: true,
                },
            );
        HurtBy {
            damage_type: damage_type.to_owned(),
            cause: Some(cause),
            direct: Some(direct),
            position: None,
            projectile_motion: Some(flight.motion.map(Double)),
            amount: Float(amount),
            knockback: None,
            fire_before: 0,
            fire_after: 0,
            effects: Vec::new(),
            arrow: false,
            extras: Vec::new(),
            sound: None,
            hit: None,
        }
    }

    /// The effects an arrow gives a living target its hurt took on: its potion's, and
    /// a spectral arrow's glowing.
    fn arrow_effects(flight: &Flight) -> Vec<EffectInstance> {
        match flight.arrow.of_kind {
            ArrowKind::Arrow => potion_effects(&flight.arrow.pickup_stack),
            ArrowKind::Spectral { glowing_duration } => vec![EffectInstance {
                effect: "minecraft:glowing".to_owned(),
                amplifier: 0,
                duration: glowing_duration,
                ambient: false,
                visible: true,
                show_icon: true,
            }],
            ArrowKind::Trident { .. } => Vec::new(),
        }
    }

    /// The region's entity `key` burns for `ticks` from now (`setRemainingFireTicks`).
    fn arrow_set_fire(&mut self, key: EntityId, ticks: i32, output: &mut TickOutput) {
        let Some((chunk, record)) = self.find_entity(key) else {
            return;
        };
        if record.fire_ticks != ticks {
            self.commit_entity(chunk, key, vec![EntityField::FireTicks(ticks)], output);
        }
    }

    /// `onHitEntity` of an arrow or a trident, and `Projectile.onHit`'s
    /// `projectile_land` after it.
    fn arrow_hit_entity(&mut self, flight: &mut Flight, hit: Struck, output: &mut TickOutput) {
        if flight.trident() {
            self.trident_hit_entity(flight, hit, output);
        } else {
            self.arrow_hits_entity(flight, hit, output);
        }
        self.vibrate(
            Vibration::PROJECTILE_LAND,
            hit.at,
            Context::of(Some(Source::of(flight.key)), None),
        );
    }

    /// `AbstractArrow.onHitEntity` (n9 AbstractArrow 938–1250), in the game's order:
    /// the damage by speed, the pierced set (full: gone silently), the crit's draw, the
    /// owner's last hurt mob, the fire before the hurt, the hurt and what follows it.
    fn arrow_hits_entity(&mut self, flight: &mut Flight, hit: Struck, output: &mut TickOutput) {
        let target = hit.target;
        let speed = physics::length_sqr(flight.motion).sqrt() as f32;
        let mut damage = flight.arrow.base_damage.0;
        if let Some(weapon) = &flight.arrow.weapon {
            damage = maths::power_damage(damage, enchantments::level(weapon, "minecraft:power"));
        }
        let mut amount = maths::damage_by_speed(speed, damage);
        let pierce = flight.arrow.pierce_level;
        if pierce > 0 {
            // `size >= pierce + 1`.
            if flight.arrow.pierced.len() > usize::from(pierce) {
                flight.removed = true;
                return;
            }
            flight.arrow.pierced.push(target.key);
        }
        if flight.arrow.crit {
            let drawn = flight.own.next_int(maths::crit_bound(amount));
            amount = maths::crit_damage(amount, drawn);
        }
        self.owner_hurt_mob(flight, target.key);
        let burning = flight.fire > 0;
        let mut hurt = self.arrow_hurt(flight, "minecraft:arrow", amount as f32);
        hurt.effects = Self::arrow_effects(flight);
        hurt.arrow = pierce == 0;
        match target.kind {
            TargetKind::Here => {
                let Some((_, record)) = self.find_entity(target.key) else {
                    return;
                };
                let record = record.clone();
                let fire_before = record.fire_ticks;
                if burning {
                    // `igniteForSeconds(5)`.
                    self.arrow_set_fire(target.key, fire_before.max(100), output);
                }
                let took = self.arrow_hurt_here(&record, &hurt, output);
                if took {
                    if let Some(weapon) = &flight.arrow.weapon {
                        let level = enchantments::level(weapon, "minecraft:punch");
                        self.punch(target.key, flight.motion, level, output);
                    }
                    let pitch = maths::hit_pitch(flight.own.next_float());
                    let sound = sound_id(&flight.arrow.hit_sound);
                    self.arrow_sound(flight, sound, 1.0, pitch, output);
                    if pierce == 0 {
                        flight.removed = true;
                    }
                } else if i32::from(record.kind) != entity_types::ENDERMAN {
                    self.arrow_set_fire(target.key, fire_before, output);
                    self.arrow_refused(flight, output);
                }
            }
            TargetKind::Copy => {
                let Some((_, record)) = self.copied_entity(target.key) else {
                    return;
                };
                if burning {
                    hurt.fire_before = 100;
                }
                let flies_on = i32::from(record.kind) == entity_types::ENDERMAN
                    && record.riding.vehicle.is_none();
                if !flies_on {
                    let pitch = maths::hit_pitch(flight.own.next_float());
                    let sound = sound_id(&flight.arrow.hit_sound);
                    hurt.sound = Some((sound, Float(1.0), Float(pitch)));
                }
                self.send_on_entity(target.key, EntityUse::Hurt(Box::new(hurt)));
                if !flies_on {
                    flight.removed = true;
                }
            }
            // A creative player refuses every hurt: the fire set before it is taken back,
            // and the arrow turns back.
            TargetKind::Player | TargetKind::Face => self.arrow_refused(flight, output),
        }
    }

    /// The hurt `hurt` of the region's entity `record` by an arrow or a trident: an armour
    /// stand is killed outright (`#always_kills_armor_stands`), any other as its kind
    /// takes it. Whether it took.
    fn arrow_hurt_here(&mut self, record: &Entity, hurt: &HurtBy, output: &mut TickOutput) -> bool {
        if let EntityData::ArmorStand { flags, .. } = record.data {
            if flags & (super::stands::INVISIBLE | super::stands::MARKER) != 0 {
                return false;
            }
            let creative = hurt
                .cause
                .as_ref()
                .is_some_and(|cause| cause.player.is_some());
            if creative {
                // A creative player's: it breaks, dropping nothing.
                self.hit_armor_stand(record.key, true, output);
            } else {
                // `brokenByPlayer`: the stand's item where it stood, then what it wears.
                self.pop_resource_at(
                    physics::containing(record.position),
                    ItemStack::new(items::ARMOR_STAND, 1),
                    output,
                );
                self.broken_by_anything(record, output);
                self.kill_stand(record.key, output);
            }
            return true;
        }
        self.take_hurt(record, hurt, output)
    }

    /// `doKnockback` of the region's mob `key` by an arrow moving by `motion` from a bow
    /// of Punch `level`: `push(x, 0.1, z)` by what its knockback resistance leaves.
    fn punch(&mut self, key: EntityId, motion: Vec3, level: i32, output: &mut TickOutput) {
        let knockback = maths::punch_knockback(level);
        if knockback <= 0.0 {
            return;
        }
        self.struck_between_ticks(key, output, |struck| {
            let resistance = struck.attribute(clustine_data::attributes::KNOCKBACK_RESISTANCE);
            if let Some(push) = maths::punch_push(motion, knockback, resistance) {
                *struck.motion = physics::add(*struck.motion, [push[0], 0.1, push[2]]);
            }
        });
    }

    /// A hurt refused (`projectileReceivesSideEffectsOnHit(false)`): the arrow turns back
    /// at a tenth of its speed (`deflect(REVERSE, …, 0.2)`, one own draw), and one left
    /// almost still is gone, dropped where it may be picked up.
    fn arrow_refused(&mut self, flight: &mut Flight, output: &mut TickOutput) {
        let draw = flight.own.next_float();
        let (motion, yaw) = maths::reverse(flight.motion, maths::ARROW_BOUNCE, flight.yaw, draw);
        flight.motion = motion;
        flight.yaw = yaw;
        if physics::length_sqr(flight.motion) < 1.0e-7 {
            if flight.arrow.pickup == Pickup::Allowed {
                let [x, y, z] = flight.position;
                self.spawn_item(
                    [x, y + DROP_RAISE, z],
                    flight.arrow.pickup_stack.clone(),
                    output,
                );
            }
            flight.removed = true;
        }
    }

    /// `LivingEntity.setLastHurtMob(target)` of the arrow's owner, before the hurt and
    /// whatever its outcome: kept for a player of the region (the module's note).
    fn owner_hurt_mob(&mut self, flight: &Flight, target: EntityId) {
        let Some(owner) = flight.arrow.owner else {
            return;
        };
        let Some(id) = self.player_with_entity(owner) else {
            return;
        };
        let now = self.clock.game_time;
        if let Some(player) = self.players.get_mut(&id) {
            player.last_hurt_mob = Some((target, now));
        }
    }

    /// `ThrownTrident.onHitEntity` (n9 ThrownTrident 298–399): 8 and Impaling, its damage
    /// dealt before the hurt, then, but at an enderman that refused it, `trident_hit`
    /// and the bounce (`deflect(REVERSE, …, (0.02, 0.2, 0.02))`, one own draw), whether
    /// or not the hurt took. Across a line only the hurt is sent.
    fn trident_hit_entity(&mut self, flight: &mut Flight, hit: Struck, output: &mut TickOutput) {
        let target = hit.target;
        let kind = match target.kind {
            TargetKind::Here => self.find_entity(target.key).map(|(_, record)| record.kind),
            TargetKind::Copy => self
                .copied_entity(target.key)
                .map(|(_, record)| record.kind),
            TargetKind::Player | TargetKind::Face => None,
        };
        let sensitive = kind
            .is_some_and(|kind| entity_in_tag(i32::from(kind), "minecraft:sensitive_to_impaling"));
        let level = enchantments::level(&flight.arrow.pickup_stack, "minecraft:impaling");
        let amount = maths::impaling_damage(maths::TRIDENT_DAMAGE, level, sensitive);
        if let ArrowKind::Trident { dealt_damage, .. } = &mut flight.arrow.of_kind {
            *dealt_damage = true;
        }
        let hurt = self.arrow_hurt(flight, "minecraft:trident", amount);
        let enderman = kind.is_some_and(|kind| i32::from(kind) == entity_types::ENDERMAN);
        let side_effects = match target.kind {
            TargetKind::Here => {
                let Some((_, record)) = self.find_entity(target.key) else {
                    return;
                };
                let record = record.clone();
                let took = self.arrow_hurt_here(&record, &hurt, output);
                took || !enderman
            }
            TargetKind::Copy => {
                let rides = self
                    .copied_entity(target.key)
                    .is_some_and(|(_, record)| record.riding.vehicle.is_some());
                self.send_on_entity(target.key, EntityUse::Hurt(Box::new(hurt)));
                !(enderman && !rides)
            }
            TargetKind::Player | TargetKind::Face => true,
        };
        if side_effects {
            self.arrow_sound(flight, sounds::ITEM_TRIDENT_HIT, 1.0, 1.0, output);
            let draw = flight.own.next_float();
            let (motion, yaw) =
                maths::reverse(flight.motion, maths::TRIDENT_BOUNCE, flight.yaw, draw);
            flight.motion = motion;
            flight.yaw = yaw;
        }
    }

    /// The trident's loyalty (`ID_LOYALTY`), read from its pickup stack.
    fn trident_loyalty(&self, arrow: &Arrow) -> i32 {
        maths::loyalty(enchantments::level(
            &arrow.pickup_stack,
            "minecraft:loyalty",
        ))
    }

    /// `ThrownTrident.tick`'s part before the arrow's: its damage dealt after four ticks
    /// in the ground; with loyalty and its damage dealt (or on its way already), its
    /// owner gone drops it, a mob owner within reach catches it, and otherwise it passes
    /// through blocks back towards its owner's eyes, with `trident_return` once.
    fn trident_before(
        &mut self,
        flight: &mut Flight,
        in_ground_time: u32,
        output: &mut TickOutput,
    ) {
        let loyalty = self.trident_loyalty(&flight.arrow);
        let owner = flight.arrow.owner;
        let seen = owner.and_then(|owner| self.returning_owner(owner));
        let ArrowKind::Trident {
            dealt_damage,
            returning,
        } = &mut flight.arrow.of_kind
        else {
            return;
        };
        if flight.arrow.in_ground.is_some() && in_ground_time > maths::TRIDENT_SETTLES {
            *dealt_damage = true;
        }
        if loyalty <= 0 || !(*dealt_damage || *returning > 0) {
            return;
        }
        let Some(seen) = seen else {
            return;
        };
        if !seen.alive {
            if flight.arrow.pickup == Pickup::Allowed {
                let [x, y, z] = flight.position;
                let stack = flight.arrow.pickup_stack.clone();
                self.spawn_item([x, y + DROP_RAISE, z], stack, output);
            }
            flight.removed = true;
            return;
        }
        if !seen.player
            && physics::distance_sqr(flight.position, seen.eyes).sqrt()
                < f64::from(seen.width) + 1.0
        {
            flight.removed = true;
            return;
        }
        let [position, motion] =
            maths::trident_return(flight.position, flight.motion, seen.eyes, loyalty);
        flight.position = position;
        flight.motion = motion;
        let first = *returning == 0;
        *returning += 1;
        if first {
            self.arrow_sound(flight, sounds::ITEM_TRIDENT_RETURN, 10.0, 1.0, output);
        }
    }

    /// The owner `owner` of a returning trident where the region sees them: its own
    /// player or entity, a face of another region's player, or the world's players.
    fn returning_owner(&self, owner: EntityId) -> Option<Seen> {
        if let Some(id) = self.player_with_entity(owner) {
            let player = self.players.get(&id)?;
            let feet = player.pose.position;
            let eyes = if player.keys.sneak {
                CROUCHING_EYES
            } else {
                PLAYER_EYES
            };
            return Some(Seen {
                eyes: [feet.x, feet.y + eyes, feet.z],
                alive: true,
                player: true,
                width: 0.6,
            });
        }
        if let Some((_, record)) = self.find_entity(owner) {
            let info = EntityTypeInfo::of(i32::from(record.kind));
            let (_, alive) = super::projectiles::pickable_and_alive(record);
            let [x, y, z] = record.position;
            return Some(Seen {
                eyes: [x, y + f64::from(info.eye_height), z],
                alive,
                player: false,
                width: info.width,
            });
        }
        if let Some(kept) = self.faces.kept.values().find(|kept| kept.entity == owner) {
            let [x, y, z] = kept.face.position;
            return Some(Seen {
                eyes: [x, y + f64::from(kept.face.eye_height), z],
                alive: kept.face.alive,
                player: true,
                width: 0.6,
            });
        }
        let known = self
            .faces
            .world
            .iter()
            .flatten()
            .find(|known| known.entity == owner)?;
        let [x, y, z] = known.position;
        Some(Seen {
            eyes: [x, y + PLAYER_EYES, z],
            alive: true,
            player: true,
            width: 0.6,
        })
    }

    /// The players' touch of arrows (`Player.touch` → `AbstractArrow.playerTouch`), in
    /// the order of the players, each taking the arrows of the region's chunks in their
    /// box grown by a block sideways and half a block up and down: one in the ground (or
    /// returning) and done shaking, a trident only by its owner or where it has none.
    /// One that may be picked up gives its stack; one only a creative player may take
    /// gives nothing (every player is creative); either way clients see it taken and it
    /// is gone.
    pub(crate) fn players_touch_arrows(&mut self, output: &mut TickOutput) {
        let players: Vec<(clustine_world::PlayerId, EntityId, [f64; 3])> = self
            .players
            .iter()
            .map(|(id, player)| {
                let at = player.pose.position;
                (*id, player.entity_id, [at.x, at.y, at.z])
            })
            .collect();
        for (id, collector, feet) in players {
            let reach = Aabb::of_entity(feet, 0.6, 1.8).inflate([1.0, 0.5, 1.0]);
            for (key, chunk) in self.entities_in(&reach) {
                let Some(record) = self.entity_record(key, chunk).cloned() else {
                    continue;
                };
                let EntityData::Arrow(arrow) = &record.data else {
                    continue;
                };
                let no_physics =
                    matches!(arrow.of_kind, ArrowKind::Trident { returning, .. } if returning > 0);
                if !(arrow.in_ground.is_some() || no_physics) || record.lived < arrow.shake_until {
                    continue;
                }
                let owned_by = arrow.owner == Some(collector);
                if i32::from(record.kind) == entity_types::TRIDENT {
                    let ownerless = arrow
                        .owner
                        .is_none_or(|owner| self.thrower(owner).is_none());
                    if !owned_by && !ownerless {
                        continue;
                    }
                }
                let (taken, adds) = match arrow.pickup {
                    Pickup::Allowed => (true, true),
                    Pickup::CreativeOnly => (true, false),
                    // `ThrownTrident.tryPickup`: its owner takes one on its way back.
                    Pickup::Disallowed => {
                        let back = i32::from(record.kind) == entity_types::TRIDENT
                            && no_physics
                            && owned_by;
                        (back, back)
                    }
                };
                if !taken {
                    continue;
                }
                if adds {
                    let Some(player) = self.players.get_mut(&id) else {
                        break;
                    };
                    let changed = super::items::add_to_inventory(
                        &mut player.inventory,
                        usize::from(player.selected_slot),
                        arrow.pickup_stack.clone(),
                    );
                    self.journal.players.insert(id);
                    for slot in changed {
                        let Some(player) = self.players.get(&id) else {
                            break;
                        };
                        let stack = player.inventory.0[slot].clone();
                        let slot = u8::try_from(slot).expect("an inventory slot fits a byte");
                        output
                            .player_events
                            .push((id, PlayerEvent::InventorySlot { slot, stack }));
                    }
                }
                output.events.push(RegionEvent::ItemTaken {
                    item: key,
                    collector,
                    count: 1,
                    chunk,
                });
                self.entity_memory.remove(&key);
                self.remove_entity(key, record.hops, chunk, output);
            }
        }
    }

    /// An arrow counted in the body of the region's mob `key`, after a hurt that took
    /// (`setArrowCount`, the living value 12; section 4.2). No body part keeps the count
    /// yet (the lead's, N9f): until one does, nothing is counted. The arrow's part is
    /// [`maths::arrow_goes_after`], the ticks until the next one goes.
    #[expect(
        clippy::unused_self,
        reason = "the body's part for the count is the lead's (N9f)"
    )]
    pub(crate) fn count_arrow(&mut self, _key: EntityId, _output: &mut TickOutput) {}
}
