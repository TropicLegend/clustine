//! The explosion (`ServerExplosion.explode`, `docs/groundwork/n6-brief.md`, section 2;
//! step N6c of `docs/groundwork/n6-plan.md`): the rays and the blocks they reach, the
//! entities hurt and pushed before any block breaks, the blocks broken in a shuffled
//! order with their drops merged into stacks of at most 16, containers spilled, TNT
//! primed with its short fuse, and the edges told, with each player's push.
//!
//! Everything the game draws from the level's random here comes from one stream, the
//! exploding entity's own in this tick (`Random::of_entity`, `docs/adr/0028-entities.md`,
//! section 4), in the game's order: the rays' floats, the shuffle, then, block by
//! block, the loot and a TNT's priming, and last the drops' places; a container
//! spills from the block's own stream as it is removed, as by any other cause. The new
//! items draw their own from their keys, as everywhere. The game's order of positions
//! before the shuffle is its `HashSet`'s; the shuffle makes any order as good, and the
//! positions are taken in ascending order.
//!
//! Across a line (decision 10): a block the rays reach in a chunk the region reads only
//! in a copy is an [`Effected::Exploded`] effect to its holder, which breaks it, spills
//! and primes there, and lays its drops apart from this side's (named); an entity in a
//! copy that the explosion hits is an [`EntityUse::Blasted`] effect on it. Players of
//! another region are not in copies and are not pushed (named). The edges send the
//! explosion's packet to every player within 64 blocks, of whatever region.
//!
//! Named differences: the game event `explode` waits for step N6f; ores give no
//! experience, as when a rule breaks them (`drops.rs`), and an infested block's
//! silverfish (`spawnAfterBreak`) is `infested.rs`'; the block entity a loot table copies from is not read
//! (`clustine_blocks::loot`); a shulker box spills what it holds as a chest does
//! (`docs/groundwork/n6-plan.md`, section 6 item 5); the fire of a bed's or an anchor's
//! explosion (`createFire`) is lit only in the region's own chunks, a block across a
//! line drawing its chance and lighting nothing; the entities are hurt in the order of
//! their chunks, sections and keys, then the region's players, where the game's order
//! mixes the players in by their sections; a primed TNT's `isInWater`, which a hanging
//! thing reads, is the one its last fluid update found.
//!
//! An explosion without a source entity (a bed's, an anchor's: `Level.explode` with a
//! null entity) has a key all the same, drawn from the region's entity ids
//! (`docs/groundwork/p-plan.md`, section 17.6), whose stream it draws from and by which
//! its drops across a line are merged; that key is no entity's, so the entities' query
//! leaves nothing out for it, its game event names no source and its loot sees no
//! `this_entity`.

use std::collections::BTreeSet;

use clustine_blocks::data::{in_tag, is_class};
use clustine_blocks::loot::{Broken, LootDraws, block_drops_as};
use clustine_data::{BlockClass, BlockState, EntityTypeInfo, ItemComponents, blocks};
use clustine_world::{
    Blast, BlockPos, ChunkPos, Double, Effect, Effected, Entity, EntityData, EntityField, EntityId,
    EntityUse, Float, ItemStack,
};
use uuid::Uuid;

use super::Region;
use super::attached::Breaker;
use super::items::{View, tracked};
use super::random_ticks::Random;
use super::stands::{INVISIBLE, MARKER, stand_size};
use super::updates::{RegionView, UPDATE_ALL, UPDATE_LIMIT};
use super::uses::block_in;
use crate::api::{ExplosionKind, RegionEvent, TickOutput, Tracked};
use crate::physics::aabb::{Aabb, lerp};
use crate::physics::body::clips_shape;
use crate::physics::collide::{Context, collision_shape};
use crate::physics::{self, World};

/// A ray's step, `0.3f` widened.
const STEP: f64 = 0.300_000_011_920_928_96;

/// What each step costs a ray beside the block it is in (`0.22500001f`).
const STEP_COST: f32 = 0.225_000_01;

/// `MAX_DROPS_PER_COMBINED_STACK`: the most a stack of merged drops holds.
const MOST_MERGED: i32 = 16;

/// How far from the world's middle a block can be (`Level.isInWorldBoundsHorizontal`).
const WORLD_BORDER: i32 = 30_000_000;

/// The eyes of a standing player above their feet, and their box.
const PLAYER_EYES: f64 = 1.62;
const PLAYER_WIDTH: f32 = 0.6;
const PLAYER_HEIGHT: f32 = 1.8;

/// One explosion, as `ServerLevel.explode` makes it: its source entity, its centre and
/// radius, whom its damage source names, and what its calculator answers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Explosion {
    /// The entity that explodes (`source`, the direct source), which the entities'
    /// query leaves out and the loot sees as `this_entity`.
    pub(crate) source: EntityId,
    pub(crate) centre: [f64; 3],
    pub(crate) radius: f32,
    /// The entity its damage source names as the cause (`DamageSource.getEntity`):
    /// who primed the TNT, which a vehicle and a hanging thing read.
    pub(crate) owner: Option<Uuid>,
    /// `getIndirectSourceEntity()`: who a TNT it primes is primed by, and whether the
    /// blocks are broken by a player. A primed TNT's owner; none for a TNT cart.
    pub(crate) indirect: Option<Uuid>,
    /// Whether its drops decay (`DESTROY_WITH_DECAY`): never for TNT, as
    /// `tnt_explosion_drop_decay` is false.
    pub(crate) decay: bool,
    /// Whether its rays pass rails and the block under them unweakened and break
    /// neither: a primed TNT cart's calculator (`MinecartTNT.getBlockExplosionResistance`
    /// and `shouldBlockExplode`).
    pub(crate) rails_pass: bool,
    /// Whether the source is in water, where frames, paintings, cushions and knots
    /// ignore it (`BlockAttachedEntity.ignoreExplosion`).
    pub(crate) source_in_water: bool,
    /// Whether it sets fire (`createFire`): a bed's and an anchor's.
    pub(crate) fire: bool,
    /// Whether it hurts entities: a bed's and an anchor's hurt nobody until survival.
    pub(crate) hurts: bool,
    /// One block whose resistance the explosion's calculator takes otherwise: the
    /// anchor's own, with water beside it (`RespawnAnchorBlock$1`).
    pub(crate) resistance_at: Option<(BlockPos, f32)>,
}

