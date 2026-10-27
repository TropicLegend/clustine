//! The iron golem (`IronGolem`, `AbstractGolem` and the golem's goals;
//! `docs/groundwork/n7k-brief.md`, sections 1.1 to 1.3, 11.6; the plan's sections 1.1,
//! 2.5 and 10): built by a player or made by an egg, it strolls, looks at players,
//! holds out a poppy in daylight, is repaired by iron ingots, never drowns and fights
//! monsters, any `Enemy` but a creeper, with its own hit that flings what it takes
//! upwards. Its anger is `NeutralMob`'s ([`neutral`]), brought up to date every tick.
//!
//! Its attack's and its offer's counters (`attackAnimationTick`, `offerFlowerTick`),
//! which only clients read, are kept as the moments they end at (ADR-0028's derived
//! counters): a restore in the middle of one goes on with it.
//!
//! Its village (`docs/groundwork/n10-plan.md`, section 10; the brief's 8.1): it defends
//! the villagers near it against a player one of them thinks badly of
//! (`DefendVillageTargetGoal`, reading each villager's reputations at or below -100,
//! which its record carries also in a copy), goes back towards a village it has strayed
//! from (`MoveBackToVillageGoal`) and strolls about it, towards a villager who wants a
//! golem or an occupied point of interest of a village's section near
//! (`GolemRandomStrollInVillageGoal`).
//!
//! Named differences: the player it would defend a village against is held by key, and
//! one no longer seen is not taken (the game keeps the player itself, and asks it
//! whether it is creative or a spectator, for as long as the golem lives); the poppy is
//! given only to a mob of the region's own (a copper golem across a line is named); a
//! player is not hurt by it (`Local::Hurt`'s); the advancement for building one is not
//! kept; its leash's offset is the client's (`getLeashOffset`, which nothing on the
//! server reads).

use clustine_blocks::data::entity_in_tag;
use clustine_data::{BlockState, attributes, blocks, entity_types, items, sounds};
use clustine_world::{EntityId, TypeState};
use serde::{Deserialize, Serialize};

use super::neutral::{self, Anger};
use super::tamable::Idle;
use super::villager::{self, Shown};
use super::{Did, Interaction, MobType, Used, polar_bear, template};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    HurtByTargetGoal, LookAtPlayerGoal, MeleeAttackGoal, NearestAttackableTargetGoal,
    RandomLookAroundGoal, Strolling, TargetRules, Targeting, ai_bounds, look_at_thing,
};
use crate::mobs::ai::maths::reduced_tick_delay;
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::{self, Class, Conditions, Looked};
use crate::mobs::ai::village_goals::{
    HALF_PI, bottom_centre, find_section_closest_to_village, section_centre, section_cube,
};
use crate::mobs::ai::{Ai, Setup, navigation, random_pos};
use crate::mobs::body::hurt::{DamageSource, Struck};
use crate::mobs::body::{Event, ladder};
use crate::mobs::tick::{Local, MobTick};
use crate::physics::Vec3;
use crate::region::pois::{Occupancy, section_of};
use crate::shown::DataShown;

/// The synced data's index of its flags (`DATA_FLAGS_ID`).
const FLAGS_INDEX: u8 = 16;

/// The flag of a golem a player built (`isPlayerCreated`).
const PLAYER_CREATED: u8 = 1;

/// The version of the iron golem's type state.
const VERSION: u8 = 1;

/// How long its arms swing after a hit (`attackAnimationTick = 10`).
const ATTACK_ANIMATION_TICKS: u32 = 10;

/// How long it holds out a poppy (`OfferFlowerGoal.OFFER_TICKS`).
pub(crate) const OFFER_TICKS: u32 = 400;

/// What an iron ingot heals (`IRON_INGOT_HEAL_AMOUNT`).
const INGOT_HEALS: f32 = 25.0;

/// The fling of what its hit takes (`0.4000000059604645`, a `float` widened).
pub(crate) const FLING: f32 = 0.4;

