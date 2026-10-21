//! Wolves (`Wolf`, `WolfAvoidEntityGoal`, `BegGoal`, `WolfVariants`,
//! `WolfSoundVariants`; `docs/groundwork/n7-brief.md`, sections 5.0, 5.4 and 5.5): a wild
//! wolf hunts sheep, rabbits and foxes, is tamed with bones one time in three, and once
//! tamed sits and stands at its owner's click, follows them, teleports to them, defends
//! them and attacks what they attack, wears a collar dyed by its owner, heals on meat and
//! breeds with another tamed wolf. Its variant is its biome's; its voice is one of seven.
//! A wolf hit turns on whoever hit it, angry until its anger's time runs out
//! (`NeutralMob`, [`super::neutral`]); a wet wolf out of the water shakes itself dry.
//!
//! Named differences: wolf armour is left out (an item stack keeps no wear yet, which
//! the armour's absorbing and cracking and its repair need), so armour on a tamed wolf
//! is a click like any other; a wolf hit does not alert its pack (an effect on other
//! mobs not built yet); a sitting wolf's head turns as far as a standing one's; rain
//! does not wet it (the weather is always clear); a wolf is let be leashed by whether its
//! anger has an end, not by the game time.

use clustine_data::{PathType, attributes, entity_types, items, sounds};
use clustine_world::BlockPos;
use serde::{Deserialize, Serialize};

use super::animal::{self, Own, variants};
use super::neutral::{self, Anger};
use super::sheep::{dye_colour, mixed_colour};
use super::tamable::{
    self, FollowOwnerGoal, Idle, NonTameRandomTargetGoal, OwnerHurtByTargetGoal,
    OwnerHurtTargetGoal, Pet, SitWhenOrderedToGoal, TamablePanicGoal, Tame, find,
};
use super::{Breeding, Did, Dimensions, Interaction, Mate, MobType, Spawning, Used, cow};
use crate::mobs::Mob;
use crate::mobs::ai::goals::{
    AvoidEntityGoal, Avoiding, FloatGoal, HurtByTargetGoal, Items, LeapAtTargetGoal,
    LookAtPlayerGoal, MeleeAttackGoal, NearestAttackableTargetGoal, RandomLookAroundGoal,
    RandomStrollGoal, TargetRules, Targeting,
};
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::{self, Class, Conditions, Looked};
use crate::mobs::ai::{Ai, Setup, controls};
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::body::{Event, attribute};
use crate::mobs::tick::{Local, MobTick};
use crate::mobs::view::{MobWorld, SeenPlayer};
use crate::region::Random;
use crate::shown::{DataShown, Holder};

/// The registries of a wolf's variants.
const VARIANTS: &str = "minecraft:wolf_variant";
const SOUND_VARIANTS: &str = "minecraft:wolf_sound_variant";

/// `#wolf_food`.
const FOOD: Items = Items::Tag("minecraft:wolf_food");

/// A pup's size (`BABY_DIMENSIONS`).
const BABY: Dimensions = Dimensions {
    width: 0.3,
    height: 0.425,
    eye_height: 0.343_75,
};

/// The synced data's indices of a wolf's own values.
const INTERESTED_INDEX: u8 = 20;
const COLLAR_INDEX: u8 = 21;
const ANGER_INDEX: u8 = 22;
const VARIANT_INDEX: u8 = 23;
const SOUND_VARIANT_INDEX: u8 = 24;

/// A collar's colour before it is dyed: red.
const RED: u8 = 14;

/// `Wolf`'s health wild and tamed.
const WILD_HEALTH: f64 = 8.0;
const TAME_HEALTH: f64 = 40.0;

/// The types of `AbstractSkeleton`, which a wolf hunts (**Inferred**: the class is not
/// in the dump; the parched is 26.3's skeleton of the desert).
const SKELETONS: &[u16] = &[
    entity_types::SKELETON as u16,
    entity_types::STRAY as u16,
    entity_types::WITHER_SKELETON as u16,
    entity_types::BOGGED as u16,
    entity_types::PARCHED as u16,
];

/// A wild wolf's prey (`Wolf.PREY_SELECTOR`: sheep, rabbits and foxes).
const PREY: &[u16] = &[
    entity_types::SHEEP as u16,
    entity_types::RABBIT as u16,
    entity_types::FOX as u16,
];

