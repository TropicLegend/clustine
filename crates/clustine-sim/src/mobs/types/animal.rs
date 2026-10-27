//! What every farm animal is beyond its body: `AgeableMob`'s age, its forced age and the
//! age lock, and `Animal`'s love, feeding and breeding (`docs/groundwork/n7-brief.md`,
//! sections 4.2 to 4.4), with the goals of breeding and following a parent (2.9).
//!
//! An animal's type state begins with [`Animal`], each type's own state after it, so
//! that what every animal shares is read from any animal's record without knowing its
//! type (a partner's love, a parent's age). The game's counters that count every tick
//! are stamps against `lived` (`docs/groundwork/n7-plan.md`, section 11.2): an age is
//! the value it had at a moment and goes on from there by one a tick (a baby's up to
//! nothing, unless locked; an adult's breeding cooldown down to nothing), love lasts
//! until a moment, and the timers of the particles likewise; so an animal at rest
//! commits nothing for them, and a baby's record changes only when it grows up, which
//! clients see. A moment is a `lived` as the record has it between ticks: a value read
//! in a tick before `aiStep`'s growth is the value at the tick's `lived`, and after the
//! growth at the next.
//!
//! Breeding across a line follows the protocol of `region/breeding.rs` (the plan's 9.4
//! and 10.7): only the higher key asks a partner that only a copy shows; an asker is
//! marked asked until it is answered or 40 ticks pass, and while so counts as not in
//! love for others, chooses no partner and refuses every request. Within one region
//! the game's order holds: the first to reach its count breeds, as the game's `this`.
//!
//! Named differences: the particles of love, of feeding and of the age lock are drawn
//! and not sent (the edge sends none of their kinds); no player is the cause of a love
//! (Clustine keeps no statistics, which is all the game uses it for); a partner's
//! panic across a line is not read (another mob's mind is not); a baby's grown-up size
//! comes at its next tick.

use clustine_blocks::data::{entity_in_tag, is_block};
use clustine_data::{blocks, items, sounds};
use clustine_world::{BlockPos, Entity, EntityData, EntityId, ItemStack, TypeState};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::{AgeableGroup, Did, Dimensions, Interaction, SpawnGroup, Spawning, Used};
use crate::mobs::Mob;
use crate::mobs::ai::goals::Items;
use crate::mobs::ai::selector::{Goal, flags};
use crate::mobs::ai::sensing::{Conditions, Looked};
use crate::mobs::ai::{self, Ai, controls, navigation};
use crate::mobs::tick::{Local, MobTick};
use crate::mobs::view::Seen;
use crate::physics;
use crate::physics::aabb::Aabb;
use crate::region::Random;

/// `AgeableMob.BABY_START_AGE`: a baby is twenty minutes from growing up.
pub(crate) const BABY_START_AGE: i32 = -24_000;

/// How long love lasts (`setInLove`'s 600).
const LOVE_TICKS: u32 = 600;

/// How long an asker waits for its answer before it may ask again (the plan's 9.4).
const ASKED_TICKS: u32 = 40;

/// `AGE_LOCK_COOLDOWN_TICKS` and `FORCED_AGE_PARTICLE_TICKS`.
const PARTICLE_TICKS: u32 = 40;

/// The cooldown both parents get from a breeding (`setAge(6000)`).
pub(crate) const BRED_AGE: i32 = 6000;

/// The version of every animal's type state.
const VERSION: u8 = 1;

/// The synced data's indices of `AgeableMob` (the brief's finding 4.0.1).
const BABY_INDEX: u8 = 16;
const AGE_LOCKED_INDEX: u8 = 17;

/// What `AgeableMob` and `Animal` keep: the age and its forced part, the timers of the
/// particles as the moments they end, the age lock, love, and the breeding asked for.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Animal {
    /// `age` at the moment `age_at`.
    pub(crate) age: i32,
    pub(crate) age_at: u32,
    /// `forcedAge`: what feeding took off a baby's time, owed back as a cooldown.
    pub(crate) forced_age: i32,
    /// The moment `forcedAgeTimer` comes to nothing.
    pub(crate) forced_until: u32,
    /// The moment `ageLockParticleTimer` comes to nothing.
    pub(crate) lock_until: u32,
    /// `AGE_LOCKED`: the golden dandelion's.
    pub(crate) age_locked: bool,
    /// The moment `inLove` comes to nothing.
    pub(crate) love_until: u32,
    /// The request to breed it made last and has not been answered.
    pub(crate) asked: Option<Asked>,
    /// The number of its last request, which an answer names.
    pub(crate) requests: u32,
}

