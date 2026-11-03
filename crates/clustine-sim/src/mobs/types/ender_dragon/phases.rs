//! The dragon's eleven phases (`EnderDragonPhase` and its `DragonPhaseInstance`s; d
//! net.minecraft.world.entity.boss.enderdragon.phases.*), by their ids: 0 the holding
//! pattern, 1 the strafe at a player, 2 the landing approach, 3 the landing, 4 the
//! take-off, 5 the sitting flame, 6 the sitting scan, 7 the sitting roar, 8 the charge at
//! a player, 9 its death's flight to the podium, 10 the hover.
//!
//! The phase manager makes each phase's instance once and keeps it for the dragon's life
//! (`EnderDragonPhaseManager.getPhase`), so what an instance holds outlives the phase:
//! [`Phases`] keeps every instance's fields. `setPhase` to the phase it is in does
//! nothing; to another it ends the old (`end`), and begins the new (`begin`).
//!
//! The players a phase looks for are found by `TargetingConditions.forCombat()`, which
//! no creative or spectator player passes (`Player.canBeSeenAsEnemy`): the dragon
//! strafes and charges survival and adventure players, and with only creative ones about
//! lands with no player in mind (`docs/groundwork/p-plan.md`, section 19.5 item 14).

use clustine_world::EntityId;
use serde::{Deserialize, Serialize};

use super::nodes::{self, Path};
use super::parts::HEAD;
use super::{DragonLocal, Step};
use crate::mobs::ai::maths::{DEGREES, RADIANS, atan2, table_cos, table_sin};
use crate::mobs::tick::Local;
use crate::physics::{self, Vec3};

pub(crate) const HOLDING_PATTERN: u8 = 0;
pub(crate) const STRAFE_PLAYER: u8 = 1;
pub(crate) const LANDING_APPROACH: u8 = 2;
pub(crate) const LANDING: u8 = 3;
pub(crate) const TAKEOFF: u8 = 4;
pub(crate) const SITTING_FLAMING: u8 = 5;
pub(crate) const SITTING_SCANNING: u8 = 6;
pub(crate) const SITTING_ATTACKING: u8 = 7;
pub(crate) const CHARGING_PLAYER: u8 = 8;
pub(crate) const DYING: u8 = 9;
pub(crate) const HOVERING: u8 = 10;

/// `EnderDragonPhase.getById(id)`: an id out of range is the holding pattern.
pub(crate) fn by_id(id: i32) -> u8 {
    u8::try_from(id)
        .ok()
        .filter(|&phase| phase <= HOVERING)
        .unwrap_or(HOLDING_PATTERN)
}

/// `isSitting()`: the three sitting phases (`AbstractDragonSittingPhase`) and the hover.
pub(crate) fn is_sitting(phase: u8) -> bool {
    matches!(
        phase,
        SITTING_FLAMING | SITTING_SCANNING | SITTING_ATTACKING | HOVERING
    )
}

/// `getFlySpeed()`: 0.6 (`AbstractDragonPhaseInstance`), the landing's 1.5, the
/// charge's and the death's 3, the hover's 1.
pub(crate) fn fly_speed(phase: u8) -> f32 {
    match phase {
        LANDING => 1.5,
        CHARGING_PLAYER | DYING => 3.0,
        HOVERING => 1.0,
        _ => 0.6,
    }
}

/// `getTurnSpeed()` at the motion `motion`: `0.7 / min(h + 1, 40) / (h + 1)` of its
/// horizontal speed `h`, the landing's without the 0.7.
pub(crate) fn turn_speed(phase: u8, motion: Vec3) -> f32 {
    let speed = physics::horizontal_sqr(motion).sqrt() as f32 + 1.0;
    let most = speed.min(40.0);
    if phase == LANDING {
        most / speed
    } else {
        0.7 / most / speed
    }
}