/// What an iron golem keeps: whether a player built it (saved as `PlayerCreated`), its
/// anger (saved as `NeutralMob`'s), the moments its arms' swing and its offer end at
/// (not saved by the game: its counters), and its cracks as a hurt began, which only
/// that hurt reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct IronGolemState {
    pub(crate) player_created: bool,
    pub(crate) anger: Anger,
    /// The moment `attackAnimationTick` is down to nothing.
    pub(crate) attack_animation_end: u32,
    /// The moment `offerFlowerTick` is down to nothing.
    pub(crate) offer_flower_end: u32,
    /// `getCrackiness()` as its `hurtServer` began, until the hurt is over.
    pub(crate) cracks_before: Option<u8>,
}

/// The iron golem's state of `mob`: a fresh golem's for a record of nothing or one this
/// build cannot read.
pub(crate) fn state_of(mob: &Mob) -> IronGolemState {
    read(&mob.state)
}

/// The iron golem's state of a type state.
pub(crate) fn read(state: &TypeState) -> IronGolemState {
    if state.bytes.is_empty() || state.version != VERSION {
        return IronGolemState::default();
    }
    postcard::from_bytes(&state.bytes).unwrap_or_default()
}

/// The type state of `own`: nothing for a fresh golem's.
pub(crate) fn write(own: &IronGolemState) -> TypeState {
    if *own == IronGolemState::default() {
        return TypeState::default();
    }
    TypeState {
        version: VERSION,
        bytes: postcard::to_allocvec(own).expect("a type state is encoded into memory"),
    }
}

/// Changes the golem's state.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut IronGolemState)) {
    let mut own = state_of(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        mob.state = write(&own);
    }
}

/// `setPlayerCreated(true)`, which building one sets.
pub(crate) fn player_created(mob: &mut Mob) {
    update(mob, |own| own.player_created = true);
}

/// `Crackiness.GOLEM.byFraction(fraction)` (Crackiness.txt 28–52, 82–98): none at three
/// quarters of its health or more, low below, medium below a half and high below a
/// quarter, each strictly, in `float`.
pub(crate) fn crackiness(fraction: f32) -> u8 {
    if fraction < 0.25 {
        3
    } else if fraction < 0.5 {
        2
    } else if fraction < 0.75 {
        1
    } else {
        0
    }
}

/// `getCrackiness()` of `mob`: of its health over its most.
fn cracks_of(mob: &Mob, max_health: f64) -> u8 {
    crackiness(mob.body.vitals.health / max_health as f32)
}

/// Whether the looked-at thing is an `Enemy` (the class: a monster's family, or a mob of
/// the monsters' category).
pub(crate) fn is_enemy(looked: &Looked) -> bool {
    looked.mob && looked.kind.is_some_and(ladder::is_enemy)
}

/// `lambda$registerGoals$0`: an `Enemy` that is not a creeper.
fn enemy_but_creeper(looked: &Looked) -> bool {
    is_enemy(looked) && looked.kind != Some(entity_types::CREEPER as u16)
}

/// `IronGolem.offerFlower(offer)`: the counter set (400) or ended, and entity event 11
/// or 34 that tells clients.
fn offer_flower(ai: &mut Ai<'_, '_>, offer: bool) {
    let moment = crate::mobs::body::moment(ai.tick);
    update(&mut ai.tick.mob, |own| {
        own.offer_flower_end = if offer { moment + OFFER_TICKS } else { 0 };
    });
    ai.tick
        .out
        .events
        .push(Event::Entity(if offer { 11 } else { 34 }));
}

/// `MoveTowardsTargetGoal`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Approaching {
    pub(crate) target: Option<EntityId>,
    pub(crate) wanted: Vec3,
}

