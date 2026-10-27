//! Goats (`Goat`; `docs/groundwork/n7-brief.md`, section 7.6): an animal of wheat that is
//! milked with a bucket at every click, one in fifty of them screaming (its own sounds,
//! its rams sooner, its horns of other instruments), one adult in ten of an egg with a
//! horn missing; a fall hurts it ten less than another animal.
//!
//! Its mind is a brain (`GoatAi`, [`PROVIDER`]): it wanders, follows wheat and adults,
//! breeds, panics when hurt, long-jumps up to five blocks once its jump's cooldown is out,
//! and rams a mob or an armour stand four to seven blocks away once its ram's is, never a
//! creative player. Its two cooldowns are integer memories its core counts down, which
//! `initMemories` sets at a spawn and a breeding ([`brain::remember`]); a goat that never
//! had them (one of no spawn) jumps and rams at once, as the game's does. The pose and the
//! friction of a jump ([`GoatState::long_jumping`], [`GoatState::discards_friction`]) are
//! its type state, which copies carry, so that a jump over a line arrives in its pose.
//!
//! What reads across a line (the plan, section 3): the senses are the shared sensors'
//! (the copies' entities, the faces' players); a long jump's candidates and arc read the
//! copies' blocks and take what is beyond them for an unloaded chunk's air, so that a
//! goat near the copies' edge plans shorter jumps; a jump that lands in another region
//! arrives with its record, its brain and its pose whole; a ram's hit on a mob only a
//! copy shows is its holder's (`Local::Hurt`, then `EntityUse::MobHit`); a target the
//! view no longer has is not taken for dead (the shared ram's reading, plan section 8
//! item 8).
//!
//! Named differences: the base of a kid's attack damage (1, `ageBoundaryReached`) is set
//! at its first tick as a kid and again at its first as an adult, where the game sets it
//! as the age crosses the boundary (only a ram reads it, in the goat's own tick);
//! `setYHeadRot`'s clamp of the head to 15 degrees of the body
//! is not made (the look control, which turns the head, sets the field without it, as the
//! game's does; the other callers are a spawn's and a teleport's); a horn drops as a
//! plain goat horn without its instrument (the owner's question 4 of the plan's 11.7),
//! though [`brain::horn_instrument`] names the one the game would give; a jump's discarded
//! friction is taken as the friction of nothing, which counts the friction of the block
//! under it in the speed of its input too, an input no goat has in a jump.

use clustine_data::{PathType, items, sounds};
use serde::{Deserialize, Serialize};

use super::animal::{self, Own};
use super::{Breeding, Did, Dimensions, Interaction, MobType, Spawning, Used};
use crate::living::Landing;
use crate::mobs::Mob;
use crate::mobs::ai::goals::Items;
use crate::mobs::ai::sensing::Looked;
use crate::mobs::ai::{Ai, HeadLimits, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::brain::behaviours::jumping::{Leaper, LongJumpMidJump, LongJumpToRandomPos};
use crate::mobs::brain::behaviours::looking::{self, LookAt};
use crate::mobs::brain::behaviours::ramming::{PrepareRamNearestTarget, RamTarget, Rammer};
use crate::mobs::brain::behaviours::social::{self, AnimalMakeLove, FollowTemptation};
use crate::mobs::brain::behaviours::{Speed, cooldowns, panic, walking};
use crate::mobs::brain::sensors::{self, NearestItemSensor, TemptingSensor};
use crate::mobs::brain::{
    self as machinery, Activity, ActivityData, Cx, DoNothing, Gate, HeldTimed, MemoryKind,
    Provider, Status, Value,
};
use crate::mobs::tick::{Local, MobTick};
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::region::Random;
use crate::shown::DataShown;
use clustine_world::BlockPos;

/// `#goat_food`.
pub(crate) const FOOD: Items = Items::Tag("minecraft:goat_food");

/// A kid's size (`BABY_DIMENSIONS`).
pub(crate) const BABY: Dimensions = Dimensions {
    width: 0.45,
    height: 0.65,
    eye_height: 0.593_75,
};

/// The synced data's indices: the pose (`Entity.DATA_POSE`), screaming, the left horn,
/// the right horn.
const POSE_INDEX: u8 = 6;
const SCREAMING_INDEX: u8 = 18;
const LEFT_HORN_INDEX: u8 = 19;
const RIGHT_HORN_INDEX: u8 = 20;

/// How much less a fall hurts a goat (`calculateFallDamage`).
const FALL_LESS: i32 = 10;

/// The chance of a screaming goat at a spawn or a breeding.
const SCREAMING_CHANCE: f64 = 0.02;

/// The ordinal of `Pose.LONG_JUMPING`.
const LONG_JUMPING: i32 = 6;

/// What a goat keeps beyond an animal: whether it screams, and its two horns
/// (`IsScreamingGoat`, `HasLeftHorn`, `HasRightHorn`), and in a long jump its pose
/// (`LONG_JUMPING`) and whether it keeps its motion unslowed (`discardFriction`), which
/// the game keeps on the entity and does not save. Its cooldowns are its brain's
/// memories, not its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct GoatState {
    pub(crate) screaming: bool,
    pub(crate) left_horn: bool,
    pub(crate) right_horn: bool,
    pub(crate) long_jumping: bool,
    pub(crate) discards_friction: bool,
}

