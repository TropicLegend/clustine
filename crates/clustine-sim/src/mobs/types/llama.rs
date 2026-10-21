//! Llamas (`Llama`, an `AbstractChestedHorse`; `docs/groundwork/n7-brief.md`, section
//! 5.16): of four coats and a strength of one to five, which is its chest's columns;
//! wild ones of a temper of 30 at most, never rearing or grazing; fed wheat or hay, hay
//! putting a tamed one in love; a carpet on its body. A llama joins a caravan behind a
//! leashed llama, or behind the last of one, up to eight behind the leashed head; it
//! spits at who hurt it, once, and at wild wolves near it.
//!
//! Caravans are links in memory only in the game (the brief's finding 25): a llama keeps
//! the llama it follows, and that a llama has one following it is read from the others
//! near it.
//!
//! Named differences: the spit (`LlamaSpit`) is a projectile, N9's: the llama spits (its
//! sound and its `didSpit`) and nothing flies or hurts; a llama's own fall damage from six
//! blocks and its passengers' share are the body's and the riding's; a tamed wolf is not
//! told from a wild one (the tamed animals' part brings the wolf's tame flag), so a llama
//! spits at any wolf near it; a llama that follows another that dies or goes out of sight
//! frees it at once (the game's head keeps its dead tail); a tail is looked for within 32
//! blocks of its head.

use clustine_data::{attributes as ids, entity_types, sounds};
use clustine_world::{BlockPos, EntityData, EntityId, ItemStack, LeashHolder};
use serde::{Deserialize, Serialize};

use super::donkey::randomize_chested;
use super::equine::{self, Behaviour, Equine, HorseBreedGoal, RunAroundLikeCrazyGoal};
use super::horse::common;
use super::{
    AgeableGroup, Breeding, Dimensions, Interaction, Mate, MobType, SpawnGroup, Spawning, Used,
    animal,
};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    FloatGoal, HurtByTargetGoal, Items, LookAtPlayerGoal, PanicGoal, RandomLookAroundGoal,
    RandomStrollGoal, Targeting, TemptGoal,
};
use crate::mobs::ai::maths::{lerp, reduced_tick_delay};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::{self, Class, Conditions, Looked};
use crate::mobs::ai::{Ai, Setup, controls, navigation};
use crate::mobs::body::Event;
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::tick::MobTick;
use crate::physics::aabb::Aabb;
use crate::physics::{self, Vec3};
use crate::region::Random;
use crate::shown::DataShown;

/// `DATA_STRENGTH_ID` and `DATA_VARIANT_ID`.
const STRENGTH_INDEX: u8 = 20;
const VARIANT_INDEX: u8 = 21;

/// `Llama$Variant`: creamy, white, brown, gray.
pub(crate) const COATS: u32 = 4;

/// `CARAVAN_LIMIT`: how many may follow a leashed head.
const CARAVAN_LIMIT: u32 = 8;

pub(crate) const EQUINE: Equine = Equine {
    kind: entity_types::LLAMA,
    max_temper: 30,
    rears: false,
    grazes: false,
    chested: true,
    grows: true,
    baby_scale: 0.5,
    food: Items::Tag("minecraft:llama_food"),
    behaviour: Behaviour::Horse,
    angry: |_| Some(sounds::ENTITY_LLAMA_ANGRY),
    eat: |_| Some(sounds::ENTITY_LLAMA_EAT),
    chest_sound: sounds::ENTITY_LLAMA_CHEST,
};

/// `setRandomStrength(r)`: one in 25 may reach five, the others three.
pub(crate) fn random_strength(r: &mut dyn clustine_world::generation::Draws) -> i32 {
    let most = if r.next_float() < 0.04 { 5 } else { 3 };
    (1 + r.next_int(most) as i32).clamp(1, 5)
}

