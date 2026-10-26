//! What each type of mob adds to the body and the mind: its goals, its own step, its
//! interactions, its spawn and its babies (`docs/groundwork/n7-plan.md`, section 11.3,
//! item 8). Each type is a module of its own that the parts from N7e on add; [`of`] is
//! the one place a type id finds its module.
//!
//! The farm animals (part N7e: cows, mooshrooms, pigs, sheep, chickens and rabbits) share
//! what `AgeableMob` and `Animal` add, which [`animal`] holds; each has a module of its
//! own beside it. The water creatures (part N7i: the four fish, the two squids, the
//! dolphin and the turtle) share what [`water`] holds, the fish what [`fish`] holds.
//!
//! The horse family (part N7h: horses, donkeys, mules, llamas, trader llamas, skeleton
//! and zombie horses) shares what `AbstractHorse` and `AbstractChestedHorse` add, which
//! [`equine`] holds.
//!
//! The wild land animals (part N7i: the fox, the panda, the polar bear, the bee and the
//! bat) are animals but for the bat; the bear and the bee are angry as [`neutral`] has
//! it.
//!
//! The animals of N7j have brains, which come with its machinery: until then each has
//! its body, its state and its interactions, and its module names the seam its brain's
//! provider will be wired at. They are the amphibians (part B: the axolotl, the frog and
//! the tadpole), the highland animals (part C: the goat, the camel, the camel husk and
//! the armadillo), the fliers (part D: the sniffer, the allay and the happy ghast, whose
//! grown adult the game drives by goals) and the keepers (part E: the nautilus, the
//! zombie nautilus and the copper golem).

pub(crate) mod allay;
#[cfg(test)]
mod amphibian_brain_tests;
#[cfg(test)]
mod amphibian_tests;
pub(crate) mod animal;
pub(crate) mod armadillo;
pub(crate) mod axolotl;
pub(crate) mod axolotl_ai;
pub(crate) mod bat;
pub(crate) mod bee;
pub(crate) mod bogged;
pub(crate) mod camel;
pub(crate) mod camel_husk;
pub(crate) mod cat;
pub(crate) mod cave_spider;
pub(crate) mod chicken;
pub(crate) mod cod;
pub(crate) mod copper_golem;
pub(crate) mod cow;
pub(crate) mod creeper;
pub(crate) mod dolphin;
pub(crate) mod donkey;
pub(crate) mod drowned;
pub(crate) mod elder_guardian;
pub(crate) mod equine;
pub(crate) mod evoker;
pub(crate) mod fish;
#[cfg(test)]
mod flier_tests;
pub(crate) mod fox;
pub(crate) mod frog;
pub(crate) mod frog_ai;
#[cfg(test)]
mod generation_tests;
pub(crate) mod glow_squid;
pub(crate) mod goat;
pub(crate) mod golem_transport;
pub(crate) mod guardian;
pub(crate) mod happy_ghast;
#[cfg(test)]
mod highland_tests;
pub(crate) mod horse;
pub(crate) mod husk;
pub(crate) mod iron_golem;
#[cfg(test)]
mod keeper_brain_tests;
#[cfg(test)]
mod keeper_tests;
pub(crate) mod llama;
pub(crate) mod mooshroom;
pub(crate) mod mule;
pub(crate) mod nautilus;
pub(crate) mod neutral;
pub(crate) mod ocelot;
pub(crate) mod panda;
pub(crate) mod parched;
pub(crate) mod parrot;
pub(crate) mod pig;
pub(crate) mod pillager;
pub(crate) mod polar_bear;
pub(crate) mod pufferfish;
pub(crate) mod rabbit;
pub(crate) mod salmon;
pub(crate) mod sheep;
pub(crate) mod skeleton;
pub(crate) mod skeleton_horse;
pub(crate) mod sniffer;
pub(crate) mod snow_golem;
pub(crate) mod spider;
pub(crate) mod squid;
pub(crate) mod stray;
pub(crate) mod tadpole;
pub(crate) mod tadpole_ai;
pub(crate) mod tamable;
#[cfg(test)]
mod tamed_tests;
pub(crate) mod template;
#[cfg(test)]
mod tests;
pub(crate) mod trader_llama;
pub(crate) mod tropical_fish;
pub(crate) mod turtle;
pub(crate) mod villager;
pub(crate) mod vindicator;
pub(crate) mod wandering_trader;
pub(crate) mod water;
#[cfg(test)]
mod water_tests;
#[cfg(test)]
mod wild_tests;
pub(crate) mod witch;
pub(crate) mod wolf;
pub(crate) mod zombie;
pub(crate) mod zombie_horse;
pub(crate) mod zombie_nautilus;
pub(crate) mod zombie_villager;

use clustine_data::{entity_types, sounds};
use clustine_world::{BlockPos, Entity, EntityData, EntityId, ItemStack};

use super::Mob;
use super::ai::{Ai, HeadLimits, Setup, controls};
use super::body::hurt::{DamageSource, Struck};
use super::body::{ladder, loot};
use super::tick::MobTick;
use crate::living::Travel;
use crate::physics::Vec3;
use crate::region::Random;

/// What a type of mob does beyond what every mob does. Every hook has the game's
/// default, which a type that does nothing more keeps.
pub(crate) trait MobType: Sync {
    /// `registerGoals`, `createNavigation` and what the constructor sets of the AI: the
    /// goals and target goals in the order the game adds them (each slot's part is its
    /// place in that order, `mind_ids::FIRST_GOAL` on, the target goals after the
    /// goals), the navigation, the maluses, the controls. Called in every AI step, as
    /// nothing of it is kept.
    fn register_goals(&self, setup: &mut Setup) {
        let _ = setup;
    }

    /// `registerGoals` where what the type adds depends on the mob (a fox's hunting,
    /// which its variant orders and which a bred fox lacks): by default the type's
    /// [`MobType::register_goals`].
    fn register_goals_of(&self, mob: &Mob, setup: &mut Setup) {
        let _ = mob;
        self.register_goals(setup);
    }

    /// Whether the navigation ticks in this AI step: `PathNavigation.tick` of the
    /// type's navigation (a pollinating bee's does nothing).
    fn navigation_ticks(&self, mob: &Mob) -> bool {
        let _ = mob;
        true
    }

    /// `lookControl.tick()` of the type's look control (a sleeping fox's turns nothing,
    /// a pouncing one's keeps its pitch).
    fn tick_look_control(&self, ai: &mut Ai<'_, '_>) {
        controls::tick_look(ai);
    }

    /// `isNoGravity()` as the type keeps it (a bee from its first move on): no gravity
    /// pulls it.
    fn no_gravity(&self, mob: &Mob) -> bool {
        let _ = mob;
        false
    }

