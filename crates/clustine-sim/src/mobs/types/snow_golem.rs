//! The snow golem (`SnowGolem` and `AbstractGolem`; `docs/groundwork/n7k-brief.md`,
//! sections 1.1 and 1.4; the plan's sections 1.1 and 10): built by a player or made by
//! an egg, it leaves a layer of snow wherever it walks and a layer can stand, melts in
//! the warm biomes, is hurt by water, strolls avoiding water, looks at players, throws
//! snowballs at any `Enemy` within 16 it sees, and loses its pumpkin to shears.
//!
//! At a line: a cell of its trail in another region's land is set by an effect, only
//! if it is still air, after its own region found on its copy that a layer can stand
//! there; that placement's `block_place` game event is not made (named, the plan's
//! section 10).
//!
//! Named differences: Clustine has no Nether, whose dimension melts it too, and no
//! rain, which would hurt it; the game rule `mob_griefing` is on, as everywhere in
//! Clustine; the shears a player uses are not worn, every player being creative; its
//! leash's offset is the client's (`getLeashOffset`, which nothing on the server
//! reads).

use clustine_data::{BlockState, blocks, items, sounds};
use clustine_world::{BlockPos, EntityId, ItemStack, TypeState};
use serde::{Deserialize, Serialize};

use super::iron_golem::is_enemy;
use super::{Did, Interaction, MobType, Used, template};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    LookAtPlayerGoal, NearestAttackableTargetGoal, RandomLookAroundGoal, RandomStrollGoal, Stroll,
    look_at_thing,
};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::{self, Class};
use crate::mobs::ai::{Ai, Setup, navigation};
use crate::mobs::body::hurt::{DamageSource, DamageType};
use crate::mobs::body::{Event, loot};
use crate::mobs::tick::{Local, MobTick};
use crate::physics::thrown::{GOLEM_POWER, GOLEM_UNCERTAINTY, golem_aim};
use crate::physics::{self, Rules};
use crate::shown::DataShown;

/// The synced data's index of its pumpkin (`DATA_PUMPKIN_ID`).
const PUMPKIN_INDEX: u8 = 16;

/// The pumpkin's bit (`PUMPKIN_FLAG`).
const PUMPKIN_FLAG: u8 = 0x10;

/// The version of the snow golem's type state.
const VERSION: u8 = 1;

/// The biomes whose `gameplay/snow_golem_melts` is true in 26.3's data, sorted (the
/// Nether's dimension type sets it too).
pub(crate) const MELTS_IN: &[&str] = &[
    "minecraft:badlands",
    "minecraft:desert",
    "minecraft:eroded_badlands",
    "minecraft:savanna",
    "minecraft:savanna_plateau",
    "minecraft:windswept_savanna",
    "minecraft:wooded_badlands",
];

/// What a snow golem keeps: whether its pumpkin was shorn off (saved as `Pumpkin`, the
/// other way round, so that a fresh golem's state is nothing).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct SnowGolemState {
    pub(crate) sheared: bool,
}

/// The snow golem's state of `mob`.
pub(crate) fn state_of(mob: &Mob) -> SnowGolemState {
    read(&mob.state)
}

/// The snow golem's state of a type state: a fresh golem's for nothing or what this
/// build cannot read.
pub(crate) fn read(state: &TypeState) -> SnowGolemState {
    if state.bytes.is_empty() || state.version != VERSION {
        return SnowGolemState::default();
    }
    postcard::from_bytes(&state.bytes).unwrap_or_default()
}

/// The type state of `own`: nothing for a fresh golem's.
pub(crate) fn write(own: &SnowGolemState) -> TypeState {
    if *own == SnowGolemState::default() {
        return TypeState::default();
    }
    TypeState {
        version: VERSION,
        bytes: postcard::to_allocvec(own).expect("a type state is encoded into memory"),
    }
}

/// `hasPumpkin()`.
pub(crate) fn has_pumpkin(mob: &Mob) -> bool {
    !state_of(mob).sheared
}

/// `setPumpkin(pumpkin)`.
pub(crate) fn set_pumpkin(mob: &mut Mob, pumpkin: bool) {
    mob.state = write(&SnowGolemState { sheared: !pumpkin });
}

/// Whether the environment attribute `gameplay/snow_golem_melts` holds in the biome
/// named `biome`.
pub(crate) fn melts_in(biome: Option<&str>) -> bool {
    biome.is_some_and(|name| MELTS_IN.binary_search(&name).is_ok())
}

