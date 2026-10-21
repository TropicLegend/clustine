//! The order of a mob's tick, as the game's (`LivingEntity.tick`, `Mob.baseTick`,
//! `aiStep`, `serverAiStep`; the brief's 2.0.12), with each step in the file of the part
//! it belongs to: the body's in [`super::body`], the mind's and controls' in
//! [`super::ai`], the type's in [`super::types`].
//!
//! A tick borrows the mob alone mutably and the rest of the world as it was when the
//! tick began ([`MobWorld`]); what it does to other entities and blocks it writes into
//! [`MobOutput::local`], which the region applies after the mob's tick and before the
//! next entity's, as the messages of the effects across a line are applied.

use clustine_data::BlockState;
use clustine_world::{BlockPos, EntityId, ItemStack};

use super::types::MobType;
use super::view::MobWorld;
use super::{Mob, ai, body};
use crate::living::Living;
use crate::region::Random;

/// What a mob's tick does to other things than itself, and what it shows, applied in
/// order after it.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct MobOutput {
    /// Writes to other entities and blocks, in the order made.
    pub(crate) local: Vec<Local>,
    /// What clients are shown of it, in the order made (the body's sounds, entity
    /// events and damage events).
    pub(crate) events: Vec<body::Event>,
    /// What it drops as it dies, drawn as it died, for the region to lay down.
    pub(crate) died: Option<body::Death>,
    /// How it left the world, if it did.
    pub(crate) removed: Option<body::Removed>,
    /// `needsSync`: its move and motion are sent at the next update.
    pub(crate) needs_sync: bool,
    /// `syncVelocity` (a hurt's `markHurt`): its motion is sent, to everyone.
    pub(crate) sync_velocity: bool,
}