/// The draws of a loot table, from the explosion's stream.
struct Loot<'a>(&'a mut Random);

impl LootDraws for Loot<'_> {
    fn next_float(&mut self) -> f32 {
        self.0.next_float()
    }

    fn next_int(&mut self, bound: i32) -> i32 {
        let bound = u32::try_from(bound.max(1)).expect("a bound of at least 1");
        i32::try_from(self.0.next_int(bound)).expect("below a bound that fits an i32")
    }
}

/// An entity the explosion's query found: in a chunk the region has loaded, or in a
/// copy (`copied`), as it was when found.
pub(super) struct Found {
    pub(super) record: Entity,
    pub(super) chunk: ChunkPos,
    pub(super) copied: bool,
}

/// What the explosion does to one entity it reaches (`hurtEntities`): the damage and
/// the push, by its distance and how much of it the explosion sees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Hit {
    pub(super) damage: f32,
    pub(super) push: [f64; 3],
}

/// Whether the owner `owner` is a creative player, as a vehicle and a hanging thing ask
/// (`DamageSource.isCreativePlayer`): an owner is only ever a player, and every player
/// is a creative one.
pub(crate) fn owner_is_creative(owner: Option<Uuid>) -> bool {
    owner.is_some()
}

/// `ExplosionDamageCalculator.getBlockExplosionResistance` of the plain calculator:
/// nothing for air without a fluid, else the more of the block's and its fluid's.
pub(crate) fn resistance(state: BlockState) -> Option<f32> {
    let props = state.props();
    let fluid = props.fluid();
    if props.is_air() && fluid.is_none() {
        return None;
    }
    let block = state
        .physics()
        .map_or(0.0, |physics| physics.explosion_resistance);
    let fluid = fluid.map_or(0.0, |fluid| {
        clustine_data::FLUID_EXPLOSION_RESISTANCE[usize::from(fluid.fluid)]
    });
    Some(block.max(fluid))
}

/// `getEntityDamageAmount`: from a hit's distance to the centre and the part of it
/// seen, in the game's widths.
fn damage_amount(radius: f32, distance: f64, seen: f32) -> f32 {
    let reach = radius * 2.0;
    let d = distance / f64::from(reach);
    let impact = (1.0 - d) * f64::from(seen);
    ((impact * impact + impact) / 2.0 * 7.0 * f64::from(reach) + 1.0) as f32
}

/// `ItemEntity.areMergable(into, other)` and `ItemEntity.merge(into, other, 16)` of a
/// collector's stack `into` and a drop `other`: where they are of one item with the same
/// components and both fit one stack, as much of `other` moves into `into` as brings it to 16.
fn try_merge(into: &mut ItemStack, other: &mut ItemStack) {
    let max = crate::physics::item::max_stack_size(other);
    if other.count + into.count > max || !into.same_kind(other) {
        return;
    }
    let most = crate::physics::item::max_stack_size(into).min(MOST_MERGED);
    let moved = (most - into.count).min(other.count);
    into.count += moved;
    other.count -= moved;
}

/// `addOrAppendStack(collectors, stack, pos)`: the drop goes into the collectors in
/// order until none is left, and what is left starts a collector of its own at `pos`.
fn add_or_append(collectors: &mut Vec<(BlockPos, ItemStack)>, stack: ItemStack, pos: BlockPos) {
    let mut stack = stack;
    for (_, collected) in collectors.iter_mut() {
        try_merge(collected, &mut stack);
        if stack.count <= 0 || stack.item == clustine_data::items::AIR {
            return;
        }
    }
    collectors.push((pos, stack));
}

impl Region {
    /// `ServerExplosion.explode()` and the rest of `ServerLevel.explode`: the rays, the
    /// entities, the blocks, and the edges told. `random` is the source's stream in this
    /// tick, which every draw of the explosion takes from in turn.
    pub(crate) fn explode(
        &mut self,
        explosion: &Explosion,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        self.explode_from(explosion, true, random, output);
    }

