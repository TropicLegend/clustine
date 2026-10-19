//! A mob's mind and its controls: the goal and target selectors, each goal's state, the
//! move, look and jump controls, sensing, and the navigation and its path (`Mob.
//! serverAiStep`; the brief's sections 2 and 3). The brain (N7j) comes beside.
//!
//! What a goal keeps is its slot's part of the mind; what the controls and the
//! navigation keep is the body's (copies carry it, so that another region reads a mob's
//! path as N10's doors will): the controls' wanted states, the path whole as it was made,
//! and the progress along it with the navigation's timers apart, so that a mob walking
//! a path writes only its progress. The type's own code says, each tick, which goals it
//! has and how it is built ([`Setup`], `MobType::register_goals`), as the game's
//! constructor does once: nothing of that is kept.
//!
//! Counters the game counts by one every tick are kept as stamps against `lived`
//! (`docs/groundwork/n7-plan.md`, section 11.2): the navigation's tick is the mob's AI
//! steps, which are its ticks while it has AI; the game time its navigation and its
//! melee attack measure by is taken as the mob's `lived`, which both regions of a line
//! agree on (the brief's 11.3, named: ticks a mob spends unticked do not count).

// What the types add from N7e on calls on these; until a type registers goals, only the
// tests do.
#[allow(
    dead_code,
    reason = "the controls the goals and the types of N7e on drive"
)]
pub(crate) mod controls;
#[allow(dead_code, reason = "the generic goals the types of N7e on register")]
pub(crate) mod goals;
#[allow(
    dead_code,
    reason = "the arithmetic of the goals the types of N7e on register"
)]
pub(crate) mod maths;
#[allow(
    dead_code,
    reason = "what the goals the types of N7e on register ask of it"
)]
pub(crate) mod navigation;
#[allow(
    dead_code,
    reason = "what the navigations of the types of N7e on search with"
)]
pub(crate) mod pathfinder;
#[allow(dead_code, reason = "the positions the goals the types of N7e on draw")]
pub(crate) mod random_pos;
pub(crate) mod selector;
#[allow(dead_code, reason = "what the goals the types of N7e on sense")]
pub(crate) mod sensing;

use clustine_data::{ATTRIBUTES, DEFAULT_ATTRIBUTES, PathType};
use clustine_world::{BlockPos, Entry};
use serde::{Deserialize, Serialize};

use super::part::{self, Part, PartError, body_ids, mind_ids};
use super::tick::MobTick;
use crate::physics::{self, Vec3};
use pathfinder::FoundPath;
use selector::{Goal, Goals, Held, Registered};

/// What the move control is doing (`MoveControl.Operation`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Operation {
    #[default]
    Wait,
    MoveTo,
    Strafe,
    Jumping,
}

/// `MoveControl`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct MoveState {
    pub(crate) wanted: Vec3,
    pub(crate) speed: f64,
    pub(crate) strafe_forwards: f32,
    pub(crate) strafe_right: f32,
    pub(crate) operation: Operation,
}

/// `LookControl`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct LookState {
    pub(crate) wanted: Vec3,
    pub(crate) y_max_rot_speed: f32,
    pub(crate) x_max_rot_angle: f32,
    /// `lookAtCooldown`.
    pub(crate) cooldown: i32,
}

/// The move, look and jump controls' wanted states (`MoveControl`, `LookControl`,
/// `JumpControl`), the body's rotation's last steady head, and what goals and
/// evaluators changed of the mob's maluses: a part of the body, which copies carry.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Controls {
    pub(crate) movement: MoveState,
    pub(crate) look: LookState,
    /// `JumpControl.jump`, set and taken in one AI step.
    pub(crate) jump: bool,
    /// `BodyRotationControl.lastStableYHeadRot`.
    pub(crate) last_stable_head_yaw: f32,
    /// `Mob.pathfindingMalus` as goals and evaluators set it beyond the type's own, by
    /// the type's ordinal, ascending.
    pub(crate) malus: Vec<(u8, f32)>,
}

impl Part for Controls {
    const ID: u16 = body_ids::CONTROLS;
    const VERSION: u8 = 1;
}

/// The path a mob follows, whole as it was made (`PathNavigation.path`), with the
/// trims made to it since (a cauldron's): a part of the body, which copies carry.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Path {
    pub(crate) followed: Option<FoundPath>,
}

impl Part for Path {
    const ID: u16 = body_ids::PATH;
    const VERSION: u8 = 1;
}