/// A write a mob's tick makes to something other than itself. Each part adds what it
/// needs, as the same messages as the effects across a line (`docs/groundwork/
/// n7-plan.md`, section 10, item 12).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Local {
    /// `Entity.push(x, y, z)` of the region's entity `key` (another mob a mob pushes in
    /// its `pushEntities`).
    Push { key: EntityId, by: [f64; 3] },
    /// An animal asks `partner` to breed, its request `number` (`region/breeding.rs`).
    BreedAsk { partner: EntityId, number: u32 },
    /// `level.setBlock(pos, state, flags)` (a sheep's grass become dirt, a carrot eaten).
    SetBlock {
        pos: BlockPos,
        state: BlockState,
        flags: u32,
    },
    /// `level.destroyBlock(pos, false)`: a plant a sheep ate, without drops.
    DestroyBlock { pos: BlockPos },
    /// `level.levelEvent(kind, pos, data)`.
    LevelEvent { kind: i32, pos: BlockPos, data: i32 },
    /// `level.gameEvent(event, pos, the mob)`, by the event's name.
    GameEvent { name: &'static str, pos: BlockPos },
    /// `level.gameEvent(event, at, Context.of(the mob, affected))`: an event at a point
    /// of its own (a step's, where its feet were), with the block it affects.
    GameEventAt {
        name: &'static str,
        at: crate::physics::Vec3,
        affected: Option<BlockState>,
    },
    /// `spawnAtLocation(stack)`: an item at `at` (a chicken's egg) with its pick-up
    /// delay.
    Drop {
        stack: ItemStack,
        at: crate::physics::Vec3,
        pick_up_delay: u16,
    },
    /// `level.playSound(null, at, sound, source, volume, pitch)`: a sound not of the
    /// mob's own source (a turtle's laying, of the blocks').
    Sound {
        sound: i32,
        source: i32,
        at: crate::physics::Vec3,
        volume: f32,
        pitch: f32,
    },
    /// `take(item, count)` and the item's `discard()`: the region's item entity `item`
    /// taken whole by the mob (a dolphin's pick-up), shown to clients as taken.
    TakeItem { item: EntityId, count: i32 },
    /// `take(item, count)` of `count` of the region's item entity `item`, the rest left
    /// lying (an allay's pick-up into a slot with room for only some), or gone where it
    /// took all.
    TakeSome { item: EntityId, count: i32 },
    /// `startFollowing(leader)` of each of `followers` (fish of the region's own, never
    /// the mob itself), and `leader_gains` more in the leader's school (none where the
    /// mob leads, which it counts itself).
    JoinSchool {
        leader: EntityId,
        followers: Vec<EntityId>,
        leader_gains: i32,
    },
    /// `stopFollowing()`'s write to the leader it left: one fewer in its school.
    LeaveSchool { leader: EntityId },
    /// `hurtServer(mobAttack(this), amount)` of the region's mob `key` by this one (of the
    /// type `kind`, its feet at `from`), and where it took, `sound` at `from` (a
    /// pufferfish's sting; a melee hit's `doHurtTarget`, of no sound). **Named**: a mob
    /// in a copy is not hurt (the entity effect across a line waits), nor a player.
    Hurt {
        key: EntityId,
        kind: u16,
        from: crate::physics::Vec3,
        amount: f32,
        sound: Option<i32>,
    },
    /// An item thrown by the mob (`new ItemEntity`, its pick-up delay set and its motion
    /// set after the constructor's draws: a dolphin's play).
    Throw {
        stack: ItemStack,
        at: crate::physics::Vec3,
        pick_up_delay: u16,
        motion: crate::physics::Vec3,
    },
    /// `ejectPassengers()` of the mob itself (a wild horse throwing its rider).
    EjectPassengers,
    /// `Block.popResource(level, pos, stack)`: an item popped out of the block at `pos`
    /// (berries a fox picks).
    Pop { pos: BlockPos, stack: ItemStack },
    /// `BeehiveBlockEntity.addOccupant(bee)`: the mob goes into the hive at `hive`, if
    /// it has room (`region/hives.rs`).
    EnterHive { hive: BlockPos },
    /// A bee's sting (`Bee.doHurtTarget`): `hurtServer(sting(this), amount)` of the
    /// region's mob `key` by this one (of the type `kind`, its feet at `from`), and where
    /// it took, the sting's sound at `from`. As [`Local::Hurt`], a mob in a copy and a
    /// player are not hurt.
    Sting {
        key: EntityId,
        kind: u16,
        from: crate::physics::Vec3,
        amount: f32,
    },
    /// `Tadpole.ageUp()`: the mob, a tadpole of age, made a frog of its biome where it
    /// is, and gone (`region/frogspawn.rs`).
    GrowUp,
}

/// A mob in its tick: its record decoded, its body as the physics have it, its type,
/// what it reads and what it draws from.
#[allow(
    dead_code,
    reason = "what the living body's and the AI's parts (N7b, N7c) read of the mob"
)]
pub(crate) struct MobTick<'w> {
    pub(crate) key: EntityId,
    /// Its entity type, by id.
    pub(crate) kind: u16,
    /// The `lived` its tick began with, which its counters are stamped against.
    pub(crate) lived: u32,
    /// The game's `tickCount`: its ticks since it was last loaded fresh.
    pub(crate) tick_count: u32,
    /// `yRot` and `xRot`, which its controls turn and its input is turned by.
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    /// Whether it rides something (`isPassenger`), and what, by type.
    pub(crate) vehicle: Option<u16>,
    /// Whether something rides it (`isVehicle`).
    pub(crate) carries: bool,
    /// Whether it is on a lead (`isLeashed`).
    pub(crate) leashed: bool,
    /// Whether this tick's `baseTick` has counted its counters down: what happens
    /// after comes at the next moment (`body::moment`).
    pub(crate) counted: bool,
    /// Whether a player rides it, and drives it (`docs/groundwork/n7h-riding-plan.md`,
    /// sections 3 and 10).
    pub(crate) ridden: Ridden,
    pub(crate) mob: Mob,
    pub(crate) living: Living,
    pub(crate) mob_type: &'static dyn MobType,
    pub(crate) world: &'w dyn MobWorld,
    /// Its own stream in this tick, on from what it has drawn: every draw of its tick,
    /// the level's on its behalf among them, at their place (the plan's 7.2.8).
    pub(crate) draws: &'w mut Random,
    pub(crate) out: MobOutput,
}