/// A request to breed an animal made of `partner` at the moment `at`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct Asked {
    pub(crate) partner: EntityId,
    pub(crate) number: u32,
    pub(crate) at: u32,
}

/// The ticks from `from` to `now`, none where `now` is earlier (a crash's restore lowers
/// `lived`).
fn since(from: u32, now: u32) -> i64 {
    (i64::from(now) - i64::from(from)).max(0)
}

/// What is left at `now` of a timer that ends at `until`.
fn left(until: u32, now: u32) -> i32 {
    (i64::from(until) - i64::from(now)).clamp(0, i64::from(i32::MAX)) as i32
}

impl Animal {
    /// `getAge()` at the moment `now`.
    pub(crate) fn age(&self, now: u32) -> i32 {
        let gone = since(self.age_at, now);
        let age = i64::from(self.age);
        let at = if self.age < 0 {
            if self.age_locked {
                age
            } else {
                (age + gone).min(0)
            }
        } else {
            (age - gone).max(0)
        };
        at as i32
    }

    /// `isBaby()`: the age is below nothing. The record's age changes its sign only
    /// where the game's does, so the sign is read as it is kept.
    pub(crate) fn is_baby(&self) -> bool {
        self.age < 0
    }

    /// `canAgeUp()`: a baby not locked.
    pub(crate) fn can_age_up(&self) -> bool {
        self.is_baby() && !self.age_locked
    }

    /// `setAge(age)` at the moment `now`.
    pub(crate) fn set_age(&mut self, age: i32, now: u32) {
        self.age = age;
        self.age_at = now;
    }

    /// `ageUp(seconds, forced)` at the moment `now`: grown by as many seconds, never
    /// past nothing, what feeding gained owed back (`forced`), and a baby grown up so
    /// given the cooldown it owes.
    pub(crate) fn age_up(&mut self, seconds: i32, forced: bool, now: u32) {
        let old = self.age(now);
        let mut age = old.wrapping_add(seconds.wrapping_mul(20));
        if age > 0 {
            age = 0;
        }
        let gained = age.wrapping_sub(old);
        self.set_age(age, now);
        if forced {
            self.forced_age = self.forced_age.wrapping_add(gained);
            if left(self.forced_until, now) == 0 {
                self.forced_until = now + PARTICLE_TICKS;
            }
        }
        if self.age(now) == 0 {
            self.set_age(self.forced_age, now);
        }
    }

    /// `inLove` at the moment `now`.
    pub(crate) fn love(&self, now: u32) -> i32 {
        left(self.love_until, now)
    }

    /// `isInLove()`.
    pub(crate) fn in_love(&self, now: u32) -> bool {
        self.love(now) > 0
    }

    /// The request it waits on the answer of at `now`, if any (40 ticks at most).
    pub(crate) fn waiting(&self, now: u32) -> Option<Asked> {
        self.asked
            .filter(|asked| since(asked.at, now) < i64::from(ASKED_TICKS))
    }

    /// In love as others see it: an animal that waits on an answer counts as not.
    pub(crate) fn in_love_for_others(&self, now: u32) -> bool {
        self.in_love(now) && self.waiting(now).is_none()
    }

    /// `setInLove(player)`.
    pub(crate) fn set_in_love(&mut self, now: u32) {
        self.love_until = now + LOVE_TICKS;
    }

    /// `resetLove()`.
    pub(crate) fn reset_love(&mut self) {
        self.love_until = 0;
    }

    /// Its part of a breeding (`finalizeSpawnChildFromBreeding`): its cooldown, and no
    /// more love.
    pub(crate) fn bred(&mut self, now: u32) {
        self.set_age(BRED_AGE, now);
        self.reset_love();
    }
}

/// `getSpeedUpSecondsWhenFeeding(ticks)`: a tenth of what is left, in whole seconds.
pub(crate) fn speed_up_seconds(ticks: i32) -> i32 {
    ((ticks / 20) as f32 * 0.1_f32) as i32
}

/// The animal's own state after [`Animal`] in its type's record.
pub(crate) trait Own: Serialize + DeserializeOwned + Default + PartialEq {}