impl Default for GoatState {
    /// `defineSynchedData`'s: not screaming, both horns; standing.
    fn default() -> Self {
        Self {
            screaming: false,
            left_horn: true,
            right_horn: true,
            long_jumping: false,
            discards_friction: false,
        }
    }
}

impl Own for GoatState {}

/// The goat's own state of `mob`.
pub(crate) fn state_of(mob: &Mob) -> GoatState {
    animal::read::<GoatState>(&mob.state).1
}

/// Changes the goat's own state of `mob`.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut GoatState)) {
    let (shared, mut own) = animal::read::<GoatState>(&mob.state);
    let before = own.clone();
    change(&mut own);
    if own != before {
        mob.state = animal::write(&shared, &own);
    }
}

/// `isScreamingGoat()`.
pub(crate) fn is_screaming(mob: &Mob) -> bool {
    state_of(mob).screaming
}

/// A sound of the goat, its screaming form for a screaming goat.
fn voiced(mob: &Mob, plain: i32, screaming: i32) -> i32 {
    if is_screaming(mob) { screaming } else { plain }
}

/// `getMilkingSound()`.
pub(crate) fn milking_sound(mob: &Mob) -> i32 {
    voiced(
        mob,
        sounds::ENTITY_GOAT_MILK,
        sounds::ENTITY_GOAT_SCREAMING_MILK,
    )
}

/// `playEatingSound()`: at the goat, of the neutral source, its pitch between 0.8 and
/// 1.2 by one of the level's draws on its behalf.
fn eating_sound(used: &mut Used<'_>) {
    let sound = voiced(
        used.mob,
        sounds::ENTITY_GOAT_EAT,
        sounds::ENTITY_GOAT_SCREAMING_EAT,
    );
    // `Mth.randomBetween(random, 0.8f, 1.2f)`.
    let pitch = used.draws.next_float() * (1.2_f32 - 0.8_f32) + 0.8_f32;
    used.did.push(Did::Sound {
        sound,
        source: SOUND_SOURCE_NEUTRAL,
        at: used.position,
        volume: 1.0,
        pitch,
        but_the_player: false,
    });
}

/// The goat.
pub(crate) struct Goat;

/// What the brain reads and calls of a goat: plain functions of the game's lambdas and
/// the goat's own methods that `GoatAi`'s behaviours call.
pub(crate) mod brain {
    use super::*;

    /// `TIME_BETWEEN_LONG_JUMPS`: 600 to 1200 ticks.
    pub(crate) const TIME_BETWEEN_LONG_JUMPS: (i32, i32) = (600, 1200);
    /// `TIME_BETWEEN_RAMS` and `TIME_BETWEEN_RAMS_SCREAMER`.
    pub(crate) const TIME_BETWEEN_RAMS: (i32, i32) = (600, 6000);
    pub(crate) const TIME_BETWEEN_RAMS_SCREAMER: (i32, i32) = (100, 300);
    /// `RAM_MIN_DISTANCE`, `RAM_MAX_DISTANCE`, `RAM_PREPARE_TIME`.
    pub(crate) const RAM_DISTANCES: (i32, i32) = (4, 7);
    pub(crate) const RAM_PREPARE_TIME: i32 = 20;
    /// The long jump's most height and width and `MAX_JUMP_VELOCITY_MULTIPLIER`.
    pub(crate) const LONG_JUMP_HEIGHT: i32 = 5;
    pub(crate) const LONG_JUMP_WIDTH: i32 = 5;
    pub(crate) const MAX_JUMP_VELOCITY_MULTIPLIER: f32 = 3.571_428_8;