/// What a wolf keeps beyond an animal: its tameness, its collar's colour, its variant and
/// voice by their ids, its interest in a held bone, its anger, and its shaking dry (not
/// saved by the game, kept here so that a wolf shakes on across a line).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct WolfState {
    pub(crate) tame: Tame,
    pub(crate) collar: u8,
    pub(crate) variant: u8,
    pub(crate) sound: u8,
    pub(crate) interested: bool,
    pub(crate) anger: Anger,
    pub(crate) wet: bool,
    pub(crate) shaking: bool,
    /// `shakeAnim` and `shakeAnimO`.
    pub(crate) shake: f32,
    pub(crate) shake_before: f32,
}

impl Default for WolfState {
    /// `defineSynchedData`'s: red, pale, the classic voice, calm.
    fn default() -> Self {
        Self {
            tame: Tame::default(),
            collar: RED,
            variant: variants::id(VARIANTS, "minecraft:pale"),
            sound: variants::id(SOUND_VARIANTS, "minecraft:classic"),
            interested: false,
            anger: Anger::default(),
            wet: false,
            shaking: false,
            shake: 0.0,
            shake_before: 0.0,
        }
    }
}

impl Own for WolfState {}

/// The wolf.
pub(crate) struct Wolf;

/// The wolf's state of `mob`.
pub(crate) fn state_of(mob: &Mob) -> (animal::Animal, WolfState) {
    animal::read::<WolfState>(&mob.state)
}

fn put(mob: &mut Mob, shared: &animal::Animal, own: &WolfState) {
    mob.state = animal::write(shared, own);
}

impl Pet for Wolf {
    fn tame(&self, mob: &Mob) -> Tame {
        state_of(mob).1.tame
    }

    fn set_tame(&self, mob: &mut Mob, tame: Tame) {
        let (shared, mut own) = state_of(mob);
        own.tame = tame;
        put(mob, &shared, &own);
    }

    /// `Wolf.wantsToAttack(target, owner)`: never a creeper, a ghast or an armour stand;
    /// a wolf only wild or another's; a player only where the owner may harm them (no
    /// player may harm another: every player is creative); never a tamed horse; another
    /// tamed animal never.
    fn wants_to_attack(&self, world: &dyn MobWorld, target: &Looked, owner: &SeenPlayer) -> bool {
        let Some(kind) = target.kind else {
            return false;
        };
        let kind = i32::from(kind);
        if [
            entity_types::CREEPER,
            entity_types::GHAST,
            entity_types::ARMOR_STAND,
        ]
        .contains(&kind)
        {
            return false;
        }
        let record = world
            .entity(target.key)
            .and_then(|seen| Mob::decode(&seen.entity.data).ok());
        if kind == entity_types::WOLF {
            let tame = record.map(|mob| state_of(&mob).1.tame).unwrap_or_default();
            return !tame.is_tame() || tame.owner != Some(owner.uuid);
        }
        // A horse's or another pet's tameness, where its type is built: a cat's, a
        // parrot's.
        let tamed = record.is_some_and(|mob| {
            super::of(kind as u16)
                .pet()
                .is_some_and(|pet| pet.tame(&mob).is_tame())
        });
        !tamed
    }
}

/// `pickRandomSoundVariant(random)`: one of the seven voices, by id.
fn random_voice(draws: &mut Random) -> u8 {
    draws.next_int(variants::count(SOUND_VARIANTS)) as u8
}