/// What every animal shares and its type's own state, from its type state; a record of
/// nothing, or one this build cannot read, as a fresh animal's.
pub(crate) fn read<O: Own>(state: &TypeState) -> (Animal, O) {
    if state.bytes.is_empty() || state.version != VERSION {
        return Default::default();
    }
    let Ok((animal, rest)) = postcard::take_from_bytes::<Animal>(&state.bytes) else {
        return Default::default();
    };
    let own = if rest.is_empty() {
        O::default()
    } else {
        postcard::from_bytes(rest).unwrap_or_default()
    };
    (animal, own)
}

/// What every animal shares, from any animal's type state.
pub(crate) fn animal_of(state: &TypeState) -> Animal {
    if state.bytes.is_empty() || state.version != VERSION {
        return Animal::default();
    }
    postcard::take_from_bytes::<Animal>(&state.bytes)
        .map(|(animal, _)| animal)
        .unwrap_or_default()
}

/// The type state of `animal` and `own`: nothing for a fresh animal's, and nothing
/// after the shared part for a fresh type's own.
pub(crate) fn write<O: Own>(animal: &Animal, own: &O) -> TypeState {
    let fresh_own = *own == O::default();
    if fresh_own && *animal == Animal::default() {
        return TypeState::default();
    }
    let mut bytes = postcard::to_allocvec(animal).expect("a type state is encoded into memory");
    if !fresh_own {
        bytes.extend(postcard::to_allocvec(own).expect("a type state is encoded into memory"));
    }
    TypeState {
        version: VERSION,
        bytes,
    }
}

/// The bytes of `state` after its shared part: the type's own, and whatever a type keeps
/// after its own (the pig's boost), nothing for a fresh animal's or one this build
/// cannot read.
pub(crate) fn own_bytes(state: &TypeState) -> &[u8] {
    if state.bytes.is_empty() || state.version != VERSION {
        return &[];
    }
    postcard::take_from_bytes::<Animal>(&state.bytes).map_or(&[], |(_, rest)| rest)
}

/// [`write`], with `tail` after the type's own part, which is written then whether it is
/// fresh or not, so that the tail is found where a reader looks for it: what a type
/// added to its own part after records of it were kept, which those records end before
/// and so read as nothing (the pig's boost, `docs/groundwork/n7h-riding-plan.md`,
/// section 10 item 4).
pub(crate) fn write_with_tail<O: Own>(animal: &Animal, own: &O, tail: &[u8]) -> TypeState {
    if tail.is_empty() {
        return write(animal, own);
    }
    let mut bytes = postcard::to_allocvec(animal).expect("a type state is encoded into memory");
    bytes.extend(postcard::to_allocvec(own).expect("a type state is encoded into memory"));
    bytes.extend_from_slice(tail);
    TypeState {
        version: VERSION,
        bytes,
    }
}

/// `state` with its shared part made `animal`, its type's own kept as it was.
pub(crate) fn with_animal(state: &TypeState, animal: &Animal) -> TypeState {
    let rest: &[u8] = if state.bytes.is_empty() || state.version != VERSION {
        &[]
    } else {
        postcard::take_from_bytes::<Animal>(&state.bytes).map_or(&[], |(_, rest)| rest)
    };
    if rest.is_empty() && *animal == Animal::default() {
        return TypeState::default();
    }
    let mut bytes = postcard::to_allocvec(animal).expect("a type state is encoded into memory");
    bytes.extend_from_slice(rest);
    TypeState {
        version: VERSION,
        bytes,
    }
}

/// Changes what every animal shares of `mob`.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut Animal)) {
    let mut animal = animal_of(&mob.state);
    let before = animal.clone();
    change(&mut animal);
    if animal != before {
        mob.state = with_animal(&mob.state, &animal);
    }
}

/// `isBaby()` of an animal.
pub(crate) fn is_baby(mob: &Mob) -> bool {
    animal_of(&mob.state).is_baby()
}

/// `getDefaultDimensions`: a baby's own size, else the type's.
pub(crate) fn dimensions(mob: &Mob, baby: Dimensions) -> Option<Dimensions> {
    is_baby(mob).then_some(baby)
}

/// `AgeableMob`'s synced values: the baby (16) and the age lock (17).
pub(crate) fn shown(mob: &Mob) -> Vec<(u8, crate::shown::DataShown, bool)> {
    use crate::shown::DataShown;
    let animal = animal_of(&mob.state);
    let baby = animal.is_baby();
    vec![
        (BABY_INDEX, DataShown::Boolean(baby), !baby),
        (
            AGE_LOCKED_INDEX,
            DataShown::Boolean(animal.age_locked),
            !animal.age_locked,
        ),
    ]
}