/// `Llama.registerGoals`, which the trader llama's builds on: the llama's goals and
/// target goals, its breeding partners those `partners` takes.
pub(crate) fn register_goals(
    setup: &mut Setup,
    equine: &'static Equine,
    partners: fn(u16) -> bool,
) {
    super::cow::animal_maluses(setup);
    setup.add_goal(0, FloatGoal);
    setup.add_goal(1, RunAroundLikeCrazyGoal { speed: 1.2, equine });
    setup.add_goal(
        2,
        LlamaFollowCaravanGoal {
            speed: 2.099_999_904_632_568_4,
        },
    );
    setup.add_goal(
        3,
        RangedAttackGoal {
            speed: 1.25,
            interval_min: 40,
            interval_max: 40,
            radius: 20.0,
        },
    );
    setup.add_goal(3, PanicGoal { speed: 1.2 });
    setup.add_goal(
        4,
        HorseBreedGoal {
            speed: 1.0,
            partners,
        },
    );
    setup.add_goal(
        5,
        TemptGoal::new(1.25, Items::Tag("minecraft:llama_tempt_items"), false),
    );
    setup.add_goal(6, animal::FollowParentGoal { speed: 1.0 });
    setup.add_goal(7, RandomStrollGoal::avoiding_water(0.7));
    setup.add_goal(8, LookAtPlayerGoal::new(Class::Player, 6.0));
    setup.add_goal(9, RandomLookAroundGoal);
    setup.add_target(1, LlamaHurtByTargetGoal);
    setup.add_target(2, LlamaAttackWolfGoal);
}

/// The llama.
pub(crate) struct Llama;

/// A llama's (or trader llama's) `finalizeSpawn`: its strength, a coat (the herd's, or
/// one drawn that the herd then shares, `LlamaGroupData`), then the chested horse's
/// health and the herd's babies one in twenty, all of the level's random.
pub(crate) fn finalize_spawn(spawn: &mut Spawning<'_>, kind: u16) {
    let strength = random_strength(spawn.level);
    let coat = match *spawn.group {
        Some(SpawnGroup::Llama { coat, .. }) => coat,
        _ => {
            let coat = spawn.level.next_int(COATS) as i32;
            *spawn.group = Some(SpawnGroup::Llama {
                ageable: AgeableGroup::new(true),
                coat,
            });
            coat
        }
    };
    equine::update(spawn.mob, |horse| {
        horse.strength = strength;
        horse.llama_variant = coat;
    });
    randomize_chested(spawn.mob, kind, spawn.level);
    equine::clamp_health(spawn.mob, kind);
    equine::finalize_ageable(spawn);
}

/// `Llama.getBreedOffspring(partner)`: its stats about its parents', then a strength of
/// up to the stronger's (one in 33 a point more), then either parent's coat, all of the
/// breeding parent's random.
pub(crate) fn breed_offspring(breeding: &mut Breeding<'_>, kind: u16) -> Mob {
    let mut child = equine::foal();
    equine::set_offspring_attributes(
        (breeding.this, kind),
        (breeding.partner, breeding.partner_kind),
        (&mut child, kind),
        breeding.draws,
    );
    let this = equine::horse_of(breeding.this);
    let partner = equine::horse_of(breeding.partner);
    let most = this.strength.max(partner.strength).max(1) as u32;
    let mut strength = breeding.draws.next_int(most) as i32 + 1;
    if breeding.draws.next_float() < 0.03 {
        strength += 1;
    }
    let coat = if breeding.draws.next_bool() {
        this.llama_variant
    } else {
        partner.llama_variant
    };
    equine::update(&mut child, |horse| {
        horse.strength = strength.clamp(1, 5);
        horse.llama_variant = coat;
    });
    child
}

/// What a llama's and a trader llama's modules share of `MobType`.
pub(crate) mod common_llama {
    use super::*;

    pub(crate) fn shown(mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let horse = equine::horse_of(mob);
        let mut values = equine::shown_chested(mob);
        values.push((
            STRENGTH_INDEX,
            DataShown::Int(horse.strength),
            horse.strength == 0,
        ));
        values.push((
            VARIANT_INDEX,
            DataShown::Int(horse.llama_variant),
            horse.llama_variant == 0,
        ));
        values
    }
}