/// The holding pattern's fields (`DragonHoldingPatternPhase`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Holding {
    pub(crate) path: Option<Path>,
    pub(crate) target: Option<Vec3>,
    pub(crate) clockwise: bool,
}

/// The strafe's fields (`DragonStrafePlayerPhase`): its target by key.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Strafe {
    pub(crate) fireball_charge: i32,
    pub(crate) path: Option<Path>,
    pub(crate) target: Option<Vec3>,
    pub(crate) attack: Option<EntityId>,
    pub(crate) clockwise: bool,
}

/// A phase that flies a path to a point (`DragonLandingApproachPhase`,
/// `DragonTakeoffPhase`'s path and target).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Approach {
    pub(crate) path: Option<Path>,
    pub(crate) target: Option<Vec3>,
}

/// The take-off's fields (`DragonTakeoffPhase`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Takeoff {
    pub(crate) first_tick: bool,
    pub(crate) path: Option<Path>,
    pub(crate) target: Option<Vec3>,
}

/// The sitting flame's fields (`DragonSittingFlamingPhase`): whether its cloud of breath
/// stands (`flame`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Flaming {
    pub(crate) ticks: i32,
    pub(crate) count: i32,
    pub(crate) flame: bool,
}

/// A phase that flies at a point it keeps, with a count (`DragonChargePlayerPhase`'s
/// `timeSinceCharge`, `DragonDeathPhase`'s `time`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Aimed {
    pub(crate) target: Option<Vec3>,
    pub(crate) time: i32,
}

/// Every phase instance's fields, kept for the dragon's life.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Phases {
    pub(crate) holding: Holding,
    pub(crate) strafe: Strafe,
    pub(crate) approach: Approach,
    pub(crate) landing: Option<Vec3>,
    pub(crate) takeoff: Takeoff,
    pub(crate) flaming: Flaming,
    pub(crate) scanning: i32,
    pub(crate) attacking: i32,
    pub(crate) charge: Aimed,
    pub(crate) dying: Aimed,
    pub(crate) hover: Option<Vec3>,
}

/// `getFlyTargetLocation()` of the phase `phase`.
pub(crate) fn fly_target(phases: &Phases, phase: u8) -> Option<Vec3> {
    match phase {
        HOLDING_PATTERN => phases.holding.target,
        STRAFE_PLAYER => phases.strafe.target,
        LANDING_APPROACH => phases.approach.target,
        LANDING => phases.landing,
        TAKEOFF => phases.takeoff.target,
        CHARGING_PLAYER => phases.charge.target,
        DYING => phases.dying.target,
        HOVERING => phases.hover,
        _ => None,
    }
}

/// `begin()` of the phase `phase`.
fn begin(phases: &mut Phases, phase: u8) {
    match phase {
        HOLDING_PATTERN => {
            phases.holding.path = None;
            phases.holding.target = None;
        }
        STRAFE_PLAYER => {
            phases.strafe.fireball_charge = 0;
            phases.strafe.target = None;
            phases.strafe.path = None;
            phases.strafe.attack = None;
        }
        LANDING_APPROACH => phases.approach = Approach::default(),
        LANDING => phases.landing = None,
        TAKEOFF => {
            phases.takeoff.first_tick = true;
            phases.takeoff.path = None;
            phases.takeoff.target = None;
        }
        SITTING_FLAMING => {
            phases.flaming.ticks = 0;
            phases.flaming.count += 1;
        }
        SITTING_SCANNING => phases.scanning = 0,
        SITTING_ATTACKING => phases.attacking = 0,
        CHARGING_PLAYER => phases.charge = Aimed::default(),
        DYING => phases.dying = Aimed::default(),
        HOVERING => phases.hover = None,
        _ => {}
    }
}