/// `Animal.actuallyHurt`: its love ends before the damage.
pub(crate) fn actually_hurt(mob: &mut Mob) {
    update(mob, Animal::reset_love);
}

/// `Animal.customServerAiStep`: love ends for an animal that is not of age nothing.
pub(crate) fn custom_server_ai_step(tick: &mut MobTick<'_>) {
    let now = tick.lived;
    update(&mut tick.mob, |animal| {
        if animal.age(now) != 0 && animal.love_until != 0 {
            animal.reset_love();
        }
    });
}

/// `getRandomX(scale)`, `getRandomY()` and `getRandomZ(scale)` of a particle's place,
/// drawn and not sent.
fn particle_draws(draws: &mut Random, count: usize) {
    for _ in 0..count {
        draws.next_double();
    }
}

/// `AgeableMob.aiStep` and `Animal.aiStep` after `LivingEntity`'s: the forced age's
/// particles, the growing (a baby's grown up at nothing, which its record says), the
/// age lock's particles, then love's end for an animal not of age nothing and love's
/// countdown with its hearts drawn every tenth tick.
pub(crate) fn after_ai_step(tick: &mut MobTick<'_>) {
    after_ai_step_growing(tick, true);
}

/// [`after_ai_step`] of a type that may not grow up where `grows` is false (a skeleton
/// or zombie foal, whose `canAgeUp` is false).
pub(crate) fn after_ai_step_growing(tick: &mut MobTick<'_>, grows: bool) {
    let now = tick.lived;
    let next = now.saturating_add(1);
    let alive = !tick.mob.body.vitals.dying();
    let mut animal = animal_of(&tick.mob.state);
    let before = animal.clone();
    let forced = left(animal.forced_until, now);
    if forced > 0 && forced % 4 == 0 {
        particle_draws(tick.draws, 3);
    }
    if alive && grows && animal.can_age_up() && animal.age(next) == 0 {
        // `setAge(0)`: the boundary, which the record shows.
        animal.set_age(0, next);
    }
    let lock = left(animal.lock_until, now);
    if lock > 0 && lock % 2 == 0 {
        particle_draws(tick.draws, 3);
    }
    if animal.age(next) != 0 && animal.love_until != 0 {
        animal.reset_love();
    }
    let love = animal.love(now);
    if love > 0 && (love - 1) % 10 == 0 {
        for _ in 0..3 {
            tick.draws.next_gaussian();
        }
        particle_draws(tick.draws, 3);
    }
    if animal != before {
        tick.mob.state = with_animal(&tick.mob.state, &animal);
    }
}

/// `Animal.getWalkTargetValue(pos)`: ten on a grass block, else the light's cost
/// (`getPathfindingCostFromLightLevels`), with `on` the block a type prefers (grass,
/// or a mooshroom's mycelium).
pub(crate) fn walk_target_value(
    ai: &Ai<'_, '_>,
    pos: BlockPos,
    on: clustine_data::BlockState,
) -> f32 {
    let below = ai.tick.world.state(BlockPos::new(pos.x, pos.y - 1, pos.z));
    if is_block(below, on) {
        return 10.0;
    }
    light_cost(ai, pos)
}

/// `LevelReader.getPathfindingCostFromLightLevels(pos)`: the light's worth less a half.
/// The brightness is the larger of the sky's light less its darkening and the block's,
/// 15 where the view has no light.
pub(crate) fn light_cost(ai: &Ai<'_, '_>, pos: BlockPos) -> f32 {
    let brightness = ai.tick.world.light(pos).map_or(15, |light| {
        (i32::from(light.sky) - ai.tick.world.sky_darken()).max(i32::from(light.block))
    });
    let b = brightness as f32 / 15.0;
    // `Mth.lerp(ambientLight, b / (4 - 3b), 1)`, the overworld's ambient light of 0.
    let magic = b / (4.0 - 3.0 * b);
    magic - 0.5
}

/// `Animal.mobInteract` (feeding: love for an adult free of its cooldown and not in
/// love, a tenth of its time for a baby, owed back) and, where the item is not its food,
/// `AgeableMob.mobInteract` (the golden dandelion on a baby). `eat` is the type's
/// eating sound, where it has one (a pig's).
pub(crate) fn mob_interact(used: &mut Used<'_>, food: Items, eat: Option<i32>) -> Interaction {
    let now = used.moment;
    let mut animal = animal_of(&used.mob.state);
    if food.test(used.held.as_ref()) {
        let age = animal.age(now);
        if age == 0 && animal.love(now) <= 0 {
            animal.set_in_love(now);
            used.mob.state = with_animal(&used.mob.state, &animal);
            used.did.push(Did::Event(18));
            eating_sound(used, eat);
            return Interaction::SuccessServer;
        }
        if animal.can_age_up() {
            animal.age_up(speed_up_seconds(age.wrapping_neg()), true, now);
            used.mob.state = with_animal(&used.mob.state, &animal);
            eating_sound(used, eat);
            return Interaction::Success;
        }
    }
    golden_dandelion(used)
}

