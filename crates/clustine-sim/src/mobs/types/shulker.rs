//! The shulker (`Shulker` and its inner classes, d javap Shulker.txt; family E8 of the
//! End's milestone, `docs/groundwork/p-plan.md`, sections 19.2, 19.5 and 19.6).
//!
//! **It clings to a face** (`DATA_ATTACH_FACE_ID`, down by default): it never moves of
//! itself (`getDeltaMovement` is always nothing and `setDeltaMovement` does nothing, so
//! no gravity, push or knockback moves it), and every place it is put is snapped to its
//! block's middle (`setPos`, 744–826). After the whole of its living tick (`tick`,
//! 282–325), where it may not stay at its block and face (`canStayAt`: its own block
//! air, or the moving piston it is in; the block behind the face full on that side; the
//! whole opened box free of collisions), it takes the first face it may stay on, down,
//! up, north, south, west, east (`findAttachableSurface`), or teleports.
//!
//! **Its peek** (`DATA_PEEK_ID`, 0 to 100) is what its goals ask for; its lid's opening
//! (`currentPeekAmount`) follows it by 0.05 a tick (`updatePeekAmount`, 405–450), and its
//! box with it: a block at its feet stretched away from its face by `getPhysicalPeek`
//! (`0.5 − sin((0.5 + peek) π) / 2`, 344–376), and on the floor its height (eyes too)
//! scaled by one and the opening (`getDefaultDimensions`, 377–404). Each ask
//! (`setRawPeekAmount`, 1249–1293) takes the `minecraft:covered` armour of 20 off, and puts
//! it back on with the close sound and `container_close` where the peek is 0, or plays the
//! open sound and `container_open` otherwise. Closed, it refuses an arrow's hurt
//! (`hurtServer` 0–22) and hurts with its closed sound; its ambient sound is heard only
//! while open.
//!
//! **Its goals** (`registerGoals`, 63–140): it looks at a player within 8 (only across),
//! attacks (`ShulkerAttackGoal`: on its start a peek of 100 and 20 ticks; every tick its
//! look at the target at 180 a tick, and within 20 blocks a bullet each time the timer
//! runs out, the timer then `20 + nextInt(10) · 20 / 2`, with its shoot sound at 2 and
//! `(a − b) · 0.2 + 1`; further it lets the target go; on its stop a peek of 0), peeks
//! (`ShulkerPeekGoal`: without a target, one ask in 20 where it may stay, a peek of 30 for
//! `adjustedTickDelay(20 · (1 + nextInt(3)))`, then 0), and looks about. Its targets: who
//! hurt it, but a shulker, alerting the shulkers about it; the nearest player it sees
//! (`ShulkerNearestAttackGoal`, whose own box is read only for what is no player); and
//! `ShulkerDefenseAttackGoal`, which asks for a team first and so never runs in Clustine,
//! where no mob has one. Its head turns about its face (`ShulkerLookControl`: the yaw of
//! the wanted point in the plane of its face, the pitch always 0, never held to its body),
//! and its body never turns (`ShulkerBodyRotationControl`).
//!
//! **It teleports** (`teleportSomewhere`, 926–1063): five tries of a block within 8 each
//! way (`randomBetweenInclusive(-8, 8)` thrice), above the floor of the world, of air,
//! free of collisions, with a face it may cling to whose block is not of
//! `#shulker_does_not_teleport_to`; there it takes that face, plays its teleport sound
//! where it was, stands at the block's middle, sends `teleport` and level event 2016 from
//! its old block (the difference packed, `BlockUtil.packDifferenceInPosition`), its peek
//! set to 0 without the armour's change, and its target dropped. A hurt that took leaves
//! it at less than half its health asks for one in four (`nextInt(4)`); a shulker bullet's
//! hurt otherwise, where it was open, asks for one and, where it went
//! (`hitByShulkerBullet`, 1137–1199), a new shulker of its colour where it was, unless
//! `nextFloat() < (n − 1) / 5` of the n shulkers alive in its box grown by 8.
//!
//! Its colour (`DATA_COLOR_ID`, 16 for none) goes to the shulker it makes; 30 health,
//! 5 experience, immune to fire (its type's), never despawned (`AbstractGolem`), an
//! ambient sound every 120 ticks at most; its loot is its table's (a shell by chance).
//!
//! Named differences:
//! - A teleport its hurt asks for, and the new shulker of a bullet's, come at the start of
//!   its next tick, drawing from its stream then, as the enderman's do (`enderman.rs`);
//!   the duplication's `nextFloat` is of its own stream, where the game's is the level's.
//! - The entities its lid's opening pushes (`onPeekAmountChange`, `MoverType.SHULKER`)
//!   are not moved by the server: a player's client moves itself, as the game's does.
//! - The opening (`currentPeekAmount`) is kept with it, where the game starts it at
//!   nothing at every load; a piston's push teleports it nowhere (`move(SHULKER_BOX)` is
//!   the pistons' to call).
//! - `finalizeSpawn`'s `setYRot(0)` is the head's alone: the entity's own yaw is the
//!   making's, which nothing of its tick reads.
//! - Its box is no obstacle to other entities' moves (`canBeCollidedWith`), as no living
//!   thing is in Clustine's physics.
//! - The face's frames are the axes' exact unit vectors, where the game's quaternions
//!   leave a few ten-millionths in them (`Direction.getRotation` of float angles).

