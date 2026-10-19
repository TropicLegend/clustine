//! Chickens (`Chicken`; `docs/groundwork/n7-brief.md`, section 4.11): an animal of seeds
//! that falls slowly, lays an egg of its variant's colour every five to ten minutes, and
//! is of a temperature variant drawn by its biome and a sound variant drawn at random.
//!
//! Its egg timer counts only while it is grown and alive, so it is kept as a stamp while
//! it counts and as a value while it waits. Named differences: its flapping is not heard
//! by sculk (the body keeps no distance flown), and no chicken is a jockey before the
//! zombies of N9 ride them.

use clustine_data::{PathType, sounds};
use serde::{Deserialize, Serialize};

use super::animal::{self, Own, variants};
use super::{Breeding, Dimensions, Interaction, MobType, Spawning, Used, cow};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    FloatGoal, Items, LookAtPlayerGoal, PanicGoal, RandomLookAroundGoal, RandomStrollGoal,
    TemptGoal,
};
use crate::mobs::ai::sensing::Class;
use crate::mobs::ai::{Ai, Setup};
use crate::mobs::body::hurt::{DamageSource, GameEvent};
use crate::mobs::body::{Event, loot};
use crate::mobs::tick::{Local, MobTick};
use crate::region::Random;
use crate::shown::{DataShown, Holder};
use clustine_world::BlockPos;

const VARIANTS: &str = "minecraft:chicken_variant";
const SOUND_VARIANTS: &str = "minecraft:chicken_sound_variant";

/// `#chicken_food`.
const FOOD: Items = Items::Tag("minecraft:chicken_food");

/// A chick's size.
const BABY: Dimensions = Dimensions {
    width: 0.3,
    height: 0.4,
    eye_height: 0.281_25,
};

const VARIANT_INDEX: u8 = 18;
const SOUND_VARIANT_INDEX: u8 = 19;

/// What a chicken keeps beyond an animal: its variants; its egg timer (`eggTime`), the
/// value it had at the moment `egg_at` and, while `counting`, down by one a tick from
/// there; and whether it is a jockey.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ChickenState {
    pub(crate) variant: u8,
    pub(crate) sound: u8,
    pub(crate) egg: i32,
    pub(crate) egg_at: u32,
    pub(crate) counting: bool,
    pub(crate) jockey: bool,
}

impl Default for ChickenState {
    fn default() -> Self {
        Self {
            variant: variants::id(VARIANTS, "minecraft:temperate"),
            sound: variants::id(SOUND_VARIANTS, "minecraft:classic"),
            egg: 0,
            egg_at: 0,
            counting: false,
            jockey: false,
        }
    }
}

impl Own for ChickenState {}

impl ChickenState {
    /// `eggTime` at the moment `now`.
    fn egg_time(&self, now: u32) -> i32 {
        if self.counting {
            let gone = (i64::from(now) - i64::from(self.egg_at)).max(0);
            (i64::from(self.egg) - gone).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
        } else {
            self.egg
        }
    }
}

/// The chicken.
pub(crate) struct Chicken;

/// `nextInt(6000) + 6000`: the next egg's time.
fn egg_timer(draws: &mut Random) -> i32 {
    draws.next_int(6000) as i32 + 6000
}

/// Its variant's name, as its laying's table names it (`chicken/variant`).
fn variant_name(own: &ChickenState) -> &'static str {
    clustine_data::synced_registry(VARIANTS)
        .and_then(|known| known.entries.get(usize::from(own.variant)))
        .copied()
        .unwrap_or("minecraft:temperate")
}

/// The sounds of its sound variant's adult or baby set: ambient, hurt, death.
fn sound_set(mob: &Mob) -> [i32; 3] {
    if animal::is_baby(mob) {
        return [
            sounds::ENTITY_BABY_CHICKEN_AMBIENT,
            sounds::ENTITY_BABY_CHICKEN_HURT,
            sounds::ENTITY_BABY_CHICKEN_DEATH,
        ];
    }
    let (_, own) = animal::read::<ChickenState>(&mob.state);
    if own.sound == variants::id(SOUND_VARIANTS, "minecraft:picky") {
        [
            sounds::ENTITY_CHICKEN_PICKY_AMBIENT,
            sounds::ENTITY_CHICKEN_PICKY_HURT,
            sounds::ENTITY_CHICKEN_PICKY_DEATH,
        ]
    } else {
        [
            sounds::ENTITY_CHICKEN_AMBIENT,
            sounds::ENTITY_CHICKEN_HURT,
            sounds::ENTITY_CHICKEN_DEATH,
        ]
    }
}