    /// `omnidirectionalAirMover()`: the air slows it up and down as it does sideways
    /// (a bee's).
    fn omnidirectional_air(&self) -> bool {
        false
    }

    /// `getWaterSlowDown()`: 0.8 in `LivingEntity` (a polar bear's 0.98).
    fn water_slow_down(&self) -> f32 {
        0.8
    }

    /// `isPushable()`, and whether its `pushEntities` pushes anything (a bat neither).
    fn pushable(&self) -> bool {
        true
    }

    /// `playAmbientSound()`, once its chance came: `Mob`'s (the sound as
    /// [`MobType::ambient_sound_drawn`] gives it, at its volume and voice), unless the
    /// type plays its own (a fox's screech at twice the volume, which reads the world).
    fn play_ambient_sound(&self, tick: &mut MobTick<'_>) {
        super::body::play_ambient_sound(tick);
    }

    /// What the type's `getVoicePitch()` scales `LivingEntity`'s by (a bat's 0.95).
    fn voice_pitch_factor(&self) -> f32 {
        1.0
    }

    /// `getMaxHeadXRot`, `getMaxHeadYRot`, `getHeadRotSpeed`.
    fn head(&self) -> HeadLimits {
        HeadLimits::default()
    }

    /// `PathfinderMob.getWalkTargetValue(pos)`: how much the mob likes a place it might
    /// wander to (0 for every place, unless the type says otherwise: an animal's
    /// prefers grass and light).
    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        let _ = (ai, pos);
        0.0
    }

    /// `customServerAiStep`: what the type does after its navigation and before its
    /// controls.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        let _ = tick;
    }

    /// `mobInteract(player, hand)`: what a player's use of the mob does that is the
    /// type's own (feeding, milking, mounting). `Mob`'s passes.
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        let _ = used;
        Interaction::Pass
    }

    /// `ItemStack.interactLivingEntity` of the item used on a mob of the type, once the
    /// mob's own use passed: a dye on a sheep. `Item`'s passes.
    fn item_interact(&self, used: &mut Used<'_>) -> Interaction {
        let _ = used;
        Interaction::Pass
    }

    /// What the type's constructor sets and draws beyond the body's (a chicken's egg
    /// timer, a rabbit's idle timer and its wanted position), from the new mob's own
    /// stream: every mob of the type is made so, by an egg, a breeding or a conversion.
    fn construct(&self, mob: &mut Mob, own: &mut Random) {
        let _ = (mob, own);
    }

    /// The type's brain (`makeBrain`'s provider), for a type whose AI is one
    /// (`mobs/brain.rs`): none for the types of goals. A type with one ticks it from its
    /// `custom_server_ai_step` by [`crate::mobs::brain::run`].
    fn brain(&self) -> Option<&'static crate::mobs::brain::Provider> {
        None
    }

    /// The type's own `finalizeSpawn`, before `Mob`'s (the body's: the follow range's
    /// bonus and the hand), with the level's draws on the mob's behalf: its variant,
    /// its colour, a baby of a group. `Mob`'s adds nothing.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let _ = spawn;
    }

    /// What the type's `finalizeSpawn` does after `Mob`'s, which it calls first (a
    /// tropical fish's variant, of the level's draws after `Mob`'s).
    fn after_finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let _ = spawn;
    }

    /// `Mob.doHurtTarget(level, target)` of a melee attack (`MeleeAttackGoal`): the
    /// attack damage, with no knockback of its own, by default (`goals::melee_hit`); the
    /// iron golem's own hit with its fling.
    fn do_hurt_target(&self, ai: &mut Ai<'_, '_>, target: &super::ai::sensing::Looked) {
        super::ai::goals::melee_hit(ai, target);
    }

    /// `Mob.setBaby(baby)`, which a conversion calls on the new mob where the old one was
    /// a baby: nothing for a type that is never one.
    fn set_baby(&self, mob: &mut Mob, baby: bool) {
        let _ = (mob, baby);
    }

    /// The step this type hands `convertTo` for after the new mob of the type `into` is
    /// made and before it is added (`ConversionTracker`'s `after`, K 11.5): the zombie's
    /// `handleAttributes(CONVERSION)`, of the new mob's own stream `own`. Nothing by
    /// default.
    fn finalize_conversion(&self, into: u16, new: &mut Mob, own: &mut Random) {
        let _ = (into, new, own);
    }

    /// What the type's `readAdditionalSaveData` takes of a structure template's tag (a
    /// compound, its type byte first, as [`template`] reads it) that outlasts the
    /// `finalizeSpawn` after it: a sheep's colour, a cat's collar. What every mob reads
    /// ([`from_template`]: its persistence, its health, its attributes) is read before
    /// it; a type that keeps nothing of its own reads nothing.
    #[expect(
        clippy::wrong_self_convention,
        reason = "the name says what it reads from, as `readAdditionalSaveData` reads a tag"
    )]
    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        let _ = (mob, tag);
    }

    /// `getBreedOffspring(level, partner)`: the child of a breeding (or of a spawn egg
    /// used on the mob, its own partner), as `EntityType.create` makes it, with what it
    /// takes of its parents; `None` where the type has none.
    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        let _ = breeding;
        None
    }

    /// What the type's `hurtServer` does once a hurt is not refused for the mob's being
    /// invulnerable to it, before `LivingEntity`'s (a wolf or a parrot stands up).
    fn before_hurt(&self, mob: &mut Mob) {
        let _ = mob;
    }

    /// What the type's `aiStep` does after `LivingEntity.aiStep` (its travel and its
    /// pushes): an ageable mob's growing, an animal's love, a chicken's slow fall and
    /// eggs, a rabbit's hop ending.
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        let _ = tick;
    }

    /// Whether a player riding it first, holding `hands` (the main hand's, the off
    /// hand's), steers it (`getControllingPassenger`): never but where the type says. The
    /// region asks every tick and at every move, as the game asks, so a change of what
    /// the player holds hands the mob back to its own mind at once
    /// (`docs/groundwork/n7h-riding-plan.md`, section 3 item 2).
    fn steered_by(&self, mob: &Mob, hands: [Option<ItemStack>; 2]) -> bool {
        let _ = (mob, hands);
        false
    }

    /// `addPassenger`'s own part, before a seat is added to `vehicle` (its record, its
    /// passengers as they were): what it does to itself, and a sound it makes at itself
    /// at volume 1 and pitch 1 (a happy ghast's goggles coming down), with `world` to read
    /// the players above it. Nothing but where the type has its own.
    fn passenger_added(
        &self,
        vehicle: &clustine_world::Entity,
        mob: &mut Mob,
        world: &dyn super::view::MobWorld,
    ) -> Option<i32> {
        let _ = (vehicle, mob, world);
        None
    }

    /// `canAddPassenger(passenger)` of a mob carrying `passengers` (seats kept for players
    /// on their way counted): one seat but where the type has more (a camel's three, a
    /// happy ghast's four).
    fn can_add_passenger(&self, mob: &Mob, passengers: usize) -> bool {
        let _ = mob;
        passengers == 0
    }

    /// `getPassengerAttachmentPoint(passenger, …)` where the type has its own: the seat of
    /// the `index`th of `count` passengers (of the type `passenger`), **already turned by
    /// the vehicle's yaw**, at the game time `game_time` (a camel's by its pose). `None`
    /// keeps the type's extracted points, and a horse's rearing offset.
    fn seat(
        &self,
        vehicle: &clustine_world::Entity,
        mob: &Mob,
        index: usize,
        count: usize,
        passenger: u16,
        game_time: i64,
    ) -> Option<crate::physics::Vec3> {
        let _ = (vehicle, mob, index, count, passenger, game_time);
        None
    }

    /// `handlePlayerCommand(START_RIDING_JUMP)` of a mob its rider steers, other than the
    /// horse family's (`canJump`, then `handleStartJump`): what it does to itself and what
    /// it is heard to do, or `None` where it cannot jump (sitting, unsaddled, no jump at
    /// all), at the mob's moment `moment` and the game time `game_time` (a camel's pose is
    /// reckoned by it). Nothing but where the type has its own.
    fn riding_jump(
        &self,
        mob: &mut Mob,
        power: i32,
        moment: u32,
        game_time: i64,
    ) -> Option<JumpStart> {
        let _ = (mob, power, moment, game_time);
        None
    }

    /// What a mob generation made is given when a region first takes it in, at the game
    /// time `game_time`: what the game sets by the clock at its making, which a chunk's
    /// generation cannot read (`docs/groundwork/n7l-plan.md`, section 2.2: a camel's last
    /// pose change). Nothing but where the type has its own.
    fn first_take(&self, mob: &mut Mob, game_time: i64) {
        let _ = (mob, game_time);
    }

    /// `getRequiresPrecisePosition()`: whether its position is sent whole (a happy ghast
    /// while it stays still). Never but where the type says.
    fn requires_precise_position(&self, mob: &Mob) -> bool {
        let _ = mob;
        false
    }

    /// `isFlyingVehicle()`: whether its driver is never kicked for floating, and its
    /// move gathers no fall (an adult happy ghast). Never but where the type says.
    fn flying_vehicle(&self, mob: &Mob) -> bool {
        let _ = mob;
        false
    }

    /// `removePassenger`'s own part, after a seat left `vehicle` (its record as it is
    /// now): what it does to itself, and a sound it makes at itself at volume 1 and
    /// pitch 1 (a happy ghast's goggles going up). Nothing but where the type has its
    /// own.
    fn passenger_removed(&self, vehicle: &clustine_world::Entity, mob: &mut Mob) -> Option<i32> {
        let _ = (vehicle, mob);
        None
    }

    /// `tickRidden(player, input)` of a mob a player steers, whose rotation is `yaw` and
    /// `pitch`: nothing but where the type has its own.
    fn tick_ridden(&self, tick: &mut MobTick<'_>, yaw: f32, pitch: f32) {
        let _ = (tick, yaw, pitch);
    }

    /// What the type's `actuallyHurt` does before the body's (an animal's love ends),
    /// at the moment `moment`.
    fn actually_hurt(&self, mob: &mut Mob, moment: u32) {
        let _ = (mob, moment);
    }

    /// `getDefaultDimensions(pose)` where it is not the type's (a baby animal's own
    /// size): its width, height and eye height.
    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        let _ = mob;
        None
    }

    /// `moveControl.tick()` of the type's move control (a rabbit's sets its speed first).
    fn tick_move_control(&self, ai: &mut Ai<'_, '_>) {
        controls::tick_move(ai);
    }

    /// `jumpControl.tick()` of the type's jump control (a rabbit's starts a hop).
    fn tick_jump_control(&self, ai: &mut Ai<'_, '_>) {
        controls::tick_jump(ai);
    }

    /// `getJumpPower()` where the type has its own (a rabbit's), given the jump factor of
    /// the block it stands on; `None` for `LivingEntity`'s.
    fn jump_power(&self, tick: &MobTick<'_>, block_factor: f32) -> Option<f32> {
        let _ = (tick, block_factor);
        None
    }

    /// What the type's `jumpFromGround` does after `LivingEntity`'s (a rabbit's push
    /// ahead and its hop shown).
    fn jumped_from_ground(&self, tick: &mut MobTick<'_>) {
        let _ = tick;
    }

    /// `removeWhenFarAway(distance)`: whether, far from every player, it may despawn.
    /// Its class decides by default ([`ladder::remove_when_far_away`]).
    fn remove_when_far_away(&self, kind: u16, mob: &Mob, tick_count: u32, distance: f64) -> bool {
        let _ = (mob, distance);
        ladder::remove_when_far_away(kind, tick_count)
    }

    /// `getBaseExperienceReward`: the experience of its death, drawn from its own
    /// stream. Its class decides by default
    /// ([`super::body::hurt::base_experience_reward`]).
    fn experience_reward(&self, kind: u16, mob: &Mob, draws: &mut Random) -> i32 {
        let _ = mob;
        super::body::hurt::base_experience_reward(kind, draws)
    }

    /// `getAmbientSound()`: none in `Mob`.
    fn ambient_sound(&self, mob: &Mob) -> Option<i32> {
        let _ = mob;
        None
    }

    /// The sound of its `playStepSound(pos, state)` where the type has its own (played
    /// at 0.15 and 1); `None` for `Entity`'s, the step of the block stepped on.
    fn step_sound(&self, mob: &Mob) -> Option<i32> {
        let _ = mob;
        None
    }

    /// The volume its own step sound ([`MobType::step_sound`]) plays at: `Entity`'s
    /// 0.15, unless the type plays it at its own (a copper golem's at 1).
    fn step_volume(&self) -> f32 {
        0.15
    }

    /// Whether its `playStepSound` plays anything: the type's own or the block's, unless
    /// the type's does nothing (a nautilus's).
    fn plays_step_sounds(&self) -> bool {
        true
    }

    /// `requiresCustomPersistence()` beyond riding and a lead: kept wherever it is, its
    /// despawning not even drawn for (a tamed nautilus).
    fn requires_custom_persistence(&self, mob: &Mob) -> bool {
        let _ = mob;
        false
    }

    /// The sound its type's `finalizeSpawn` plays before `Mob`'s, at its feet at full
    /// volume and pitch (a copper golem's `playSpawnSound`); none by default.
    fn spawn_sound(&self, mob: &Mob) -> Option<i32> {
        let _ = mob;
        None
    }

    /// `getAmbientSound()` as its tick asks it, where the type's depends on where the
    /// mob is (a dolphin's in water, a turtle's on land); the place-free one by default.
    fn ambient_sound_at(&self, mob: &Mob, in_water: bool, on_ground: bool) -> Option<i32> {
        let _ = (in_water, on_ground);
        self.ambient_sound(mob)
    }

    /// `getAmbientSound()` of a type that draws for it from its own random (a wolf's
    /// pant or whine, a cat's purreow), at the game time `now` and the mob's moment
    /// `lived`. `place` is where its tick finds it (in water, on the ground), `None` at
    /// a spawn egg's sound: by default [`MobType::ambient_sound_at`] there, else
    /// [`MobType::ambient_sound`], which draw nothing.
    fn ambient_sound_drawn(
        &self,
        mob: &Mob,
        draws: &mut Random,
        now: i64,
        lived: u32,
        place: Option<(bool, bool)>,
    ) -> Option<i32> {
        let _ = (draws, now, lived);
        match place {
            Some((in_water, on_ground)) => self.ambient_sound_at(mob, in_water, on_ground),
            None => self.ambient_sound(mob),
        }
    }

    /// `getAmbientSoundInterval()`: 80 in `Mob`.
    fn ambient_interval(&self) -> u32 {
        80
    }

    /// `getAmbientSoundInterval()` of one that carries riders where `carries`: the type's
    /// own interval but where a rider changes it (a happy ghast's six times as long).
    fn ambient_interval_while(&self, carries: bool) -> u32 {
        let _ = carries;
        self.ambient_interval()
    }

    /// `getHurtSound(source)`: `generic.hurt` in `LivingEntity`.
    fn hurt_sound(&self, mob: &Mob, source: &DamageSource) -> Option<i32> {
        let _ = (mob, source);
        Some(sounds::ENTITY_GENERIC_HURT)
    }

    /// `getDeathSound()`: `generic.death` in `LivingEntity`.
    fn death_sound(&self, mob: &Mob) -> Option<i32> {
        let _ = mob;
        Some(sounds::ENTITY_GENERIC_DEATH)
    }

    /// `getSoundVolume()`: 1 in `LivingEntity`.
    fn sound_volume(&self) -> f32 {
        1.0
    }

    /// `isBaby()`: false but for an ageable mob of a negative age (N7e's).
    fn is_baby(&self, mob: &Mob) -> bool {
        let _ = mob;
        false
    }

    /// The type's own synced values (`defineSynchedData`), each with its index and
    /// whether it is the default: an ageable type's baby (16), a sheep's wool (18) and
    /// the like (`crate::shown`). None for a type that adds none.
    fn shown(&self, mob: &Mob) -> Vec<(u8, crate::shown::DataShown, bool)> {
        let _ = mob;
        Vec::new()
    }

    /// What its loot table asks of it that is its type's (a sheep's colour).
    fn loot_facts(&self, mob: &Mob) -> loot::Facts {
        let _ = mob;
        loot::Facts::default()
    }

    /// The type as a tamed animal's goals read it (`TamableAnimal`), if it is one.
    fn pet(&self) -> Option<&'static dyn tamable::Pet> {
        None
    }

    /// `canBeLeashed()`: its class decides by default ([`ladder::can_be_leashed`]).
    fn can_be_leashed(&self, kind: u16, mob: &Mob) -> bool {
        let _ = mob;
        ladder::can_be_leashed(kind)
    }

    /// What the type's `tick` does before the entity's own, after the level's despawn
    /// check (a pufferfish's puffing).
    fn before_tick(&self, tick: &mut MobTick<'_>) {
        let _ = tick;
    }

    /// What the type's `tick` does after the whole of the living one (a dolphin's
    /// moistness, a school's census).
    fn after_tick(&self, tick: &mut MobTick<'_>) {
        let _ = tick;
    }

    /// What its `baseTick` does after `Mob.baseTick` and its ambient sound (a water
    /// animal's own air, `handleAirSupply`), given the air it had as the tick began.
    fn after_base_tick(&self, tick: &mut MobTick<'_>, air: i32) {
        let _ = (tick, air);
    }

    /// `isImmobile()`: a dying mob's (`LivingEntity`'s), unless its type says otherwise
    /// (a horse eating or rearing, a dying horse only under a saddled rider). An
    /// immobile mob's AI does not run and its inputs stop.
    fn immobile(&self, tick: &MobTick<'_>) -> bool {
        tick.mob.body.vitals.dying()
    }

    /// What its `aiStep` does before `LivingEntity.aiStep` (a fish's flop, a horse's
    /// tail's draw, a parrot's imitation).
    fn before_ai_step(&self, tick: &mut MobTick<'_>) {
        let _ = tick;
    }

    /// How it travels in this tick, where its `travel` or `travelInWater` is its own.
    fn travel(&self, tick: &MobTick<'_>) -> Travel {
        let _ = tick;
        Travel::Living
    }

    /// `isPushedByFluid()`: true in `Entity`.
    fn pushed_by_fluid(&self) -> bool {
        true
    }

    /// `getMaxAirSupply()`: 300 in `Entity`, its air at construction too.
    fn max_air(&self) -> i32 {
        crate::living::MAX_AIR
    }

    /// Whether its `increaseAirSupply` fills it at once (a dolphin's).
    fn refills_air_at_once(&self) -> bool {
        false
    }

    /// What its `hurtServer` does after `LivingEntity`'s, which `took` says took (a
    /// squid's ink).
    fn after_hurt(&self, struck: &mut Struck<'_>, source: &DamageSource, took: bool) {
        let _ = (struck, source, took);
    }

    /// A breeding of the type that makes no child but changes the parent that bred, as
    /// a turtle's goal gives it an egg: the parent's type state so changed, or `None`
    /// for a type whose breeding makes a child ([`MobType::breed_offspring`]).
    fn bred_without_child(&self, this: &Mob) -> Option<clustine_world::TypeState> {
        let _ = this;
        None
    }

    /// `PathNavigation.isStableDestination(pos)` where its navigation has its own (a
    /// turtle's travelling); `None` for its navigation's kind's.
    fn stable_destination(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> Option<bool> {
        let _ = (ai, pos);
        None
    }

    /// What the type's `hurtServer` does after a hurt that took (a horse rears one time
    /// in three), from the mob's own stream, at the moment `moment`.
    fn hurt_taken(&self, mob: &mut Mob, draws: &mut Random, moment: u32) {
        let _ = (mob, draws, moment);
    }

    /// `dropEquipment` beyond the body's: what the type keeps of its own and drops as it
    /// dies (a horse's inventory, a donkey's chest), in the order dropped, taken out of
    /// the mob.
    fn drop_equipment(&self, mob: &mut Mob) -> Vec<ItemStack> {
        let _ = mob;
        Vec::new()
    }

    /// `canMate(partner)` beyond both being in love, which the region asks of a
    /// breeding it carries out: by default of one type (`Animal.canMate`'s class test);
    /// a horse and a donkey may, each where it can parent; a wolf only tamed, with a
    /// tamed wolf not sitting.
    fn can_mate(&self, this: Mate<'_>, partner: Mate<'_>) -> bool {
        this.entity.kind == partner.entity.kind
    }

    /// What the type's `hurtServer` does before any of `LivingEntity`'s, whether or not
    /// the hurt is then refused (an axolotl's draws for playing dead).
    fn before_hurt_server(&self, struck: &mut Struck<'_>, source: &DamageSource, amount: f32) {
        let _ = (struck, source, amount);
    }

    /// `AgeableMob.canBeABaby()`: whether `setBaby(true)` makes it one (never a frog).
    fn can_be_a_baby(&self) -> bool {
        true
    }

    /// Whether its `finalizeSpawn` runs for a mob poured from a bucket
    /// (`EntitySpawnReason.BUCKET`): an axolotl's returns before anything, `Mob`'s
    /// included.
    fn finalizes_when_poured(&self) -> bool {
        true
    }

    /// `setFromBucket(true)` of a mob of the type `kind` poured from a bucket: a fish's
    /// mark, by default; nothing for a type that keeps none.
    fn poured_from_bucket(&self, kind: u16, mob: &mut Mob) {
        if fish::Species::of(kind).is_some() {
            fish::set_from_bucket(kind, mob);
        }
    }
    /// What the type's `die` does before `LivingEntity`'s (a sniffer is idle again).
    fn before_die(&self, mob: &mut Mob) {
        let _ = mob;
    }

    /// Whether the type's `hurtServer` refuses `source` before anything else (an allay
    /// is not hurt by the player it likes).
    fn refuses_hurt(&self, mob: &Mob, source: &DamageSource) -> bool {
        let _ = (mob, source);
        false
    }

    /// `spawnChildFromBreeding` of a type that lays an item instead of a child (a
    /// sniffer's egg): the item, laid at the parent's feet, and its sound.
    fn laid_from_breeding(&self) -> Option<Laid> {
        None
    }

    /// `supportQuadLeash()`: whether a lead holds it by four points where its holder
    /// holds by four (`supportQuadLeashAsHolder`): `AbstractHorse`'s but a llama's.
    fn quad_leash(&self, kind: u16) -> bool {
        equine::is_equine(kind) && !equine::is_llama(kind)
    }

    /// `supportQuadLeashAsHolder()`: a happy ghast's.
    fn quad_leash_holder(&self) -> bool {
        false
    }

    /// `leashElasticDistance()` and `leashSnapDistance()` of it leashed: 6 and 12.
    fn leash_distances(&self) -> (f64, f64) {
        (6.0, 12.0)
    }

    /// What its `onElasticLeashPull` does beyond the fall it stops (a happy ghast's move
    /// control waits).
    fn on_elastic_leash_pull(&self, mob: &mut Mob) {
        let _ = mob;
    }

    /// What its tick does with the time a quad leashee keeps it a leash holder
    /// (`leashHolderTime`, which the game does not save and the region keeps in its
    /// memory), after the whole of its tick: nothing but for a happy ghast.
    fn leash_holder_tick(&self, mob: &mut Mob, time: &mut i32) {
        let _ = (mob, time);
    }

    /// `canBreatheUnderwater()` beyond `#can_breathe_under_water` (a ghastling).
    fn breathes_under_water(&self, mob: &Mob) -> bool {
        let _ = mob;
        false
    }

    /// `getSoundVolume()` where the type's depends on the mob (a ghastling's 1, a happy
    /// ghast's 4): [`MobType::sound_volume`] by default.
    fn sound_volume_of(&self, mob: &Mob) -> f32 {
        let _ = mob;
        self.sound_volume()
    }

    /// What the type's `hurtServer` does to the amount before `LivingEntity`'s (a
    /// rolled-up armadillo takes `(amount − 1) / 2`): the amount as it is by default.
    fn hurt_amount(&self, mob: &Mob, amount: f32) -> f32 {
        let _ = mob;
        amount
    }

    /// What the type's `actuallyHurt` does before its superclass's, where it needs the
    /// source or the world's clock (a camel stands up at once): nothing by default.
    fn before_actually_hurt(&self, struck: &mut Struck<'_>, source: &DamageSource) {
        let _ = (struck, source);
    }

    /// What the type's `actuallyHurt` does after its superclass's, whether or not the
    /// hurt reached its health (an armadillo rolls up from what hurt it): nothing by
    /// default.
    fn after_actually_hurt(&self, struck: &mut Struck<'_>, source: &DamageSource) {
        let _ = (struck, source);
    }

    /// Whose `causeFallDamage` its landing takes where its type says (a camel's is the
    /// horse family's, a goat's the living one's less ten); `None` for its class's
    /// ([`super::body::landing`]).
    fn landing(&self, mob: &Mob) -> Option<crate::living::Landing> {
        let _ = mob;
        None
    }

    /// The sound, volume and pitch of its `playStepSound(pos, state)` on the block
    /// `state` where the type has its own: by default [`MobType::step_sound`] at 0.15
    /// and 1, `None` for the block's own step.
    fn step_sound_on(
        &self,
        mob: &Mob,
        state: clustine_data::BlockState,
    ) -> Option<(i32, f32, f32)> {
        let _ = state;
        self.step_sound(mob)
            .map(|sound| (sound, self.step_volume(), 1.0))
    }

    /// What the type's `travel(input)` does before its superclass's, with the input and
    /// the motion in the living body (a sitting camel's are taken to nothing but their
    /// height on the ground): nothing by default.
    fn before_travel(&self, tick: &mut MobTick<'_>) {
        let _ = tick;
    }

    /// Whether its `BodyRotationControl.clientTick` turns its body in this tick (a
    /// sitting camel's and a rolled-up armadillo's do not): always by default.
    fn body_rotation_ticks(&self, tick: &MobTick<'_>) -> bool {
        let _ = tick;
        true
    }
}

