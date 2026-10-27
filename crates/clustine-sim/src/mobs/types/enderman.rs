//! The enderman (`Enderman` and its inner goals; `docs/groundwork/n9-plan.md`, section
//! 8.4; n9 Enderman), with `randomTeleport` (`region/teleports.rs`) and the blocks it
//! carries.
//!
//! It strolls on land (never into water, whose malus is −1), looks about, takes a block
//! of `#enderman_holdable` now and then (one ask in ten of its goal ticks, the block
//! three `nextDouble` away within 2 across and 3 up, seen by an outline clip from its own
//! column) and puts it down again (one ask in a thousand, within a block across and two
//! up, on a full block that is no bedrock, where the block can stay and nothing stands).
//! A block it carries keeps it in the world and drops at its death (its item, as a
//! silk-touched axe's loot gives every holdable block).
//!
//! It teleports: by day in light above a half under the open sky, ten seconds after it
//! last had a target, by `nextFloat() · 30 < (f − 0.4) · 2`; hurt by what no living thing
//! caused, nine times in ten; a projectile it refuses (unless it rides), teleporting up to
//! 64 times until one lands; a thrown potion's hurt is followed by the same. Water hurts
//! it (`isSensitiveToWater`: drowning's 1 a tick in water). It fights endermites and what
//! hurts it, and is angry as a `NeutralMob` is (`neutral.rs`), its anger 20 to 39 seconds.
//!
//! Its target's change shows: screaming (`DATA_CREEPY`, 17) and 0.15 faster while it has
//! one, both gone with it. Every player is creative, whom no combat test takes: it never
//! looks for one (`EndermanLookForPlayerGoal` finds none and draws nothing), never freezes
//! under one's stare, and is never stared at (18).
//!
//! **Across a line** (section 8.4): a block it takes in a copy is a change set only if
//! the block is still what it read, one it puts down there only if the place is still
//! air (`Region::set_block_updated`'s `SetIf`); its carried block changes as the change is
//! sent. Named, both: a take refused at the holder leaves it holding a block the world
//! still has, a duplicate; a put-down refused there loses the block.
//!
//! Named differences: a teleport its hurt asks for is made at the next point of its tick
//! (the start of its next tick for a hurt between its ticks, the start of `aiStep` for one
//! met in its `baseTick`, at once for its water's), drawing then from its tick's stream,
//! where the game teleports within the hurt; a harming splash, which the region does not
//! hurt it with, does not teleport it; its `targetChangeTime` is the tick count of the
//! latest tick it held a target in, where the game's is that of its latest `setTarget`
//! (each full selector tick while it holds one); the carried block's shape is updated
//! from its neighbours without the ticks that would schedule; rain is Clustine's none.

use clustine_blocks::data::in_tag;
use clustine_data::{BlockState, Face, PathType, attributes, entity_types, sounds};
use clustine_world::{BlockPos, EntityId, ItemStack, TypeState};
use serde::{Deserialize, Serialize};

use super::neutral::{self, Anger};
use super::{MobType, polar_bear, tamable};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    FloatGoal, LookAtPlayerGoal, MeleeAttackGoal, RandomLookAroundGoal, RandomStrollGoal, Stroll,
};
use crate::mobs::ai::maths::reduced_tick_delay;
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::Class;
use crate::mobs::ai::targets::{self, NearestTarget, Target};
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::attributes::ADD_VALUE;
use crate::mobs::body::hurt::{DamageSource, DamageType, Struck};
use crate::mobs::body::{Event, Modifier, attribute};
use crate::mobs::tick::{Local, MobTick};
use crate::physics::aabb::Aabb;
use crate::physics::{self, Vec3};
use crate::region::Random;
use crate::region::teleports;
use crate::shown::DataShown;

/// Whether the type is whole, so that `built()` gives it and an egg makes it.
pub(crate) const BUILT: bool = true;

/// The version of an enderman's type state.
const VERSION: u8 = 1;

