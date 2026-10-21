//! The allay (`Allay`, `AllayAi`, `InventoryCarrier`; `docs/groundwork/n7-brief.md`,
//! section 7.10): a flying helper, not an animal, that holds in its hand the kind of item
//! a player gave it, gathers items of that kind into a slot of its own and brings them to
//! that player or to a note block it heard. It dances by a playing jukebox, and dancing
//! is duplicated by an amethyst shard; it heals a point every half a second, takes no
//! fall and never despawns, and the player it likes cannot hurt it.
//!
//! Here are its flight (`travelFlying`, the flying move control that hovers), its state,
//! its synced data, the gift of an item and its taking back, the duplication with its
//! cooldown, the healing, the dance's end, the pick-up of what it wants, its drops and
//! sounds, and its brain (`AllayAi`, B 7.10.2 and 7.10.3): the provider of the game's
//! memories (the player it likes among them, which the gift sets, the taking back clears
//! and its hurts read), its sensors (the items it wants among them), its two activities,
//! and its three behaviours of its own: `GoToWantedItem`, `GoAndGiveItemsToTarget` and
//! `StayCloseToTarget`, with the throw. [`brain_seams`] are what its two listeners will
//! call.
//!
//! **Across a line** (`docs/groundwork/n7j-plan.md`, section 3): it sees items of the
//! region's land and copies only (item 1). The player it likes is looked up among the
//! world's players (`MobWorld::player_anywhere`, item 2), within 64 blocks; one the
//! region's view does not show is followed to where the world's players last put them
//! (a point, a second late), as no tracker of the view follows them. Its liked note block
//! is read where the land or the copies hold its chunk; where neither does the answer is
//! "unknown" (item 3, question 7.11): the memory is kept, and that tick it takes the
//! player it likes as its target, as it would were the note block not liked. "Unknown"
//! never erases it; only a block known not to be a note block, its cooldown run out or a
//! distance of more than 1024 do (plan, section 8 item 8). What it throws crosses a line
//! as any item does.
//!
//! Named differences: the jukebox and the note block are heard through entity listeners
//! that move with it (`DynamicGameEventListener`), which `region/vibrations.rs` does not
//! have yet, so it never starts dancing nor likes a note block by itself; its arm's swing
//! as a player takes its item is not shown; a copy made by duplication faces south (the
//! game keeps the yaw its constructor drew); its steps on the ground sound as the block's
//! (its own are silent); what it throws keeps no thrower; no advancement is given for a
//! throw on a block; the liked player's game mode is not asked beyond spectating (every
//! player is creative); `GlobalPos.isCloseEnough`'s distance is read as the chessboard
//! distance (**Inferred**: `GlobalPos` is not dumped).

use clustine_blocks::data::{is_class, item_in_tag};
use clustine_data::{BlockClass, entity_types, sounds};
use clustine_world::{BlockPos, EntityData, EntityId, ItemStack, TypeState};
use serde::{Deserialize, Serialize};

use super::{Did, Interaction, MobType, Used};
use crate::living::{Travel, slots};
use crate::mobs::Mob;
use crate::mobs::ai::navigation::{Kind, Settings};
use crate::mobs::ai::{Ai, MoveKind, Operation, Setup, controls};
use crate::mobs::body::Event;
use crate::mobs::body::hurt::DamageSource;
use crate::mobs::brain::behaviours::cooldowns::count_down;
use crate::mobs::brain::behaviours::looking::{LookAt, look_sometimes};
use crate::mobs::brain::behaviours::panic::{panic, swim};
use crate::mobs::brain::behaviours::walking::{
    StrollTo, look_sink, sink, stroll_to, walk_to_look_target,
};
use crate::mobs::brain::behaviours::{entity_of, position_of, tracking, walk};
use crate::mobs::brain::sensors::{self, NearestItemSensor, closer_than, find};
use crate::mobs::brain::{
    self, Activity, ActivityData, Brain, Cx, DoNothing, Gate, HeldShot, HeldTimed, Memories,
    MemoryKind, Provider, Shot, Status, Timed, Tracker, Value,
};
use crate::mobs::tick::{Local, MobTick};
use crate::physics::aabb::Aabb;
use crate::physics::item::{SOUND_SOURCE_NEUTRAL, max_stack_size};
use crate::physics::{self, Vec3};
use crate::region::DEFAULT_PICK_UP_DELAY;
use crate::shown::DataShown;

/// The version of an allay's type state: 2 since the player it likes is its brain's.
const VERSION: u8 = 2;

/// The synced data's indices: dancing and whether it may duplicate (`PathfinderMob`
/// defines none after `Mob`'s 15), and every entity's `NoGravity`.
pub(crate) const DANCING_INDEX: u8 = 16;
pub(crate) const CAN_DUPLICATE_INDEX: u8 = 17;
const NO_GRAVITY_INDEX: u8 = 5;

/// `DUPLICATION_COOLDOWN_TICKS`.
pub(crate) const DUPLICATION_COOLDOWN: u32 = 6000;

