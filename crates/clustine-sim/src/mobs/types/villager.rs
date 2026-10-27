//! The villager as a body (`Villager`, `AbstractVillager`; `docs/groundwork/n7k-brief.md`
//! section 2 and `n7k-plan.md` section 10, "The villager"), part K2, and its brain
//! (`docs/groundwork/n10-brief.md` section 1, cited as V; `n10-plan.md` section 5), part
//! V1 of N10: an ageable mob of a type, a profession and a level (`VillagerData`), its
//! type settled by the biome at its feet unless a saved one settled it, which keeps food
//! and its profession's wants in its eight slots.
//!
//! Its mind is a brain ([`PROVIDER`], `Villager.BRAIN_PROVIDER`): the game's nine sensors
//! (the bed's, the babies', the secondary points' and the golem's are
//! `mobs/brain/sensors_villager.rs`), and its ten activities with their packages as
//! `VillagerGoalPackages` makes them (V 1.4, D10 B section 9), its schedule
//! (`villager_schedule`'s activities by the overworld's clock) reaching work where it has
//! a job site, the meeting where it has a meeting point, and a baby's play. The packages
//! name the behaviours of work and points of interest (`behaviours/work.rs`, part V2) and
//! of its social life (`behaviours/social_villagers.rs`, part V3) by their constructors;
//! its own walks, sleep and play are `behaviours/rest.rs` and `behaviours/villagers.rs`.
//! A villager sleeps in its bed (`startSleeping`, the bed's `occupied`), gets up at dawn,
//! when its bed is gone or when a player uses the bed (`region/sleep.rs`).
//!
//! Its state ([`VillagerState`]) follows what every ageable mob keeps
//! ([`animal::Animal`]), so that the golden dandelion and a baby's growing are the
//! animals'. What its neighbours read of it ([`Shown`]) is written from its brain at the
//! end of each of its ticks, so that a copy of it in another region shows it too.
//!
//! **Across a line**: what it sees and runs from is the region's land and copies; what
//! hurt it and walked over a line is kept, as every brain's; its type and data are in
//! its record, which copies carry.
//!
//! Named differences: the profession its core and work packages are made for is read
//! at each tick, where the game's is the one of the brain's last making, which differs
//! only between a profession's change and the making of the brain again in the same
//! tick; a baby's brain has the work activity, and an adult's the play, which their
//! schedules never ask; the schedule's activity is set at its first tick, not at its
//! construction; a load of a template's tag draws nothing for the brain it makes again
//! (`refreshBrain`), as the hook has no stream; the template's `Inventory`, `Gossips`
//! and `Xp` are not read; raids are none until N10d, so the pre-raid, raid and hide
//! activities are never reached but by a bell (part V3); a player's use of a bed it
//! sleeps in wakes it at its next tick, which is this tick's where the use comes first.

use clustine_blocks::data::item_in_tag;
use clustine_data::villagers::{self, professions, types as kinds};
use clustine_data::{MobCategory, PathType, entity_types, items, sounds};
use clustine_world::{EntityData, EntityId, ItemStack};
use serde::{Deserialize, Serialize};

use super::animal::{self, Animal, Own};
use super::{
    AgeableGroup, Breeding, Dimensions, Interaction, MobType, SpawnGroup, SpawnReason, Spawning,
    Used, template,
};
use crate::mobs::Mob;
use crate::mobs::ai::Setup;
use crate::mobs::body::hurt::{DamageSource, Struck};
use crate::mobs::body::{Event, ladder};
use crate::mobs::brain::behaviours::fleeing::away_from;
use crate::mobs::brain::behaviours::items::{GoToWantedItem, go_to_wanted_item};
use crate::mobs::brain::behaviours::looking::{LookAt, look_at};
use crate::mobs::brain::behaviours::rest::{
    self, go_to_closest_village, inside_brownian_walk, move_to_sky_seeing_spot,
    set_closest_home_as_walk_target, sleep_in_bed, village_bound_random_stroll,
    village_bound_random_stroll_within, wake_up,
};
use crate::mobs::brain::behaviours::social_villagers::{self as social};
use crate::mobs::brain::behaviours::villagers::{
    calm_down, from_schedule, jump_on_bed, panic_trigger, play_tag_with_other_kids,
    reset_raid_status, set_raid_status,
};
use crate::mobs::brain::behaviours::walking::{
    MoveToTargetSink, look_sink, sink, walk_to_look_target,
};
use crate::mobs::brain::behaviours::{doors, panic, trigger_if, work};
use crate::mobs::brain::sensors::{self, NearestItemSensor, VILLAGER_HOSTILES};
use crate::mobs::brain::sensors_villager;
use crate::mobs::brain::{
    self, Activity, ActivityData, Behaviour, Cx, DoNothing, Gate, HeldTimed, MemoryKind, Order,
    Provider, Running, Status, Value,
};
use crate::mobs::tick::{Local, MobTick};
use crate::physics::aabb::Aabb;
use crate::physics::item::max_stack_size;
use crate::region::Random;
use crate::shown::DataShown;

/// The synced data's indices: `AbstractVillager.DATA_UNHAPPY_COUNTER`, then
/// `DATA_VILLAGER_DATA` and `DATA_VILLAGER_DATA_FINALIZED` (after `AgeableMob`'s 16 and
/// 17).
pub(crate) const UNHAPPY_INDEX: u8 = 18;
pub(crate) const DATA_INDEX: u8 = 19;
pub(crate) const FINALIZED_INDEX: u8 = 20;

/// `setUnhappy()`'s 40 ticks of a shaking head.
pub(crate) const UNHAPPY_TICKS: i32 = 40;

/// `Villager.BABY_DIMENSIONS`: `scalable(0.49, 0.98).withEyeHeight(0.63)`.
pub(crate) const BABY: Dimensions = Dimensions {
    width: 0.49,
    height: 0.98,
    eye_height: 0.63,
};

/// `Villager.SPEED_MODIFIER`, the speed every package of it is made with.
const SPEED: f32 = 0.5;

/// The eight slots of `AbstractVillager.inventory` (`SimpleContainer(8)`).
pub(crate) const SLOTS: usize = 8;

/// `Container.getMaxStackSize()`: a slot of a `SimpleContainer` holds 99 at most.
const CONTAINER_MOST: i32 = 99;

/// `VillagerData`: the type and profession by their registry ids
/// ([`clustine_data::villagers`]) and the level, 1 to 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Data {
    pub(crate) kind: u8,
    pub(crate) profession: u8,
    pub(crate) level: i32,
}