/// `Mob.makeSound(eatSound)` of a pig fed: at its volume and voice, two draws.
fn eating_sound(used: &mut Used<'_>, eat: Option<i32>) {
    let Some(sound) = eat else {
        return;
    };
    let baby = is_baby(used.mob);
    let pitch = crate::mobs::body::hurt::voice_pitch(used.draws, baby);
    used.did.push(Did::Sound {
        sound,
        source: crate::physics::item::SOUND_SOURCE_NEUTRAL,
        at: used.position,
        volume: 1.0,
        pitch,
        but_the_player: false,
    });
}

/// The sound source of what players do (`SoundSource.PLAYERS`).
pub(crate) const SOUND_SOURCE_PLAYERS: i32 = 7;

/// `AgeableMob.mobInteract`: a golden dandelion on a baby that may be locked, once its
/// last use's 40 ticks are over, locks it (or unlocks it) as a newborn.
fn golden_dandelion(used: &mut Used<'_>) -> Interaction {
    let now = used.moment;
    let mut animal = animal_of(&used.mob.state);
    let usable = used
        .held
        .as_ref()
        .is_some_and(|stack| stack.count > 0 && stack.item == items::GOLDEN_DANDELION)
        && animal.is_baby()
        && left(animal.lock_until, now) == 0
        && !entity_in_tag(i32::from(used.kind), "minecraft:cannot_be_age_locked");
    if !usable {
        return Interaction::Pass;
    }
    animal.age_locked = !animal.age_locked;
    animal.set_age(BABY_START_AGE, now);
    animal.lock_until = now + PARTICLE_TICKS;
    used.mob.state = with_animal(&used.mob.state, &animal);
    if animal.age_locked {
        used.mob.body.flags.persistent = true;
    }
    // At the middle of its block (`playSound(null, blockPosition(), …)`).
    let block = physics::containing(used.position);
    used.did.push(Did::Sound {
        sound: if animal.age_locked {
            sounds::ITEM_GOLDEN_DANDELION_USE
        } else {
            sounds::ITEM_GOLDEN_DANDELION_UNUSE
        },
        source: SOUND_SOURCE_PLAYERS,
        at: [
            f64::from(block.x) + 0.5,
            f64::from(block.y) + 0.5,
            f64::from(block.z) + 0.5,
        ],
        volume: 1.0,
        pitch: 1.0,
        but_the_player: false,
    });
    Interaction::Success
}

/// A baby made by a breeding or an egg: `setBaby(true)` at its first moment, of the
/// usual start age unless its type made it a baby of its own (`getBabyStartAge`: a
/// snifflet's twice as long).
pub(crate) fn made_baby(mob: &mut Mob) {
    update(mob, |animal| {
        if !animal.is_baby() {
            animal.set_age(BABY_START_AGE, 0);
        }
    });
}

/// `AgeableMob.finalizeSpawn`'s part before `Mob`'s (`docs/groundwork/n7-brief.md`,
/// section 8.7): the group made `group` where the spawn has none (the first of a group,
/// or an egg's mob); a later member of a group that may have babies draws one
/// `nextFloat()` and is a baby at or below the group's chance; then the group counts
/// it. Whether the draw made a baby, which the caller sets as its type's
/// `setAge(getBabyStartAge())` does.
pub(crate) fn ageable_draw(spawn: &mut Spawning<'_>, group: SpawnGroup) -> bool {
    let ageable = spawn.group.get_or_insert(group).ageable();
    let baby =
        ageable.babies && ageable.made > 0 && spawn.level.next_float() <= ageable.baby_chance;
    ageable.made += 1;
    baby
}

/// [`ageable_draw`] of an animal that becomes a baby by it at the usual start age: the
/// age set at the mob's first moment, as everything of a generated mob counts from its
/// `lived` 0 (`docs/groundwork/n7l-plan.md`, section 2.2).
pub(crate) fn finalize_ageable(spawn: &mut Spawning<'_>, group: SpawnGroup) {
    if ageable_draw(spawn, group) {
        update(spawn.mob, |animal| animal.set_age(BABY_START_AGE, 0));
    }
}