/// How far a playing jukebox keeps it dancing (`shouldStopDancing`'s 10, and
/// `JUKEBOX_PLAY`'s notification radius).
const JUKEBOX_REACH: f64 = 10.0;

/// `0.3f` widened: how far below its eyes it throws from (`BehaviorUtils.throwItem`).
const THROW_DROP: f64 = 0.300_000_011_920_928_96;

/// What an allay keeps: whether it dances and to which jukebox (neither saved by the
/// game), when its duplication's cooldown ends (`duplicationCooldown`, kept as the moment
/// it comes to nothing, and none once it has: `DATA_CAN_DUPLICATE`), the one slot of its
/// inventory, and whether its flight has set `NoGravity`. The player it likes is its
/// brain's (`LIKED_PLAYER`, [`liked_player`]).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct AllayState {
    pub(crate) dancing: bool,
    pub(crate) jukebox: Option<BlockPos>,
    pub(crate) cooldown_until: Option<u32>,
    pub(crate) inventory: Option<ItemStack>,
    pub(crate) no_gravity: bool,
}

impl AllayState {
    /// `canDuplicate()`: its cooldown has come to nothing.
    pub(crate) fn can_duplicate(&self) -> bool {
        self.cooldown_until.is_none()
    }

    /// `duplicationCooldown` at the moment `now`.
    pub(crate) fn cooldown(&self, now: u32) -> i32 {
        self.cooldown_until
            .map_or(0, |until| (i64::from(until) - i64::from(now)).max(0) as i32)
    }
}

/// An allay's state from its type state; a record of nothing, or one this build cannot
/// read, as a fresh allay's.
pub(crate) fn read(mob: &Mob) -> AllayState {
    if mob.state.bytes.is_empty() || mob.state.version != VERSION {
        return AllayState::default();
    }
    postcard::from_bytes(&mob.state.bytes).unwrap_or_default()
}

fn put(mob: &mut Mob, own: &AllayState) {
    mob.state = if *own == AllayState::default() {
        TypeState::default()
    } else {
        TypeState {
            version: VERSION,
            bytes: postcard::to_allocvec(own).expect("a type state is encoded into memory"),
        }
    };
}

/// Changes an allay's state.
pub(crate) fn update(mob: &mut Mob, change: impl FnOnce(&mut AllayState)) {
    let mut own = read(mob);
    let before = own.clone();
    change(&mut own);
    if own != before {
        put(mob, &own);
    }
}

/// `resetDuplicationCooldown()` at the moment `now`: 6000 ticks until it may again.
pub(crate) fn reset_duplication_cooldown(own: &mut AllayState, now: u32) {
    own.cooldown_until = Some(now.saturating_add(DUPLICATION_COOLDOWN));
}

/// `allayConsidersItemEqual(a, b)`: the same item (and the same potion's contents, which
/// no stack carries yet).
fn considers_equal(a: ItemStack, b: ItemStack) -> bool {
    a.item == b.item
}

/// `wantsToPickUp(level, stack)`: it holds something of the same kind and its slot can
/// take the stack (`mobGriefing` is always on).
pub(crate) fn wants_to_pick_up(mob: &Mob, stack: ItemStack) -> bool {
    let Some(hand) = mob.body.equipment.slots[usize::from(slots::MAIN_HAND)] else {
        return false;
    };
    considers_equal(hand, stack) && can_add_item(read(mob).inventory, stack)
}

/// `SimpleContainer.canAddItem(stack)` of its one slot: empty, or of the same item and
/// not full.
fn can_add_item(slot: Option<ItemStack>, stack: ItemStack) -> bool {
    match slot.filter(|held| held.count > 0) {
        None => true,
        Some(held) => held.item == stack.item && held.count < max_stack_size(held),
    }
}

/// `SimpleContainer.addItem(stack)` of its one slot: how many of the stack go in.
fn add_item(slot: &mut Option<ItemStack>, stack: ItemStack) -> i32 {
    match slot.filter(|held| held.count > 0) {
        None => {
            *slot = Some(stack);
            stack.count
        }
        Some(held) if held.item == stack.item => {
            let moved = (max_stack_size(held) - held.count).clamp(0, stack.count);
            *slot = Some(ItemStack {
                count: held.count + moved,
                ..held
            });
            moved
        }
        Some(_) => 0,
    }
}

/// The seams of its brain and its listeners (`AllayAi`, `Allay$JukeboxListener`,
/// `Allay$VibrationUser`): what its behaviours call, and what the entity listeners of
/// `region/vibrations.rs` will call once they come.
pub(crate) mod brain_seams {
    use super::*;

    /// `setDancing(dancing)`: a panicking allay does not start.
    pub(crate) fn set_dancing(mob: &mut Mob, dancing: bool, panicking: bool) {
        if dancing && panicking {
            return;
        }
        update(mob, |own| own.dancing = dancing);
    }