/// The synced data's indices: the carried block, screaming, stared at.
const CARRIED_INDEX: u8 = 16;
const CREEPY_INDEX: u8 = 17;
const STARED_AT_INDEX: u8 = 18;

/// `SPEED_MODIFIER_ATTACKING`: `minecraft:attacking`, 0.15 added.
const ATTACKING: &str = "minecraft:attacking";
const ATTACKING_SPEED: f64 = 0.150_000_005_960_464_48;

/// `teleport()`'s reach each way (`MAX_TELEPORT_RADIUS`).
const REACH: f64 = 64.0;

/// What its hurt asked of it, done at the next point of its tick (see the module's
/// comment).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Pending {
    /// `teleport()`, once.
    Once,
    /// `repeatedlyTryToTeleport()`: up to 64 tries until one lands.
    Repeatedly,
}

/// What an enderman keeps: the block it carries (a state's id), its synced flags, the
/// tick count it last held a target at and that target, its anger, a teleport its hurt
/// asked for, and whether it rode as its latest hurt began.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct EndermanState {
    pub(crate) carried: Option<u16>,
    pub(crate) creepy: bool,
    pub(crate) stared_at: bool,
    pub(crate) target_change_time: u32,
    pub(crate) last_target: Option<EntityId>,
    pub(crate) anger: Anger,
    pub(crate) pending: Option<Pending>,
    pub(crate) hurt_riding: bool,
}

/// An enderman's state from its type state; a record of nothing, or one this build
/// cannot read, as a fresh enderman's.
pub(crate) fn state_of(mob: &Mob) -> EndermanState {
    if mob.state.bytes.is_empty() || mob.state.version != VERSION {
        return EndermanState::default();
    }
    postcard::from_bytes(&mob.state.bytes).unwrap_or_default()
}

/// The state changed by `change`, written back where it changed.
pub(crate) fn change(mob: &mut Mob, change: impl FnOnce(&mut EndermanState)) {
    let mut own = state_of(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        mob.state = if own == EndermanState::default() {
            TypeState::default()
        } else {
            TypeState {
                version: VERSION,
                bytes: postcard::to_allocvec(&own).expect("a type state is encoded into memory"),
            }
        };
    }
}

/// `getCarriedBlock()`.
pub(crate) fn carried(mob: &Mob) -> Option<BlockState> {
    state_of(mob).carried.map(BlockState)
}

const fn kind() -> u16 {
    entity_types::ENDERMAN as u16
}

/// The endermites it fights.
const ENDERMITES: &[u16] = &[entity_types::ENDERMITE as u16];

/// `Enderman.registerGoals`' targets: the player it looks for (finding none), who hurt
/// it, endermites it sees (interval 10), and the universal anger's reset.
pub(crate) const TARGETS: &[(i32, Target)] = &[
    (
        2,
        Target::HurtBy {
            ignore: &[],
            alert: false,
        },
    ),
    (3, Target::Nearest(NearestTarget::mobs(ENDERMITES, true))),
];

/// `Enderman.registerGoals`.
fn register_goals(setup: &mut Setup) {
    setup.set_malus(PathType::Water, -1.0);
    setup.add_goal(0, FloatGoal);
    setup.add_goal(1, EndermanFreezeWhenLookedAt);
    setup.add_goal(
        2,
        MeleeAttackGoal {
            speed: 1.0,
            following_even_if_not_seen: false,
        },
    );
    setup.add_goal(
        7,
        RandomStrollGoal {
            kind: Stroll::AvoidingWater { probability: 0.0 },
            ..RandomStrollGoal::new(1.0)
        },
    );
    setup.add_goal(8, LookAtPlayerGoal::new(Class::Player, 8.0));
    setup.add_goal(8, RandomLookAroundGoal);
    setup.add_goal(10, EndermanLeaveBlockGoal);
    setup.add_goal(11, EndermanTakeBlockGoal);
    setup.add_target(1, EndermanLookForPlayerGoal);
    targets::register(setup, TARGETS);
    // `ResetUniversalAngerTargetGoal(false)`: only with `universal_anger`, which is off.
    setup.add_target(4, tamable::Idle { flags: 0 });
}