use clustine_blocks::data::in_tag;
use clustine_data::{Face, attributes, blocks, entity_types, sounds};
use clustine_world::{BlockPos, Entity, EntityData, TypeState};
use serde::{Deserialize, Serialize};

use super::{Dimensions, MobType, Spawning};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{LookAtPlayerGoal, RandomLookAroundGoal};
use crate::mobs::ai::maths::{atan2, reduced_tick_delay, rotate_towards};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::{self, Class};
use crate::mobs::ai::targets::{self, NearestTarget, Target};
use crate::mobs::ai::{Ai, HeadLimits, Setup, controls};
use crate::mobs::body::attributes::ADD_VALUE;
use crate::mobs::body::hurt::{DamageSource, Struck};
use crate::mobs::body::{Event, Modifier, attribute};
use crate::mobs::tick::{Local, MobTick};
use crate::physics::aabb::Aabb;
use crate::physics::collide::{Context, no_collision};
use crate::physics::{self, Vec3};
use crate::region::Random;
use crate::shown::DataShown;

/// Whether the type is whole, so that `built()` gives it and an egg makes it.
pub(crate) const BUILT: bool = true;

/// The version of [`ShulkerState`]'s encoding.
const VERSION: u8 = 1;

/// The shulker's entity type.
pub(crate) const SHULKER: u16 = entity_types::SHULKER as u16;

/// The synced data's indices, the first three after `Mob`'s.
pub(crate) const ATTACH_FACE_INDEX: u8 = 16;
pub(crate) const PEEK_INDEX: u8 = 17;
pub(crate) const COLOUR_INDEX: u8 = 18;

/// `NO_COLOR`: the plain shulker's colour.
pub(crate) const NO_COLOUR: u8 = 16;

/// `COVERED_ARMOR_MODIFIER`: `minecraft:covered`, 20 added to its armour while closed.
pub(crate) const COVERED: &str = "minecraft:covered";
const COVERED_ARMOUR: f64 = 20.0;

/// `PEEK_PER_TICK`: how far the lid moves a tick towards the peek asked for.
const PEEK_PER_TICK: f32 = 0.05;

/// `MAX_TELEPORT_DISTANCE`: the reach of a teleport each way.
const TELEPORT_REACH: i32 = 8;

/// `OTHER_SHULKER_SCAN_RADIUS` and `OTHER_SHULKER_LIMIT`: the shulkers a bullet's
/// duplication counts about it, and how many make it unlikely.
const SCAN_RADIUS: f64 = 8.0;
const OTHER_LIMIT: f32 = 5.0;

/// The level event of a shulker's teleport, from its old block.
pub(crate) const TELEPORT_EVENT: i32 = 2016;

/// `ShulkerAttackGoal`'s reach: within 20 blocks it shoots, beyond it lets the target go.
const ATTACK_REACH_SQR: f64 = 400.0;

/// `Direction`'s unit steps by its 3D value: down, up, north, south, west, east.
pub(crate) const STEPS: [[i32; 3]; 6] = [
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
    [-1, 0, 0],
    [1, 0, 0],
];

/// The face opposite the face `face` (both by their 3D values).
pub(crate) fn opposite(face: u8) -> u8 {
    face ^ 1
}

/// `Direction.getAxis()` of the face `face`: 0 for x, 1 for y, 2 for z.
pub(crate) fn axis_of(face: u8) -> u8 {
    match face {
        0 | 1 => 1,
        2 | 3 => 2,
        _ => 0,
    }
}

/// The block `pos` one step towards the face `face`.
pub(crate) fn relative(pos: BlockPos, face: u8) -> BlockPos {
    let [x, y, z] = STEPS[usize::from(face % 6)];
    pos.offset(x, y, z)
}

/// The game's face of the 3D value `face`.
fn face_of(face: u8) -> Face {
    match face % 6 {
        0 => Face::Down,
        1 => Face::Up,
        2 => Face::North,
        3 => Face::South,
        4 => Face::West,
        _ => Face::East,
    }
}