    /// `AllayAi.hearNoteblock(allay, pos)`: a note block heard where it likes none is
    /// liked, for 600 ticks; the one it likes heard again is liked 600 ticks more; another
    /// is passed over.
    #[allow(
        dead_code,
        reason = "what its vibration listener calls once entity listeners come"
    )]
    pub(crate) fn hear_noteblock(memories: &mut Memories, pos: BlockPos) {
        match memories.get(MemoryKind::LikedNoteblock) {
            None => {
                memories.set(MemoryKind::LikedNoteblock, Value::Block(pos));
                memories.set(
                    MemoryKind::LikedNoteblockCooldownTicks,
                    Value::Int(LIKED_NOTEBLOCK_TICKS),
                );
            }
            Some(Value::Block(liked)) if liked == pos => memories.set(
                MemoryKind::LikedNoteblockCooldownTicks,
                Value::Int(LIKED_NOTEBLOCK_TICKS),
            ),
            Some(_) => {}
        }
    }

    /// `Allay$VibrationUser.canReceiveVibration(level, pos, event, context)` of an allay
    /// at the block `at` with its AI: any where it likes no note block, else only the one
    /// it likes, while within 1024.
    #[allow(
        dead_code,
        reason = "what its vibration listener calls once entity listeners come"
    )]
    pub(crate) fn can_receive_vibration(
        memories: &Memories,
        at: BlockPos,
        pos: BlockPos,
        no_ai: bool,
    ) -> bool {
        if no_ai {
            return false;
        }
        match memories.get(MemoryKind::LikedNoteblock) {
            Some(Value::Block(liked)) => close_enough(liked, at) && liked == pos,
            _ => true,
        }
    }

    /// `setJukeboxPlaying(pos, playing)`: a jukebox that starts sets it dancing, where it
    /// does not already; one that stops (or any, where it knows none) ends it.
    #[allow(
        dead_code,
        reason = "what its jukebox listener calls once entity listeners come"
    )]
    pub(crate) fn set_jukebox_playing(mob: &mut Mob, pos: BlockPos, playing: bool) {
        let own = read(mob);
        if playing {
            if !own.dancing {
                update(mob, |own| own.jukebox = Some(pos));
                set_dancing(mob, true, false);
            }
        } else if own.jukebox == Some(pos) || own.jukebox.is_none() {
            update(mob, |own| {
                own.jukebox = None;
                own.dancing = false;
            });
        }
    }

    /// `hasItemToThrow`: its slot holds something.
    pub(crate) fn has_item_to_throw(mob: &Mob) -> bool {
        read(mob).inventory.is_some_and(|stack| stack.count > 0)
    }

    /// `AllayAi.throwItem`'s first step: one item out of its slot to throw.
    pub(crate) fn take_one_to_throw(mob: &mut Mob) -> Option<ItemStack> {
        let mut taken = None;
        update(mob, |own| {
            if let Some(stack) = own.inventory.filter(|stack| stack.count > 0) {
                taken = Some(ItemStack { count: 1, ..stack });
                own.inventory = (stack.count > 1).then_some(ItemStack {
                    count: stack.count - 1,
                    ..stack
                });
            }
        });
        taken
    }

    /// `THROW_SOUND_PITCHES`, one of which a throw's sound is at.
    pub(crate) const THROW_SOUND_PITCHES: [f32; 16] = [
        0.5625, 0.625, 0.75, 0.9375, 1.0, 1.0, 1.125, 1.25, 1.5, 1.875, 2.0, 2.25, 2.5, 3.0, 3.75,
        4.0,
    ];
}

// ------------------------------------------------------------------------------------
// The brain (`Allay.brainProvider`, `AllayAi`).

/// `TIME_TO_FORGET_NOTEBLOCK`: how long a note block heard is liked.
pub(crate) const LIKED_NOTEBLOCK_TICKS: i32 = 600;

/// `ITEM_PICKUP_COOLDOWN_DURATION`: how long after a throw it picks nothing up.
pub(crate) const PICKUP_COOLDOWN: i32 = 60;

/// `DISTANCE_TO_WANTED_ITEM`, `TOO_FAR_FROM_TARGET`, `CLOSE_ENOUGH_TO_TARGET`.
const WANTED_ITEM_REACH: f64 = 32.0;
const TOO_FAR: f64 = 16.0;
const CLOSE_ENOUGH: i32 = 4;

/// `getLikedPlayer`'s reach, and a liked note block's (`isCloseEnough(.., 1024)`).
const LIKED_PLAYER_REACH: f64 = 64.0;
const NOTEBLOCK_REACH: i32 = 1024;

/// `SPEED_MULTIPLIER_WHEN_RETRIEVING_ITEM` and `..._FOLLOWING_DEPOSIT_TARGET`.
const RETRIEVING: f32 = 1.75;
const DEPOSITING: f32 = 2.25;

/// `THROW_VELOCITY`: the floats 0.2, 0.3 and 0.2 widened.
const THROW_VELOCITY: Vec3 = [
    0.200_000_002_980_232_24,
    0.300_000_011_920_928_96,
    0.200_000_002_980_232_24,
];