/// `EndermanLookForPlayerGoal`: a `NearestAttackableTargetGoal` of players whose `canUse`
/// is its own, the nearest player within its follow range by combat conditions that its
/// stare or its anger provokes, no interval drawn. Every player being creative, whom
/// combat conditions refuse, it finds none, and nothing else of it is reached.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EndermanLookForPlayerGoal;

impl Goal for EndermanLookForPlayerGoal {
    type State = ();

    fn flags(&self) -> u8 {
        flags::TARGET
    }

    fn can_use(&self, _: &mut (), _: &mut Ai<'_, '_>) -> bool {
        false
    }
}

/// `EndermanFreezeWhenLookedAt`: still and looking back at a player target within 16 who
/// stares at it. Its target is never a player (every player being creative).
#[derive(Debug, Clone, Copy)]
pub(crate) struct EndermanFreezeWhenLookedAt;

impl Goal for EndermanFreezeWhenLookedAt {
    type State = ();

    fn flags(&self) -> u8 {
        flags::JUMP | flags::MOVE
    }

    /// `canUse`: a player target within 16 who stares at it (`isBeingStaredBy`). No
    /// player is ever its target, so it is never used, and the stare is not tested.
    fn can_use(&self, _: &mut (), _: &mut Ai<'_, '_>) -> bool {
        false
    }
}

/// `Direction.UPDATE_SHAPE_ORDER`.
const UPDATE_SHAPE_ORDER: [Face; 6] = [
    Face::West,
    Face::East,
    Face::North,
    Face::South,
    Face::Down,
    Face::Up,
];

/// The block next to `pos` towards `face`.
fn beside(pos: BlockPos, face: Face) -> BlockPos {
    let [dx, dy, dz] = match face {
        Face::Down => [0, -1, 0],
        Face::Up => [0, 1, 0],
        Face::North => [0, 0, -1],
        Face::South => [0, 0, 1],
        Face::West => [-1, 0, 0],
        Face::East => [1, 0, 0],
    };
    BlockPos::new(pos.x + dx, pos.y + dy, pos.z + dz)
}

/// `EndermanLeaveBlockGoal`: with a block, one ask in 1000 of its goal ticks
/// (`reducedTickDelay(2000)`, `mob_griefing` being on), it puts it down within a block
/// across and two up where it can (`canPlaceBlock`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct EndermanLeaveBlockGoal;

impl Goal for EndermanLeaveBlockGoal {
    type State = ();

    fn flags(&self) -> u8 {
        0
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        if carried(&ai.tick.mob).is_none() {
            return false;
        }
        ai.tick.draws.next_int(reduced_tick_delay(2000) as u32) == 0
    }

    fn tick(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        let [x, y, z] = ai.position();
        let x = physics::floor(x - 1.0 + ai.tick.draws.next_double() * 2.0);
        let y = physics::floor(y + ai.tick.draws.next_double() * 2.0);
        let z = physics::floor(z - 1.0 + ai.tick.draws.next_double() * 2.0);
        let pos = BlockPos::new(x, y, z);
        let world = ai.tick.world;
        let at = world.state(pos);
        let below_pos = BlockPos::new(x, y - 1, z);
        let below = world.state(below_pos);
        let Some(held) = carried(&ai.tick.mob) else {
            return;
        };
        // `Block.updateFromNeighbourShapes`.
        let mut placed = held;
        for face in UPDATE_SHAPE_ORDER {
            let neighbour = world.state(beside(pos, face));
            placed = clustine_blocks::shape::update_shape(
                &mut physics::Rules(world),
                placed,
                pos,
                face,
                neighbour,
            );
        }
        if !can_place_block(ai, pos, placed, at, below) {
            return;
        }
        ai.tick
            .out
            .local
            .push(Local::PlaceBlock { pos, state: placed });
        change(&mut ai.tick.mob, |own| own.carried = None);
    }
}