    /// The explosion that a bed or an anchor makes where it does not work
    /// (`BedBlock.destroyOnUse`, n7d BedBlock.txt 32–75; `RespawnAnchorBlock.explode`,
    /// d RespawnAnchorBlock.txt 215–270): `Level.explode` with no entity, power 5, fire,
    /// and `ExplosionInteraction.BLOCK`, whose drops decay (`block_explosion_drop_decay`
    /// is on: d ServerLevel.txt 2566–2620), centred on `centre`, with the anchor's water
    /// rule as `resistance_at`. Its damage source, `bad_respawn_point`, hurts nobody until
    /// survival (the owner's answer, `docs/groundwork/p-plan.md`, section 17.6). Its key
    /// is drawn from the region's entity ids; where none is left, nothing explodes
    /// (named).
    pub(super) fn bad_respawn_point_explosion(
        &mut self,
        centre: [f64; 3],
        resistance_at: Option<(BlockPos, f32)>,
        output: &mut TickOutput,
    ) {
        let Some(key) = self.take_key() else {
            return;
        };
        let explosion = Explosion {
            source: key,
            centre,
            radius: 5.0,
            owner: None,
            indirect: None,
            decay: true,
            rails_pass: false,
            source_in_water: false,
            fire: true,
            hurts: false,
            resistance_at,
        };
        // The game draws from the level's random; the new key's own stream stands for
        // it, as that key is no other explosion's or entity's.
        let mut random = self.entity_stream(key).random;
        self.explode_from(&explosion, false, &mut random, output);
    }

    /// `ServerExplosion.explode()` (n6 ServerExplosion.txt 849–876), of the entity
    /// `explosion.source` where `entity`, else of no entity, whose key is no entity's.
    fn explode_from(
        &mut self,
        explosion: &Explosion,
        entity: bool,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        // `explode` first, of the entity that explodes (`vibrations.rs`), if one does.
        let source = entity.then(|| super::vibrations::Source::of(explosion.source));
        let context = super::vibrations::Context::of(source, None);
        self.vibrate(
            super::vibrations::Vibration::EXPLODE,
            explosion.centre,
            context,
        );
        let positions = self.exploded_positions(explosion, random);
        // An explosion that hurts nobody (a bed's, an anchor's, until survival) neither
        // hurts nor pushes an entity or a player.
        let knockback = if explosion.hurts {
            self.hurt_entities(explosion, entity, output)
        } else {
            Vec::new()
        };
        // Every explosion here breaks blocks (`interactsWithBlocks`: its interaction is
        // not `KEEP`). The positions come back in the shuffled order the game's list is
        // left in, which `createFire` walks.
        let shuffled =
            self.interact_with_blocks(explosion, entity, positions.clone(), random, output);
        if explosion.fire {
            self.create_fire(&shuffled, random, output);
        }
        output.events.push(RegionEvent::Explosion {
            centre: explosion.centre,
            radius: explosion.radius,
            blocks: i32::try_from(positions.len()).unwrap_or(i32::MAX),
            knockback,
            // `isSmall()`: a radius below 2, or one that breaks no blocks.
            large: explosion.radius >= 2.0,
            // The source is never silent.
            sound: true,
            kind: ExplosionKind::Blast,
        });
    }

    /// What the explosion's effects across a line carry of it: `direct` where its source
    /// is an entity, which the holder's loot then sees as `this_entity`.
    fn blast_of(&self, explosion: &Explosion, direct: bool) -> Blast {
        Blast {
            fire: explosion.fire,
            hurts: explosion.hurts,
            source: explosion.source,
            tick: self.tick,
            centre: explosion.centre.map(Double),
            radius: Float(explosion.radius),
            decay: explosion.decay,
            direct,
            owner: explosion.owner,
            owner_creative: owner_is_creative(explosion.owner),
        }
    }

    /// The lowest block's height and the height above the highest, of the chunk of the
    /// explosion's centre, or the world's as configured.
    fn build_heights(&self, explosion: &Explosion) -> (i32, i32) {
        let chunk = ChunkPos::containing(explosion.centre[0], explosion.centre[2]);
        self.readable_chunk(chunk).map_or(
            (self.config.lowest_y, self.config.lowest_y + 384),
            |chunk| {
                let height = i32::try_from(chunk.height()).unwrap_or(384);
                (chunk.min_y(), chunk.min_y() + height)
            },
        )
    }

    /// The calculator's resistance of the block `state` at `pos`: the plain one, a
    /// primed TNT cart's, which gives rails and what is under them none, or an anchor's,
    /// which gives its own position the resistance kept, whatever is there now
    /// (`RespawnAnchorBlock$1.getBlockExplosionResistance`).
    fn blast_resistance(
        &self,
        explosion: &Explosion,
        pos: BlockPos,
        state: BlockState,
    ) -> Option<f32> {
        if let Some((at, kept)) = explosion.resistance_at
            && at == pos
        {
            return Some(kept);
        }
        let plain = resistance(state)?;
        if explosion.rails_pass && self.rails_at_or_above(pos, state) {
            return Some(0.0);
        }
        Some(plain)
    }

    /// The calculator's `shouldBlockExplode` of the block `state` at `pos`.
    fn blast_breaks(&self, explosion: &Explosion, pos: BlockPos, state: BlockState) -> bool {
        !(explosion.rails_pass && self.rails_at_or_above(pos, state))
    }

    /// Whether `state` at `pos`, or the block above it, is of `#minecraft:rails`.
    fn rails_at_or_above(&self, pos: BlockPos, state: BlockState) -> bool {
        let above = self.block(pos.offset(0, 1, 0)).unwrap_or(blocks::AIR);
        in_tag(state, "minecraft:rails") || in_tag(above, "minecraft:rails")
    }