/// The `0.2f` below its eyes it throws from (`AllayAi.throwItem`).
const DEPOSIT_DROP: f32 = 0.2;

/// `NEAREST_ITEMS`: what it wants is what its hand holds.
static NEAREST_ITEMS: NearestItemSensor = NearestItemSensor {
    wants: |ai, stack| wants_to_pick_up(&ai.tick.mob, stack),
};

/// `Allay.brainProvider()` (Allay.txt 1195–1206): the player and note block it likes and
/// the note block's cooldown named, and its four sensors in the game's order.
pub(crate) static PROVIDER: Provider = Provider {
    memories: &[
        MemoryKind::LikedPlayer,
        MemoryKind::LikedNoteblock,
        MemoryKind::LikedNoteblockCooldownTicks,
    ],
    sensors: &[
        &sensors::NEAREST_LIVING_ENTITIES,
        &sensors::NEAREST_PLAYERS,
        &sensors::HURT_BY,
        &NEAREST_ITEMS,
    ],
    activities,
    core: &[Activity::Core],
    default: Activity::Idle,
};

/// `AllayAi.getActivities()`: core and idle.
fn activities() -> Vec<ActivityData> {
    let core = ActivityData::numbered(
        Activity::Core,
        0,
        vec![
            swim(0.8),
            panic(2.5),
            look_sink(45, 90),
            sink(),
            count_down(MemoryKind::LikedNoteblockCooldownTicks),
            count_down(MemoryKind::ItemPickupCooldownTicks),
        ],
    );
    let idle = ActivityData::numbered(
        Activity::Idle,
        0,
        vec![
            Box::new(HeldShot::new(GoToWantedItem)),
            Box::new(HeldTimed::new(GoAndGiveItemsToTarget)),
            Box::new(HeldShot::new(StayCloseToTarget)),
            look_sometimes(LookAt::Any, 6.0, (30, 60)),
            Box::new(Gate::run_one(vec![
                (stroll_to(1.0, StrollTo::Fly), 2),
                (walk_to_look_target(1.0, 3), 2),
                (Box::new(HeldTimed::new(DoNothing { min: 30, max: 60 })), 1),
            ])),
        ],
    );
    vec![core, idle]
}

/// What a fresh load of an allay resets (the plan's 2.10, the brief's 7.10.7): its dance
/// and its jukebox, which the game does not save; its inventory, its duplication's
/// cooldown, its `NoGravity` and the memories of a codec (the player and note block it
/// likes, the cooldowns) come back. For the brain's `fresh_load` when it comes.
#[allow(
    dead_code,
    reason = "the fresh load of N7j1's contracts, not built yet"
)]
pub(crate) fn fresh_load(mob: &mut Mob) {
    update(mob, |own| {
        own.dancing = false;
        own.jukebox = None;
    });
}

/// `AllayAi.updateActivity`: idle.
pub(crate) fn update_activity(brain: &mut Brain) {
    brain.set_active_to_first_valid(&[Activity::Idle]);
}

/// The memories of an allay outside its brain's tick (a use, a hurt), as of its last.
fn memories(mob: &Mob, lived: u32) -> Memories {
    brain::memories_of(&PROVIDER, mob, lived)
}

/// The player it likes (`LIKED_PLAYER`), by their UUID.
pub(crate) fn liked_player(mob: &Mob) -> Option<u128> {
    // Neither expiring nor counted down: any moment reads it alike.
    match memories(mob, 1).get(MemoryKind::LikedPlayer)? {
        Value::Uuid(uuid) => Some(uuid),
        _ => None,
    }
}

/// `isPanicking()` of an allay outside its brain's tick, after it at `lived`.
fn panicking(mob: &Mob, lived: u32) -> bool {
    memories(mob, lived.saturating_add(1)).check(MemoryKind::IsPanicking, Status::Present)
}

/// `canPickUpLoot()` (Allay.txt 451–462): not waiting after a throw, and holding
/// something.
fn can_pick_up_loot(mob: &Mob, memories: &Memories) -> bool {
    !memories.check(MemoryKind::ItemPickupCooldownTicks, Status::Present)
        && mob.body.equipment.slots[usize::from(slots::MAIN_HAND)].is_some()
}

/// `GlobalPos.isCloseEnough(dimension, pos, 1024)` in the one dimension.
fn close_enough(liked: BlockPos, at: BlockPos) -> bool {
    let chessboard = (liked.x - at.x)
        .abs()
        .max((liked.y - at.y).abs())
        .max((liked.z - at.z).abs());
    chessboard <= NOTEBLOCK_REACH
}

/// `AllayAi.getLikedPlayer(allay)` as a tracker of their eyes: a player of the world not
/// spectating, within 64 blocks. One the region's view shows is tracked; one only the
/// world's published players show is taken where they last were (see the module's
/// comment).
fn liked_player_tracker(cx: &Cx<'_, '_, '_>) -> Option<Tracker> {
    let Value::Uuid(uuid) = cx.memories.get(MemoryKind::LikedPlayer)? else {
        return None;
    };
    let player = cx.ai.tick.world.player_anywhere(uuid)?;
    if player.spectator
        || physics::distance_sqr(player.position, cx.ai.position())
            >= LIKED_PLAYER_REACH * LIKED_PLAYER_REACH
    {
        return None;
    }
    if find(cx.ai, player.key).is_some() {
        return Some(tracking(player.key, true));
    }
    let [x, y, z] = player.position;
    Some(Tracker::Point([x, y + player.eye_height, z]))
}

