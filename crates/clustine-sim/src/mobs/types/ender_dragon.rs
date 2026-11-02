//! The ender dragon (`EnderDragon`, d EnderDragon.txt; family E9 of the End's milestone,
//! `docs/groundwork/p-plan.md`, sections 19.2, 19.5 items 10 and 14, and 19.6; the brief
//! `docs/groundwork/p-brief.md`, section 6.4).
//!
//! **Its tick** is its own `aiStep` (341–1303), which replaces the living body's whole:
//! no goals, no gravity and no collisions (`noPhysics`). Dying, it draws its particles'
//! three floats and does nothing else. Otherwise the nearest crystal heals it (below);
//! its yaw is wrapped; without AI it stops there. Its flight's history takes its height
//! and yaw (`DragonFlightHistory`); its phase ticks (`phases.rs`), and the phase it went
//! to ticks too; then it steers at its phase's target: up or down by the slope at most
//! the phase's speed, its yaw turned by `yRotA` towards the target (at most 50° a tick,
//! by the phase's turn speed), pushed forward by `0.06` scaled by how well it faces
//! the target, moved by its motion (four fifths of it in a wall), and slowed by `0.8` to
//! `0.95` across as its motion and its facing agree, `0.91` up and down. Then its parts
//! are placed about it (`tickPart`, `parts.rs`): the body, the wings, the head and neck
//! by its yaw and the history's slope, the tail by the history's yaws. Its wings push
//! the living things in their boxes away from the body, and hurt them by 5 where it is
//! not sitting and they were not hurt by a mob in their last two ticks; its head and
//! neck hurt them by 10 (`mobAttack`): none of it reaches a creative or spectator
//! player (`EntitySelector.NO_CREATIVE_OR_SPECTATOR`), and only after it was not hurt
//! itself lately (`wasHurtRecently`). Last its head, neck and body break what they are
//! in (`checkWalls`, 1578–1716): every block but air, `#dragon_transparent` and
//! `#dragon_immune` is removed (`removeBlock`), with level event 2008 at a block of the
//! box drawn where any was; an immune block holds it in the wall (`inWall`), which
//! slows its next move to four fifths.
//!
//! **Healing** (`checkCrystals`, 1349–1424): while its nearest crystal stands, one more
//! health every tenth tick; one tick in ten (`nextInt(10)`) the nearest end crystal in
//! its box grown by 32 is chosen again. The beam from that crystal to it is drawn by
//! each client from its own dragon, which runs the same search: nothing of it is sent.
//!
//! **Its hurt** (`hurt(part, …)`, 1717–1803): nothing while dying; the phase's own first
//! (a sitting phase takes nothing of an arrow or a wind charge); a part other than the
//! head or the neck takes `a / 4 + min(a, 1)`; below `0.01` nothing; then only a
//! player's hurt or one of `#always_hurts_ender_dragons` is taken (`reallyHurt`). While
//! sitting, more than a quarter of its health taken since it sat makes it take off. A
//! hit on no part (an arrow, a cloud, a blast) is the body's (`hurtServer`). The blow
//! that kills it outside a sitting phase leaves it at 1 and dying (`handleKillingBlow`);
//! a sitting dragon's knockback is none.
//!
//! **Its death** (`tickDeath`, 1880–2088): 200 ticks counted in `DragonDeathTime`; the
//! fight told of its health each of them; its explosions' draws from 180; after 150
//! every fifth tick a twelfth and a half of its experience (12000 the first time, 500
//! after) as orbs; at its first the global level event 1028; it rises by 0.1 a tick,
//! its parts with it; at 200 the last fifth of its experience, the fight told it is
//! killed, and it is gone.
//!
//! **Its fight** is read each tick (`end_fight.rs`, `fight_view`): none outside the
//! End's home or for a dragon that is not the fight's own, which is then the game's
//! dragon without a fight (the inner rings, no experience but 500, nothing told).
//!
//! Named differences:
//! - What `LivingEntity.aiStep` the framework runs is undone for it: its small motion
//!   is kept (`after_ai_step`), and its moves are its own.
//! - Its flight history keeps the 17 newest samples, all the server reads of 64.
//! - Its flaps and growls are the client's: nothing of them is drawn on the server.
//! - The arrow or wind charge a sitting phase takes nothing of is not set alight.
//! - Its wings push only mobs of the region's own; an armour stand and a mob in a copy
//!   are not pushed (a mob in a copy is hurt by its holder, as any `Local::Hurt`).
//! - An effect given to it is taken, where the game's `addEffect` refuses every one.
//! - The sitting flame's cloud is found to be discarded by its owner and its making, not
//!   by a reference to it.
//! - Its phases' fields are kept across a load, where the game makes the phases anew.
//! - A projectile meets its whole box, as any mob's, and hurts its body, where the
//!   game's meets its parts (`isPickable` is false of it and true of each part).

pub(crate) mod nodes;
pub(crate) mod parts;
pub(crate) mod phases;

use clustine_blocks::data::in_tag;
use clustine_data::{EntityTypeInfo, entity_types, sounds};
use clustine_world::{BlockPos, Entity, EntityData, EntityId, TypeState};
use serde::{Deserialize, Serialize};