/// What a shulker teleports for at its next tick, asked by a hurt (the module's note).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Pending {
    /// `teleportSomewhere()`, of a hurt that left it at less than half its health.
    Teleport,
    /// `hitByShulkerBullet()` of an open shulker: a teleport, and where it went, perhaps
    /// a new shulker where it was.
    Duplicate,
}

/// What a shulker keeps: its synced face, peek and colour, the lid's opening, and a
/// teleport its hurt asked for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ShulkerState {
    /// `DATA_ATTACH_FACE_ID`, by the face's 3D value.
    pub(crate) attach_face: u8,
    /// `DATA_PEEK_ID`: the peek its goals asked for, 0 to 100.
    pub(crate) peek: u8,
    /// `DATA_COLOR_ID`: a dye's id, or 16 for none.
    pub(crate) colour: u8,
    /// `currentPeekAmount`: the lid's opening, 0 to 1.
    pub(crate) opening: f32,
    pub(crate) pending: Option<Pending>,
}

impl Default for ShulkerState {
    /// `defineSynchedData`'s: down, closed, of no colour.
    fn default() -> Self {
        Self {
            attach_face: 0,
            peek: 0,
            colour: NO_COLOUR,
            opening: 0.0,
            pending: None,
        }
    }
}

/// A shulker's state from its type state; nothing, or what this build cannot read, as a
/// fresh shulker's.
pub(crate) fn state_of(mob: &Mob) -> ShulkerState {
    if mob.state.bytes.is_empty() || mob.state.version != VERSION {
        return ShulkerState::default();
    }
    postcard::from_bytes(&mob.state.bytes).unwrap_or_default()
}

/// The state changed by `change`, written back where it changed.
pub(crate) fn change(mob: &mut Mob, change: impl FnOnce(&mut ShulkerState)) {
    let mut own = state_of(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        mob.state = if own == ShulkerState::default() {
            TypeState::default()
        } else {
            TypeState {
                version: VERSION,
                bytes: postcard::to_allocvec(&own).expect("a type state is encoded into memory"),
            }
        };
    }
}

/// `isClosed()`: no peek asked for.
pub(crate) fn is_closed(mob: &Mob) -> bool {
    state_of(mob).peek == 0
}

/// `getPhysicalPeek(peek)` (344–376): how far the lid's opening carries its box, by
/// `Mth.sin`'s table.
pub(crate) fn physical_peek(opening: f32) -> f32 {
    let angle = (0.5_f32 + opening) * std::f32::consts::PI;
    0.5 - clustine_noise::trig::sin(f64::from(angle)) * 0.5
}

/// `getProgressDeltaAabb(1, direction, from, to, at)` (551–658): the block at `at`'s
/// feet stretched towards `direction` by the greater of `from` and `to`, and cut back
/// from that side by one and the lesser.
pub(crate) fn progress_delta_box(direction: u8, from: f32, to: f32, at: Vec3) -> Aabb {
    let base = Aabb {
        min: [-0.5, 0.0, -0.5],
        max: [0.5, 1.0, 0.5],
    };
    let most = f64::from(from.max(to));
    let least = f64::from(from.min(to));
    let step = STEPS[usize::from(direction % 6)].map(f64::from);
    let stretched = base.expand_towards(step.map(|along| along * most));
    // `AABB.contract`: a negative component raises the minimum, a positive one lowers
    // the maximum.
    let mut cut = stretched;
    for (axis, along) in step.into_iter().enumerate() {
        let by = -along * (1.0 + least);
        if by < 0.0 {
            cut.min[axis] -= by;
        } else if by > 0.0 {
            cut.max[axis] -= by;
        }
    }
    Aabb::new(cut.min, cut.max).moved(at)
}

/// `getProgressAabb(1, direction, peek, at)`: the block at its feet stretched towards
/// `direction` by `peek`.
pub(crate) fn progress_box(direction: u8, peek: f32, at: Vec3) -> Aabb {
    progress_delta_box(direction, -1.0, peek, at)
}

/// `makeBoundingBox(at)` of a shulker of the state `own` at `at`: its block stretched
/// away from its face by the physical peek of its opening.
pub(crate) fn box_of(own: &ShulkerState, at: Vec3) -> Aabb {
    progress_box(opposite(own.attach_face), physical_peek(own.opening), at)
}

/// The box of the region's record `entity` where it is a shulker's
/// (`Region::entity_box`); `None` for any other.
pub(crate) fn bounds_of(entity: &Entity) -> Option<Aabb> {
    if entity.kind != SHULKER || !BUILT {
        return None;
    }
    let EntityData::Mob { state, .. } = &entity.data else {
        return None;
    };
    let mob = Mob {
        body: Default::default(),
        mind: Default::default(),
        state: state.clone(),
    };
    Some(box_of(&state_of(&mob), entity.position))
}