/// `AllayAi.getItemDepositPosition(allay)`: above the note block it likes, where it
/// still should take its items there (`shouldDepositItemsAtLikedNoteblock`: within 1024,
/// a note block still, and heard within 600 ticks); else, that note block forgotten, the
/// player it likes. A note block whose chunk the region does not read is "unknown": kept,
/// and the player it likes taken for this time.
fn deposit_target(cx: &mut Cx<'_, '_, '_>) -> Option<Tracker> {
    if let Some(Value::Block(liked)) = cx.memories.get(MemoryKind::LikedNoteblock) {
        let cooling = cx
            .memories
            .check(MemoryKind::LikedNoteblockCooldownTicks, Status::Present);
        let near = close_enough(liked, cx.ai.block_position());
        let world = cx.ai.tick.world;
        if near && !world.has_chunk(liked.chunk()) {
            return liked_player_tracker(cx);
        }
        if near && is_class(world.state(liked), BlockClass::NoteBlock) && cooling {
            return Some(Tracker::Block(liked.offset(0, 1, 0)));
        }
        cx.memories.erase(MemoryKind::LikedNoteblock);
    }
    liked_player_tracker(cx)
}

/// `GoToWantedItem.create(e -> true, 1.75, true, 32)` (B 7.10.3): the wanted item it sees
/// within 32, while it may pick up, looked at and flown to. An item the view no longer has
/// is "unknown" and not gone after.
struct GoToWantedItem;

impl Shot for GoToWantedItem {
    fn required(&self) -> &[MemoryKind] {
        &[
            MemoryKind::WalkTarget,
            MemoryKind::LookTarget,
            MemoryKind::NearestVisibleWantedItem,
            MemoryKind::ItemPickupCooldownTicks,
        ]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx
            .memories
            .check(MemoryKind::WalkTarget, Status::Registered)
            || !cx
                .memories
                .check(MemoryKind::LookTarget, Status::Registered)
            || !cx
                .memories
                .check(MemoryKind::NearestVisibleWantedItem, Status::Present)
            || !cx
                .memories
                .check(MemoryKind::ItemPickupCooldownTicks, Status::Registered)
        {
            return false;
        }
        let Some(item) =
            entity_of(cx, MemoryKind::NearestVisibleWantedItem).and_then(|key| find(cx.ai, key))
        else {
            return false;
        };
        if cx
            .memories
            .check(MemoryKind::ItemPickupCooldownTicks, Status::Present)
            || !closer_than(cx.ai, &item, WANTED_ITEM_REACH)
            || !can_pick_up_loot(&cx.ai.tick.mob, cx.memories)
        {
            return false;
        }
        cx.memories.set(
            MemoryKind::LookTarget,
            Value::Tracker(tracking(item.key, true)),
        );
        cx.memories.set(
            MemoryKind::WalkTarget,
            walk(tracking(item.key, false), RETRIEVING, 0),
        );
        true
    }
}

/// `GoAndGiveItemsToTarget(getItemDepositPosition, 2.25, 20, throwItem,
/// ITEM_PICKUP_COOLDOWN_TICKS, 60, hasItemToThrow)` (B 7.10.3): with items gathered and a
/// place to take them, it flies there for 20 ticks at a time, and within 3 of it (its
/// eyes) throws one each tick, waiting 60 ticks before it picks anything up again.
struct GoAndGiveItemsToTarget;

impl GoAndGiveItemsToTarget {
    /// `canThrowItemToTarget`: items to throw, and a place to.
    fn can_throw(cx: &mut Cx<'_, '_, '_>) -> bool {
        brain_seams::has_item_to_throw(&cx.ai.tick.mob) && deposit_target(cx).is_some()
    }
}

impl Timed for GoAndGiveItemsToTarget {
    type State = ();

    fn entry(&self) -> &[(MemoryKind, Status)] {
        &[
            (MemoryKind::LookTarget, Status::Registered),
            (MemoryKind::WalkTarget, Status::Registered),
            (MemoryKind::ItemPickupCooldownTicks, Status::Registered),
        ]
    }

    fn durations(&self) -> (i64, i64) {
        (20, 20)
    }

    fn extra_start(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) -> bool {
        Self::can_throw(cx)
    }

    fn can_still_use(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) -> bool {
        Self::can_throw(cx)
    }

    fn start(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        // `BehaviorUtils.setWalkAndLookTargetMemories(allay, target, 2.25, 3)`.
        if let Some(target) = deposit_target(cx) {
            cx.memories
                .set(MemoryKind::LookTarget, Value::Tracker(target));
            cx.memories
                .set(MemoryKind::WalkTarget, walk(target, DEPOSITING, 3));
        }
    }