/// `canPlaceBlock(level, pos, carried, at, below, belowPos)`: air there on a block below
/// that is neither air nor bedrock and is a full block, where the block can stay and no
/// other entity's box meets the place.
fn can_place_block(
    ai: &Ai<'_, '_>,
    pos: BlockPos,
    placed: BlockState,
    at: BlockState,
    below: BlockState,
) -> bool {
    if !at.is_air()
        || below.is_air()
        || below == clustine_data::blocks::BEDROCK
        || !below.props().collision_is_full_block()
    {
        return false;
    }
    if !clustine_blocks::survive::can_survive(placed, pos, &mut physics::Rules(ai.tick.world)) {
        return false;
    }
    let cube = Aabb::new(
        [f64::from(pos.x), f64::from(pos.y), f64::from(pos.z)],
        [
            f64::from(pos.x) + 1.0,
            f64::from(pos.y) + 1.0,
            f64::from(pos.z) + 1.0,
        ],
    );
    let entities = ai
        .tick
        .world
        .entities_in(&cube)
        .iter()
        .any(|seen| seen.entity.key != ai.tick.key);
    let players = ai.tick.world.players().iter().any(|player| {
        let height = if player.sneaking { 1.5 } else { 1.8 };
        Aabb::of_entity(player.position, 0.6, height).intersects(cube)
    });
    !entities && !players
}

/// `EndermanTakeBlockGoal`: with nothing carried, one ask in 10 of its goal ticks
/// (`reducedTickDelay(20)`), it takes a block of `#enderman_holdable` within 2 across
/// and 3 up that an outline clip from its own column at that height meets first.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EndermanTakeBlockGoal;

impl Goal for EndermanTakeBlockGoal {
    type State = ();

    fn flags(&self) -> u8 {
        0
    }

    fn can_use(&self, _: &mut (), ai: &mut Ai<'_, '_>) -> bool {
        if carried(&ai.tick.mob).is_some() {
            return false;
        }
        ai.tick.draws.next_int(reduced_tick_delay(20) as u32) == 0
    }

    fn tick(&self, _: &mut (), ai: &mut Ai<'_, '_>) {
        let [x, y, z] = ai.position();
        let x = physics::floor(x - 2.0 + ai.tick.draws.next_double() * 4.0);
        let y = physics::floor(y + ai.tick.draws.next_double() * 3.0);
        let z = physics::floor(z - 2.0 + ai.tick.draws.next_double() * 4.0);
        let pos = BlockPos::new(x, y, z);
        let world = ai.tick.world;
        let state = world.state(pos);
        let own = ai.block_position();
        let from = [
            f64::from(own.x) + 0.5,
            f64::from(y) + 0.5,
            f64::from(own.z) + 0.5,
        ];
        let to = [f64::from(x) + 0.5, f64::from(y) + 0.5, f64::from(z) + 0.5];
        // A miss is at the end of the clip, which is the block's own.
        let met = clip_outline(world, from, to).unwrap_or_else(|| physics::containing(to));
        if !in_tag(state, "minecraft:enderman_holdable") || met != pos {
            return;
        }
        // `removeBlock(pos, false)`: its fluid's block in its place, every update.
        let left = clustine_blocks::fluid::FluidState::of(state).legacy_block();
        ai.tick.out.local.push(Local::SetBlock {
            pos,
            state: left,
            flags: 3,
        });
        ai.tick.out.local.push(Local::GameEventAt {
            name: "minecraft:block_destroy",
            at: [f64::from(x) + 0.5, f64::from(y) + 0.5, f64::from(z) + 0.5],
            affected: Some(state),
        });
        let taken = state.block().map_or(state, |block| block.default_state);
        change(&mut ai.tick.mob, |own| own.carried = Some(taken.0));
    }
}