use super::MobType;
use crate::mobs::Mob;
use crate::mobs::ai::maths::{RADIANS, atan2, clamp, table_cos, table_sin, wrap_degrees};
use crate::mobs::body::hurt::{DamageSource, Struck};
use crate::mobs::body::{Event, Removed, hurt::GameEvent};
use crate::mobs::tick::{Local, MobTick};
use crate::mobs::view::{MobWorld, SeenPlayer};
use crate::physics::aabb::Aabb;
use crate::physics::{self, Vec3};
use crate::region::Random;
use crate::region::end_fight::FightView;
use crate::shown::DataShown;
use nodes::{INNER, Node, Path};
use parts::{BODY, HEAD, NECK, TAIL, WING};
use phases::{DYING, HOVERING, TAKEOFF, is_sitting};

/// Whether the type is whole, so that `built()` gives it.
pub(crate) const BUILT: bool = true;

/// The dragon's entity type.
pub(crate) const DRAGON: u16 = entity_types::ENDER_DRAGON as u16;

/// Its fireball's entity type (`DragonFireball`).
pub(crate) const DRAGON_FIREBALL: u16 = entity_types::DRAGON_FIREBALL as u16;

/// `DATA_PHASE`, the first index after `Mob`'s.
pub(crate) const PHASE_INDEX: u8 = 16;

/// The level event of its fireball shot (`levelEvent(1017)`).
pub(crate) const FIREBALL_SHOT: i32 = 1017;

/// The global level event of its death (`globalLevelEvent(1028)`).
pub(crate) const DEATH_EVENT: i32 = 1028;

/// The level event of a wall it broke through (`levelEvent(2008)`).
pub(crate) const WALL_EVENT: i32 = 2008;

/// Its health (`createAttributes`).
pub(crate) const MAX_HEALTH: f32 = 200.0;

/// `SITTING_ALLOWED_DAMAGE_PERCENTAGE`.
const SITTING_ALLOWED: f32 = 0.25;

/// The death's ticks and its experience (`tickDeath`).
const DEATH_TICKS: i32 = 200;
const FIRST_EXPERIENCE: i32 = 12_000;
const LATER_EXPERIENCE: i32 = 500;

/// `mob_drops`, on everywhere in Clustine: its death's experience is awarded.
const MOB_DROPS: bool = true;

/// `mob_griefing`, on everywhere in Clustine: it breaks the blocks it flies through.
const MOB_GRIEFING: bool = true;

/// The newest samples of its flight's history the server reads (`get(0)` to `get(16)`).
const HISTORY_READ: usize = 17;

/// The version of [`DragonState`]'s encoding.
const VERSION: u8 = 1;

/// What a dragon's tick or hurt asks of the region beyond the shared [`Local`]s
/// (`region/ender_dragon.rs`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DragonLocal {
    /// `EnderDragonFight.updateDragon(this)`: its health as the fight's bar shows it.
    FightUpdated { health: f32, max: f32 },
    /// `EnderDragonFight.setDragonKilled(this)`, at the end of its death.
    FightKilled,
    /// `ServerLevel.globalLevelEvent(event, pos, 0)`.
    GlobalEvent { event: i32, pos: BlockPos },
    /// `ExperienceOrb.award(level, at, amount)`.
    Award { at: Vec3, amount: i32 },
    /// The sitting flame's cloud of dragon's breath at `at` (`DragonSittingFlamingPhase`,
    /// 259–354), owned by the dragon.
    BreathCloud { at: Vec3 },
    /// `end()` of the sitting flame: its cloud discarded.
    DiscardFlame,
}

/// The samples of its flight's history the server reads, the newest first: each its
/// height and yaw (`DragonFlightHistory$Sample`). None before its first: the game's
/// history is then 64 samples of nothing, and its first record fills it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct History {
    pub(crate) samples: Vec<(f64, f32)>,
}

impl History {
    /// `record(y, yRot)`.
    pub(crate) fn record(&mut self, y: f64, yaw: f32) {
        if self.samples.is_empty() {
            self.samples = vec![(y, yaw); HISTORY_READ];
            return;
        }
        self.samples.insert(0, (y, yaw));
        self.samples.truncate(HISTORY_READ);
    }

    /// `get(back)`: the sample `back` records ago.
    pub(crate) fn get(&self, back: usize) -> (f64, f32) {
        self.samples.get(back).copied().unwrap_or((0.0, 0.0))
    }
}

/// A hurt's verdict, made before the living body's `hurtServer` runs and read back by
/// the hooks after it (`hurt(part, …)`'s own part).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) enum Verdict {
    Refused,
    Taken {
        amount: f32,
        health: f32,
        motion: Vec3,
        sitting: bool,
    },
}

/// What a dragon keeps beside its body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct DragonState {
    /// `DATA_PHASE`: the phase it is in, by id (`DragonPhase`).
    pub(crate) phase: u8,
    pub(crate) phases: phases::Phases,
    /// `dragonDeathTime` (`DragonDeathTime`).
    pub(crate) death_time: i32,
    /// `sittingDamageReceived`.
    pub(crate) sitting_damage: f32,
    /// `yRotA`.
    pub(crate) y_rot_a: f32,
    /// `inWall`.
    pub(crate) in_wall: bool,
    /// `nearestCrystal`, by key.
    pub(crate) nearest_crystal: Option<EntityId>,
    pub(crate) history: History,
    /// Its graph's nodes, worked out at its first search.
    pub(crate) nodes: Option<Vec<Node>>,
    /// Where its parts stand, in [`parts`]' order.
    pub(crate) parts: [Vec3; parts::COUNT as usize],
    /// The part a hit names, for the hurt that follows (`hurt(part, …)`), and the hurt's
    /// verdict: both only within one hurt.
    pub(crate) hit_part: Option<u8>,
    pub(crate) verdict: Option<Verdict>,
    /// Its motion as its own step left it, within one tick (the module's note).
    pub(crate) kept_motion: Option<Vec3>,
}

