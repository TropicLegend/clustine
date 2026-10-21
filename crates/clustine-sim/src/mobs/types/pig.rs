//! Pigs (`Pig`; `docs/groundwork/n7-brief.md`, section 4.9): an animal of carrots,
//! potatoes and beetroots, tempted by a carrot on a stick too, that takes a saddle, of a
//! temperature variant drawn by its biome and a sound variant drawn at random.
//!
//! A saddled pig takes a player on, who steers it holding a carrot on a stick and
//! boosts it with a use of the stick (`docs/groundwork/n7h-riding-plan.md`).
//!
//! Named differences: lightning does not make a zombified piglin of it (N9); a player
//! of another region does not get on it across a line, as on a boat.

use clustine_blocks::data::entity_in_tag;
use clustine_data::{items, sounds};
use serde::{Deserialize, Serialize};

use super::animal::{self, Own, variants};
use super::{Breeding, Did, Dimensions, Interaction, MobType, Spawning, Used, cow};
use crate::living::slots;
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    FloatGoal, Items, LookAtPlayerGoal, PanicGoal, RandomLookAroundGoal, RandomStrollGoal,
    TemptGoal,
};
use crate::mobs::ai::sensing::Class;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::MobTick;
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::shown::{DataShown, Holder};
use clustine_world::BlockPos;

const VARIANTS: &str = "minecraft:pig_variant";
const SOUND_VARIANTS: &str = "minecraft:pig_sound_variant";

/// `#pig_food`.
const FOOD: Items = Items::Tag("minecraft:pig_food");

/// The carrot on a stick, which tempts a pig and is no food of it.
const ON_A_STICK: Items = Items::Of(&[items::CARROT_ON_A_STICK]);

/// A piglet's size.
const BABY: Dimensions = Dimensions {
    width: 0.45,
    height: 0.45,
    eye_height: 0.406_25,
};

/// The synced data's indices: the boost's time, the variant and the sound variant.
const BOOST_INDEX: u8 = 18;
const VARIANT_INDEX: u8 = 19;
const SOUND_VARIANT_INDEX: u8 = 20;

/// What a pig keeps beyond an animal: its variant and sound variant, by their ids. Its
/// saddle is the body's equipment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct PigState {
    pub(crate) variant: u8,
    pub(crate) sound: u8,
}

impl Default for PigState {
    fn default() -> Self {
        Self {
            variant: variants::id(VARIANTS, "minecraft:temperate"),
            sound: variants::id(SOUND_VARIANTS, "minecraft:classic"),
        }
    }
}

impl Own for PigState {}

/// `ItemBasedSteering`: whether it is boosting, the ticks it has boosted, and the
/// boost's length (`DATA_BOOST_TIME`, synced). Kept after [`PigState`] in its type's
/// record, so that a pig kept before there was a boost reads as without one; durable,
/// as a crossing of a line carries the record and a boost kept apart would be
/// forgotten at every line (`docs/groundwork/n7h-riding-plan.md`, section 10 item 5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Boost {
    pub(crate) boosting: bool,
    pub(crate) time: u32,
    pub(crate) total: i32,
}

/// The pig's boost, as its record has it.
pub(crate) fn boost_of(mob: &Mob) -> Boost {
    let rest = animal::own_bytes(&mob.state);
    match postcard::take_from_bytes::<PigState>(rest) {
        Ok((_, after)) if !after.is_empty() => postcard::from_bytes(after).unwrap_or_default(),
        _ => Boost::default(),
    }
}

/// Keeps `boost` in the pig's record, all else as it was.
fn set_boost(mob: &mut Mob, boost: Boost) {
    let (animal, own) = animal::read::<PigState>(&mob.state);
    let tail = if boost == Boost::default() {
        Vec::new()
    } else {
        postcard::to_allocvec(&boost).expect("a boost is encoded into memory")
    };
    mob.state = animal::write_with_tail(&animal, &own, &tail);
}

/// `Pig.boost()`, `ItemBasedSteering.boost(random)`: refused while it boosts; else it
/// boosts from 0 for `nextInt(841) + 140` ticks, drawn from the pig's own random
/// (the class's 140 and 700 are not what it draws). Whether it began.
pub(crate) fn boost(mob: &mut Mob, draws: &mut crate::region::Random) -> bool {
    let mut boost = boost_of(mob);
    if boost.boosting {
        return false;
    }
    boost.boosting = true;
    boost.time = 0;
    boost.total = draws.next_int(841) as i32 + 140;
    set_boost(mob, boost);
    true
}