/// `VariantUtils.selectVariantToSpawn` of `wolf_variant` (the brief's 5.5): the
/// biome's variant at priority 1, else pale, one drawn even when one is left.
pub(crate) fn select_variant(biome: Option<&str>, level: &mut Random) -> u8 {
    let entries = clustine_data::synced_registry(VARIANTS).map_or(&[][..], |known| known.entries);
    let condition = |name: &str| -> Option<(i32, Option<&'static str>)> {
        let biome = match name {
            "minecraft:ashen" => "minecraft:snowy_taiga",
            "minecraft:black" => "minecraft:old_growth_pine_taiga",
            "minecraft:chestnut" => "minecraft:old_growth_spruce_taiga",
            "minecraft:rusty" => "#minecraft:is_jungle",
            "minecraft:snowy" => "minecraft:grove",
            "minecraft:spotted" => "#minecraft:is_savanna",
            "minecraft:striped" => "#minecraft:is_badlands",
            "minecraft:woods" => "minecraft:forest",
            "minecraft:pale" => return Some((0, None)),
            _ => return None,
        };
        Some((1, Some(biome)))
    };
    let mut pairs: Vec<(&str, i32, Option<&str>)> = entries
        .iter()
        .filter_map(|name| condition(name).map(|(priority, test)| (*name, priority, test)))
        .collect();
    pairs.sort_by_key(|(_, priority, _)| std::cmp::Reverse(*priority));
    let mut best = i32::MIN;
    let mut kept = Vec::new();
    for (name, priority, test) in pairs {
        if priority < best {
            continue;
        }
        let holds = test.is_none_or(|test| match test.strip_prefix('#') {
            Some(tag) => animal::biome_in_tag(biome, tag),
            None => biome == Some(test),
        });
        if holds {
            best = priority;
            kept.push(name);
        }
    }
    let chosen = if kept.is_empty() {
        "minecraft:pale"
    } else {
        kept[level.next_int(kept.len() as u32) as usize]
    };
    variants::id(VARIANTS, chosen)
}

/// The sounds of a wolf's voice, as its age has them.
#[derive(Debug, Clone, Copy)]
enum Voice {
    Ambient,
    Death,
    Growl,
    Hurt,
    Pant,
    Whine,
}

/// The sound of `voice` in the wolf's sound variant, a pup's own where it is one.
fn voice(mob: &Mob, voice: Voice) -> Option<i32> {
    let (shared, own) = state_of(mob);
    let what = match voice {
        Voice::Ambient => "ambient",
        Voice::Death => "death",
        Voice::Growl => "growl",
        Voice::Hurt => "hurt",
        Voice::Pant => "pant",
        Voice::Whine => "whine",
    };
    let name = if shared.is_baby() {
        format!("minecraft:entity.baby_wolf.{what}")
    } else {
        let variant = clustine_data::synced_registry(SOUND_VARIANTS)
            .and_then(|known| known.entries.get(usize::from(own.sound)))
            .and_then(|name| name.strip_prefix("minecraft:"))
            .unwrap_or("classic");
        if variant == "classic" {
            format!("minecraft:entity.wolf.{what}")
        } else {
            format!("minecraft:entity.wolf_{variant}.{what}")
        }
    };
    tamable::sound(&name)
}

/// `applyTamingSideEffects()`: a tamed wolf's health is 40 and full, a wild one's most is
/// 8 again.
pub(crate) fn taming_side_effects(mob: &mut Mob, tamed: bool) {
    let kind = entity_types::WOLF as u16;
    let base = if tamed { TAME_HEALTH } else { WILD_HEALTH };
    if let Some(instance) = mob
        .body
        .attributes
        .instance(kind, attribute(attributes::MAX_HEALTH))
    {
        instance.base = base;
    }
    if tamed {
        mob.body.vitals.health = TAME_HEALTH as f32;
    }
}

/// `tryToTame(player)`: one time in three, tamed by the player, its path and target
/// dropped and ordered to sit, with hearts; else smoke.
fn try_to_tame(used: &mut Used<'_>) {
    if used.draws.next_int(3) != 0 {
        used.did.push(Did::Event(6));
        return;
    }
    let (shared, mut own) = state_of(used.mob);
    tamable::tame(&mut own.tame, used.player_uuid);
    own.tame.ordered_to_sit = true;
    put(used.mob, &shared, &own);
    taming_side_effects(used.mob, true);
    used.mob.body.path.followed = None;
    used.mob.body.combat.target = None;
    used.did.push(Did::Event(7));
}