/// `Level.clip(ClipContext(from, to, OUTLINE, NONE, the enderman))`: the first block on
/// the way whose outline shape the segment meets; `None` for a miss.
fn clip_outline(world: &dyn crate::mobs::view::MobWorld, from: Vec3, to: Vec3) -> Option<BlockPos> {
    let mut met = None;
    physics::walk::traverse(from, to, |pos| {
        let state = world.state(pos);
        let hits = state
            .props()
            .outline_shape()
            .boxes()
            .any(|[x0, y0, z0, x1, y1, z1]| {
                let shape = Aabb::new(
                    [
                        f64::from(pos.x) + x0,
                        f64::from(pos.y) + y0,
                        f64::from(pos.z) + z0,
                    ],
                    [
                        f64::from(pos.x) + x1,
                        f64::from(pos.y) + y1,
                        f64::from(pos.z) + z1,
                    ],
                );
                physics::body::clips_shape(&shape, from, to)
            });
        if hits {
            met = Some(pos);
        }
        hits
    });
    met
}

/// `teleport()` of the enderman in `tick`: a place drawn within 32 each way across
/// (`nextDouble`, `nextInt(64)`, `nextDouble`), tried. Nothing, and no draw, for one not
/// alive.
pub(crate) fn teleport(tick: &mut MobTick<'_>) -> bool {
    if tick.mob.body.vitals.dying() {
        return false;
    }
    let [x, y, z] = tick.living.body.position;
    let x = x + (tick.draws.next_double() - 0.5) * REACH;
    let y = y + f64::from(tick.draws.next_int(64) as i32 - 32);
    let z = z + (tick.draws.next_double() - 0.5) * REACH;
    teleport_to(tick, [x, y, z])
}

/// `teleport(x, y, z)`: `randomTeleport(x, y, z, true, #enderman_does_not_teleport_to)`
/// of one riding nothing; where it lands, the game event of the teleport where it was,
/// and its sound there (at where its tick began) and where it is.
fn teleport_to(tick: &mut MobTick<'_>, to: Vec3) -> bool {
    if tick.vehicle.is_some() {
        return false;
    }
    let from = tick.living.body.position;
    let reached = {
        let who = teleports::enderman(&tick.living.body.mover);
        teleports::random_teleport(tick.world, to, &who)
    };
    let Some(reached) = reached else {
        return false;
    };
    // `teleportTo(x, y, z)`: there, still; its move sent; the portal's particles shown
    // (`broadcastEntityEvent(46)`); its navigation stopped.
    tick.living.body.position = reached;
    tick.living.body.motion = [0.0; 3];
    tick.out.needs_sync = true;
    tick.out.events.push(Event::Entity(46));
    tick.mob.body.path.followed = None;
    tick.out.local.push(Local::GameEventAt {
        name: "minecraft:teleport",
        at: from,
        affected: None,
    });
    tick.out.local.push(Local::Sound {
        sound: sounds::ENTITY_ENDERMAN_TELEPORT,
        source: crate::mobs::body::ladder::sound_source(kind()),
        at: tick.living.body.old_position,
        volume: 1.0,
        pitch: 1.0,
    });
    tick.out.events.push(Event::Sound {
        sound: sounds::ENTITY_ENDERMAN_TELEPORT,
        volume: 1.0,
        pitch: 1.0,
    });
    true
}

/// The teleport its hurt asked for, done now (see the module's comment).
fn run_pending(tick: &mut MobTick<'_>) {
    let Some(pending) = state_of(&tick.mob).pending else {
        return;
    };
    change(&mut tick.mob, |own| own.pending = None);
    match pending {
        Pending::Once => {
            teleport(tick);
        }
        // `repeatedlyTryToTeleport()`: not riding, up to 64 tries until one lands.
        Pending::Repeatedly => {
            if tick.vehicle.is_none() {
                for _ in 0..64 {
                    if teleport(tick) {
                        break;
                    }
                }
            }
        }
    }
}