/// `EnderDragonPhaseManager.setPhase(phase)` of a dragon in the phase `*current`:
/// nothing where it is in it already; else the old one's `end()` (the sitting flame's
/// cloud discarded, `locals` taking the word), the new one's `begin()`.
pub(crate) fn set_phase(current: &mut u8, phases: &mut Phases, phase: u8, locals: &mut Vec<Local>) {
    if *current == phase {
        return;
    }
    if *current == SITTING_FLAMING && phases.flaming.flame {
        phases.flaming.flame = false;
        locals.push(Local::Dragon(Box::new(DragonLocal::DiscardFlame)));
    }
    *current = phase;
    begin(phases, phase);
}

/// The ring a new path of a strafe or a take-off ends on: about node `j`'s place, kept
/// on the outer ring while crystals stand, else on the two inner.
fn ring_of(j: i32, outer: bool) -> usize {
    let j = if outer {
        let k = j % 12;
        if k < 0 { k + 12 } else { k }
    } else {
        ((j - 12) & 7) + 12
    };
    j as usize
}

impl Step<'_, '_> {
    fn set_phase(&mut self, phase: u8) {
        set_phase(
            &mut self.own.phase,
            &mut self.own.phases,
            phase,
            &mut self.tick.out.local,
        );
    }

    /// `doServerTick` of the phase the dragon is in.
    pub(crate) fn phase_tick(&mut self) {
        match self.own.phase {
            HOLDING_PATTERN => self.holding_tick(),
            STRAFE_PLAYER => self.strafe_tick(),
            LANDING_APPROACH => self.approach_tick(),
            LANDING => self.landing_tick(),
            TAKEOFF => self.takeoff_tick(),
            SITTING_FLAMING => self.flaming_tick(),
            SITTING_SCANNING => self.scanning_tick(),
            SITTING_ATTACKING => {
                // `attackingTicks++ >= 40`: the count before it rises
                // (DragonSittingAttackingPhase.txt 45–61).
                let before = self.own.phases.attacking;
                self.own.phases.attacking += 1;
                if before >= 40 {
                    self.set_phase(SITTING_FLAMING);
                }
            }
            CHARGING_PLAYER => self.charge_tick(),
            DYING => self.dying_tick(),
            HOVERING if self.own.phases.hover.is_none() => {
                self.own.phases.hover = Some(self.position());
            }
            _ => {}
        }
    }

    /// Whether the dragon is close enough to `target` to look for another, or too far
    /// (`d < 100 || d > 22500`), or ran into something (never, as it moves through
    /// everything: `noPhysics`).
    fn reached(&self, target: Option<Vec3>) -> bool {
        let d = target.map_or(0.0, |target| physics::distance_sqr(target, self.position()));
        d < 100.0 || d > 22_500.0
    }

    /// `navigateToNextPathNode()` of the holding pattern, the strafe and the landing
    /// approach: the next node of `path`, advanced past, with a height of up to 20 more
    /// drawn (`nextFloat() · 20`, again until it is not below the node).
    fn next_target(&mut self, path: &mut Option<Path>, target: &mut Option<Vec3>) {
        let Some(path) = path.as_mut().filter(|path| !path.is_done()) else {
            return;
        };
        let node = path.next_node();
        path.advance();
        *target = Some(self.target_above(node));
    }

    /// The point above `node` a path flies to: its x and z, its y raised by
    /// `nextFloat() · 20`, drawn again while it is below the node.
    fn target_above(&mut self, node: nodes::Node) -> Vec3 {
        loop {
            let y = f64::from(node[1] as f32 + self.tick.draws.next_float() * 20.0);
            if y >= f64::from(node[1]) {
                return [f64::from(node[0]), y, f64::from(node[2])];
            }
        }
    }

    // The holding pattern (DragonHoldingPatternPhase.txt).

    fn holding_tick(&mut self) {
        if self.reached(self.own.phases.holding.target) {
            self.holding_new_target();
        }
    }