/// `MoveTowardsTargetGoal(mob, speed, within)`: towards a target within `within`, to a
/// place drawn on its way (`DefaultRandomPos.getPosTowards(mob, 16, 7, its feet,
/// π/2)`), while the path lasts and the target lives within reach.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MoveTowardsTargetGoal {
    pub(crate) speed: f64,
    pub(crate) within: f32,
}

impl MoveTowardsTargetGoal {
    /// `within * within`, a `float` widened.
    fn reach_sqr(&self) -> f64 {
        f64::from(self.within * self.within)
    }
}

impl Goal for MoveTowardsTargetGoal {
    type State = Approaching;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, state: &mut Approaching, ai: &mut Ai<'_, '_>) -> bool {
        let target = sensing::target(ai);
        state.target = target.as_ref().map(|target| target.key);
        let Some(target) = target else {
            return false;
        };
        if target.distance_sqr(ai.position()) > self.reach_sqr() {
            return false;
        }
        let Some(wanted) =
            random_pos::default_pos_towards(ai, 16, 7, target.position, 1.570_796_370_506_286_6)
        else {
            return false;
        };
        state.wanted = wanted;
        true
    }

    fn can_continue(&self, state: &mut Approaching, ai: &mut Ai<'_, '_>) -> bool {
        if navigation::is_done(ai) {
            return false;
        }
        let Some(target) = state.target.and_then(|key| sensing::find(ai, key)) else {
            return false;
        };
        target.alive && target.distance_sqr(ai.position()) < self.reach_sqr()
    }

    fn start(&self, state: &mut Approaching, ai: &mut Ai<'_, '_>) {
        navigation::move_to(ai, state.wanted, self.speed);
    }

    fn stop(&self, state: &mut Approaching, _: &mut Ai<'_, '_>) {
        state.target = None;
    }
}

/// Where one of the golem's strolls goes: `MoveBackToVillageGoal`'s, towards a section
/// nearer a village, or `GolemRandomStrollInVillageGoal`'s.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum GolemStroll {
    BackToVillage,
    InVillage,
}

/// `MoveBackToVillageGoal(mob, 0.6, false)` and `GolemRandomStrollInVillageGoal(mob,
/// 0.6)`: `RandomStrollGoal`s of intervals 10 and 240 that do not ask the time without
/// action (B 8.15). The first asks first whether it stands in a village, and draws
/// nothing there.
#[derive(Debug, Clone, Copy)]
pub(crate) struct GolemStrollGoal {
    pub(crate) speed: f64,
    pub(crate) kind: GolemStroll,
}

/// `Villager.wantsToSpawnGolem(gameTime)` of what a villager shows (its record's, also
/// in a copy): it slept within the last 24 000 ticks and has seen no golem lately.
pub(crate) fn wants_golem(shown: &Shown, now: i64) -> bool {
    shown.last_slept.is_some_and(|slept| now - slept < 24_000) && !shown.golem_detected
}

/// What the villager `entity` shows, where it is a villager.
fn shown_of(entity: &clustine_world::Entity) -> Option<Shown> {
    if i32::from(entity.kind) != entity_types::VILLAGER {
        return None;
    }
    let mob = Mob::decode(&entity.data).ok()?;
    Some(villager::state_of(&mob).1.shown)
}

impl GolemStrollGoal {
    fn interval(&self) -> i32 {
        match self.kind {
            GolemStroll::BackToVillage => 10,
            GolemStroll::InVillage => 240,
        }
    }

    /// `getPosition()`.
    fn position(&self, ai: &mut Ai<'_, '_>) -> Option<Vec3> {
        match self.kind {
            GolemStroll::BackToVillage => {
                let section = section_of(ai.block_position());
                let closest = find_section_closest_to_village(ai.tick.world, section, 2);
                if closest == section {
                    return None;
                }
                let towards = bottom_centre(section_centre(closest));
                random_pos::default_pos_towards(ai, 10, 7, towards, HALF_PI)
            }
            GolemStroll::InVillage => {
                // Of the level's random: anywhere three times in ten; else a villager
                // who wants it or a point of interest, in an order the second float
                // decides, then the other; anywhere where neither gives a place.
                if ai.tick.draws.next_float() < 0.3 {
                    return random_pos::land_pos(ai, 10, 7);
                }
                let found = if ai.tick.draws.next_float() < 0.7 {
                    Self::towards_villager(ai).or_else(|| Self::towards_poi(ai))
                } else {
                    Self::towards_poi(ai).or_else(|| Self::towards_villager(ai))
                };
                found.or_else(|| random_pos::land_pos(ai, 10, 7))
            }
        }
    }