/// The cells of its trail at `feet`, in its order (`i` from 0 to 3): a quarter of a
/// block either way in x, then in z, of its feet's x and z, at its feet's height.
pub(crate) fn trail_cells(feet: physics::Vec3) -> [BlockPos; 4] {
    std::array::from_fn(|i| {
        let i = i as i32;
        let dx = f64::from(((i % 2) * 2 - 1) as f32 * 0.25);
        let dz = f64::from(((i / 2 % 2) * 2 - 1) as f32 * 0.25);
        BlockPos::new(
            physics::floor(feet[0] + dx),
            physics::floor(feet[1]),
            physics::floor(feet[2] + dz),
        )
    })
}

/// `RangedAttackGoal`'s state: its target, the ticks to its next shot (`attackTime`,
/// −1 between targets) and how long it has seen the target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Ranging {
    pub(crate) target: Option<EntityId>,
    pub(crate) attack_time: i32,
    pub(crate) see_time: i32,
}

impl Default for Ranging {
    fn default() -> Self {
        Self {
            target: None,
            attack_time: -1,
            see_time: 0,
        }
    }
}

/// `RangedAttackGoal(mob, speed, intervalMin, intervalMax, radius)`: to a target it
/// walks until within the radius and seen for five ticks, looking at it, and shoots
/// whenever its interval is up and it sees it (`performRangedAttack`, here the snow
/// golem's).
#[derive(Debug, Clone, Copy)]
pub(crate) struct RangedAttackGoal {
    pub(crate) speed: f64,
    pub(crate) interval_min: i32,
    pub(crate) interval_max: i32,
    pub(crate) radius: f32,
}

impl Goal for RangedAttackGoal {
    type State = Ranging;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, state: &mut Ranging, ai: &mut Ai<'_, '_>) -> bool {
        let Some(target) = sensing::target(ai).filter(|target| target.alive) else {
            return false;
        };
        state.target = Some(target.key);
        true
    }

    fn can_continue(&self, state: &mut Ranging, ai: &mut Ai<'_, '_>) -> bool {
        if self.can_use(state, ai) {
            return true;
        }
        let alive = state
            .target
            .and_then(|key| sensing::find(ai, key))
            .is_some_and(|target| target.alive);
        alive && !navigation::is_done(ai)
    }

    fn stop(&self, state: &mut Ranging, _: &mut Ai<'_, '_>) {
        *state = Ranging::default();
    }

    fn tick(&self, state: &mut Ranging, ai: &mut Ai<'_, '_>) {
        let Some(target) = state.target.and_then(|key| sensing::find(ai, key)) else {
            return;
        };
        let distance = target.distance_sqr(ai.position());
        let sees = sensing::has_line_of_sight(ai, &target);
        if sees {
            state.see_time += 1;
        } else {
            state.see_time = 0;
        }
        if distance <= f64::from(self.radius * self.radius) && state.see_time >= 5 {
            navigation::stop(ai);
        } else {
            navigation::move_to_entity(ai, target.block_position(), self.speed);
        }
        look_at_thing(ai, &target, 30.0, 30.0);
        state.attack_time -= 1;
        if state.attack_time == 0 {
            if !sees {
                return;
            }
            let fraction = distance.sqrt() as f32 / self.radius;
            let power = fraction.clamp(0.1, 1.0);
            perform_ranged_attack(ai, &target, power);
            state.attack_time = (fraction * (self.interval_max - self.interval_min) as f32
                + self.interval_min as f32)
                .floor() as i32;
        } else if state.attack_time < 0 {
            let t = distance.sqrt() / f64::from(self.radius);
            let a = f64::from(self.interval_min);
            let b = f64::from(self.interval_max);
            state.attack_time = (a + t * (b - a)).floor() as i32;
        }
    }
}

/// `SnowGolem.performRangedAttack(target, power)`: a snowball from a tenth below its
/// eyes, shot at the target's eyes less 1.1 and lifted by a fifth of the way across, at
/// 1.6 and an uncertainty of 12 (`region/thrown.rs` makes it); then its shoot sound, at
/// a pitch of its own draw.
fn perform_ranged_attack(ai: &mut Ai<'_, '_>, target: &sensing::Looked, _power: f32) {
    let [at, direction] = golem_aim(ai.position(), ai.eye_y(), target.position, target.eye_y);
    ai.tick.out.local.push(Local::Shoot {
        stack: ItemStack {
            item: items::SNOWBALL,
            count: 1,
        },
        at,
        direction,
        power: GOLEM_POWER,
        uncertainty: GOLEM_UNCERTAINTY,
    });
    let pitch = 0.4 / (ai.tick.draws.next_float() * 0.4 + 0.8);
    ai.tick.out.events.push(Event::Sound {
        sound: sounds::ENTITY_SNOW_GOLEM_SHOOT,
        volume: 1.0,
        pitch,
    });
}