    /// `findNewTarget(level)` (DragonHoldingPatternPhase.txt 81–200 of the method): at
    /// the end of a path, a landing one draw in `crystals + 3`, else a strafe at the
    /// player nearest the podium by two draws; else, or with no path, a new path about
    /// the ring from the nearest node, turned about one draw in eight.
    fn holding_new_target(&mut self) {
        let done = self
            .own
            .phases
            .holding
            .path
            .as_ref()
            .is_some_and(Path::is_done);
        if done {
            let podium = self.podium(false);
            let crystals = self.crystals();
            if self.tick.draws.next_int(crystals as u32 + 3) == 0 {
                self.set_phase(LANDING_APPROACH);
                return;
            }
            let at = [
                f64::from(podium.x),
                f64::from(podium.y),
                f64::from(podium.z),
            ];
            let player = self.nearest_player(None, at, None);
            let d = player.as_ref().map_or(64.0, |player| {
                dist_to_center_sqr(podium, player.position) / 512.0
            });
            if let Some(player) = player
                && (self.tick.draws.next_int((d + 2.0) as i32 as u32) == 0
                    || self.tick.draws.next_int(crystals as u32 + 2) == 0)
            {
                self.strafe_player(player.key, player.position);
                return;
            }
        }
        let done = self
            .own
            .phases
            .holding
            .path
            .as_ref()
            .is_none_or(Path::is_done);
        if done {
            let i = self.closest_node();
            let mut j = i as i32;
            if self.tick.draws.next_int(8) == 0 {
                self.own.phases.holding.clockwise = !self.own.phases.holding.clockwise;
                j += 6;
            }
            j += if self.own.phases.holding.clockwise {
                1
            } else {
                -1
            };
            // `getDragonFight() == null || aliveCrystals() < 0`: with a fight, always
            // the outer ring.
            let j = ring_of(j, self.fight.is_some());
            let mut path = self.find_path(i, j, None);
            if let Some(path) = path.as_mut() {
                path.advance();
            }
            self.own.phases.holding.path = path;
        }
        let mut path = self.own.phases.holding.path.take();
        let mut target = self.own.phases.holding.target;
        self.next_target(&mut path, &mut target);
        self.own.phases.holding.path = path;
        self.own.phases.holding.target = target;
    }

    /// `strafePlayer(player)`: the strafe, at the player `key` standing at `at`.
    pub(crate) fn strafe_player(&mut self, key: EntityId, at: Vec3) {
        self.set_phase(STRAFE_PLAYER);
        self.strafe_set_target(key, at);
    }

    // The strafe (DragonStrafePlayerPhase.txt).

    /// `setTarget(target)`: a path from the nearest node to the node nearest the target,
    /// on to a point above its block (`0.4 + d / 80 − 1`, at most 10, over its feet).
    fn strafe_set_target(&mut self, key: EntityId, at: Vec3) {
        self.own.phases.strafe.attack = Some(key);
        let i = self.closest_node();
        let j = self.closest_node_at(at);
        let x = at[0].floor() as i32;
        let z = at[2].floor() as i32;
        let dx = f64::from(x) - self.position()[0];
        let dz = f64::from(z) - self.position()[2];
        let d = (dx * dx + dz * dz).sqrt();
        let above = (0.400_000_005_960_464_5 + d / 80.0 - 1.0).min(10.0);
        let y = (at[1] + above).floor() as i32;
        let mut path = self.find_path(i, j, Some([x, y, z]));
        if let Some(found) = path.as_mut() {
            found.advance();
            let mut target = self.own.phases.strafe.target;
            self.next_target(&mut path, &mut target);
            self.own.phases.strafe.target = target;
        }
        self.own.phases.strafe.path = path;
    }

