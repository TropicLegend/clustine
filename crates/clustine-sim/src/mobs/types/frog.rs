//! The frog (`Frog` with its controls; `docs/groundwork/n7-brief.md`, sections 7.5.2 to
//! 7.5.7): an animal of its biome's variant (temperate, warm or cold), never a baby, fed
//! and bred with slime balls, its breeding making no child but frogspawn, which a
//! pregnant frog lays (`region/frogspawn.rs` hatches it), five less of every fall's hurt.
//!
//! Its mind is a brain (`FrogAi`, [`super::frog_ai`]): its croak, its long jumps, its
//! tongue and its laying are the brain's behaviours, its pose their sign to clients.
//!
//! Named difference: its swimming sounds are not made (the living body plays none yet).

use clustine_data::{PathType, sounds};
use clustine_world::{EntityId, TypeState};
use serde::{Deserialize, Serialize};

use super::animal::{self, Own, biome_in_tag, variants};
use super::{Breeding, Did, Interaction, MobType, Spawning, Used, cow};
use crate::living::{Sinks, Travel};
use crate::mobs::Mob;
use crate::mobs::ai::goals::Items;
use crate::mobs::ai::navigation::{Kind, Settings};
use crate::mobs::ai::{HeadLimits, MoveKind, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::MobTick;
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::region::Random;
use crate::shown::{DataShown, Holder};

/// `#frog_food`: slime balls.
pub(crate) const FOOD: Items = Items::Tag("minecraft:frog_food");

/// The registry of its variants, which the server sends.
const REGISTRY: &str = "minecraft:frog_variant";

/// The synced index of `DATA_VARIANT_ID`, on from `AgeableMob`'s 16 and 17;
/// `DATA_TONGUE_TARGET_ID` (19) is not shown (see [`super::frog_ai`]).
pub(crate) const VARIANT_INDEX: u8 = 18;

/// The synced index of `Entity.DATA_POSE`.
pub(crate) const POSE_INDEX: u8 = 6;

/// The poses a frog takes (`Pose`), by their ordinals.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Pose {
    #[default]
    Standing,
    LongJumping,
    Croaking,
    UsingTongue,
}

impl Pose {
    /// Its ordinal, as clients are sent it.
    pub(crate) fn ordinal(self) -> i32 {
        match self {
            Self::Standing => 0,
            Self::LongJumping => 6,
            Self::Croaking => 8,
            Self::UsingTongue => 9,
        }
    }
}

/// A frog's variant, by Clustine's own number of it, fixed for good: the registry's ids
/// are the order the server sends it in, which another version may change.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Variant {
    /// `frog_variant/temperate`, the default and the fallback.
    #[default]
    Temperate,
    Warm,
    Cold,
}

impl Variant {
    /// Its registry name.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Temperate => "minecraft:temperate",
            Self::Warm => "minecraft:warm",
            Self::Cold => "minecraft:cold",
        }
    }

    /// Its spawn condition and priority (`frog_variant/*.json`): warm and cold of their
    /// biome tags at 1, temperate always at 0.
    fn condition(self) -> (i32, Option<&'static str>) {
        match self {
            Self::Temperate => (0, None),
            Self::Warm => (1, Some("minecraft:spawns_warm_variant_frogs")),
            Self::Cold => (1, Some("minecraft:spawns_cold_variant_frogs")),
        }
    }

    /// The variant of the registry name `name`.
    fn named(name: &str) -> Option<Self> {
        [Self::Temperate, Self::Warm, Self::Cold]
            .into_iter()
            .find(|variant| variant.name() == name)
    }

    /// Its id in the registry the server sends.
    pub(crate) fn id(self) -> i32 {
        i32::from(variants::id(REGISTRY, self.name()))
    }
}

