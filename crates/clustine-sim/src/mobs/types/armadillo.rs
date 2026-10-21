//! Armadillos (`Armadillo`; `docs/groundwork/n7-brief.md`, section 7.8): an animal of
//! spider eyes that rolls up when something living hurts it (and takes half a hit less
//! one rolled up), unrolls when fire, lava or the like hurts it instead, gives a scute
//! to a brush and sheds one every 6000 to 11999 ticks; rolled up it is not fed nor
//! bred, makes no ambient sound and its hurt sound is the reduced one.
//!
//! Its state (`ArmadilloState`: idle, rolling, scared, unrolling) is synced by a
//! serializer of its own; how long it has been in it (`inStateTicks`, which counts every
//! tick and is neither saved nor sent) is the moment it began, and its scute's timer
//! (`scuteTime`, saved) the moment it runs out, so that an armadillo at rest writes
//! nothing.
//!
//! Its mind is a brain (`ArmadilloAi`), which N7j's machinery brings: until its provider
//! is wired an armadillo has no goals and stands and falls by its body alone, and one
//! that rolled up stays so (rolling on to scared, peeking and unrolling are its brain's
//! `ArmadilloBallUp` and its rolling out). What the brain will call is in [`brain`]: the
//! behaviours' bodies, the scare sensor's test, the conditions they read.
//!
//! Named differences: a baby sheds no scute (`shouldDropLoot` is false for a baby, which
//! the brief has otherwise) and its timer waits below nothing until it grows up; the
//! timer counts the ticks its AI does not run (`NoAI`), as a stamp does; the shed and
//! brush tables, which the extraction does not compile yet (the lead's N7j1), are their
//! one scute and draw nothing; a brush wears nothing, every player being creative.

use clustine_data::{items, sounds};
use serde::{Deserialize, Serialize};

use super::animal::{self, Own};
use super::{Breeding, Did, Dimensions, Interaction, MobType, Used};
use crate::mobs::Mob;
use crate::mobs::ai::goals::Items;
use crate::mobs::ai::{Ai, HeadLimits, Setup};
use crate::mobs::body::hurt::{DamageSource, GameEvent, Struck, voice_pitch};
use crate::mobs::body::{Event, loot};
use crate::mobs::tick::{Local, MobTick};
use crate::physics::Vec3;
use crate::physics::item::SOUND_SOURCE_NEUTRAL;
use crate::region::Random;
use crate::shown::{DataShown, Serialiser};
use clustine_world::{BlockPos, ItemStack};

/// `#armadillo_food`.
pub(crate) const FOOD: Items = Items::Tag("minecraft:armadillo_food");

/// The synced state's index.
const STATE_INDEX: u8 = 18;

/// The shed's and the brush's tables.
const SHED_TABLE: &str = "minecraft:gameplay/armadillo_shed";
const BRUSH_TABLE: &str = "minecraft:brush/armadillo";

/// The entity event of a peek, whose sound and look are the client's.
pub(crate) const PEEK: u8 = 64;

/// `ArmadilloState`, by its id (`ByIdMap.continuous`) and name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum Phase {
    #[default]
    Idle,
    Rolling,
    Scared,
    Unrolling,
}

impl Phase {
    /// Its id, which clients are sent.
    pub(crate) fn id(self) -> i32 {
        match self {
            Self::Idle => 0,
            Self::Rolling => 1,
            Self::Scared => 2,
            Self::Unrolling => 3,
        }
    }

    /// `isThreatened()`: all but idle.
    pub(crate) fn threatened(self) -> bool {
        self != Self::Idle
    }

    /// `animationDuration()`.
    pub(crate) fn animation_duration(self) -> i32 {
        match self {
            Self::Idle => 0,
            Self::Rolling => 10,
            Self::Scared => 50,
            Self::Unrolling => 30,
        }
    }
}

/// What an armadillo keeps beyond an animal: its state and the moment it began
/// (`inStateTicks` is the moments since), and the moment its scute's timer comes to
/// nothing (`scuteTime` is that less now).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ArmadilloState {
    pub(crate) phase: Phase,
    pub(crate) phase_since: u32,
    pub(crate) scute_at: u32,
}