/// `setPos`' snap of a place to its block's middle: x and z at the half, y at the
/// nearest whole.
pub(crate) fn snapped(at: Vec3) -> Vec3 {
    [
        at[0].floor() + 0.5,
        (at[1] + 0.5).floor(),
        at[2].floor() + 0.5,
    ]
}

/// `isPositionBlocked(pos)` of a shulker whose block is `own`: anything but air, or the
/// moving piston it is in itself.
fn position_blocked(tick: &MobTick<'_>, pos: BlockPos, own: BlockPos) -> bool {
    let state = tick.world.state(pos);
    if state.props().is_air() {
        return false;
    }
    !(clustine_blocks::data::is_block(state, blocks::MOVING_PISTON) && pos == own)
}

/// `Level.noCollision(this, bounds)`: no block and no entity that collides meets it.
fn no_collision_at(tick: &MobTick<'_>, bounds: &Aabb) -> bool {
    let context = Context {
        stands_on_lava: false,
        bottom: tick.living.body.position[1],
        fall_distance: 0.0,
        falling_block: false,
        noclip: None,
    };
    no_collision(tick.world, context, &tick.living.body.mover, bounds)
}

/// `canStayAt(pos, face)` (855–893): its block not blocked, the block behind the face
/// loaded and full on the side towards it (`loadedAndEntityCanStandOnFace`, the
/// collision shape's), and the whole opened box free of collisions.
pub(crate) fn can_stay_at(tick: &MobTick<'_>, pos: BlockPos, face: u8) -> bool {
    let own = physics::containing(tick.living.body.position);
    if position_blocked(tick, pos, own) {
        return false;
    }
    let behind = relative(pos, face);
    let side = opposite(face);
    if !tick.world.is_loaded(behind) {
        return false;
    }
    let collision: Vec<[f64; 6]> = tick
        .world
        .state(behind)
        .props()
        .collision_shape()
        .boxes()
        .collect();
    if !clustine_data::faces::face_full(&collision, face_of(side)) {
        return false;
    }
    let bottom = [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y),
        f64::from(pos.z) + 0.5,
    ];
    no_collision_at(tick, &progress_box(side, 1.0, bottom).deflate(1e-6))
}

/// `findAttachableSurface(pos)` (827–854): the first face, down to east, it may stay on.
fn attachable_surface(tick: &MobTick<'_>, pos: BlockPos) -> Option<u8> {
    (0..6).find(|&face| can_stay_at(tick, pos, face))
}

/// `teleportSomewhere()` of the shulker in `tick` (926–1063), as the module's comment
/// says: whether it went.
pub(crate) fn teleport_somewhere(tick: &mut MobTick<'_>) -> bool {
    if tick.mob.body.flags.no_ai || tick.mob.body.vitals.dying() {
        return false;
    }
    let from = tick.living.body.position;
    let old = physics::containing(from);
    for _ in 0..5 {
        let mut offset = [0; 3];
        for along in &mut offset {
            // `Mth.randomBetweenInclusive(random, -8, 8)`.
            *along = tick.draws.next_int(2 * TELEPORT_REACH as u32 + 1) as i32 - TELEPORT_REACH;
        }
        let pos = old.offset(offset[0], offset[1], offset[2]);
        if pos.y <= tick.world.min_y() || !tick.world.state(pos).props().is_air() {
            continue;
        }
        if !no_collision_at(tick, &Aabb::of_block(pos).deflate(1e-6)) {
            continue;
        }
        let Some(face) = attachable_surface(tick, pos) else {
            continue;
        };
        if in_tag(
            tick.world.state(relative(pos, face)),
            "minecraft:shulker_does_not_teleport_to",
        ) {
            continue;
        }
        change(&mut tick.mob, |own| own.attach_face = face);
        tick.out.local.push(Local::Sound {
            sound: sounds::ENTITY_SHULKER_TELEPORT,
            source: crate::mobs::body::ladder::sound_source(SHULKER),
            at: from,
            volume: 1.0,
            pitch: 1.0,
        });
        let to = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y),
            f64::from(pos.z) + 0.5,
        ];
        tick.living.body.position = to;
        tick.out.needs_sync = true;
        tick.out.local.push(Local::GameEvent {
            name: "minecraft:teleport",
            pos: old,
        });
        tick.out.local.push(Local::LevelEvent {
            kind: TELEPORT_EVENT,
            pos: old,
            data: pack_difference(old, pos),
        });
        // `entityData.set(DATA_PEEK_ID, 0)`: not `setRawPeekAmount`, so its armour stays
        // as it was.
        change(&mut tick.mob, |own| own.peek = 0);
        tick.mob.body.combat.target = None;
        return true;
    }
    false
}