/// What a breeding lays instead of a child ([`MobType::laid_from_breeding`]): the item,
/// and the sound it is laid with, at a pitch about `pitch_base` drawn of the parent's own
/// random (`(a - b) * 0.2 + pitch_base`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Laid {
    pub(crate) stack: ItemStack,
    pub(crate) sound: i32,
    pub(crate) pitch_base: f32,
}

/// One of the two of a breeding, as `canMate` reads it: its record and its mob.
#[derive(Clone, Copy)]
pub(crate) struct Mate<'a> {
    pub(crate) entity: &'a Entity,
    pub(crate) mob: &'a Mob,
}

/// A player's use of a mob, as `mobInteract` reads it.
#[allow(
    dead_code,
    reason = "what the uses of later parts read: a tamer's key, a sneaking rider's"
)]
pub(crate) struct Used<'a> {
    pub(crate) key: EntityId,
    /// The mob's entity type, by id.
    pub(crate) kind: u16,
    pub(crate) mob: &'a mut Mob,
    /// The player's entity.
    pub(crate) player: EntityId,
    /// The player's UUID, where the region knows it (its own player's, or a face's):
    /// what `isOwnedBy` compares and taming keeps.
    pub(crate) player_uuid: Option<u128>,
    /// The world's game time (`Level.getGameTime()`), which a wolf's anger is reckoned
    /// against.
    pub(crate) game_time: i64,
    /// What the hand used holds.
    pub(crate) held: Option<ItemStack>,
    /// Whether the player is a creative one (every player is: what they use is kept).
    pub(crate) creative: bool,
    pub(crate) sneaking: bool,
    /// Whether anything rides the mob (`isVehicle`).
    pub(crate) carries: bool,
    /// How many ride it, seats kept for players on their way counted and the player's
    /// own stale seat, which getting on replaces, not (`getPassengers().size()`).
    pub(crate) passengers: usize,
    /// Whether the mob stands on the ground (a parrot flying is not sat down).
    pub(crate) on_ground: bool,
    /// The player's feet.
    pub(crate) from: Vec3,
    /// The mob's feet.
    pub(crate) position: Vec3,
    /// The mob's `lived`: the moment its counters see the use at.
    pub(crate) moment: u32,
    /// The mob's own stream, aside from its tick, on from what it drew in it.
    pub(crate) draws: &'a mut Random,
    /// What the use did beyond the mob, for the region to do in order.
    pub(crate) did: Vec<Did>,
}