/// How far along its path a mob is, and the navigation's timers
/// (`PathNavigation`'s fields, the brief's 3.20).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Progress {
    /// `nextNodeIndex`.
    pub(crate) next: u32,
    /// `speedModifier`.
    pub(crate) speed: f64,
    /// `lastStuckCheck`, in the navigation's ticks.
    pub(crate) stuck_check_at: u32,
    /// `lastStuckCheckPos`.
    pub(crate) stuck_position: Vec3,
    /// `isStuck`.
    pub(crate) stuck: bool,
    /// `timeoutCachedNode`.
    pub(crate) timeout_node: [i32; 3],
    /// `timeoutTimer`, in game ticks summed.
    pub(crate) timeout_timer: i64,
    /// `lastTimeoutCheck`.
    pub(crate) last_timeout_check: i64,
    /// `timeoutLimit`.
    pub(crate) timeout_limit: f64,
    /// `hasDelayedRecomputation`.
    pub(crate) delayed_recompute: bool,
    /// `timeLastRecompute`, none before the first.
    pub(crate) last_recompute: Option<u32>,
    /// `targetPos` and `reachRange` of the last path made.
    pub(crate) target: Option<BlockPos>,
    pub(crate) reach: i32,
}

impl Part for Progress {
    const ID: u16 = body_ids::PROGRESS;
    const VERSION: u8 = 1;
}

/// A selector's state (`GoalSelector`): its running goals by their index among its
/// goals, the goal holding each flag by ordinal (one not running holds it only until
/// the next full tick's clean-up, as the game's do), and the flags disabled.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Selector {
    pub(crate) running: Vec<u16>,
    pub(crate) locks: [Option<u16>; 4],
    pub(crate) disabled: u8,
}

/// The goal selector's.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct GoalSelector(pub(crate) Selector);

impl Part for GoalSelector {
    const ID: u16 = mind_ids::GOAL_SELECTOR;
    const VERSION: u8 = 1;
}

/// The target selector's.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct TargetSelector(pub(crate) Selector);

impl Part for TargetSelector {
    const ID: u16 = mind_ids::TARGET_SELECTOR;
    const VERSION: u8 = 1;
}

/// A mob's mind: the selectors, and the goals' own parts by slot, which their goals'
/// code reads and writes (the slot's part is `mind_ids::FIRST_GOAL` plus its index).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Mind {
    pub(crate) goals: GoalSelector,
    pub(crate) targets: TargetSelector,
    /// The goals' parts, ascending by id, each in its goal's encoding.
    pub(crate) slots: Vec<Entry>,
}

impl Mind {
    pub(crate) fn decode(entries: &[Entry]) -> Result<Self, PartError> {
        Ok(Self {
            goals: part::decode(entries)?,
            targets: part::decode(entries)?,
            slots: entries
                .iter()
                .filter(|entry| entry.id >= mind_ids::FIRST_GOAL)
                .cloned()
                .collect(),
        })
    }

    /// The mind's entries, ascending by id.
    pub(crate) fn encode(&self) -> Vec<Entry> {
        let mut entries = Vec::new();
        part::put(&mut entries, &self.goals);
        part::put(&mut entries, &self.targets);
        entries.extend(self.slots.iter().cloned());
        entries
    }
}

/// How far a mob turns its head (`getMaxHeadXRot`, `getMaxHeadYRot`, `getHeadRotSpeed`),
/// which types change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct HeadLimits {
    pub(crate) max_x_rot: i32,
    pub(crate) max_y_rot: i32,
    pub(crate) rot_speed: i32,
}

impl Default for HeadLimits {
    /// `Mob`'s: 40, 75 and 10.
    fn default() -> Self {
        Self {
            max_x_rot: 40,
            max_y_rot: 75,
            rot_speed: 10,
        }
    }
}

/// Which move control a type has (`Mob.moveControl` as its constructor makes it).
#[allow(
    dead_code,
    reason = "the flyers' and swimmers' controls, which their parts choose"
)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) enum MoveKind {
    #[default]
    Plain,
    /// `FlyingMoveControl(mob, maxTurn, hoversInPlace)`.
    Flying { max_turn: i32, hovers: bool },
    /// `SmoothSwimmingMoveControl(mob, maxTurnX, maxTurnY, inWater, outsideWater,
    /// applyGravity)`.
    SmoothSwimming {
        max_turn_x: i32,
        max_turn_y: i32,
        in_water: f32,
        outside_water: f32,
        gravity: bool,
    },
}

/// Which look control a type has.
#[allow(dead_code, reason = "the swimmers' control, which their parts choose")]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) enum LookKind {
    #[default]
    Plain,
    /// `SmoothSwimmingLookControl(mob, maxYRotFromCenter)`.
    SmoothSwimming { max_y_rot_from_center: i32 },
}