    /// `getPositionTowardsVillagerWhoWantsGolem()`: of the villagers in its box grown by
    /// 32 that want a golem, one by the level's `nextInt`, and a place on land towards
    /// it.
    fn towards_villager(ai: &mut Ai<'_, '_>) -> Option<Vec3> {
        let bounds = ai_bounds(ai).inflate([32.0, 32.0, 32.0]);
        let now = ai.tick.world.game_time();
        let wanting: Vec<Vec3> = ai
            .tick
            .world
            .entities_in(&bounds)
            .iter()
            .filter(|seen| shown_of(seen.entity).is_some_and(|shown| wants_golem(&shown, now)))
            .map(|seen| seen.entity.position)
            .collect();
        if wanting.is_empty() {
            return None;
        }
        let chosen = wanting[ai.tick.draws.next_int(wanting.len() as u32) as usize];
        random_pos::land_pos_towards(ai, 10, 7, chosen)
    }

    /// `getPositionTowardsPoi()`: of the sections within two of its own that are in a
    /// village, one by the level's `nextInt`; of the occupied points of interest within
    /// 8 of that section's middle, one by the level's `nextInt`; and a place on land
    /// towards it.
    fn towards_poi(ai: &mut Ai<'_, '_>) -> Option<Vec3> {
        let world = ai.tick.world;
        let sections: Vec<[i32; 3]> = section_cube(section_of(ai.block_position()), 2)
            .into_iter()
            .filter(|section| world.sections_to_village(*section) == 0)
            .collect();
        if sections.is_empty() {
            return None;
        }
        let section = sections[ai.tick.draws.next_int(sections.len() as u32) as usize];
        let pois = ai.tick.world.pois_in_range(
            section_centre(section),
            8,
            &|_| true,
            Occupancy::IsOccupied,
        );
        if pois.is_empty() {
            return None;
        }
        let poi = pois[ai.tick.draws.next_int(pois.len() as u32) as usize].pos;
        random_pos::land_pos_towards(ai, 10, 7, bottom_centre(poi))
    }
}

impl Goal for GolemStrollGoal {
    type State = Strolling;

    fn flags(&self) -> u8 {
        flags::MOVE
    }

    fn can_use(&self, state: &mut Strolling, ai: &mut Ai<'_, '_>) -> bool {
        if self.kind == GolemStroll::BackToVillage && ai.tick.world.is_village(ai.block_position())
        {
            return false;
        }
        if !state.forced {
            let bound = reduced_tick_delay(self.interval()).max(1) as u32;
            if ai.tick.draws.next_int(bound) != 0 {
                return false;
            }
        }
        let Some(wanted) = self.position(ai) else {
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
        navigation::move_to(ai, state.wanted, self.speed);
    }

    fn stop(&self, _: &mut Strolling, ai: &mut Ai<'_, '_>) {
        navigation::stop(ai);
    }
}

/// `Villager`, the class the golem's defence looks for.
const VILLAGER: &[u16] = &[entity_types::VILLAGER as u16];

/// `DefendVillageTargetGoal`'s state: `TargetGoal`'s, and the player it found
/// (`potentialTarget`), never let go.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Defending {
    pub(crate) targeting: Targeting,
    pub(crate) potential: Option<EntityId>,
}