    /// `calculateExplodedPositions()`: 1352 rays from the centre to the cube's surface,
    /// each of a strength drawn in turn, losing at every step what the block there
    /// resists and a little more, and taking the blocks they reach while they last, air
    /// among them. Ascending.
    fn exploded_positions(&self, explosion: &Explosion, random: &mut Random) -> Vec<BlockPos> {
        let (lowest, above) = self.build_heights(explosion);
        let in_world = |pos: BlockPos| {
            pos.y >= lowest
                && pos.y < above
                && (-WORLD_BORDER..WORLD_BORDER).contains(&pos.x)
                && (-WORLD_BORDER..WORLD_BORDER).contains(&pos.z)
        };
        let mut found = BTreeSet::new();
        let [cx, cy, cz] = explosion.centre;
        for i in 0..16 {
            for j in 0..16 {
                for k in 0..16 {
                    let surface = [i, j, k].iter().any(|&n| n == 0 || n == 15);
                    if !surface {
                        continue;
                    }
                    let along = |n: i32| f64::from(n as f32 / 15.0 * 2.0 - 1.0);
                    let (mut dx, mut dy, mut dz) = (along(i), along(j), along(k));
                    let length = (dx * dx + dy * dy + dz * dz).sqrt();
                    dx /= length;
                    dy /= length;
                    dz /= length;
                    let mut power = explosion.radius * (0.7 + random.next_float() * 0.6);
                    let (mut x, mut y, mut z) = (cx, cy, cz);
                    while power > 0.0 {
                        let pos =
                            BlockPos::new(physics::floor(x), physics::floor(y), physics::floor(z));
                        if !in_world(pos) {
                            break;
                        }
                        let state = self.block(pos).unwrap_or(blocks::AIR);
                        if let Some(resists) = self.blast_resistance(explosion, pos, state) {
                            power -= (resists + 0.3) * 0.3;
                        }
                        if power > 0.0 && self.blast_breaks(explosion, pos, state) {
                            found.insert(pos);
                        }
                        x += dx * STEP;
                        y += dy * STEP;
                        z += dz * STEP;
                        power -= STEP_COST;
                    }
                }
            }
        }
        found.into_iter().collect()
    }

    /// The entities whose boxes meet `bounds`, of the region's loaded chunks and its
    /// copies, but `except`: by chunk, then section, then key.
    pub(super) fn blast_targets(&self, bounds: &Aabb, except: EntityId) -> Vec<Found> {
        let low = ChunkPos::containing(bounds.min[0] - 2.0, bounds.min[2] - 2.0);
        let high = ChunkPos::containing(bounds.max[0] + 2.0, bounds.max[2] + 2.0);
        let mut found = Vec::new();
        for x in low.x..=high.x {
            for z in low.z..=high.z {
                let chunk = ChunkPos::new(x, z);
                let (records, copied) = match self.chunks.get(&chunk) {
                    Some(loaded) => (loaded.entities(), false),
                    None => match self.copies.get(&chunk) {
                        Some(copy) => (copy.entities(), true),
                        None => continue,
                    },
                };
                let mut here: Vec<(i32, &Entity)> = records
                    .iter()
                    .filter(|entity| entity.key != except)
                    .filter(|entity| self.entity_box(entity).intersects(*bounds))
                    .map(|entity| (physics::floor(entity.position[1]) >> 4, entity))
                    .collect();
                here.sort_by_key(|(section, entity)| (*section, entity.key));
                found.extend(here.into_iter().map(|(_, record)| Found {
                    record: record.clone(),
                    chunk,
                    copied,
                }));
            }
        }
        found
    }

    /// `hurtEntities()`: every entity in the box of twice the radius and a block about
    /// the centre, within twice the radius of its feet, is hurt by its kind's rule and
    /// pushed away from the centre by how near it is and how much of it is seen; the
    /// region's players, whom nothing hurts, are pushed where they do not fly. What it
    /// returns is each player's push, for the packet.
    fn hurt_entities(
        &mut self,
        explosion: &Explosion,
        entity: bool,
        output: &mut TickOutput,
    ) -> Vec<(EntityId, [f64; 3])> {
        if explosion.radius < 1.0e-5 {
            return Vec::new();
        }
        let reach = f64::from(explosion.radius * 2.0);
        let [cx, cy, cz] = explosion.centre;
        let corner = |at: f64, by: f64| f64::from(physics::floor(at + by));
        let bounds = Aabb::new(
            [
                corner(cx, -reach - 1.0),
                corner(cy, -reach - 1.0),
                corner(cz, -reach - 1.0),
            ],
            [
                corner(cx, reach + 1.0),
                corner(cy, reach + 1.0),
                corner(cz, reach + 1.0),
            ],
        );
        // The key of an explosion of no entity is no entity's, and leaves nothing out.
        for found in self.blast_targets(&bounds, explosion.source) {
            // An earlier hit may have moved or removed it (a vehicle that broke and let
            // its riders off).
            let record = if found.copied {
                found.record
            } else {
                match self.find_entity(found.record.key) {
                    Some((_, record)) => record.clone(),
                    None => continue,
                }
            };
            if self.ignores(explosion, &record) {
                continue;
            }
            let Some(hit) = self.hit_of(explosion, &record) else {
                continue;
            };
            if found.copied {
                if self.sends_to(found.chunk) {
                    let by = EntityUse::Blasted {
                        damage: Float(hit.damage),
                        push: hit.push.map(Double),
                        blast: self.blast_of(explosion, entity),
                    };
                    self.made.push(Effect {
                        at: block_in(found.chunk, &record),
                        what: Effected::OnEntity {
                            key: record.key,
                            by,
                        },
                    });
                }
                continue;
            }
            self.blasted(explosion, &record, hit, output);
        }
        self.player_pushes(explosion.centre, reach, &bounds)
    }