impl Own for ArmadilloState {}

impl ArmadilloState {
    /// `inStateTicks` as read at the moment `now`.
    pub(crate) fn in_state_ticks(&self, now: u32) -> i64 {
        (i64::from(now) - i64::from(self.phase_since)).max(0)
    }

    /// `scuteTime` at the moment `now`, below nothing where it waits.
    pub(crate) fn scute_time(&self, now: u32) -> i64 {
        i64::from(self.scute_at) - i64::from(now)
    }

    /// `switchToState(state)` at the moment `now`: its count begins again only where the
    /// state changes (`onSyncedDataUpdated` is called of a new value alone).
    pub(crate) fn switch_to(&mut self, phase: Phase, now: u32) {
        if phase != self.phase {
            self.phase = phase;
            self.phase_since = now;
        }
    }
}

/// The armadillo's own state of `mob`.
pub(crate) fn state_of(mob: &Mob) -> ArmadilloState {
    animal::read::<ArmadilloState>(&mob.state).1
}

/// Changes the armadillo's own state of `mob`.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut ArmadilloState)) {
    let (shared, mut own) = animal::read::<ArmadilloState>(&mob.state);
    let before = own;
    change(&mut own);
    if own != before {
        mob.state = animal::write(&shared, &own);
    }
}

/// `isScared()`: any state but idle.
pub(crate) fn is_scared(mob: &Mob) -> bool {
    state_of(mob).phase.threatened()
}

/// `pickNextScuteDropTime()`: 6000 to 11999 ticks, of its own draw.
fn next_scute_time(draws: &mut Random) -> u32 {
    draws.next_int(6000) + 6000
}

/// The stacks of the table `name`, which gives one scute and draws nothing where the
/// extraction does not compile it yet.
fn scute_drops(name: &str, kind: u16, draws: &mut Random) -> Vec<ItemStack> {
    if clustine_data::loot::table_by_name(name).is_none() {
        return vec![ItemStack {
            item: items::ARMADILLO_SCUTE,
            count: 1,
        }];
    }
    loot::named_drops(name, kind, false, loot::Facts::default(), draws)
}

/// `canStayRolledUp()`: not panicking, out of liquids, on no lead, riding nothing and
/// carrying no one.
pub(crate) fn can_stay_rolled_up(
    panicking: bool,
    in_liquid: bool,
    leashed: bool,
    passenger: bool,
    vehicle: bool,
) -> bool {
    !panicking && !in_liquid && !leashed && !passenger && !vehicle
}

/// What a rolling up or out shows: its game event and its sound (`makeSound`, the
/// voice's two draws), in the order made.
fn act(mob: &Mob, sound: i32, draws: &mut Random, events: &mut Vec<Event>) {
    events.push(Event::GameEvent(GameEvent::EntityAction));
    let pitch = voice_pitch(draws, animal::is_baby(mob));
    events.push(Event::Sound {
        sound,
        volume: 1.0,
        pitch,
    });
}

/// `rollUp()` of an armadillo not scared, at the moment `now`: it stops where it is
/// (`stopInPlace`: its path, its inputs, its motion), its love ends, it acts with the
/// roll's sound, and it is rolling.
fn roll_up(
    mob: &mut Mob,
    motion: &mut Vec3,
    now: u32,
    draws: &mut Random,
    events: &mut Vec<Event>,
) {
    if is_scared(mob) {
        return;
    }
    mob.body.path.followed = None;
    let inputs = &mut mob.body.inputs;
    inputs.xxa = 0.0;
    inputs.yya = 0.0;
    inputs.zza = 0.0;
    inputs.speed = 0.0;
    *motion = [0.0; 3];
    animal::update(mob, animal::Animal::reset_love);
    act(mob, sounds::ENTITY_ARMADILLO_ROLL, draws, events);
    update(mob, |own| own.switch_to(Phase::Rolling, now));
}