/// [`finalize_ageable`] with `AgeableMobGroupData(true)`, the group of an animal whose
/// type makes none of its own.
pub(crate) fn finalize_ageable_default(spawn: &mut Spawning<'_>) {
    finalize_ageable(spawn, SpawnGroup::Ageable(AgeableGroup::new(true)));
}

/// A held stack of `item`, as tests and the tables of food name it.
pub(crate) fn is_item(stack: Option<&ItemStack>, item: i32) -> bool {
    stack.is_some_and(|stack| stack.count > 0 && stack.item == item)
}

/// Whether the biome named `biome` is in the biome tag `tag`.
pub(crate) fn biome_in_tag(biome: Option<&str>, tag: &str) -> bool {
    let Some(biome) = biome else {
        return false;
    };
    let Some(id) = clustine_data::synced_registry("minecraft:worldgen/biome")
        .and_then(|biomes| biomes.id_of(biome))
    else {
        return false;
    };
    clustine_data::TAGS
        .iter()
        .find(|tags| tags.registry == "minecraft:worldgen/biome")
        .and_then(|tags| tags.tags.iter().find(|known| known.name == tag))
        .is_some_and(|known| known.entries.contains(&id))
}

/// The farm animals' temperature variants (cow, pig, chicken) and their sound variants:
/// their ids are their places in the registries the server sends, by name.
pub(crate) mod variants {
    use super::biome_in_tag;

    /// The id of the entry `name` of the registry `registry`.
    pub(crate) fn id(registry: &str, name: &str) -> u8 {
        clustine_data::synced_registry(registry)
            .and_then(|known| known.id_of(name))
            .and_then(|id| u8::try_from(id).ok())
            .expect("the farm animals' variants are in the registries the server sends")
    }

    /// How many entries the registry `registry` has.
    pub(crate) fn count(registry: &str) -> u32 {
        clustine_data::synced_registry(registry).map_or(1, |known| known.entries.len() as u32)
    }

    /// `VariantUtils.selectVariantToSpawn(context, registry)` of a temperature variant
    /// (the vanilla data: cold and warm by biome tag at priority 1, temperate always at
    /// 0): the pairs of the highest priority whose condition holds, in the registry's
    /// order, one drawn even when one is left.
    pub(crate) fn select(
        registry: &str,
        biome: Option<&str>,
        level: &mut dyn clustine_world::generation::Draws,
    ) -> u8 {
        let entries =
            clustine_data::synced_registry(registry).map_or(&[][..], |known| known.entries);
        // (name, priority, condition), stably sorted by priority, highest first.
        let mut pairs: Vec<(&str, i32, Option<&str>)> = entries
            .iter()
            .map(|name| match *name {
                "minecraft:cold" => (*name, 1, Some("minecraft:spawns_cold_variant_farm_animals")),
                "minecraft:warm" => (*name, 1, Some("minecraft:spawns_warm_variant_farm_animals")),
                _ => (*name, 0, None),
            })
            .collect();
        pairs.sort_by_key(|(_, priority, _)| std::cmp::Reverse(*priority));
        let mut best = i32::MIN;
        let mut kept = Vec::new();
        for (name, priority, condition) in pairs {
            if priority < best {
                continue;
            }
            if condition.is_none_or(|tag| biome_in_tag(biome, tag)) {
                best = priority;
                kept.push(name);
            }
        }
        let chosen = if kept.is_empty() {
            "minecraft:temperate"
        } else {
            kept[level.next_int(kept.len() as u32) as usize]
        };
        id(registry, chosen)
    }
}

/// An animal that a goal reads in the view: where it is, whether it lives, what every
/// animal shares as of its `lived`, and whether it is in a copy.
#[derive(Debug, Clone)]
pub(crate) struct AnimalSeen {
    pub(crate) looked: Looked,
    pub(crate) animal: Animal,
    pub(crate) lived: u32,
    /// `isPanicking()`: a panic goal runs (never read across a line).
    pub(crate) panicking: bool,
}

impl AnimalSeen {
    fn of(seen: &Seen<'_>) -> Option<Self> {
        let EntityData::Mob { state, .. } = &seen.entity.data else {
            return None;
        };
        Some(Self {
            looked: Looked::of_seen(seen),
            animal: animal_of(state),
            lived: seen.entity.lived,
            panicking: ai::panicking_record(seen.entity),
        })
    }
}

