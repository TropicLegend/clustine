//! The wind charged effect's burst (`WindChargedMobEffect.onMobRemoved`'s
//! `ServerLevel.explode` with `ExplosionInteraction.TRIGGER` and the wind charge's
//! calculator; `docs/groundwork/n9-plan.md`, section 5.2; part S1's).
//!
//! The burst is an explosion (`ServerExplosion.explode`, `explosions.rs`) whose
//! calculator (`AbstractWindCharge.EXPLOSION_DAMAGE_CALCULATOR`, a
//! `SimpleExplosionDamageCalculator(true, false, empty, #blocks_wind_charge_explosions)`)
//! lets the blocks of `#blocks_wind_charge_explosions` resist as 3 600 000 and the
//! others as any explosion's, hurts no entity, and pushes as any explosion pushes
//! (`getKnockbackMultiplier` 1, and nothing for a player who flies). Its blocks are
//! triggered, not broken (`Explosion.BlockInteraction.TRIGGER_BLOCK`): each of the
//! blocks that override `onExplosionHit` for it answers in the shuffled order, a button
//! pressed, a lever pulled, a door (but an iron one), a trapdoor (but an iron one) or a
//! fence gate opened or closed unless power holds it, a bell rung, a lit candle put
//! out; the rest stay as they are. Every draw is the level's, which is the dying mob's
//! stream aside (`effect_deaths.rs`): the rays' floats, the shuffle, a door's,
//! trapdoor's or gate's sound's pitch.
//!
//! Clients are sent the explosion's packet as any explosion's, of the wind's kind
//! ([`ExplosionKind::Wind`]: the gust emitters, `breeze_wind_charge_burst`, no particles
//! over the blocks), with each of the region's players' push as a TNT's
//! (`explosions.rs`): the wind charge's calculator pushes them by 1, and a player who
//! flies not at all (`SimpleExplosionDamageCalculator.getKnockbackMultiplier`).
//!
//! Named differences: a block, an entity or a player in another region's land is not
//! reached (an explosion's effects across a line break and hurt, which this burst must
//! not do). A beehive's bees are not angered (`BeehiveBlock.onExplosionHit`), as
//! Clustine's bees take no anger from a hive.

use std::collections::BTreeSet;

use clustine_blocks::data::{flag, in_tag, is_block, is_class, set_flag, tick_id};
use clustine_data::{BlockClass, blocks, sounds};
use clustine_world::{BlockPos, ChunkPos, EntityData, EntityId, Ticked};

use super::explosions::{Explosion, resistance};
use super::random_ticks::Random;
use super::updates::{UPDATE_ALL, UPDATE_CLIENTS, UPDATE_LIMIT};
use super::vibrations::{Context, Source, Vibration};
use super::{Region, redstone};
use crate::api::{ExplosionKind, RegionEvent, TickOutput};
use crate::mobs::Mob;
use crate::mobs::body::attribute;
use crate::physics::aabb::Aabb;
use crate::physics::{self, Vec3};

/// A ray's step, `0.3f` widened, and what each step costs beside the block
/// (`0.22500001f`), as every explosion's.
const STEP: f64 = 0.300_000_011_920_928_96;
const STEP_COST: f32 = 0.225_000_01;

/// How far from the world's middle a block can be (`Level.isInWorldBoundsHorizontal`).
const WORLD_BORDER: i32 = 30_000_000;

/// What the calculator gives a block of `#blocks_wind_charge_explosions`.
const IMMUNE: f32 = 3_600_000.0;

/// `Block.UPDATE_IMMEDIATE`, with which a door is set.
const UPDATE_IMMEDIATE: u32 = 8;

/// The wind charge's calculator's `getBlockExplosionResistance`: 3 600 000 for a block
/// the wind charge cannot move, else any explosion's (none for air without a fluid).
fn wind_resistance(state: clustine_data::BlockState) -> Option<f32> {
    if in_tag(state, "minecraft:blocks_wind_charge_explosions") {
        return Some(IMMUNE);
    }
    resistance(state)
}

impl Region {
    /// The burst of wind of `radius` at `at` that the region's entity `source` makes as it
    /// dies (`ServerLevel.explode(source, null, the wind charge's calculator, …, false,
    /// TRIGGER, …)`): its `explode` heard, its rays, the entities and players it pushes,
    /// the blocks it triggers, and the explosion's packet for the edges. `random` is the
    /// level's, whose draws it makes in the game's order.
    pub(crate) fn wind_burst(
        &mut self,
        source: EntityId,
        at: Vec3,
        radius: f32,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        let context = Context::of(Some(Source::of(source)), None);
        self.vibrate(Vibration::EXPLODE, at, context);
        let positions = self.burst_positions(at, radius, random);
        let blocks = i32::try_from(positions.len()).unwrap_or(i32::MAX);
        let knockback = self.burst_push(source, at, radius, output);
        self.burst_trigger(source, positions, random, output);
        output.events.push(RegionEvent::Explosion {
            centre: at,
            radius,
            blocks,
            knockback,
            // `isSmall()`: a radius below 2 (a burst triggers blocks, so it interacts
            // with them).
            large: radius >= 2.0,
            // `ClientboundExplodePacket`'s `playSound`: its source is not silent.
            sound: true,
            kind: ExplosionKind::Wind,
        });
    }