/// `Wolf.mobInteract` (the brief's 5.4), armour aside.
fn mob_interact(used: &mut Used<'_>) -> Interaction {
    let (shared, mut own) = state_of(used.mob);
    let held = used.held.filter(|stack| stack.count > 0);
    if own.tame.is_tame() {
        if FOOD.test(held) && tamable::hurt(used.mob, used.kind) {
            tamable::feed(used, 2.0, 2.0, None);
            return Interaction::Success;
        }
        let collar_dye = held.is_some_and(|stack| {
            clustine_blocks::data::item_in_tag(stack.item, "minecraft:wolf_collar_dyes")
        });
        if collar_dye && own.tame.is_owned_by(used.player_uuid) {
            let dye = held.and_then(|stack| dye_colour(stack.item));
            if let Some(dye) = dye.filter(|dye| *dye != own.collar) {
                own.collar = dye;
                put(used.mob, &shared, &own);
                return Interaction::Success;
            }
            return animal::mob_interact(used, FOOD, None);
        }
        let result = animal::mob_interact(used, FOOD, None);
        if result == Interaction::Pass && own.tame.is_owned_by(used.player_uuid) {
            let (shared, mut own) = state_of(used.mob);
            own.tame.ordered_to_sit = !own.tame.ordered_to_sit;
            put(used.mob, &shared, &own);
            used.mob.body.inputs.jumping = false;
            used.mob.body.path.followed = None;
            used.mob.body.combat.target = None;
            return Interaction::Success;
        }
        return result;
    }
    if animal::is_item(held, items::BONE) && !own.anger.is_angry(used.game_time) {
        try_to_tame(used);
        return Interaction::SuccessServer;
    }
    animal::mob_interact(used, FOOD, None)
}

/// `startPersistentAngerTimer()`'s time: twenty to thirty-nine seconds.
fn anger_time(draws: &mut Random) -> i64 {
    i64::from(draws.next_int(381)) + 400
}

/// `Wolf.aiStep` after `Animal`'s: a wet wolf on the ground, not walking a path, starts
/// to shake, and its anger is kept in step with its target.
fn ai_step(tick: &mut MobTick<'_>) {
    let (shared, mut own) = state_of(&tick.mob);
    let walking = tick
        .mob
        .body
        .path
        .followed
        .as_ref()
        .is_some_and(|path| (tick.mob.body.progress.next as usize) < path.nodes.len());
    if own.wet && !own.shaking && !walking && tick.living.body.on_ground {
        own.shaking = true;
        own.shake = 0.0;
        own.shake_before = 0.0;
        tick.out.events.push(Event::Entity(8));
    }
    let now = tick.world.game_time();
    neutral::update_persistent_anger(
        &mut tick.mob,
        &mut own.anger,
        tick.world,
        now,
        true,
        tick.draws,
        anger_time,
    );
    put(&mut tick.mob, &shared, &own);
}

/// `Wolf.tick` after the living body's: in water it is wet, and a shake stops; out of
/// it a shake goes on, sounds at its start, and draws for its splashes, which the server
/// does not send.
fn shake(tick: &mut MobTick<'_>) {
    if tick.mob.body.vitals.dying() {
        return;
    }
    let (shared, mut own) = state_of(&tick.mob);
    if tick.living.fluids.in_water() {
        own.wet = true;
        if own.shaking {
            tick.out.events.push(Event::Entity(56));
            own.shaking = false;
            own.shake = 0.0;
            own.shake_before = 0.0;
        }
    } else if (own.wet || own.shaking) && own.shaking {
        if own.shake == 0.0 {
            let pitch = (tick.draws.next_float() - tick.draws.next_float()) * 0.2 + 1.0;
            tick.out.events.push(Event::Sound {
                sound: sounds::ENTITY_WOLF_SHAKE,
                volume: 0.4,
                pitch,
            });
            let pos = crate::physics::containing(tick.living.body.position);
            tick.out.local.push(Local::GameEvent {
                name: "minecraft:entity_action",
                pos,
            });
        }
        own.shake_before = own.shake;
        own.shake += 0.05;
        if own.shake_before >= 2.0 {
            own.wet = false;
            own.shaking = false;
            own.shake_before = 0.0;
            own.shake = 0.0;
        }
        if own.shake > 0.4 {
            let angle = f64::from((own.shake - 0.4) * std::f32::consts::PI);
            let splashes = (clustine_noise::trig::sin(angle) * 7.0) as i32;
            for _ in 0..splashes.max(0) {
                tick.draws.next_float();
                tick.draws.next_float();
            }
        }
    }
    put(&mut tick.mob, &shared, &own);
}

/// `WolfAvoidEntityGoal(wolf, Llama, 24, 1.5, 1.5)`: a wild wolf runs from a llama as
/// strong as a draw of five (`Llama.getStrength()`, its record's).
#[derive(Clone, Copy)]
struct WolfAvoidEntityGoal;

const AVOID_LLAMAS: AvoidEntityGoal = AvoidEntityGoal {
    class: Class::Kinds(&[
        entity_types::LLAMA as u16,
        entity_types::TRADER_LLAMA as u16,
    ]),
    max_distance: 24.0,
    walk_speed: 1.5,
    sprint_speed: 1.5,
};