/// What a player's use of a mob does beyond the mob itself, which the region carries
/// out in the order given.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Did {
    /// `playSound` at `at` from `source`, heard by everyone, or by everyone but the
    /// player who used it (`Player.playSound`, whose own client plays it).
    Sound {
        sound: i32,
        source: i32,
        at: Vec3,
        volume: f32,
        pitch: f32,
        but_the_player: bool,
    },
    /// `broadcastEntityEvent(mob, event)`.
    Event(u8),
    /// `gameEvent(event, player)` at the mob, by its name.
    GameEvent(&'static str),
    /// `ItemUtils.createFilledResult`: the stack goes to the player.
    Give {
        stack: ItemStack,
        no_duplicates: bool,
    },
    /// An item entity of `stack` at `at` (`spawnAtLocation`, a shearing's drops), with
    /// its pick-up delay, and a push added to its own motion.
    Drop {
        stack: ItemStack,
        at: Vec3,
        pick_up_delay: u16,
        push: Option<Vec3>,
    },
    /// `hurt(playerAttack(player), amount)` of the mob, by the player who used it (a
    /// cookie fed to a parrot).
    HurtByPlayer { amount: f32 },
    /// `convertTo(kind, single)`: the mob made a mob of `kind` where it stands, with
    /// `drops` laid at its top before the new mob is added, and itself gone.
    Convert { kind: u16, drops: Vec<ItemStack> },
    /// `player.startRiding(mob)`: the player who used it gets on, where they may.
    Mount,
    /// [`Did::Mount`], and where the mob then carries nobody (the mount was refused),
    /// `clearHome()` (a nautilus's `doPlayerRide`).
    MountOrClearHome,
    /// `discard()`: the mob gone, dropping nothing (a fish taken in a bucket).
    Discard,
    /// `openCustomInventoryScreen(player)`: the player's menu of the mob's inventory
    /// opens (a horse's saddle, armour and chest), where the player is the region's.
    OpenInventory,
    /// `doPlayerRide(player)`: the player turned to the horse's way, and on.
    Ride,
    /// A tadpole fed to its end grows up (`Tadpole.ageUp()`): a frog of its biome made
    /// where it is, and itself gone (`region/frogspawn.rs`).
    GrowUp,
    /// `BehaviorUtils.throwItem`: an item thrown from `at` with its motion set after its
    /// constructor's draws (what a copper golem holds, taken by an empty hand).
    /// `BehaviorUtils.throwItem`: an item at `at` whose motion is set after its
    /// constructor's draws (an allay's gathered items, thrown at its own feet).
    Throw {
        stack: ItemStack,
        at: Vec3,
        pick_up_delay: u16,
        motion: Vec3,
    },
    /// `level.levelEvent(kind, pos, data)` (a copper golem waxed or scraped).
    LevelEvent { kind: i32, pos: BlockPos, data: i32 },
    /// `addFreshEntity` of a mob of `kind` made where the used mob stands, with no
    /// `finalizeSpawn` (an allay's duplicate).
    Spawn { kind: u16, mob: Box<Mob> },
}

