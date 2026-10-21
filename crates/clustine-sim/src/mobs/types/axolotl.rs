//! The axolotl (`Axolotl` with `$Variant`, `$AxolotlGroupData` and its controls;
//! `docs/groundwork/n7-brief.md`, section 7.4): an amphibious animal of five colours, four
//! of them common, that breathes air for 6000 ticks out of water and dries out after,
//! swims without sinking, is fed and bred with a bucket of tropical fish (the player
//! given a water bucket back), and is taken into a water bucket and poured out of its
//! own as the fish are.
//!
//! Its mind is a brain (`AxolotlAi`), which N7j's machinery brings; until then it has no
//! activities and only floats, falls and swims as its body does. What the brain will
//! read and write of it is named in [`brain`]: the play-dead memory a hurt sets and its
//! synced flag follows, and what its controls and its voice ask of that flag.
//!
//! Named differences: a bucket keeps nothing of it (the plan's question 4), so one
//! poured is a fresh lucy, as a bucket from the game's creative menu pours; its swimming
//! sounds are not made (the living body plays none yet); one poured from a bucket still
//! draws for an idle despawn it cannot take (`requiresCustomPersistence` is not asked of
//! a type).

use clustine_data::{PathType, attributes as ids, items, sounds};
use clustine_world::ItemStack;
use serde::{Deserialize, Serialize};

use super::animal::{self, Own};
use super::{Breeding, Did, Dimensions, Interaction, MobType, Spawning, Used, cow, water};
use crate::living::{Damage, Sinks, Travel};
use crate::mobs::Mob;
use crate::mobs::ai::goals::Items;
use crate::mobs::ai::navigation::{Kind, Settings};
use crate::mobs::ai::{Ai, HeadLimits, LookKind, MoveKind, Setup, controls};
use crate::mobs::body::hurt::{DamageSource, Struck};
use crate::mobs::tick::MobTick;
use crate::region::Random;
use crate::shown::DataShown;

/// The variants' ids (`Axolotl.Variant`), the first four common.
pub(crate) const LUCY: i32 = 0;
pub(crate) const WILD: i32 = 1;
pub(crate) const GOLD: i32 = 2;
pub(crate) const CYAN: i32 = 3;
pub(crate) const BLUE: i32 = 4;

/// The common variants, in `values()` order, which `getSpawnVariant` keeps.
pub(crate) const COMMON: [i32; 4] = [LUCY, WILD, GOLD, CYAN];

/// `#axolotl_food`: a bucket of tropical fish.
const FOOD: Items = Items::Tag("minecraft:axolotl_food");

/// The synced indices of `DATA_VARIANT`, `DATA_PLAYING_DEAD` and `FROM_BUCKET`, on from
/// `AgeableMob`'s 16 and 17.
pub(crate) const VARIANT_INDEX: u8 = 18;
pub(crate) const PLAYING_DEAD_INDEX: u8 = 19;
pub(crate) const FROM_BUCKET_INDEX: u8 = 20;

/// `getMaxAirSupply()`: five minutes out of water.
pub(crate) const MAX_AIR: i32 = 6000;

/// `PLAY_DEAD_TICKS` as a hurt sets it.
pub(crate) const PLAY_DEAD_TICKS: i32 = 200;

/// What an axolotl keeps beyond an animal: its variant (`DATA_VARIANT`, saved as
/// `Variant`), whether it shows itself playing dead (`DATA_PLAYING_DEAD`, which its
/// brain's memory sets every tick) and whether it came from a bucket (`FROM_BUCKET`,
/// saved as `FromBucket`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct AxolotlState {
    pub(crate) variant: i32,
    pub(crate) playing_dead: bool,
    pub(crate) from_bucket: bool,
}

impl Own for AxolotlState {}

/// An axolotl's own state.
pub(crate) fn read(mob: &Mob) -> AxolotlState {
    animal::read::<AxolotlState>(&mob.state).1
}

/// Changes an axolotl's own state.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut AxolotlState)) {
    let (shared, mut own) = animal::read::<AxolotlState>(&mob.state);
    let before = own.clone();
    change(&mut own);
    if own != before {
        mob.state = animal::write(&shared, &own);
    }
}