/// `rollOut()` of a scared armadillo, at the moment `now`: it acts with the unroll's
/// sound and is idle.
fn roll_out(mob: &mut Mob, now: u32, draws: &mut Random, events: &mut Vec<Event>) {
    if !is_scared(mob) {
        return;
    }
    act(mob, sounds::ENTITY_ARMADILLO_UNROLL_FINISH, draws, events);
    update(mob, |own| own.switch_to(Phase::Idle, now));
}

/// The armadillo.
pub(crate) struct Armadillo;

/// What the brain will call of an armadillo, until its provider is wired (part A and
/// the lead's N7j1): its own behaviours' bodies (`ArmadilloBallUp`, `ArmadilloPanic`'s
/// start, the core's rolling out, the walk sink that waits while it is scared), the
/// scare sensor's test, and the memory it sets when hurt.
#[allow(dead_code, reason = "what the armadillo's brain, not wired yet, calls")]
pub(crate) mod brain {
    use super::*;

    /// `DANGER_DETECTED_RECENTLY`'s expiry where it is set, and the times to it the
    /// rolled-up armadillo compares (75 for danger about, `UNROLLING`'s 30).
    pub(crate) const DANGER_TICKS: i64 = 80;
    pub(crate) const DANGER_ABOUT: i64 = 75;

    /// `ArmadilloBallUp`'s length, `5 × 60 × 20`.
    pub(crate) const BALL_UP_TICKS: i64 = 6000;

    /// `FollowTemptation`'s speed, and its distance: a baby's 1, else 2.
    pub(crate) const TEMPTED_SPEED: f32 = 1.25;
    pub(crate) fn tempted_distance(mob: &Mob) -> f64 {
        if animal::is_baby(mob) { 1.0 } else { 2.0 }
    }

    /// The seam where a hurt by something living sets `DANGER_DETECTED_RECENTLY` with
    /// an expiry of 80 (`actuallyHurt`): kept nowhere until the brain is wired.
    pub(crate) fn remember_danger(mob: &mut Mob) {
        let _ = mob;
    }

    /// `isPanicking()`: the brain's `IS_PANICKING`, which nothing sets until it is
    /// wired.
    pub(crate) fn panicking(mob: &Mob) -> bool {
        let _ = mob;
        false
    }

    /// `ArmadilloBallUp`'s fields: its next peek's countdown and whether danger was about
    /// at its last tick.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
    pub(crate) struct BallUp {
        pub(crate) next_peek_timer: i32,
        pub(crate) danger_was_around: bool,
    }

    /// `pickNextPeekTimer`: `SCARED`'s 50 and 100 to 400 more, of its own draw.
    pub(crate) fn next_peek_timer(draws: &mut Random) -> i32 {
        Phase::Scared.animation_duration() + draws.next_int_between(100, 400)
    }

    /// `ArmadilloBallUp.checkExtraStartConditions`: on the ground, out of liquids.
    pub(crate) fn ball_up_may_start(tick: &MobTick<'_>) -> bool {
        tick.living.body.on_ground && !(tick.living.body.in_water || tick.living.in_lava())
    }

    /// `ArmadilloBallUp.start`: it rolls up.
    pub(crate) fn ball_up_start(tick: &mut MobTick<'_>) {
        let now = tick.lived;
        roll_up(
            &mut tick.mob,
            &mut tick.living.body.motion,
            now,
            tick.draws,
            &mut tick.out.events,
        );
    }

    /// `ArmadilloBallUp.canStillUse`: while it is threatened.
    pub(crate) fn ball_up_may_go_on(mob: &Mob) -> bool {
        state_of(mob).phase.threatened()
    }

    /// `ArmadilloBallUp.stop`: one that may not stay rolled up rolls out.
    pub(crate) fn ball_up_stop(tick: &mut MobTick<'_>) {
        let stays = can_stay_rolled_up(
            panicking(&tick.mob),
            tick.living.body.in_water || tick.living.in_lava(),
            tick.leashed,
            tick.vehicle.is_some(),
            tick.carries,
        );
        if !stays {
            let now = tick.lived;
            roll_out(&mut tick.mob, now, tick.draws, &mut tick.out.events);
        }
    }