/// A mob being made by an egg (`finalizeSpawn`): its type, the mob, the level's random
/// on its behalf, its own random, and the biome at its block.
pub(crate) struct Spawning<'a> {
    pub(crate) mob: &'a mut Mob,
    /// The level's random: the player's stream aside for an egg, the run's region
    /// random (R) or the spawn's (W and R) for generation (`docs/groundwork/n7l-plan.md`,
    /// sections 2.1 and 3).
    pub(crate) level: &'a mut dyn clustine_world::generation::Draws,
    /// The mob's own stream, on from its constructor's draws (a salmon draws its size
    /// from it).
    pub(crate) own: &'a mut Random,
    /// The name of the biome at the mob's block, where its chunk is read.
    pub(crate) biome: Option<&'static str>,
    /// The mob's block (`blockPosition()`: a turtle's home).
    pub(crate) at: BlockPos,
    /// The overworld clock's day time (`world_clock`). A cat's variant now asks the
    /// moon's brightness, which the makers derive from it.
    #[allow(
        dead_code,
        reason = "no type reads the day since the moon's brightness came; the makers still set it"
    )]
    pub(crate) day: u64,
    /// The world's game time (`Level.getGameTime()`), which a camel's pose is stamped
    /// with.
    pub(crate) game_time: i64,
    /// Why it is made (`EntitySpawnReason`): an egg's, a chunk's generation, a
    /// structure's. No creature's draws differ between these three: the reasons
    /// that change them (a bucket's, an event's, a natural spawn's) are not made here.
    #[allow(
        dead_code,
        reason = "the reasons that change a creature's draws are not made yet"
    )]
    pub(crate) reason: SpawnReason,
    /// What the first `finalizeSpawn` of a group returned, which the group's later
    /// members are given (`SpawnGroupData`): none for an egg or the first of a group,
    /// whose `finalizeSpawn` leaves here what it returns.
    pub(crate) group: &'a mut Option<SpawnGroup>,
    /// The structure whose piece places it (the swamp hut's black cat), if one does.
    pub(crate) structure: Option<&'static str>,
    /// The moon's brightness (`getMoonBrightness`): the clock's for an egg; a full moon,
    /// 1, for generation, whose region has no time layer.
    pub(crate) moon_brightness: f32,
    /// The difficulty at its block (`DifficultyInstance`): its effective value and its
    /// special multiplier, which a zombie's and a villager's draws are weighed against.
    #[allow(
        dead_code,
        reason = "read by the creatures' `finalizeSpawn` (N7l's D1)"
    )]
    pub(crate) difficulty: (f32, f32),
    #[allow(dead_code, reason = "N7k's types (parts K1 to K5) call it")]
    /// Where a mob made beside this one comes from (a zombie's chicken, a spider's
    /// skeleton, a drowned's zombie nautilus), as the game makes them in the middle of
    /// `finalizeSpawn`; none where the maker makes no others (`making::companion`).
    pub(crate) companions: Option<&'a mut dyn Companions>,
}