    /// The cooldowns `GoatAi.initMemories` gives a goat: its long jump's, then its
    /// ram's.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct Cooldowns {
        pub(crate) long_jump: i32,
        pub(crate) ram: i32,
    }

    /// `UniformInt.of(min, max).sample(random)`: `nextInt(max − min + 1) + min`.
    pub(crate) fn sample(
        (min, max): (i32, i32),
        draws: &mut dyn clustine_world::generation::Draws,
    ) -> i32 {
        draws.next_int((max - min + 1) as u32) as i32 + min
    }

    /// `GoatAi.initMemories(goat, random)`: the long jump's cooldown, then the ram's, of
    /// the plain goat's range even for a screamer (screaming is decided after).
    pub(crate) fn init_memories(draws: &mut dyn clustine_world::generation::Draws) -> Cooldowns {
        let long_jump = sample(TIME_BETWEEN_LONG_JUMPS, draws);
        let ram = sample(TIME_BETWEEN_RAMS, draws);
        Cooldowns { long_jump, ram }
    }

    /// The two cooldowns made the brain's memories (`setMemory`, no expiry), at rest
    /// until its core's countdowns start in its next tick.
    pub(crate) fn remember(mob: &mut Mob, cooldowns: Cooldowns) {
        let mut memories = machinery::memories_of(&PROVIDER, mob, 0);
        memories.set(
            MemoryKind::LongJumpCoolingDown,
            Value::Int(cooldowns.long_jump),
        );
        memories.set(MemoryKind::RamCooldownTicks, Value::Int(cooldowns.ram));
        machinery::write_memories(&PROVIDER, mob, memories);
    }

    /// `getTimeBetweenRams`: a screamer rams sooner.
    pub(crate) fn time_between_rams(mob: &Mob) -> (i32, i32) {
        if is_screaming(mob) {
            TIME_BETWEEN_RAMS_SCREAMER
        } else {
            TIME_BETWEEN_RAMS
        }
    }

    /// The cooldown `PrepareRamNearestTarget` sets where no ram can be prepared: the
    /// range's least.
    pub(crate) fn least_ram_cooldown(mob: &Mob) -> i32 {
        time_between_rams(mob).0
    }

    /// The ram's knockback: a kid's 1, an adult's 2.5.
    pub(crate) fn ram_knockback(mob: &Mob) -> f64 {
        if animal::is_baby(mob) { 1.0 } else { 2.5 }
    }

    /// `ATTACK_DAMAGE` as `ageBoundaryReached` sets its base: a kid's 1, an adult's 2.
    pub(crate) fn attack_damage(mob: &Mob) -> f64 {
        if animal::is_baby(mob) { 1.0 } else { 2.0 }
    }

    /// The sound of a ram's preparation, of its impact, of a horn breaking, of a long
    /// jump.
    pub(crate) fn prepare_ram_sound(mob: &Mob) -> i32 {
        voiced(
            mob,
            sounds::ENTITY_GOAT_PREPARE_RAM,
            sounds::ENTITY_GOAT_SCREAMING_PREPARE_RAM,
        )
    }

    pub(crate) fn ram_impact_sound(mob: &Mob) -> i32 {
        voiced(
            mob,
            sounds::ENTITY_GOAT_RAM_IMPACT,
            sounds::ENTITY_GOAT_SCREAMING_RAM_IMPACT,
        )
    }

    pub(crate) const HORN_BREAK_SOUND: i32 = sounds::ENTITY_GOAT_HORN_BREAK;

    pub(crate) fn long_jump_sound(mob: &Mob) -> i32 {
        voiced(
            mob,
            sounds::ENTITY_GOAT_LONG_JUMP,
            sounds::ENTITY_GOAT_SCREAMING_LONG_JUMP,
        )
    }

    /// `RAM_TARGET_CONDITIONS`' selector of the entity type `kind`: anything but a goat,
    /// an armour stand only where mobs may grief (always: Clustine has no game rules).
    /// The rest of its test is the combat targeting's, which never takes a creative
    /// player (the brief's finding 7.0.3.15).
    pub(crate) fn rams(kind: u16) -> bool {
        i32::from(kind) != clustine_data::entity_types::GOAT
    }

    /// `getDefaultDimensions(pose)` of a goat in its long jump's pose (`LONG_JUMPING`,
    /// which `LongJumpMidJump` sets): its size of the pose, scaled by 0.7.
    pub(crate) fn long_jumping_dimensions(mob: &Mob) -> Dimensions {
        let own = if animal::is_baby(mob) {
            BABY
        } else {
            let info = clustine_data::EntityTypeInfo::of(clustine_data::entity_types::GOAT);
            Dimensions {
                width: info.width,
                height: info.height,
                eye_height: info.eye_height,
            }
        };
        Dimensions {
            width: own.width * 0.7,
            height: own.height * 0.7,
            eye_height: own.eye_height * 0.7,
        }
    }

    /// `dropHorn()`, which `RamTarget` calls of a ram into a block of `#snaps_goat_horn`:
    /// a kid or a goat without horns drops none; else the horn left (or one of two by
    /// the goat's own draw) comes off and drops at its feet with a push of three draws
    /// more. The horn and its motion, for the behaviour to throw; none where none came
    /// off.
    pub(crate) fn drop_horn(
        mob: &mut Mob,
        draws: &mut Random,
    ) -> Option<(clustine_world::ItemStack, crate::physics::Vec3)> {
        let own = state_of(mob);
        if animal::is_baby(mob) || !(own.left_horn || own.right_horn) {
            return None;
        }
        let left = if own.left_horn && own.right_horn {
            draws.next_bool()
        } else {
            own.left_horn
        };
        update(mob, |goat| {
            if left {
                goat.left_horn = false;
            } else {
                goat.right_horn = false;
            }
        });
        // `Mth.randomBetween(random, min, max)`, each widened.
        let mut between = |min: f32, max: f32| f64::from(draws.next_float() * (max - min) + min);
        let motion = [between(-0.2, 0.2), between(0.3, 0.7), between(-0.2, 0.2)];
        let horn = clustine_world::ItemStack::new(items::GOAT_HORN, 1);
        Some((horn, motion))
    }

    /// The instrument `createHorn()` gives a goat whose UUID is `uuid`: of its tag
    /// (`#regular_goat_horns`, or `#screaming_goat_horns` for a screamer) in the tag's
    /// order, by one `nextInt(4)` of a `SingleThreadedRandomSource` seeded with the
    /// UUID's hash (the brief's finding 7.0.3.3; the pick **Inferred** to be
    /// `Util.getRandomSafe`). Both horns of one goat are of one instrument.
    #[allow(
        dead_code,
        reason = "the instrument a horn carries once items have components"
    )]
    pub(crate) fn horn_instrument(uuid: u128, screaming: bool) -> &'static str {
        const REGULAR: [&str; 4] = [
            "minecraft:ponder_goat_horn",
            "minecraft:sing_goat_horn",
            "minecraft:seek_goat_horn",
            "minecraft:feel_goat_horn",
        ];
        const SCREAMING: [&str; 4] = [
            "minecraft:admire_goat_horn",
            "minecraft:call_goat_horn",
            "minecraft:yearn_goat_horn",
            "minecraft:dream_goat_horn",
        ];
        let tag = if screaming { SCREAMING } else { REGULAR };
        let pick = JavaRandom::new(i64::from(uuid_hash(uuid))).next_int(tag.len() as i32);
        tag[pick as usize]
    }

    /// `UUID.hashCode()`: the two halves' exclusive or, folded to an int.
    #[allow(
        dead_code,
        reason = "the instrument a horn carries once items have components"
    )]
    pub(crate) fn uuid_hash(uuid: u128) -> i32 {
        let hilo = ((uuid >> 64) as u64 ^ uuid as u64) as i64;
        ((hilo >> 32) as i32) ^ (hilo as i32)
    }

    /// `LegacyRandomSource` (`SingleThreadedRandomSource`'s algorithm): Java's linear
    /// congruential generator, which nothing else of Clustine draws from.
    #[allow(
        dead_code,
        reason = "the instrument a horn carries once items have components"
    )]
    pub(crate) struct JavaRandom(i64);

    #[allow(
        dead_code,
        reason = "the instrument a horn carries once items have components"
    )]
    impl JavaRandom {
        const MULTIPLIER: i64 = 0x5_DEEC_E66D;
        const MASK: i64 = (1 << 48) - 1;

        pub(crate) fn new(seed: i64) -> Self {
            Self((seed ^ Self::MULTIPLIER) & Self::MASK)
        }

        fn next(&mut self, bits: u32) -> i32 {
            self.0 = (self.0.wrapping_mul(Self::MULTIPLIER).wrapping_add(0xB)) & Self::MASK;
            (self.0 >> (48 - bits)) as i32
        }

        /// `nextInt(bound)` of a bound above nought.
        pub(crate) fn next_int(&mut self, bound: i32) -> i32 {
            if bound & -bound == bound {
                return ((i64::from(bound) * i64::from(self.next(31))) >> 31) as i32;
            }
            loop {
                let bits = self.next(31);
                let value = bits % bound;
                if bits - value + (bound - 1) >= 0 {
                    return value;
                }
            }
        }
    }
}