impl Default for DragonState {
    /// A dragon as it is made: hovering (`EnderDragonPhaseManager`'s constructor), its
    /// parts at the world's origin as an entity begins.
    fn default() -> Self {
        Self {
            phase: HOVERING,
            phases: phases::Phases::default(),
            death_time: 0,
            sitting_damage: 0.0,
            y_rot_a: 0.0,
            in_wall: false,
            nearest_crystal: None,
            history: History::default(),
            nodes: None,
            parts: [[0.0; 3]; parts::COUNT as usize],
            hit_part: None,
            verdict: None,
            kept_motion: None,
        }
    }
}

/// A dragon's state from its type state; nothing, or what this build cannot read, as a
/// new dragon's.
pub(crate) fn state_of(mob: &Mob) -> DragonState {
    if mob.state.bytes.is_empty() || mob.state.version != VERSION {
        return DragonState::default();
    }
    postcard::from_bytes(&mob.state.bytes).unwrap_or_default()
}

/// The state changed by `change`, written back where it changed.
pub(crate) fn change(mob: &mut Mob, change: impl FnOnce(&mut DragonState)) {
    let mut own = state_of(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        put(mob, &own);
    }
}

/// `own` written into the mob's type state.
fn put(mob: &mut Mob, own: &DragonState) {
    mob.state = if *own == DragonState::default() {
        TypeState::default()
    } else {
        TypeState {
            version: VERSION,
            bytes: postcard::to_allocvec(own).expect("a type state is encoded into memory"),
        }
    };
}

/// The dragon `mob` set to the phase `phase` as `setPhase` sets it (what a fight's
/// making of its dragon does: `HOLDING_PATTERN`), with what its old phase's end asks of
/// the region.
pub(crate) fn set_phase(mob: &mut Mob, phase: u8) -> Vec<Local> {
    let mut locals = Vec::new();
    change(mob, |own| {
        phases::set_phase(&mut own.phase, &mut own.phases, phase, &mut locals);
    });
    locals
}