impl MobType for Chicken {
    fn register_goals(&self, setup: &mut Setup) {
        cow::animal_maluses(setup);
        setup.set_malus(PathType::Water, 0.0);
        setup.add_goal(0, FloatGoal);
        setup.add_goal(1, PanicGoal { speed: 1.4 });
        setup.add_goal(2, animal::BreedGoal { speed: 1.0 });
        setup.add_goal(3, TemptGoal::new(1.0, FOOD, false));
        setup.add_goal(4, animal::FollowParentGoal { speed: 1.1 });
        setup.add_goal(5, RandomStrollGoal::avoiding_water(1.0));
        setup.add_goal(6, LookAtPlayerGoal::new(Class::Player, 6.0));
        setup.add_goal(7, RandomLookAroundGoal);
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        animal::walk_target_value(ai, pos, clustine_data::blocks::GRASS_BLOCK)
    }

    fn construct(&self, mob: &mut Mob, own: &mut Random) {
        let (animal, mut state) = animal::read::<ChickenState>(&mob.state);
        state.egg = egg_timer(own);
        mob.state = animal::write(&animal, &state);
    }

    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::custom_server_ai_step(tick);
    }

    /// `Chicken.aiStep` after `Animal`'s: a fall slowed to six tenths, and the egg
    /// timer, which counts only for a grown chicken alive and no jockey; at its end an
    /// egg of its table at its feet, with its sound and `ENTITY_PLACE`, and a new timer
    /// drawn whether or not an egg came.
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step(tick);
        let body = &mut tick.living.body;
        if !body.on_ground && body.motion[1] < 0.0 {
            body.motion[1] *= 0.6;
        }
        let now = tick.lived;
        let next = now.saturating_add(1);
        let (animal, mut own) = animal::read::<ChickenState>(&tick.mob.state);
        let before = own.clone();
        let counts = !tick.mob.body.vitals.dying() && !animal.is_baby() && !own.jockey;
        if counts {
            if !own.counting {
                own.counting = true;
                own.egg_at = now;
            }
            // `--eggTime <= 0`.
            if own.egg_time(next) <= 0 {
                let facts = loot::Facts {
                    chicken_variant: Some(variant_name(&own)),
                    ..loot::Facts::default()
                };
                let eggs = loot::named_drops(
                    "minecraft:gameplay/chicken_lay",
                    tick.kind,
                    false,
                    facts,
                    tick.draws,
                );
                let at = tick.living.body.position;
                let mut laid = false;
                for stack in eggs {
                    if stack.count > 0 {
                        laid = true;
                        tick.out.local.push(Local::Drop {
                            stack,
                            at,
                            pick_up_delay: crate::region::DEFAULT_PICK_UP_DELAY,
                        });
                    }
                }
                if laid {
                    let pitch = (tick.draws.next_float() - tick.draws.next_float()) * 0.2 + 1.0;
                    tick.out.events.push(Event::Sound {
                        sound: sounds::ENTITY_CHICKEN_EGG,
                        volume: 1.0,
                        pitch,
                    });
                    tick.out
                        .events
                        .push(Event::GameEvent(GameEvent::EntityPlace));
                }
                own.egg = egg_timer(tick.draws);
                own.egg_at = next;
            }
        } else if own.counting {
            own.egg = own.egg_time(now);
            own.counting = false;
        }
        if own != before {
            tick.mob.state = animal::write(&animal, &own);
        }
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        animal::mob_interact(used, FOOD, None)
    }

    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let (animal, mut own) = animal::read::<ChickenState>(&spawn.mob.state);
        own.variant = variants::select(VARIANTS, spawn.biome, spawn.level);
        own.sound = spawn.level.next_int(variants::count(SOUND_VARIANTS)) as u8;
        spawn.mob.state = animal::write(&animal, &own);
    }

    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        let (_, this) = animal::read::<ChickenState>(&breeding.this.state);
        let (_, partner) = animal::read::<ChickenState>(&breeding.partner.state);
        let variant = if breeding.draws.next_bool() {
            this.variant
        } else {
            partner.variant
        };
        let mut child = Mob::fresh();
        child.state = animal::write(
            &animal::Animal::default(),
            &ChickenState {
                variant,
                ..ChickenState::default()
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

    /// A jockey despawns as a monster does; a chicken is kept.
    fn remove_when_far_away(&self, _: u16, mob: &Mob, _: u32, _: f64) -> bool {
        animal::read::<ChickenState>(&mob.state).1.jockey
    }

    /// A jockey's is 10.
    fn experience_reward(&self, kind: u16, mob: &Mob, draws: &mut Random) -> i32 {
        if animal::read::<ChickenState>(&mob.state).1.jockey {
            10
        } else {
            crate::mobs::body::hurt::base_experience_reward(kind, draws)
        }
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

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let (_, own) = animal::read::<ChickenState>(&mob.state);
        let fresh = ChickenState::default();
        let mut values = animal::shown(mob);
        values.push((
            VARIANT_INDEX,
            DataShown::Holder(Holder::ChickenVariant, i32::from(own.variant)),
            own.variant == fresh.variant,
        ));
        values.push((
            SOUND_VARIANT_INDEX,
            DataShown::Holder(Holder::ChickenSoundVariant, i32::from(own.sound)),
            own.sound == fresh.sound,
        ));
        values
    }
}