/// `FOOD_TEMPTATIONS` of a goat (`TemptingSensor.forAnimal`): wheat.
static TEMPTATIONS: TemptingSensor = TemptingSensor {
    tempts: |_, stack| FOOD.test(Some(stack)),
};

/// `NEAREST_ITEMS` of a goat: every item (`Mob.wantsToPickUp`, `canHoldItem`'s true),
/// a memory nothing of a goat reads but that its scan writes, as the game's.
static ITEMS: NearestItemSensor = NearestItemSensor { wants: |_, _| true };

/// `Goat.BRAIN_PROVIDER`: its sensors in the game's order, and `GoatAi`'s activities.
pub(crate) static PROVIDER: Provider = Provider {
    memories: &[],
    sensors: &[
        &sensors::NEAREST_LIVING_ENTITIES,
        &sensors::NEAREST_PLAYERS,
        &ITEMS,
        &sensors::NEAREST_ADULT,
        &sensors::HURT_BY,
        &TEMPTATIONS,
    ],
    activities,
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// `GoatAi.updateActivity`: the first of ram, long jump and idle whose conditions hold;
/// none holding, the activity stays (a goat tempted while its ram's cooldown is out).
pub(crate) const ACTIVITIES: [Activity; 3] = [Activity::Ram, Activity::LongJump, Activity::Idle];

/// `GoatAi.getActivities`.
fn activities() -> Vec<ActivityData> {
    use MemoryKind::{
        BreedTarget, LongJumpCoolingDown, RamCooldownTicks, TemptationCooldownTicks,
        TemptingPlayer, WalkTarget,
    };
    let core = ActivityData::numbered(
        Activity::Core,
        0,
        vec![
            panic::swim(0.8),
            panic::panic(2.0),
            walking::look_sink(45, 90),
            walking::sink(),
            cooldowns::count_down(TemptationCooldownTicks),
            cooldowns::count_down(LongJumpCoolingDown),
            cooldowns::count_down(RamCooldownTicks),
        ],
    );
    let idle = ActivityData {
        activity: Activity::Idle,
        behaviours: vec![
            (0, looking::look_sometimes(LookAt::Player, 6.0, (30, 60))),
            (0, AnimalMakeLove::new(GOAT_KIND).boxed()),
            (1, FollowTemptation::new(Speed::Fixed(1.25)).boxed()),
            (2, social::follow_adult((5, 16), 1.25)),
            (
                3,
                Box::new(Gate::run_one(vec![
                    (walking::stroll(1.0), 2),
                    (walking::walk_to_look_target(1.0, 3), 2),
                    (Box::new(HeldTimed::new(DoNothing { min: 30, max: 60 })), 1),
                ])),
            ),
        ],
        conditions: vec![
            (RamCooldownTicks, Status::Present),
            (LongJumpCoolingDown, Status::Present),
        ],
        erase: Vec::new(),
    };
    let long_jump = ActivityData {
        activity: Activity::LongJump,
        behaviours: vec![
            (
                0,
                LongJumpMidJump {
                    interval: brain::TIME_BETWEEN_LONG_JUMPS,
                    landing_sound: sounds::ENTITY_GOAT_STEP,
                    leaper: &LEAPER,
                }
                .boxed(),
            ),
            (
                1,
                LongJumpToRandomPos {
                    interval: brain::TIME_BETWEEN_LONG_JUMPS,
                    max_height: brain::LONG_JUMP_HEIGHT,
                    max_width: brain::LONG_JUMP_WIDTH,
                    max_velocity_multiplier: brain::MAX_JUMP_VELOCITY_MULTIPLIER,
                    leaper: &LEAPER,
                    preferred: None,
                }
                .boxed(),
            ),
        ],
        conditions: vec![
            (TemptingPlayer, Status::Absent),
            (BreedTarget, Status::Absent),
            (WalkTarget, Status::Absent),
            (LongJumpCoolingDown, Status::Absent),
        ],
        erase: Vec::new(),
    };
    let ram = ActivityData {
        activity: Activity::Ram,
        behaviours: vec![
            (
                0,
                RamTarget {
                    speed: 3.0,
                    targets: rams_target,
                    rammer: &RAMMER,
                }
                .boxed(),
            ),
            (
                1,
                PrepareRamNearestTarget {
                    cooldown_on_fail: brain::least_ram_cooldown,
                    min_distance: brain::RAM_DISTANCES.0,
                    max_distance: brain::RAM_DISTANCES.1,
                    walk_speed: 1.25,
                    targets: rams_target,
                    prepare_time: i64::from(brain::RAM_PREPARE_TIME),
                    sound: brain::prepare_ram_sound,
                }
                .boxed(),
            ),
        ],
        conditions: vec![
            (TemptingPlayer, Status::Absent),
            (BreedTarget, Status::Absent),
            (RamCooldownTicks, Status::Absent),
        ],
        erase: Vec::new(),
    };
    vec![core, idle, long_jump, ram]
}

/// `ageBoundaryReached`: the base of `ATTACK_DAMAGE` a kid's 1 and an adult's 2, set
/// where the instance has not that value (once a kid is made, once it grows up).
fn age_boundary(mob: &mut Mob) {
    let wanted = brain::attack_damage(mob);
    let id = crate::mobs::body::attribute(clustine_data::attributes::ATTACK_DAMAGE);
    if mob.body.attributes.value(GOAT_KIND, id) != wanted
        && let Some(instance) = mob.body.attributes.instance(GOAT_KIND, id)
    {
        instance.base = wanted;
    }
}

/// The goat's entity type, as the breeding's partner type.
const GOAT_KIND: u16 = clustine_data::entity_types::GOAT as u16;

/// `RAM_TARGET_CONDITIONS`' selector: anything but a goat (an armour stand too, mobs
/// griefing always). Players pass it, and the combat targeting it is part of turns away
/// every creative one.
fn rams_target(looked: &Looked) -> bool {
    looked.kind.is_none_or(brain::rams)
}

/// What a goat's long jump asks of it.
pub(crate) struct GoatLeaper;

pub(crate) static LEAPER: GoatLeaper = GoatLeaper;

impl Leaper for GoatLeaper {
    fn jump_sound(&self, mob: &Mob) -> i32 {
        brain::long_jump_sound(mob)
    }

    /// `getDimensions(LONG_JUMPING)`: its size scaled by 0.7.
    fn long_jumping_size(&self, cx: &Cx<'_, '_, '_>) -> (f32, f32) {
        let size = brain::long_jumping_dimensions(&cx.ai.tick.mob);
        (size.width, size.height)
    }

    fn set_discard_friction(&self, cx: &mut Cx<'_, '_, '_>, discard: bool) {
        update(&mut cx.ai.tick.mob, |goat| goat.discards_friction = discard);
    }

    fn set_long_jumping(&self, cx: &mut Cx<'_, '_, '_>, jumping: bool) {
        update(&mut cx.ai.tick.mob, |goat| goat.long_jumping = jumping);
    }
}

/// What a goat's ram asks of it (`GoatAi`'s lambdas, `Goat.dropHorn`).
pub(crate) struct GoatRammer;

pub(crate) static RAMMER: GoatRammer = GoatRammer;

impl Rammer for GoatRammer {
    fn time_between_rams(&self, mob: &Mob) -> (i32, i32) {
        brain::time_between_rams(mob)
    }

    fn knockback(&self, mob: &Mob) -> f64 {
        brain::ram_knockback(mob)
    }

    fn impact_sound(&self, mob: &Mob) -> i32 {
        brain::ram_impact_sound(mob)
    }

    fn horn_break_sound(&self, _: &Mob) -> i32 {
        brain::HORN_BREAK_SOUND
    }

    /// `dropHorn()`: the horn, where one came off, an item at the goat's feet with the
    /// push drawn (`new ItemEntity(level, x, y, z, horn, dx, dy, dz)`, of no pick-up
    /// delay).
    fn drop_horn(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        let tick = &mut *cx.ai.tick;
        let Some((stack, motion)) = brain::drop_horn(&mut tick.mob, tick.draws) else {
            return false;
        };
        let at = tick.living.body.position;
        tick.out.local.push(Local::Throw {
            stack,
            at,
            pick_up_delay: 0,
            motion,
        });
        true
    }
}

/// What a fresh load of a goat resets (the plan's 2.10): its pose and friction, which
/// the game neither saves nor sends again; for the brain's `fresh_load` hook when it
/// comes.
#[allow(
    dead_code,
    reason = "the fresh load of N7j1's contracts, not built yet"
)]
pub(crate) fn fresh_load(mob: &mut Mob) {
    update(mob, |goat| {
        goat.long_jumping = false;
        goat.discards_friction = false;
    });
}