/// `BlockUtil.packDifferenceInPosition(from, to, 8, 8, 8)`: each difference less than 8
/// within a byte, x highest.
pub(crate) fn pack_difference(from: BlockPos, to: BlockPos) -> i32 {
    let byte = |difference: i32| (difference + TELEPORT_REACH) & 255;
    byte(to.x - from.x) << 16 | byte(to.y - from.y) << 8 | byte(to.z - from.z)
}

/// `setRawPeekAmount(peek)` (1249–1293): the covered armour off, and on again with the
/// close sound and `container_close` where the peek is 0, else the open sound and
/// `container_open`; then the peek.
pub(crate) fn set_raw_peek(tick: &mut MobTick<'_>, peek: u8) {
    if let Some(armour) = tick
        .mob
        .body
        .attributes
        .instance(SHULKER, attribute(attributes::ARMOR))
    {
        armour.remove_modifier(COVERED);
        if peek == 0 {
            armour.add_modifier(Modifier {
                id: COVERED.to_owned(),
                amount: COVERED_ARMOUR,
                operation: ADD_VALUE,
            });
        }
    }
    let (sound, event) = if peek == 0 {
        (sounds::ENTITY_SHULKER_CLOSE, "minecraft:container_close")
    } else {
        (sounds::ENTITY_SHULKER_OPEN, "minecraft:container_open")
    };
    tick.out.events.push(Event::Sound {
        sound,
        volume: 1.0,
        pitch: 1.0,
    });
    tick.out.local.push(Local::GameEventAt {
        name: event,
        at: tick.living.body.position,
        affected: None,
    });
    change(&mut tick.mob, |own| own.peek = peek);
}

/// `updatePeekAmount()` (405–450): the opening a step of 0.05 towards the peek asked
/// for, kept between it and the far end; whether it moved.
pub(crate) fn update_peek_amount(own: &mut ShulkerState) -> bool {
    let wanted = f32::from(own.peek) * 0.01;
    if own.opening == wanted {
        return false;
    }
    own.opening = if own.opening > wanted {
        (own.opening - PEEK_PER_TICK).clamp(wanted, 1.0)
    } else {
        (own.opening + PEEK_PER_TICK).clamp(0.0, wanted)
    };
    true
}

/// `ShulkerLookControl.getYRotD()` (Shulker$ShulkerLookControl.txt 0–191): the yaw of
/// the wanted point `wanted` from the eyes `eyes` in the frame of the face it looks out
/// of (`forward` the south turned by the face's rotation, `right` the face's normal
/// across it), none where the point is straight out of the face.
pub(crate) fn look_yaw(attach_face: u8, eyes: Vec3, wanted: Vec3) -> Option<f32> {
    let out = opposite(attach_face);
    // `getRotation().transform(SOUTH)`: down's is north, up's south, every side's down.
    let forward: [f32; 3] = match out {
        0 => [0.0, 0.0, -1.0],
        1 => [0.0, 0.0, 1.0],
        _ => [0.0, -1.0, 0.0],
    };
    let normal = STEPS[usize::from(out)].map(|value| value as f32);
    // `Vector3f.cross`: the normal across the forward.
    let right = [
        normal[1] * forward[2] - normal[2] * forward[1],
        normal[2] * forward[0] - normal[0] * forward[2],
        normal[0] * forward[1] - normal[1] * forward[0],
    ];
    let delta = [
        (wanted[0] - eyes[0]) as f32,
        (wanted[1] - eyes[1]) as f32,
        (wanted[2] - eyes[2]) as f32,
    ];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let across = dot(right, delta);
    let along = dot(forward, delta);
    (across.abs() > 1e-5 || along.abs() > 1e-5)
        .then(|| (atan2(f64::from(-across), f64::from(along)) * 57.295_776_367_187_5) as f32)
}

/// `ShulkerLookControl.tick()`, `LookControl.tick` with its own yaw, a pitch of 0 and
/// no hold to the body.
fn tick_look(ai: &mut Ai<'_, '_>) {
    ai.tick.pitch = 0.0;
    let look = ai.tick.mob.body.controls.look.clone();
    if look.cooldown > 0 {
        ai.tick.mob.body.controls.look.cooldown -= 1;
        let face = state_of(&ai.tick.mob).attach_face;
        let [x, _, z] = ai.position();
        if let Some(yaw) = look_yaw(face, [x, ai.eye_y(), z], look.wanted) {
            let head = &mut ai.tick.mob.body.head;
            head.head_yaw = rotate_towards(head.head_yaw, yaw, look.y_max_rot_speed);
        }
        ai.tick.pitch = rotate_towards(ai.tick.pitch, 0.0, look.x_max_rot_angle);
    } else {
        let head = &mut ai.tick.mob.body.head;
        head.head_yaw = rotate_towards(head.head_yaw, head.body_yaw, 10.0);
    }
}