/// What a frog keeps beyond an animal: its variant (`DATA_VARIANT_ID`, saved as
/// `variant`), its pose (`DATA_POSE`, which its brain's behaviours set), its tongue's
/// target (`DATA_TONGUE_TARGET_ID`), a pregnancy a breeding gave it that its brain has
/// not taken yet (`IS_PREGNANT`, [`super::frog_ai::step`]), and whether its long jump
/// has it discard friction (`setDiscardFriction`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct FrogState {
    pub(crate) variant: Variant,
    pub(crate) pose: Pose,
    pub(crate) tongue_target: Option<EntityId>,
    pub(crate) pregnant: bool,
    pub(crate) discards_friction: bool,
}

impl Own for FrogState {}

/// A frog's own state.
pub(crate) fn read(mob: &Mob) -> FrogState {
    animal::read::<FrogState>(&mob.state).1
}

/// Changes a frog's own state.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut FrogState)) {
    let (shared, mut own) = animal::read::<FrogState>(&mob.state);
    let before = own.clone();
    change(&mut own);
    if own != before {
        mob.state = animal::write(&shared, &own);
    }
}

/// `VariantUtils.selectVariantToSpawn(context, FROG_VARIANT)` at the biome `biome`: of
/// the variants in the registry's order, stably by priority, highest first, those of
/// the highest priority whose condition holds, one drawn of them (`getRandomSafe`, a
/// `nextInt` even of one), of the level's random; the tags are disjoint, so it is always
/// one of one.
pub(crate) fn spawn_variant(biome: Option<&str>, level: &mut Random) -> Variant {
    let entries = clustine_data::synced_registry(REGISTRY).map_or(&[][..], |known| known.entries);
    let mut ranked: Vec<(Variant, i32, Option<&str>)> = entries
        .iter()
        .filter_map(|name| Variant::named(name))
        .map(|variant| {
            let (priority, tag) = variant.condition();
            (variant, priority, tag)
        })
        .collect();
    ranked.sort_by_key(|(_, priority, _)| std::cmp::Reverse(*priority));
    let mut best = i32::MIN;
    let mut kept = Vec::new();
    for (variant, priority, tag) in ranked {
        if priority < best {
            continue;
        }
        if tag.is_none_or(|tag| biome_in_tag(biome, tag)) {
            best = priority;
            kept.push(variant);
        }
    }
    if kept.is_empty() {
        return Variant::Temperate;
    }
    kept[level.next_int(kept.len() as u32) as usize]
}

/// `FrogAi.initMemories(frog, random)`'s draw: `LONG_JUMP_COOLDOWN_TICKS` of
/// `UniformInt(100, 140)`, `nextInt(41) + 100`, which the brain keeps.
pub(crate) fn init_memories(mob: &mut Mob, random: &mut Random) {
    let cooldown = 100 + random.next_int(41) as i32;
    super::frog_ai::set_long_jump_cooldown(mob, cooldown);
}

/// The frog.
pub(crate) struct Frog;

impl MobType for Frog {
    /// No goals: its mind is its brain's. Its constructor's navigation (amphibious: the
    /// frog's own evaluator is not in the navigation, see [`super::frog_ai`]), maluses
    /// and move control (`SmoothSwimmingMoveControl(85, 10, 0.02, 0.1, true)`); its look
    /// control is a plain one.
    fn register_goals(&self, setup: &mut Setup) {
        setup.config.navigation = Settings::of(Kind::Amphibious);
        cow::animal_maluses(setup);
        setup.set_malus(PathType::Water, 4.0);
        setup.set_malus(PathType::Trapdoor, -1.0);
        setup.config.move_kind = MoveKind::SmoothSwimming {
            max_turn_x: 85,
            max_turn_y: 10,
            in_water: 0.02,
            outside_water: 0.1,
            gravity: true,
        };
    }

    /// `getHeadRotSpeed()` 35 and `getMaxHeadYRot()` 5.
    fn head(&self) -> HeadLimits {
        HeadLimits {
            max_y_rot: 5,
            rot_speed: 35,
            ..HeadLimits::default()
        }
    }