    fn strafe_tick(&mut self) {
        let Some(target) = self.strafe_target() else {
            self.set_phase(HOLDING_PATTERN);
            return;
        };
        let done = self
            .own
            .phases
            .strafe
            .path
            .as_ref()
            .is_some_and(Path::is_done);
        if done {
            let [x, y, z] = target.position;
            let dx = x - self.position()[0];
            let dz = z - self.position()[2];
            let d = (dx * dx + dz * dz).sqrt();
            let above = (0.400_000_005_960_464_5 + d / 80.0 - 1.0).min(10.0);
            self.own.phases.strafe.target = Some([x, y + above, z]);
        }
        if self.reached(self.own.phases.strafe.target) {
            self.strafe_new_target();
        }
        let far = physics::distance_sqr(target.position, self.position()) >= 4096.0;
        let charge = &mut self.own.phases.strafe.fireball_charge;
        if far {
            if *charge > 0 {
                *charge -= 1;
            }
            return;
        }
        if !self.sees(&target) {
            let charge = &mut self.own.phases.strafe.fireball_charge;
            if *charge > 0 {
                *charge -= 1;
            }
            return;
        }
        self.own.phases.strafe.fireball_charge += 1;
        let position = self.position();
        let towards = physics::normalize([
            target.position[0] - position[0],
            0.0,
            target.position[2] - position[2],
        ]);
        let yaw = self.tick.yaw * RADIANS;
        let facing =
            physics::normalize([f64::from(table_sin(yaw)), 0.0, f64::from(-table_cos(yaw))]);
        let dot = (facing[0] * towards[0] + facing[1] * towards[1] + facing[2] * towards[2]) as f32;
        let angle = (libm::acos(f64::from(dot)) * DEGREES) as f32 + 0.5;
        if self.own.phases.strafe.fireball_charge < 5 || !(0.0..10.0).contains(&angle) {
            return;
        }
        let view = crate::mobs::types::ghast::view_vector(self.tick.yaw, self.tick.pitch);
        let head = self.own.parts[usize::from(HEAD)];
        let from = [head[0] - view[0], head[1] + 0.5 + 0.5, head[2] - view[2]];
        let direction = [
            target.position[0] - from[0],
            target.position[1] + target.height * 0.5 - from[1],
            target.position[2] - from[2],
        ];
        self.tick.out.local.push(Local::LevelEvent {
            kind: super::FIREBALL_SHOT,
            pos: physics::containing(position),
            data: 0,
        });
        self.tick.out.local.push(Local::ShootFireball {
            kind: super::DRAGON_FIREBALL,
            at: from,
            direction: physics::normalize(direction),
            explosion_power: 0,
        });
        self.own.phases.strafe.fireball_charge = 0;
        if let Some(path) = self.own.phases.strafe.path.as_mut() {
            while !path.is_done() {
                path.advance();
            }
        }
        self.set_phase(HOLDING_PATTERN);
    }

    /// The strafe's target as the dragon sees it now: a player it reads by key, or a mob
    /// of the region's. **Named**: one it no longer sees is taken as none, and the strafe
    /// ends, where the game's keeps flying at the last it knew of it.
    fn strafe_target(&self) -> Option<Target> {
        let key = self.own.phases.strafe.attack?;
        if let Some(player) = self
            .tick
            .world
            .players()
            .into_iter()
            .find(|player| player.key == key)
        {
            return Some(Target {
                position: player.position,
                eye_height: player.eye_height,
                height: PLAYER_HEIGHT,
            });
        }
        let seen = self.tick.world.entity(key)?;
        let size = seen.bounds.size();
        let info = clustine_data::EntityTypeInfo::of(i32::from(seen.entity.kind));
        Some(Target {
            position: seen.entity.position,
            eye_height: f64::from(info.eye_height),
            height: size[1],
        })
    }