/// `DefendVillageTargetGoal(golem)` (n7 DefendVillageTargetGoal 9–131; `TargetGoal(mob,
/// false, true)`): of the villagers and the players in its box grown by 10, 8 and 10
/// that pass combat conditions of range 64 (a player-built golem attacks no player), the
/// last pair in which the villager's reputation of the player is -100 or less gives the
/// player it would target; it takes that player, unless creative or a spectator. No
/// draws.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DefendVillageTargetGoal;

impl DefendVillageTargetGoal {
    const RULES: TargetRules = TargetRules {
        must_see: false,
        must_reach: true,
    };
}

impl Goal for DefendVillageTargetGoal {
    type State = Defending;

    fn flags(&self) -> u8 {
        flags::TARGET
    }

    fn can_use(&self, state: &mut Defending, ai: &mut Ai<'_, '_>) -> bool {
        let bounds = ai_bounds(ai).inflate([10.0, 8.0, 10.0]);
        let conditions = Conditions::combat().range(64.0);
        let villagers = sensing::nearby(ai, Class::Kinds(VILLAGER), &conditions, bounds);
        // `IronGolem.canAttack`: one a player built attacks no player.
        let players = if state_of(&ai.tick.mob).player_created {
            Vec::new()
        } else {
            sensing::nearby(ai, Class::Player, &conditions, bounds)
        };
        let seen_players = ai.tick.world.players();
        let uuid_of = |key: EntityId| {
            seen_players
                .iter()
                .find(|player| player.key == key)
                .map(|player| uuid::Uuid::from_u128(player.uuid))
        };
        for villager in &villagers {
            let Some(shown) = ai
                .tick
                .world
                .entity(villager.key)
                .and_then(|seen| shown_of(seen.entity))
            else {
                continue;
            };
            for player in &players {
                let Some(uuid) = uuid_of(player.key) else {
                    continue;
                };
                // A reputation above -100 is not shown.
                let bad = shown
                    .bad_reputations
                    .iter()
                    .any(|&(whom, reputation)| whom == uuid && reputation <= -100);
                if bad {
                    state.potential = Some(player.key);
                }
            }
        }
        let Some(target) = state.potential.and_then(|key| sensing::find(ai, key)) else {
            return false;
        };
        !(target.is_player() && (target.spectator || target.creative))
    }

    fn can_continue(&self, state: &mut Defending, ai: &mut Ai<'_, '_>) -> bool {
        Self::RULES.can_continue(&mut state.targeting, ai)
    }

    fn start(&self, state: &mut Defending, ai: &mut Ai<'_, '_>) {
        let target = state.potential.and_then(|key| sensing::find(ai, key));
        sensing::set_target(ai, target.as_ref());
        TargetRules::start(&mut state.targeting);
    }

    fn stop(&self, state: &mut Defending, ai: &mut Ai<'_, '_>) {
        TargetRules::stop(&mut state.targeting, ai);
    }
}

/// `OfferFlowerGoal`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Offering {
    pub(crate) entity: Option<EntityId>,
    pub(crate) tick: i32,
}

/// `OfferFlowerGoal(golem)`: in daylight, about once in 8000 asks, it holds out a poppy
/// to the nearest `#candidate_for_iron_golem_gift` within 6 for 400 ticks, looking at
/// it; a mob of `#accepts_iron_golem_gift` with nothing on its antenna and still near
/// at the end is given it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OfferFlowerGoal;

impl OfferFlowerGoal {
    /// `getGolemBoundingBox()`: its box grown by 6, 2 and 6.
    fn golem_box(ai: &Ai<'_, '_>) -> crate::physics::aabb::Aabb {
        ai_bounds(ai).inflate([6.0, 2.0, 6.0])
    }
}