impl Default for Data {
    /// `Villager.createDefaultVillagerData()`: plains, none, 1.
    fn default() -> Self {
        Self {
            kind: kinds::PLAINS as u8,
            profession: professions::NONE as u8,
            level: 1,
        }
    }
}

/// What a villager keeps beyond an ageable mob: its data and whether it is settled, the
/// unhappy counter (synced, counted down each tick), its eight slots, what the game
/// saves of trading and gossip (kept as read, never changed: no trades), and its brain's
/// last schedule update (`Brain.lastScheduleUpdate`, none before its first).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct VillagerState {
    pub(crate) data: Data,
    pub(crate) finalized: bool,
    pub(crate) unhappy: i32,
    pub(crate) inventory: [Option<ItemStack>; SLOTS],
    pub(crate) food_level: i8,
    pub(crate) xp: i32,
    pub(crate) last_restock: i64,
    pub(crate) last_gossip_decay: i64,
    pub(crate) restocks_today: i32,
    pub(crate) schedule_at: Option<i64>,
    /// Its offers, drawn when first asked (`AbstractVillager.offers`, none before;
    /// `docs/groundwork/n10-plan.md`, section 4).
    pub(crate) offers: Option<clustine_world::trading::Offers>,
    pub(crate) gossips: super::gossip::Gossips,
    /// The player it trades with (`tradingPlayer`), whose merchant menu on it is open;
    /// cleared at the menu's every close and at every take-in where that player is not
    /// the region's with that menu (section 8.2).
    pub(crate) trading_player: Option<clustine_world::PlayerId>,
    /// `lastGossipTime` and `lastRestockCheckDay`, which the game does not save and
    /// Clustine keeps (decision 14).
    pub(crate) last_gossip_time: Option<i64>,
    pub(crate) last_restock_check_day: i64,
    /// What other villagers read of it across a line, written from its brain at the end
    /// of each of its ticks (decision 3): copies carry the type's state, not the mind.
    pub(crate) shown: Shown,
    /// The player of its last trade (`lastTradedPlayer`), set by the trade (part T2,
    /// `region/trading.rs`) and let go after its next brain by the trade's reputation
    /// event (part V3's `after_brain`).
    pub(crate) last_traded_player: Option<clustine_world::PlayerId>,
    /// The head of the bed it sleeps in (`LivingEntity.getSleepingPos`), none awake.
    pub(crate) sleeping: Option<clustine_world::BlockPos>,
    /// A player used the bed it sleeps in (`kickVillagerOutOfBed`): it gets up at its
    /// next tick (`region/sleep.rs`).
    pub(crate) kicked: bool,
    /// What a baby's bed sensor keeps between its scans (`NearestBedSensor`'s
    /// `lastUpdate` and its cache of beds tried).
    pub(crate) bed_search: BedSearch,
}

/// `NearestBedSensor`'s own fields, which the game keeps in the sensor of each brain.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct BedSearch {
    pub(crate) last_update: i64,
    pub(crate) cache: rest::BedCache,
}

/// What a villager's neighbours read of it, kept in its type's state so that a copy of it
/// in another region carries it (`docs/groundwork/n10-plan.md`, decision 3): its job site
/// and the one it reserved, when it last slept, whether a golem was seen lately, whom it
/// interacts with, its food points, whether it sleeps, and the reputations at or below
/// -100 a golem's defence reads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Shown {
    pub(crate) job_site: Option<clustine_world::BlockPos>,
    pub(crate) potential_job_site: Option<clustine_world::BlockPos>,
    pub(crate) last_slept: Option<i64>,
    pub(crate) golem_detected: bool,
    pub(crate) interaction_target: Option<EntityId>,
    pub(crate) food_points: i32,
    pub(crate) sleeping: bool,
    pub(crate) bad_reputations: Vec<(uuid::Uuid, i32)>,
}

impl Default for VillagerState {
    fn default() -> Self {
        Self {
            data: Data::default(),
            finalized: false,
            unhappy: 0,
            inventory: [const { None }; SLOTS],
            food_level: 0,
            xp: 0,
            last_restock: 0,
            last_gossip_decay: 0,
            restocks_today: 0,
            schedule_at: None,
            offers: None,
            gossips: super::gossip::Gossips::default(),
            trading_player: None,
            last_gossip_time: None,
            last_restock_check_day: 0,
            shown: Shown::default(),
            last_traded_player: None,
            sleeping: None,
            kicked: false,
            bed_search: BedSearch::default(),
        }
    }
}

impl Own for VillagerState {}

/// A villager's ageable part and its own state.
pub(crate) fn state_of(mob: &Mob) -> (Animal, VillagerState) {
    animal::read::<VillagerState>(&mob.state)
}

/// Changes a villager's own state.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut VillagerState)) {
    let (shared, mut own) = state_of(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        mob.state = animal::write(&shared, &own);
    }
}

/// `finalizeVillagerType(level, pos)` (`VillagerDataHolder`): unless settled, its type
/// the one of the biome named `biome` (plains where the chunk is not read), and settled.
/// No draw.
pub(crate) fn finalize_type(mob: &mut Mob, biome: Option<&str>) {
    update(mob, |own| {
        if !own.finalized {
            own.data.kind = type_by_biome(biome);
            own.finalized = true;
        }
    });
}

/// `VillagerType.byBiome` of a biome by its name, plains for none.
fn type_by_biome(biome: Option<&str>) -> u8 {
    villagers::type_by_biome_name(biome.unwrap_or("minecraft:plains")) as u8
}

/// The `villager_food` component's nutrition of an item (the extracted item components:
/// bread 4; carrot, potato and beetroot 1), none for an item without it.
pub(crate) fn villager_food(item: i32) -> Option<i32> {
    match item {
        items::BREAD => Some(4),
        items::CARROT | items::POTATO | items::BEETROOT => Some(1),
        _ => None,
    }
}

/// `VillagerProfession.requestedItems()`: the farmer's wheat, seeds and bone meal
/// (VillagerProfession.txt 331–377), nothing for every other.
fn requested(profession: u8, item: i32) -> bool {
    i32::from(profession) == professions::FARMER
        && matches!(
            item,
            items::WHEAT | items::WHEAT_SEEDS | items::BEETROOT_SEEDS | items::BONE_MEAL
        )
}