    /// `ArmadilloBallUp.tick`, `ttl` the time to the expiry of its danger memory
    /// (`i64::MAX` where it has none): the peek's countdown; a roll of more than ten
    /// ticks becomes scared, with the landing's sound on the ground, and nothing more
    /// that tick; else a change of danger about draws the next peek, a scared one peeks
    /// when its countdown is out and danger is about and unrolls when the danger is 30
    /// from its end, an unrolling one is scared again when danger comes back.
    pub(crate) fn ball_up_tick(tick: &mut MobTick<'_>, ball: &mut BallUp, ttl: i64) {
        let now = tick.lived;
        if ball.next_peek_timer > 0 {
            ball.next_peek_timer -= 1;
        }
        let own = state_of(&tick.mob);
        // `shouldSwitchToScaredState()`: rolling for more than ten ticks.
        if own.phase == Phase::Rolling && own.in_state_ticks(now) > 10 {
            update(&mut tick.mob, |own| own.switch_to(Phase::Scared, now));
            if tick.living.body.on_ground {
                sound(tick, sounds::ENTITY_ARMADILLO_LAND);
            }
            return;
        }
        let danger = ttl > DANGER_ABOUT;
        if danger != ball.danger_was_around {
            ball.next_peek_timer = next_peek_timer(tick.draws);
        }
        ball.danger_was_around = danger;
        let unrolling = i64::from(Phase::Unrolling.animation_duration());
        match own.phase {
            Phase::Scared => {
                if ball.next_peek_timer == 0 && tick.living.body.on_ground && danger {
                    tick.out.events.push(Event::Entity(PEEK));
                    ball.next_peek_timer = next_peek_timer(tick.draws);
                }
                if ttl < unrolling {
                    sound(tick, sounds::ENTITY_ARMADILLO_UNROLL_START);
                    update(&mut tick.mob, |own| own.switch_to(Phase::Unrolling, now));
                }
            }
            Phase::Unrolling if ttl > unrolling => {
                update(&mut tick.mob, |own| own.switch_to(Phase::Scared, now));
            }
            _ => {}
        }
    }

    /// `Entity.playSound(sound)`: at full volume and pitch, no draw.
    fn sound(tick: &mut MobTick<'_>, sound: i32) {
        tick.out.events.push(Event::Sound {
            sound,
            volume: 1.0,
            pitch: 1.0,
        });
    }

    /// `ARMADILLO_ROLLING_OUT`, of a brain without its danger memory: a scared armadillo
    /// rolls out. Whether it did.
    pub(crate) fn rolling_out(tick: &mut MobTick<'_>) -> bool {
        if !is_scared(&tick.mob) {
            return false;
        }
        let now = tick.lived;
        roll_out(&mut tick.mob, now, tick.draws, &mut tick.out.events);
        true
    }

    /// `ArmadilloPanic.start` before `AnimalPanic`'s: it rolls out.
    pub(crate) fn panic_start(tick: &mut MobTick<'_>) {
        let now = tick.lived;
        roll_out(&mut tick.mob, now, tick.draws, &mut tick.out.events);
    }

    /// `ArmadilloAi$1`, the walk sink, starts only where the armadillo is not scared.
    pub(crate) fn walk_sink_may_start(mob: &Mob) -> bool {
        !is_scared(mob)
    }

    /// What `isScaredBy` reads of a living entity near.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub(crate) struct Near {
        pub(crate) bounds: crate::physics::aabb::Aabb,
        /// Its entity type, by id.
        pub(crate) kind: u16,
        /// Whether it is the armadillo's `getLastHurtByMob()`.
        pub(crate) hurt_it_last: bool,
        /// A player's: spectating, sprinting, riding.
        pub(crate) player: Option<(bool, bool, bool)>,
    }

    /// `isScaredBy(entity)`, the scare sensor's test of the armadillo whose box is
    /// `own`: within 7, 2 and 7 of it, the undead, what last hurt it, and a player who
    /// sprints or rides (never one who spectates).
    pub(crate) fn is_scared_by(own: crate::physics::aabb::Aabb, near: &Near) -> bool {
        if !own.inflate([7.0, 2.0, 7.0]).intersects(near.bounds) {
            return false;
        }
        if clustine_blocks::data::entity_in_tag(i32::from(near.kind), "minecraft:undead") {
            return true;
        }
        if near.hurt_it_last {
            return true;
        }
        match near.player {
            Some((spectator, sprinting, riding)) => !spectator && (sprinting || riding),
            None => false,
        }
    }
}

