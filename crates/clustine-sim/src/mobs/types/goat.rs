//! Goats (`Goat`; `docs/groundwork/n7-brief.md`, section 7.6): an animal of wheat that is
//! milked with a bucket at every click, one in fifty of them screaming (its own sounds,
//! its rams sooner, its horns of other instruments), one adult in ten of an egg with a
//! horn missing; a fall hurts it ten less than another animal.
//!
//! Its mind is a brain (`GoatAi`), which N7j's machinery brings (part A and the lead's
//! contracts): until its provider is wired a goat has no goals and stands and falls by
//! its body alone. What the brain will read and call of the goat is here as plain
//! functions in [`brain`]: the two cooldowns `initMemories` draws, the ram's ranges,
//! knockback and sounds, the long jump's sound and size, the horn a ram knocks off.
//!
//! Named differences: the attack damage of a kid (1, `ageBoundaryReached`) is derived
//! from its age by [`brain::attack_damage`] rather than kept as the attribute's base,
//! which only a ram reads; `setYHeadRot`'s clamp of the head to 15 degrees of the body
//! waits for the brain's looking (no goal turns a goat's head before it); a horn drops
//! as a plain goat horn without its instrument (the owner's question 4 of the plan's
//! 11.7), though [`brain::horn_instrument`] names the one the game would give.

use clustine_data::{PathType, items, sounds};
use serde::{Deserialize, Serialize};

use super::animal::{self, Own};
use super::{Breeding, Did, Dimensions, Interaction, MobType, Spawning, Used};
use crate::living::Landing;
use crate::mobs::Mob;
use crate::mobs::ai::goals::Items;
use crate::mobs::ai::{Ai, HeadLimits, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::MobTick;
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

/// The synced data's indices: screaming, the left horn, the right horn.
const SCREAMING_INDEX: u8 = 18;
const LEFT_HORN_INDEX: u8 = 19;
const RIGHT_HORN_INDEX: u8 = 20;

/// How much less a fall hurts a goat (`calculateFallDamage`).
const FALL_LESS: i32 = 10;

/// The chance of a screaming goat at a spawn or a breeding.
const SCREAMING_CHANCE: f64 = 0.02;

/// What a goat keeps beyond an animal: whether it screams, and its two horns
/// (`IsScreamingGoat`, `HasLeftHorn`, `HasRightHorn`). Its cooldowns are its brain's
/// memories, not its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct GoatState {
    pub(crate) screaming: bool,
    pub(crate) left_horn: bool,
    pub(crate) right_horn: bool,
}

impl Default for GoatState {
    /// `defineSynchedData`'s: not screaming, both horns.
    fn default() -> Self {
        Self {
            screaming: false,
            left_horn: true,
            right_horn: true,
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

/// What the brain will read and call of a goat, until its provider is wired (part A
/// and the lead's N7j1): plain functions of the game's lambdas and the goat's own
/// methods that `GoatAi`'s behaviours call.
#[allow(dead_code, reason = "what the goat's brain, not wired yet, reads")]
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
    /// The entity events of a goat lowering its head and raising it again.
    pub(crate) const LOWER_HEAD: u8 = 58;
    pub(crate) const RAISE_HEAD: u8 = 59;

    /// The cooldowns `GoatAi.initMemories` gives a goat: its long jump's, then its
    /// ram's.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct Cooldowns {
        pub(crate) long_jump: i32,
        pub(crate) ram: i32,
    }

    /// `UniformInt.of(min, max).sample(random)`: `nextInt(max − min + 1) + min`.
    pub(crate) fn sample((min, max): (i32, i32), draws: &mut Random) -> i32 {
        draws.next_int((max - min + 1) as u32) as i32 + min
    }

    /// `GoatAi.initMemories(goat, random)`: the long jump's cooldown, then the ram's, of
    /// the plain goat's range even for a screamer (screaming is decided after).
    pub(crate) fn init_memories(draws: &mut Random) -> Cooldowns {
        let long_jump = sample(TIME_BETWEEN_LONG_JUMPS, draws);
        let ram = sample(TIME_BETWEEN_RAMS, draws);
        Cooldowns { long_jump, ram }
    }

    /// Where the two cooldowns become the brain's memories (`setMemory`, no expiry):
    /// the seam the provider is wired at. Until then they are drawn in the game's order
    /// and kept nowhere.
    pub(crate) fn remember(mob: &mut Mob, cooldowns: Cooldowns) {
        let _ = (mob, cooldowns);
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
        let horn = clustine_world::ItemStack {
            item: items::GOAT_HORN,
            count: 1,
        };
        Some((horn, motion))
    }

    /// The instrument `createHorn()` gives a goat whose UUID is `uuid`: of its tag
    /// (`#regular_goat_horns`, or `#screaming_goat_horns` for a screamer) in the tag's
    /// order, by one `nextInt(4)` of a `SingleThreadedRandomSource` seeded with the
    /// UUID's hash (the brief's finding 7.0.3.3; the pick **Inferred** to be
    /// `Util.getRandomSafe`). Both horns of one goat are of one instrument.
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
    pub(crate) fn uuid_hash(uuid: u128) -> i32 {
        let hilo = ((uuid >> 64) as u64 ^ uuid as u64) as i64;
        ((hilo >> 32) as i32) ^ (hilo as i32)
    }

    /// `LegacyRandomSource` (`SingleThreadedRandomSource`'s algorithm): Java's linear
    /// congruential generator, which nothing else of Clustine draws from.
    pub(crate) struct JavaRandom(i64);

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

impl MobType for Goat {
    /// The constructor's: `Animal`'s maluses, floating, and never into or onto powder
    /// snow. Its goals are its brain's, which are not wired yet.
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

    /// `brain.tick` and `GoatAi.updateActivity` (the seam the provider is wired at),
    /// then `Animal`'s.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::custom_server_ai_step(tick);
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step(tick);
    }

    /// `Goat.mobInteract`: a bucket on a grown goat is a milk bucket for the player,
    /// every time; else an animal's use, and where that fed it, its eating sound once
    /// more (the game plays it inside `Animal.mobInteract` and again after, where the
    /// stack is still food: always for a creative player, who keeps it).
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        if animal::is_item(used.held, items::BUCKET) && !animal::is_baby(used.mob) {
            used.did.push(Did::Sound {
                sound: milking_sound(used.mob),
                source: animal::SOUND_SOURCE_PLAYERS,
                at: used.from,
                volume: 1.0,
                pitch: 1.0,
                but_the_player: true,
            });
            used.did.push(Did::Give {
                stack: clustine_world::ItemStack {
                    item: items::MILK_BUCKET,
                    count: 1,
                },
                no_duplicates: true,
            });
            return Interaction::Success;
        }
        let fed = FOOD.test(used.held);
        let result = animal::mob_interact(used, FOOD, None);
        if fed && result != Interaction::Pass {
            eating_sound(used);
            let still_food = used.creative || used.held.is_some_and(|stack| stack.count > 1);
            if still_food {
                eating_sound(used);
            }
        }
        result
    }

    /// `Goat.finalizeSpawn`, before `Animal`'s, of the level's draws: the brain's two
    /// cooldowns, screaming one time in fifty, and for an adult (always, here: the
    /// group's baby comes after) a horn off one time in ten.
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

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
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