impl Goal for WolfAvoidEntityGoal {
    type State = Avoiding;

    fn flags(&self) -> u8 {
        AVOID_LLAMAS.flags()
    }

    fn can_use(&self, state: &mut Avoiding, ai: &mut Ai<'_, '_>) -> bool {
        if !AVOID_LLAMAS.can_use(state, ai) {
            return false;
        }
        if Wolf.tame(&ai.tick.mob).is_tame() {
            return false;
        }
        let strength = state
            .to_avoid
            .and_then(|key| ai.tick.world.entity(key))
            .and_then(|seen| Mob::decode(&seen.entity.data).ok())
            .map_or(0, |llama| super::equine::horse_of(&llama).strength);
        strength >= ai.tick.draws.next_int(5) as i32
    }

    fn can_continue(&self, state: &mut Avoiding, ai: &mut Ai<'_, '_>) -> bool {
        AVOID_LLAMAS.can_continue(state, ai)
    }

    fn start(&self, state: &mut Avoiding, ai: &mut Ai<'_, '_>) {
        sensing::set_target(ai, None);
        AVOID_LLAMAS.start(state, ai);
    }

    fn stop(&self, state: &mut Avoiding, ai: &mut Ai<'_, '_>) {
        AVOID_LLAMAS.stop(state, ai);
    }

    fn tick(&self, state: &mut Avoiding, ai: &mut Ai<'_, '_>) {
        sensing::set_target(ai, None);
        AVOID_LLAMAS.tick(state, ai);
    }
}

/// `BegGoal`'s state: the player it looks at and for how long yet.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Begging {
    player: Option<clustine_world::EntityId>,
    look_time: i32,
}

/// `BegGoal(wolf, 8)`: the nearest player within 8 holding a bone or its food is begged
/// of, its head tilted (interested).
#[derive(Clone, Copy)]
struct BegGoal;

/// `playerHoldingInteresting(player)`: a bone or the wolf's food in either hand.
fn holds_interesting(player: &Looked) -> bool {
    [player.main_hand, player.off_hand]
        .into_iter()
        .any(|stack| animal::is_item(stack, items::BONE) || FOOD.test(stack))
}

impl Goal for BegGoal {
    type State = Begging;

    fn flags(&self) -> u8 {
        flags::LOOK
    }

    fn can_use(&self, state: &mut Begging, ai: &mut Ai<'_, '_>) -> bool {
        let conditions = Conditions::non_combat().range(8.0);
        let from = ai.position();
        let found = sensing::nearest_player(ai, &conditions, &|_| true, from);
        state.player = found.as_ref().map(|found| found.key);
        found.is_some_and(|player| holds_interesting(&player))
    }

    fn can_continue(&self, state: &mut Begging, ai: &mut Ai<'_, '_>) -> bool {
        let Some(player) = state.player.and_then(|key| find(ai.tick.world, key)) else {
            return false;
        };
        player.alive
            && player.distance_sqr(ai.position()) <= 64.0
            && state.look_time > 0
            && holds_interesting(&player)
    }

    fn start(&self, state: &mut Begging, ai: &mut Ai<'_, '_>) {
        set_interested(&mut ai.tick.mob, true);
        let drawn = ai.tick.draws.next_int(40) as i32;
        state.look_time = self.adjusted_tick_delay(40 + drawn);
    }

    fn stop(&self, state: &mut Begging, ai: &mut Ai<'_, '_>) {
        set_interested(&mut ai.tick.mob, false);
        state.player = None;
    }

    fn tick(&self, state: &mut Begging, ai: &mut Ai<'_, '_>) {
        if let Some(player) = state.player.and_then(|key| find(ai.tick.world, key)) {
            let max_pitch = ai.head().max_x_rot as f32;
            controls::set_look_at(
                ai,
                [player.position[0], player.eye_y, player.position[2]],
                10.0,
                max_pitch,
            );
        }
        state.look_time -= 1;
    }
}

fn set_interested(mob: &mut Mob, interested: bool) {
    let (shared, mut own) = state_of(mob);
    if own.interested != interested {
        own.interested = interested;
        put(mob, &shared, &own);
    }
}