/// `Variant.byId(id)`: an id out of range is lucy (`ByIdMap.continuous`, `ZERO`).
pub(crate) fn variant_by_id(id: i32) -> i32 {
    if (LUCY..=BLUE).contains(&id) {
        id
    } else {
        LUCY
    }
}

/// `Variant.getCommonSpawnVariant(random)`: one of the four common ones, `nextInt(4)`.
pub(crate) fn common_variant(random: &mut Random) -> i32 {
    COMMON[random.next_int(COMMON.len() as u32) as usize]
}

/// `finalizeSpawn`'s variant for the first of a group (an egg's): the group's two
/// colours, then this one's of them (`AxolotlGroupData.getVariant`, `nextInt(2)`), all
/// of the level's random.
pub(crate) fn spawn_variant(level: &mut Random) -> i32 {
    let group = [common_variant(level), common_variant(level)];
    group[level.next_int(2) as usize]
}

/// `getBreedOffspring`'s variant, of the parent's own random: blue one time in 1200
/// (`getRareSpawnVariant`, a `nextInt(1)` of the one rare kind), else either parent's
/// by a coin.
pub(crate) fn offspring_variant(draws: &mut Random, this: i32, partner: i32) -> i32 {
    if draws.next_int(1200) == 0 {
        let rare = [BLUE];
        return rare[draws.next_int(rare.len() as u32) as usize];
    }
    if draws.next_bool() { this } else { partner }
}

/// What `hurtServer`'s test of playing dead reads of the axolotl and its hurt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Hurt {
    pub(crate) no_ai: bool,
    pub(crate) amount: f32,
    /// Its health before the hurt.
    pub(crate) health: f32,
    pub(crate) max_health: f32,
    /// `isInWater()`, as its last tick left it.
    pub(crate) in_water: bool,
    /// Whether the source names an entity that caused or dealt it.
    pub(crate) by_entity: bool,
    pub(crate) playing_dead: bool,
}

/// The game's test of a hurt that makes it play dead (`hurtServer`, before its parent's):
/// with AI, one time in three (`nextInt(3) == 0`), then hurt harder than a second draw
/// of nought to two or below half its health, the hurt less than its health, in water,
/// by an entity, and not playing dead already. The second draw is made only after the
/// first came up nought, and none without AI.
pub(crate) fn plays_dead(draws: &mut Random, hurt: &Hurt) -> bool {
    if hurt.no_ai || draws.next_int(3) != 0 {
        return false;
    }
    let second = draws.next_int(3) as f32;
    (second < hurt.amount || hurt.health / hurt.max_health < 0.5)
        && hurt.amount < hurt.health
        && hurt.in_water
        && hurt.by_entity
        && !hurt.playing_dead
}

/// The brain's seams: what N7j's machinery will give and take here. Each is small and
/// named, so that wiring the provider changes these alone.
pub(crate) mod brain {
    use super::Mob;
    use crate::mobs::body::hurt::Struck;

    /// `PLAY_DEAD_TICKS` as the brain has it after its tick, which the synced flag
    /// follows (`customServerAiStep`): none until the brain keeps memories.
    pub(crate) fn play_dead_ticks(mob: &Mob) -> Option<i32> {
        let _ = mob;
        None
    }

    /// `brain.setMemory(PLAY_DEAD_TICKS, ticks)`, of a hurt that made it play dead:
    /// nothing until the brain keeps memories.
    pub(crate) fn set_play_dead_ticks(mob: &mut Mob, ticks: i32) {
        let _ = (mob, ticks);
    }

    /// `isInWater()` of a mob a hurt finds: a [`Struck`] does not carry the body's
    /// state of its last tick, which N7j1 adds beside the memory this seam is for; till
    /// then no hurt makes it play dead, and its draws are made as the game's are.
    pub(crate) fn in_water(struck: &Struck<'_>) -> bool {
        let _ = struck;
        false
    }
}