/// Whether a mob made in another's `finalizeSpawn` is that one's vehicle or rides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompanionSeat {
    /// The mob being made rides it (a zombie's chicken, a drowned's zombie nautilus),
    /// added before it as the game's `addFreshEntity` inside `finalizeSpawn` adds it.
    Vehicle,
    #[allow(dead_code, reason = "N7k's types (parts K1 to K5) call it")]
    /// It rides the mob being made (a spider's skeleton), added after it, with it, as
    /// `addFreshEntityWithPassengers` adds it.
    Passenger,
}

/// A mob made beside another, whole: its record (its key the maker's), its seat, and its
/// own stream as its making left it.
pub(crate) struct Companion {
    pub(crate) entity: Entity,
    pub(crate) seat: CompanionSeat,
    pub(crate) own: Random,
}

#[allow(dead_code, reason = "N7k's types (parts K1 to K5) call it")]
/// The maker's side of a mob made beside another (`n7k-plan.md`, section 10, "Mobs
/// riding mobs"): a key and a stream for it, and taking it once made.
pub(crate) trait Companions {
    /// A key and own stream for a mob made beside the one being finalised, or `None`
    /// where none can be made (an egg's mob made in another region's land).
    fn next(&mut self) -> Option<(EntityId, Random)>;
    /// The companion made, which the maker adds with the mob as `seat` says.
    fn add(&mut self, companion: Companion);
}

