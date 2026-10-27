//! `SpawnUtil.trySpawnMob` (n7 SpawnUtil 9–160, `SpawnUtil$Strategy` 55–137): a mob made
//! near a place by a few tries, each a drawn column and the first spot down it that the
//! strategy takes, as a villager summons an iron golem (`docs/groundwork/n10-plan.md`,
//! section 7; `n10-dumps.md`, part B, correction 2).
//!
//! It reads the world as the mob that asks reads it ([`MobWorld`]) and draws from that
//! mob's stream, where the game's level draws on its behalf (A28 section 4): for each try
//! two `Mth.randomBetweenInclusive(-range, range)`, and where a spot is found the new
//! mob's yaw (`EntityType.create`'s `snapTo`) and its `finalizeSpawn`'s draws. What it
//! makes is the caller's to add (`Local::MakeMob`), which the region constructs with the
//! draws of its own stream, as the game's constructor draws its own.
//!
//! Named: the world border is not asked (no village is near it); the yaw drawn is the
//! mob's head and body's, its record's own being the one `Local::MakeMob` makes, south.

use clustine_blocks::data::{in_tag, is_block, is_class};
use clustine_data::{BlockClass, BlockState, EntityTypeInfo, Face, SupportType, blocks};
use clustine_world::{BlockPos, EntityData};

use super::Random;
use crate::mobs::Mob;
use crate::mobs::types::{SpawnReason, Spawning};
use crate::mobs::view::MobWorld;
use crate::physics::Vec3;
use crate::physics::aabb::Aabb;

/// `SpawnUtil$Strategy`: which spot down a column takes a mob. Only the iron golem's
/// legacy one is asked for so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Strategy {
    /// `LEGACY_IRON_GOLEM`: on a solid block (or powder snow) that is none of glass,
    /// panes, leaves, ice, cobweb, cactus, conduit, TNT, glowstone, a beacon or a sea
    /// lantern, under air or a liquid.
    LegacyIronGolem,
}

impl Strategy {
    /// `canSpawnOn(level, below, belowState, above, aboveState)`.
    fn can_spawn_on(self, below: BlockState, above: BlockState) -> bool {
        match self {
            Strategy::LegacyIronGolem => {
                let refused = [
                    blocks::COBWEB,
                    blocks::CACTUS,
                    blocks::GLASS_PANE,
                    blocks::CONDUIT,
                    blocks::ICE,
                    blocks::TNT,
                    blocks::GLOWSTONE,
                    blocks::BEACON,
                    blocks::SEA_LANTERN,
                    blocks::FROSTED_ICE,
                    blocks::TINTED_GLASS,
                    blocks::GLASS,
                ]
                .iter()
                .any(|block| is_block(below, *block))
                    || is_class(below, BlockClass::StainedGlassPaneBlock)
                    || is_class(below, BlockClass::StainedGlassBlock)
                    || is_class(below, BlockClass::LeavesBlock);
                if refused {
                    return false;
                }
                let open = above.props().is_air() || above.props().is_liquid();
                open && (below.props().is_solid() || is_block(below, blocks::POWDER_SNOW))
            }
        }
    }
}

/// What the world reads at `pos`: air where the view has no block.
fn state_at(world: &dyn MobWorld, pos: BlockPos) -> BlockState {
    world.state(pos)
}

/// `moveToPossibleSpawnPosition(level, yOffset, pos, strategy)`: from `pos` down to
/// `yOffset` below the place the column was drawn about, the first block the strategy
/// takes to stand on; the place above it.
fn possible_spawn_position(
    world: &dyn MobWorld,
    y_offset: i32,
    mut pos: BlockPos,
    strategy: Strategy,
) -> Option<BlockPos> {
    let mut above_state = state_at(world, pos);
    for _ in (-y_offset..=y_offset).rev() {
        pos = pos.offset(0, -1, 0);
        let state = state_at(world, pos);
        if strategy.can_spawn_on(state, above_state) {
            return Some(pos.offset(0, 1, 0));
        }
        above_state = state;
    }
    None
}

/// `NaturalSpawner.isValidEmptySpawnBlock(level, pos, state, fluid, type)`, with the
/// fluid asked only where `fluid` (the golem's own block is asked with no fluid).
fn valid_empty(state: BlockState, kind: u16, fluid: bool) -> bool {
    !state.voxel_collision_shape().is_block()
        && !state.props().is_signal_source()
        && (!fluid || state.props().fluid().is_none())
        && !in_tag(state, "minecraft:prevent_mob_spawning_inside")
        && !clustine_data::spawning::is_block_dangerous(kind, u32::from(state.0))
}

/// `Level.isUnobstructed(mob)`: no entity that blocks building (a living one, a boat, a
/// minecart, an armour stand that is no marker) and no player but a spectator in its
/// box.
fn unobstructed(world: &dyn MobWorld, bounds: &Aabb) -> bool {
    let blocked = world.entities_in(bounds).iter().any(|seen| {
        matches!(seen.entity.data, EntityData::Mob { .. })
            || crate::physics::entities::blocks_building(seen.entity.kind, &seen.entity.data)
    });
    if blocked {
        return false;
    }
    !world.players().iter().any(|player| {
        let height = if player.sneaking { 1.5 } else { 1.8 };
        !player.spectator && Aabb::of_entity(player.position, 0.6, height).intersects(*bounds)
    })
}