    /// `calculateExplodedPositions()` with the wind charge's calculator: 1352 rays, each
    /// of a strength drawn in turn, losing what each block resists, every block they
    /// reach while they last taken (`shouldBlockExplode` is always true). Ascending, as
    /// the shuffle that follows makes any order as good (`explosions.rs`).
    fn burst_positions(&self, at: Vec3, radius: f32, random: &mut Random) -> Vec<BlockPos> {
        let chunk = ChunkPos::containing(at[0], at[2]);
        let (lowest, above) = self.readable_chunk(chunk).map_or(
            (self.config.lowest_y, self.config.lowest_y + 384),
            |chunk| {
                let height = i32::try_from(chunk.height()).unwrap_or(384);
                (chunk.min_y(), chunk.min_y() + height)
            },
        );
        let in_world = |pos: BlockPos| {
            pos.y >= lowest
                && pos.y < above
                && (-WORLD_BORDER..WORLD_BORDER).contains(&pos.x)
                && (-WORLD_BORDER..WORLD_BORDER).contains(&pos.z)
        };
        let mut found = BTreeSet::new();
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
                    let mut power = radius * (0.7 + random.next_float() * 0.6);
                    let [mut x, mut y, mut z] = at;
                    while power > 0.0 {
                        let pos =
                            BlockPos::new(physics::floor(x), physics::floor(y), physics::floor(z));
                        if !in_world(pos) {
                            break;
                        }
                        let state = self.block(pos).unwrap_or(blocks::AIR);
                        if let Some(resists) = wind_resistance(state) {
                            power -= (resists + 0.3) * 0.3;
                        }
                        if power > 0.0 {
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

    /// `hurtEntities()` with the wind charge's calculator: no entity is hurt, and each
    /// within twice the radius of its feet, but `source`, is pushed away from the centre
    /// by how near it is and how much of it the burst sees, less what of it resists an
    /// explosion's push (`explosion_knockback_resistance`). Only the region's own
    /// entities. What it returns is each of the region's players' push, for the packet.
    fn burst_push(
        &mut self,
        source: EntityId,
        at: Vec3,
        radius: f32,
        output: &mut TickOutput,
    ) -> Vec<(EntityId, [f64; 3])> {
        if radius < 1.0e-5 {
            return Vec::new();
        }
        let reach = f64::from(radius * 2.0);
        let corner = |value: f64, by: f64| f64::from(physics::floor(value + by));
        let bounds = Aabb::new(
            [
                corner(at[0], -reach - 1.0),
                corner(at[1], -reach - 1.0),
                corner(at[2], -reach - 1.0),
            ],
            [
                corner(at[0], reach + 1.0),
                corner(at[1], reach + 1.0),
                corner(at[2], reach + 1.0),
            ],
        );
        let explosion = Explosion {
            source,
            centre: at,
            radius,
            owner: None,
            indirect: None,
            decay: false,
            rails_pass: false,
            source_in_water: false,
        };
        for found in self.blast_targets(&bounds, source) {
            if found.copied {
                continue;
            }
            // An earlier push may have taken it (a frame it broke).
            let Some((_, record)) = self.find_entity(found.record.key) else {
                continue;
            };
            let record = record.clone();
            if self.ignores(&explosion, &record) {
                continue;
            }
            let Some(hit) = self.hit_of(&explosion, &record) else {
                continue;
            };
            let resists = match &record.data {
                EntityData::Mob { .. } => Mob::decode(&record.data).map_or(0.0, |mob| {
                    mob.body.attributes.value(
                        record.kind,
                        attribute(clustine_data::attributes::EXPLOSION_KNOCKBACK_RESISTANCE),
                    )
                }),
                _ => 0.0,
            };
            let push = physics::scale(hit.push, 1.0 - resists);
            self.push_from_explosion(record.key, push, 0, output);
        }
        self.player_pushes(at, reach, &bounds)
    }

    /// `interactWithBlocks(positions)` of a burst that triggers: the positions shuffled
    /// (`Util.shuffle` of the level's random), then each block's `onExplosionHit` with
    /// `canTriggerBlocks()` true, in that order. Only the region's own blocks.
    fn burst_trigger(
        &mut self,
        source: EntityId,
        positions: Vec<BlockPos>,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        let mut positions = positions;
        let mut left = positions.len();
        while left > 1 {
            let bound = u32::try_from(left).unwrap_or(u32::MAX);
            let chosen = random.next_int(bound) as usize;
            positions.swap(left - 1, chosen);
            left -= 1;
        }
        for pos in positions {
            if !self.chunks.contains_key(&pos.chunk()) {
                continue;
            }
            let Some(state) = self.block(pos) else {
                continue;
            };
            self.triggered(source, pos, state, random, output);
        }
    }

    /// `onExplosionHit` of the block `state` at `pos` for a burst that triggers: what
    /// each block that answers does (`ButtonBlock`, `LeverBlock`, `DoorBlock`,
    /// `TrapDoorBlock`, `FenceGateBlock`, `BellBlock`, `AbstractCandleBlock`), each heard
    /// as by no one's hand.
    fn triggered(
        &mut self,
        source: EntityId,
        pos: BlockPos,
        state: clustine_data::BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        let _ = source;
        if is_class(state, BlockClass::ButtonBlock) {
            // `press(state, level, pos, null)`, unless it is pressed.
            if flag(state, "powered") {
                return;
            }
            let pressed = set_flag(state, "powered", true);
            self.set_block(pos, pressed, output);
            self.update_attached(pos, pressed, output);
            let delay = redstone::button_delay(state);
            self.schedule_tick(pos, Ticked::Block(tick_id(state)), delay, 0);
            let sound = redstone::button_sound(state, true);
            redstone::play(pos, sound, 1.0, 1.0, None, output);
            self.vibrate_at(Vibration::BLOCK_ACTIVATE, pos, Context::of(None, None));
        } else if is_class(state, BlockClass::LeverBlock) {
            // `pull(state, level, pos, null)`.
            let pulled = set_flag(state, "powered", !flag(state, "powered"));
            self.set_block(pos, pulled, output);
            self.update_attached(pos, pulled, output);
            let on = flag(pulled, "powered");
            let pitch = if on { 0.6 } else { 0.5 };
            redstone::play(pos, sounds::BLOCK_LEVER_CLICK, 0.3, pitch, None, output);
            let event = if on {
                Vibration::BLOCK_ACTIVATE
            } else {
                Vibration::BLOCK_DEACTIVATE
            };
            self.vibrate_at(event, pos, Context::of(None, None));
        } else if is_class(state, BlockClass::DoorBlock) {
            // `setOpen(null, level, state, pos, !open)` of the lower half of a door the
            // wind can open (not iron) that power does not hold.
            let lower = state.property("half") == Some("lower");
            if !lower || is_block(state, blocks::IRON_DOOR) || flag(state, "powered") {
                return;
            }
            let opened = set_flag(state, "open", !flag(state, "open"));
            self.set_block_updated(
                pos,
                opened,
                UPDATE_CLIENTS | UPDATE_IMMEDIATE,
                UPDATE_LIMIT,
                output,
            );
            self.burst_opened(pos, opened, random, output);
        } else if is_class(state, BlockClass::TrapDoorBlock) {
            // `toggle(state, level, pos, null)` of a trapdoor the wind can open.
            if is_block(state, blocks::IRON_TRAPDOOR) || flag(state, "powered") {
                return;
            }
            let opened = set_flag(state, "open", !flag(state, "open"));
            self.set_block_updated(pos, opened, UPDATE_CLIENTS, UPDATE_LIMIT, output);
            if flag(state, "waterlogged") {
                let water = clustine_blocks::fluid::Kind::Water;
                self.schedule_tick(pos, Ticked::Fluid(water.source()), water.tick_delay(), 0);
            }
            self.burst_opened(pos, opened, random, output);
        } else if is_class(state, BlockClass::FenceGateBlock) {
            // Opened or closed and set with every update, unless power holds it.
            if flag(state, "powered") {
                return;
            }
            let opened = set_flag(state, "open", !flag(state, "open"));
            self.set_block_updated(pos, opened, UPDATE_ALL, UPDATE_LIMIT, output);
            self.burst_opened(pos, opened, random, output);
        } else if is_class(state, BlockClass::BellBlock) {
            // `attemptToRing(level, pos, null)`: struck from the way it faces.
            self.ring_bell(pos, None, output);
        } else if is_class(state, BlockClass::AbstractCandleBlock) && flag(state, "lit") {
            // `AbstractCandleBlock.extinguish(null, state, level, pos)`: put out with
            // every update and its neighbours told, its particles the client's.
            let out = set_flag(state, "lit", false);
            self.set_block_updated(
                pos,
                out,
                UPDATE_ALL | UPDATE_IMMEDIATE,
                UPDATE_LIMIT,
                output,
            );
            redstone::play(pos, sounds::BLOCK_CANDLE_EXTINGUISH, 1.0, 1.0, None, output);
            self.vibrate_at(Vibration::BLOCK_CHANGE, pos, Context::of(None, None));
        }
    }

    /// The sound of a door, trapdoor or gate the burst opened or closed, its pitch the
    /// level's `nextFloat() · 0.1 + 0.9`, and its `block_open` or `block_close` of no
    /// one.
    fn burst_opened(
        &mut self,
        pos: BlockPos,
        state: clustine_data::BlockState,
        random: &mut Random,
        output: &mut TickOutput,
    ) {
        let pitch = random.next_float() * 0.1 + 0.9;
        if let Some(sound) = crate::placement::use_sound(state) {
            redstone::play(pos, sound, 1.0, pitch, None, output);
        }
        self.vibrate_open(pos, state, None);
    }
}