    /// `findNewTarget()` of the strafe: a new path about the ring where the old one is
    /// done, then its next node.
    fn strafe_new_target(&mut self) {
        let done = self
            .own
            .phases
            .strafe
            .path
            .as_ref()
            .is_none_or(Path::is_done);
        if done {
            let i = self.closest_node();
            let mut j = i as i32;
            if self.tick.draws.next_int(8) == 0 {
                self.own.phases.strafe.clockwise = !self.own.phases.strafe.clockwise;
                j += 6;
            }
            j += if self.own.phases.strafe.clockwise {
                1
            } else {
                -1
            };
            let j = ring_of(j, self.crystals() > 0);
            let mut path = self.find_path(i, j, None);
            if let Some(path) = path.as_mut() {
                path.advance();
            }
            self.own.phases.strafe.path = path;
        }
        let mut path = self.own.phases.strafe.path.take();
        let mut target = self.own.phases.strafe.target;
        self.next_target(&mut path, &mut target);
        self.own.phases.strafe.path = path;
        self.own.phases.strafe.target = target;
    }

    /// `hasLineOfSight(target)`: from its eyes to the target's, within 128 blocks, no
    /// block's collision shape in the way.
    fn sees(&self, target: &Target) -> bool {
        let position = self.position();
        let info = clustine_data::EntityTypeInfo::of(i32::from(self.tick.kind));
        let eyes = [
            position[0],
            position[1] + f64::from(info.eye_height),
            position[2],
        ];
        let to = [
            target.position[0],
            target.position[1] + target.eye_height,
            target.position[2],
        ];
        super::line_of_sight(self.tick.world, position[1], eyes, to)
    }

    // The landing approach (DragonLandingApproachPhase.txt).

    fn approach_tick(&mut self) {
        if !self.reached(self.own.phases.approach.target) {
            return;
        }
        let renew = self
            .own
            .phases
            .approach
            .path
            .as_ref()
            .is_none_or(Path::is_done);
        if renew {
            let i = self.closest_node();
            let podium = self.podium(false);
            let at = [
                f64::from(podium.x),
                f64::from(podium.y),
                f64::from(podium.z),
            ];
            let j = match self.nearest_player(None, at, None) {
                Some(player) => {
                    let away = physics::normalize([player.position[0], 0.0, player.position[2]]);
                    self.closest_node_at([-away[0] * 40.0, 105.0, -away[2] * 40.0])
                }
                None => self.closest_node_at([40.0, f64::from(podium.y), 0.0]),
            };
            let mut path = self.find_path(i, j, Some([podium.x, podium.y, podium.z]));
            if let Some(path) = path.as_mut() {
                path.advance();
            }
            self.own.phases.approach.path = path;
        }
        let mut path = self.own.phases.approach.path.take();
        let mut target = self.own.phases.approach.target;
        self.next_target(&mut path, &mut target);
        let done = path.as_ref().is_some_and(Path::is_done);
        self.own.phases.approach.path = path;
        self.own.phases.approach.target = target;
        if done {
            self.set_phase(LANDING);
        }
    }

    // The landing (DragonLandingPhase.txt).

    fn landing_tick(&mut self) {
        if self.own.phases.landing.is_none() {
            let podium = self.podium(false);
            self.own.phases.landing = Some(bottom_centre(podium));
        }
        let target = self.own.phases.landing.unwrap_or_default();
        if physics::distance_sqr(target, self.position()) < 1.0 {
            // `getPhase(SITTING_FLAMING).resetFlameCount()`.
            self.own.phases.flaming.count = 0;
            self.set_phase(SITTING_SCANNING);
        }
    }

    // The take-off (DragonTakeoffPhase.txt).

    fn takeoff_tick(&mut self) {
        if !self.own.phases.takeoff.first_tick && self.own.phases.takeoff.path.is_some() {
            let podium = self.podium(false);
            if dist_to_center_sqr(podium, self.position()) >= 100.0 {
                self.set_phase(HOLDING_PATTERN);
            }
            return;
        }
        self.own.phases.takeoff.first_tick = false;
        self.takeoff_new_target();
    }