/// `this.canMate(other)` of the mob's type beyond their love, for the animal `other` the
/// view shows (`MobType::can_mate`), the mob's own record as its tick has it so far.
fn mates(ai: &Ai<'_, '_>, other: &Seen<'_>) -> bool {
    let Some(me) = ai.tick.world.entity(ai.tick.key) else {
        return false;
    };
    let Ok(partner) = Mob::decode(&other.entity.data) else {
        return false;
    };
    let mut entity = me.entity.clone();
    entity.lived = ai.tick.lived;
    ai.tick.mob_type.can_mate(
        super::Mate {
            entity: &entity,
            mob: &ai.tick.mob,
        },
        super::Mate {
            entity: other.entity,
            mob: &partner,
        },
    )
}

/// The animal `key` of the view, if it is one.
fn animal_seen(ai: &Ai<'_, '_>, key: EntityId) -> Option<AnimalSeen> {
    ai.tick.world.entity(key).as_ref().and_then(AnimalSeen::of)
}

/// The mob's box.
fn own_box(ai: &Ai<'_, '_>) -> Aabb {
    Aabb::of_entity(ai.position(), ai.width(), ai.height())
}

/// `BreedGoal`'s state: the partner and the ticks of love spent on it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Courting {
    pub(crate) partner: Option<EntityId>,
    pub(crate) love_time: i32,
}

/// `BreedGoal(animal, speed)`: an animal in love goes to the nearest of its kind in love
/// within 8 blocks, and once it has spent its count on it close by, breeds.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BreedGoal {
    pub(crate) speed: f64,
}

impl BreedGoal {
    /// `getFreePartner()`: of the animals of its kind within 8 blocks that it may mate
    /// with and that do not panic, the nearest; the earlier of two as near.
    fn free_partner(ai: &mut Ai<'_, '_>) -> Option<EntityId> {
        let bounds = own_box(ai).inflate([8.0, 8.0, 8.0]);
        let kind = ai.tick.kind;
        let found: Vec<AnimalSeen> = ai
            .tick
            .world
            .entities_in(&bounds)
            .iter()
            .filter(|seen| seen.entity.kind == kind)
            .filter(|seen| mates(ai, seen))
            .filter_map(AnimalSeen::of)
            .collect();
        let conditions = Conditions::non_combat().range(8.0).ignore_line_of_sight();
        let from = ai.position();
        let mut best: Option<(f64, EntityId)> = None;
        for other in found {
            if !conditions.test(ai, &other.looked, &|_| true) {
                continue;
            }
            if !other.animal.in_love_for_others(other.lived) || other.panicking {
                continue;
            }
            let distance = other.looked.distance_sqr(from);
            if best.is_none_or(|(nearest, _)| distance < nearest) {
                best = Some((distance, other.looked.key));
            }
        }
        best.map(|(_, key)| key)
    }

    /// `breed()`: the request to breed `partner`, which the region carries out at once
    /// where it holds the partner, and as an effect across a line, where only the higher
    /// key asks. An asker is marked asked, stops, and lets its goal end.
    fn breed(state: &mut Courting, ai: &mut Ai<'_, '_>, partner: &AnimalSeen) {
        let key = partner.looked.key;
        if partner.looked.copied && ai.tick.key < key {
            // The lower key's count does nothing: the partner's region answers it.
            return;
        }
        let now = ai.tick.lived;
        let mut number = 0;
        update(&mut ai.tick.mob, |animal| {
            animal.requests = animal.requests.wrapping_add(1);
            number = animal.requests;
            animal.asked = Some(Asked {
                partner: key,
                number,
                at: now,
            });
        });
        ai.tick.out.local.push(Local::BreedAsk {
            partner: key,
            number,
        });
        navigation::stop(ai);
        state.partner = None;
    }
}

impl Goal for BreedGoal {
    type State = Courting;

    fn flags(&self) -> u8 {
        flags::MOVE | flags::LOOK
    }

    fn can_use(&self, state: &mut Courting, ai: &mut Ai<'_, '_>) -> bool {
        let now = ai.tick.lived;
        let animal = animal_of(&ai.tick.mob.state);
        if !animal.in_love(now) || animal.waiting(now).is_some() {
            return false;
        }
        state.partner = Self::free_partner(ai);
        state.partner.is_some()
    }

    fn can_continue(&self, state: &mut Courting, ai: &mut Ai<'_, '_>) -> bool {
        let Some(partner) = state.partner.and_then(|key| animal_seen(ai, key)) else {
            return false;
        };
        partner.looked.alive
            && partner.animal.in_love_for_others(partner.lived)
            && state.love_time < 60
            && !partner.panicking
    }

