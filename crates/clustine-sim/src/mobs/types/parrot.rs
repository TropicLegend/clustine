//! Parrots (`Parrot`, `ParrotWanderGoal`, `ShoulderRidingEntity`;
//! `docs/groundwork/n7-brief.md`, sections 5.0 and 5.8): a flying animal of five colours
//! that wanders among the trees, follows other mobs, imitates the monsters near it, is
//! tamed with seeds one time in ten, and once tamed sits and stands at its owner's
//! click while it is on the ground, follows its owner and flies to them. A cookie kills
//! it. It neither breeds nor grows.
//!
//! It flies without gravity while its move control steers it, as `FlyingMoveControl`
//! sets `NoGravity`; that flag is the parrot's own state here, shown to clients, and its
//! travel is given no gravity while it is set.
//!
//! Named differences: a parrot on a player's shoulder is the player's state, which the
//! lead owns: it lands on no shoulder (its goal holds its place and never starts); the
//! dance by a jukebox is the client's (the brief's finding 5.0.15); its flapping's sound
//! is the step sounds' part's; the poison of a cookie is not given (no effects yet), the
//! hurt that kills it is; the imitations are drawn from the types in the order the game
//! fills its map, where the game's own order changes from run to run (the brief's
//! finding 5.0.14).

use clustine_blocks::data::{in_tag, is_class, item_in_tag};
use clustine_data::{BlockClass, PathType, entity_types, sounds};
use clustine_world::BlockPos;
use serde::{Deserialize, Serialize};

use super::animal::{self, Own};
use super::tamable::{
    self, FollowOwnerGoal, Idle, Pet, SitWhenOrderedToGoal, TamablePanicGoal, Tame,
};
use super::{Did, Interaction, MobType, Spawning, Used};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    FloatGoal, FollowMobGoal, Items, LookAtPlayerGoal, Strolling, no_action_time,
};
use crate::mobs::ai::maths::{self, reduced_tick_delay};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::Class;
use crate::mobs::ai::{Ai, MoveKind, Operation, Setup, controls, navigation, random_pos};
use crate::mobs::body::Event;
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::MobTick;
use crate::physics::aabb::Aabb;
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::physics::{self, Vec3};
use crate::region::Random;
use crate::shown::DataShown;

/// The synced data's index of its variant, and of every entity's `NoGravity`.
const VARIANT_INDEX: u8 = 20;
const NO_GRAVITY_INDEX: u8 = 5;

/// `Parrot.Variant`'s count: red and blue, blue, green, yellow and blue, grey.
const VARIANTS: u32 = 5;

/// What a parrot keeps beyond an animal: its tameness, its variant, and whether it flies
/// without gravity.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ParrotState {
    pub(crate) tame: Tame,
    pub(crate) variant: i32,
    pub(crate) no_gravity: bool,
}

impl Own for ParrotState {}

/// The parrot.
pub(crate) struct Parrot;

pub(crate) fn state_of(mob: &Mob) -> (animal::Animal, ParrotState) {
    animal::read::<ParrotState>(&mob.state)
}

fn update(mob: &mut Mob, change: impl FnOnce(&mut ParrotState)) {
    let (shared, mut own) = state_of(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        mob.state = animal::write(&shared, &own);
    }
}

impl Pet for Parrot {
    fn tame(&self, mob: &Mob) -> Tame {
        state_of(mob).1.tame
    }

    fn set_tame(&self, mob: &mut Mob, tame: Tame) {
        update(mob, |own| own.tame = tame);
    }

    fn can_fly_to_owner(&self) -> bool {
        true
    }
}