    /// `finalizeSpawn` before `Animal`'s: its biome's variant and its first long jump's
    /// cooldown, of the level's random. An egg's frog is the first of its group, which
    /// draws nothing for a baby.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let variant = spawn_variant(spawn.biome, spawn.level);
        update(spawn.mob, |frog| frog.variant = variant);
        init_memories(spawn.mob, spawn.level);
    }

    /// `canBeABaby()`: never.
    fn can_be_a_baby(&self) -> bool {
        false
    }

    fn brain(&self) -> Option<&'static crate::mobs::brain::Provider> {
        Some(&super::frog_ai::PROVIDER)
    }

    /// `customServerAiStep`: the brain's tick and its activities, then `Animal`'s.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        super::frog_ai::step(tick);
        animal::custom_server_ai_step(tick);
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step(tick);
    }

    /// `travelInAir` of a frog that discards friction (`shouldDiscardFriction`, its long
    /// jump's): its motion kept but for gravity, which a friction and an air drag
    /// modified to nothing give in the living body's travel. The air's drag it bounces
    /// with on a slime block is so too, where the game's is not.
    fn before_travel(&self, tick: &mut MobTick<'_>) {
        if read(&tick.mob).discards_friction && !tick.living.body.in_water && !tick.living.in_lava()
        {
            tick.living.attributes.friction_modifier = 0.0;
            tick.living.attributes.air_drag_modifier = 0.0;
        }
    }

    /// `travelInWater`: at its own speed along its input, slowed to nine tenths, never
    /// sinking.
    fn travel(&self, tick: &MobTick<'_>) -> Travel {
        Travel::Swims {
            speed: tick.mob.body.inputs.speed,
            sinks: Sinks::Never,
        }
    }

    fn pushed_by_fluid(&self) -> bool {
        false
    }

    /// `Animal.mobInteract` with slime balls, and its eating sound when fed
    /// (`playEatingSound`: `FROG_EAT` at 2 and 1, no draw).
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        let fed = FOOD.test(used.held);
        let result = animal::mob_interact(used, FOOD, None);
        if fed && result != Interaction::Pass {
            used.did.push(Did::Sound {
                sound: sounds::ENTITY_FROG_EAT,
                source: SOUND_SOURCE_NEUTRAL,
                at: used.position,
                volume: 2.0,
                pitch: 1.0,
                but_the_player: false,
            });
        }
        result
    }

    /// `getBreedOffspring`: a fresh frog (of the default variant) with its long jump's
    /// first cooldown, which only a spawn egg used on a frog makes.
    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        let mut child = Mob::fresh();
        init_memories(&mut child, breeding.draws);
        Some(child)
    }

    /// `spawnChildFromBreeding`: no child, but the frog that bred pregnant, which its
    /// brain takes at its next tick ([`super::frog_ai::step`]).
    fn bred_without_child(&self, this: &Mob) -> Option<TypeState> {
        let mut bred = this.clone();
        update(&mut bred, |frog| frog.pregnant = true);
        Some(bred.state)
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_FROG_AMBIENT)
    }

    fn step_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_FROG_STEP)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_FROG_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_FROG_DEATH)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let own = read(mob);
        let variant = own.variant;
        let mut values = animal::shown(mob);
        values.push((
            POSE_INDEX,
            DataShown::Pose(own.pose.ordinal()),
            own.pose == Pose::Standing,
        ));
        values.push((
            VARIANT_INDEX,
            DataShown::Holder(Holder::FrogVariant, variant.id()),
            variant == Variant::Temperate,
        ));
        values
    }
}

/// The frog a tadpole grows into (`Tadpole.ageUp()`): `EntityType.create` and
/// `convertTo`'s copy of the tadpole's flags and head, then its `finalizeSpawn` of
/// `CONVERSION` with the level's draws (its variant by the biome it stands in, its long
/// jump's cooldown, then `Mob`'s), and kept for good.
pub(crate) fn grown_from(tadpole: &Mob) -> Mob {
    let mut frog = Mob::fresh();
    frog.body.flags = tadpole.body.flags.clone();
    frog.body.head = tadpole.body.head.clone();
    frog
}
