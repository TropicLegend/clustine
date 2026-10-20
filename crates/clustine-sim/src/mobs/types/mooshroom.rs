//! Mooshrooms (`MushroomCow`; `docs/groundwork/n7-brief.md`, sections 4.8.3 to 4.8.5): a
//! cow red or brown that gives stew for a bowl, a suspicious stew for a bowl once a brown
//! one has eaten a flower, and is sheared into a cow with its mushrooms.
//!
//! Named differences: a suspicious stew carries no effects (Clustine's stacks have no
//! components), so a brown mooshroom keeps only which flower it ate; the swap of colour
//! by lightning is left out with the weather (N9); the explosion and the effect's
//! particles are not sent.

use clustine_blocks::data::is_class;
use clustine_data::{BlockClass, entity_types, items, sounds};
use clustine_world::ItemStack;
use serde::{Deserialize, Serialize};

use super::animal::{self, Own};
use super::{Breeding, Did, Dimensions, Interaction, MobType, Used, cow};
use crate::mobs::Mob;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::body::loot;
use crate::mobs::tick::MobTick;
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::shown::DataShown;
use clustine_world::BlockPos;

/// The synced data's index of its colour (`DATA_TYPE`: red 0, brown 1).
const TYPE_INDEX: u8 = 18;

/// What a mooshroom keeps beyond an animal: whether it is brown (it spawns red), and the
/// flower a brown one ate (`stew_effects`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct MooshroomState {
    pub(crate) brown: bool,
    pub(crate) stew: Option<i32>,
}

impl Own for MooshroomState {}

/// The mooshroom.
pub(crate) struct Mooshroom;

/// `SuspiciousEffectHolder.tryGet(item)`: an item whose block is a flower.
fn is_suspicious(item: i32) -> bool {
    usize::try_from(item)
        .ok()
        .and_then(|at| clustine_data::ITEMS.get(at))
        .and_then(|known| known.block)
        .is_some_and(|state| is_class(state, BlockClass::FlowerBlock))
}

/// Its colour as its shearing's table names it (`mooshroom/variant`).
pub(crate) fn colour_name(mob: &Mob) -> &'static str {
    if animal::read::<MooshroomState>(&mob.state).1.brown {
        "brown"
    } else {
        "red"
    }
}

impl MobType for Mooshroom {
    fn register_goals(&self, setup: &mut Setup) {
        cow::register_goals(setup);
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        animal::walk_target_value(ai, pos, clustine_data::blocks::MYCELIUM)
    }

    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::custom_server_ai_step(tick);
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step(tick);
    }

    /// `MushroomCow.mobInteract`: a bowl, shears, a flower on a brown one, else a cow's.
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        let (animal, mut own) = animal::read::<MooshroomState>(&used.mob.state);
        let baby = animal.is_baby();
        let held = used.held.filter(|stack| stack.count > 0);
        let item = held.map(|stack| stack.item);
        if item == Some(items::BOWL) && !baby {
            let suspicious = own.stew.take().is_some();
            let result = if suspicious {
                items::SUSPICIOUS_STEW
            } else {
                items::MUSHROOM_STEW
            };
            used.mob.state = animal::write(&animal, &own);
            used.did.push(Did::Give {
                stack: ItemStack {
                    item: result,
                    count: 1,
                },
                no_duplicates: false,
            });
            used.did.push(Did::Sound {
                sound: if suspicious {
                    sounds::ENTITY_MOOSHROOM_SUSPICIOUS_MILK
                } else {
                    sounds::ENTITY_MOOSHROOM_MILK
                },
                source: SOUND_SOURCE_NEUTRAL,
                at: used.position,
                volume: 1.0,
                pitch: 1.0,
                but_the_player: false,
            });
            return Interaction::Success;
        }
        if item == Some(items::SHEARS) && !baby {
            // `shear(level, PLAYERS, stack)`: its sound, then the cow it becomes with its
            // mushrooms; `gameEvent(SHEAR, player)`.
            used.did.push(Did::Sound {
                sound: sounds::ENTITY_MOOSHROOM_SHEAR,
                source: animal::SOUND_SOURCE_PLAYERS,
                at: used.position,
                volume: 1.0,
                pitch: 1.0,
                but_the_player: false,
            });
            let facts = loot::Facts {
                mooshroom_variant: Some(colour_name(used.mob)),
                ..loot::Facts::default()
            };
            let drops = loot::named_drops(
                "minecraft:shearing/mooshroom",
                used.kind,
                baby,
                facts,
                used.draws,
            );
            used.did.push(Did::Convert {
                kind: entity_types::COW as u16,
                drops,
            });
            used.did.push(Did::GameEvent("minecraft:shear"));
            return Interaction::Success;
        }
        if own.brown && !baby {
            let Some(flower) = item.filter(|item| is_suspicious(*item)) else {
                return cow::milk_or_feed(used);
            };
            if own.stew.is_none() {
                // The flower is eaten (a creative player keeps it).
                own.stew = Some(flower);
                used.mob.state = animal::write(&animal, &own);
                used.did.push(Did::Sound {
                    sound: sounds::ENTITY_MOOSHROOM_EAT,
                    source: SOUND_SOURCE_NEUTRAL,
                    at: used.position,
                    volume: 2.0,
                    pitch: 1.0,
                    but_the_player: false,
                });
            }
            return Interaction::Success;
        }
        cow::milk_or_feed(used)
    }

    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        let (_, this) = animal::read::<MooshroomState>(&breeding.this.state);
        let (_, partner) = animal::read::<MooshroomState>(&breeding.partner.state);
        // `getOffspringVariant`: one of the same colour in 1024 is of the other.
        let brown = if this.brown == partner.brown && breeding.draws.next_int(1024) == 0 {
            !this.brown
        } else if breeding.draws.next_bool() {
            this.brown
        } else {
            partner.brown
        };
        let mut child = Mob::fresh();
        child.state = animal::write(
            &animal::Animal::default(),
            &MooshroomState { brown, stew: None },
        );
        Some(child)
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        animal::dimensions(mob, cow::BABY)
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(cow::classic_sounds()[0])
    }

    /// `AbstractCow.playStepSound` of the classic set, which a mooshroom keeps.
    fn step_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_COW_STEP)
    }

    fn ambient_interval(&self) -> u32 {
        120
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(cow::classic_sounds()[1])
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(cow::classic_sounds()[2])
    }

    fn sound_volume(&self) -> f32 {
        0.4
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let (_, own) = animal::read::<MooshroomState>(&mob.state);
        let mut values = animal::shown(mob);
        values.push((TYPE_INDEX, DataShown::Int(i32::from(own.brown)), !own.brown));
        values
    }
}