/// A baby's size (`BABY_DIMENSIONS`): `scalable(0.375, 0.21)`, its eyes at `0.09375`.
pub(crate) fn baby_size() -> Dimensions {
    Dimensions {
        width: 0.375,
        height: 0.21,
        eye_height: 0.093_75,
    }
}

/// `handleAirSupply(level, air)` after the living body's air rule, which it overwrites:
/// alive out of water, one less a tick from what it had as the tick began, and at -20 a
/// drying out of 2 with its air back to nothing; in water, or dead, all 6000 at once.
/// Without AI the living rule alone holds.
fn handle_air_supply(tick: &mut MobTick<'_>, air: i32) {
    if tick.mob.body.flags.no_ai {
        return;
    }
    if water::alive(&tick.mob) && !tick.living.body.in_water {
        tick.living.air = air - 1;
        // `shouldTakeDrowningDamage`: at -20 or below.
        if tick.living.air <= -20 {
            tick.living.air = 0;
            crate::mobs::body::hurt_server(tick, Damage::DryOut, 2.0);
        }
    } else {
        tick.living.air = MAX_AIR;
    }
}

/// `Axolotl.mobInteract`: a water bucket takes it, with its own filling sound; else an
/// animal's feeding with a bucket of tropical fish, the player given a water bucket for
/// it where they have none (`usePlayerItem`, `createFilledResult` of three arguments: a
/// creative player keeps the fish), else the golden dandelion on a baby.
fn mob_interact(used: &mut Used<'_>) -> Interaction {
    if let Some(taken) = water::bucket_pickup_sounding(used, sounds::ITEM_BUCKET_FILL_AXOLOTL) {
        return taken;
    }
    let fed = FOOD.test(used.held);
    let at = used.did.len();
    let result = animal::mob_interact(used, FOOD, None);
    if fed && result != Interaction::Pass {
        // Before the love's hearts: `usePlayerItem` comes first.
        used.did.insert(
            at,
            Did::Give {
                stack: ItemStack {
                    item: items::WATER_BUCKET,
                    count: 1,
                },
                no_duplicates: true,
            },
        );
    }
    result
}

/// The axolotl.
pub(crate) struct Axolotl;

impl MobType for Axolotl {
    /// No goals: its mind is its brain's. Its constructor's navigation, maluses and
    /// controls (`AxolotlMoveControl`, `AxolotlLookControl`).
    fn register_goals(&self, setup: &mut Setup) {
        setup.config.navigation = Settings::of(Kind::Amphibious);
        cow::animal_maluses(setup);
        setup.set_malus(PathType::Water, 0.0);
        setup.config.move_kind = MoveKind::SmoothSwimming {
            max_turn_x: 85,
            max_turn_y: 10,
            in_water: 0.1,
            outside_water: 0.5,
            gravity: false,
        };
        setup.config.look_kind = LookKind::SmoothSwimming {
            max_y_rot_from_center: 20,
        };
    }

    /// `getMaxHeadXRot()` and `getMaxHeadYRot()`: a degree each.
    fn head(&self) -> HeadLimits {
        HeadLimits {
            max_x_rot: 1,
            max_y_rot: 1,
            ..HeadLimits::default()
        }
    }

    /// Its move control does nothing while it plays dead.
    fn tick_move_control(&self, ai: &mut Ai<'_, '_>) {
        if !read(&ai.tick.mob).playing_dead {
            controls::tick_move(ai);
        }
    }

    /// Its look control does nothing while it plays dead.
    fn tick_look_control(&self, ai: &mut Ai<'_, '_>) {
        if !read(&ai.tick.mob).playing_dead {
            controls::tick_look(ai);
        }
    }