/// `ShulkerAttackGoal`'s field: `attackTime`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Volley {
    pub(crate) attack_time: i32,
}

/// `Shulker$ShulkerAttackGoal` (Shulker$ShulkerAttackGoal.txt): of the move and look
/// flags, every tick.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ShulkerAttackGoal;

impl Goal for ShulkerAttackGoal {
    type State = Volley;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    fn every_tick(&self) -> bool {
        true
    }

    /// `canUse()`: a target, alive; the difficulty is never peaceful in Clustine.
    fn can_use(&self, _: &mut Volley, ai: &mut Ai<'_, '_>) -> bool {
        sensing::target(ai).is_some_and(|target| target.alive)
    }

    fn start(&self, state: &mut Volley, ai: &mut Ai<'_, '_>) {
        state.attack_time = 20;
        set_raw_peek(ai.tick, 100);
    }

    fn stop(&self, _: &mut Volley, ai: &mut Ai<'_, '_>) {
        set_raw_peek(ai.tick, 0);
    }

    fn tick(&self, state: &mut Volley, ai: &mut Ai<'_, '_>) {
        state.attack_time -= 1;
        let Some(target) = sensing::target(ai) else {
            return;
        };
        controls::set_look_at(
            ai,
            [target.position[0], target.eye_y, target.position[2]],
            180.0,
            180.0,
        );
        if target.distance_sqr(ai.position()) < ATTACK_REACH_SQR {
            if state.attack_time <= 0 {
                state.attack_time = 20 + ai.tick.draws.next_int(10) as i32 * 20 / 2;
                let own = state_of(&ai.tick.mob);
                ai.tick.out.local.push(Local::ShootShulkerBullet {
                    target: target.key,
                    axis: axis_of(own.attach_face),
                    at: box_of(&own, ai.position()).center(),
                });
                let a = ai.tick.draws.next_float();
                let b = ai.tick.draws.next_float();
                ai.tick.out.events.push(Event::Sound {
                    sound: sounds::ENTITY_SHULKER_SHOOT,
                    volume: 2.0,
                    pitch: (a - b) * 0.2 + 1.0,
                });
            }
        } else {
            sensing::set_target(ai, None);
        }
    }
}

/// `ShulkerPeekGoal`'s field: `peekTime`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Peeking {
    pub(crate) peek_time: i32,
}

/// `Shulker$ShulkerPeekGoal` (Shulker$ShulkerPeekGoal.txt): of no flags.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ShulkerPeekGoal;

impl Goal for ShulkerPeekGoal {
    type State = Peeking;

    fn flags(&self) -> u8 {
        0
    }

    /// `canUse()`: no target, one ask in `reducedTickDelay(40)`, and where it may stay.
    fn can_use(&self, _: &mut Peeking, ai: &mut Ai<'_, '_>) -> bool {
        if sensing::target(ai).is_some() {
            return false;
        }
        if ai.tick.draws.next_int(reduced_tick_delay(40) as u32) != 0 {
            return false;
        }
        let face = state_of(&ai.tick.mob).attach_face;
        let pos = ai.block_position();
        can_stay_at(ai.tick, pos, face)
    }

    fn can_continue(&self, state: &mut Peeking, ai: &mut Ai<'_, '_>) -> bool {
        sensing::target(ai).is_none() && state.peek_time > 0
    }

    fn start(&self, state: &mut Peeking, ai: &mut Ai<'_, '_>) {
        let draw = ai.tick.draws.next_int(3) as i32;
        state.peek_time = self.adjusted_tick_delay(20 * (1 + draw));
        set_raw_peek(ai.tick, 30);
    }

    fn stop(&self, _: &mut Peeking, ai: &mut Ai<'_, '_>) {
        if sensing::target(ai).is_none() {
            set_raw_peek(ai.tick, 0);
        }
    }

    fn tick(&self, state: &mut Peeking, _: &mut Ai<'_, '_>) {
        state.peek_time -= 1;
    }
}

/// `Shulker.registerGoals`' targets (63–140): who hurt it, but a shulker, alerting the
/// shulkers about it; the nearest player it sees (`ShulkerNearestAttackGoal`, which
/// searches for a player as its parent does). `ShulkerDefenseAttackGoal` asks for a team
/// before anything, draws nothing and never runs: it is left out.
const TARGETS: &[(i32, Target)] = &[
    (
        1,
        Target::HurtBy {
            ignore: &[SHULKER],
            alert: true,
        },
    ),
    (2, Target::Nearest(NearestTarget::players(true))),
];

