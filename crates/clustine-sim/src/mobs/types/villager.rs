//! The villager as a body (`Villager`, `AbstractVillager`; `docs/groundwork/n7k-brief.md`
//! section 2 and `n7k-plan.md` section 10, "The villager"), part K2: an ageable mob of a
//! type, a profession and a level (`VillagerData`), its type settled by the biome at its
//! feet unless a saved one settled it, which floats, strolls by day and looks about,
//! panics by a zombie or when hit, goes for food on the ground and keeps it in its eight
//! slots, rests at night as a villager with no bed does, and shakes its head at a
//! player who would trade with it.
//!
//! Its mind is a brain ([`PROVIDER`]): the game's nine sensors, of which those of points
//! of interest find nothing (the bed's, the secondary points', the golems' and the
//! babies' are kept for their start delays and the bed's draw); the core package
//! reduced to what runs without points of interest, trading, bells and raids (`Swim`,
//! `InteractWithDoor`, `LookAtTargetSink`, `VillagerPanicTrigger`, `MoveToTargetSink`,
//! `GoToWantedItem`); the idle package of B 7.14.2 (a stroll, walking to what it looks
//! at, doing nothing, and the full look); the panic package whole; the rest package's
//! part that runs without a home; and the schedule (`villager_schedule`'s activities by
//! the overworld's clock), whose work, meeting and play need what no villager has
//! (`job_site`, `meeting_point`, a baby's play package), so that those times are idle.
//!
//! Its state ([`VillagerState`]) follows what every ageable mob keeps
//! ([`animal::Animal`]), so that the golden dandelion and a baby's growing are the
//! animals'.
//!
//! **Across a line**: what it sees and runs from is the region's land and copies; what
//! hurt it and walked over a line is kept, as every brain's; its type and data are in
//! its record, which copies carry.
//!
//! Named differences: no trades, so a click never opens a screen and the template's
//! `Offers` are not kept (an igloo's cleric shakes its head too); the statistic of a
//! talk is not kept; no gossip, reputation, golem summoning, raids, points of interest,
//! work or beds (N10, N9); the stroll is `VillageBoundRandomStroll` with no village
//! anywhere, which is `LandRandomPos.getPos(10, 7)`, so villagers leave their village;
//! the core's poi and trading behaviours (`WakeUp`, `ReactToBell`, `SetRaidStatus`,
//! `ValidateNearbyPoi`, `PoiCompetitorScan`, `LookAndFollowTradingPlayerSink`,
//! `AcquirePoi`, `GoToPotentialJobSite`, `YieldJobSite`, `AssignProfessionFromJobSite`,
//! `ResetProfession`) and the idle package's interactions, gifts, trades, bed jumping
//! and love are left out, so that its draws are not the game's once they would run;
//! the schedule's activity is set at its first tick, not at its construction; a load of
//! a template's tag draws nothing for the brain it makes again (`refreshBrain`), as the
//! hook has no stream; the template's `Inventory`, `Gossips` and `Xp` are not read.

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
use crate::mobs::brain::behaviours::social_villagers;
use crate::mobs::brain::behaviours::villagers::{
    calm_down, from_schedule, go_to_closest_village, inside_brownian_walk, panic_trigger,
    set_raid_status, wants_a_home,
};
use crate::mobs::brain::behaviours::walking::{
    StrollTo, look_sink, sink, stroll_to, walk_to_look_target,
};
use crate::mobs::brain::behaviours::{doors, panic};
use crate::mobs::brain::sensors::{self, InertSensor, NearestItemSensor, VILLAGER_HOSTILES};
use crate::mobs::brain::{
    self, Activity, ActivityData, Behaviour, Cx, DoNothing, Gate, HeldTimed, MemoryKind, Order,
    Provider, Running, Status,
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

/// What `setActiveActivityIfPossible` makes of a scheduled activity: idle, the default,
/// for those this brain never added or whose memories no villager has (work, meeting,
/// play).
fn possible(activity: Activity) -> Activity {
    match activity {
        Activity::Rest => Activity::Rest,
        _ => Activity::Idle,
    }
}

/// `brain.updateActivityFromSchedule(attributes, gameTime, position)`: more than 20 game
/// ticks after the last, the schedule's activity asked for where it is not active.
fn update_from_schedule(cx: &mut Cx<'_, '_, '_>) {
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
    brain::set_recorded_activity(&mut tick.mob, possible(scheduled(day, baby)));
}

/// `NEAREST_ITEMS`: what it wants to pick up.
static NEAREST_ITEMS: NearestItemSensor = NearestItemSensor {
    wants: |ai, stack| wants_to_pick_up(&ai.tick.mob, stack),
};

/// `VILLAGER_BABIES`, `SECONDARY_POIS` and `GOLEM_DETECTED`, which find nothing that
/// anything of it reads.
static VILLAGER_BABIES: InertSensor = InertSensor {
    rate: 20,
    requires: &[MemoryKind::VisibleVillagerBabies],
};
static SECONDARY_POIS: InertSensor = InertSensor {
    rate: 40,
    requires: &[MemoryKind::SecondaryJobSite],
};
static GOLEM_DETECTED: InertSensor = InertSensor {
    rate: 200,
    requires: &[MemoryKind::VisibleMobs, MemoryKind::GolemDetectedRecently],
};

/// `Villager.BRAIN_PROVIDER` (Villager.txt 2313–2343): its nine sensors in the game's
/// order and its activities.
pub(crate) static PROVIDER: Provider = Provider {
    memories: &[],
    sensors: &[
        &sensors::NEAREST_LIVING_ENTITIES,
        &sensors::NEAREST_PLAYERS,
        &NEAREST_ITEMS,
        &sensors::NEAREST_BED,
        &sensors::HURT_BY,
        &VILLAGER_HOSTILES,
        &VILLAGER_BABIES,
        &SECONDARY_POIS,
        &GOLEM_DETECTED,
    ],
    activities,
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// `canPickUpLoot()` of the villager its brain runs.
fn can_pick_up(cx: &Cx<'_, '_, '_>) -> bool {
    cx.ai.tick.mob.body.flags.picks_up_loot
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

/// The villager's activities (`lambda$static$0`, Villager.txt 2216–2311, and
/// `VillagerGoalPackages`), each as the module's comment says it runs here.
fn activities() -> Vec<ActivityData> {
    let core = ActivityData {
        activity: Activity::Core,
        behaviours: vec![
            (0, panic::swim(0.8)),
            (0, doors::interact_with_door()),
            (0, look_sink(45, 90)),
            (0, panic_trigger()),
            (0, set_raid_status()),
            (1, sink()),
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
        ],
        conditions: Vec::new(),
        erase: Vec::new(),
    };
    let rest = ActivityData {
        activity: Activity::Rest,
        behaviours: vec![
            (
                2,
                wants_a_home(&[
                    MemoryKind::WalkTarget,
                    MemoryKind::CantReachWalkTargetSince,
                    MemoryKind::Home,
                ]),
            ),
            (3, wants_a_home(&[MemoryKind::Home])),
            (
                3,
                wants_a_home(&[
                    MemoryKind::Home,
                    MemoryKind::LastWoken,
                    MemoryKind::NearestBed,
                ]),
            ),
            (
                5,
                Box::new(Gate::new(
                    vec![(MemoryKind::Home, Status::Absent)],
                    Vec::new(),
                    Order::Shuffled,
                    Running::RunOne,
                    vec![
                        (wants_a_home(&[MemoryKind::WalkTarget, MemoryKind::Home]), 1),
                        (inside_brownian_walk(SPEED), 4),
                        (go_to_closest_village(SPEED, 4), 2),
                        (Box::new(HeldTimed::new(DoNothing { min: 20, max: 40 })), 2),
                    ],
                )),
            ),
            minimal_look(),
            (99, from_schedule(update_from_schedule)),
        ],
        conditions: Vec::new(),
        erase: Vec::new(),
    };
    let idle = ActivityData {
        activity: Activity::Idle,
        behaviours: vec![
            (
                2,
                Box::new(Gate::run_one(vec![
                    (
                        stroll_to(
                            SPEED,
                            StrollTo::Land {
                                horizontal: 10,
                                vertical: 7,
                                in_water: true,
                            },
                        ),
                        1,
                    ),
                    (walk_to_look_target(SPEED, 2), 1),
                    (Box::new(HeldTimed::new(DoNothing { min: 30, max: 60 })), 1),
                ])),
            ),
            full_look(),
            (99, from_schedule(update_from_schedule)),
        ],
        conditions: Vec::new(),
        erase: Vec::new(),
    };
    // `getPanicPackage(0.5)`: its speed half as much again.
    let fleeing = SPEED * 1.5;
    let panic = ActivityData {
        activity: Activity::Panic,
        behaviours: vec![
            (0, calm_down(update_from_schedule)),
            (1, away_from(MemoryKind::NearestHostile, fleeing, 6, false)),
            (1, away_from(MemoryKind::HurtByEntity, fleeing, 6, false)),
            (
                3,
                stroll_to(
                    fleeing,
                    StrollTo::Land {
                        horizontal: 2,
                        vertical: 2,
                        in_water: true,
                    },
                ),
            ),
            minimal_look(),
        ],
        conditions: Vec::new(),
        erase: Vec::new(),
    };
    vec![core, rest, idle, panic]
}

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
    /// block, of which there is none. Its schedule's activity is set before its first
    /// brain tick.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        if state_of(&tick.mob).1.schedule_at.is_none() {
            schedule_now(tick);
        }
        brain::run(tick, |_, _| {});
        // `customServerAiStep`'s trade's reputation event after the brain (part V3).
        social_villagers::after_brain(tick);
        if !tick.mob.body.flags.no_ai {
            tick.draws.next_int(100);
        }
    }

    /// `Villager.die`'s witnesses told and points of interest given back (part V3).
    fn on_die(&self, struck: &mut Struck<'_>, source: &DamageSource) {
        social_villagers::died(struck, source);
    }

    /// `Mob.aiStep`'s pick-up, then `AgeableMob.aiStep`'s growing; a baby grown up has
    /// its brain made again (`refreshBrain`) with the adult's schedule.
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        pick_up(tick);
        let was_baby = animal::is_baby(&tick.mob);
        animal::after_ai_step(tick);
        if was_baby && !animal::is_baby(&tick.mob) {
            brain::remake(tick);
            schedule_now(tick);
        }
    }

    /// `Villager.tick`'s after the entity's: the unhappy counter counted down.
    fn after_tick(&self, tick: &mut MobTick<'_>) {
        update(&mut tick.mob, |own| {
            if own.unhappy > 0 {
                own.unhappy -= 1;
            }
        });
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
        social_villagers::hurt(struck, source, took);
    }

    fn dimensions(&self, mob: &Mob) -> Option<Dimensions> {
        animal::dimensions(mob, BABY)
    }

    /// `getAmbientSound()`: never asleep nor trading, so its ambient.
    fn ambient_sound(&self, _: &Mob) -> Option<i32> {
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
        values
    }
}