/// How a player rides a mob, as its tick begins.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) enum Ridden {
    /// No player drives it: it thinks and moves itself, a rider on it or not.
    #[default]
    Not,
    /// Its first seat is kept for a player on their way to it across a line: kept still
    /// until they come, as a driven boat is, so that the moves their client sent meanwhile
    /// are judged from where it stood (the plan, section 10 item 2).
    Pending,
    /// Its first passenger is a player of the region its type lets steer it
    /// (`getControllingPassenger`): the player's client moves it, and its tick takes the
    /// player's rotation, `yRot` and `xRot` (`travelRidden`, `tickRidden`).
    Driven { yaw: f32, pitch: f32 },
}

impl MobTick<'_> {
    /// `isEffectiveAi`: its AI runs unless it has none, or a player drives it (the
    /// game's `canSimulateMovement` is the driver's client's then).
    pub(crate) fn effective_ai(&self) -> bool {
        !self.mob.body.flags.no_ai && self.ridden == Ridden::Not
    }
}

/// The tick of a mob, in the game's order.
pub(crate) fn tick(tick: &mut MobTick<'_>) {
    // `Mob.checkDespawn`, which the level runs before the entity's tick.
    body::check_despawn(tick);
    if tick.out.removed.is_some() {
        return;
    }
    // What the type's `tick` does before the entity's.
    tick.mob_type.before_tick(tick);
    // `baseTick`: the entity's (fluids, fire, the void), the living body's (its wall,
    // its air), the hurts these met in order, the counters, the death's tick, and
    // `Mob.baseTick`'s ambient sound; then the type's own (a water animal's air).
    let air = tick.living.air;
    body::base_tick(tick);
    take_hurts(tick);
    body::after_base_tick(tick);
    if tick.out.removed.is_some() {
        return;
    }
    tick.mob_type.after_base_tick(tick, air);
    take_hurts(tick);
    // `aiStep`: the type's own part before `LivingEntity`'s first (a fish's flop, a
    // horse's tail); a dying mob does not think, but falls and slides.
    tick.mob_type.before_ai_step(tick);
    body::start_ai_step(tick);
    if tick.effective_ai() && !body::immobile(tick) {
        ai::server_ai_step(tick);
    }
    match tick.ridden {
        Ridden::Not => {
            body::jump(tick);
            body::travel(tick);
        }
        // Its moves are its driver's, which come with their client's packets: its tick
        // takes their rotation, and its motion is nothing (`travelRidden`, the move
        // simulated elsewhere: `setDeltaMovement(ZERO)`); a living one only.
        Ridden::Driven { yaw, pitch } => {
            if !tick.mob.body.vitals.dying() {
                tick.mob_type.tick_ridden(tick, yaw, pitch);
            }
            tick.living.body.motion = [0.0; 3];
        }
        Ridden::Pending => {}
    }
    take_hurts(tick);
    body::push_entities(tick);
    // The type's `aiStep`, after `LivingEntity`'s.
    tick.mob_type.after_ai_step(tick);
    // After `aiStep`, in `LivingEntity.tick`.
    ai::rotate_body(tick);
    // What the type's `tick` does after the living one.
    tick.mob_type.after_tick(tick);
    take_hurts(tick);
}

/// The hurts the body met of itself so far in the tick, taken in the order met
/// (`hurtServer` of each, as the game calls it where it meets it).
pub(crate) fn take_hurts(tick: &mut MobTick<'_>) {
    let hurts = match &mut tick.living.kind {
        crate::living::Kind::Mob { hurts, .. } => std::mem::take(hurts),
        crate::living::Kind::ArmorStand { .. } => return,
    };
    for (damage, amount) in hurts {
        body::hurt_server(tick, damage, amount);
    }
}