impl MobType for Llama {
    fn register_goals(&self, setup: &mut Setup) {
        register_goals(setup, &EQUINE, equine::is_llama);
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        common::walk_target_value(ai, pos)
    }

    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::custom_server_ai_step(tick);
    }

    fn before_ai_step(&self, tick: &mut MobTick<'_>) {
        equine::before_ai_step(tick);
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        common::after_ai_step(tick, &EQUINE);
    }

    fn immobile(&self, tick: &MobTick<'_>) -> bool {
        equine::immobile(tick, &EQUINE)
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        equine::mob_interact(used, &EQUINE, true)
    }

    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        finalize_spawn(spawn, entity_types::LLAMA as u16);
    }

    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        Some(breed_offspring(breeding, entity_types::LLAMA as u16))
    }

    fn can_mate(&self, this: Mate<'_>, partner: Mate<'_>) -> bool {
        equine::llama_mates(this, partner)
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    fn hurt_taken(&self, mob: &mut Mob, draws: &mut Random, moment: u32) {
        equine::hurt_taken(mob, entity_types::LLAMA as u16, draws, moment);
    }

    fn drop_equipment(&self, mob: &mut Mob) -> Vec<ItemStack> {
        equine::drop_equipment(mob)
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        equine::dimensions(mob, &EQUINE)
    }

    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_LLAMA_AMBIENT)
    }

    fn ambient_interval(&self) -> u32 {
        400
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_LLAMA_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_LLAMA_DEATH)
    }

    fn sound_volume(&self) -> f32 {
        0.8
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        common_llama::shown(mob)
    }
}

/// A llama a caravan's goal reads: its key, its place, whether it is leashed, and the
/// llama it follows.
#[derive(Debug, Clone)]
struct CaravanLlama {
    key: EntityId,
    position: Vec3,
    leashed: bool,
    alive: bool,
    head: Option<EntityId>,
}

impl CaravanLlama {
    fn of(entity: &clustine_world::Entity) -> Option<Self> {
        if !equine::is_llama(entity.kind) {
            return None;
        }
        let EntityData::Mob { .. } = &entity.data else {
            return None;
        };
        let mob = Mob::decode(&entity.data).ok()?;
        Some(Self {
            key: entity.key,
            position: entity.position,
            leashed: entity.riding.leash.is_some(),
            alive: !mob.body.vitals.dying(),
            head: equine::horse_of(&mob).caravan_head,
        })
    }
}

/// The llama `key` of the view.
fn caravan_llama(ai: &Ai<'_, '_>, key: EntityId) -> Option<CaravanLlama> {
    let seen = ai.tick.world.entity(key)?;
    CaravanLlama::of(seen.entity)
}

/// `LlamaFollowCaravanGoal`'s state: its speed, which grows while it lags far behind,
/// and the grace before it gives up.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Following {
    /// `speedModifier` where it is not the goal's own.
    pub(crate) speed: Option<f64>,
    pub(crate) dist_check_counter: i32,
}

/// `LlamaFollowCaravanGoal(llama, speed)`: a llama not on a lead joins behind the nearest
/// llama of a caravan without a follower within 9 by 4 by 9 blocks, or failing one the
/// nearest leashed llama without one, if it is no nearer than two blocks and within
/// eight of a leashed head; it follows two blocks behind, faster while more than 26 off.
pub(crate) struct LlamaFollowCaravanGoal {
    pub(crate) speed: f64,
}

impl LlamaFollowCaravanGoal {
    /// The llamas within the box of `bounds` grown by 32 that follow another, by whom
    /// they follow: `hasCaravanTail` of those near it.
    fn followed(ai: &Ai<'_, '_>, bounds: &Aabb) -> Vec<EntityId> {
        let around = bounds.inflate([32.0; 3]);
        ai.tick
            .world
            .entities_in(&around)
            .iter()
            .filter(|seen| seen.entity.key != ai.tick.key)
            .filter_map(|seen| CaravanLlama::of(seen.entity))
            .filter_map(|llama| llama.head)
            .collect()
    }

    /// `firstIsLeashed(llama, n)`: a leashed head within eight links.
    fn first_is_leashed(ai: &Ai<'_, '_>, llama: &CaravanLlama, depth: u32) -> bool {
        if depth > CARAVAN_LIMIT {
            return false;
        }
        let Some(head) = llama.head.and_then(|key| caravan_llama(ai, key)) else {
            return false;
        };
        if head.leashed {
            return true;
        }
        Self::first_is_leashed(ai, &head, depth + 1)
    }