    /// The push of each of the region's players within `reach` of `centre` and in
    /// `bounds` (`hurtEntities`' hit players): away from the centre by how near they are
    /// and how much of them is seen, none for a player who flies. A creative player is
    /// invulnerable, and pushed unless they fly; the push reaches them only in the
    /// packet, as the client moves the player.
    pub(super) fn player_pushes(
        &self,
        centre: [f64; 3],
        reach: f64,
        bounds: &Aabb,
    ) -> Vec<(EntityId, [f64; 3])> {
        let players: Vec<(EntityId, [f64; 3], bool)> = self
            .players
            .values()
            .map(|player| {
                let at = player.pose.position;
                (player.entity_id, [at.x, at.y, at.z], player.flying)
            })
            .collect();
        let mut knockback = Vec::new();
        for (entity, feet, flying) in players {
            let shape = Aabb::of_entity(feet, PLAYER_WIDTH, PLAYER_HEIGHT);
            if !shape.intersects(*bounds) {
                continue;
            }
            let distance = physics::distance_sqr(feet, centre).sqrt();
            let d = distance / reach;
            if d > 1.0 {
                continue;
            }
            let eyes = [feet[0], feet[1] + PLAYER_EYES, feet[2]];
            let towards = physics::normalize(physics::sub(eyes, centre));
            let context = Context {
                stands_on_lava: false,
                bottom: feet[1],
                fall_distance: 0.0,
                falling_block: false,
                noclip: None,
            };
            let seen = self.seen_percent(centre, shape, context);
            if !flying {
                let push = physics::scale(towards, (1.0 - d) * f64::from(seen));
                knockback.push((entity, push));
            }
        }
        knockback
    }

    /// `ignoreExplosion(explosion)` of the entity `record`: an invisible armour stand,
    /// and a frame, a painting, a cushion or a knot where the source is in water.
    pub(super) fn ignores(&self, explosion: &Explosion, record: &Entity) -> bool {
        match record.data {
            EntityData::ArmorStand { flags, .. } => flags & INVISIBLE != 0,
            EntityData::ItemFrame { .. }
            | EntityData::Painting { .. }
            | EntityData::Cushion { .. }
            | EntityData::LeashKnot { .. } => explosion.source_in_water,
            _ => false,
        }
    }

    /// The explosion's damage and push of the entity `record`, if it is within reach
    /// of its feet: the push from the centre to its eyes, or to its feet for primed TNT.
    pub(super) fn hit_of(&self, explosion: &Explosion, record: &Entity) -> Option<Hit> {
        let reach = f64::from(explosion.radius * 2.0);
        let distance = physics::distance_sqr(record.position, explosion.centre).sqrt();
        let d = distance / reach;
        if d > 1.0 {
            return None;
        }
        let at = match record.data {
            EntityData::PrimedTnt { .. } => record.position,
            _ => {
                let eyes = match record.data {
                    EntityData::ArmorStand { flags, .. } => stand_size(record.kind, flags).2,
                    _ => EntityTypeInfo::of(i32::from(record.kind)).eye_height,
                };
                let [x, y, z] = record.position;
                [x, y + f64::from(eyes), z]
            }
        };
        let towards = physics::normalize(physics::sub(at, explosion.centre));
        let context = Context {
            stands_on_lava: false,
            bottom: record.position[1],
            fall_distance: record.fall_distance,
            falling_block: matches!(record.data, EntityData::FallingBlock { .. }),
            noclip: None,
        };
        let seen = self.seen_percent(explosion.centre, self.entity_box(record), context);
        let damage = damage_amount(explosion.radius, distance, seen);
        // Nothing the region has resists knockback (`explosion_knockback_resistance`
        // is 0 for armour stands, the one living kind).
        let push = physics::scale(towards, (1.0 - d) * f64::from(seen));
        Some(Hit { damage, push })
    }

    /// `getSeenPercent(centre, entity)`: of a grid of points over the box `shape`, the
    /// part from which the centre is seen past every block's collision shape (as
    /// `context` sees them), fluids aside. The grid's steps are added up as the game adds
    /// them, so that their number is the game's.
    fn seen_percent(&self, centre: [f64; 3], shape: Aabb, context: Context) -> f32 {
        let size = shape.size();
        let sx = 1.0 / (size[0] * 2.0 + 1.0);
        let sy = 1.0 / (size[1] * 2.0 + 1.0);
        let sz = 1.0 / (size[2] * 2.0 + 1.0);
        let ox = (1.0 - (1.0 / sx).floor() * sx) / 2.0;
        let oz = (1.0 - (1.0 / sz).floor() * sz) / 2.0;
        if sx < 0.0 || sy < 0.0 || sz < 0.0 {
            return 0.0;
        }
        let view = View {
            region: self,
            home: ChunkPos::containing(centre[0], centre[2]),
        };
        let mut seen = 0_u32;
        let mut total = 0_u32;
        let mut a = 0.0;
        while a <= 1.0 {
            let mut c = 0.0;
            while c <= 1.0 {
                let mut g = 0.0;
                while g <= 1.0 {
                    let from = [
                        lerp(a, shape.min[0], shape.max[0]) + ox,
                        lerp(c, shape.min[1], shape.max[1]),
                        lerp(g, shape.min[2], shape.max[2]) + oz,
                    ];
                    if !clips_collider(&view, context, from, centre) {
                        seen += 1;
                    }
                    total += 1;
                    g += sz;
                }
                c += sy;
            }
            a += sx;
        }
        seen as f32 / total as f32
    }

