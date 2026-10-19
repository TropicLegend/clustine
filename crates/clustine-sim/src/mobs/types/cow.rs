//! Cows (`AbstractCow` and `Cow`; `docs/groundwork/n7-brief.md`, sections 4.8.1 and
//! 4.8.2): an animal of wheat that is milked with a bucket, of a temperature variant
//! drawn by the biome it is made in and a sound variant drawn at random. What a cow and
//! a mooshroom share (`AbstractCow`) is here and the mooshroom's module uses it.

use clustine_data::{PathType, items, sounds};
use serde::{Deserialize, Serialize};

use super::animal::{self, Own, variants};
use super::{Breeding, Did, Dimensions, Interaction, MobType, Spawning, Used};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    FloatGoal, Items, LookAtPlayerGoal, PanicGoal, RandomLookAroundGoal, RandomStrollGoal,
    TemptGoal,
};
use crate::mobs::ai::sensing::Class;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::MobTick;
use crate::shown::{DataShown, Holder};
use clustine_world::BlockPos;

/// The registries of a cow's variants.
const VARIANTS: &str = "minecraft:cow_variant";
const SOUND_VARIANTS: &str = "minecraft:cow_sound_variant";

/// `#cow_food`, a mooshroom's too.
pub(crate) const FOOD: Items = Items::Tag("minecraft:cow_food");

/// A baby cow's or mooshroom's size (`BABY_DIMENSIONS`).
pub(crate) const BABY: Dimensions = Dimensions {
    width: 0.45,
    height: 0.7,
    eye_height: 0.69,
};

/// The synced data's indices of a cow's variants.
const VARIANT_INDEX: u8 = 18;
const SOUND_VARIANT_INDEX: u8 = 19;

/// What a cow keeps beyond an animal: its variant and its sound variant, by their ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct CowState {
    pub(crate) variant: u8,
    pub(crate) sound: u8,
}

impl Default for CowState {
    /// `defineSynchedData`'s: temperate, and the classic sounds.
    fn default() -> Self {
        Self {
            variant: variants::id(VARIANTS, "minecraft:temperate"),
            sound: variants::id(SOUND_VARIANTS, "minecraft:classic"),
        }
    }
}

impl Own for CowState {}

/// The cow.
pub(crate) struct Cow;

/// `AbstractCow.registerGoals` and `Animal`'s maluses.
pub(crate) fn register_goals(setup: &mut Setup) {
    animal_maluses(setup);
    setup.add_goal(0, FloatGoal);
    setup.add_goal(1, PanicGoal { speed: 2.0 });
    setup.add_goal(2, animal::BreedGoal { speed: 1.0 });
    setup.add_goal(3, TemptGoal::new(1.25, FOOD, false));
    setup.add_goal(4, animal::FollowParentGoal { speed: 1.25 });
    setup.add_goal(5, RandomStrollGoal::avoiding_water(1.0));
    setup.add_goal(6, LookAtPlayerGoal::new(Class::Player, 6.0));
    setup.add_goal(7, RandomLookAroundGoal);
}

/// `Animal`'s constructor: fire beside a path costs 16, and fire is never walked.
pub(crate) fn animal_maluses(setup: &mut Setup) {
    setup.set_malus(PathType::FireInNeighbor, 16.0);
    setup.set_malus(PathType::Fire, -1.0);
}

/// `AbstractCow.mobInteract`: a bucket on a grown cow is a milk bucket for the player,
/// whose own client plays the sound; else an animal's use.
pub(crate) fn milk_or_feed(used: &mut Used<'_>) -> Interaction {
    if animal::is_item(used.held, items::BUCKET) && !animal::is_baby(used.mob) {
        used.did.push(Did::Sound {
            sound: sounds::ENTITY_COW_MILK,
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
    animal::mob_interact(used, FOOD, None)
}

/// The sounds of a cow's sound variant: ambient, hurt, death.
fn sound_set(mob: &Mob) -> [i32; 3] {
    let (_, own) = animal::read::<CowState>(&mob.state);
    if own.sound == variants::id(SOUND_VARIANTS, "minecraft:moody") {
        [
            sounds::ENTITY_COW_MOODY_AMBIENT,
            sounds::ENTITY_COW_MOODY_HURT,
            sounds::ENTITY_COW_MOODY_DEATH,
        ]
    } else {
        classic_sounds()
    }
}

/// `SoundEvents.COW_SOUNDS`, the classic set, a mooshroom's.
pub(crate) fn classic_sounds() -> [i32; 3] {
    [
        sounds::ENTITY_COW_AMBIENT,
        sounds::ENTITY_COW_HURT,
        sounds::ENTITY_COW_DEATH,
    ]
}

impl MobType for Cow {
    fn register_goals(&self, setup: &mut Setup) {
        register_goals(setup);
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

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        milk_or_feed(used)
    }

    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let (animal, mut own) = animal::read::<CowState>(&spawn.mob.state);
        own.variant = variants::select(VARIANTS, spawn.biome, spawn.level);
        own.sound = spawn.level.next_int(variants::count(SOUND_VARIANTS)) as u8;
        spawn.mob.state = animal::write(&animal, &own);
    }

    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        let (_, this) = animal::read::<CowState>(&breeding.this.state);
        let (_, partner) = animal::read::<CowState>(&breeding.partner.state);
        let variant = if breeding.draws.next_bool() {
            this.variant
        } else {
            partner.variant
        };
        let mut child = Mob::fresh();
        let own = CowState {
            variant,
            ..CowState::default()
        };
        child.state = animal::write(&animal::Animal::default(), &own);
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

    fn ambient_interval(&self) -> u32 {
        120
    }

    fn hurt_sound(&self, mob: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sound_set(mob)[1])
    }

    fn death_sound(&self, mob: &Mob) -> Option<i32> {
        Some(sound_set(mob)[2])
    }

    fn sound_volume(&self) -> f32 {
        0.4
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let (_, own) = animal::read::<CowState>(&mob.state);
        let fresh = CowState::default();
        let mut values = animal::shown(mob);
        values.push((
            VARIANT_INDEX,
            DataShown::Holder(Holder::CowVariant, i32::from(own.variant)),
            own.variant == fresh.variant,
        ));
        values.push((
            SOUND_VARIANT_INDEX,
            DataShown::Holder(Holder::CowSoundVariant, i32::from(own.sound)),
            own.sound == fresh.sound,
        ));
        values
    }
}