/// `Parrot.MOB_SOUND_MAP` in the order the game fills it: each type a parrot imitates,
/// and its imitation (the happy ghast's is the silent `EMPTY`).
const IMITATIONS: [(i32, i32); 42] = [
    (entity_types::BLAZE, sounds::ENTITY_PARROT_IMITATE_BLAZE),
    (entity_types::BOGGED, sounds::ENTITY_PARROT_IMITATE_BOGGED),
    (entity_types::BREEZE, sounds::ENTITY_PARROT_IMITATE_BREEZE),
    (
        entity_types::CAMEL_HUSK,
        sounds::ENTITY_PARROT_IMITATE_CAMEL_HUSK,
    ),
    (
        entity_types::CAVE_SPIDER,
        sounds::ENTITY_PARROT_IMITATE_SPIDER,
    ),
    (
        entity_types::CREAKING,
        sounds::ENTITY_PARROT_IMITATE_CREAKING,
    ),
    (entity_types::CREEPER, sounds::ENTITY_PARROT_IMITATE_CREEPER),
    (entity_types::DROWNED, sounds::ENTITY_PARROT_IMITATE_DROWNED),
    (
        entity_types::ELDER_GUARDIAN,
        sounds::ENTITY_PARROT_IMITATE_ELDER_GUARDIAN,
    ),
    (
        entity_types::ENDER_DRAGON,
        sounds::ENTITY_PARROT_IMITATE_ENDER_DRAGON,
    ),
    (
        entity_types::ENDERMITE,
        sounds::ENTITY_PARROT_IMITATE_ENDERMITE,
    ),
    (entity_types::EVOKER, sounds::ENTITY_PARROT_IMITATE_EVOKER),
    (entity_types::GHAST, sounds::ENTITY_PARROT_IMITATE_GHAST),
    (entity_types::HAPPY_GHAST, sounds::INTENTIONALLY_EMPTY),
    (
        entity_types::GUARDIAN,
        sounds::ENTITY_PARROT_IMITATE_GUARDIAN,
    ),
    (entity_types::HOGLIN, sounds::ENTITY_PARROT_IMITATE_HOGLIN),
    (entity_types::HUSK, sounds::ENTITY_PARROT_IMITATE_HUSK),
    (
        entity_types::ILLUSIONER,
        sounds::ENTITY_PARROT_IMITATE_ILLUSIONER,
    ),
    (
        entity_types::MAGMA_CUBE,
        sounds::ENTITY_PARROT_IMITATE_MAGMA_CUBE,
    ),
    (entity_types::PARCHED, sounds::ENTITY_PARROT_IMITATE_PARCHED),
    (entity_types::PHANTOM, sounds::ENTITY_PARROT_IMITATE_PHANTOM),
    (entity_types::PIGLIN, sounds::ENTITY_PARROT_IMITATE_PIGLIN),
    (
        entity_types::PIGLIN_BRUTE,
        sounds::ENTITY_PARROT_IMITATE_PIGLIN_BRUTE,
    ),
    (
        entity_types::PILLAGER,
        sounds::ENTITY_PARROT_IMITATE_PILLAGER,
    ),
    (entity_types::RAVAGER, sounds::ENTITY_PARROT_IMITATE_RAVAGER),
    (entity_types::SHULKER, sounds::ENTITY_PARROT_IMITATE_SHULKER),
    (
        entity_types::SILVERFISH,
        sounds::ENTITY_PARROT_IMITATE_SILVERFISH,
    ),
    (
        entity_types::SKELETON,
        sounds::ENTITY_PARROT_IMITATE_SKELETON,
    ),
    (entity_types::SLIME, sounds::ENTITY_PARROT_IMITATE_SLIME),
    (entity_types::SPIDER, sounds::ENTITY_PARROT_IMITATE_SPIDER),
    (entity_types::STRAY, sounds::ENTITY_PARROT_IMITATE_STRAY),
    (entity_types::VEX, sounds::ENTITY_PARROT_IMITATE_VEX),
    (
        entity_types::VINDICATOR,
        sounds::ENTITY_PARROT_IMITATE_VINDICATOR,
    ),
    (entity_types::WARDEN, sounds::ENTITY_PARROT_IMITATE_WARDEN),
    (entity_types::WITCH, sounds::ENTITY_PARROT_IMITATE_WITCH),
    (entity_types::WITHER, sounds::ENTITY_PARROT_IMITATE_WITHER),
    (
        entity_types::WITHER_SKELETON,
        sounds::ENTITY_PARROT_IMITATE_WITHER_SKELETON,
    ),
    (entity_types::ZOGLIN, sounds::ENTITY_PARROT_IMITATE_ZOGLIN),
    (entity_types::ZOMBIE, sounds::ENTITY_PARROT_IMITATE_ZOMBIE),
    (
        entity_types::ZOMBIE_HORSE,
        sounds::ENTITY_PARROT_IMITATE_ZOMBIE_HORSE,
    ),
    (
        entity_types::ZOMBIE_NAUTILUS,
        sounds::ENTITY_PARROT_IMITATE_ZOMBIE_NAUTILUS,
    ),
    (
        entity_types::ZOMBIE_VILLAGER,
        sounds::ENTITY_PARROT_IMITATE_ZOMBIE_VILLAGER,
    ),
];