    /// The hit of the explosion on the entity `record` of the region's own chunks: its
    /// kind's `hurtServer`, then the push added to its motion where it is still there.
    fn blasted(
        &mut self,
        explosion: &Explosion,
        record: &Entity,
        hit: Hit,
        output: &mut TickOutput,
    ) {
        let key = record.key;
        let creative = owner_is_creative(explosion.owner);
        let mut flags = 0;
        match record.data {
            EntityData::Item { ref stack, .. } => {
                if self.blast_item(record, stack.clone(), hit.damage, output) {
                    return;
                }
                flags |= Tracked::SYNC_VELOCITY;
            }
            // `hurtServer` only marks it hurt, which sends its motion (a projectile's
            // too, `Projectile.hurtServer`).
            EntityData::FallingBlock { .. }
            | EntityData::Thrown { .. }
            | EntityData::Arrow(_)
            | EntityData::Spit(_)
            | EntityData::Firework(_) => {
                flags |= Tracked::SYNC_VELOCITY;
            }
            // It cannot be hurt, and is pushed from its feet. Neither can a cloud nor
            // the fangs (their `hurtServer` hurts nothing).
            EntityData::PrimedTnt { .. } | EntityData::Cloud(_) | EntityData::Fangs(_) => {}
            // A fireball's own (`fireballs.rs`).
            EntityData::Fireball(_) => {
                flags |= self.fireball_blasted(record, explosion.owner, output)
            }
            // `LivingEntity.hurtServer` of the explosion's source (`mob_bodies.rs`),
            // which pushes nothing of itself (`#no_knockback`).
            EntityData::Mob { .. } => {
                let kind = if explosion.owner.is_some() {
                    crate::mobs::body::hurt::DamageType("minecraft:player_explosion")
                } else {
                    crate::mobs::body::hurt::DamageType("minecraft:explosion")
                };
                let source = crate::mobs::body::hurt::DamageSource::of(kind);
                self.hurt_mob(key, &source, hit.damage, output);
            }
            EntityData::ExperienceOrb { .. } => {
                if self.hurt_orb(key, hit.damage, output) {
                    return;
                }
                flags |= Tracked::SYNC_VELOCITY;
            }
            EntityData::Minecart { .. } | EntityData::Boat { .. } => {
                // An explosion sets a TNT cart alight, with the owner as its cause.
                self.hurt_vehicle_by(key, hit.damage, creative, Some(explosion.owner), output);
            }
            EntityData::ArmorStand { flags: stand, .. } => {
                // `ArmorStand.hurtServer`: a marker is not hurt; any other stand breaks,
                // dropping what it wears and not itself (`brokenByAnything`, `kill`).
                if stand & MARKER == 0 {
                    self.broken_by_anything(record, output);
                    self.kill_stand(key, output);
                }
            }
            EntityData::ItemFrame { fixed, .. } => {
                // `ItemFrame.hurtServer`: a fixed frame only to a creative player, and
                // an explosion takes the frame with its item, not the item alone.
                if !fixed || creative {
                    self.hurt_attached(key, Breaker::of_owner(explosion.owner), output);
                }
            }
            EntityData::Painting { .. }
            | EntityData::Cushion { .. }
            | EntityData::LeashKnot { .. } => {
                self.hurt_attached(key, Breaker::of_owner(explosion.owner), output);
            }
        }
        self.push_from_explosion(key, hit.push, flags, output);
    }

    /// `ItemEntity.hurtServer` by the explosion's `damage`: nothing to a stack that
    /// resists explosions (a nether star's `damage_resistant`); else its health falls
    /// by the damage, as the game's `int` and `float` take it, and at nought or less it
    /// is gone (a shulker box's contents would spill from its components, which
    /// Clustine's stacks do not have). Whether it is gone.
    fn blast_item(
        &mut self,
        record: &Entity,
        stack: ItemStack,
        damage: f32,
        output: &mut TickOutput,
    ) -> bool {
        let resists = usize::try_from(stack.item)
            .ok()
            .filter(|item| *item < clustine_data::ITEMS.len())
            .is_some_and(|_| {
                ItemComponents::of(stack.item).damage_resistant == Some("minecraft:is_explosion")
            });
        if resists {
            return false;
        }
        let Some((chunk, current)) = self.find_entity(record.key) else {
            return true;
        };
        let EntityData::Item {
            ref stack,
            pick_up_delay,
            thrower,
            health,
            age_offset,
        } = current.data
        else {
            return false;
        };
        let hops = current.hops;
        let health = (f32::from(health) - damage) as i32;
        if health <= 0 {
            self.remove_entity(record.key, hops, chunk, output);
            return true;
        }
        let data = EntityData::Item {
            stack: stack.clone(),
            pick_up_delay,
            thrower,
            health: i16::try_from(health).unwrap_or(i16::MAX),
            age_offset,
        };
        self.commit_entity(chunk, record.key, vec![EntityField::Data(data)], output);
        false
    }

    /// `pushFromExplosion(push)`: `Entity.push`, which adds a finite push to the motion
    /// and sends it (`needsSync`), committed, of the entity `key` where it is still
    /// there; a frame, painting, cushion or knot breaks at any push, but the explosion
    /// broke it already. The edges are told with `flags`.
    pub(super) fn push_from_explosion(
        &mut self,
        key: EntityId,
        push: [f64; 3],
        flags: u8,
        output: &mut TickOutput,
    ) {
        let Some((chunk, record)) = self.find_entity(key) else {
            return;
        };
        if super::attached::is_block_attached(&record.data) {
            self.moved_attached(key, push, output);
            return;
        }
        let mut flags = flags;
        let mut after = record.clone();
        if push.iter().all(|component| component.is_finite()) {
            after.motion = physics::add(record.motion, push);
            flags |= Tracked::NEEDS_SYNC;
            self.commit_entity(chunk, key, vec![EntityField::Motion(after.motion)], output);
        }
        let mut report = tracked(&after, flags, false);
        report.chunk = chunk;
        output.events.push(RegionEvent::EntityTracked(report));
    }

