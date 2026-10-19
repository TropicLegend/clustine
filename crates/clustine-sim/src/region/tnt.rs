//! TNT (`TntBlock`, `PrimedTnt`, `MinecartTNT`; `docs/groundwork/n6-brief.md`,
//! section 1; step N6c of `docs/groundwork/n6-plan.md`): the block primed by redstone,
//! by fire burning it, by a flint and steel or a fire charge, and by an explosion with a
//! short fuse; the primed TNT's fall, its fuse and its explosion (`explosions.rs`); a
//! flint and steel's and a fire charge's use on other blocks; and the TNT cart primed on
//! a powered activator rail, by fire, by an explosion or by a hit while it moves fast,
//! and set off by its fuse, a hard landing and a fast collision.
//!
//! What the game draws from the level's random at a priming (the motion's angle, and
//! from an explosion the short fuse) comes from the block's stream in the tick
//! (`Random::of_block`, kept on as `drops.rs` keeps it), or from the explosion's own
//! where an explosion primes it. The TNT cart's draws are its own stream's, as the
//! game's are the cart's own random.
//!
//! Named differences: a burning projectile priming TNT waits for its projectiles
//! (`onProjectileHit`, N9), and a survival player's break of unstable TNT for survival
//! players; a flint and steel lights no nether portal, which Clustine has none of; the
//! game events `prime_fuse` and `block_place` wait for step N6f; a primed TNT is no
//! obstacle to a block placed into it (`blocksBuilding`), as only players' boxes are;
//! the game rule `tnt_explodes` is always true.

use clustine_blocks::data::{flag, in_tag, is_class, same_block, set_flag};
use clustine_blocks::shape::fire_state;
use clustine_blocks::survive::can_survive;
use clustine_data::{BlockClass, BlockState, blocks, entity_types, items, sounds};
use clustine_world::{
    BlockPos, ChunkPos, Effect, Effected, Entity, EntityChange, EntityData, EntityField, EntityId,
    Float, PlayerId,
};
use uuid::Uuid;

use super::Region;
use super::entities::insecure_uuid;
use super::explosions::Explosion;
use super::items::{CartMemory, Memory, View, tracked};
use super::random_ticks::Random;
use super::riding::Relocated;
use super::updates::{
    RegionView, UPDATE_ALL, UPDATE_CLIENTS, UPDATE_LIMIT, UPDATE_NEIGHBORS, fluid_block,
};
use crate::api::{RegionEvent, SOUND_SOURCE_BLOCKS, TickOutput, Tracked};
use crate::physics::body::{AIR_DRAG, Body};
use crate::physics::item::{move_guarded, tick_movements};
use crate::physics::{self, fluids, horizontal_sqr};
use crate::placement::Click;

/// A primed TNT's fuse at priming (`setFuse(80)`), in ticks.
pub(crate) const FUSE: i16 = 80;

/// A primed TNT's explosion's power, unless summoned with another
/// (`explosionPower`'s default).
const POWER: f32 = 4.0;

/// How far above its feet a primed TNT explodes: `getY(0.0625)` of its box, which is
/// `0.98f` high, widened.
const EXPLOSION_HEIGHT: f64 = 0.98_f32 as f64 * 0.0625;

/// A turn, `(float) (2π)` widened, by which the priming's draw turns its motion.
const TURN: f64 = 6.283_185_482_025_146_5;

/// The speed, squared and along x and z, at which a TNT cart that is broken or that
/// collides goes off: `0.01f` widened.
const CART_SPEED: f64 = 0.009_999_999_776_482_582;

/// A TNT cart's fuse at priming (`primeFuse`).
const CART_FUSE: i32 = 80;

/// A TNT cart's power before its speed adds to it (`explosionPowerBase`), and how much
/// its speed adds (`explosionSpeedFactor`), as floats.
const CART_POWER: f32 = 4.0;
const CART_SPEED_FACTOR: f32 = 1.0;

/// The entity event that tells clients a TNT cart is primed.
const CART_PRIMED: u8 = 70;

/// `Block.UPDATE_IMMEDIATE`, which Clustine's sets always are.
const UPDATE_IMMEDIATE: u32 = 8;

/// The flags of TNT lit by a flint and steel, and of what a flint and steel sets
/// (`setBlock(pos, state, 11)`).
const UPDATE_LIT: u32 = UPDATE_NEIGHBORS | UPDATE_CLIENTS | UPDATE_IMMEDIATE;