/// `getImitatedSound(type)`: its imitation, else the parrot's own ambient sound.
fn imitated_sound(kind: i32) -> i32 {
    IMITATIONS
        .iter()
        .find(|(imitated, _)| *imitated == kind)
        .map_or(sounds::ENTITY_PARROT_AMBIENT, |(_, sound)| *sound)
}

/// `getPitch(random)`: about 1.
pub(crate) fn pitch(draws: &mut Random) -> f32 {
    (draws.next_float() - draws.next_float()) * 0.2 + 1.0
}

/// `Parrot.imitateNearbyMobs(level, parrot)`, the level's draws on its behalf: one time
/// in two, one of the mobs it imitates within twenty blocks, its imitation at the parrot
/// at 0.7. Whether it imitated.
pub(crate) fn imitate_nearby_mobs(tick: &mut MobTick<'_>) -> bool {
    if tick.mob.body.vitals.dying() || tick.draws.next_int(2) != 0 {
        return false;
    }
    let living = &tick.living.body;
    let bounds =
        Aabb::of_entity(living.position, living.width, living.height).inflate([20.0, 20.0, 20.0]);
    let near: Vec<u16> = tick
        .world
        .entities_in(&bounds)
        .iter()
        .filter(|seen| matches!(seen.entity.data, clustine_world::EntityData::Mob { .. }))
        .map(|seen| seen.entity.kind)
        .filter(|kind| {
            IMITATIONS
                .iter()
                .any(|(imitated, _)| *imitated == i32::from(*kind))
        })
        .collect();
    if near.is_empty() {
        return false;
    }
    let chosen = near[tick.draws.next_int(near.len() as u32) as usize];
    let sound = imitated_sound(i32::from(chosen));
    let pitch = pitch(tick.draws);
    tick.out.events.push(Event::Sound {
        sound,
        volume: 0.7,
        pitch,
    });
    true
}

/// `ParrotWanderGoal(parrot, 1.0)`: a flying stroll that goes, now and then, to a spot
/// among leaves or logs near.
#[derive(Clone, Copy)]
struct ParrotWanderGoal;

/// `WaterAvoidingRandomStrollGoal`'s chance of a stroll anywhere.
const PROBABILITY: f32 = 0.001;

impl ParrotWanderGoal {
    /// `getPosition()`: in water, somewhere on land (which the tree's draw then takes
    /// over); one time but in a thousand, a tree's spot; else the flyer's.
    fn position(ai: &mut Ai<'_, '_>) -> Option<Vec3> {
        let mut found = None;
        if ai.in_water() {
            found = random_pos::land_pos(ai, 15, 15);
        }
        if ai.tick.draws.next_float() >= PROBABILITY {
            found = Self::tree_pos(ai);
        }
        found.or_else(|| Self::flying_pos(ai))
    }

    /// `getTreePos()`: the first block of the box six high and three about it (by x, then
    /// y, then z) but its own, of air with air above and leaves or a log below; its
    /// bottom middle.
    fn tree_pos(ai: &Ai<'_, '_>) -> Option<Vec3> {
        let own = ai.block_position();
        let [x, y, z] = ai.position();
        let (x0, y0, z0) = (
            physics::floor(x - 3.0),
            physics::floor(y - 6.0),
            physics::floor(z - 3.0),
        );
        let (x1, y1, z1) = (
            physics::floor(x + 3.0),
            physics::floor(y + 6.0),
            physics::floor(z + 3.0),
        );
        let world = ai.tick.world;
        for bz in z0..=z1 {
            for by in y0..=y1 {
                for bx in x0..=x1 {
                    let pos = BlockPos::new(bx, by, bz);
                    if pos == own {
                        continue;
                    }
                    let below = world.state(BlockPos::new(bx, by - 1, bz));
                    let perch =
                        is_class(below, BlockClass::LeavesBlock) || in_tag(below, "minecraft:logs");
                    if perch
                        && world.state(pos).is_air()
                        && world.state(BlockPos::new(bx, by + 1, bz)).is_air()
                    {
                        return Some([f64::from(bx) + 0.5, f64::from(by), f64::from(bz) + 0.5]);
                    }
                }
            }
        }
        None
    }