    fn tick(&self, _: &mut (), cx: &mut Cx<'_, '_, '_>) {
        let Some(target) = deposit_target(cx) else {
            return;
        };
        let Some(at) = position_of(cx, &target) else {
            return;
        };
        let [x, _, z] = cx.ai.position();
        let eyes = [x, cx.ai.eye_y(), z];
        if physics::distance_sqr(at, eyes).sqrt() < 3.0 {
            throw_item(cx, at);
            cx.memories.set(
                MemoryKind::ItemPickupCooldownTicks,
                Value::Int(PICKUP_COOLDOWN),
            );
        }
    }
}

/// `AllayAi.throwItem(level, allay, target)`: one item of its slot thrown towards a block
/// above the target from a fifth below its eyes (`BehaviorUtils.throwItem`); on a game
/// tick that is a multiple of 7, nine times in ten its throw is heard, at one of sixteen
/// pitches (two draws of the level's).
fn throw_item(cx: &mut Cx<'_, '_, '_>, target: Vec3) {
    let Some(stack) = brain_seams::take_one_to_throw(&mut cx.ai.tick.mob) else {
        return;
    };
    let position = cx.ai.position();
    let aim = [target[0], target[1] + 1.0, target[2]];
    let way = physics::normalize(physics::sub(aim, position));
    let motion = [
        way[0] * THROW_VELOCITY[0],
        way[1] * THROW_VELOCITY[1],
        way[2] * THROW_VELOCITY[2],
    ];
    let at = [
        position[0],
        cx.ai.eye_y() - f64::from(DEPOSIT_DROP),
        position[2],
    ];
    cx.ai.tick.out.local.push(Local::Throw {
        stack,
        at,
        pick_up_delay: DEFAULT_PICK_UP_DELAY,
        motion,
    });
    if cx.time.rem_euclid(7) == 0 && cx.ai.tick.draws.next_double() < 0.9 {
        let pick = cx.ai.tick.draws.next_int(16) as usize;
        cx.ai.tick.out.events.push(Event::Sound {
            sound: sounds::ENTITY_ALLAY_ITEM_THROWN,
            volume: 1.0,
            pitch: brain_seams::THROW_SOUND_PITCHES[pick],
        });
    }
}

/// `StayCloseToTarget.create(getItemDepositPosition, not(hasWantedItem), 4, 16, 2.25)`
/// (B 7.10.3): seeing no item it wants, 16 or more from where it takes its items, it
/// flies back to within 4.
struct StayCloseToTarget;

impl Shot for StayCloseToTarget {
    fn required(&self) -> &[MemoryKind] {
        &[MemoryKind::LookTarget, MemoryKind::WalkTarget]
    }

    fn trigger(&self, cx: &mut Cx<'_, '_, '_>) -> bool {
        if !cx
            .memories
            .check(MemoryKind::LookTarget, Status::Registered)
            || !cx
                .memories
                .check(MemoryKind::WalkTarget, Status::Registered)
        {
            return false;
        }
        let Some(target) = deposit_target(cx) else {
            return false;
        };
        // `hasWantedItem`: `NEAREST_VISIBLE_WANTED_ITEM` there.
        if cx
            .memories
            .check(MemoryKind::NearestVisibleWantedItem, Status::Present)
        {
            return false;
        }
        let Some(at) = position_of(cx, &target) else {
            return false;
        };
        if physics::distance_sqr(cx.ai.position(), at) < TOO_FAR * TOO_FAR {
            return false;
        }
        cx.memories
            .set(MemoryKind::LookTarget, Value::Tracker(target));
        cx.memories.set(
            MemoryKind::WalkTarget,
            walk(target, DEPOSITING, CLOSE_ENOUGH),
        );
        true
    }
}

/// `shouldStopDancing()`: it knows no jukebox, is ten or more from its middle, or the
/// block there is no jukebox any more.
fn should_stop_dancing(tick: &MobTick<'_>, own: &AllayState) -> bool {
    let Some(jukebox) = own.jukebox else {
        return true;
    };
    !super::turtle::closer_to_centre_than(jukebox, tick.living.body.position, JUKEBOX_REACH)
        || !is_class(tick.world.state(jukebox), BlockClass::JukeboxBlock)
}