/// Mixed into a block's stream for a priming's draws.
const PRIMING: u64 = 0x0070_7269_6d69_6e67;

/// How a TNT cart moves as its tick reaches its fuse: where it is, its motion, whether
/// its last move met a block along x or z, and whether it is in water.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Moving {
    pub(crate) position: [f64; 3],
    pub(crate) motion: [f64; 3],
    pub(crate) horizontal_collision: bool,
    pub(crate) in_water: bool,
}

/// The fall that `Block.fallOn` of the block `on` hands to `causeFallDamage`, after a
/// fall of `fallen`: powder snow takes none, a pointed dripstone's tip adds two and a
/// half, and any other block passes it on, as a falling anvil's chipping reads it
/// (`falling.rs`).
pub(crate) fn fall_onto(on: BlockState, fallen: f64) -> Option<f64> {
    if is_class(on, BlockClass::PowderSnowBlock) {
        None
    } else if is_class(on, BlockClass::PointedDripstoneBlock)
        && on.property("vertical_direction") == Some("up")
        && on.property("thickness") == Some("tip")
    {
        Some(fallen + 2.5)
    } else {
        Some(fallen)
    }
}

/// `PrimedTnt`'s motion at priming, from the level's draw `drawn`: two hundredths of a
/// block a tick in the direction it turns, by `Math.sin` and `Math.cos` (`StrictMath`'s,
/// as ADR-0028 section 4 takes them), and `0.2f` up.
pub(crate) fn priming_motion(drawn: f64) -> [f64; 3] {
    let angle = drawn * TURN;
    [
        -libm::sin(angle) * 0.02,
        f64::from(0.2_f32),
        -libm::cos(angle) * 0.02,
    ]
}

/// `PrimedTnt.getRandomShortFuse(fuse, random)`: a quarter of the fuse drawn, and an
/// eighth more, 10 to 29 ticks of a fuse of 80.
fn short_fuse(fuse: i16, random: &mut Random) -> i16 {
    let quarter = u32::try_from((fuse / 4).max(1)).unwrap_or(1);
    let drawn = i16::try_from(random.next_int(quarter)).unwrap_or(i16::MAX);
    drawn + fuse / 8
}

/// The fields of a primed TNT's data that a commit carries: all but its fuse, which
/// burns down every tick as a counter does.
fn primed_parts(data: &EntityData) -> Option<(u16, Float, Option<Uuid>)> {
    match data {
        EntityData::PrimedTnt {
            block,
            power,
            owner,
            ..
        } => Some((*block, *power, *owner)),
        _ => None,
    }
}

impl Region {
    /// `TntBlock.prime(level, pos, owner)`: primed TNT where the block is, its owner the
    /// player who lit it, if one did, and the sound of its priming. The block is the
    /// caller's to remove, where this says the TNT was made; it is not where the region
    /// cannot keep the entity (its chunk is full, or no key is left).
    pub(crate) fn prime_tnt(
        &mut self,
        pos: BlockPos,
        owner: Option<Uuid>,
        output: &mut TickOutput,
    ) -> bool {
        let mut random = self
            .block_streams
            .remove(&pos)
            .unwrap_or_else(|| Random::of_block(self.config.seed, pos, self.tick, PRIMING));
        let made = self.spawn_primed_tnt(pos, owner, false, &mut random, output);
        self.block_streams.insert(pos, random);
        let Some(position) = made else {
            return false;
        };
        // `playSound(null, …)`: everyone near hears it.
        output.events.push(RegionEvent::Sound {
            sound: sounds::ENTITY_TNT_PRIMED,
            source: SOUND_SOURCE_BLOCKS,
            position: clustine_world::Vec3::new(position[0], position[1], position[2]),
            volume: 1.0,
            pitch: 1.0,
            except: None,
        });
        true
    }