/// `Enderman.setTarget(target)` for the target it holds now, where it changed or it holds
/// one: the time of the change, screaming and the attacking speed with a target, and
/// neither, nor the stare, without.
fn sync_target(tick: &mut MobTick<'_>) {
    let target = tick.mob.body.combat.target;
    let own = state_of(&tick.mob);
    if target.is_none() && own.last_target.is_none() {
        return;
    }
    let tick_count = tick.tick_count.saturating_add(1);
    change(&mut tick.mob, |own| {
        own.last_target = target;
        if target.is_some() {
            own.target_change_time = tick_count;
            own.creepy = true;
        } else {
            own.target_change_time = 0;
            own.creepy = false;
            own.stared_at = false;
        }
    });
    let speed = tick
        .mob
        .body
        .attributes
        .instance(kind(), attribute(attributes::MOVEMENT_SPEED));
    if let Some(speed) = speed {
        if target.is_some() {
            if !speed.has_modifier(ATTACKING) {
                speed.add_modifier(Modifier {
                    id: ATTACKING.to_owned(),
                    amount: ATTACKING_SPEED,
                    operation: ADD_VALUE,
                });
            }
        } else {
            speed.remove_modifier(ATTACKING);
        }
    }
}

/// `Entity.getLightLevelDependentMagicValue()` at its eyes: nothing where the view has
/// no light.
fn magic_value(tick: &MobTick<'_>) -> f32 {
    let at = tick.living.body.position;
    let eyes = physics::containing([at[0], at[1] + f64::from(tick.living.eye_height), at[2]]);
    let Some(light) = tick.world.light(eyes) else {
        return 0.0;
    };
    let raw = (i32::from(light.sky) - tick.world.sky_darken()).max(i32::from(light.block));
    let f = raw as f32 / 15.0;
    f / (4.0 - 3.0 * f)
}

/// The enderman.
pub(crate) struct Enderman;

impl MobType for Enderman {
    fn register_goals(&self, setup: &mut Setup) {
        register_goals(setup);
    }

    fn before_tick(&self, tick: &mut MobTick<'_>) {
        run_pending(tick);
    }

    /// `Enderman.aiStep` before `Monster`'s: not jumping, its anger brought up to date
    /// (its timer drawn again every tick it has a target).
    fn before_ai_step(&self, tick: &mut MobTick<'_>) {
        run_pending(tick);
        tick.mob.body.inputs.jumping = false;
        let mut own = state_of(&tick.mob);
        let before = own.anger.clone();
        let now = tick.world.game_time();
        neutral::update_persistent_anger(
            &mut tick.mob,
            &mut own.anger,
            tick.world,
            now,
            true,
            tick.draws,
            polar_bear::anger_time,
        );
        if own.anger != before {
            change(&mut tick.mob, |kept| kept.anger = own.anger);
        }
    }