    /// Itself as a caravan's llama.
    fn me(ai: &Ai<'_, '_>) -> CaravanLlama {
        CaravanLlama {
            key: ai.tick.key,
            position: ai.position(),
            leashed: ai.tick.leashed,
            alive: !ai.tick.mob.body.vitals.dying(),
            head: equine::horse_of(&ai.tick.mob).caravan_head,
        }
    }

    /// `leaveCaravan()`.
    fn leave(ai: &mut Ai<'_, '_>) {
        equine::update(&mut ai.tick.mob, |horse| horse.caravan_head = None);
    }

    fn speed(&self, state: &Following) -> f64 {
        state.speed.unwrap_or(self.speed)
    }
}

impl Goal for LlamaFollowCaravanGoal {
    type State = Following;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, _: &mut Following, ai: &mut Ai<'_, '_>) -> bool {
        let me = Self::me(ai);
        if me.leashed || me.head.is_some() {
            return false;
        }
        let bounds =
            Aabb::of_entity(ai.position(), ai.width(), ai.height()).inflate([9.0, 4.0, 9.0]);
        let llamas: Vec<CaravanLlama> = ai
            .tick
            .world
            .entities_in(&bounds)
            .iter()
            .filter(|seen| seen.entity.key != ai.tick.key)
            .filter_map(|seen| CaravanLlama::of(seen.entity))
            .collect();
        let followed = Self::followed(ai, &bounds);
        let has_tail = |llama: &CaravanLlama| followed.contains(&llama.key);
        let from = ai.position();
        let mut best: Option<(f64, &CaravanLlama)> = None;
        // The nearest in a caravan without a tail, the later of two as near.
        for llama in &llamas {
            if llama.head.is_none() || has_tail(llama) {
                continue;
            }
            let distance = physics::distance_sqr(from, llama.position);
            if best.is_none_or(|(nearest, _)| distance <= nearest) {
                best = Some((distance, llama));
            }
        }
        if best.is_none() {
            for llama in &llamas {
                if !llama.leashed || has_tail(llama) {
                    continue;
                }
                let distance = physics::distance_sqr(from, llama.position);
                if best.is_none_or(|(nearest, _)| distance <= nearest) {
                    best = Some((distance, llama));
                }
            }
        }
        let Some((distance, head)) = best else {
            return false;
        };
        if distance < 4.0 {
            return false;
        }
        if !head.leashed && !Self::first_is_leashed(ai, head, 1) {
            return false;
        }
        let key = head.key;
        equine::update(&mut ai.tick.mob, |horse| horse.caravan_head = Some(key));
        true
    }

    fn can_continue(&self, state: &mut Following, ai: &mut Ai<'_, '_>) -> bool {
        let me = Self::me(ai);
        let Some(head) = me.head.and_then(|key| caravan_llama(ai, key)) else {
            return false;
        };
        if !head.alive || !Self::first_is_leashed(ai, &me, 0) {
            return false;
        }
        let distance = physics::distance_sqr(ai.position(), head.position);
        if distance > 676.0 {
            if self.speed(state) <= 3.0 {
                state.speed = Some(self.speed(state) * 1.2);
                state.dist_check_counter = reduced_tick_delay(40);
                return true;
            }
            if state.dist_check_counter == 0 {
                return false;
            }
        }
        if state.dist_check_counter > 0 {
            state.dist_check_counter -= 1;
        }
        true
    }

    fn stop(&self, state: &mut Following, ai: &mut Ai<'_, '_>) {
        Self::leave(ai);
        state.speed = Some(2.1);
    }

    fn tick(&self, state: &mut Following, ai: &mut Ai<'_, '_>) {
        let me = Self::me(ai);
        if me.head.is_none() {
            return;
        }
        let tied = ai
            .tick
            .world
            .entity(ai.tick.key)
            .and_then(|seen| seen.entity.riding.leash.as_ref().map(|leash| leash.holder))
            .is_some_and(|holder| matches!(holder, LeashHolder::Fence(_)));
        if tied {
            return;
        }
        let Some(head) = me.head.and_then(|key| caravan_llama(ai, key)) else {
            return;
        };
        let at = ai.position();
        // `distanceTo` is a float's.
        let distance = f64::from(physics::distance_sqr(at, head.position).sqrt() as f32);
        let towards = [
            head.position[0] - at[0],
            head.position[1] - at[1],
            head.position[2] - at[2],
        ];
        let length =
            (towards[0] * towards[0] + towards[1] * towards[1] + towards[2] * towards[2]).sqrt();
        let unit = if length < 1.0e-5 {
            [0.0; 3]
        } else {
            towards.map(|value| value / length)
        };
        let scale = (distance - 2.0).max(0.0);
        let wanted = [
            at[0] + unit[0] * scale,
            at[1] + unit[1] * scale,
            at[2] + unit[2] * scale,
        ];
        let speed = self.speed(state);
        navigation::move_to(ai, wanted, speed);
    }
}