/// The snow golem.
pub(crate) struct SnowGolem;

/// `SnowGolem.aiStep` after `AbstractGolem`'s, with `LivingEntity.aiStep`'s hurt of a
/// body sensitive to water before it: in water, `drown` 1; in a biome where it melts,
/// `onFire` 1; then a layer of snow at each of its four corners that is air and where a
/// layer can stand, each cell once.
fn after_ai_step(tick: &mut MobTick<'_>) {
    if tick.living.fluids.in_water() {
        crate::mobs::body::hurt_by(tick, &DamageSource::of(DamageType::DROWN), 1.0);
    }
    let feet = tick.living.body.position;
    if melts_in(tick.world.biome(physics::containing(feet))) {
        crate::mobs::body::hurt_by(tick, &DamageSource::of(DamageType::ON_FIRE), 1.0);
    }
    let snow: BlockState = blocks::SNOW;
    let mut placed: Vec<BlockPos> = Vec::with_capacity(4);
    for pos in trail_cells(feet) {
        // A cell placed at an earlier corner is snow now, no longer air.
        if placed.contains(&pos) {
            continue;
        }
        let air = tick.world.block(pos).is_some_and(|state| state.is_air());
        if air && clustine_blocks::can_survive(snow, pos, &mut Rules(tick.world)) {
            tick.out.local.push(Local::PlaceBlock { pos, state: snow });
            placed.push(pos);
        }
    }
}

/// `SnowGolem.mobInteract`: shears take off a pumpkin it has (`shear`: its sound, the
/// pumpkin gone, its shearing table's drops at its eyes), with `GameEvent.SHEAR` of the
/// player; anything else passes.
fn mob_interact(used: &mut Used<'_>) -> Interaction {
    let shears = used
        .held
        .is_some_and(|stack| stack.count > 0 && stack.item == items::SHEARS);
    if !shears || !has_pumpkin(used.mob) {
        return Interaction::Pass;
    }
    used.did.push(Did::Sound {
        sound: sounds::ENTITY_SNOW_GOLEM_SHEAR,
        source: crate::mobs::types::animal::SOUND_SOURCE_PLAYERS,
        at: used.position,
        volume: 1.0,
        pitch: 1.0,
        but_the_player: false,
    });
    set_pumpkin(used.mob, false);
    let drops = loot::named_drops(
        "minecraft:shearing/snow_golem",
        used.kind,
        false,
        loot::Facts::default(),
        used.draws,
    );
    let eyes = clustine_data::EntityTypeInfo::of(i32::from(used.kind)).eye_height;
    let [x, y, z] = used.position;
    for stack in drops {
        used.did.push(Did::Drop {
            stack,
            at: [x, y + f64::from(eyes), z],
            pick_up_delay: crate::region::DEFAULT_PICK_UP_DELAY,
            push: None,
        });
    }
    used.did.push(Did::GameEvent("minecraft:shear"));
    Interaction::Success
}

impl MobType for SnowGolem {
    fn register_goals(&self, setup: &mut Setup) {
        setup.add_goal(
            1,
            RangedAttackGoal {
                speed: 1.25,
                interval_min: 20,
                interval_max: 20,
                radius: 10.0,
            },
        );
        setup.add_goal(
            2,
            RandomStrollGoal {
                kind: Stroll::AvoidingWater {
                    probability: 1.000_000_1e-5,
                },
                ..RandomStrollGoal::new(1.0)
            },
        );
        setup.add_goal(3, LookAtPlayerGoal::new(Class::Player, 6.0));
        setup.add_goal(4, RandomLookAroundGoal);
        setup.add_target(
            1,
            NearestAttackableTargetGoal {
                class: Class::Mob,
                interval: 10,
                must_see: true,
                must_reach: false,
                selector: Some(is_enemy),
            },
        );
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        after_ai_step(tick);
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        mob_interact(used)
    }

    fn ambient_interval(&self) -> u32 {
        120
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SNOW_GOLEM_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_SNOW_GOLEM_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_SNOW_GOLEM_DEATH)
    }

    /// `readAdditionalSaveData`: `Pumpkin`, true where it is not named.
    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        set_pumpkin(mob, template::boolean(tag, "Pumpkin").unwrap_or(true));
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let pumpkin = has_pumpkin(mob);
        let value = if pumpkin { PUMPKIN_FLAG } else { 0 };
        vec![(PUMPKIN_INDEX, DataShown::Byte(value), pumpkin)]
    }
}