impl Goal for OfferFlowerGoal {
    type State = Offering;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    fn can_use(&self, state: &mut Offering, ai: &mut Ai<'_, '_>) -> bool {
        // `Level.isBrightOutside()`: the overworld has no fixed time, and its sky is
        // darkened by less than 4.
        if ai.tick.world.sky_darken() >= 4 {
            return false;
        }
        if ai.tick.draws.next_int(8000) != 0 {
            return false;
        }
        // `getNearestEntity(#candidate_for_iron_golem_gift, forNonCombat().range(6),
        // golem, its feet, its box grown)`.
        let conditions = Conditions::non_combat().range(6.0);
        let bounds = Self::golem_box(ai);
        let candidates = sensing::things_in(
            ai,
            bounds,
            Class::Tag("minecraft:candidate_for_iron_golem_gift"),
        );
        let feet = ai.position();
        let found = sensing::nearest(ai, candidates, &conditions, &|_| true, feet);
        state.entity = found.map(|found| found.key);
        state.entity.is_some()
    }

    fn can_continue(&self, state: &mut Offering, _: &mut Ai<'_, '_>) -> bool {
        state.tick > 0
    }

    fn start(&self, state: &mut Offering, ai: &mut Ai<'_, '_>) {
        state.tick = self.adjusted_tick_delay(OFFER_TICKS as i32);
        offer_flower(ai, true);
    }

    fn stop(&self, state: &mut Offering, ai: &mut Ai<'_, '_>) {
        offer_flower(ai, false);
        let entity = state.entity.take();
        if state.tick != 0 {
            return;
        }
        let Some(entity) = entity.and_then(|key| sensing::find(ai, key)) else {
            return;
        };
        let accepts = entity.mob
            && entity.kind.is_some_and(|kind| {
                entity_in_tag(i32::from(kind), "minecraft:accepts_iron_golem_gift")
            });
        if accepts && Self::golem_box(ai).intersects(entity.bounds) {
            // Its antenna's emptiness is asked by whoever holds it, as it is written.
            ai.tick
                .out
                .local
                .push(Local::GiveFlower { key: entity.key });
        }
    }

    fn tick(&self, state: &mut Offering, ai: &mut Ai<'_, '_>) {
        if let Some(entity) = state.entity.and_then(|key| sensing::find(ai, key)) {
            look_at_thing(ai, &entity, 30.0, 30.0);
        }
        state.tick -= 1;
    }
}

/// The iron golem.
pub(crate) struct IronGolem;

/// `canSpawnSprintParticle()` and `spawnSprintParticle()`, of `Entity.baseTick`: moving,
/// one tick in five it reads the block under its feet, and unless the block is not
/// drawn at all, places a particle at two draws more. The particle is the client's; the
/// draws are the server's too.
fn sprint_particle_draws(tick: &mut MobTick<'_>) {
    let [x, _, z] = tick.living.body.motion;
    if x * x + z * z <= 2.500_000_277_905_201e-7 || tick.draws.next_int(5) != 0 {
        return;
    }
    let below = tick.living.body.on_pos_legacy(tick.world);
    if !renders_invisible(tick.world.state(below)) {
        tick.draws.next_double();
        tick.draws.next_double();
    }
}

/// `getRenderShape() == INVISIBLE`. **Inferred**, the shapes not being extracted: the
/// airs, the liquids and the blocks the game draws by other means or not at all.
fn renders_invisible(state: BlockState) -> bool {
    use clustine_blocks::data::{is_block, is_class};
    state.is_air()
        || is_class(state, clustine_data::BlockClass::LiquidBlock)
        || [
            blocks::BUBBLE_COLUMN,
            blocks::STRUCTURE_VOID,
            blocks::BARRIER,
            blocks::LIGHT,
            blocks::MOVING_PISTON,
            blocks::END_PORTAL,
            blocks::END_GATEWAY,
        ]
        .into_iter()
        .any(|block| is_block(state, block))
}