/// What a fresh load of an armadillo resets (`inStateTicks` is not saved; the plan's
/// 2.10), at the moment `now`: for the brain's `fresh_load` hook when it comes.
#[allow(
    dead_code,
    reason = "the fresh load of N7j1's contracts, not built yet"
)]
pub(crate) fn fresh_load(mob: &mut Mob, now: u32) {
    update(mob, |own| own.phase_since = now);
}

impl MobType for Armadillo {
    /// The constructor's: `Animal`'s maluses and floating. Its goals are its brain's,
    /// which are not wired yet.
    fn register_goals(&self, setup: &mut Setup) {
        super::cow::animal_maluses(setup);
        setup.config.navigation.flags.can_float = true;
    }

    /// `getMaxHeadYRot()`: 32, and none while scared, which its tick's clamp holds.
    fn head(&self) -> HeadLimits {
        HeadLimits {
            max_y_rot: 32,
            ..HeadLimits::default()
        }
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        animal::walk_target_value(ai, pos, clustine_data::blocks::GRASS_BLOCK)
    }

    /// The constructor's draw after the brain's: the scute's first timer.
    fn construct(&self, mob: &mut Mob, own: &mut Random) {
        let time = next_scute_time(own);
        update(mob, |state| state.scute_at = time);
    }

    /// The body control turns nothing while it is scared.
    fn body_rotation_ticks(&self, tick: &MobTick<'_>) -> bool {
        !is_scared(&tick.mob)
    }