/// `hitByShulkerBullet()`'s count (1137–1199): the shulkers alive whose boxes meet
/// `bounds`, itself among them.
fn shulkers_about(tick: &MobTick<'_>, bounds: &Aabb) -> usize {
    let others = tick
        .world
        .entities_in(bounds)
        .into_iter()
        .filter(|seen| {
            seen.entity.key != tick.key
                && seen.entity.kind == SHULKER
                && Mob::decode(&seen.entity.data).is_ok_and(|mob| !mob.body.vitals.dying())
        })
        .count();
    others + 1
}

/// The teleport its hurt asked for, and a bullet's duplication, done now (the module's
/// note).
fn run_pending(tick: &mut MobTick<'_>) {
    let Some(pending) = state_of(&tick.mob).pending else {
        return;
    };
    change(&mut tick.mob, |own| own.pending = None);
    match pending {
        Pending::Teleport => {
            teleport_somewhere(tick);
        }
        Pending::Duplicate => {
            let at = tick.living.body.position;
            let own = state_of(&tick.mob);
            let bounds = box_of(&own, at);
            if !teleport_somewhere(tick) {
                return;
            }
            let count = shulkers_about(tick, &bounds.inflate([SCAN_RADIUS; 3]));
            let chance = (count as f32 - 1.0) / OTHER_LIMIT;
            if tick.draws.next_float() < chance {
                return;
            }
            // `EntityType.SHULKER.create(level, BREEDING)`, its colour, there.
            let mut made = Mob::fresh();
            change(&mut made, |new| new.colour = own.colour);
            tick.out.local.push(Local::MakeMob {
                kind: SHULKER,
                mob: Box::new(made),
                at,
            });
        }
    }
}

/// The shulker.
pub(crate) struct Shulker;

impl MobType for Shulker {
    /// `Shulker.registerGoals` (63–140).
    fn register_goals(&self, setup: &mut Setup) {
        setup.add_goal(
            1,
            LookAtPlayerGoal {
                only_horizontal: true,
                ..LookAtPlayerGoal::new(Class::Player, 8.0)
            },
        );
        setup.add_goal(4, ShulkerAttackGoal);
        setup.add_goal(7, ShulkerPeekGoal);
        setup.add_goal(8, RandomLookAroundGoal);
        targets::register(setup, TARGETS);
    }

    fn tick_look_control(&self, ai: &mut Ai<'_, '_>) {
        tick_look(ai);
    }

    /// `getMaxHeadXRot` and `getMaxHeadYRot`: 180.
    fn head(&self) -> HeadLimits {
        HeadLimits {
            max_x_rot: 180,
            max_y_rot: 180,
            ..HeadLimits::default()
        }
    }

    /// `ShulkerBodyRotationControl.clientTick()` does nothing.
    fn body_rotation_ticks(&self, _: &MobTick<'_>) -> bool {
        false
    }

    /// Its place snapped (`setPos`), its motion nothing, and the teleport its hurt asked
    /// for.
    fn before_tick(&self, tick: &mut MobTick<'_>) {
        hold_still(tick);
        run_pending(tick);
    }

    fn before_ai_step(&self, tick: &mut MobTick<'_>) {
        tick.living.body.motion = [0.0; 3];
    }

    /// `getDeltaMovement()` is always nothing: its travel moves it nowhere.
    fn before_travel(&self, tick: &mut MobTick<'_>) {
        tick.living.body.motion = [0.0; 3];
    }

    fn travel(&self, _: &MobTick<'_>) -> crate::living::Travel {
        crate::living::Travel::Bare
    }

    /// `Shulker.tick` after the living one (282–325): a new face or a teleport where it
    /// may not stay, then the lid's step.
    fn after_tick(&self, tick: &mut MobTick<'_>) {
        tick.living.body.motion = [0.0; 3];
        if tick.vehicle.is_none() {
            let face = state_of(&tick.mob).attach_face;
            let pos = physics::containing(tick.living.body.position);
            if !can_stay_at(tick, pos, face) {
                match attachable_surface(tick, pos) {
                    Some(face) => change(&mut tick.mob, |own| own.attach_face = face),
                    None => {
                        teleport_somewhere(tick);
                    }
                }
            }
        }
        change(&mut tick.mob, |own| {
            update_peek_amount(own);
        });
    }