    fn stop(&self, state: &mut Courting, _: &mut Ai<'_, '_>) {
        state.partner = None;
        state.love_time = 0;
    }

    fn tick(&self, state: &mut Courting, ai: &mut Ai<'_, '_>) {
        let Some(partner) = state.partner.and_then(|key| animal_seen(ai, key)) else {
            return;
        };
        let at = partner.looked.position;
        let max_pitch = ai.head().max_x_rot as f32;
        controls::set_look_at(ai, [at[0], partner.looked.eye_y, at[2]], 10.0, max_pitch);
        navigation::move_to_entity(ai, partner.looked.block_position(), self.speed);
        state.love_time += 1;
        if state.love_time >= self.adjusted_tick_delay(60)
            && partner.looked.distance_sqr(ai.position()) < 9.0
        {
            Self::breed(state, ai, &partner);
        }
    }
}

/// `FollowParentGoal`'s state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct FollowingParent {
    pub(crate) parent: Option<EntityId>,
    pub(crate) recalc: i32,
}

/// `FollowParentGoal(animal, speed)`: a baby keeps near the nearest grown animal of its
/// kind. It holds no flag, so it runs beside whatever moves the baby.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FollowParentGoal {
    pub(crate) speed: f64,
}

impl Goal for FollowParentGoal {
    type State = FollowingParent;

    fn flags(&self) -> u8 {
        0
    }

    fn can_use(&self, state: &mut FollowingParent, ai: &mut Ai<'_, '_>) -> bool {
        if !animal_of(&ai.tick.mob.state).is_baby() {
            return false;
        }
        let bounds = own_box(ai).inflate([8.0, 4.0, 8.0]);
        let kind = ai.tick.kind;
        let from = ai.position();
        let mut best: Option<(f64, EntityId)> = None;
        for seen in ai.tick.world.entities_in(&bounds) {
            if seen.entity.kind != kind || seen.entity.key == ai.tick.key {
                continue;
            }
            let Some(other) = AnimalSeen::of(&seen) else {
                continue;
            };
            if other.animal.is_baby() {
                continue;
            }
            // `d <= best`: the later of two as near.
            let distance = other.looked.distance_sqr(from);
            if best.is_none_or(|(nearest, _)| distance <= nearest) {
                best = Some((distance, other.looked.key));
            }
        }
        match best {
            Some((distance, key)) if distance >= 9.0 => {
                state.parent = Some(key);
                true
            }
            _ => false,
        }
    }

    fn can_continue(&self, state: &mut FollowingParent, ai: &mut Ai<'_, '_>) -> bool {
        if !animal_of(&ai.tick.mob.state).is_baby() {
            return false;
        }
        let Some(parent) = state.parent.and_then(|key| animal_seen(ai, key)) else {
            return false;
        };
        let distance = parent.looked.distance_sqr(ai.position());
        parent.looked.alive && (9.0..=256.0).contains(&distance)
    }

    fn start(&self, state: &mut FollowingParent, _: &mut Ai<'_, '_>) {
        state.recalc = 0;
    }

    fn stop(&self, state: &mut FollowingParent, _: &mut Ai<'_, '_>) {
        state.parent = None;
    }

    fn tick(&self, state: &mut FollowingParent, ai: &mut Ai<'_, '_>) {
        state.recalc -= 1;
        if state.recalc > 0 {
            return;
        }
        state.recalc = self.adjusted_tick_delay(10);
        if let Some(parent) = state.parent.and_then(|key| animal_seen(ai, key)) {
            navigation::move_to_entity(ai, parent.looked.block_position(), self.speed);
        }
    }
}

/// What the region needs of a farm animal's record to breed it: whether it is one, and
/// what every animal shares.
pub(crate) fn of_record(entity: &Entity) -> Option<Animal> {
    // Of the built types, those of the game's `Animal` (not a bat, not a fish).
    let family = crate::mobs::body::ladder::of(entity.kind);
    if !super::is_built(entity.kind) || family != crate::mobs::body::ladder::Family::Animal {
        return None;
    }
    match &entity.data {
        EntityData::Mob { state, .. } => Some(animal_of(state)),
        _ => None,
    }
}

/// Whether the block is a grass block (what a sheep eats below it).
pub(crate) fn is_grass_block(state: clustine_data::BlockState) -> bool {
    is_block(state, blocks::GRASS_BLOCK)
}