    /// `interactWithBlocks(positions)`: the positions shuffled, each block that is not
    /// air broken in that order (`BlockBehaviour.onExplosionHit`), its loot collected
    /// and merged, a container spilled and a TNT primed as it goes, and then the merged
    /// drops laid, each at the first block of its stack. A block in a copy is an effect
    /// to its holder. The positions are returned in their shuffled order.
    fn interact_with_blocks(
        &mut self,
        explosion: &Explosion,
        entity: bool,
        positions: Vec<BlockPos>,
        random: &mut Random,
        output: &mut TickOutput,
    ) -> Vec<BlockPos> {
        let mut positions = positions;
        // `Util.shuffle(list, random)`.
        let mut left = positions.len();
        while left > 1 {
            let bound = u32::try_from(left).unwrap_or(u32::MAX);
            let chosen = random.next_int(bound) as usize;
            positions.swap(left - 1, chosen);
            left -= 1;
        }
        let broken = Broken {
            this_entity: entity,
            explosion_radius: explosion.decay.then_some(explosion.radius),
        };
        let mut collectors: Vec<(BlockPos, ItemStack)> = Vec::new();
        for &pos in &positions {
            let Some(state) = self.block(pos) else {
                continue;
            };
            if state.props().is_air() {
                continue;
            }
            if !self.chunks.contains_key(&pos.chunk()) {
                if self.sends_to(pos.chunk()) {
                    self.made.push(Effect {
                        at: pos,
                        what: Effected::Exploded {
                            expected: state,
                            blast: self.blast_of(explosion, entity),
                        },
                    });
                }
                continue;
            }
            let tnt = is_class(state, BlockClass::TntBlock);
            // `dropFromExplosion`: every block but TNT, which is primed instead.
            if !tnt {
                // `spawnAfterBreak(level, pos, EMPTY, false)`, before the loot.
                self.infested_broken(pos, state, output);
                let stacks = block_drops_as(state, pos, broken, &mut Loot(random), &mut |at| {
                    self.block(at).unwrap_or(blocks::AIR)
                });
                for stack in stacks {
                    add_or_append(&mut collectors, stack, pos);
                }
            }
            // `setBlockAndUpdate(pos, AIR)`, inside which a container spills what it
            // holds (`preRemoveSideEffects`, `Containers.dropContents`), as every
            // removal does (`containers.rs`), from the block's own stream.
            self.set_block_updated(pos, blocks::AIR, UPDATE_ALL, UPDATE_LIMIT, output);
            // `TntBlock.wasExploded`: primed with a short fuse, by whoever primed this.
            if tnt {
                self.spawn_primed_tnt(pos, explosion.indirect, true, random, output);
            }
        }
        for (pos, stack) in collectors {
            self.pop_resource(pos, stack, random, output);
        }
        positions
    }

    /// `createFire(positions)` (n6 ServerExplosion.txt 806–847): of the positions in
    /// their shuffled order, one in three by a draw from the explosion's stream (made
    /// for every position, before anything is read) that is air over a block of solid
    /// render becomes fire as `BaseFireBlock.getState` makes it, with every update. A
    /// position in a copy draws and lights nothing (named).
    fn create_fire(
        &mut self,
        positions: &[BlockPos],
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        for &pos in positions {
            if random.next_int(3) != 0 || !self.chunks.contains_key(&pos.chunk()) {
                continue;
            }
            let air = self.block(pos).is_some_and(|state| state.props().is_air());
            let below = self.block(pos.offset(0, -1, 0)).unwrap_or(blocks::AIR);
            if !air || !below.props().is_solid_render() {
                continue;
            }
            let fire = clustine_blocks::shape::fire_state(&mut RegionView(self), pos);
            self.set_block_updated(pos, fire, UPDATE_ALL, UPDATE_LIMIT, output);
        }
    }
}

/// The mark of the streams a holder draws the loot and the drops of a block an
/// explosion across a line broke from, apart from every other stream.
const FAR_BLAST: u64 = 0x6661_7262_6c61_7374;

impl Region {
    /// A mob's own explosion (a creeper's `explodeCreeper`), after its tick:
    /// `ExplosionInteraction.MOB` at its feet, its blocks broken (`mob_griefing` is on)
    /// with their drops decaying (`mob_explosion_drop_decay`), its draws from the mob's
    /// own stream as TNT's are; then the mob discarded, with no death and no loot. A block
    /// or an entity across a line goes as TNT's does.
    ///
    /// **Named**: the damage names no cause (the game's names the creeper, a
    /// `player_explosion` of it), so a hurt mob's death is no creeper's kill and TNT it
    /// primes is primed by nobody, as `owner` is a player's here.
    pub(crate) fn mob_explodes(
        &mut self,
        key: EntityId,
        at: [f64; 3],
        radius: f32,
        in_water: bool,
        output: &mut TickOutput,
    ) {
        if self.find_entity(key).is_none() {
            return;
        }
        let explosion = Explosion {
            fire: false,
            hurts: true,
            resistance_at: None,
            source: key,
            centre: at,
            radius,
            owner: None,
            indirect: None,
            decay: true,
            rails_pass: false,
            source_in_water: in_water,
        };
        let mut random = self.entity_stream(key).random;
        let start = random.clone();
        self.explode(&explosion, &mut random, output);
        self.entity_drew(key, random.draws_since(&start));
        if let Some((chunk, record)) = self.find_entity(key) {
            let hops = record.hops;
            self.unride_removed(key, true, output);
            self.remove_entity(key, hops, chunk, output);
        }
    }