/// What a type's constructor sets of the AI beside its goals: its navigation, its
/// maluses, its controls.
#[derive(Debug, Clone, Default)]
pub(crate) struct Config {
    pub(crate) navigation: navigation::Settings,
    /// The maluses its constructor puts, by the type's ordinal.
    pub(crate) malus: [Option<f32>; 27],
    pub(crate) move_kind: MoveKind,
    pub(crate) look_kind: LookKind,
}

/// A mob's AI as its type builds it in each tick (`registerGoals`, `createNavigation`
/// and the constructor's settings): its goals and target goals in the order added, and
/// the rest of its [`Config`].
#[derive(Default)]
pub(crate) struct Setup {
    pub(crate) goals: Vec<Registered>,
    pub(crate) targets: Vec<Registered>,
    pub(crate) config: Config,
}

#[allow(dead_code, reason = "what the types of N7e on register their goals by")]
impl Setup {
    /// `goalSelector.addGoal(priority, goal)`.
    pub(crate) fn add_goal<G: Goal>(&mut self, priority: i32, goal: G) {
        goal.on_register(&mut self.config);
        self.goals.push(Registered {
            priority,
            slot: Box::new(Held {
                goal,
                state: G::State::default(),
            }),
        });
    }

    /// `targetSelector.addGoal(priority, goal)`.
    pub(crate) fn add_target<G: Goal>(&mut self, priority: i32, goal: G) {
        goal.on_register(&mut self.config);
        self.targets.push(Registered {
            priority,
            slot: Box::new(Held {
                goal,
                state: G::State::default(),
            }),
        });
    }

    /// `setPathfindingMalus(type, malus)` in the constructor.
    pub(crate) fn set_malus(&mut self, kind: PathType, malus: f32) {
        self.config.malus[kind as usize] = Some(malus);
    }
}

/// A mob's AI step: its tick, its type's configuration and what it has sensed in it.
pub(crate) struct Ai<'t, 'w> {
    pub(crate) tick: &'t mut MobTick<'w>,
    pub(crate) config: &'t Config,
    pub(crate) sensing: sensing::Sensing,
    /// Whether the navigation has ticked in this step, after which the navigation's tick
    /// counts this step.
    pub(crate) navigation_ticked: bool,
}

impl Ai<'_, '_> {
    /// Its feet.
    pub(crate) fn position(&self) -> Vec3 {
        self.tick.living.body.position
    }

    /// `getEyeY`.
    pub(crate) fn eye_y(&self) -> f64 {
        self.position()[1] + f64::from(self.tick.living.eye_height)
    }

    /// `blockPosition`.
    pub(crate) fn block_position(&self) -> BlockPos {
        physics::containing(self.position())
    }

    pub(crate) fn width(&self) -> f32 {
        self.tick.living.body.width
    }

    pub(crate) fn height(&self) -> f32 {
        self.tick.living.body.height
    }

    pub(crate) fn on_ground(&self) -> bool {
        self.tick.living.body.on_ground
    }

    /// `isInWater`.
    pub(crate) fn in_water(&self) -> bool {
        self.tick.living.fluids.in_water()
    }

    /// `isInLava`.
    pub(crate) fn in_lava(&self) -> bool {
        !self.tick.living.first_tick && self.tick.living.fluids.in_lava()
    }

    /// `isInLiquid`.
    pub(crate) fn in_liquid(&self) -> bool {
        self.in_water() || self.in_lava()
    }

    /// `getAttributeValue(attribute)`, by its id.
    pub(crate) fn attribute(&self, attribute: i32) -> f64 {
        attribute_value(self.tick, attribute)
    }

    /// `getPathfindingMalus(type)`.
    pub(crate) fn malus(&self, kind: PathType) -> f32 {
        malus_of(&self.tick.mob.body.controls, self.config, kind)
    }

    /// `setPathfindingMalus(type, malus)` by a goal or an evaluator.
    pub(crate) fn set_malus(&mut self, kind: PathType, malus: f32) {
        let own = self.config.malus[kind as usize].unwrap_or(kind.malus());
        let overrides = &mut self.tick.mob.body.controls.malus;
        let at = overrides.binary_search_by_key(&(kind as u8), |(held, _)| *held);
        match (at, malus.to_bits() == own.to_bits()) {
            (Ok(at), true) => {
                overrides.remove(at);
            }
            (Ok(at), false) => overrides[at].1 = malus,
            (Err(at), false) => overrides.insert(at, (kind as u8, malus)),
            (Err(_), true) => {}
        }
    }