    /// `finalizeSpawn` before `Animal`'s: an egg's is the first of a group, which draws
    /// the group's two colours and its own of them; never a baby.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        let variant = spawn_variant(spawn.level);
        update(spawn.mob, |axolotl| axolotl.variant = variant);
    }

    /// `finalizeSpawn` of `BUCKET` returns before anything, `Mob`'s too.
    fn finalizes_when_poured(&self) -> bool {
        false
    }

    /// `setFromBucket(true)`.
    fn poured_from_bucket(&self, _: u16, mob: &mut Mob) {
        update(mob, |axolotl| axolotl.from_bucket = true);
    }

    /// `customServerAiStep`: the brain's tick and its activities (the brain's, to
    /// come), then with AI the synced flag of playing dead from its memory. It does not
    /// call `Animal`'s: its love is not ended for an age not nothing.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        if tick.mob.body.flags.no_ai {
            return;
        }
        let playing_dead = brain::play_dead_ticks(&tick.mob).is_some_and(|ticks| ticks > 0);
        update(&mut tick.mob, |axolotl| axolotl.playing_dead = playing_dead);
    }

    /// The draws of its test of playing dead, before any of the hurt's.
    fn before_hurt_server(&self, struck: &mut Struck<'_>, source: &DamageSource, amount: f32) {
        let max_health = struck
            .mob
            .body
            .attributes
            .value(struck.kind, crate::mobs::body::attribute(ids::MAX_HEALTH))
            as f32;
        let hurt = Hurt {
            no_ai: struck.mob.body.flags.no_ai,
            amount,
            health: struck.mob.body.vitals.health,
            max_health,
            in_water: brain::in_water(struck),
            by_entity: source.cause.is_some() || source.direct.is_some(),
            playing_dead: read(struck.mob).playing_dead,
        };
        if plays_dead(struck.draws, &hurt) {
            brain::set_play_dead_ticks(struck.mob, PLAY_DEAD_TICKS);
        }
    }

    fn after_base_tick(&self, tick: &mut MobTick<'_>, air: i32) {
        handle_air_supply(tick, air);
    }

    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        animal::after_ai_step(tick);
    }

    /// `travelInWater`: at its own speed along its input, slowed to nine tenths, never
    /// sinking.
    fn travel(&self, tick: &MobTick<'_>) -> Travel {
        Travel::Swims {
            speed: tick.mob.body.inputs.speed,
            sinks: Sinks::Never,
        }
    }

    fn pushed_by_fluid(&self) -> bool {
        false
    }

    fn max_air(&self) -> i32 {
        MAX_AIR
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        mob_interact(used)
    }

    /// A child of either parent's colour, or one time in 1200 blue, kept for good.
    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        let this = read(breeding.this).variant;
        let partner = read(breeding.partner).variant;
        let variant = offspring_variant(breeding.draws, this, partner);
        let mut child = Mob::fresh();
        update(&mut child, |axolotl| axolotl.variant = variant);
        child.body.flags.persistent = true;
        Some(child)
    }

    fn actually_hurt(&self, mob: &mut Mob, _: u32) {
        animal::actually_hurt(mob);
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        animal::dimensions(mob, baby_size())
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    /// `removeWhenFarAway`: unless it came from a bucket (no axolotl has a name).
    fn remove_when_far_away(&self, _: u16, mob: &Mob, _: u32, _: f64) -> bool {
        !read(mob).from_bucket
    }

    /// Not while it plays dead.
    fn play_ambient_sound(&self, tick: &mut MobTick<'_>) {
        if !read(&tick.mob).playing_dead {
            crate::mobs::body::play_ambient_sound(tick);
        }
    }

    /// Out of water, as a fresh axolotl's `isInWater` is.
    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_AXOLOTL_IDLE_AIR)
    }

    fn ambient_sound_at(&self, _: &Mob, in_water: bool, _: bool) -> Option<i32> {
        Some(if in_water {
            sounds::ENTITY_AXOLOTL_IDLE_WATER
        } else {
            sounds::ENTITY_AXOLOTL_IDLE_AIR
        })
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_AXOLOTL_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_AXOLOTL_DEATH)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let own = read(mob);
        let mut values = animal::shown(mob);
        let variant = variant_by_id(own.variant);
        values.push((VARIANT_INDEX, DataShown::Int(variant), variant == LUCY));
        values.push((
            PLAYING_DEAD_INDEX,
            DataShown::Boolean(own.playing_dead),
            !own.playing_dead,
        ));
        values.push((
            FROM_BUCKET_INDEX,
            DataShown::Boolean(own.from_bucket),
            !own.from_bucket,
        ));
        values
    }
}