/// `Mob.aiStep`'s pick-up after its brain's tick (`canPickUpLoot`: not waiting after a
/// throw, and its hand holds something): each item within a block of its box that it
/// wants, as much of it as its slot takes (`InventoryCarrier.pickUpItem`), shown as taken.
fn pick_up(tick: &mut MobTick<'_>) {
    let after_brain = memories(&tick.mob, tick.lived.saturating_add(1));
    if tick.mob.body.vitals.dying() || !can_pick_up_loot(&tick.mob, &after_brain) {
        return;
    }
    let body = &tick.living.body;
    let bounds = Aabb::of_entity(body.position, body.width, body.height).inflate([1.0, 1.0, 1.0]);
    let found: Vec<(EntityId, ItemStack)> = tick
        .world
        .entities_in(&bounds)
        .iter()
        .filter(|seen| !seen.copied)
        .filter_map(|seen| match seen.entity.data {
            EntityData::Item {
                stack,
                pick_up_delay: 0,
                ..
            } if stack.count > 0 => Some((seen.entity.key, stack)),
            _ => None,
        })
        .collect();
    for (key, stack) in found {
        if !wants_to_pick_up(&tick.mob, stack) {
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

/// `Allay.mobInteract` (the brief's 7.10.5): dancing, an amethyst shard duplicates it;
/// an empty hand of its own takes what the player holds, one of it, and likes them; a
/// player with an empty main hand takes its item back, and what it gathered falls at
/// its eyes. The player keeps what they gave (every player is creative), and the sounds
/// are not sent to them, whose client plays its own.
fn mob_interact(used: &mut Used<'_>) -> Interaction {
    let hand = usize::from(slots::MAIN_HAND);
    let held = used.held.filter(|stack| stack.count > 0);
    let own = read(used.mob);
    let holding = used.mob.body.equipment.slots[hand].filter(|stack| stack.count > 0);
    let sound = |sound: i32| Did::Sound {
        sound,
        source: SOUND_SOURCE_NEUTRAL,
        at: used.position,
        volume: 2.0,
        pitch: 1.0,
        but_the_player: true,
    };
    if own.dancing
        && held.is_some_and(|stack| item_in_tag(stack.item, "minecraft:duplicates_allays"))
        && own.can_duplicate()
    {
        // `duplicateAllay()`: a copy where it is, kept, both waiting 6000 ticks.
        let mut copy = Mob::fresh();
        copy.body.flags.persistent = true;
        update(&mut copy, |made| reset_duplication_cooldown(made, 0));
        let now = used.moment;
        update(used.mob, |own| reset_duplication_cooldown(own, now));
        used.did.push(Did::Spawn {
            kind: used.kind,
            mob: Box::new(copy),
        });
        used.did.push(Did::Event(18));
        used.did.push(sound(sounds::BLOCK_AMETHYST_BLOCK_CHIME));
        return Interaction::Success;
    }
    if holding.is_none()
        && let Some(given) = held
    {
        used.mob.body.equipment.slots[hand] = Some(ItemStack { count: 1, ..given });
        used.did.push(sound(sounds::ENTITY_ALLAY_ITEM_GIVEN));
        // `LIKED_PLAYER = player.getUUID()`.
        if let Some(player) = used.player_uuid {
            let mut memories = memories(used.mob, used.moment);
            memories.set(MemoryKind::LikedPlayer, Value::Uuid(player));
            brain::write_memories(&PROVIDER, used.mob, memories);
        }
        return Interaction::Success;
    }
    if let Some(taken) = holding
        && held.is_none()
    {
        used.mob.body.equipment.slots[hand] = None;
        used.did.push(sound(sounds::ENTITY_ALLAY_ITEM_TAKEN));
        let gathered = own.inventory.filter(|stack| stack.count > 0);
        update(used.mob, |own| own.inventory = None);
        let mut memories = memories(used.mob, used.moment);
        memories.erase(MemoryKind::LikedPlayer);
        brain::write_memories(&PROVIDER, used.mob, memories);
        if let Some(stack) = gathered {
            // `BehaviorUtils.throwItem(this, stack, position())`: aimed at its own feet,
            // so with no motion of its own.
            let info = clustine_data::EntityTypeInfo::of(entity_types::ALLAY);
            let [x, y, z] = used.position;
            used.did.push(Did::Throw {
                stack,
                at: [x, y + f64::from(info.eye_height) - THROW_DROP, z],
                pick_up_delay: DEFAULT_PICK_UP_DELAY,
                motion: [0.0; 3],
            });
        }
        used.did.push(Did::Give {
            stack: taken,
            no_duplicates: false,
        });
        return Interaction::Success;
    }
    Interaction::Pass
}

/// The allay.
pub(crate) struct Allay;

impl MobType for Allay {
    /// `Mob.readAdditionalSaveData`'s `Brain`, which `makeBrain` builds anew of the
    /// tag's memories. The census's allays (the outposts' cage) carry an empty one
    /// (`Brain {memories {}}`), which is what a new allay's brain remembers, so the
    /// brain its making built is kept as it is. The only memories an allay keeps (a
    /// liked player, a liked note block) name what a newly generated world has not.
    fn from_template(&self, mob: &mut Mob, tag: &[u8]) {
        let _ = (mob, tag);
    }

    /// Its constructor's controls and `createNavigation`: a flying navigation that opens
    /// no doors, floats and plans 48 blocks, and a flying move control that turns 20 and
    /// hovers. No goals: its mind is its brain's.
    fn register_goals(&self, setup: &mut Setup) {
        setup.config.navigation = Settings::of(Kind::Flying);
        setup.config.navigation.flags.can_open_doors = false;
        setup.config.navigation.set_can_float(true);
        setup.config.navigation.set_required_path_length(48.0);
        setup.config.move_kind = MoveKind::Flying {
            max_turn: 20,
            hovers: true,
        };
    }

    /// `FlyingMoveControl.tick()` and its `NoGravity`, which a hovering one sets as it
    /// first steers and never clears.
    fn tick_move_control(&self, ai: &mut Ai<'_, '_>) {
        let steering = ai.tick.mob.body.controls.movement.operation == Operation::MoveTo;
        controls::tick_move(ai);
        if steering {
            update(&mut ai.tick.mob, |own| own.no_gravity = true);
        }
    }

    fn brain(&self) -> Option<&'static Provider> {
        Some(&PROVIDER)
    }

    /// `Allay.customServerAiStep`: the brain's tick and its activity's update.
    fn custom_server_ai_step(&self, tick: &mut MobTick<'_>) {
        brain::run(tick, |brain, _| update_activity(brain));
    }

    /// `Allay.tick()` on the server after the living one: a panicking allay stops
    /// dancing.
    fn after_tick(&self, tick: &mut MobTick<'_>) {
        if read(&tick.mob).dancing && panicking(&tick.mob, tick.lived) {
            brain_seams::set_dancing(&mut tick.mob, false, true);
        }
    }

    /// `Allay.travel`: `travelFlying` at its speed in the air, 0.02 in water and lava.
    fn travel(&self, tick: &MobTick<'_>) -> Travel {
        Travel::Flying {
            water: 0.02,
            lava: 0.02,
            air: tick.mob.body.inputs.speed,
        }
    }

    /// `Allay.aiStep` after the living one: the pick-up (`Mob.aiStep`'s), then every
    /// tenth tick a point of health; every twentieth, the end of a dance it should stop;
    /// and its duplication's cooldown counted down.
    fn after_ai_step(&self, tick: &mut MobTick<'_>) {
        pick_up(tick);
        let tick_count = tick.tick_count.wrapping_add(1);
        if !tick.mob.body.vitals.dying() && tick_count.is_multiple_of(10) {
            super::tamable::heal(&mut tick.mob, tick.kind, 1.0);
        }
        let own = read(&tick.mob);
        if own.dancing && should_stop_dancing(tick, &own) && tick_count.is_multiple_of(20) {
            update(&mut tick.mob, |own| {
                own.dancing = false;
                own.jukebox = None;
            });
        }
        // `updateDuplicationCooldown()`: down by one, `canDuplicate` again at nothing.
        let next = tick.lived.saturating_add(1);
        update(&mut tick.mob, |own| {
            if own.cooldown_until.is_some() && own.cooldown(next) == 0 {
                own.cooldown_until = None;
            }
        });
    }

    fn mob_interact(&self, used: &mut Used<'_>) -> Interaction {
        mob_interact(used)
    }

    /// `Allay.hurtServer`: the player it likes does it no harm.
    fn refuses_hurt(&self, mob: &Mob, source: &DamageSource) -> bool {
        let liked = liked_player(mob);
        liked.is_some()
            && source
                .cause
                .as_ref()
                .and_then(|cause| cause.player)
                .is_some_and(|player| Some(player) == liked)
    }

    /// `Allay.dropEquipment`: what it gathered, then what it holds.
    fn drop_equipment(&self, mob: &mut Mob) -> Vec<ItemStack> {
        let mut drops = Vec::new();
        let own = read(mob);
        if let Some(stack) = own.inventory.filter(|stack| stack.count > 0) {
            drops.push(stack);
        }
        update(mob, |own| own.inventory = None);
        if let Some(stack) = mob.body.equipment.slots[usize::from(slots::MAIN_HAND)].take() {
            drops.push(stack);
        }
        drops
    }

    fn ambient_sound(&self, mob: &Mob) -> Option<i32> {
        Some(
            if mob.body.equipment.slots[usize::from(slots::MAIN_HAND)].is_some() {
                sounds::ENTITY_ALLAY_AMBIENT_WITH_ITEM
            } else {
                sounds::ENTITY_ALLAY_AMBIENT_WITHOUT_ITEM
            },
        )
    }

    fn hurt_sound(&self, _: &Mob, _: &DamageSource) -> Option<i32> {
        Some(sounds::ENTITY_ALLAY_HURT)
    }

    fn death_sound(&self, _: &Mob) -> Option<i32> {
        Some(sounds::ENTITY_ALLAY_DEATH)
    }

    fn sound_volume(&self) -> f32 {
        0.4
    }

    fn shown(&self, mob: &Mob) -> Vec<(u8, DataShown, bool)> {
        let own = read(mob);
        vec![
            (
                NO_GRAVITY_INDEX,
                DataShown::Boolean(own.no_gravity),
                !own.no_gravity,
            ),
            (DANCING_INDEX, DataShown::Boolean(own.dancing), !own.dancing),
            (
                CAN_DUPLICATE_INDEX,
                DataShown::Boolean(own.can_duplicate()),
                own.can_duplicate(),
            ),
        ]
    }
}