/// `RangedAttackGoal`'s state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Ranged {
    pub(crate) target: Option<EntityId>,
    pub(crate) attack_time: i32,
    pub(crate) see_time: i32,
}

impl Default for Ranged {
    fn default() -> Self {
        Self {
            target: None,
            attack_time: -1,
            see_time: 0,
        }
    }
}

/// `RangedAttackGoal(mob, speed, interval, radius)`: towards its target until it has
/// seen it for five ticks within the radius, then it stands, and every interval it
/// spits.
pub(crate) struct RangedAttackGoal {
    pub(crate) speed: f64,
    pub(crate) interval_min: i32,
    pub(crate) interval_max: i32,
    pub(crate) radius: f32,
}

impl Goal for RangedAttackGoal {
    type State = Ranged;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    fn every_tick(&self) -> bool {
        true
    }

    fn can_use(&self, state: &mut Ranged, ai: &mut Ai<'_, '_>) -> bool {
        let Some(target) = sensing::target(ai).filter(|target| target.alive) else {
            return false;
        };
        state.target = Some(target.key);
        true
    }

    fn can_continue(&self, state: &mut Ranged, ai: &mut Ai<'_, '_>) -> bool {
        if self.can_use(state, ai) {
            return true;
        }
        let alive = state
            .target
            .and_then(|key| sensing::find(ai, key))
            .is_some_and(|target| target.alive);
        alive && !navigation::is_done(ai)
    }

    fn stop(&self, state: &mut Ranged, _: &mut Ai<'_, '_>) {
        *state = Ranged::default();
    }

    fn tick(&self, state: &mut Ranged, ai: &mut Ai<'_, '_>) {
        let Some(target) = state.target.and_then(|key| sensing::find(ai, key)) else {
            return;
        };
        let distance = physics::distance_sqr(ai.position(), target.position);
        let sees = sensing::has_line_of_sight(ai, &target);
        if sees {
            state.see_time += 1;
        } else {
            state.see_time = 0;
        }
        let radius_sqr = self.radius * self.radius;
        if distance <= f64::from(radius_sqr) && state.see_time >= 5 {
            navigation::stop(ai);
        } else {
            navigation::move_to_entity(ai, target.block_position(), self.speed);
        }
        controls::set_look_at(
            ai,
            [target.position[0], target.eye_y, target.position[2]],
            30.0,
            30.0,
        );
        state.attack_time -= 1;
        let span = (self.interval_max - self.interval_min) as f32;
        if state.attack_time == 0 {
            if !sees {
                return;
            }
            let f = distance.sqrt() as f32 / self.radius;
            let power = f.clamp(0.1, 1.0);
            spit(ai, &target, power);
            state.attack_time = (f * span + self.interval_min as f32).floor() as i32;
        } else if state.attack_time < 0 {
            let t = distance.sqrt() / f64::from(self.radius);
            let lerped = lerp(
                t,
                f64::from(self.interval_min),
                f64::from(self.interval_max),
            );
            state.attack_time = lerped.floor() as i32;
        }
    }
}