    /// The mob's maluses, every type.
    pub(crate) fn maluses(&self) -> pathfinder::Malus {
        std::array::from_fn(|ordinal| self.malus(PathType::ALL[ordinal]))
    }

    /// The game's `tickCount` in this tick: one more than the ticks before it.
    pub(crate) fn tick_count(&self) -> u32 {
        self.tick.tick_count.saturating_add(1)
    }

    /// The game time the AI measures by: the mob's `lived` (see the module's comment).
    pub(crate) fn now(&self) -> i64 {
        i64::from(self.tick.lived)
    }

    /// `getId()`: the key's low 32 bits (ADR-0028, section 1).
    pub(crate) fn id(&self) -> i32 {
        self.tick.key.0 as u32 as i32
    }

    /// `hasHome`, its centre and radius.
    pub(crate) fn home(&self) -> Option<(BlockPos, i32)> {
        let home = &self.tick.mob.body.home;
        home.centre
            .filter(|_| home.radius != -1)
            .map(|centre| (centre, home.radius))
    }

    /// `isWithinHome(BlockPos)`: no home, or the block's corner nearer the centre than
    /// the radius.
    pub(crate) fn within_home(&self, pos: BlockPos) -> bool {
        match self.home() {
            None => true,
            Some((centre, radius)) => {
                let (dx, dy, dz) = (
                    f64::from(centre.x - pos.x),
                    f64::from(centre.y - pos.y),
                    f64::from(centre.z - pos.z),
                );
                dx * dx + dy * dy + dz * dz < f64::from(radius.wrapping_mul(radius))
            }
        }
    }

    /// The mob's head limits (its type's).
    pub(crate) fn head(&self) -> HeadLimits {
        self.tick.mob_type.head()
    }
}

/// `getAttributeValue(attribute)` of the mob in `tick`: its instance's value where it has
/// made one, else its type's default, kept within the attribute's bounds.
pub(crate) fn attribute_value(tick: &MobTick<'_>, attribute: i32) -> f64 {
    let Some(info) = usize::try_from(attribute)
        .ok()
        .and_then(|at| ATTRIBUTES.get(at))
    else {
        return 0.0;
    };
    let instance = tick
        .mob
        .body
        .attributes
        .instances
        .iter()
        .find(|instance| i32::from(instance.attribute) == attribute);
    let Some(instance) = instance else {
        return default_attribute(tick.kind, attribute).clamp(info.min, info.max);
    };
    // `AttributeInstance.calculateValue`: what is added, then what multiplies the base,
    // then what multiplies the total.
    let mut base = instance.base;
    for modifier in &instance.modifiers {
        if modifier.operation == 0 {
            base += modifier.amount;
        }
    }
    let mut value = base;
    for modifier in &instance.modifiers {
        if modifier.operation == 1 {
            value += base * modifier.amount;
        }
    }
    for modifier in &instance.modifiers {
        if modifier.operation == 2 {
            value *= 1.0 + modifier.amount;
        }
    }
    if value.is_nan() {
        info.min
    } else {
        value.clamp(info.min, info.max)
    }
}

/// `getAttributeBaseValue(attribute)`.
pub(crate) fn attribute_base(tick: &MobTick<'_>, attribute: i32) -> f64 {
    tick.mob
        .body
        .attributes
        .instances
        .iter()
        .find(|instance| i32::from(instance.attribute) == attribute)
        .map_or_else(
            || default_attribute(tick.kind, attribute),
            |instance| instance.base,
        )
}

/// The type's default of the attribute (`DefaultAttributes`), else the attribute's own.
fn default_attribute(kind: u16, attribute: i32) -> f64 {
    DEFAULT_ATTRIBUTES
        .get(usize::from(kind))
        .copied()
        .flatten()
        .and_then(|values| values.iter().find(|(id, _)| i32::from(*id) == attribute))
        .map(|(_, value)| *value)
        .or_else(|| {
            usize::try_from(attribute)
                .ok()
                .and_then(|at| ATTRIBUTES.get(at))
                .map(|info| info.default)
        })
        .unwrap_or(0.0)
}

/// `getPathfindingMalus(type)`: what goals set, else what the type set, else the type's
/// own.
fn malus_of(controls: &Controls, config: &Config, kind: PathType) -> f32 {
    controls
        .malus
        .binary_search_by_key(&(kind as u8), |(held, _)| *held)
        .map(|at| controls.malus[at].1)
        .ok()
        .or(config.malus[kind as usize])
        .unwrap_or(kind.malus())
}