/// `Mob.isSaddled()`: a saddle in its saddle slot.
fn saddled(mob: &Mob) -> bool {
    mob.body.equipment.slots[slots::SADDLE as usize]
        .is_some_and(|stack| stack.count > 0 && stack.item == items::SADDLE)
}

/// The pig.
pub(crate) struct Pig;

/// The sounds of its sound variant's adult or baby set: ambient, hurt, death, eating.
fn sound_set(mob: &Mob) -> [i32; 4] {
    if animal::is_baby(mob) {
        return [
            sounds::ENTITY_BABY_PIG_AMBIENT,
            sounds::ENTITY_BABY_PIG_HURT,
            sounds::ENTITY_BABY_PIG_DEATH,
            sounds::ENTITY_BABY_PIG_EAT,
        ];
    }
    let (_, own) = animal::read::<PigState>(&mob.state);
    if own.sound == variants::id(SOUND_VARIANTS, "minecraft:big") {
        [
            sounds::ENTITY_PIG_BIG_AMBIENT,
            sounds::ENTITY_PIG_BIG_HURT,
            sounds::ENTITY_PIG_BIG_DEATH,
            sounds::ENTITY_PIG_BIG_EAT,
        ]
    } else if own.sound == variants::id(SOUND_VARIANTS, "minecraft:mini") {
        [
            sounds::ENTITY_PIG_MINI_AMBIENT,
            sounds::ENTITY_PIG_MINI_HURT,
            sounds::ENTITY_PIG_MINI_DEATH,
            sounds::ENTITY_PIG_MINI_EAT,
        ]
    } else {
        [
            sounds::ENTITY_PIG_AMBIENT,
            sounds::ENTITY_PIG_HURT,
            sounds::ENTITY_PIG_DEATH,
            sounds::ENTITY_PIG_EAT,
        ]
    }
}

/// `isEquippableInSlot(stack, SADDLE)` and `Equippable.equipOnTarget`: a saddle on a
/// grown pig without one is put on, kept to be dropped whatever kills it, with the pig's
/// own saddling sound (its seed one draw of its random) and `GameEvent.EQUIP`.
fn equip_saddle(used: &mut Used<'_>) -> Interaction {
    let saddle = slots::SADDLE as usize;
    let Some(stack) = used.held.filter(|stack| stack.count > 0) else {
        return Interaction::Pass;
    };
    let fits = clustine_data::ItemComponents::of(stack.item)
        .equippable
        .is_some_and(|equippable| usize::from(equippable.slot) == saddle)
        && entity_in_tag(i32::from(used.kind), "minecraft:can_equip_saddle")
        && !used.mob.body.vitals.dying()
        && !animal::is_baby(used.mob);
    if !fits || used.mob.body.equipment.slots[saddle].is_some() {
        return Interaction::Pass;
    }
    used.mob.body.equipment.slots[saddle] = Some(clustine_world::ItemStack {
        item: stack.item,
        count: 1,
    });
    // `setGuaranteedDrop(SADDLE)`.
    let mut chances = used.mob.body.equipment.drop_chances.unwrap_or([0.085; 8]);
    chances[saddle] = 2.0;
    used.mob.body.equipment.drop_chances = Some(chances);
    // `onEquipItem`: `playSeededSound` with the seed the pig's random draws.
    let _seed = used.draws.next_u64();
    used.did.push(Did::Sound {
        sound: sounds::ENTITY_PIG_SADDLE,
        source: SOUND_SOURCE_NEUTRAL,
        at: used.position,
        volume: 1.0,
        pitch: 1.0,
        but_the_player: false,
    });
    used.did.push(Did::GameEvent("minecraft:equip"));
    Interaction::Success
}