    /// An explosion's block across a line, taken by its holder (`Effected::Exploded`,
    /// `docs/groundwork/n6-plan.md`, section 6 item 9): broken if it is still `expected`,
    /// as the exploding region would have broken it, its loot collected with the other
    /// blocks of the same explosion taken in this tick, a container spilled by the
    /// removal, and TNT primed with the short fuse by the explosion's owner. Its draws
    /// come from the block's own stream; the drops are laid with
    /// [`Region::lay_blast_drops`]. Named: a TNT cart's explosion primes the TNT it
    /// reaches across a line by whoever lit the cart, where the game names nobody.
    pub(super) fn take_exploded(
        &mut self,
        at: BlockPos,
        expected: BlockState,
        blast: Blast,
        output: &mut TickOutput,
    ) {
        let Some(state) = self.own_block(at) else {
            return;
        };
        if state != expected || state.props().is_air() {
            return;
        }
        let mut random = Random::of_block(self.seed(), at, self.tick, FAR_BLAST);
        let tnt = is_class(state, BlockClass::TntBlock);
        if !tnt {
            // `spawnAfterBreak(level, pos, EMPTY, false)`, before the loot.
            self.infested_broken(at, state, output);
            let broken = Broken {
                this_entity: blast.direct,
                explosion_radius: blast.decay.then_some(blast.radius.0),
            };
            let stacks = block_drops_as(state, at, broken, &mut Loot(&mut random), &mut |pos| {
                self.block(pos).unwrap_or(blocks::AIR)
            });
            let collectors = self
                .blast_drops
                .entry((blast.source, blast.tick))
                .or_default();
            for stack in stacks {
                add_or_append(collectors, stack, at);
            }
        }
        self.set_block_updated(at, blocks::AIR, UPDATE_ALL, UPDATE_LIMIT, output);
        if tnt {
            self.spawn_primed_tnt(at, blast.owner, true, &mut random, output);
        }
    }

    /// Lays the drops of the blocks of each explosion taken across a line in this tick,
    /// merged within that explosion alone, each at the first block of its stack: the
    /// effects of one explosion that come apart in two ticks lay apart (named).
    pub(super) fn lay_blast_drops(&mut self, output: &mut TickOutput) {
        for ((source, _), collectors) in std::mem::take(&mut self.blast_drops) {
            let Some(&(first, _)) = collectors.first() else {
                continue;
            };
            let mut random = Random::of_block(self.seed(), first, self.tick, FAR_BLAST ^ source.0);
            for (pos, stack) in collectors {
                self.pop_resource(pos, stack, &mut random, output);
            }
        }
    }

    /// Fire across a line burnt the block at `at` (`Effected::Burnt`): if it is still
    /// `expected`, TNT is primed by nobody, and the block goes as the fire's region
    /// would have taken it (`FireBlock.checkBurnOut`'s removal with flags 3).
    pub(super) fn take_burnt(
        &mut self,
        at: BlockPos,
        expected: BlockState,
        output: &mut TickOutput,
    ) {
        let Some(state) = self.own_block(at) else {
            return;
        };
        if state != expected {
            return;
        }
        if is_class(state, BlockClass::TntBlock) {
            let mut random = Random::of_block(self.seed(), at, self.tick, FAR_BLAST);
            self.spawn_primed_tnt(at, None, false, &mut random, output);
        }
        self.set_block_updated(at, blocks::AIR, UPDATE_ALL, UPDATE_LIMIT, output);
    }

    /// An explosion's hit across a line on the entity `key` of this region
    /// (`EntityUse::Blasted`): the damage and the push the exploding region worked out
    /// from its copy, as its kind takes them. Named: whether the source was in water,
    /// which hanging things read, is not carried, and they are hit.
    pub(super) fn take_blasted(
        &mut self,
        key: EntityId,
        damage: Float,
        push: [Double; 3],
        blast: Blast,
        output: &mut TickOutput,
    ) {
        // A blast that hurts nobody files no hit; one that says so is taken as nothing.
        if !blast.hurts {
            return;
        }
        let Some((_, record)) = self.find_entity(key) else {
            return;
        };
        let record = record.clone();
        let explosion = Explosion {
            fire: blast.fire,
            hurts: blast.hurts,
            resistance_at: None,
            source: blast.source,
            centre: blast.centre.map(|component| component.0),
            radius: blast.radius.0,
            owner: blast.owner,
            indirect: blast.owner,
            decay: blast.decay,
            rails_pass: false,
            source_in_water: false,
        };
        let hit = Hit {
            damage: damage.0,
            push: push.map(|component| component.0),
        };
        self.blasted(&explosion, &record, hit, output);
    }
}

/// `Level.clip` with `ClipContext.Block.COLLIDER` and `Fluid.NONE` from `from` to `to`:
/// whether a block's collision shape, as `context` sees it, is in the way.
fn clips_collider<W: World + ?Sized>(
    world: &W,
    context: Context,
    from: [f64; 3],
    to: [f64; 3],
) -> bool {
    physics::walk::traverse(from, to, |pos| {
        let state = world.state(pos);
        collision_shape(state, pos, context).is_some_and(|(shape, _)| {
            shape
                .boxes()
                .iter()
                .any(|bounds| clips_shape(bounds, from, to))
        })
    })
}

#[cfg(test)]
mod tests;