impl MobType for IronGolem {
    fn register_goals(&self, setup: &mut Setup) {
        setup.add_goal(
            1,
            MeleeAttackGoal {
                speed: 1.0,
                following_even_if_not_seen: true,
            },
        );
        setup.add_goal(
            2,
            MoveTowardsTargetGoal {
                speed: 0.9,
                within: 32.0,
            },
        );
        setup.add_goal(
            2,
            GolemStrollGoal {
                speed: 0.6,
                kind: GolemStroll::BackToVillage,
            },
        );
        setup.add_goal(
            4,
            GolemStrollGoal {
                speed: 0.6,
                kind: GolemStroll::InVillage,
            },
        );
        setup.add_goal(5, OfferFlowerGoal);
        setup.add_goal(7, LookAtPlayerGoal::new(Class::Player, 6.0));
        setup.add_goal(8, RandomLookAroundGoal);
        setup.add_target(1, DefendVillageTargetGoal);
        // `HurtByTargetGoal()`: a creeper that hurt it is refused by its `canAttack`,
        // before anything is drawn, as an ignored type is.
        setup.add_target(
            2,
            HurtByTargetGoal {
                ignore: &[entity_types::CREEPER as u16],
            },
        );
        // `this::isAngryAt`: no player is ever one, every player being creative.
        setup.add_target(3, NearestAttackableTargetGoal::new(Class::Player, true));
        setup.add_target(
            3,
            NearestAttackableTargetGoal {
                class: Class::Mob,
                interval: 5,
                must_see: false,
                must_reach: false,
                selector: Some(enemy_but_creeper),
            },
        );
        // `ResetUniversalAngerTargetGoal(false)`: only with `universal_anger`, which is off.
        setup.add_target(4, Idle { flags: 0 });
    }

    /// `doHurtTarget`: its arms swing (entity event 4), it hits for half its attack and
    /// a draw of the whole (7.5 to 21.5), and where the hit takes, the target is flung
    /// up by 0.4 less its knockback resistance; its attack sound either way.
    fn do_hurt_target(&self, ai: &mut Ai<'_, '_>, target: &Looked) {
        let moment = crate::mobs::body::moment(ai.tick);
        update(&mut ai.tick.mob, |own| {
            own.attack_animation_end = moment + ATTACK_ANIMATION_TICKS;
        });
        ai.tick.out.events.push(Event::Entity(4));
        let damage = ai.attribute(attributes::ATTACK_DAMAGE) as f32;
        let whole = damage as i32;
        let amount = if whole > 0 {
            damage / 2.0 + ai.tick.draws.next_int(whole as u32) as f32
        } else {
            damage
        };
        ai.tick.out.local.push(Local::Hurt {
            key: target.key,
            kind: ai.tick.kind,
            from: ai.position(),
            amount,
            sound: None,
            knockback: None,
            fling: Some(FLING),
        });
        ai.tick.out.events.push(Event::Sound {
            sound: sounds::ENTITY_IRON_GOLEM_ATTACK,
            volume: 1.0,
            pitch: 1.0,
        });
    }

    /// `Entity.baseTick`'s sprint particles, the first of its tick's draws.
    fn before_tick(&self, tick: &mut MobTick<'_>) {
        sprint_particle_draws(tick);
    }

    /// `doPush(entity)`: a monster it bumps that is not a creeper becomes its target one
    /// time in 20, drawn only for such a one.
    fn before_push(&self, tick: &mut MobTick<'_>, other: Option<(EntityId, u16)>) {
        let Some((key, kind)) = other else {
            return;
        };
        if !ladder::is_enemy(kind) || i32::from(kind) == entity_types::CREEPER {
            return;
        }
        if tick.draws.next_int(20) == 0 {
            tick.mob.body.combat.target = Some(key);
        }
    }