    /// `brain.tick` and `ArmadilloAi.updateActivity` (the seam its provider is wired
    /// at); then, alive, its scute's timer: run out, a grown one sheds a scute, with its
    /// sound (two draws) and the place's game event, and draws its next; then `Animal`'s.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        if !tick.mob.body.vitals.dying() {
            let next = tick.lived.saturating_add(1);
            let own = state_of(&tick.mob);
            if own.scute_time(next) <= 0 && !animal::is_baby(&tick.mob) {
                let at = tick.living.body.position;
                let mut shed = false;
                for stack in scute_drops(SHED_TABLE, tick.kind, tick.draws) {
                    if stack.count > 0 {
                        shed = true;
                        tick.out.local.push(Local::Drop {
                            stack,
                            at,
                            pick_up_delay: crate::region::DEFAULT_PICK_UP_DELAY,
                        });
                    }
                }
                if shed {
                    let pitch = (tick.draws.next_float() - tick.draws.next_float()) * 0.2 + 1.0;
                    tick.out.events.push(Event::Sound {
                        sound: sounds::ENTITY_ARMADILLO_SCUTE_DROP,
                        volume: 1.0,
                        pitch,
                    });
                    tick.out
                        .events
                        .push(Event::GameEvent(GameEvent::EntityPlace));
                }
                let time = next_scute_time(tick.draws);
                update(&mut tick.mob, |own| {
                    own.scute_at = next.saturating_add(time)
                });
            }
        }
        animal::custom_server_ai_step(tick);
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step(tick);
    }

    /// `Armadillo.tick` after the animal's: a scared armadillo's head is held to its
    /// body. Its `inStateTicks` counts by itself.
    fn after_tick(&self, tick: &mut MobTick<'_>) {
        if is_scared(&tick.mob) {
            super::camel::clamp_head_to_body(&mut tick.mob, 0.0);
        }
    }

    /// `Armadillo.mobInteract`: a brush on a grown one brushes a scute off (its table,
    /// its sound, the interaction's game event; the brush worn by 16, which a creative
    /// player's is not); a scared one is not fed (`FAIL`, which passes on as nothing
    /// done); else an animal's use, with its eating sound.
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        if animal::is_item(used.held, items::BRUSH) && !animal::is_baby(used.mob) {
            for stack in scute_drops(BRUSH_TABLE, used.kind, used.draws) {
                if stack.count > 0 {
                    used.did.push(Did::Drop {
                        stack,
                        at: used.position,
                        pick_up_delay: crate::region::DEFAULT_PICK_UP_DELAY,
                        push: None,
                    });
                }
            }
            used.did.push(Did::Sound {
                sound: sounds::ENTITY_ARMADILLO_BRUSH,
                source: SOUND_SOURCE_NEUTRAL,
                at: used.position,
                volume: 1.0,
                pitch: 1.0,
                but_the_player: false,
            });
            used.did
                .push(Did::GameEvent(GameEvent::EntityInteract.name()));
            return Interaction::Success;
        }
        if is_scared(used.mob) {
            return Interaction::Pass;
        }
        animal::mob_interact(used, FOOD, Some(sounds::ENTITY_ARMADILLO_EAT))
    }

    fn breed_offspring(&self, _: &mut Breeding<'_>) -> Option<Mob> {
        Some(Mob::fresh())
    }

    /// `Armadillo.hurtServer`: a scared one takes `(amount − 1) / 2`.
    fn hurt_amount(&self, mob: &Mob, amount: f32) -> f32 {
        if is_scared(mob) {
            (amount - 1.0) / 2.0
        } else {
            amount
        }
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    /// `Armadillo.actuallyHurt` after `Animal`'s: of a living mob with AI, something
    /// living that hurt it is a danger it remembers, and it rolls up where it may stay
    /// so; fire, lava and the like of `#panic_environmental_causes` roll it out.
    fn after_actually_hurt(&self, struck: &mut Struck<'_>, source: &DamageSource) {
        if struck.mob.body.flags.no_ai || struck.dying() {
            return;
        }
        let now = struck.moment;
        if source.cause.as_ref().is_some_and(|cause| cause.living) {
            brain::remember_danger(struck.mob);
            let stays = can_stay_rolled_up(
                brain::panicking(struck.mob),
                struck.in_liquid,
                struck.leashed,
                struck.vehicle.is_some(),
                struck.carries,
            );
            if stays {
                roll_up(
                    struck.mob,
                    struck.motion,
                    now,
                    struck.draws,
                    &mut struck.out.events,
                );
            }
        } else if source.kind.is("minecraft:panic_environmental_causes") {
            roll_out(struck.mob, now, struck.draws, &mut struck.out.events);
        }
    }

    /// A baby's size: the type's scaled by 0.6, its eye at 0.21875.
    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        if !animal::is_baby(mob) {
            return None;
        }
        let info = clustine_data::EntityTypeInfo::of(clustine_data::entity_types::ARMADILLO);
        Some(Dimensions {
            width: info.width * 0.6,
            height: info.height * 0.6,
            eye_height: 0.218_75,
        })
    }

    fn ambient_sound(&self, mob: &Mob) -> Option<i32> {
        (!is_scared(mob)).then_some(sounds::ENTITY_ARMADILLO_AMBIENT)
    }

    /// `playStepSound`: its own at 0.15 and 1.
    fn step_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_ARMADILLO_STEP)
    }

    fn hurt_sound(&self, mob: &Mob, _: &DamageSource) -> Option<i32> {
        Some(if is_scared(mob) {
            sounds::ENTITY_ARMADILLO_HURT_REDUCED
        } else {
            sounds::ENTITY_ARMADILLO_HURT
        })
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_ARMADILLO_DEATH)
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let phase = state_of(mob).phase;
        let mut values = animal::shown(mob);
        values.push((
            STATE_INDEX,
            DataShown::Serialised(Serialiser::ArmadilloState, phase.id()),
            phase == Phase::Idle,
        ));
        values
    }
}