    /// `WaterAvoidingRandomFlyingGoal.getPosition()`: hovering ahead, else in the air or
    /// water.
    fn flying_pos(ai: &mut Ai<'_, '_>) -> Option<Vec3> {
        let pitch = ai.tick.pitch * maths::RADIANS;
        let yaw = -ai.tick.yaw * maths::RADIANS;
        let (yaw_cos, yaw_sin) = (maths::table_cos(yaw), maths::table_sin(yaw));
        let pitch_cos = maths::table_cos(pitch);
        let view = [
            f64::from(yaw_sin * pitch_cos),
            f64::from(yaw_cos * pitch_cos),
        ];
        random_pos::hover_pos(ai, 8, 7, view[0], view[1], 1.570_796_4, 3, 1).or_else(|| {
            random_pos::air_and_water_pos(ai, 8, 4, -2, view[0], view[1], 1.570_796_370_506_286_6)
        })
    }
}

impl Goal for ParrotWanderGoal {
    type State = Strolling;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, state: &mut Strolling, ai: &mut Ai<'_, '_>) -> bool {
        if !state.forced {
            if no_action_time(ai) >= 100 {
                return false;
            }
            let bound = reduced_tick_delay(120).max(1) as u32;
            if ai.tick.draws.next_int(bound) != 0 {
                return false;
            }
        }
        let Some(wanted) = Self::position(ai) else {
            return false;
        };
        state.wanted = wanted;
        state.forced = false;
        true
    }

    fn can_continue(&self, _: &mut Strolling, ai: &mut Ai<'_, '_>) -> bool {
        !navigation::is_done(ai)
    }

    fn start(&self, state: &mut Strolling, ai: &mut Ai<'_, '_>) {
        navigation::move_to(ai, state.wanted, 1.0);
    }

    fn stop(&self, _: &mut Strolling, ai: &mut Ai<'_, '_>) {
        navigation::stop(ai);
    }
}

/// `Parrot.mobInteract` (the brief's 5.8).
fn mob_interact(used: &mut Used<'_>) -> Interaction {
    let (_, own) = state_of(used.mob);
    let held = used.held.as_ref().filter(|stack| stack.count > 0);
    let is = |tag: &str| held.is_some_and(|stack| item_in_tag(stack.item, tag));
    if !own.tame.is_tame() && is("minecraft:parrot_food") {
        let pitch = pitch(used.draws);
        used.did.push(Did::Sound {
            sound: sounds::ENTITY_PARROT_EAT,
            source: SOUND_SOURCE_NEUTRAL,
            at: used.position,
            volume: 1.0,
            pitch,
            but_the_player: false,
        });
        if used.draws.next_int(10) == 0 {
            let player = used.player_uuid;
            update(used.mob, |own| tamable::tame(&mut own.tame, player));
            used.did.push(Did::Event(7));
        } else {
            used.did.push(Did::Event(6));
        }
        return Interaction::Success;
    }
    if is("minecraft:parrot_poisonous_food") {
        // Every player is creative: the hurt comes whatever the parrot is.
        used.did.push(Did::HurtByPlayer { amount: f32::MAX });
        return Interaction::Success;
    }
    if used.on_ground && own.tame.is_tame() && own.tame.is_owned_by(used.player_uuid) {
        update(used.mob, |own| {
            own.tame.ordered_to_sit = !own.tame.ordered_to_sit
        });
        return Interaction::Success;
    }
    animal::mob_interact(used, Items::Of(&[]), None)
}