/// `SimpleContainer.canAddItem(stack)`: an empty slot, or one of the same item with the
/// same components not full (the item's own most).
pub(crate) fn can_add_item(inventory: &[Option<ItemStack>; SLOTS], stack: ItemStack) -> bool {
    inventory.iter().any(|slot| match slot {
        None => true,
        Some(held) => {
            held.count <= 0 || (held.same_kind(&stack) && held.count < max_stack_size(held))
        }
    })
}

/// `SimpleContainer.addItem(stack)`: into the slots of the same item with the same
/// components first, each up to
/// the lesser of 99 and the item's most, then the rest into the first empty slot. How
/// many it took.
pub(crate) fn add_item(inventory: &mut [Option<ItemStack>; SLOTS], stack: ItemStack) -> i32 {
    let most = max_stack_size(&stack).min(CONTAINER_MOST);
    let mut left = stack.count;
    for held in inventory.iter_mut().flatten() {
        if left <= 0 {
            break;
        }
        if !held.same_kind(&stack) || held.count <= 0 {
            continue;
        }
        let moved = left.min(most - held.count);
        if moved > 0 {
            held.count += moved;
            left -= moved;
        }
    }
    if left > 0
        && let Some(slot) = inventory
            .iter_mut()
            .find(|slot| slot.as_ref().is_none_or(|held| held.count <= 0))
    {
        *slot = Some(ItemStack {
            count: left,
            ..stack
        });
        left = 0;
    }
    stack.count - left
}

/// `Villager.wantsToPickUp(level, stack)`: of `#villager_picks_up`, villager food, or
/// what its profession asks for, and room for it.
pub(crate) fn wants_to_pick_up(mob: &Mob, stack: ItemStack) -> bool {
    let (_, own) = state_of(mob);
    (item_in_tag(stack.item, "minecraft:villager_picks_up")
        || villager_food(stack.item).is_some()
        || requested(own.data.profession, stack.item))
        && can_add_item(&own.inventory, stack)
}

/// The activity `villager_schedule` gives at the overworld clock's `day` for a baby or
/// an adult (`timeline/villager_schedule.json`, its tracks `baby_villager_activity` and
/// `villager_activity`): each keyframe's from its tick of the day until the next, the
/// last's until the first's of the next day (**Inferred**: a step track, as the
/// timeline's activities cannot be eased).
pub(crate) fn scheduled(day: u64, baby: bool) -> Activity {
    const ADULT: [(u64, Activity); 5] = [
        (10, Activity::Idle),
        (2000, Activity::Work),
        (9000, Activity::Meet),
        (11_000, Activity::Idle),
        (12_000, Activity::Rest),
    ];
    const BABY_DAY: [(u64, Activity); 5] = [
        (10, Activity::Idle),
        (3000, Activity::Play),
        (6000, Activity::Idle),
        (10_000, Activity::Play),
        (12_000, Activity::Rest),
    ];
    let frames = if baby { &BABY_DAY } else { &ADULT };
    let at = day % crate::time::DAY;
    frames
        .iter()
        .rev()
        .find(|(from, _)| at >= *from)
        .unwrap_or(&frames[frames.len() - 1])
        .1
}

/// What `setActiveActivityIfPossible` makes of a scheduled activity for a brain whose
/// memories are `memories`: work only with a job site and the meeting only with a
/// meeting point (their activities' conditions); idle, the default, otherwise.
fn possible(activity: Activity, memories: &brain::Memories) -> Activity {
    let met = match activity {
        Activity::Work => memories.check(MemoryKind::JobSite, Status::Present),
        Activity::Meet => memories.check(MemoryKind::MeetingPoint, Status::Present),
        _ => true,
    };
    if met { activity } else { Activity::Idle }
}

/// `brain.updateActivityFromSchedule(attributes, gameTime, position)`: more than 20 game
/// ticks after the last, the schedule's activity asked for where it is not active.
pub(crate) fn update_from_schedule(cx: &mut Cx<'_, '_, '_>) {
    let time = cx.time;
    let day = cx.ai.tick.world.day();
    let mob = &mut cx.ai.tick.mob;
    let (shared, mut own) = state_of(mob);
    if time - own.schedule_at.unwrap_or(-9999) <= 20 {
        return;
    }
    own.schedule_at = Some(time);
    mob.state = animal::write(&shared, &own);
    let activity = scheduled(day, shared.is_baby());
    if !cx.memories.active.contains(&activity) {
        cx.memories.requested.push(activity);
    }
}

/// `registerBrainGoals`' `updateActivityFromSchedule` of a brain just made: its
/// activity the schedule's, where it may be, and the time of it kept.
fn schedule_now(tick: &mut MobTick<'_>) {
    let time = tick.world.game_time();
    let day = tick.world.day();
    let baby = animal::is_baby(&tick.mob);
    update(&mut tick.mob, |own| own.schedule_at = Some(time));
    let memories = brain::memories_of(&PROVIDER, &tick.mob, tick.lived);
    brain::set_recorded_activity(&mut tick.mob, possible(scheduled(day, baby), &memories));
}

/// `NEAREST_ITEMS`: what it wants to pick up.
static NEAREST_ITEMS: NearestItemSensor = NearestItemSensor {
    wants: |ai, stack| wants_to_pick_up(&ai.tick.mob, stack),
};