/// `NearestAttackableTargetGoal(wolf, Player, 10, true, false, wolf::isAngryAt)`: the
/// nearest player it is angry at, kept while it sees them.
#[derive(Clone, Copy)]
struct AngryAtPlayerGoal;

const SEEN: TargetRules = TargetRules {
    must_see: true,
    must_reach: false,
};

impl Goal for AngryAtPlayerGoal {
    type State = Targeting;

    fn flags(&self) -> u8 {
        flags::TARGET
    }

    fn can_use(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        let interval = crate::mobs::ai::maths::reduced_tick_delay(10);
        if ai.tick.draws.next_int(interval as u32) != 0 {
            return false;
        }
        let anger = state_of(&ai.tick.mob).1.anger;
        let found = tamable::nearest_target(ai, Class::Player, &|looked| anger.is_angry_at(looked));
        state.found = found.map(|found| found.key);
        state.found.is_some()
    }

    fn can_continue(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) -> bool {
        SEEN.can_continue(state, ai)
    }

    fn start(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        let found = state.found.and_then(|key| find(ai.tick.world, key));
        sensing::set_target(ai, found.as_ref());
        TargetRules::start(state);
    }

    fn stop(&self, state: &mut Targeting, ai: &mut Ai<'_, '_>) {
        TargetRules::stop(state, ai);
    }
}

/// A wild wolf's prey: sheep, rabbits and foxes (the class is `Animal`'s, the selector
/// the three).
fn prey(_: &dyn MobWorld, looked: &Looked) -> bool {
    looked.kind.is_some_and(|kind| PREY.contains(&kind))
}

impl MobType for Wolf {
    fn register_goals(&self, setup: &mut Setup) {
        cow::animal_maluses(setup);
        setup.set_malus(PathType::PowderSnow, -1.0);
        setup.set_malus(PathType::OnTopOfPowderSnow, -1.0);
        setup.add_goal(1, FloatGoal);
        setup.add_goal(
            1,
            TamablePanicGoal {
                pet: &Wolf,
                speed: 1.5,
                environmental: true,
            },
        );
        setup.add_goal(2, SitWhenOrderedToGoal { pet: &Wolf });
        setup.add_goal(3, WolfAvoidEntityGoal);
        setup.add_goal(4, LeapAtTargetGoal { yd: 0.4 });
        setup.add_goal(
            5,
            MeleeAttackGoal {
                speed: 1.0,
                following_even_if_not_seen: true,
            },
        );
        setup.add_goal(
            6,
            FollowOwnerGoal {
                pet: &Wolf,
                speed: 1.0,
                start: 10.0,
                stop: 2.0,
            },
        );
        setup.add_goal(7, animal::BreedGoal { speed: 1.0 });
        setup.add_goal(8, RandomStrollGoal::avoiding_water(1.0));
        setup.add_goal(9, BegGoal);
        setup.add_goal(10, LookAtPlayerGoal::new(Class::Player, 8.0));
        setup.add_goal(10, RandomLookAroundGoal);
        setup.add_target(1, OwnerHurtByTargetGoal { pet: &Wolf });
        setup.add_target(2, OwnerHurtTargetGoal { pet: &Wolf });
        setup.add_target(3, HurtByTargetGoal { ignore: &[] });
        setup.add_target(4, AngryAtPlayerGoal);
        setup.add_target(
            5,
            NonTameRandomTargetGoal {
                pet: &Wolf,
                class: Class::Kinds(PREY),
                selector: Some(prey),
            },
        );
        setup.add_target(
            6,
            NonTameRandomTargetGoal {
                pet: &Wolf,
                class: Class::Kinds(&[entity_types::TURTLE as u16]),
                selector: Some(tamable::baby_turtle_on_land),
            },
        );
        setup.add_target(
            7,
            NearestAttackableTargetGoal::new(Class::Kinds(SKELETONS), false),
        );
        // `ResetUniversalAngerTargetGoal(true)`: only with `universal_anger`, which is
        // off.
        setup.add_target(8, Idle { flags: 0 });
    }