/// `EntitySpawnReason`, as far as a mob's making reads it.
#[allow(
    dead_code,
    reason = "the reasons generation and breeding make mobs for (N7l)"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpawnReason {
    SpawnEgg,
    Breeding,
    ChunkGeneration,
    Structure,
    Natural,
    /// A mob made of another (a tadpole grown into a frog).
    Conversion,
    /// A mob made to carry or ride another as it is made (a zombie's chicken, a spider's
    /// skeleton).
    Jockey,
}

/// `AgeableMob.AgeableMobGroupData`: how many of the group were made so far
/// (`groupSize`), whether a later one may be a baby, and the chance that it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AgeableGroup {
    pub(crate) made: u32,
    pub(crate) babies: bool,
    pub(crate) baby_chance: f32,
}

impl AgeableGroup {
    /// `AgeableMobGroupData(shouldSpawnBaby)`: the chance 0.05.
    pub(crate) const fn new(babies: bool) -> Self {
        Self {
            made: 0,
            babies,
            baby_chance: 0.05,
        }
    }

    /// `AgeableMobGroupData(babySpawnChance)`: babies at that chance.
    pub(crate) const fn with_chance(baby_chance: f32) -> Self {
        Self {
            made: 0,
            babies: true,
            baby_chance,
        }
    }
}

/// `SpawnGroupData` that a group's first `finalizeSpawn` makes and its later members are
/// given (`docs/groundwork/n7-brief.md`, section 8.7). Each of a creature's is an
/// [`AgeableGroup`] with what its type adds, as the game's subclasses of
/// `AgeableMobGroupData` are.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum SpawnGroup {
    /// `AgeableMobGroupData` itself.
    Ageable(AgeableGroup),
    /// `Horse.HorseGroupData`: the herd's coat (`(true)`: babies one in twenty).
    Horse { ageable: AgeableGroup, coat: i32 },
    /// `Llama.LlamaGroupData`: the herd's coat (`(true)`).
    Llama { ageable: AgeableGroup, coat: i32 },
    /// `Rabbit.RabbitGroupData`: the group's variant (`(1.0f)`: every later one a
    /// baby).
    Rabbit { ageable: AgeableGroup, variant: i32 },
    /// `Fox.FoxGroupData`: the group's variant (`(false)`: the fox makes its own babies,
    /// from the third, by the group's size).
    Fox { ageable: AgeableGroup, variant: i32 },
    /// `Wolf.WolfPackData`: the pack's variant (`(false)`).
    WolfPack { ageable: AgeableGroup, variant: u8 },
    /// `Axolotl.AxolotlGroupData`: the group's two variants, of which each member draws
    /// one (`(false)`: the axolotl makes its own babies, from the third).
    Axolotl {
        ageable: AgeableGroup,
        variants: [i32; 2],
    },
}

impl SpawnGroup {
    /// The group as `AgeableMobGroupData`, which every group of a creature is.
    pub(crate) fn ageable(&mut self) -> &mut AgeableGroup {
        match self {
            Self::Ageable(ageable)
            | Self::Horse { ageable, .. }
            | Self::Llama { ageable, .. }
            | Self::Rabbit { ageable, .. }
            | Self::Fox { ageable, .. }
            | Self::WolfPack { ageable, .. }
            | Self::Axolotl { ageable, .. } => ageable,
        }
    }
}