    /// `findNewTarget()` of the take-off: a path from the nearest node to the node
    /// nearest a point 40 out along the way its head looks, at y 105.
    fn takeoff_new_target(&mut self) {
        let i = self.closest_node();
        let look = self.head_look_vector();
        let j = self.closest_node_at([-look[0] * 40.0, 105.0, -look[2] * 40.0]);
        let j = ring_of(j as i32, self.crystals() > 0);
        let mut path = self.find_path(i, j, None);
        // `navigateToNextPathNode()`: past the first node, then the next.
        if let Some(found) = path.as_mut() {
            found.advance();
            if !found.is_done() {
                let node = found.next_node();
                found.advance();
                self.own.phases.takeoff.target = Some(self.target_above(node));
            }
        }
        self.own.phases.takeoff.path = path;
    }

    // The sitting phases (DragonSittingFlamingPhase.txt, DragonSittingScanningPhase.txt).

    /// The flame (`doServerTick`, 0–355): at 200 ticks the take-off after its fourth,
    /// else the scan again; at its tenth tick its cloud of breath, 2.5 blocks out from its
    /// head along its neck, on the first block below that is not air.
    fn flaming_tick(&mut self) {
        self.own.phases.flaming.ticks += 1;
        if self.own.phases.flaming.ticks >= 200 {
            let next = if self.own.phases.flaming.count >= 4 {
                TAKEOFF
            } else {
                SITTING_SCANNING
            };
            self.set_phase(next);
            return;
        }
        if self.own.phases.flaming.ticks != 10 {
            return;
        }
        let head = self.own.parts[usize::from(HEAD)];
        let position = self.position();
        let out = physics::normalize([head[0] - position[0], 0.0, head[2] - position[2]]);
        let x = head[0] + out[0] * 5.0 / 2.0;
        let z = head[2] + out[2] * 5.0 / 2.0;
        // `head.getY(0.5)`: half its height up.
        let from = head[1] + 0.5;
        let mut y = from;
        loop {
            let pos =
                clustine_world::BlockPos::new(x.floor() as i32, y.floor() as i32, z.floor() as i32);
            if !self.tick.world.state(pos).props().is_air() {
                break;
            }
            y -= 1.0;
            if y < 0.0 {
                y = from;
                break;
            }
        }
        let y = f64::from(y.floor() as i32 + 1);
        self.tick
            .out
            .local
            .push(Local::Dragon(Box::new(DragonLocal::BreathCloud {
                at: [x, y, z],
            })));
        self.own.phases.flaming.flame = true;
    }

    /// The scan (`doServerTick`, 0–473): a player within 20 and 10 up or down that it
    /// sees roars after 25 ticks, and it turns to face one that is off to its side; with
    /// none after 100 ticks it takes off, and charges a player within 150 it sees.
    fn scanning_tick(&mut self) {
        self.own.phases.scanning += 1;
        let position = self.position();
        let height = position[1];
        let near = self.nearest_player(
            Some(20.0),
            position,
            Some(&|player: &crate::mobs::view::SeenPlayer| {
                (player.position[1] - height).abs() <= 10.0
            }),
        );
        if let Some(player) = near {
            if self.own.phases.scanning > 25 {
                self.set_phase(SITTING_ATTACKING);
                return;
            }
            let towards = physics::normalize([
                player.position[0] - position[0],
                0.0,
                player.position[2] - position[2],
            ]);
            let yaw = self.tick.yaw * RADIANS;
            let facing =
                physics::normalize([f64::from(table_sin(yaw)), 0.0, f64::from(-table_cos(yaw))]);
            let dot =
                (facing[0] * towards[0] + facing[1] * towards[1] + facing[2] * towards[2]) as f32;
            let angle = (libm::acos(f64::from(dot)) * DEGREES) as f32 + 0.5;
            if angle < 0.0 || angle > 10.0 {
                let head = self.own.parts[usize::from(HEAD)];
                let dx = player.position[0] - head[0];
                let dz = player.position[2] - head[2];
                let turn = crate::mobs::ai::maths::clamp_f64(
                    wrap_degrees_f64(180.0 - atan2(dx, dz) * DEGREES - f64::from(self.tick.yaw)),
                    -100.0,
                    100.0,
                );
                self.own.y_rot_a *= 0.8;
                let mut f = (dx * dx + dz * dz).sqrt() as f32 + 1.0;
                let g = f;
                if f > 40.0 {
                    f = 40.0;
                }
                self.own.y_rot_a += turn as f32 * (0.7 / f / g);
                self.tick.yaw += self.own.y_rot_a;
            }
            return;
        }
        if self.own.phases.scanning < 100 {
            return;
        }
        let charged = self.nearest_player(Some(150.0), position, None);
        self.set_phase(TAKEOFF);
        if let Some(player) = charged {
            self.set_phase(CHARGING_PLAYER);
            self.own.phases.charge.target = Some(player.position);
        }
    }