/// `performRangedAttack(target, power)`: `spit(target)`. The spit itself is N9's
/// projectile (named); the llama's sound with its two draws and its `didSpit` are here.
fn spit(ai: &mut Ai<'_, '_>, _target: &Looked, _power: f32) {
    let pitch = 1.0 + (ai.tick.draws.next_float() - ai.tick.draws.next_float()) * 0.2;
    ai.tick.out.events.push(Event::Sound {
        sound: sounds::ENTITY_LLAMA_SPIT,
        volume: 1.0,
        pitch,
    });
    equine::update(&mut ai.tick.mob, |horse| horse.did_spit = true);
}

/// `Llama$LlamaHurtByTargetGoal`: who hurt it is its target, until it has spat once.
pub(crate) struct LlamaHurtByTargetGoal;

impl Goal for LlamaHurtByTargetGoal {
    type State = Targeting;

    fn flags(&self) -> u8 {
        flags::TARGET
    }

    fn can_use(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        HurtByTargetGoal { ignore: &[] }.can_use(state, ai)
    }

    fn can_continue(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        if equine::horse_of(&ai.tick.mob).did_spit {
            equine::update(&mut ai.tick.mob, |horse| horse.did_spit = false);
            return false;
        }
        HurtByTargetGoal { ignore: &[] }.can_continue(state, ai)
    }

    fn start(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        HurtByTargetGoal { ignore: &[] }.start(state, ai);
    }

    fn stop(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        HurtByTargetGoal { ignore: &[] }.stop(state, ai);
    }
}

/// Whether a wolf is a wild one (`!isTame()`). **Named**: until the tamed animals' part
/// gives the wolf's tame flag, every wolf is taken as wild.
fn wild_wolf(_: &Looked) -> bool {
    true
}

/// `Llama$LlamaAttackWolfGoal`: a `NearestAttackableTargetGoal` of wild wolves, one try in
/// eight of its steps, within a quarter of its follow range.
pub(crate) struct LlamaAttackWolfGoal;

impl LlamaAttackWolfGoal {
    /// `getFollowDistance()`: a quarter of the follow range.
    fn follow(ai: &Ai<'_, '_>) -> f64 {
        ai.attribute(ids::FOLLOW_RANGE) * 0.25
    }
}

impl Goal for LlamaAttackWolfGoal {
    type State = Targeting;

    fn flags(&self) -> u8 {
        flags::TARGET
    }

    fn can_use(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        let interval = reduced_tick_delay(16);
        if interval > 0 && ai.tick.draws.next_int(interval as u32) != 0 {
            return false;
        }
        let follow = Self::follow(ai);
        let conditions = Conditions::combat().range(follow);
        let [x, _, z] = ai.position();
        let eyes = [x, ai.eye_y(), z];
        let bounds = Aabb::of_entity(ai.position(), ai.width(), ai.height())
            .inflate([follow, follow, follow]);
        let candidates = sensing::things_in(ai, bounds, Class::Kinds(&WOLVES));
        let found = sensing::nearest(ai, candidates, &conditions, &wild_wolf, eyes);
        state.found = found.map(|found| found.key);
        state.found.is_some()
    }

    /// `TargetGoal.canContinueToUse`, its follow distance the quarter, with no need to
    /// see the wolf.
    fn can_continue(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        let held = ai.tick.mob.body.combat.target.or(state.target_mob);
        let Some(target) = held.and_then(|key| sensing::find(ai, key)) else {
            return false;
        };
        if !sensing::can_attack(&target) {
            return false;
        }
        let follow = Self::follow(ai);
        if target.distance_sqr(ai.position()) > follow * follow {
            return false;
        }
        sensing::set_target(ai, Some(&target));
        true
    }

    fn start(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        let found = state.found.and_then(|key| sensing::find(ai, key));
        sensing::set_target(ai, found.as_ref());
        state.reach_cache = 0;
        state.reach_cache_time = 0;
        state.unseen_ticks = 0;
    }

    fn stop(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        sensing::set_target(ai, None);
        state.target_mob = None;
    }
}

/// The wolf's type, as the wolf goal's class.
const WOLVES: [u16; 1] = [entity_types::WOLF as u16];