impl MobType for Pig {
    fn register_goals(&self, setup: &mut Setup) {
        cow::animal_maluses(setup);
        setup.add_goal(0, FloatGoal);
        setup.add_goal(1, PanicGoal { speed: 1.25 });
        setup.add_goal(3, animal::BreedGoal { speed: 1.0 });
        setup.add_goal(4, TemptGoal::new(1.2, ON_A_STICK, false));
        setup.add_goal(4, TemptGoal::new(1.2, FOOD, false));
        setup.add_goal(5, animal::FollowParentGoal { speed: 1.1 });
        setup.add_goal(6, RandomStrollGoal::avoiding_water(1.0));
        setup.add_goal(7, LookAtPlayerGoal::new(Class::Player, 6.0));
        setup.add_goal(8, RandomLookAroundGoal);
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        animal::walk_target_value(ai, pos, clustine_data::blocks::GRASS_BLOCK)
    }

    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::custom_server_ai_step(tick);
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step(tick);
    }

    /// `Pig.mobInteract`: a saddled pig that carries nobody takes on a player who does
    /// not sneak and holds no food of it; else an animal's use, then a saddle put on.
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        let food = FOOD.test(used.held);
        if !food && saddled(used.mob) && !used.carries && !used.sneaking {
            used.did.push(Did::Mount);
            return Interaction::Success;
        }
        let eat = sound_set(used.mob)[3];
        let result = animal::mob_interact(used, FOOD, Some(eat));
        if result != Interaction::Pass {
            return result;
        }
        equip_saddle(used)
    }

    /// `Pig.getControllingPassenger`: saddled, and the player holds a carrot on a stick
    /// in either hand.
    fn steered_by(&self, mob: &Mob, hands: [Option<clustine_world::ItemStack>; 2]) -> bool {
        saddled(mob)
            && hands.iter().any(|hand| {
                hand.is_some_and(|stack| stack.count > 0 && stack.item == items::CARROT_ON_A_STICK)
            })
    }

    /// `Pig.tickRidden`: the rider's yaw and half their pitch, each `% 360`, the body
    /// and the head turned to the yaw; and `ItemBasedSteering.tickBoost`, which counts
    /// only while it is steered: `boosting && boostTime++ > total` ends it.
    fn tick_ridden(&self, tick: &mut MobTick<'_>, yaw: f32, pitch: f32) {
        tick.yaw = yaw % 360.0;
        tick.pitch = (pitch * 0.5) % 360.0;
        tick.mob.body.head.body_yaw = tick.yaw;
        tick.mob.body.head.head_yaw = tick.yaw;
        let mut boost = boost_of(&tick.mob);
        if boost.boosting {
            let before = boost.time;
            boost.time += 1;
            if i64::from(before) > i64::from(boost.total) {
                boost.boosting = false;
            }
            set_boost(&mut tick.mob, boost);
        }
    }

    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let (animal, mut own) = animal::read::<PigState>(&spawn.mob.state);
        own.variant = variants::select(VARIANTS, spawn.biome, spawn.level);
        own.sound = spawn.level.next_int(variants::count(SOUND_VARIANTS)) as u8;
        spawn.mob.state = animal::write(&animal, &own);
        animal::finalize_ageable_default(spawn);
    }

    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        let (_, this) = animal::read::<PigState>(&breeding.this.state);
        let (_, partner) = animal::read::<PigState>(&breeding.partner.state);
        let variant = if breeding.draws.next_bool() {
            this.variant
        } else {
            partner.variant
        };
        let mut child = Mob::fresh();
        child.state = animal::write(
            &animal::Animal::default(),
            &PigState {
                variant,
                ..PigState::default()
            },
        );
        Some(child)
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        animal::dimensions(mob, BABY)
    }

    fn ambient_sound(&self, mob: &Mob) -> Option<i32> {
        Some(sound_set(mob)[0])
    }

    /// `Pig.playStepSound`: its set's step, a baby's its own; the big and the mini
    /// sets step as the classic one does (the registry of sounds has no step of theirs).
    fn step_sound(&self, mob: &Mob) -> Option<i32> {
        Some(if animal::is_baby(mob) {
            sounds::ENTITY_BABY_PIG_STEP
        } else {
            sounds::ENTITY_PIG_STEP
        })
    }

    fn ambient_interval(&self) -> u32 {
        120
    }

    fn hurt_sound(&self, mob: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sound_set(mob)[1])
    }

    fn death_sound(&self, mob: &Mob) -> Option<i32> {
        Some(sound_set(mob)[2])
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let (_, own) = animal::read::<PigState>(&mob.state);
        let fresh = PigState::default();
        let mut values = animal::shown(mob);
        // `DATA_BOOST_TIME`: the last boost's length, kept when it is over.
        let total = boost_of(mob).total;
        values.push((BOOST_INDEX, DataShown::Int(total), total == 0));
        values.push((
            VARIANT_INDEX,
            DataShown::Holder(Holder::PigVariant, i32::from(own.variant)),
            own.variant == fresh.variant,
        ));
        values.push((
            SOUND_VARIANT_INDEX,
            DataShown::Holder(Holder::PigSoundVariant, i32::from(own.sound)),
            own.sound == fresh.sound,
        ));
        values
    }
}