/// The boxes of the parts of the region's record `entity` where it is a dragon's, in
/// [`parts`]' order; none for any other.
pub(crate) fn part_boxes(entity: &Entity) -> Option<[Aabb; parts::COUNT as usize]> {
    if entity.kind != DRAGON || !BUILT {
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
    let own = state_of(&mob);
    Some(std::array::from_fn(|part| {
        parts::box_of(part as u8, own.parts[part])
    }))
}

/// The part a hit that follows names (the region's hit on a part's id): read by its hurt.
pub(crate) fn hit_part(mob: &mut Mob, part: u8) {
    change(mob, |own| own.hit_part = Some(part.min(parts::COUNT - 1)));
}

/// `LivingEntity.hasLineOfSight`'s clip from `eyes` to `to`: within 128 blocks, no
/// block's collision shape (as an entity standing at `bottom` meets it) in the way.
pub(crate) fn line_of_sight(world: &dyn MobWorld, bottom: f64, eyes: Vec3, to: Vec3) -> bool {
    use crate::physics::collide::{Context, collision_shape};
    if physics::distance_sqr(eyes, to).sqrt() > 128.0 {
        return false;
    }
    let context = Context {
        stands_on_lava: false,
        bottom,
        fall_distance: 0.0,
        falling_block: false,
        noclip: None,
    };
    !physics::walk::traverse(eyes, to, |pos| {
        collision_shape(world.state(pos), pos, context).is_some_and(|(shape, _)| {
            shape
                .boxes()
                .iter()
                .any(|bounds| physics::body::clips_shape(bounds, eyes, to))
        })
    })
}

/// A dragon's step: its tick, its state and what it reads of its fight.
pub(crate) struct Step<'a, 'w> {
    pub(crate) tick: &'a mut MobTick<'w>,
    pub(crate) own: &'a mut DragonState,
    pub(crate) fight: Option<FightView>,
}

impl Step<'_, '_> {
    pub(crate) fn position(&self) -> Vec3 {
        self.tick.living.body.position
    }

    /// `getDragonFight().aliveCrystals()`, none without a fight.
    pub(crate) fn crystals(&self) -> i32 {
        self.fight.map_or(0, |fight| {
            i32::try_from(fight.crystals_alive).unwrap_or(i32::MAX)
        })
    }

    /// The first node a search looks at: the inner rings' where there is no fight or no
    /// crystal stands (`findClosestNode`, `findPath`).
    fn first_node(&self) -> usize {
        if self.fight.is_none() || self.crystals() == 0 {
            INNER
        } else {
            0
        }
    }

    /// Its graph, worked out now where it has none.
    fn nodes(&mut self) -> Vec<Node> {
        if self.own.nodes.is_none() {
            let world = self.tick.world;
            let floor = world.min_y();
            self.own.nodes = Some(nodes::make_nodes(|x, z| {
                world
                    .motion_blocking_no_leaves_height(x, z)
                    .unwrap_or(floor)
            }));
        }
        self.own.nodes.clone().unwrap_or_default()
    }

    /// `findClosestNode()`.
    pub(crate) fn closest_node(&mut self) -> usize {
        let at = self.position();
        self.closest_node_at(at)
    }

    /// `findClosestNode(x, y, z)`.
    pub(crate) fn closest_node_at(&mut self, at: Vec3) -> usize {
        let nodes = self.nodes();
        nodes::closest(&nodes, self.first_node(), at)
    }

    /// `findPath(from, to, last)`.
    pub(crate) fn find_path(&mut self, from: usize, to: usize, last: Option<Node>) -> Option<Path> {
        let nodes = self.nodes();
        nodes::find_path(&nodes, self.first_node(), from, to, last)
    }

    /// `getHeightmapPos(type, EndPodiumFeature.getLocation(fightOrigin))`: the column of
    /// the fight's origin, (0, 0), at its `MOTION_BLOCKING` height, or with `motion`
    /// false its `MOTION_BLOCKING_NO_LEAVES`; the floor of the world where it is not read.
    pub(crate) fn podium(&self, motion: bool) -> BlockPos {
        let world = self.tick.world;
        let height = if motion {
            world.motion_blocking_height(0, 0)
        } else {
            world.motion_blocking_no_leaves_height(0, 0)
        };
        BlockPos::new(0, height.unwrap_or_else(|| world.min_y()), 0)
    }

    /// `ServerLevel.getNearestPlayer(TargetingConditions.forCombat() …, dragon, at)`: of
    /// the players it reads that are no creative or spectator and alive, within `range`
    /// of it and seen from its eyes where a range is given (the scan's and the charge's,
    /// which check the line of sight), and passing `selector`, the one nearest `at`.
    pub(crate) fn nearest_player(
        &self,
        range: Option<f64>,
        at: Vec3,
        selector: Option<&dyn Fn(&SeenPlayer) -> bool>,
    ) -> Option<SeenPlayer> {
        let position = self.position();
        let info = EntityTypeInfo::of(i32::from(self.tick.kind));
        let eyes = [
            position[0],
            position[1] + f64::from(info.eye_height),
            position[2],
        ];
        let mut nearest: Option<(f64, SeenPlayer)> = None;
        for player in self.tick.world.players() {
            if player.creative || player.spectator || !player.alive {
                continue;
            }
            if let Some(range) = range {
                let reach = range.max(2.0);
                if physics::distance_sqr(position, player.position) > reach * reach {
                    continue;
                }
                let to = [
                    player.position[0],
                    player.position[1] + player.eye_height,
                    player.position[2],
                ];
                if !line_of_sight(self.tick.world, position[1], eyes, to) {
                    continue;
                }
            }
            if selector.is_some_and(|selector| !selector(&player)) {
                continue;
            }
            let d = physics::distance_sqr(player.position, at);
            if nearest.as_ref().is_none_or(|(least, _)| d < *least) {
                nearest = Some((d, player));
            }
        }
        nearest.map(|(_, player)| player)
    }

    /// `getHeadLookVector(1)` (2769–2853): about the podium on landing and take-off, its
    /// pitch for the look `−6 / max(√d / 4, 1) · 1.5 · 5` of its distance to the
    /// podium's middle; sitting, −45; else its own.
    pub(crate) fn head_look_vector(&self) -> Vec3 {
        let pitch = match self.own.phase {
            phases::LANDING | TAKEOFF => {
                let podium = self.podium(false);
                let d = phases::dist_to_center_sqr(podium, self.position());
                let far = ((d.sqrt() as f32) / 4.0).max(1.0);
                let f7 = 6.0 / far;
                -f7 * 1.5 * 5.0
            }
            phase if is_sitting(phase) => -45.0,
            _ => self.tick.pitch,
        };
        crate::mobs::types::ghast::view_vector(self.tick.yaw, pitch)
    }

    /// `aiStep()` (341–1303), from the server's side: the module's note.
    fn ai_step(&mut self) {
        if self.tick.mob.body.vitals.health <= 0.0 {
            // The explosions about it, the client's: their draws.
            for _ in 0..3 {
                self.tick.draws.next_float();
            }
            return;
        }
        self.check_crystals();
        self.tick.yaw = wrap_degrees(self.tick.yaw);
        if self.tick.mob.body.flags.no_ai {
            return;
        }
        let position = self.position();
        self.own.history.record(position[1], self.tick.yaw);
        let ticked = self.own.phase;
        self.phase_tick();
        if self.own.phase != ticked {
            self.phase_tick();
        }
        let phase = self.own.phase;
        if let Some(target) = phases::fly_target(&self.own.phases, phase) {
            self.fly_at(target, phase);
        }
        self.tick.mob.body.head.body_yaw = self.tick.yaw;
        self.place_parts();
    }

    /// The flight at `target` of `aiStep` (510–985).
    fn fly_at(&mut self, target: Vec3, phase: u8) {
        let position = self.position();
        let dx = target[0] - position[0];
        let mut dy = target[1] - position[1];
        let dz = target[2] - position[2];
        let distance = dx * dx + dy * dy + dz * dz;
        let speed = phases::fly_speed(phase);
        let across = (dx * dx + dz * dz).sqrt();
        if across > 0.0 {
            dy =
                crate::mobs::ai::maths::clamp_f64(dy / across, f64::from(-speed), f64::from(speed));
        }
        self.tick.living.body.motion[1] += dy * 0.01;
        self.tick.yaw = wrap_degrees(self.tick.yaw);
        let aim = physics::normalize(physics::sub(target, position));
        let yaw = self.tick.yaw * RADIANS;
        let facing = physics::normalize([
            f64::from(table_sin(yaw)),
            self.tick.living.body.motion[1],
            f64::from(-table_cos(yaw)),
        ]);
        let agreement = ((dot(facing, aim) as f32 + 0.5) / 1.5).max(0.0);
        if dx.abs() > 9.999_999_747_378_752e-6 || dz.abs() > 9.999_999_747_378_752e-6 {
            let towards = wrap_degrees(180.0 - (atan2(dx, dz) as f32) * 57.295_776 - self.tick.yaw);
            let turn = clamp(towards, -50.0, 50.0);
            self.own.y_rot_a *= 0.8;
            self.own.y_rot_a += turn * phases::turn_speed(phase, self.tick.living.body.motion);
            self.tick.yaw += self.own.y_rot_a * 0.1;
        }
        let span = (2.0 / (distance + 1.0)) as f32;
        let push = crate::physics::minecart::input_vector(
            [0.0, 0.0, -1.0],
            0.06 * (agreement * span + (1.0 - span)),
            self.tick.yaw,
        );
        let body = &mut self.tick.living.body;
        body.motion = physics::add(body.motion, push);
        let by = if self.own.in_wall {
            physics::scale(body.motion, 0.800_000_011_920_929)
        } else {
            body.motion
        };
        self.move_by(by);
        let body = &mut self.tick.living.body;
        let along = physics::normalize(body.motion);
        let slide = 0.8 + 0.15 * (dot(along, facing) + 1.0) / 2.0;
        body.motion = [
            body.motion[0] * slide,
            body.motion[1] * 0.910_000_026_226_043_7,
            body.motion[2] * slide,
        ];
    }

    /// `move(SELF, by)` of a body that moves through everything: its place, and the
    /// move recorded for the effects of the blocks it went through. Like the game's
    /// `move`, it does not ask for a sync: the dragon is sent on its tracker's every
    /// third tick, as the game sends it.
    fn move_by(&mut self, by: Vec3) {
        let body = &mut self.tick.living.body;
        let from = body.position;
        body.position = physics::add(from, by);
        body.movements.push(crate::physics::body::Movement {
            from,
            to: body.position,
            asked: Some(by),
        });
    }

    /// The parts placed about it, its wings' and head's hurts, and its walls (1002–1931).
    fn place_parts(&mut self) {
        let position = self.position();
        let history = &self.own.history;
        let slope = (history.get(5).0 - history.get(10).0) as f32 * 10.0 * RADIANS;
        let slope_cos = table_cos(slope);
        let slope_sin = table_sin(slope);
        let yaw = self.tick.yaw * RADIANS;
        let sin = table_sin(yaw);
        let cos = table_cos(yaw);
        let at = |offset: [f32; 3]| -> Vec3 {
            [
                position[0] + f64::from(offset[0]),
                position[1] + f64::from(offset[1]),
                position[2] + f64::from(offset[2]),
            ]
        };
        self.own.parts[usize::from(BODY)] = at([sin * 0.5, 0.0, -cos * 0.5]);
        self.own.parts[usize::from(WING[0])] = at([cos * 4.5, 2.0, sin * 4.5]);
        self.own.parts[usize::from(WING[1])] = at([cos * -4.5, 2.0, sin * -4.5]);
        if !self.hurt_recently() {
            let body = parts::box_of(BODY, self.own.parts[usize::from(BODY)]);
            for wing in WING {
                let reach = parts::box_of(wing, self.own.parts[usize::from(wing)])
                    .inflate([4.0, 2.0, 4.0])
                    .moved([0.0, -2.0, 0.0]);
                self.knock_back(body, reach);
            }
            // The head and neck as they stood: they are placed after.
            for part in [HEAD, NECK] {
                let reach =
                    parts::box_of(part, self.own.parts[usize::from(part)]).inflate([1.0; 3]);
                self.hurt_in(reach);
            }
        }
        let turned = self.tick.yaw * RADIANS - self.own.y_rot_a * 0.01;
        let head_sin = table_sin(turned);
        let head_cos = table_cos(turned);
        let head_y = self.head_y_offset();
        self.own.parts[usize::from(HEAD)] = at([
            head_sin * 6.5 * slope_cos,
            head_y + slope_sin * 6.5,
            -head_cos * 6.5 * slope_cos,
        ]);
        self.own.parts[usize::from(NECK)] = at([
            head_sin * 5.5 * slope_cos,
            head_y + slope_sin * 5.5,
            -head_cos * 5.5 * slope_cos,
        ]);
        let base = self.own.history.get(5);
        for (i, tail) in TAIL.into_iter().enumerate() {
            let sample = self.own.history.get(12 + i * 2);
            let turn = wrap_degrees_double(f64::from(sample.1 - base.1));
            let angle = self.tick.yaw * RADIANS + turn * RADIANS;
            let tail_sin = table_sin(angle);
            let tail_cos = table_cos(angle);
            let out = (i + 1) as f32 * 2.0;
            self.own.parts[usize::from(tail)] = [
                position[0] + f64::from(-(sin * 1.5 + tail_sin * out) * slope_cos),
                position[1] + (sample.0 - base.0 - f64::from((out + 1.5) * slope_sin) + 1.5),
                position[2] + f64::from((cos * 1.5 + tail_cos * out) * slope_cos),
            ];
        }
        let mut broken = Vec::new();
        let mut in_wall = false;
        for part in [HEAD, NECK, BODY] {
            let bounds = parts::box_of(part, self.own.parts[usize::from(part)]);
            in_wall |= self.check_walls(bounds, &mut broken);
        }
        self.own.in_wall = in_wall;
        if self.fight.is_some() {
            let health = self.tick.mob.body.vitals.health;
            self.tick
                .out
                .local
                .push(Local::Dragon(Box::new(DragonLocal::FightUpdated {
                    health,
                    max: MAX_HEALTH,
                })));
        }
    }

    /// `getHeadYOffset()`: sitting, −1; else the history's fall over its last five.
    fn head_y_offset(&self) -> f32 {
        if is_sitting(self.own.phase) {
            return -1.0;
        }
        (self.own.history.get(5).0 - self.own.history.get(0).0) as f32
    }

    /// `wasHurtRecently()`: its hurt time (10 from a hurt) not run out.
    fn hurt_recently(&self) -> bool {
        let moment = self.tick.lived.saturating_add(u32::from(self.tick.counted));
        self.tick
            .mob
            .body
            .vitals
            .hurt_from
            .is_some_and(|from| i64::from(moment) - i64::from(from) < 10)
    }

    /// `checkCrystals()` (1349–1424).
    fn check_crystals(&mut self) {
        if let Some(crystal) = self.own.nearest_crystal {
            let stands = self
                .tick
                .world
                .entity(crystal)
                .is_some_and(|seen| matches!(seen.entity.data, EntityData::EndCrystal(_)));
            if !stands {
                self.own.nearest_crystal = None;
            } else if self.tick.tick_count.wrapping_add(1).is_multiple_of(10) {
                let vitals = &mut self.tick.mob.body.vitals;
                if vitals.health < MAX_HEALTH {
                    vitals.health = (vitals.health + 1.0).clamp(0.0, MAX_HEALTH);
                }
            }
        }
        if self.tick.draws.next_int(10) == 0 {
            let position = self.position();
            let info = EntityTypeInfo::of(i32::from(self.tick.kind));
            let bounds = Aabb::of_entity(position, info.width, info.height).inflate([32.0; 3]);
            let mut nearest: Option<(f64, EntityId)> = None;
            for seen in self.tick.world.entities_in(&bounds) {
                if !matches!(seen.entity.data, EntityData::EndCrystal(_)) {
                    continue;
                }
                let d = physics::distance_sqr(seen.entity.position, position);
                if nearest.is_none_or(|(least, _)| d < least) {
                    nearest = Some((d, seen.entity.key));
                }
            }
            self.own.nearest_crystal = nearest.map(|(_, key)| key);
        }
    }

    /// `knockBack(level, entities)` (1425–1533) of the living things in `reach` but the
    /// dragon: each pushed `4 / d` away from the middle of its body, and 0.2 up; and,
    /// unless it sits, hurt by 5 where no mob hurt it in its last two ticks.
    fn knock_back(&mut self, body: Aabb, reach: Aabb) {
        let middle = [
            (body.min[0] + body.max[0]) / 2.0,
            (body.min[2] + body.max[2]) / 2.0,
        ];
        let sitting = is_sitting(self.own.phase);
        let mut locals = Vec::new();
        for seen in self.tick.world.entities_in(&reach) {
            let entity = seen.entity;
            if entity.key == self.tick.key {
                continue;
            }
            let Ok(mob) = Mob::decode(&entity.data) else {
                continue;
            };
            let dx = entity.position[0] - middle[0];
            let dz = entity.position[2] - middle[1];
            let d = (dx * dx + dz * dz).max(0.1);
            if !seen.copied {
                locals.push(Local::Push {
                    key: entity.key,
                    by: [dx / d * 4.0, 0.200_000_002_980_232_24, dz / d * 4.0],
                });
            }
            let stamp = mob.body.combat.last_hurt_by.map_or(0, |(_, at)| at);
            if !sitting && i64::from(stamp) < i64::from(entity.lived) - 2 {
                locals.push(self.hurt_local(entity.key, 5.0));
            }
        }
        self.tick.out.local.extend(locals);
    }

    /// `hurt(level, entities)` (1534–1570): every living thing in `reach` but the dragon
    /// hurt by 10.
    fn hurt_in(&mut self, reach: Aabb) {
        let keys: Vec<EntityId> = self
            .tick
            .world
            .entities_in(&reach)
            .into_iter()
            .filter(|seen| seen.entity.key != self.tick.key)
            .filter(|seen| matches!(seen.entity.data, EntityData::Mob { .. }))
            .map(|seen| seen.entity.key)
            .collect();
        for key in keys {
            let local = self.hurt_local(key, 10.0);
            self.tick.out.local.push(local);
        }
    }

    /// `hurtServer(mobAttack(dragon), amount)` of the mob `key`.
    fn hurt_local(&self, key: EntityId, amount: f32) -> Local {
        Local::Hurt {
            key,
            kind: self.tick.kind,
            from: self.position(),
            amount,
            sound: None,
            knockback: None,
            fling: None,
        }
    }

    /// `checkWalls(level, bounds)` (1578–1716): whether an immune block held it.
    /// `broken` holds what this tick's earlier boxes removed, which a later box finds as
    /// the game's finds it: gone.
    fn check_walls(&mut self, bounds: Aabb, broken: &mut Vec<BlockPos>) -> bool {
        let low = physics::containing(bounds.min);
        let high = physics::containing(bounds.max);
        let mut held = false;
        let mut removed = false;
        for x in low.x..=high.x {
            for y in low.y..=high.y {
                for z in low.z..=high.z {
                    let pos = BlockPos::new(x, y, z);
                    let state = self.tick.world.state(pos);
                    if broken.contains(&pos)
                        || state.props().is_air()
                        || in_tag(state, "minecraft:dragon_transparent")
                    {
                        continue;
                    }
                    if !MOB_GRIEFING || in_tag(state, "minecraft:dragon_immune") {
                        held = true;
                        continue;
                    }
                    // `removeBlock(pos, false)`: its fluid's block in its place.
                    let left = clustine_blocks::fluid::FluidState::of(state).legacy_block();
                    self.tick.out.local.push(Local::SetBlock {
                        pos,
                        state: left,
                        flags: 3,
                    });
                    if left.props().is_air() {
                        broken.push(pos);
                    }
                    removed = true;
                }
            }
        }
        if removed {
            let draws = &mut *self.tick.draws;
            let x = low.x + draws.next_int((high.x - low.x + 1) as u32) as i32;
            let y = low.y + draws.next_int((high.y - low.y + 1) as u32) as i32;
            let z = low.z + draws.next_int((high.z - low.z + 1) as u32) as i32;
            self.tick.out.local.push(Local::LevelEvent {
                kind: WALL_EVENT,
                pos: BlockPos::new(x, y, z),
                data: 0,
            });
        }
        held
    }

    /// `tickDeath()` (1880–2088): the module's note.
    fn tick_death(&mut self) {
        if self.fight.is_some() {
            let health = self.tick.mob.body.vitals.health;
            self.tick
                .out
                .local
                .push(Local::Dragon(Box::new(DragonLocal::FightUpdated {
                    health,
                    max: MAX_HEALTH,
                })));
        }
        self.own.death_time += 1;
        let time = self.own.death_time;
        if (180..=DEATH_TICKS).contains(&time) {
            // The explosions about it, the client's: their draws.
            for _ in 0..3 {
                self.tick.draws.next_float();
            }
        }
        let experience = match self.fight {
            Some(fight) if !fight.previously_killed => FIRST_EXPERIENCE,
            _ => LATER_EXPERIENCE,
        };
        let position = self.position();
        if time > 150 && time % 5 == 0 && MOB_DROPS {
            self.award(position, experience as f32 * 0.08);
        }
        if time == 1 {
            self.tick
                .out
                .local
                .push(Local::Dragon(Box::new(DragonLocal::GlobalEvent {
                    event: DEATH_EVENT,
                    pos: physics::containing(position),
                })));
        }
        let up = [0.0, 0.100_000_001_490_116_12, 0.0];
        self.tick.living.body.position = physics::add(position, up);
        for part in &mut self.own.parts {
            *part = physics::add(*part, up);
        }
        if time >= DEATH_TICKS {
            if MOB_DROPS {
                let risen = self.position();
                self.award(risen, experience as f32 * 0.2);
            }
            if self.fight.is_some() {
                self.tick
                    .out
                    .local
                    .push(Local::Dragon(Box::new(DragonLocal::FightKilled)));
            }
            self.tick.out.removed = Some(Removed::Killed);
            self.tick
                .out
                .events
                .push(Event::GameEvent(GameEvent::EntityDie));
        }
    }

    /// `ExperienceOrb.award(level, position(), Mth.floor(amount))` at its feet `at`.
    fn award(&mut self, at: Vec3, amount: f32) {
        self.tick
            .out
            .local
            .push(Local::Dragon(Box::new(DragonLocal::Award {
                at,
                amount: amount.floor() as i32,
            })));
    }
}

/// `Vec3.dot`.
fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `rotWrap(d)`: `Mth.wrapDegrees(double)`, narrowed.
fn wrap_degrees_double(degrees: f64) -> f32 {
    phases::wrap_degrees_f64(degrees) as f32
}

/// The dragon.
pub(crate) struct EnderDragon;

impl EnderDragon {
    /// The step of its tick, with its state and its fight.
    fn step(tick: &mut MobTick<'_>, what: impl FnOnce(&mut Step<'_, '_>)) {
        let mut own = state_of(&tick.mob);
        let fight = tick.world.dragon_fight(tick.key);
        {
            let mut step = Step {
                tick: &mut *tick,
                own: &mut own,
                fight,
            };
            what(&mut step);
        }
        put(&mut tick.mob, &own);
    }
}

impl MobType for EnderDragon {
    /// It moves through everything (`noPhysics`), no wall smothers it, and a blow that
    /// left it at 1 and dying keeps it alive; a dead one counts its own death, never the
    /// body's twenty ticks (the module's note).
    fn before_tick(&self, tick: &mut MobTick<'_>) {
        tick.living.no_physics = true;
        let vitals = &mut tick.mob.body.vitals;
        if vitals.health > 0.0 {
            vitals.died_from = None;
        } else {
            vitals.died_from = Some(tick.lived);
        }
    }

    /// Its own `tickDeath`, in `baseTick`.
    fn after_base_tick(&self, tick: &mut MobTick<'_>, _: i32) {
        if tick.mob.body.vitals.health <= 0.0 && tick.out.removed.is_none() {
            Self::step(tick, |step| step.tick_death());
        }
    }

    /// Its `aiStep`, in place of the living body's: it travels no more of itself.
    fn before_ai_step(&self, tick: &mut MobTick<'_>) {
        if tick.out.removed.is_some() {
            return;
        }
        Self::step(tick, |step| {
            step.ai_step();
            step.own.kept_motion = Some(step.tick.living.body.motion);
        });
        tick.living.travels = false;
    }

    /// Its motion as its step left it, where only the living body's zeroing of small
    /// motion changed it since.
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        let mut own = state_of(&tick.mob);
        let Some(kept) = own.kept_motion.take() else {
            return;
        };
        let zeroed = kept.map(|value| if value.abs() < 0.003 { 0.0 } else { value });
        if tick.living.body.motion.map(f64::to_bits) == zeroed.map(f64::to_bits) {
            tick.living.body.motion = kept;
        }
        put(&mut tick.mob, &own);
    }

    /// No goal moves it.
    fn immobile(&self, _: &MobTick<'_>) -> bool {
        true
    }

    fn no_gravity(&self, _: &Mob) -> bool {
        true
    }

    /// Nothing pushes it, nor does it push (`noPhysics`).
    fn pushable(&self) -> bool {
        false
    }

    fn pushed_by_fluid(&self) -> bool {
        false
    }

    /// `checkDespawn()` does nothing.
    fn requires_custom_persistence(&self, _: &Mob) -> bool {
        true
    }

    fn remove_when_far_away(&self, _: u16, _: &Mob, _: u32, _: f64) -> bool {
        false
    }

    /// Its own death awards its experience: its body's drops none.
    fn experience_reward(&self, _: u16, _: &Mob, _: &mut Random) -> i32 {
        0
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_ENDER_DRAGON_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_ENDER_DRAGON_HURT)
    }

    /// `getSoundVolume()`: 5.
    fn sound_volume(&self) -> f32 {
        5.0
    }

    /// `hurt(part, …)` before `reallyHurt`: the module's note. The part is the one a hit
    /// named, else the body (`hurtServer`).
    fn before_hurt_server(&self, struck: &mut Struck<'_>, source: &DamageSource, amount: f32) {
        let health = struck.mob.body.vitals.health;
        let motion = *struck.motion;
        change(struck.mob, |own| {
            let part = own.hit_part.take().unwrap_or(BODY);
            own.verdict = Some(verdict(own.phase, part, source, amount, health, motion));
        });
    }

    fn hurt_amount(&self, mob: &Mob, amount: f32) -> f32 {
        match state_of(mob).verdict {
            Some(Verdict::Taken { amount, .. }) => amount,
            _ => amount,
        }
    }

    fn refuses_hurt(&self, mob: &Mob, _: &DamageSource) -> bool {
        matches!(state_of(mob).verdict, Some(Verdict::Refused))
    }

    /// After `reallyHurt`: a sitting dragon's knockback undone (`knockback` does nothing
    /// while it sits), and more than a quarter of its health taken while it sits makes
    /// it take off.
    fn after_hurt(&self, struck: &mut Struck<'_>, _: &DamageSource, _: bool) {
        let health = struck.mob.body.vitals.health;
        let mut own = state_of(struck.mob);
        let Some(verdict) = own.verdict.take() else {
            return;
        };
        if let Verdict::Taken {
            health: before,
            motion,
            sitting,
            ..
        } = verdict
        {
            if sitting {
                *struck.motion = motion;
            }
            if is_sitting(own.phase) {
                own.sitting_damage = own.sitting_damage + before - health;
                if own.sitting_damage > SITTING_ALLOWED * MAX_HEALTH {
                    own.sitting_damage = 0.0;
                    phases::set_phase(
                        &mut own.phase,
                        &mut own.phases,
                        TAKEOFF,
                        &mut struck.out.local,
                    );
                }
            }
        }
        put(struck.mob, &own);
    }

    /// `handleKillingBlow()` (1825–1840): outside a sitting phase, it lives on at 1,
    /// dying.
    fn on_die(&self, struck: &mut Struck<'_>, _: &DamageSource) {
        let mut own = state_of(struck.mob);
        if is_sitting(own.phase) {
            return;
        }
        struck.mob.body.vitals.health = 1.0;
        phases::set_phase(
            &mut own.phase,
            &mut own.phases,
            DYING,
            &mut struck.out.local,
        );
        put(struck.mob, &own);
    }

    /// `DATA_PHASE`, hovering by default.
    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let phase = state_of(mob).phase;
        vec![(
            PHASE_INDEX,
            DataShown::Int(i32::from(phase)),
            phase == HOVERING,
        )]
    }

    /// `readAdditionalSaveData`: `DragonPhase` set as `setPhase` sets it, and
    /// `DragonDeathTime`.
    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        let phase = super::template::int(tag, "DragonPhase");
        let death_time = super::template::int(tag, "DragonDeathTime");
        change(mob, |own| {
            if let Some(phase) = phase {
                let mut ignored = Vec::new();
                phases::set_phase(
                    &mut own.phase,
                    &mut own.phases,
                    phases::by_id(phase),
                    &mut ignored,
                );
            }
            own.death_time = death_time.unwrap_or(0);
        });
    }
}