impl MobType for Parrot {
    fn register_goals(&self, setup: &mut Setup) {
        setup.config.navigation = navigation::Settings::of(navigation::Kind::Flying);
        setup.config.navigation.flags.can_open_doors = false;
        setup.config.move_kind = MoveKind::Flying {
            max_turn: 10,
            hovers: false,
        };
        setup.set_malus(PathType::FireInNeighbor, -1.0);
        setup.set_malus(PathType::Fire, -1.0);
        setup.set_malus(PathType::Cocoa, -1.0);
        setup.add_goal(
            0,
            TamablePanicGoal {
                pet: &Parrot,
                speed: 1.25,
                environmental: false,
            },
        );
        setup.add_goal(0, FloatGoal);
        setup.add_goal(1, LookAtPlayerGoal::new(Class::Player, 8.0));
        setup.add_goal(2, SitWhenOrderedToGoal { pet: &Parrot });
        setup.add_goal(
            2,
            FollowOwnerGoal {
                pet: &Parrot,
                speed: 1.0,
                start: 5.0,
                stop: 1.0,
            },
        );
        setup.add_goal(2, ParrotWanderGoal);
        // `LandOnOwnersShoulderGoal`: the shoulder is the player's (the module's comment).
        setup.add_goal(3, Idle { flags: 0 });
        setup.add_goal(
            3,
            FollowMobGoal {
                speed: 1.0,
                stop_distance: 3.0,
                area_size: 7.0,
            },
        );
    }

    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::custom_server_ai_step(tick);
    }

    /// `FlyingMoveControl.tick()`, and its `NoGravity`: off while it waits (it does not
    /// hover in place), on while it steers.
    fn tick_move_control(&self, ai: &mut Ai<'_, '_>) {
        let steering = ai.tick.mob.body.controls.movement.operation == Operation::MoveTo;
        controls::tick_move(ai);
        update(&mut ai.tick.mob, |own| own.no_gravity = steering);
        ai.tick.living.attributes.gravity = if steering {
            0.0
        } else {
            ai.attribute(clustine_data::attributes::GRAVITY)
        };
    }

    /// `Parrot.aiStep` before `LivingEntity`'s: one time in four hundred, the level's
    /// draw, an imitation.
    fn before_ai_step(&self, tick: &mut MobTick<'_>) {
        if state_of(&tick.mob).1.no_gravity {
            tick.living.attributes.gravity = 0.0;
        }
        if tick.draws.next_int(400) == 0 {
            imitate_nearby_mobs(tick);
        }
    }

    /// `calculateFlapping()`'s slowed fall: off the ground and falling, its fall is six
    /// tenths.
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step(tick);
        let body = &mut tick.living.body;
        if !body.on_ground && body.motion[1] < 0.0 {
            body.motion[1] *= 0.6;
        }
    }

    fn before_hurt(&self, mob: &mut Mob) {
        tamable::update(&Parrot, mob, |tame| tame.ordered_to_sit = false);
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        mob_interact(used)
    }

    /// `Parrot.finalizeSpawn`: its variant first, then its base's with a group that
    /// wants no babies (`AgeableMobGroupData(false)`) where it has none.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let variant = spawn.level.next_int(VARIANTS) as i32;
        update(spawn.mob, |own| own.variant = variant);
        let group = super::SpawnGroup::Ageable(super::AgeableGroup::new(false));
        // A parrot is never a baby (`canBeABaby`), and its group wants none.
        let _ = animal::ageable_draw(spawn, group);
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_PARROT_AMBIENT)
    }

    /// `Parrot.getAmbient(level, random)`, the level's draws: one time in a thousand an
    /// imitation of any type it imitates, else its own.
    fn ambient_sound_drawn(
        &self,
        _: &Mob,
        draws: &mut Random,
        _: i64,
        _: u32,
        _: Option<(bool, bool)>,
    ) -> Option<i32> {
        if draws.next_int(1000) == 0 {
            let (kind, _) = IMITATIONS[draws.next_int(IMITATIONS.len() as u32) as usize];
            return Some(imitated_sound(kind));
        }
        Some(sounds::ENTITY_PARROT_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_PARROT_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_PARROT_DEATH)
    }

    fn pet(&self) -> Option<&'static dyn Pet> {
        Some(&Parrot)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let (_, own) = state_of(mob);
        let mut values = animal::shown(mob);
        values.push((
            NO_GRAVITY_INDEX,
            DataShown::Boolean(own.no_gravity),
            !own.no_gravity,
        ));
        values.extend(own.tame.shown());
        values.push((VARIANT_INDEX, DataShown::Int(own.variant), own.variant == 0));
        values
    }
}