/// `Mob.load` of a structure template's entity tag (`docs/groundwork/n7-brief.md`,
/// section 8.11), as far as what it reads outlasts the `finalizeSpawn` that follows:
/// `PersistenceRequired`, the attributes (bases and modifiers: a follow range that
/// carries its `random_spawn_bonus` draws no triangle), the health (held to the maximum
/// the attributes give), then the type's own ([`MobType::from_template`]). `tag` is the
/// compound with its type byte first, as a template keeps it.
#[allow(
    dead_code,
    reason = "called by the making of the structures' template entities (N7l's D3)"
)]
pub(crate) fn from_template(kind: u16, mob: &mut Mob, tag: &[u8]) {
    template::read_mob(kind, mob, tag);
    of(kind).from_template(mob, tag);
}

/// A breeding as `getBreedOffspring` reads it: the parent that breeds (`this`) and its
/// partner, the draws, and the biome at the parent's block.
pub(crate) struct Breeding<'a> {
    pub(crate) this: &'a Mob,
    pub(crate) partner: &'a Mob,
    /// The partner's entity type, by id (a donkey's partner a horse: a mule).
    pub(crate) partner_kind: u16,
    /// The child's entity type where it is not the parent's (a mule), which
    /// `breed_offspring` sets.
    pub(crate) child_kind: Option<u16>,
    /// The parent's own stream: its random's draws, and the level's on its behalf at
    /// their place among them (the plan's 7.2.8).
    pub(crate) draws: &'a mut Random,
    /// The name of the biome at the parent's block, where its chunk is read.
    pub(crate) biome: Option<&'static str>,
    /// The UUID of the player whose spawn egg used on the mob makes the child
    /// (`onOffspringSpawnedFromEgg`: a fox's child trusts them); none for a breeding.
    pub(crate) egg_user: Option<u128>,
}

/// An entity's size (`EntityDimensions`): its width, height and eye height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Dimensions {
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) eye_height: f32,
}

/// What a mob's riding jump is heard as (`handleStartJump`'s `makeSound` and
/// `gameEvent`; `docs/groundwork/n7j-riding-plan.md`, section 9 item 9).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct JumpStart {
    pub(crate) sound: i32,
    /// `getSoundVolume()`.
    pub(crate) volume: f32,
    /// Whether its pitch is `getVoicePitch()`, `(nextFloat() − nextFloat()) · 0.2 + 1` of
    /// its own random (else 1).
    pub(crate) voice: bool,
    /// Whether `gameEvent(ENTITY_ACTION)` follows.
    pub(crate) action: bool,
}

/// `InteractionResult` as a use of a mob answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Interaction {
    /// Nothing happened; the item's own use of an entity may follow.
    Pass,
    /// Done, the arm swinging for the trackers.
    Success,
    /// Done, the arm swinging for the player too.
    SuccessServer,
    /// Done, no arm swinging.
    Consume,
}

/// A type that adds nothing: a mob whose type no part has built yet is a body of the
/// game's defaults.
pub(crate) struct Plain;

impl MobType for Plain {}

/// Whether a type has its module, so that an egg makes it: a mob of a type without its
/// AI would stand like a statue, so until its part brings it, its egg makes nothing
/// (`docs/groundwork/n7-plan.md`, section 11.6).
pub(crate) fn is_built(kind: u16) -> bool {
    built(kind).is_some()
}

/// The module of the entity type `kind`, by its id in `clustine_data::ENTITY_TYPES`.
pub(crate) fn of(kind: u16) -> &'static dyn MobType {
    built(kind).unwrap_or(&Plain)
}

/// The module of a type some part has built: one arm a type, as each part brings it.
fn built(kind: u16) -> Option<&'static dyn MobType> {
    let found: &'static dyn MobType = match i32::from(kind) {
        entity_types::COW => &cow::Cow,
        entity_types::MOOSHROOM => &mooshroom::Mooshroom,
        entity_types::PIG => &pig::Pig,
        entity_types::SHEEP => &sheep::Sheep,
        entity_types::CHICKEN => &chicken::Chicken,
        entity_types::RABBIT => &rabbit::Rabbit,
        entity_types::COD => &cod::Cod,
        entity_types::SALMON => &salmon::Salmon,
        entity_types::PUFFERFISH => &pufferfish::Pufferfish,
        entity_types::TROPICAL_FISH => &tropical_fish::TropicalFish,
        entity_types::SQUID => &squid::Squid,
        entity_types::GLOW_SQUID => &glow_squid::GlowSquid,
        entity_types::DOLPHIN => &dolphin::Dolphin,
        entity_types::TURTLE => &turtle::Turtle,
        entity_types::HORSE => &horse::Horse,
        entity_types::DONKEY => &donkey::Donkey,
        entity_types::MULE => &mule::Mule,
        entity_types::LLAMA => &llama::Llama,
        entity_types::TRADER_LLAMA => &trader_llama::TraderLlama,
        entity_types::SKELETON_HORSE => &skeleton_horse::SkeletonHorse,
        entity_types::ZOMBIE_HORSE => &zombie_horse::ZombieHorse,
        entity_types::WOLF => &wolf::Wolf,
        entity_types::CAT => &cat::Cat,
        entity_types::OCELOT => &ocelot::Ocelot,
        entity_types::PARROT => &parrot::Parrot,
        entity_types::FOX => &fox::Fox,
        entity_types::PANDA => &panda::Panda,
        entity_types::POLAR_BEAR => &polar_bear::PolarBear,
        entity_types::BEE => &bee::Bee,
        entity_types::BAT => &bat::Bat,
        entity_types::AXOLOTL => &axolotl::Axolotl,
        entity_types::FROG => &frog::Frog,
        entity_types::TADPOLE => &tadpole::Tadpole,
        entity_types::NAUTILUS => &nautilus::Nautilus,
        entity_types::ZOMBIE_NAUTILUS => &zombie_nautilus::ZombieNautilus,
        entity_types::COPPER_GOLEM => &copper_golem::CopperGolem,
        entity_types::SNIFFER => &sniffer::Sniffer,
        entity_types::ALLAY => &allay::Allay,
        entity_types::HAPPY_GHAST => &happy_ghast::HappyGhast,
        entity_types::GOAT => &goat::Goat,
        entity_types::CAMEL => &camel::CAMEL,
        entity_types::CAMEL_HUSK => &camel_husk::CAMEL_HUSK,
        entity_types::ARMADILLO => &armadillo::Armadillo,
        _ => return None,
    };
    Some(found)
}

/// The size of `entity` where its type's module gives it one of its own (a baby
/// animal's), else `None`: the type's.
pub(crate) fn own_size(entity: &Entity) -> Option<Dimensions> {
    let module = built(entity.kind)?;
    let EntityData::Mob { state, .. } = &entity.data else {
        return None;
    };
    // Only the type's state decides a baby's size: the body is not decoded for it.
    let mob = Mob {
        body: Default::default(),
        mind: Default::default(),
        state: state.clone(),
    };
    module.dimensions(&mob)
}