    /// `IronGolem.aiStep` after `AbstractGolem`'s: its counters are moments, and its
    /// anger brought up to date, its timer drawn again every tick it has a target.
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        let mut own = state_of(&tick.mob);
        let before = own.clone();
        let now = tick.world.game_time();
        neutral::update_persistent_anger(
            &mut tick.mob,
            &mut own.anger,
            tick.world,
            now,
            true,
            tick.draws,
            polar_bear::anger_time,
        );
        if own != before {
            tick.mob.state = write(&own);
        }
    }

    /// `decreaseAirSupply` gives back the air it had: under water its air stays whole.
    fn breathes_under_water(&self, _: &Mob) -> bool {
        true
    }

    /// `IronGolem.hurtServer`'s first step: its cracks as the hurt begins.
    fn before_hurt_server(&self, struck: &mut Struck<'_>, _: &DamageSource, _: f32) {
        let cracks = cracks_of(struck.mob, struck.attribute(attributes::MAX_HEALTH));
        update(struck.mob, |own| own.cracks_before = Some(cracks));
    }

    /// `IronGolem.hurtServer`'s last: where the hurt took and its cracks changed, its
    /// damage sound.
    fn after_hurt(&self, struck: &mut Struck<'_>, _: &DamageSource, took: bool) {
        let before = state_of(struck.mob).cracks_before;
        update(struck.mob, |own| own.cracks_before = None);
        let now = cracks_of(struck.mob, struck.attribute(attributes::MAX_HEALTH));
        if took && before != Some(now) {
            struck.out.events.push(Event::Sound {
                sound: sounds::ENTITY_IRON_GOLEM_DAMAGE,
                volume: 1.0,
                pitch: 1.0,
            });
        }
    }

    /// `IronGolem.mobInteract`: an iron ingot heals 25, and where that healed anything,
    /// its repair sound at a pitch of two draws; the creative player keeps the ingot.
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        if !used
            .held
            .as_ref()
            .is_some_and(|stack| stack.count > 0 && stack.item == items::IRON_INGOT)
        {
            return Interaction::Pass;
        }
        let before = used.mob.body.vitals.health;
        // `heal(25)`: a living one only, up to its most.
        if before > 0.0 {
            let max = used.mob.body.attributes.value(
                used.kind,
                crate::mobs::body::attribute(attributes::MAX_HEALTH),
            ) as f32;
            used.mob.body.vitals.health = (before + INGOT_HEALS).clamp(0.0, max);
        }
        if used.mob.body.vitals.health == before {
            return Interaction::Pass;
        }
        let pitch = 1.0 + (used.draws.next_float() - used.draws.next_float()) * 0.2;
        used.did.push(Did::Sound {
            sound: sounds::ENTITY_IRON_GOLEM_REPAIR,
            source: ladder::sound_source(used.kind),
            at: used.position,
            volume: 1.0,
            pitch,
            but_the_player: false,
        });
        Interaction::Success
    }

    fn ambient_interval(&self) -> u32 {
        120
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_IRON_GOLEM_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_IRON_GOLEM_DEATH)
    }

    fn step_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_IRON_GOLEM_STEP)
    }

    /// `playSound(IRON_GOLEM_STEP, 1, 1)`.
    fn step_volume(&self) -> f32 {
        1.0
    }

    /// `readAdditionalSaveData`: `PlayerCreated`, false where it is not named.
    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        let created = template::boolean(tag, "PlayerCreated").unwrap_or(false);
        update(mob, |own| own.player_created = created);
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let flags = if state_of(mob).player_created {
            PLAYER_CREATED
        } else {
            0
        };
        vec![(FLAGS_INDEX, DataShown::Byte(flags), flags == 0)]
    }
}

/// Whether `own`'s arms still swing at `moment` (`getAttackAnimationTick() > 0`).
#[cfg(test)]
pub(crate) fn attack_animation(own: &IronGolemState, moment: u32) -> u32 {
    own.attack_animation_end.saturating_sub(moment)
}

/// What is left of `own`'s offer at `moment` (`getOfferFlowerTick()`).
#[cfg(test)]
pub(crate) fn offer_flower_tick(own: &IronGolemState, moment: u32) -> u32 {
    own.offer_flower_end.saturating_sub(moment)
}