    /// `new PrimedTnt(level, x + 0.5, y, z + 0.5, owner)` and `addFreshEntity` for the
    /// TNT block at `pos`: its UUID from its own stream, its motion from a draw of
    /// `random`, and its fuse of 80 or, primed by an explosion (`short`), a short fuse
    /// drawn after (`TntBlock.wasExploded`). Into a chunk the region does not have loaded
    /// it is sent as an arrival, as a throw is. Where it is, or `None` where the region
    /// cannot keep it.
    pub(crate) fn spawn_primed_tnt(
        &mut self,
        pos: BlockPos,
        owner: Option<Uuid>,
        short: bool,
        random: &mut Random,
        output: &mut TickOutput,
    ) -> Option<[f64; 3]> {
        let position = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        ];
        let chunk = ChunkPos::containing(position[0], position[2]);
        let here = self.chunks.contains_key(&chunk);
        let key = if here {
            self.take_entity_key(chunk)
        } else {
            self.take_key()
        }?;
        let motion = priming_motion(random.next_double());
        let fuse = if short {
            short_fuse(FUSE, random)
        } else {
            FUSE
        };
        // `Entity.<init>`: the UUID's two longs from the new entity's own random.
        let mut own = Random::of_entity(self.config.seed, key, self.tick);
        let uuid = insecure_uuid(own.next_u64(), own.next_u64());
        let entity = Entity {
            key,
            kind: u16::try_from(entity_types::TNT).expect("entity type ids fit 16 bits"),
            uuid,
            position,
            motion,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            hops: 0,
            age: 0,
            data: EntityData::PrimedTnt {
                fuse,
                block: blocks::TNT.0,
                power: Float(POWER),
                owner,
            },
            fall_distance: 0.0,
            fire_ticks: 0,
            riding: Default::default(),
        };
        if here {
            self.entity_draws.insert(key, 2);
            self.add_fresh_entity(entity, output);
        } else {
            self.send_made(entity, output);
        }
        Some(position)
    }

    /// `TntBlock.onPlace`: TNT placed where power reaches it is primed and removed,
    /// unless it was TNT before (its `unstable` changed). Where the chunk does not tick,
    /// what the power reads may not all be there, and the TNT waits as an update of
    /// itself, as a copper bulb placed does (`redstone.rs`).
    pub(crate) fn tnt_on_place(
        &mut self,
        pos: BlockPos,
        state: BlockState,
        old: BlockState,
        output: &mut TickOutput,
    ) {
        if !is_class(state, BlockClass::TntBlock) || same_block(state, old) {
            return;
        }
        if !self.ticks_chunk(pos.chunk()) {
            self.neighbour_changed_at(pos, clustine_data::Face::Down, state, false, output);
            return;
        }
        self.tnt_neighbour_changed(pos, output);
    }

    /// `TntBlock.neighborChanged`: TNT that power reaches is primed, with no owner, and
    /// removed (`removeBlock`, with every update).
    pub(crate) fn tnt_neighbour_changed(&mut self, pos: BlockPos, output: &mut TickOutput) {
        let Some(state) = self.own_block(pos) else {
            return;
        };
        if !is_class(state, BlockClass::TntBlock) {
            return;
        }
        let powered = clustine_blocks::signal::has_neighbour_signal(&mut RegionView(self), pos);
        if powered && self.prime_tnt(pos, None, output) {
            self.set_block(pos, fluid_block(state), output);
        }
    }

    /// Fire burnt the TNT `old` at `pos` in a chunk the region reads only in a copy: the
    /// holder removes it and primes it (`Effected::Burnt`), where it is another's or
    /// unknown and not wanted, as a set there would be an effect.
    pub(crate) fn tnt_burnt_across(&mut self, pos: BlockPos, old: BlockState) {
        if self.sends_to(pos.chunk()) {
            self.made.push(Effect {
                at: pos,
                what: Effected::Burnt { expected: old },
            });
        }
    }

    /// `TntBlock.useItemOn` of a click of `player`'s with a flint and steel or a fire
    /// charge on TNT: primed with the player as its owner, and the block set to air with
    /// flags 11. A sneaking player with something in hand passes the block by. A
    /// creative player's flint and steel takes no wear and their fire charge is not used
    /// up. Whether it took the click.
    pub(crate) fn light_tnt(
        &mut self,
        click: &Click,
        player: PlayerId,
        output: &mut TickOutput,
    ) -> bool {
        let lights = matches!(
            click.held,
            Some(items::FLINT_AND_STEEL | items::FIRE_CHARGE)
        );
        if !lights || click.sneaking {
            return false;
        }
        let Some(state) = self.own_block(click.position) else {
            return false;
        };
        if !is_class(state, BlockClass::TntBlock) {
            return false;
        }
        if self.prime_tnt(click.position, Some(player.0), output) {
            self.set_block_updated(
                click.position,
                blocks::AIR,
                UPDATE_LIT,
                UPDATE_LIMIT,
                output,
            );
        }
        true
    }

    /// `FlintAndSteelItem.useOn` and `FireChargeItem.useOn` of a click of `player`'s on a
    /// block that did nothing with it: an unlit campfire, candle or candle cake is lit;
    /// otherwise fire is set in the place beside the clicked face where fire can stay
    /// (`BaseFireBlock.canBePlacedAt`), with the item's sound, a flint and steel's heard
    /// by all but the player, a fire charge's by everyone. Whether it did either.
    pub(crate) fn fire_item_used_on(
        &mut self,
        click: &Click,
        player: PlayerId,
        output: &mut TickOutput,
    ) -> bool {
        let flint = match click.held {
            Some(items::FLINT_AND_STEEL) => true,
            Some(items::FIRE_CHARGE) => false,
            _ => return false,
        };
        let Some(clicked) = self.own_block(click.position) else {
            return false;
        };
        if can_light(clicked) {
            let lit = set_flag(clicked, "lit", true);
            self.fire_item_sound(click.position, flint, player, output);
            let flags = if flint { UPDATE_LIT } else { UPDATE_ALL };
            self.set_block_updated(click.position, lit, flags, UPDATE_LIMIT, output);
            return true;
        }
        let target = click.face.neighbour(click.position);
        let Some(there) = self.block(target) else {
            return false;
        };
        if !there.props().is_air() {
            return false;
        }
        let fire = fire_state(&mut RegionView(self), target);
        if !can_survive(fire, target, &mut RegionView(self)) {
            return false;
        }
        self.fire_item_sound(target, flint, player, output);
        let flags = if flint { UPDATE_LIT } else { UPDATE_ALL };
        self.set_block_updated(target, fire, flags, UPDATE_LIMIT, output);
        true
    }

    /// The sound of a flint and steel used at `pos` (`FLINTANDSTEEL_USE`, its pitch
    /// drawn from 0.8 to 1.2, heard by all but `player`), or of a fire charge
    /// (`FIRECHARGE_USE`, its pitch two draws apart about 1, heard by all).
    fn fire_item_sound(
        &mut self,
        pos: BlockPos,
        flint: bool,
        player: PlayerId,
        output: &mut TickOutput,
    ) {
        let mut random = self.next_draw();
        let (sound, pitch, except) = if flint {
            let pitch = random.next_float() * 0.4 + 0.8;
            (sounds::ITEM_FLINTANDSTEEL_USE, pitch, Some(player))
        } else {
            let first = random.next_float();
            let second = random.next_float();
            (
                sounds::ITEM_FIRECHARGE_USE,
                (first - second) * 0.2 + 1.0,
                None,
            )
        };
        output.events.push(RegionEvent::Sound {
            sound,
            source: SOUND_SOURCE_BLOCKS,
            position: clustine_world::Vec3::new(
                f64::from(pos.x) + 0.5,
                f64::from(pos.y) + 0.5,
                f64::from(pos.z) + 0.5,
            ),
            volume: 1.0,
            pitch,
            except,
        });
    }

    /// The body of the primed TNT `entity` of the loaded chunk `position`, as its
    /// record and the region's memory of it have it.
    fn primed_body(&self, entity: &Entity, position: ChunkPos) -> Body {
        let mut body = self.falling_body(entity, position);
        body.falling_block = false;
        body.in_water = self
            .entity_memory
            .get(&entity.key)
            .is_some_and(|memory| memory.cart.in_water);
        body
    }

    /// `PrimedTnt.tick()` of `entity`, in the loaded chunk `position`: gravity, its move,
    /// the blocks it touches, the air's drag and the ground's, its fuse burning down, and
    /// at nought its explosion; until then the fluids push it. It has no `baseTick`: it
    /// does not burn, and falls out of the world to explode there.
    pub(crate) fn tick_primed_tnt(
        &mut self,
        entity: Entity,
        position: ChunkPos,
        output: &mut TickOutput,
    ) {
        let EntityData::PrimedTnt {
            fuse,
            block,
            power,
            owner,
        } = entity.data
        else {
            return;
        };
        let key = entity.key;
        // Burning TNT is work at a line where regions follow players, as a circuit is:
        // they merge at once, so that its explosion is whole in one region
        // (`docs/groundwork/n6-plan.md`, section 6 item 9; ADR-0027, section 7).
        self.circuit_changed(position);
        let mut body = self.primed_body(&entity, position);
        let view = View {
            region: self,
            home: position,
        };
        body.apply_gravity();
        let motion = body.motion;
        move_guarded(&mut body, &view, false, motion);
        let movements = tick_movements(&mut body);
        // `applyEffectsFromBlocks`: a cobweb's or powder snow's hold, slime, lit
        // redstone ore. Fire sets it alight, which it is immune to.
        self.falling_effects(&mut body, &movements, position, output);
        body.motion = physics::scale(body.motion, f64::from(AIR_DRAG));
        if body.on_ground {
            let [x, y, z] = body.motion;
            body.motion = [x * 0.7, y * -0.5, z * 0.7];
        }
        let fuse = fuse.saturating_sub(1);
        if fuse <= 0 {
            // `discard()`, then `explode()`, from where it is now.
            let in_water = body.in_water;
            self.remove_entity(key, entity.hops, position, output);
            let [x, y, z] = body.position;
            let explosion = Explosion {
                source: key,
                centre: [x, y + EXPLOSION_HEIGHT, z],
                radius: power.0,
                owner,
                indirect: owner,
                decay: false,
                rails_pass: false,
                source_in_water: in_water,
            };
            let mut random = self.own_stream(key);
            self.explode(&explosion, &mut random, output);
            return;
        }
        // `updateFluidInteraction`.
        let view = View {
            region: self,
            home: position,
        };
        let found = fluids::update(&view, &body.bounds());
        body.in_water = found.in_water();
        if found.in_water() {
            body.fall_distance = 0.0;
        }
        body.motion = fluids::push(&found, body.motion);
        let mut flags = 0;
        if body.horizontal_collision {
            flags |= Tracked::HORIZONTAL_COLLISION;
        }
        if body.vertical_collision {
            flags |= Tracked::VERTICAL_COLLISION;
        }
        // `setRequiresPrecisePosition(horizontalCollision || verticalCollision)`.
        if body.horizontal_collision || body.vertical_collision {
            flags |= Tracked::PRECISE;
        }
        self.entity_memory.insert(
            key,
            Memory {
                supporting: body.supporting,
                on_ground_no_blocks: body.on_ground_no_blocks,
                stuck_speed: body.stuck_speed,
                cart: CartMemory {
                    in_water: body.in_water,
                    ..CartMemory::default()
                },
                ..Memory::default()
            },
        );
        let after = Entity {
            position: body.position,
            motion: body.motion,
            on_ground: body.on_ground,
            fall_distance: body.fall_distance,
            age: entity.age.saturating_add(1),
            data: EntityData::PrimedTnt {
                fuse,
                block,
                power,
                owner,
            },
            ..entity.clone()
        };
        self.write_primed_tnt(&entity, after, flags, position, output);
    }

    /// What the primed TNT's tick leaves: its record, its commit, the edges' report.
    /// Its fuse burns down in the record in place, as a counter does, its chunk ageing
    /// ([`TickOutput::entities_ageing`]), and a commit carries it with a change of its
    /// place or its being on the ground; the edges are told every tick, as the game sends
    /// the fuse's data every tick it changes.
    fn write_primed_tnt(
        &mut self,
        before: &Entity,
        after: Entity,
        flags: u8,
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
        let bits = |values: [f64; 3]| values.map(f64::to_bits);
        let moved = bits(after.position) != bits(before.position);
        let committed = moved
            || after.on_ground != before.on_ground
            || primed_parts(&after.data) != primed_parts(&before.data);
        if to == position {
            if committed {
                let mut fields = Vec::new();
                if moved {
                    fields.push(EntityField::Position(after.position));
                }
                if bits(after.motion) != bits(before.motion) {
                    fields.push(EntityField::Motion(after.motion));
                }
                if after.on_ground != before.on_ground {
                    fields.push(EntityField::OnGround(after.on_ground));
                }
                if after.fall_distance.to_bits() != before.fall_distance.to_bits() {
                    fields.push(EntityField::FallDistance(after.fall_distance));
                }
                fields.push(EntityField::Data(after.data.clone()));
                output
                    .entity_changes
                    .push((position, EntityChange::Set(key, fields)));
            }
            if let Some(chunk) = self.chunks.get_mut(&position) {
                chunk.apply_entity(&EntityChange::Whole(after.clone()));
            }
        }
        output.entities_ageing.push(to);
        let mut report = tracked(&after, flags, true);
        report.chunk = to;
        output.events.push(RegionEvent::EntityTracked(report));
    }

    /// The stream of the entity `key` in this tick, on from what it has drawn.
    fn own_stream(&self, key: EntityId) -> Random {
        let drawn = self.entity_draws.get(&key).copied().unwrap_or(0);
        let mut random = Random::of_entity(self.config.seed, key, self.tick);
        for _ in 0..drawn {
            random.next_u64();
        }
        random
    }

    /// Counts what the entity `key` has drawn from `start`, its stream as
    /// [`Region::own_stream`] gave it, to `now`.
    fn count_draws(&mut self, key: EntityId, start: &Random, now: &Random) {
        let used = now.draws_since(start);
        if used > 0 {
            *self.entity_draws.entry(key).or_default() += used;
        }
    }

    /// The fuse of the TNT cart `key` (`-1` while it is not primed), if the region has
    /// it.
    pub(crate) fn tnt_cart_fuse(&self, key: EntityId) -> Option<i32> {
        match self.find_entity(key)?.1.data {
            EntityData::Minecart { fuse, .. } => Some(fuse),
            _ => None,
        }
    }

    /// Sets the fuse of the TNT cart `key`, committed (a priming) or in place as a
    /// counter (its burning down).
    fn set_cart_fuse(&mut self, key: EntityId, to: i32, commit: bool, output: &mut TickOutput) {
        let Some((chunk, record)) = self.find_entity(key) else {
            return;
        };
        let mut data = record.data.clone();
        if let EntityData::Minecart { fuse, .. } = &mut data {
            *fuse = to;
        }
        let fields = vec![EntityField::Data(data)];
        if commit {
            self.commit_entity(chunk, key, fields, output);
        } else {
            self.age_entity(chunk, key, fields, output);
        }
    }

    /// `MinecartTNT.primeFuse(source)`: the fuse at 80; the source, where there is one
    /// and the cart has none yet, kept as what its explosion will be caused by (its
    /// entity, `source`'s inner part); clients told (entity event 70) and the priming
    /// heard.
    pub(crate) fn prime_tnt_cart(
        &mut self,
        key: EntityId,
        source: Option<Option<Uuid>>,
        output: &mut TickOutput,
    ) {
        self.set_cart_fuse(key, CART_FUSE, true, output);
        if let Some(cause) = source
            && let Some(memory) = self.entity_memory.get_mut(&key)
            && memory.cart.ignition.is_none()
        {
            memory.cart.ignition = Some(cause);
        }
        let Some((chunk, record)) = self.find_entity(key) else {
            return;
        };
        let [x, y, z] = record.position;
        output.events.push(RegionEvent::EntityEvent {
            entity: key,
            chunk,
            event: CART_PRIMED,
        });
        output.events.push(RegionEvent::Sound {
            sound: sounds::ENTITY_TNT_PRIMED,
            source: SOUND_SOURCE_BLOCKS,
            position: clustine_world::Vec3::new(x, y, z),
            volume: 1.0,
            pitch: 1.0,
            except: None,
        });
    }

    /// `MinecartTNT.destroy(level, source)` of the TNT cart `key`: by a source that sets
    /// TNT alight (`igniter`, with its cause), or any while it moves at a tenth of a
    /// block a tick or more, it is not destroyed but primed, if it is not yet, with a
    /// fuse of two draws of its own; otherwise it breaks as any vehicle does, dropping
    /// its item.
    pub(crate) fn tnt_cart_destroyed(
        &mut self,
        key: EntityId,
        igniter: Option<Option<Uuid>>,
        output: &mut TickOutput,
    ) {
        let Some((_, record)) = self.find_entity(key) else {
            return;
        };
        let speed = horizontal_sqr(record.motion);
        if igniter.is_none() && speed < CART_SPEED {
            self.destroy_vehicle(key, output);
            return;
        }
        if self.tnt_cart_fuse(key).is_some_and(|fuse| fuse >= 0) {
            return;
        }
        // The damage source is always there; its entity is the cause.
        self.prime_tnt_cart(key, Some(igniter.flatten()), output);
        let start = self.own_stream(key);
        let mut random = start.clone();
        let first = i32::try_from(random.next_int(20)).expect("below 20");
        let second = i32::try_from(random.next_int(20)).expect("below 20");
        self.count_draws(key, &start, &random);
        self.set_cart_fuse(key, first + second, true, output);
    }

    /// The TNT cart's part of its tick after `AbstractMinecart.tick` (`MinecartTNT.tick`):
    /// a fuse above nought burns down, one at nought sets it off, and a collision along
    /// x or z at speed sets it off too, a second time where its fuse has just done so,
    /// as the game's does. Whether it is gone.
    pub(crate) fn tnt_cart_tick(
        &mut self,
        key: EntityId,
        moving: Moving,
        output: &mut TickOutput,
    ) -> bool {
        let Some(fuse) = self.tnt_cart_fuse(key) else {
            return false;
        };
        let owner = self.cart_igniter(key);
        let start = self.own_stream(key);
        let mut random = start.clone();
        let mut gone = false;
        if fuse > 0 {
            self.set_cart_fuse(key, fuse - 1, false, output);
        } else if fuse == 0 {
            let speed = horizontal_sqr(moving.motion);
            self.explode_tnt_cart(key, moving, true, owner, speed, &mut random, output);
            gone = true;
        }
        if moving.horizontal_collision {
            let speed = horizontal_sqr(moving.motion);
            if speed >= CART_SPEED {
                let primed = fuse >= 0;
                self.explode_tnt_cart(key, moving, primed, owner, speed, &mut random, output);
                gone = true;
            }
        }
        if !gone {
            self.count_draws(key, &start, &random);
        }
        gone
    }

    /// `MinecartTNT.causeFallDamage` of a fall that sets the cart off: `speed` is the
    /// square of a tenth of the fall.
    pub(crate) fn tnt_cart_fell(
        &mut self,
        key: EntityId,
        moving: Moving,
        speed: f64,
        output: &mut TickOutput,
    ) {
        let primed = self.tnt_cart_fuse(key).is_some_and(|fuse| fuse >= 0);
        let owner = self.cart_igniter(key);
        let mut random = self.own_stream(key);
        self.explode_tnt_cart(key, moving, primed, owner, speed, &mut random, output);
    }

    /// The cause of the TNT cart `key`'s ignition, if it has one with a cause.
    fn cart_igniter(&self, key: EntityId) -> Option<Uuid> {
        self.entity_memory
            .get(&key)
            .and_then(|memory| memory.cart.ignition)
            .flatten()
    }

    /// `MinecartTNT.explode(source, speedSqr)`: an explosion at the cart, of a power of
    /// four and up to seven and a half more by its speed and a draw of its own, caused by
    /// what primed it; then the cart is discarded.
    #[allow(clippy::too_many_arguments, reason = "the game's fields of the cart")]
    fn explode_tnt_cart(
        &mut self,
        key: EntityId,
        moving: Moving,
        primed: bool,
        owner: Option<Uuid>,
        speed: f64,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        let by_speed = speed.sqrt().min(5.0);
        let power = (f64::from(CART_POWER)
            + f64::from(CART_SPEED_FACTOR) * random.next_double() * 1.5 * by_speed)
            as f32;
        let explosion = Explosion {
            source: key,
            centre: moving.position,
            radius: power,
            owner,
            // `getIndirectSourceEntity` of a cart: none.
            indirect: None,
            decay: false,
            rails_pass: primed,
            source_in_water: moving.in_water,
        };
        self.explode(&explosion, random, output);
        if self.find_entity(key).is_some() {
            self.remove_destroyed(key, output);
        }
    }
}

/// `CampfireBlock.canLight`, `CandleBlock.canLight` and `CandleCakeBlock.canLight`: an
/// unlit campfire or candle out of water, or an unlit candle cake.
pub(super) fn can_light(state: BlockState) -> bool {
    let unlit = state.property("lit") == Some("false");
    if in_tag(state, "minecraft:campfires") || in_tag(state, "minecraft:candles") {
        return unlit && !flag(state, "waterlogged") && state.property("waterlogged").is_some();
    }
    in_tag(state, "minecraft:candle_cakes") && unlit
}

#[cfg(test)]
pub(super) mod tests;