/// `navigation.canFloat()`, which the type's goals set as they are made (`FloatGoal`'s):
/// what the body's jump in a fluid reads.
pub(crate) fn can_float(tick: &MobTick<'_>) -> bool {
    let mut setup = Setup::default();
    tick.mob_type.register_goals(&mut setup);
    setup.config.navigation.flags.can_float
}

/// The mob's two selectors in its tick, with their goals' states read from its mind.
fn selectors(tick: &MobTick<'_>) -> (Goals, Goals, Config) {
    let mut setup = Setup::default();
    tick.mob_type.register_goals(&mut setup);
    let first_target = mind_ids::FIRST_GOAL + setup.goals.len() as u16;
    let mut goals = Goals {
        slots: setup.goals,
        state: tick.mob.mind.goals.0.clone(),
    };
    goals.load(&tick.mob.mind.slots, mind_ids::FIRST_GOAL);
    let mut targets = Goals {
        slots: setup.targets,
        state: tick.mob.mind.targets.0.clone(),
    };
    targets.load(&tick.mob.mind.slots, first_target);
    (goals, targets, setup.config)
}

/// `PathfinderMob.isPanicking` of a mob without a brain: whether a panic goal runs.
#[allow(dead_code, reason = "what breeding and the leash ask, N7b's and N7e's")]
pub(crate) fn is_panicking(tick: &MobTick<'_>) -> bool {
    selectors(tick).0.panicking()
}

/// `Mob.serverAiStep`: sensing cleared, the target selector, the goal selector, the
/// navigation, the type's own step (`customServerAiStep`, [`super::types`]), then the
/// move, look and jump controls, in the game's order (the brief's 2.1). `noActionTime`'s
/// step is its stamp's (the body's), and needs nothing here.
pub(crate) fn server_ai_step(tick: &mut MobTick<'_>) {
    let (mut goals, mut targets, config) = selectors(tick);
    {
        let mut ai = Ai {
            tick,
            config: &config,
            sensing: sensing::Sensing::default(),
            navigation_ticked: false,
        };
        // `(tickCount + getId()) % 2 == 0`, Java's remainder taking the dividend's sign:
        // the sum is even.
        let tick_count = ai.tick_count();
        let full = (tick_count as i32).wrapping_add(ai.id()) & 1 == 0 || tick_count <= 1;
        if full {
            targets.tick(&mut ai);
            goals.tick(&mut ai);
        } else {
            targets.tick_running(false, &mut ai);
            goals.tick_running(false, &mut ai);
        }
        navigation::tick(&mut ai);
        ai.navigation_ticked = true;
    }
    tick.mob_type.custom_server_ai_step(tick);
    {
        let mut ai = Ai {
            tick,
            config: &config,
            sensing: sensing::Sensing::default(),
            navigation_ticked: true,
        };
        controls::tick_move(&mut ai);
        controls::tick_look(&mut ai);
        controls::tick_jump(&mut ai);
    }
    let mut slots = Vec::new();
    goals.save(mind_ids::FIRST_GOAL, &mut slots);
    targets.save(mind_ids::FIRST_GOAL + goals.slots.len() as u16, &mut slots);
    tick.mob.mind.slots = slots;
    tick.mob.mind.goals.0 = goals.state;
    tick.mob.mind.targets.0 = targets.state;
}

/// The body's rotation after `aiStep` (`Mob.tickHeadTurn`, which runs
/// `BodyRotationControl.clientTick` on the server too), then, every five ticks,
/// `updateControlFlags`.
pub(crate) fn rotate_body(tick: &mut MobTick<'_>) {
    controls::rotate_body(tick);
    if tick.tick_count.saturating_add(1).is_multiple_of(5) {
        // `updateControlFlags`. **Named**: the region does not yet tell a mob's tick
        // what it rides or carries, so no mob is taken as controlled by a mob it
        // carries or as riding a boat, and every flag stays enabled.
        update_control_flags(&mut tick.mob.mind.goals.0, false, false);
    }
}

/// `Mob.updateControlFlags`: movement and looking are a carried mob's when it controls
/// this one, and jumping too, or a boat's.
pub(crate) fn update_control_flags(selector: &mut Selector, mob_controls: bool, in_boat: bool) {
    let not_mob = !mob_controls;
    let not_boat = !in_boat;
    let mut disabled = 0;
    if !not_mob {
        disabled |= selector::flags::MOVE | selector::flags::LOOK;
    }
    if !(not_mob && not_boat) {
        disabled |= selector::flags::JUMP;
    }
    selector.disabled = disabled;
}

#[cfg(test)]
mod tests;