    // The charge and the death (DragonChargePlayerPhase.txt, DragonDeathPhase.txt).

    fn charge_tick(&mut self) {
        let Some(target) = self.own.phases.charge.target else {
            self.set_phase(HOLDING_PATTERN);
            return;
        };
        // `timeSinceCharge > 0 && timeSinceCharge++ >= 10`: the count before it rises
        // (DragonChargePlayerPhase.txt 31–49).
        if self.own.phases.charge.time > 0 {
            let before = self.own.phases.charge.time;
            self.own.phases.charge.time += 1;
            if before >= 10 {
                self.set_phase(HOLDING_PATTERN);
                return;
            }
        }
        let d = physics::distance_sqr(target, self.position());
        if d < 100.0 || d > 22_500.0 {
            self.own.phases.charge.time += 1;
        }
    }

    /// Its death's flight to the top of the podium (`MOTION_BLOCKING`, not leaving
    /// leaves out): there, or far, its health is 0 and it dies; until then 1.
    fn dying_tick(&mut self) {
        self.own.phases.dying.time += 1;
        if self.own.phases.dying.target.is_none() {
            let podium = self.podium(true);
            self.own.phases.dying.target = Some(bottom_centre(podium));
        }
        let target = self.own.phases.dying.target.unwrap_or_default();
        let d = physics::distance_sqr(target, self.position());
        self.tick.mob.body.vitals.health = if !(100.0..=22_500.0).contains(&d) {
            0.0
        } else {
            1.0
        };
    }
}

/// A standing player's height, which a strafe aims at the middle of (`getY(0.5)`).
/// **Named**: a crouching player's middle is not told apart.
const PLAYER_HEIGHT: f64 = 1.8;

/// What a strafe aims at.
struct Target {
    position: Vec3,
    eye_height: f64,
    height: f64,
}

/// `BlockPos.distToCenterSqr(position)`.
pub(crate) fn dist_to_center_sqr(pos: clustine_world::BlockPos, at: Vec3) -> f64 {
    let dx = f64::from(pos.x) + 0.5 - at[0];
    let dy = f64::from(pos.y) + 0.5 - at[1];
    let dz = f64::from(pos.z) + 0.5 - at[2];
    dx * dx + dy * dy + dz * dz
}

/// `Vec3.atBottomCenterOf(pos)`.
fn bottom_centre(pos: clustine_world::BlockPos) -> Vec3 {
    [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y),
        f64::from(pos.z) + 0.5,
    ]
}

/// `Mth.wrapDegrees(double)`: into [-180, 180), the remainder taking the dividend's sign.
pub(crate) fn wrap_degrees_f64(degrees: f64) -> f64 {
    let mut wrapped = degrees % 360.0;
    if wrapped >= 180.0 {
        wrapped -= 360.0;
    }
    if wrapped < -180.0 {
        wrapped += 360.0;
    }
    wrapped
}