    fn walk_target_value(&self, ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
        animal::walk_target_value(ai, pos, clustine_data::blocks::GRASS_BLOCK)
    }

    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::custom_server_ai_step(tick);
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step(tick);
        ai_step(tick);
        shake(tick);
    }

    fn before_hurt(&self, mob: &mut Mob) {
        tamable::update(&Wolf, mob, |tame| tame.ordered_to_sit = false);
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        mob_interact(used)
    }

    /// `Wolf.finalizeSpawn`: its biome's variant, its voice, then `AgeableMob`'s (a
    /// pack's data, which wants no pups) before `Mob`'s.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let (shared, mut own) = state_of(spawn.mob);
        own.variant = select_variant(spawn.biome, spawn.level);
        own.sound = random_voice(spawn.level);
        put(spawn.mob, &shared, &own);
    }

    /// `Wolf.getBreedOffspring`: a parent's variant by this one's coin, and where this
    /// one is tamed, its owner, tamed, and a collar of the parents' mixed; then a voice
    /// of its own.
    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        let (_, this) = state_of(breeding.this);
        let (_, partner) = state_of(breeding.partner);
        let mut own = WolfState {
            variant: if breeding.draws.next_bool() {
                this.variant
            } else {
                partner.variant
            },
            ..WolfState::default()
        };
        let mut child = Mob::fresh();
        let tamed = this.tame.is_tame();
        if tamed {
            own.tame.owner = this.tame.owner;
            own.tame.set_tame_flag(true);
            own.collar = mixed_colour(this.collar, partner.collar, breeding.draws);
        }
        own.sound = random_voice(breeding.draws);
        child.state = animal::write(&animal::Animal::default(), &own);
        if tamed {
            taming_side_effects(&mut child, true);
        }
        Some(child)
    }

    /// `Wolf.canMate(partner)`: of one type, this one tamed, the other a tamed wolf not
    /// sitting (their love is the breeding's).
    fn can_mate(&self, this: Mate<'_>, partner: Mate<'_>) -> bool {
        let (_, mine) = state_of(this.mob);
        let (_, other) = state_of(partner.mob);
        this.entity.key != partner.entity.key
            && this.entity.kind == partner.entity.kind
            && mine.tame.is_tame()
            && other.tame.is_tame()
            && !other.tame.in_sitting_pose()
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        animal::dimensions(mob, BABY)
    }

    fn ambient_sound(&self, mob: &Mob) -> Option<i32> {
        voice(mob, Voice::Ambient)
    }

    /// `Wolf.getAmbientSound()`: a growl while angry; else one time in three a whine from
    /// a tamed wolf under 20 health or a pant; else its ambient sound.
    fn ambient_sound_drawn(
        &self,
        mob: &Mob,
        draws: &mut Random,
        now: i64,
        _: u32,
        _: Option<(bool, bool)>,
    ) -> Option<i32> {
        let (_, own) = state_of(mob);
        if own.anger.is_angry(now) {
            return voice(mob, Voice::Growl);
        }
        if draws.next_int(3) == 0 {
            if own.tame.is_tame() && mob.body.vitals.health < 20.0 {
                return voice(mob, Voice::Whine);
            }
            return voice(mob, Voice::Pant);
        }
        voice(mob, Voice::Ambient)
    }

    fn hurt_sound(&self, mob: &Mob, _: &DamageSource) -> Option<i32> {
        voice(mob, Voice::Hurt)
    }

    fn death_sound(&self, mob: &Mob) -> Option<i32> {
        voice(mob, Voice::Death)
    }

    fn sound_volume(&self) -> f32 {
        0.4
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn can_be_leashed(&self, _: u16, mob: &Mob) -> bool {
        state_of(mob).1.anger.end <= 0
    }

    fn pet(&self) -> Option<&'static dyn Pet> {
        Some(&Wolf)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let (_, own) = state_of(mob);
        let fresh = WolfState::default();
        let mut values = animal::shown(mob);
        values.extend(own.tame.shown());
        values.extend([
            (
                INTERESTED_INDEX,
                DataShown::Boolean(own.interested),
                !own.interested,
            ),
            (
                COLLAR_INDEX,
                DataShown::Int(i32::from(own.collar)),
                own.collar == RED,
            ),
            (
                ANGER_INDEX,
                DataShown::Long(own.anger.end),
                own.anger.end == -1,
            ),
            (
                VARIANT_INDEX,
                DataShown::Holder(Holder::WolfVariant, i32::from(own.variant)),
                own.variant == fresh.variant,
            ),
            (
                SOUND_VARIANT_INDEX,
                DataShown::Holder(Holder::WolfSoundVariant, i32::from(own.sound)),
                own.sound == fresh.sound,
            ),
        ]);
        values
    }
}