/// `Villager.BRAIN_PROVIDER` (Villager.txt 2313–2343): its nine sensors in the game's
/// order and its activities. The memories of its points of interest
/// (`Villager.POI_MEMORIES`) are named here too, so that they are kept whichever of the
/// behaviours that name them are built.
pub(crate) static PROVIDER: Provider = Provider {
    memories: &[
        MemoryKind::Home,
        MemoryKind::JobSite,
        MemoryKind::PotentialJobSite,
        MemoryKind::MeetingPoint,
    ],
    sensors: &[
        &sensors::NEAREST_LIVING_ENTITIES,
        &sensors::NEAREST_PLAYERS,
        &NEAREST_ITEMS,
        &sensors_villager::NEAREST_BED,
        &sensors::HURT_BY,
        &VILLAGER_HOSTILES,
        &sensors_villager::VILLAGER_BABIES,
        &sensors_villager::SECONDARY_POIS,
        &sensors_villager::GOLEM_DETECTED,
    ],
    activities,
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// `canPickUpLoot()` of the villager its brain runs.
fn can_pick_up(cx: &Cx<'_, '_, '_>) -> bool {
    cx.ai.tick.mob.body.flags.picks_up_loot
}

/// The profession of the villager whose brain runs, by its registry id.
fn profession(cx: &Cx<'_, '_, '_>) -> u8 {
    state_of(&cx.ai.tick.mob).1.data.profession
}

/// A behaviour its package makes for the villager's profession (`getCorePackage(
/// profession, …)`, `getWorkPackage`): one made for each profession it may have, the one
/// of the profession it has now asked to start (see the module's comment). Each keeps its
/// own state; at most one runs, since a profession changes only as the brain is made
/// again.
struct ByProfession {
    options: Vec<Box<dyn Behaviour>>,
    /// The option of the villager's profession now.
    pick: fn(&Cx<'_, '_, '_>) -> usize,
}

impl Behaviour for ByProfession {
    fn required(&self, into: &mut std::collections::BTreeSet<MemoryKind>) {
        for option in &self.options {
            option.required(into);
        }
    }

    fn running(&self) -> bool {
        self.options.iter().any(|option| option.running())
    }

    fn try_start(&mut self, cx: &mut Cx<'_, '_, '_>) -> bool {
        let at = (self.pick)(cx).min(self.options.len() - 1);
        self.options[at].try_start(cx)
    }

    fn tick_or_stop(&mut self, cx: &mut Cx<'_, '_, '_>) {
        for option in &mut self.options {
            if option.running() {
                option.tick_or_stop(cx);
            }
        }
    }

    fn do_stop(&mut self, cx: &mut Cx<'_, '_, '_>) {
        for option in &mut self.options {
            if option.running() {
                option.do_stop(cx);
            }
        }
    }

    fn count(&self) -> usize {
        self.options.iter().map(|option| option.count()).sum()
    }

    fn load(&mut self, first: u16, parts: &std::collections::BTreeMap<u16, clustine_world::Entry>) {
        let mut next = first;
        for option in &mut self.options {
            option.load(next, parts);
            next += option.count() as u16;
        }
    }

    fn save(&self, first: u16, into: &mut Vec<clustine_world::Entry>) {
        let mut next = first;
        for option in &self.options {
            option.save(next, into);
            next += option.count() as u16;
        }
    }

    fn left(&mut self, now: u32, time: i64) {
        for option in &mut self.options {
            option.left(now, time);
        }
    }
}

/// `ValidateNearbyPoi.create(profession.heldJobSite(), JOB_SITE)` (core 0) for the
/// profession it has, among one for each of the fifteen.
fn validate_job_site() -> Box<dyn Behaviour> {
    let count = villagers::PROFESSION_NAMES.len();
    Box::new(ByProfession {
        options: (0..count)
            .map(|profession| work::validate_job_site(profession as u8))
            .collect(),
        pick: |cx| usize::from(profession(cx)),
    })
}

/// `getMinimalLookBehavior()` (VillagerGoalPackages.txt 884–914): 5, a `RunOne` of a
/// look at a villager or a player within 8, or nothing for 30 to 60 ticks.
fn minimal_look() -> (i32, Box<dyn Behaviour>) {
    let villager = entity_types::VILLAGER as u16;
    (
        5,
        Box::new(Gate::run_one(vec![
            (look_at(LookAt::Kind(villager), 8.0), 2),
            (look_at(LookAt::Player, 8.0), 2),
            (Box::new(HeldTimed::new(DoNothing { min: 30, max: 60 })), 8),
        ])),
    )
}

/// `getFullLookBehavior()` (811–883): 5, a `RunOne` of a look at a cat, a villager, a
/// player, or a mob of six categories within 8, or nothing.
fn full_look() -> (i32, Box<dyn Behaviour>) {
    let kind = |id: i32| LookAt::Kind(id as u16);
    let mut children: Vec<(Box<dyn Behaviour>, u32)> = vec![
        (look_at(kind(entity_types::CAT), 8.0), 8),
        (look_at(kind(entity_types::VILLAGER), 8.0), 2),
        (look_at(LookAt::Player, 8.0), 2),
    ];
    for category in [
        MobCategory::Creature,
        MobCategory::WaterCreature,
        MobCategory::Axolotls,
        MobCategory::UndergroundWaterCreature,
        MobCategory::WaterAmbient,
        MobCategory::Monster,
    ] {
        children.push((look_at(LookAt::Category(category), 8.0), 1));
    }
    children.push((Box::new(HeldTimed::new(DoNothing { min: 30, max: 60 })), 2));
    (5, Box::new(Gate::run_one(children)))
}

/// `DoNothing(min, max)`, boxed.
fn do_nothing(min: i64, max: i64) -> Box<dyn Behaviour> {
    Box::new(HeldTimed::new(DoNothing { min, max }))
}

/// A gate of one child, in order, that erases `memory` as it stops: the trade between
/// villagers' (`interaction_target`) and love's (`breed_target`) in the idle and meeting
/// packages (an ordered gate draws nothing).
fn erasing_gate(memory: MemoryKind, child: Box<dyn Behaviour>) -> Box<dyn Behaviour> {
    Box::new(Gate::new(
        Vec::new(),
        vec![memory],
        Order::Ordered,
        Running::RunOne,
        vec![(child, 1)],
    ))
}

/// `TriggerGate.triggerOneShuffled(children)`: its children shuffled at each try (one
/// draw a child) and the first that fires.
fn trigger_one_shuffled(children: Vec<(Box<dyn Behaviour>, u32)>) -> Box<dyn Behaviour> {
    Box::new(Gate::run_one(children))
}

/// `VillagerGoalPackages.raidExistsAndNotVictory` (D10 B section 9, #8): a raid near
/// that is won. None is kept until N10d.
fn raid_won(_: &mut Cx<'_, '_, '_>) -> bool {
    false
}

/// `raidExistsAndActive` (#9): a raid near, active, neither won nor lost. None is kept
/// until N10d.
fn raid_active(_: &mut Cx<'_, '_, '_>) -> bool {
    false
}

/// The core package (`getCorePackage(profession, 0.5)`, VillagerGoalPackages.txt 17–180).
fn core_package() -> ActivityData {
    ActivityData {
        activity: Activity::Core,
        behaviours: vec![
            (0, panic::swim(0.8)),
            (0, doors::interact_with_door()),
            (0, look_sink(45, 90)),
            (0, panic_trigger()),
            (0, wake_up()),
            (0, social::react_to_bell()),
            (0, set_raid_status()),
            (0, validate_job_site()),
            (0, work::validate_potential_job_site()),
            (1, sink()),
            (2, work::poi_competitor_scan()),
            (3, social::look_and_follow_trading_player(SPEED)),
            (
                5,
                go_to_wanted_item(GoToWantedItem {
                    may: |_| true,
                    speed: SPEED,
                    override_walk: false,
                    range: 4,
                    can_pick_up,
                }),
            ),
            (6, work::acquire_job_site()),
            (7, work::go_to_potential_job_site(SPEED)),
            (8, work::yield_job_site(SPEED)),
            (10, work::acquire_home()),
            (10, work::acquire_meeting_point()),
            (10, work::assign_profession_from_job_site()),
            (10, work::reset_profession()),
        ],
        conditions: Vec::new(),
        erase: Vec::new(),
    }
}

/// The `RunOne` of the work package for a farmer or for any other profession (the
/// farmer's composter, and its weights of harvest and bone meal).
fn work_run_one(farmer: bool) -> Box<dyn Behaviour> {
    let work_at = if farmer {
        work::work_at_composter()
    } else {
        work::work_at_poi()
    };
    Box::new(Gate::run_one(vec![
        (work_at, 7),
        (work::stroll_around_poi(MemoryKind::JobSite, 0.4, 4), 2),
        (work::stroll_to_poi(MemoryKind::JobSite, 0.4, 1, 10), 5),
        (
            work::stroll_to_poi_list(
                MemoryKind::SecondaryJobSite,
                SPEED,
                1,
                6,
                MemoryKind::JobSite,
            ),
            5,
        ),
        (work::harvest_farmland(), if farmer { 2 } else { 5 }),
        (work::use_bonemeal(), if farmer { 4 } else { 7 }),
    ]))
}

/// The work package (`getWorkPackage(profession, 0.5)`, 202–313), active with a job site.
fn work_package() -> ActivityData {
    let player = entity_types::PLAYER as u16;
    ActivityData {
        activity: Activity::Work,
        behaviours: vec![
            minimal_look(),
            (
                5,
                Box::new(ByProfession {
                    options: vec![work_run_one(true), work_run_one(false)],
                    pick: |cx| usize::from(i32::from(profession(cx)) != professions::FARMER),
                }),
            ),
            (10, social::show_trades_to_player(400, 1600)),
            (10, social::set_look_and_interact(player, 4)),
            (
                2,
                work::set_walk_target_from_block_memory(MemoryKind::JobSite, SPEED, 9, 100, 1200),
            ),
            (3, social::give_gift_to_hero(100)),
            (99, from_schedule(update_from_schedule)),
        ],
        conditions: vec![(MemoryKind::JobSite, Status::Present)],
        erase: Vec::new(),
    }
}

/// The play package (`getPlayPackage(0.5)`, 314–389), a baby's.
fn play_package() -> ActivityData {
    let (villager, cat) = (entity_types::VILLAGER as u16, entity_types::CAT as u16);
    ActivityData {
        activity: Activity::Play,
        behaviours: vec![
            (0, Box::new(MoveToTargetSink::new(80, 120))),
            full_look(),
            (5, play_tag_with_other_kids()),
            (
                5,
                Box::new(Gate::new(
                    vec![(MemoryKind::VisibleVillagerBabies, Status::Absent)],
                    Vec::new(),
                    Order::Shuffled,
                    Running::RunOne,
                    vec![
                        (
                            social::interact_with(
                                villager,
                                8,
                                MemoryKind::InteractionTarget,
                                SPEED,
                                2,
                            ),
                            2,
                        ),
                        (
                            social::interact_with(cat, 8, MemoryKind::InteractionTarget, SPEED, 2),
                            1,
                        ),
                        (village_bound_random_stroll(SPEED), 1),
                        (walk_to_look_target(SPEED, 2), 1),
                        (jump_on_bed(SPEED), 2),
                        (do_nothing(20, 40), 2),
                    ],
                )),
            ),
            (99, from_schedule(update_from_schedule)),
        ],
        conditions: Vec::new(),
        erase: Vec::new(),
    }
}

/// The meeting package (`getMeetPackage(0.5)`, 455–534), active with a meeting point.
fn meet_package() -> ActivityData {
    let player = entity_types::PLAYER as u16;
    ActivityData {
        activity: Activity::Meet,
        behaviours: vec![
            (
                2,
                trigger_one_shuffled(vec![
                    (
                        work::stroll_around_poi(MemoryKind::MeetingPoint, 0.4, 40),
                        2,
                    ),
                    (social::socialize_at_bell(), 2),
                ]),
            ),
            (10, social::show_trades_to_player(400, 1600)),
            (10, social::set_look_and_interact(player, 4)),
            (
                2,
                work::set_walk_target_from_block_memory(
                    MemoryKind::MeetingPoint,
                    SPEED,
                    6,
                    100,
                    200,
                ),
            ),
            (3, social::give_gift_to_hero(100)),
            (3, work::validate_meeting_point()),
            (
                3,
                erasing_gate(MemoryKind::InteractionTarget, social::trade_with_villager()),
            ),
            full_look(),
            (99, from_schedule(update_from_schedule)),
        ],
        conditions: vec![(MemoryKind::MeetingPoint, Status::Present)],
        erase: Vec::new(),
    }
}

/// The rest package (`getRestPackage(0.5)`, 390–454).
fn rest_package() -> ActivityData {
    ActivityData {
        activity: Activity::Rest,
        behaviours: vec![
            (
                2,
                work::set_walk_target_from_block_memory(MemoryKind::Home, SPEED, 1, 150, 1200),
            ),
            (3, work::validate_home()),
            (3, sleep_in_bed()),
            (
                5,
                Box::new(Gate::new(
                    vec![(MemoryKind::Home, Status::Absent)],
                    Vec::new(),
                    Order::Shuffled,
                    Running::RunOne,
                    vec![
                        (set_closest_home_as_walk_target(SPEED), 1),
                        (inside_brownian_walk(SPEED), 4),
                        (go_to_closest_village(SPEED, 4), 2),
                        (do_nothing(20, 40), 2),
                    ],
                )),
            ),
            minimal_look(),
            (99, from_schedule(update_from_schedule)),
        ],
        conditions: Vec::new(),
        erase: Vec::new(),
    }
}

/// The idle package (`getIdlePackage(0.5)`, 535–663).
fn idle_package() -> ActivityData {
    let (villager, cat, player) = (
        entity_types::VILLAGER as u16,
        entity_types::CAT as u16,
        entity_types::PLAYER as u16,
    );
    ActivityData {
        activity: Activity::Idle,
        behaviours: vec![
            (
                2,
                Box::new(Gate::run_one(vec![
                    (
                        social::interact_with(villager, 8, MemoryKind::InteractionTarget, SPEED, 2),
                        2,
                    ),
                    (social::interact_with_breedable(8, SPEED, 2), 1),
                    (
                        social::interact_with(cat, 8, MemoryKind::InteractionTarget, SPEED, 2),
                        1,
                    ),
                    (village_bound_random_stroll(SPEED), 1),
                    (walk_to_look_target(SPEED, 2), 1),
                    (jump_on_bed(SPEED), 1),
                    (do_nothing(30, 60), 1),
                ])),
            ),
            (3, social::give_gift_to_hero(100)),
            (3, social::set_look_and_interact(player, 4)),
            (3, social::show_trades_to_player(400, 1600)),
            (
                3,
                erasing_gate(MemoryKind::InteractionTarget, social::trade_with_villager()),
            ),
            (
                3,
                erasing_gate(MemoryKind::BreedTarget, social::villager_make_love()),
            ),
            full_look(),
            (99, from_schedule(update_from_schedule)),
        ],
        conditions: Vec::new(),
        erase: Vec::new(),
    }
}

/// The panic package (`getPanicPackage(0.5)`, 664–700): its speed half as much again.
fn panic_package() -> ActivityData {
    let fleeing = SPEED * 1.5;
    ActivityData {
        activity: Activity::Panic,
        behaviours: vec![
            (0, calm_down(update_from_schedule)),
            (1, away_from(MemoryKind::NearestHostile, fleeing, 6, false)),
            (1, away_from(MemoryKind::HurtByEntity, fleeing, 6, false)),
            (3, village_bound_random_stroll_within(fleeing, 2, 2)),
            minimal_look(),
        ],
        conditions: Vec::new(),
        erase: Vec::new(),
    }
}

/// The pre-raid package (`getPreRaidPackage(0.5)`, 701–737).
fn pre_raid_package() -> ActivityData {
    let hurried = SPEED * 1.5;
    ActivityData {
        activity: Activity::PreRaid,
        behaviours: vec![
            (0, social::ring_bell()),
            (
                0,
                trigger_one_shuffled(vec![
                    (
                        work::set_walk_target_from_block_memory(
                            MemoryKind::MeetingPoint,
                            hurried,
                            2,
                            150,
                            200,
                        ),
                        6,
                    ),
                    (village_bound_random_stroll(hurried), 2),
                ]),
            ),
            minimal_look(),
            (99, reset_raid_status(update_from_schedule)),
        ],
        conditions: Vec::new(),
        erase: Vec::new(),
    }
}

/// The raid package (`getRaidPackage(0.5)`, 738–787; its predicates D10 B section 9).
fn raid_package() -> ActivityData {
    ActivityData {
        activity: Activity::Raid,
        behaviours: vec![
            (
                0,
                trigger_if(
                    raid_won,
                    trigger_one_shuffled(vec![
                        (move_to_sky_seeing_spot(SPEED), 5),
                        (village_bound_random_stroll(SPEED * 1.1), 2),
                    ]),
                ),
            ),
            (0, social::celebrate_villagers_survived_raid(600, 600)),
            (
                2,
                trigger_if(raid_active, social::locate_hiding_place(24, SPEED * 1.4, 1)),
            ),
            minimal_look(),
            (99, reset_raid_status(update_from_schedule)),
        ],
        conditions: Vec::new(),
        erase: Vec::new(),
    }
}

/// The hiding package (`getHidePackage(0.5)`, 788–810).
fn hide_package() -> ActivityData {
    ActivityData {
        activity: Activity::Hide,
        behaviours: vec![
            (0, social::set_hidden_state(15, 3)),
            (1, social::locate_hiding_place(32, SPEED * 1.25, 2)),
            minimal_look(),
        ],
        conditions: Vec::new(),
        erase: Vec::new(),
    }
}

/// The villager's activities (`lambda$static$0`, Villager.txt 2216–2311), in the order
/// the game adds them, which orders those of one priority and one bucket (B 7.1.1.6): a
/// baby's play or an adult's work first (both here, see the module's comment), then the
/// core, the meeting, rest, idle, panic, pre-raid, raid and hiding. None erases anything
/// as it stops.
fn activities() -> Vec<ActivityData> {
    vec![
        play_package(),
        work_package(),
        core_package(),
        meet_package(),
        rest_package(),
        idle_package(),
        panic_package(),
        pre_raid_package(),
        raid_package(),
        hide_package(),
    ]
}

/// What its neighbours read of it, from its state and its brain's memories as its tick
/// leaves them (decision 3).
fn shown_now(mob: &Mob, lived: u32) -> Shown {
    let (_, own) = state_of(mob);
    let memories = brain::memories_of(&PROVIDER, mob, lived.saturating_add(1));
    let block = |kind: MemoryKind| match memories.get(kind) {
        Some(Value::Block(pos)) => Some(pos),
        _ => None,
    };
    let last_slept = match memories.get(MemoryKind::LastSlept) {
        Some(Value::Long(time)) => Some(time),
        _ => None,
    };
    let interaction_target = match memories.get(MemoryKind::InteractionTarget) {
        Some(Value::Entity(key)) => Some(key),
        _ => None,
    };
    // `countFoodPointsInInventory`: each slot's count by its food's nutrition.
    let food_points = own
        .inventory
        .iter()
        .flatten()
        .filter_map(|stack| villager_food(stack.item).map(|food| food * stack.count))
        .sum();
    // The reputations of at most −100 (`getPlayerReputation`), each target once.
    let mut bad_reputations: Vec<(uuid::Uuid, i32)> = Vec::new();
    for (target, _, _) in &own.gossips.entries {
        if bad_reputations
            .last()
            .is_some_and(|(seen, _)| seen == target)
        {
            continue;
        }
        let reputation = own.gossips.reputation(*target);
        if reputation <= -100 {
            bad_reputations.push((*target, reputation));
        }
    }
    Shown {
        job_site: block(MemoryKind::JobSite),
        potential_job_site: block(MemoryKind::PotentialJobSite),
        last_slept,
        golem_detected: memories.check(MemoryKind::GolemDetectedRecently, Status::Present),
        interaction_target,
        food_points,
        sleeping: own.sleeping.is_some(),
        bad_reputations,
    }
}

/// The sleeper's size (`LivingEntity.SLEEPING_DIMENSIONS`).
const SLEEPING: Dimensions = Dimensions {
    width: 0.2,
    height: 0.2,
    eye_height: 0.2,
};

/// The synced indices of `Entity.DATA_POSE` and `LivingEntity.SLEEPING_POS_ID`, and the
/// pose's ordinals.
const POSE_INDEX: u8 = 6;
const SLEEPING_POS_INDEX: u8 = 14;
const STANDING_POSE: i32 = 0;
const SLEEPING_POSE: i32 = 2;

/// The villager.
pub(crate) struct Villager;

/// `setUnhappy()` of a player's use: 40 ticks of a shaking head, and `villager_no` at
/// its volume and voice (two draws of its own stream), heard by everyone.
pub(crate) fn set_unhappy(used: &mut Used<'_>) {
    update(used.mob, |own| own.unhappy = UNHAPPY_TICKS);
    let baby = animal::is_baby(used.mob);
    let pitch = crate::mobs::body::hurt::voice_pitch(used.draws, baby);
    used.did.push(super::Did::Sound {
        sound: sounds::ENTITY_VILLAGER_NO,
        source: ladder::sound_source(used.kind),
        at: used.position,
        volume: 1.0,
        pitch,
        but_the_player: false,
    });
}

/// `Mob.aiStep`'s pick-up of a villager (`canPickUpLoot`, alive; `mob_griefing` is on):
/// each item of the region's within its box grown by 1, 0 and 1, with no pick-up delay,
/// that it wants, as much of it as its slots take (`InventoryCarrier.pickUpItem`), shown
/// as taken.
fn pick_up(tick: &mut MobTick<'_>) {
    if !tick.mob.body.flags.picks_up_loot || tick.mob.body.vitals.dying() {
        return;
    }
    let body = &tick.living.body;
    let bounds = Aabb::of_entity(body.position, body.width, body.height).inflate([1.0, 0.0, 1.0]);
    let found: Vec<(EntityId, ItemStack)> = tick
        .world
        .entities_in(&bounds)
        .iter()
        .filter(|seen| !seen.copied)
        .filter_map(|seen| match seen.entity.data {
            EntityData::Item {
                ref stack,
                pick_up_delay: 0,
                ..
            } if stack.count > 0 => Some((seen.entity.key, stack.clone())),
            _ => None,
        })
        .collect();
    for (key, stack) in found {
        if !wants_to_pick_up(&tick.mob, stack.clone()) {
            continue;
        }
        let mut moved = 0;
        update(&mut tick.mob, |own| {
            moved = add_item(&mut own.inventory, stack)
        });
        if moved > 0 {
            tick.out.local.push(Local::TakeSome {
                item: key,
                count: moved,
            });
        }
    }
}

impl MobType for Villager {
    /// `AbstractVillager`'s and `Villager`'s constructors: fire beside a path costs 16
    /// and fire is never walked; its navigation opens doors, floats across water and
    /// looks 48 blocks ahead. Its goals are its brain's.
    fn register_goals(&self, setup: &mut Setup) {
        setup.set_malus(PathType::FireInNeighbor, 16.0);
        setup.set_malus(PathType::Fire, -1.0);
        let navigation = &mut setup.config.navigation;
        navigation.flags.can_open_doors = true;
        navigation.set_can_float(true);
        navigation.set_required_path_length(48.0);
    }

    fn brain(&self) -> Option<&'static Provider> {
        Some(&PROVIDER)
    }

    /// `setCanPickUpLoot(true)`; and a villager made a baby before it is added (an
    /// egg's child) has its brain made again (`setBaby` → `ageBoundaryReached` →
    /// `refreshBrain`): its sensors' delays drawn once more.
    fn construct(&self, mob: &mut Mob, own: &mut Random) {
        mob.body.flags.picks_up_loot = true;
        if animal::is_baby(mob) {
            let state = brain::made(&PROVIDER, 0, own);
            crate::mobs::part::put(&mut mob.mind.brain, &state);
        }
    }

    /// `Villager.finalizeSpawn` (1440–1465): a bred one unemployed, its type settled by
    /// the biome at its block, then `AbstractVillager`'s: a group of no babies, so no
    /// draw. `Mob`'s follows.
    fn finalize_spawn(&self, spawn: &mut Spawning<'_>) {
        if spawn.reason == SpawnReason::Breeding {
            update(spawn.mob, |own| {
                own.data.profession = professions::NONE as u8;
            });
        }
        finalize_type(spawn.mob, spawn.biome);
        animal::finalize_ageable(spawn, SpawnGroup::Ageable(AgeableGroup::new(false)));
    }

    /// `brain.tick`, then (unless it has no AI) `random.nextInt(100)` for a raid at its
    /// block, of which there is none until N10d. Its schedule's activity is set before
    /// its first brain tick.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        if state_of(&tick.mob).1.schedule_at.is_none() {
            schedule_now(tick);
        }
        brain::run(tick, |_, _| {});
        // `customServerAiStep`'s trade's reputation event after the brain (part V3).
        social::after_brain(tick);
        if !tick.mob.body.flags.no_ai {
            tick.draws.next_int(100);
        }
    }

    /// `Villager.die`'s witnesses told and points of interest given back (part V3).
    fn on_die(&self, struck: &mut Struck<'_>, source: &DamageSource) {
        social::died(struck, source);
    }

    /// `LivingEntity.tick`'s check before `aiStep`: a sleeper whose bed is gone gets up
    /// (`checkBedExists`), and one a player's use of its bed woke (`kickVillagerOutOfBed`,
    /// `region/sleep.rs`) too; either way `Villager.stopSleeping`'s `last_woken`.
    fn before_ai_step(&self, tick: &mut MobTick<'_>) {
        let (_, own) = state_of(&tick.mob);
        let Some(head) = own.sleeping else {
            if own.kicked {
                update(&mut tick.mob, |own| own.kicked = false);
            }
            return;
        };
        let bed = tick.world.is_loaded(head)
            && clustine_blocks::data::is_class(
                tick.world.state(head),
                clustine_data::BlockClass::AbstractBedBlock,
            );
        if bed && !own.kicked {
            return;
        }
        update(&mut tick.mob, |own| own.kicked = false);
        if rest::stop_sleeping(tick) {
            let time = tick.world.game_time();
            let mut memories = brain::memories_of(&PROVIDER, &tick.mob, tick.lived);
            memories.set(MemoryKind::LastWoken, Value::Long(time));
            brain::write_memories(&PROVIDER, &mut tick.mob, memories);
        }
    }

    /// `Mob.aiStep`'s pick-up, then `AgeableMob.aiStep`'s growing; a baby grown up has
    /// its brain made again (`refreshBrain`) with the adult's schedule, and its bed
    /// sensor's search afresh.
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        pick_up(tick);
        let was_baby = animal::is_baby(&tick.mob);
        animal::after_ai_step(tick);
        if was_baby && !animal::is_baby(&tick.mob) {
            brain::remake(tick);
            update(&mut tick.mob, |own| own.bed_search = BedSearch::default());
            schedule_now(tick);
        }
    }

    /// `Villager.tick`'s after the entity's: the unhappy counter counted down, then
    /// `maybeDecayGossip` (part V3); and what its neighbours read of it written from its
    /// brain as its tick leaves it.
    fn after_tick(&self, tick: &mut MobTick<'_>) {
        let shown = shown_now(&tick.mob, tick.lived);
        update(&mut tick.mob, |own| {
            if own.unhappy > 0 {
                own.unhappy -= 1;
            }
            own.shown = shown;
        });
        social::maybe_decay_gossip(tick);
    }

    /// `Villager.mobInteract` (279–337): with anything but its egg, alive (and neither
    /// trading nor asleep: never), a baby shakes its head; an adult has no offers
    /// (none is ever made, its profession's or a template's), so it shakes its head and
    /// the use is consumed. Otherwise `AgeableMob`'s (the golden dandelion).
    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        // Trading's (part T2, `trading/merchant.rs`).
        crate::trading::merchant::villager_interact(used)
    }

    /// `Villager.getBreedOffspring` (1466–1521), of its own random: one `nextDouble()`,
    /// below a half the type of the biome at its block, below three quarters its own,
    /// else its partner's; unemployed and settled.
    fn breed_offspring(&self, breeding: &mut Breeding<'_>) -> Option<Mob> {
        let r = breeding.draws.next_double();
        let kind = if r < 0.5 {
            type_by_biome(breeding.biome)
        } else if r < 0.75 {
            state_of(breeding.this).1.data.kind
        } else {
            state_of(breeding.partner).1.data.kind
        };
        let mut child = Mob::fresh();
        update(&mut child, |own| {
            own.data = Data {
                kind,
                ..Data::default()
            };
            own.finalized = true;
        });
        Some(child)
    }

    /// `Mob.setBaby(baby)` of a villager made of another (a cured zombie villager).
    fn set_baby(&self, mob: &mut Mob, baby: bool) {
        if baby {
            animal::made_baby(mob);
        }
    }

    /// `Villager.readAdditionalSaveData` (832–920) of a template's tag, with
    /// `AgeableMob`'s `Age`, `ForcedAge` and `AgeLocked`: a `VillagerData` read, or a
    /// `VillagerDataFinalized` true, settles its type; the data's parts each default
    /// where missing (plains, none, 1); then the food level, experience and restocks.
    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        let (mut shared, mut own) = state_of(mob);
        shared.set_age(template::int(tag, "Age").unwrap_or(0), 0);
        shared.forced_age = template::int(tag, "ForcedAge").unwrap_or(0);
        shared.age_locked = template::boolean(tag, "AgeLocked").unwrap_or(false);
        let data = template::compound(tag, "VillagerData");
        if template::boolean(tag, "VillagerDataFinalized").unwrap_or(false) || data.is_some() {
            own.finalized = true;
            let data = data.unwrap_or_default();
            let named = |name: &str, names: &[&str], default: i32| {
                template::text(&data, name)
                    .and_then(|text| names.iter().position(|known| *known == text))
                    .map_or(default, |at| at as i32)
            };
            own.data = Data {
                kind: named("type", &villagers::TYPE_NAMES, kinds::PLAINS) as u8,
                profession: named(
                    "profession",
                    &villagers::PROFESSION_NAMES,
                    professions::NONE,
                ) as u8,
                level: template::int(&data, "level").unwrap_or(1),
            };
        }
        own.food_level = template::int(tag, "FoodLevel").unwrap_or(0) as i8;
        own.xp = template::int(tag, "Xp").unwrap_or(0);
        own.restocks_today = template::int(tag, "RestocksToday").unwrap_or(0);
        mob.state = animal::write(&shared, &own);
    }

    /// `setLastHurtByMob(player)` of a living villager: its angry particles (entity event
    /// 13) for a player's hurt.
    fn after_hurt(&self, struck: &mut Struck<'_>, source: &DamageSource, took: bool) {
        let by_player = source
            .cause
            .as_ref()
            .is_some_and(|cause| cause.living && cause.player.is_some());
        if took && by_player && !source.kind.is("minecraft:no_anger") && !struck.dying() {
            struck.out.events.push(Event::Entity(13));
        }
        // `setLastHurtByMob`'s `VILLAGER_HURT` reputation event (part V3).
        social::hurt(struck, source, took);
    }

    /// Its size: a sleeper's while it sleeps (its pose's), else an ageable mob's.
    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        if state_of(mob).1.sleeping.is_some() {
            return Some(SLEEPING);
        }
        animal::dimensions(mob, BABY)
    }

    /// `getAmbientSound()`: none asleep, else its ambient (a trade's is part T2's).
    fn ambient_sound(&self, mob: &Mob) -> Option<i32> {
        if state_of(mob).1.sleeping.is_some() {
            return None;
        }
        Some(sounds::ENTITY_VILLAGER_AMBIENT)
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_VILLAGER_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_VILLAGER_DEATH)
    }

    fn is_baby(&self, mob: &Mob) -> bool {
        animal::is_baby(mob)
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let (_, own) = state_of(mob);
        let mut values = animal::shown(mob);
        values.push((UNHAPPY_INDEX, DataShown::Int(own.unhappy), own.unhappy == 0));
        values.push((
            DATA_INDEX,
            DataShown::VillagerData {
                kind: i32::from(own.data.kind),
                profession: i32::from(own.data.profession),
                level: own.data.level,
            },
            own.data == Data::default(),
        ));
        values.push((
            FINALIZED_INDEX,
            DataShown::Boolean(own.finalized),
            !own.finalized,
        ));
        // A sleeper's pose and its bed (`startSleeping`'s `setPose`, `setSleepingPos`).
        let pose = if own.sleeping.is_some() {
            SLEEPING_POSE
        } else {
            STANDING_POSE
        };
        values.push((POSE_INDEX, DataShown::Pose(pose), own.sleeping.is_none()));
        values.push((
            SLEEPING_POS_INDEX,
            DataShown::OptionalBlockPos(own.sleeping),
            own.sleeping.is_none(),
        ));
        values
    }
}