    /// `finalizeSpawn`'s `setYRot(0)` and `yHeadRot` with it (701–719).
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        spawn.mob.body.head.head_yaw = 0.0;
    }

    /// `getDefaultDimensions(pose)` (377–404): on the floor and open, its height and eyes
    /// scaled by one and the opening.
    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        let own = state_of(mob);
        if own.attach_face != 0 || own.opening <= 0.0 {
            return None;
        }
        let info = clustine_data::EntityTypeInfo::of(entity_types::SHULKER);
        let scale = 1.0 + own.opening;
        Some(Dimensions {
            width: info.width,
            height: info.height * scale,
            eye_height: info.eye_height * scale,
        })
    }

    /// `hurtServer` (1069–1111): closed, it refuses what an arrow deals.
    fn refuses_hurt(&self, mob: &Mob, source: &DamageSource) -> bool {
        is_closed(mob)
            && source.direct.as_ref().is_some_and(|direct| {
                matches!(
                    i32::from(direct.kind),
                    entity_types::ARROW | entity_types::SPECTRAL_ARROW | entity_types::TRIDENT
                )
            })
    }

    /// `hurtServer` after its parent's, where the hurt took: under half its health one
    /// draw in four teleports it; else a bullet's hurt of an open shulker teleports it and
    /// may make another (both at its next tick, the module's note).
    fn after_hurt(&self, struck: &mut Struck<'_>, source: &DamageSource, took: bool) {
        // `setDeltaMovement` does nothing: a hurt's knockback leaves it still.
        *struck.motion = [0.0; 3];
        if !took {
            return;
        }
        let max = struck.attribute(attributes::MAX_HEALTH);
        let pending = if f64::from(struck.mob.body.vitals.health) < max * 0.5
            && struck.draws.next_int(4) == 0
        {
            Some(Pending::Teleport)
        } else if source.kind.is("minecraft:is_projectile")
            && source
                .direct
                .as_ref()
                .is_some_and(|direct| i32::from(direct.kind) == entity_types::SHULKER_BULLET)
            && !is_closed(struck.mob)
        {
            Some(Pending::Duplicate)
        } else {
            None
        };
        if pending.is_some() {
            change(struck.mob, |own| own.pending = pending);
        }
    }

    /// `playAmbientSound()`: only while open.
    fn play_ambient_sound(&self, tick: &mut MobTick<'_>) {
        if !is_closed(&tick.mob) {
            crate::mobs::body::play_ambient_sound(tick);
        }
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SHULKER_AMBIENT)
    }

    /// `AbstractGolem.getAmbientSoundInterval()`: 120.
    fn ambient_interval(&self) -> u32 {
        120
    }

    fn hurt_sound(&self, mob: &Mob, _: &DamageSource) -> Option<i32> {
        Some(if is_closed(mob) {
            sounds::ENTITY_SHULKER_HURT_CLOSED
        } else {
            sounds::ENTITY_SHULKER_HURT
        })
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SHULKER_DEATH)
    }

    /// `AbstractGolem.removeWhenFarAway`: never.
    fn remove_when_far_away(&self, _: u16, _: &Mob, _: u32, _: f64) -> bool {
        false
    }

    /// `xpReward` 5 (the constructor's), with what its equipment adds.
    fn experience_reward(&self, _: u16, mob: &Mob, draws: &mut Random) -> i32 {
        super::illagers::experience(5, mob, draws)
    }

    /// `readAdditionalSaveData`: `AttachFace`, `Peek` and `Color`, each set as its
    /// synced value is (no armour's change).
    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        let face = super::template::int(tag, "AttachFace");
        let peek = super::template::int(tag, "Peek");
        let colour = super::template::int(tag, "Color");
        change(mob, |own| {
            if let Some(face) = face.and_then(|face| u8::try_from(face).ok()) {
                // `Direction.LEGACY_ID_CODEC`: an id out of range reads as down.
                own.attach_face = if face < 6 { face } else { 0 };
            }
            if let Some(peek) = peek {
                own.peek = peek as u8;
            }
            if let Some(colour) = colour {
                own.colour = colour as u8;
            }
        });
    }

    /// `DATA_ATTACH_FACE_ID`, `DATA_PEEK_ID` and `DATA_COLOR_ID`.
    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let own = state_of(mob);
        vec![
            (
                ATTACH_FACE_INDEX,
                DataShown::Direction(own.attach_face),
                own.attach_face == 0,
            ),
            (PEEK_INDEX, DataShown::Byte(own.peek), own.peek == 0),
            (
                COLOUR_INDEX,
                DataShown::Byte(own.colour),
                own.colour == NO_COLOUR,
            ),
        ]
    }
}

/// `setPos`' snap and `setDeltaMovement`'s nothing, as its tick begins.
fn hold_still(tick: &mut MobTick<'_>) {
    tick.living.body.motion = [0.0; 3];
    if tick.vehicle.is_some() {
        return;
    }
    let at = tick.living.body.position;
    let snapped = snapped(at);
    if snapped.map(f64::to_bits) != at.map(f64::to_bits) {
        tick.living.body.position = snapped;
        tick.out.needs_sync = true;
    }
}

#[cfg(test)]
#[path = "shulker_tests.rs"]
mod tests;