/// `hurt(part, source, amount)`'s own part (1717–1803) of a dragon in the phase `phase`,
/// of health `health` and motion `motion`, hit at `part`.
fn verdict(
    phase: u8,
    part: u8,
    source: &DamageSource,
    amount: f32,
    health: f32,
    motion: Vec3,
) -> Verdict {
    if phase == DYING {
        return Verdict::Refused;
    }
    // `AbstractDragonSittingPhase.onHurt`: an arrow's or a wind charge's hurt is nothing.
    let sitting = is_sitting(phase);
    let mut amount = amount;
    if matches!(
        phase,
        phases::SITTING_FLAMING | phases::SITTING_SCANNING | phases::SITTING_ATTACKING
    ) && source.direct.as_ref().is_some_and(|direct| {
        matches!(
            i32::from(direct.kind),
            entity_types::ARROW
                | entity_types::SPECTRAL_ARROW
                | entity_types::TRIDENT
                | entity_types::WIND_CHARGE
        )
    }) {
        amount = 0.0;
    }
    if part != HEAD && part != NECK {
        amount = amount / 4.0 + amount.min(1.0);
    }
    if amount < 0.01 {
        return Verdict::Refused;
    }
    let by_player = source
        .cause
        .as_ref()
        .is_some_and(|cause| cause.player.is_some());
    if !by_player && !source.kind.is("minecraft:always_hurts_ender_dragons") {
        return Verdict::Refused;
    }
    Verdict::Taken {
        amount,
        health,
        motion,
        sitting,
    }
}

#[cfg(test)]
mod tests;