    /// `customServerAiStep`: its target's change kept, then by day in the light under the
    /// open sky, ten seconds after its last target, a teleport by chance.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        sync_target(tick);
        let own = state_of(&tick.mob);
        let tick_count = tick.tick_count.saturating_add(1);
        if tick.world.sky_darken() >= 4 || tick_count < own.target_change_time.saturating_add(600) {
            return;
        }
        let f = magic_value(tick);
        let block = physics::containing(tick.living.body.position);
        let sees_sky = tick.world.light(block).is_none_or(|light| light.sky >= 15);
        if f > 0.5 && sees_sky && tick.draws.next_float() * 30.0 < (f - 0.4) * 2.0 {
            tick.mob.body.combat.target = None;
            sync_target(tick);
            teleport(tick);
        }
    }

    /// `LivingEntity.aiStep`'s water hurt (`isSensitiveToWater`, in water: drowning's 1),
    /// and the teleport it asks for.
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        if tick.living.fluids.in_water() && !tick.mob.body.vitals.dying() {
            crate::mobs::body::hurt_by(tick, &DamageSource::of(DamageType::DROWN), 1.0);
        }
        run_pending(tick);
    }

    fn after_tick(&self, tick: &mut MobTick<'_>) {
        run_pending(tick);
    }

    /// `Enderman.hurtServer`'s head: whether it rides as the hurt begins, and a
    /// projectile's or a thrown potion's teleports asked for.
    fn before_hurt_server(&self, struck: &mut Struck<'_>, source: &DamageSource, _: f32) {
        let riding = struck.vehicle.is_some();
        let potion = source.direct.as_ref().is_some_and(|direct| {
            matches!(
                i32::from(direct.kind),
                entity_types::SPLASH_POTION | entity_types::LINGERING_POTION
            )
        });
        let projectile = source.kind.is("minecraft:is_projectile") && !riding;
        change(struck.mob, |own| {
            own.hurt_riding = riding;
            if projectile || potion {
                own.pending = Some(Pending::Repeatedly);
            }
        });
    }

    /// A projectile it refuses, unless it rides.
    fn refuses_hurt(&self, mob: &Mob, source: &DamageSource) -> bool {
        source.kind.is("minecraft:is_projectile") && !state_of(mob).hurt_riding
    }

    /// `Enderman.hurtServer`'s tail: after a hurt of what no living thing caused (not a
    /// projectile's or a potion's, which ask their own), `nextInt(10)`, and a teleport
    /// unless it is 0.
    fn after_hurt(&self, struck: &mut Struck<'_>, source: &DamageSource, _: bool) {
        let riding = state_of(struck.mob).hurt_riding;
        let potion = source.direct.as_ref().is_some_and(|direct| {
            matches!(
                i32::from(direct.kind),
                entity_types::SPLASH_POTION | entity_types::LINGERING_POTION
            )
        });
        let projectile = source.kind.is("minecraft:is_projectile") && !riding;
        change(struck.mob, |own| own.hurt_riding = false);
        if projectile || potion {
            return;
        }
        let living = source.cause.as_ref().is_some_and(|cause| cause.living);
        if !living && struck.draws.next_int(10) != 0 {
            change(struck.mob, |own| own.pending = Some(Pending::Once));
        }
    }

    fn requires_custom_persistence(&self, mob: &Mob) -> bool {
        carried(mob).is_some()
    }

    /// `dropCustomDeathLoot`'s carried block, after `Mob`'s: what a silk-touched axe
    /// breaks it into, which for every holdable block is its own item.
    fn drop_equipment(&self, mob: &mut Mob) -> Vec<ItemStack> {
        let Some(held) = carried(mob) else {
            return Vec::new();
        };
        change(mob, |own| own.carried = None);
        crate::region::item_of_block(held)
            .map(|item| ItemStack::new(item, 1))
            .into_iter()
            .collect()
    }

    fn ambient_sound(&self, mob: &Mob) -> Option<i32> {
        Some(if state_of(mob).creepy {
            sounds::ENTITY_ENDERMAN_SCREAM
        } else {
            sounds::ENTITY_ENDERMAN_AMBIENT
        })
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_ENDERMAN_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_ENDERMAN_DEATH)
    }

    fn experience_reward(&self, _: u16, mob: &Mob, draws: &mut Random) -> i32 {
        super::illagers::experience(5, mob, draws)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let own = state_of(mob);
        vec![
            (
                CARRIED_INDEX,
                DataShown::OptionalBlockState(own.carried),
                own.carried.is_none(),
            ),
            (CREEPY_INDEX, DataShown::Boolean(own.creepy), !own.creepy),
            (
                STARED_AT_INDEX,
                DataShown::Boolean(own.stared_at),
                !own.stared_at,
            ),
        ]
    }
}