impl MobType for Goat {
    /// The constructor's: `Animal`'s maluses, floating, and never into or onto powder
    /// snow. Its goals are its brain's.
    fn register_goals(&self, setup: &mut Setup) {
        super::cow::animal_maluses(setup);
        setup.config.navigation.flags.can_float = true;
        setup.set_malus(PathType::PowderSnow, -1.0);
        setup.set_malus(PathType::OnTopOfPowderSnow, -1.0);
    }

    fn head(&self) -> HeadLimits {
        HeadLimits {
            max_y_rot: 15,
            ..HeadLimits::default()
        }
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        animal::walk_target_value(ai, pos, clustine_data::blocks::GRASS_BLOCK)
    }

    fn brain(&self) -> Option<&'static Provider> {
        Some(&PROVIDER)
    }

    /// `brain.tick` and `GoatAi.updateActivity`, then `Animal`'s; before them the base
    /// of its attack damage as its age has it (`ageBoundaryReached`, which the game
    /// calls as the age crosses the boundary and the ram is the first to read after).
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        age_boundary(&mut tick.mob);
        machinery::run(tick, |brain, _| {
            brain.set_active_to_first_valid(&ACTIVITIES)
        });
        animal::custom_server_ai_step(tick);
    }

    /// `LivingEntity.travelInAir` of a goat in a jump (`shouldDiscardFriction`): its
    /// motion kept whole, neither the block's friction nor the air's drag taken off.
    fn before_travel(&self, tick: &mut MobTick<'_>) {
        if state_of(&tick.mob).discards_friction {
            tick.living.attributes.friction_modifier = 0.0;
            tick.living.attributes.air_drag_modifier = 0.0;
        }
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step(tick);
    }

    /// `Goat.mobInteract`: a bucket on a grown goat is a milk bucket for the player,
    /// every time; else an animal's use, and where that fed it, its eating sound once
    /// more (the game plays it inside `Animal.mobInteract` and again after, where the
    /// stack is still food: always for a creative player, who keeps it).
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        if animal::is_item(used.held.as_ref(), items::BUCKET) && !animal::is_baby(used.mob) {
            used.did.push(Did::Sound {
                sound: milking_sound(used.mob),
                source: animal::SOUND_SOURCE_PLAYERS,
                at: used.from,
                volume: 1.0,
                pitch: 1.0,
                but_the_player: true,
            });
            used.did.push(Did::Give {
                stack: clustine_world::ItemStack::new(items::MILK_BUCKET, 1),
                no_duplicates: true,
            });
            return Interaction::Success;
        }
        let fed = FOOD.test(used.held.as_ref());
        let result = animal::mob_interact(used, FOOD, None);
        if fed && result != Interaction::Pass {
            eating_sound(used);
            let still_food =
                used.creative || used.held.as_ref().is_some_and(|stack| stack.count > 1);
            if still_food {
                eating_sound(used);
            }
        }
        result
    }

    /// `Goat.finalizeSpawn`, of the level's draws: the brain's two cooldowns, screaming
    /// one time in fifty, and for an adult (always, here: the group's kid is made
    /// after) a horn off one time in ten; then `Animal`'s, a later goat of a group a kid
    /// one time in twenty, which may so have lost a horn already.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let cooldowns = brain::init_memories(spawn.level);
        brain::remember(spawn.mob, cooldowns);
        let screaming = spawn.level.next_double() < SCREAMING_CHANCE;
        update(spawn.mob, |goat| goat.screaming = screaming);
        if !animal::is_baby(spawn.mob) && f64::from(spawn.level.next_float()) < f64::from(0.1_f32) {
            let left = spawn.level.next_bool();
            update(spawn.mob, |goat| {
                if left {
                    goat.left_horn = false;
                } else {
                    goat.right_horn = false;
                }
            });
        }
        animal::finalize_ageable_default(spawn);
    }

    /// `Goat.getBreedOffspring`, of the level's draws on the breeding parent's behalf:
    /// the kid's cooldowns, a parent by a coin, and screaming where that parent screams,
    /// else one time in fifty. Its horns are both there.
    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        let mut child = Mob::fresh();
        let cooldowns = brain::init_memories(breeding.draws);
        brain::remember(&mut child, cooldowns);
        let (parent, parent_is_goat) = if breeding.draws.next_bool() {
            (breeding.this, true)
        } else {
            (
                breeding.partner,
                i32::from(breeding.partner_kind) == clustine_data::entity_types::GOAT,
            )
        };
        let screaming = (parent_is_goat && is_screaming(parent))
            || breeding.draws.next_double() < SCREAMING_CHANCE;
        update(&mut child, |goat| goat.screaming = screaming);
        Some(child)
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    fn landing(&self, _: &Mob) -> Option<Landing> {
        Some(Landing::Lowered { by: FALL_LESS })
    }

    /// A kid's size, and in a long jump's pose its size scaled by 0.7.
    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        if state_of(mob).long_jumping {
            return Some(brain::long_jumping_dimensions(mob));
        }
        animal::dimensions(mob, BABY)
    }

    fn ambient_sound(&self, mob: &Mob) -> Option<i32> {
        Some(voiced(
            mob,
            sounds::ENTITY_GOAT_AMBIENT,
            sounds::ENTITY_GOAT_SCREAMING_AMBIENT,
        ))
    }

    /// `playStepSound`: its own at 0.15 and 1, screaming or not.
    fn step_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_GOAT_STEP)
    }

    fn hurt_sound(&self, mob: &Mob, _: &DamageSource) -> Option<i32> {
        Some(voiced(
            mob,
            sounds::ENTITY_GOAT_HURT,
            sounds::ENTITY_GOAT_SCREAMING_HURT,
        ))
    }

    fn death_sound(&self, mob: &Mob) -> Option<i32> {
        Some(voiced(
            mob,
            sounds::ENTITY_GOAT_DEATH,
            sounds::ENTITY_GOAT_SCREAMING_DEATH,
        ))
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let own = state_of(mob);
        let mut values = animal::shown(mob);
        values.push((
            POSE_INDEX,
            DataShown::Pose(if own.long_jumping { LONG_JUMPING } else { 0 }),
            !own.long_jumping,
        ));
        values.push((
            SCREAMING_INDEX,
            DataShown::Boolean(own.screaming),
            !own.screaming,
        ));
        values.push((
            LEFT_HORN_INDEX,
            DataShown::Boolean(own.left_horn),
            own.left_horn,
        ));
        values.push((
            RIGHT_HORN_INDEX,
            DataShown::Boolean(own.right_horn),
            own.right_horn,
        ));
        values
    }
}