/// `Mob.checkSpawnObstruction(level)` of a mob of `kind` at `position`: an iron golem's
/// own (`IronGolem.checkSpawnObstruction`, n7 IronGolem 670–733: it stands on what it
/// can, the two blocks above its feet' are empty, its feet's block empty of all but a
/// fluid, and nothing in its box); any other's no liquid in its box and nothing in it.
fn spawn_obstruction(world: &dyn MobWorld, kind: u16, position: Vec3) -> bool {
    let info = EntityTypeInfo::of(i32::from(kind));
    let bounds = Aabb::of_entity(position, info.width, info.height);
    if i32::from(kind) == clustine_data::entity_types::IRON_GOLEM {
        let at = crate::physics::containing(position);
        // `entityCanStandOn`: the top of its collision shape is full (**Inferred** from
        // the face's sturdiness, as the navigation reads it).
        if !state_at(world, at.offset(0, -1, 0))
            .props()
            .is_face_sturdy(Face::Up, SupportType::Full)
        {
            return false;
        }
        for up in 1..3 {
            if !valid_empty(state_at(world, at.offset(0, up, 0)), kind, true) {
                return false;
            }
        }
        return valid_empty(state_at(world, at), kind, false) && unobstructed(world, &bounds);
    }
    let low = crate::physics::containing(bounds.min);
    let high = crate::physics::containing(bounds.max);
    let liquid = (low.x..=high.x).any(|x| {
        (low.y..=high.y).any(|y| {
            (low.z..=high.z).any(|z| {
                state_at(world, BlockPos::new(x, y, z))
                    .props()
                    .fluid()
                    .is_some()
            })
        })
    });
    !liquid && unobstructed(world, &bounds)
}

/// The draws and what a `finalizeSpawn` needs to know of the place it is made, beyond
/// the world: the day's clock and the game time.
pub(crate) struct Making<'a> {
    pub(crate) world: &'a dyn MobWorld,
    pub(crate) draws: &'a mut Random,
    pub(crate) day: u64,
    pub(crate) game_time: i64,
}

/// `SpawnUtil.trySpawnMob(type, reason, level, pos, attempts, range, yOffset, strategy,
/// checkCollision)`: the mob of `kind` finalised where a try found room, and the point it
/// stands at; none where every try failed. A try that finds a spot and is then refused by
/// the mob's obstruction made its draws (its yaw and its `finalizeSpawn`), as the game's
/// discarded mob did.
#[expect(
    clippy::too_many_arguments,
    reason = "`SpawnUtil.trySpawnMob`'s arguments"
)]
pub(crate) fn try_spawn_mob(
    making: &mut Making<'_>,
    kind: u16,
    reason: SpawnReason,
    pos: BlockPos,
    attempts: i32,
    range: i32,
    y_offset: i32,
    strategy: Strategy,
    check_collision: bool,
) -> Option<(Mob, Vec3)> {
    let world = making.world;
    let info = EntityTypeInfo::of(i32::from(kind));
    for _ in 0..attempts {
        let dx = making.draws.next_int_between(-range, range);
        let dz = making.draws.next_int_between(-range, range);
        let drawn = pos.offset(dx, y_offset, dz);
        let Some(at) = possible_spawn_position(world, y_offset, drawn, strategy) else {
            continue;
        };
        let position = [
            f64::from(at.x) + 0.5,
            f64::from(at.y),
            f64::from(at.z) + 0.5,
        ];
        if check_collision
            && !world
                .entities_in(&Aabb::of_entity(position, info.width, info.height))
                .is_empty()
        {
            continue;
        }
        // `EntityType.create`: its yaw, the level's draw, then its `finalizeSpawn` with
        // the difficulty at its block.
        let yaw = crate::mobs::ai::maths::wrap_degrees(making.draws.next_float() * 360.0);
        let mut mob = Mob::fresh();
        mob.body.head.head_yaw = yaw;
        mob.body.head.body_yaw = yaw;
        // Its own draws (its UUID, its constructor's) are the region's to make with its
        // key; no `finalizeSpawn` a summoning makes draws them.
        let mut own = Random::of_entity(0, clustine_world::EntityId(0), 0);
        let day = making.day;
        crate::mobs::making::finalize(
            kind,
            &mut Spawning {
                mob: &mut mob,
                level: &mut *making.draws,
                own: &mut own,
                biome: world.biome(at),
                at,
                day,
                game_time: making.game_time,
                reason,
                group: &mut None,
                structure: None,
                moon_brightness: crate::mobs::making::moon_brightness(day),
                difficulty: difficulty(day),
                companions: None,
            },
        );
        // `checkSpawnRules`: a mob's walk target value at its block, which is nothing for
        // the golem and every mob this is asked for, so it passes.
        if spawn_obstruction(world, kind, position) {
            return Some((mob, position));
        }
    }
    None
}

/// `getCurrentDifficultyAt(pos)` reckoned from the clock's day, as the region reckons it
/// (`region/difficulty.rs`; the inhabited time is not kept), and its special multiplier.
fn difficulty(day: u64) -> (f32, f32) {
    use clustine_world::Difficulty;
    let base = Difficulty::WORLD;
    if base == Difficulty::Peaceful {
        return (0.0, 0.0);
    }
    let level_time = day as f32;
    let t = ((level_time - 72_000.0) / 1_440_000.0).clamp(0.0, 1.0) * 0.25;
    let mut c = (crate::mobs::making::moon_brightness(day) * 0.25).clamp(0.0, t);
    if base == Difficulty::Easy {
        c *= 0.5;
    }
    let effective = base.id() as f32 * (0.75 + t + c);
    let special = if effective < 2.0 {
        0.0
    } else if effective > 4.0 {
        1.0
    } else {
        (effective - 2.0) / 2.0
    };
    (effective, special)
}
