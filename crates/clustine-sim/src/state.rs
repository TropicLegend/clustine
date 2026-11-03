//! What a region is, apart from its chunks, and how it changes from tick to tick.
//!
//! A region can be rebuilt from its chunks and a [`RegionState`]; the state after a tick
//! is the state before it with the tick's [`StateDelta`] applied. Both are serialisable,
//! so that a worker can keep them in the world store and another worker can carry on
//! with the region. See `docs/adr/0008-durable-regions-and-resuming.md`, section 1.

use std::collections::BTreeMap;

use clustine_world::{BlockPos, ChunkPos, EdgeId, EntityId, EntityIds, PlayerId};
use serde::{Deserialize, Serialize};

use crate::api::{Durable, EnderChest, Inventory, ItemStack, Keys, MenuKind, Pose, VehicleMove};

/// Everything a region knows apart from its chunks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegionState {
    /// The number of the last tick; 0 before the first.
    pub tick: u64,
    /// The region's block of entity ids, issued once and the region's for life.
    pub entity_ids: EntityIds,
    /// The next entity id to give out. Once it is past the block, joining players are
    /// refused.
    pub next_entity_id: EntityId,
    pub players: BTreeMap<PlayerId, PlayerState>,
    /// The edges the region knows.
    pub edges: BTreeMap<EdgeId, EdgeState>,
    /// The stays the region has given out and holds until the world store has said
    /// where each is to enter: at most one for a player. See
    /// `docs/adr/0020-one-stay-per-player.md`, section 4, step 3.
    pub entering: BTreeMap<PlayerId, EnteringState>,
    /// The chunks across a pinned line the region has asked to be lent and has not been,
    /// each with the tick it asked last, which it asks again twenty ticks after
    /// (`docs/adr/0027-lending-at-a-line.md`, section 2). Kept so that a crash before
    /// the grant loses no ask.
    pub lends_asked: BTreeMap<ChunkPos, u64>,
    /// The world's clock as of the last tick.
    pub clock: crate::api::WorldClock,
    /// The stays a portal sent here that wait to be placed: in the state, apart from
    /// the players in the world (`docs/groundwork/p-plan.md`, sections 16.7 and 16.8).
    pub waiting: BTreeMap<PlayerId, WaitingState>,
    /// The entities a portal sent here that wait to be placed, each under the number it
    /// was filed with, taken in the commit that put them here.
    pub waiting_travellers: Vec<WaitingTraveller>,
    /// The End's dragon fight, kept by the End's home region
    /// (`docs/groundwork/p-plan.md`, section 15.2); boxed, as one region of a world
    /// has it.
    pub dragon_fight: Option<Box<clustine_world::dragon::DragonFight>>,
}

/// A stay a portal sent to this region that waits to be placed: what it arrived with,
/// the edge it came through, and the inputs of it that have nothing to do with where the
/// player is, kept to be applied at the placing (`docs/groundwork/p-plan.md`, section
/// 16.8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaitingState {
    pub transfer: crate::api::PlayerTransfer,
    pub edge: EdgeId,
    pub kept: Vec<crate::api::PlayerInput>,
    /// The portal query of the wait, once asked, and its answer, once it came.
    pub asked: Option<crate::api::QueryId>,
    pub answer: Option<Vec<BlockPos>>,
    /// The tick the wait began, for its limit on loading.
    pub since: u64,
    /// Whether the stay waits for the player to ask for their respawn: in the credits,
    /// or dead (`docs/groundwork/p-plan.md`, section 19.7).
    pub until_asked: bool,
}

/// An entity a portal sent to this region that waits to be placed: its number, which
/// the store filed it under, and its wait, as [`WaitingState`]'s.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaitingTraveller {
    pub id: clustine_world::EffectId,
    pub traveller: clustine_world::Traveller,
    pub asked: Option<crate::api::QueryId>,
    pub answer: Option<Vec<BlockPos>>,
    pub since: u64,
}

/// A stay the home region has given a joining player and holds as entering: the player
/// has an entity and is nowhere yet. See `docs/adr/0020-one-stay-per-player.md`,
/// section 4.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnteringState {
    pub entity_id: EntityId,
    pub name: String,
    /// The edge the join came through.
    pub edge: EdgeId,
    /// [`PlayerJoin::attempt`] of the join.
    ///
    /// [`PlayerJoin::attempt`]: crate::PlayerJoin::attempt
    pub attempt: u64,
    /// [`PlayerJoin::appearance`] of the join.
    ///
    /// [`PlayerJoin::appearance`]: crate::PlayerJoin::appearance
    pub appearance: crate::api::Appearance,
}

/// The poses a player's entity takes (`Pose`, by its ordinal in the game's enumeration,
/// which the entity data's `POSE` carries).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayerPose {
    #[default]
    Standing,
    FallFlying,
    Sleeping,
    Swimming,
    /// A Riptide trident's spin (`isAutoSpinAttack`), which nothing begins yet: Riptide
    /// is not built.
    SpinAttack,
    Crouching,
}

impl PlayerPose {
    /// The pose's ordinal in the game's `Pose`.
    pub fn ordinal(self) -> i32 {
        match self {
            Self::Standing => 0,
            Self::FallFlying => 1,
            Self::Sleeping => 2,
            Self::Swimming => 3,
            Self::SpinAttack => 4,
            Self::Crouching => 5,
        }
    }
}

/// What the region decides of how a player's entity is posed, which the game keeps in
/// the entity's data and the others are shown: the pose (`Player.updatePlayerPose`) and
/// the shared flags of swimming (bit 4, `Entity.updateSwimming`) and gliding (bit 7,
/// `Player.tryToStartFallFlying`, `LivingEntity.updateFallFlying`). It goes wherever the
/// player goes, so that a crossing changes nothing of what is shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Posture {
    pub pose: PlayerPose,
    pub swimming: bool,
    pub fall_flying: bool,
}

/// What the game's `ServerPlayerGameMode` keeps of a survival player's dig, field for
/// field (n5 `ServerPlayerGameMode`, its constructor @0-60: `destroyPos` and
/// `delayedDestroyPos` `BlockPos.ZERO`, `destroyDirection` `DOWN`, `lastSentState` -1;
/// `docs/groundwork/v-plan.md`, section 4.5). `start` and `delayed_start` are the
/// player's `gameTicks` of the moment. It goes wherever the player goes, so that a dig
/// survives a crossing, a merge and a split; it is not in the place, so a crash in
/// mid-dig loses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dig {
    /// `isDestroyingBlock`.
    pub is_destroying: bool,
    /// `destroyPos`.
    pub destroy_pos: BlockPos,
    /// `destroyDirection`, the face the level events of the dig name.
    pub direction: crate::api::Face,
    /// `destroyProgressStart`.
    pub start: i32,
    /// `hasDelayedDestroy`.
    pub has_delayed: bool,
    /// `delayedDestroyPos`.
    pub delayed_pos: BlockPos,
    /// `delayedTickStart`.
    pub delayed_start: i32,
    /// `lastSentState`.
    pub last_sent: i32,
}

impl Default for Dig {
    fn default() -> Self {
        Self {
            is_destroying: false,
            destroy_pos: BlockPos::new(0, 0, 0),
            direction: crate::api::Face::Bottom,
            start: 0,
            has_delayed: false,
            delayed_pos: BlockPos::new(0, 0, 0),
            delayed_start: 0,
            last_sent: -1,
        }
    }
}

/// A player's dig as a transfer carries it: their `gameTicks` as the region that lets
/// them go counted it at the end of its tick, and the dig, whose ticks are of that
/// count.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Digging {
    pub game_ticks: i32,
    pub dig: Dig,
}

/// A player's most health (`Player`'s `MAX_HEALTH` of 20).
pub const PLAYER_MAX_HEALTH: f32 = 20.0;

/// A player in the region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerState {
    pub entity_id: EntityId,
    pub name: String,
    pub pose: Pose,
    /// Everything the player carries, numbered as the game's `Inventory` numbers it.
    pub inventory: Inventory,
    /// The hotbar slot whose item the player holds.
    pub selected_slot: u8,
    /// What their ender chest holds.
    pub ender: EnderChest,
    /// The number of the last input that was applied.
    pub last_input: u64,
    /// The highest sequence number of the player's own actions on blocks of this region
    /// that has been handled, if any. Actions passed on to another region are not
    /// covered: they are handled when a [`Durable::RemoteDone`] says so.
    pub handled: Option<i32>,
    /// The edge the player belongs to: the one they joined or arrived through.
    pub edge: EdgeId,
    /// How often the stay has been handed on; see [`PlayerTransfer::hops`].
    ///
    /// [`PlayerTransfer::hops`]: crate::PlayerTransfer::hops
    pub hops: u32,
    /// Whether the player flies; see [`PlayerTransfer::flying`].
    ///
    /// [`PlayerTransfer::flying`]: crate::PlayerTransfer::flying
    pub flying: bool,
    /// The attempt of the join that began the stay, until the first input of the stay
    /// is applied; see [`PlayerTransfer::attempt`].
    ///
    /// [`PlayerTransfer::attempt`]: crate::PlayerTransfer::attempt
    pub attempt: Option<u64>,
    /// What the player rides (`docs/groundwork/n5-plan.md`).
    pub riding: Option<PlayerRiding>,
    /// The keys the player holds, as their client last said.
    pub keys: Keys,
    /// The moves of their vehicle the player's client sent while they waited for it,
    /// oldest first, at most [`KEPT_MOVES`]: applied in order when they are seated
    /// (`docs/groundwork/n5-plan.md`, section 7 item 4).
    pub kept: Vec<VehicleMove>,
    /// The menu the player has open, if any. A hand-over closes it, so it is in no
    /// transfer (`docs/groundwork/n6-plan.md`, section 6 items 1 and 2).
    pub menu: Option<Menu>,
    pub experience: crate::api::Experience,
    /// The tick in which their attack strength was last reset (`attackStrengthTicker`
    /// counts the ticks since), if it was: an arrival and a merge put it on the region's
    /// ticks. None is full strength, as a player who joins has (a named difference: the
    /// game's ticker begins at 0 then, a quarter of a second of weak hits).
    pub attack_reset: Option<u64>,
    /// The bed they sleep in, if they do.
    pub sleeping: Option<crate::api::Sleep>,
    /// Where a bed set their respawn point.
    pub respawn: Option<crate::api::Respawn>,
    /// Where they last died, the level and the block (`Player.lastDeathLocation`;
    /// `docs/groundwork/p-plan.md`, section 19.9), which every login, respawn and change
    /// of level tells their client, and the recovery compass points to.
    pub last_death: Option<(clustine_world::Dimension, clustine_world::BlockPos)>,
    /// Their game mode and the one before it (`docs/groundwork/v-plan.md`, section 4.1).
    pub mode: clustine_world::PlayerMode,
    /// The use of an item under way (`docs/groundwork/n9-plan.md`, section 5.5).
    pub using: Option<crate::api::Using>,
    /// The item groups' cooldowns, on the region's ticks; a merge moves them onto the
    /// survivor's.
    pub cooldowns: Vec<crate::api::Cooldown>,
    /// Their effects, each level's end on the region's ticks (`docs/groundwork/n9-plan.md`,
    /// section 5.4), so that a state changes only when they do; a merge moves them onto
    /// the survivor's ticks.
    pub effects: Vec<KeptEffect>,
    /// The tick their time since rest counts from.
    pub rested: u64,
    /// Where their raid omen began (`raidOmenPosition`), where it has; the raid starts
    /// there at its end (`docs/groundwork/n10-plan.md`, section 9.5).
    pub raid_omen_position: Option<BlockPos>,
    /// Their portal clock and cooldown.
    pub portal: clustine_world::PortalState,
    /// Where a portal sends them while they wait in this region to be placed
    /// (`docs/groundwork/p-plan.md`, section 15.4).
    pub travel: Option<clustine_world::Travel>,
    pub seen_credits: bool,
    /// Whether they watch the credits, hidden and still (the plan, section 5.3).
    pub in_credits: bool,
    /// Their health, which only the void takes yet (the plan, sections 19.5 and 19.6),
    /// [`PLAYER_MAX_HEALTH`] at most.
    pub health: f32,
    /// Whether they are dead and shown the respawn screen, their stay kept until they
    /// ask to respawn.
    pub dead: bool,
    /// How their client asked to be drawn, and their pose as the region decided it.
    pub appearance: crate::api::Appearance,
    pub posture: Posture,
    /// The region's tick in whose end the player's `gameTicks` was 0: the count is the
    /// region's tick less this (`docs/groundwork/v-plan.md`, section 4.5), so that a
    /// state changes only when the dig does. A merge moves it onto the survivor's ticks.
    pub game_origin: i64,
    /// Their dig.
    pub dig: Dig,
    /// What their own inventory screen holds of its own (window 0), which stays with
    /// them across a hand-over as the game never closes it (`docs/groundwork/v-plan.md`,
    /// section 4.7).
    pub crafting: Crafting,
    /// What their body keeps of survival (V4, `docs/groundwork/v-plan.md`, section 4.3
    /// and 4.4): food, air, fire, freezing, falling, absorption, the hurt cooldown, who
    /// hurt them last.
    pub vitals: Vitals,
}

/// The air a player has at most and starts with (`Entity.TOTAL_AIR_SUPPLY`,
/// `LivingEntity.getMaxAirSupply @0`).
pub const PLAYER_MAX_AIR: i32 = 300;

/// What a player's body keeps of survival beyond their health and effects (V4,
/// `docs/groundwork/v-plan.md`, sections 4.2 to 4.4; the build design's sections 7 and
/// 8). Ticks are the region's, signed, so that a merge, a hand-over and the stored place
/// move them all by [`Vitals::moved_by`]: a transfer and a place carry them on tick 0 (as
/// ticks left, or since), the region that takes them on its own. Counters that change
/// only at an event are kept as the tick of their end, so that a state changes only when
/// something happens; the fire, the air and the freezing change every tick while they
/// act, as the game's do.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Vitals {
    pub food: FoodData,
    /// `Entity.airSupply`, [`PLAYER_MAX_AIR`] at most; below 0 down to −20 before a
    /// drowning hit.
    pub air: i32,
    /// `Entity.remainingFireTicks`: burning while above 0; −20 after leaving fire, rising
    /// by one a tick in fire before the player ignites (`Player.getFireImmuneTicks`).
    pub fire: i32,
    /// `Entity.TicksFrozen`, in powder snow.
    pub frozen: i32,
    /// `Entity.fallDistance`, from the client's moves.
    pub fall_distance: f64,
    /// The kept copy of `LivingEntity.absorptionAmount`, written at the end of the tick
    /// as the health is; the region's `Effects.absorption` is the truth in memory.
    pub absorption: f32,
    /// The tick the damage cooldown (`LivingEntity.damageCooldownTime`, 20 after a full
    /// hit) ends, if it runs. Not saved by the game: left out of the place.
    pub cooldown_until: Option<i64>,
    /// `LivingEntity.lastHurt`, the damage the cooldown's second half compares with. Not
    /// saved.
    pub last_hurt: f32,
    /// Who hurt them last, and until when it counts (`lastHurtByMob` with its 100 ticks,
    /// `lastHurtByPlayer`): tamed wolves defend against it, and a death names it.
    pub last_hurt_by: Option<LastHurtBy>,
    /// Where a wind charge last threw them, which a fall is measured from
    /// (`currentImpulseImpactPos`, `currentImpulseContextResetGraceTime`). Nothing sets
    /// it yet: there are no wind charges.
    pub impulse: Option<Impulse>,
    /// The motion the server keeps for them (`getDeltaMovement`): set by knockback and
    /// pushes, slowed ×0.98 a tick, its y cut by a landing; knockback starts from it. Not
    /// saved.
    pub motion: [f64; 3],
    /// The arrows stuck in them (`DATA_ARROW_COUNT_ID`). Not saved.
    pub arrows: i32,
    /// The combat tracker's entries for the death message
    /// (`CombatTracker.recordDamage`). Not saved: lost at a rejoin, as the game's.
    pub combat: Vec<CombatEntry>,
}

impl Default for Vitals {
    fn default() -> Self {
        Self {
            food: FoodData::default(),
            air: PLAYER_MAX_AIR,
            fire: 0,
            frozen: 0,
            fall_distance: 0.0,
            absorption: 0.0,
            cooldown_until: None,
            last_hurt: 0.0,
            last_hurt_by: None,
            impulse: None,
            motion: [0.0; 3],
            arrows: 0,
            combat: Vec::new(),
        }
    }
}

impl Vitals {
    /// The same, every tick moved by `by`: onto a survivor's ticks at a merge, onto tick
    /// 0 for a transfer or a place, onto the arriving region's from there.
    pub fn moved_by(&self, by: i128) -> Self {
        let moved = |tick: i64| i64::try_from(i128::from(tick) + by).unwrap_or(i64::MAX);
        let mut vitals = self.clone();
        vitals.food.timer_from = vitals.food.timer_from.map(moved);
        vitals.cooldown_until = vitals.cooldown_until.map(moved);
        if let Some(by) = &mut vitals.last_hurt_by {
            by.until = moved(by.until);
        }
        if let Some(impulse) = &mut vitals.impulse {
            impulse.grace_until = moved(impulse.grace_until);
        }
        for entry in &mut vitals.combat {
            entry.at = moved(entry.at);
        }
        vitals
    }

    /// What of them the stored place keeps: what the game saves of a player
    /// (`Player.addAdditionalSaveData`, `LivingEntity.addAdditionalSaveData`), without
    /// the cooldown, the last hurt, the motion, the arrows and the combat tracker, which
    /// a rejoin begins without. Who hurt them last is left out too (named: the game keeps
    /// a mob's UUID for its 100 ticks; a rejoin's entity ids are new).
    pub fn kept_in_place(&self) -> Self {
        Self {
            cooldown_until: None,
            last_hurt: 0.0,
            last_hurt_by: None,
            motion: [0.0; 3],
            arrows: 0,
            combat: Vec::new(),
            ..self.clone()
        }
    }
}

/// `FoodData`: the food level (20 at most), the saturation (at most the food), the
/// exhaustion (to 40) and the tick its one timer began, `None` at rest (the game's
/// `tickTimer` of 0, reset in every tick no branch applies).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FoodData {
    pub food: i32,
    pub saturation: f32,
    pub exhaustion: f32,
    pub timer_from: Option<i64>,
}

impl Default for FoodData {
    /// A new player's (`FoodData`'s fields): 20, 5, none.
    fn default() -> Self {
        Self {
            food: 20,
            saturation: 5.0,
            exhaustion: 0.0,
            timer_from: None,
        }
    }
}

/// Who hurt a player last: the entity, a player's UUID where it was one, and the tick
/// until which it counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastHurtBy {
    pub entity: EntityId,
    pub player: Option<uuid::Uuid>,
    pub until: i64,
}

/// Where a wind charge threw a player, and the tick its grace ends.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Impulse {
    pub at: [f64; 3],
    pub grace_until: i64,
}

/// One hurt the combat tracker keeps (`CombatEntry`): its type, how much, who did it,
/// where they were falling from, and the tick.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CombatEntry {
    pub damage_type: String,
    pub amount: f32,
    pub killer: Option<crate::api::Killer>,
    /// `CombatTracker.getFallLocation` at the hurt (a ladder, vines, water …), by the
    /// game's id, if they were falling from one.
    pub fall_location: Option<String>,
    pub fall_distance: f32,
    pub at: i64,
}

/// The player's own inventory screen's (`InventoryMenu`'s) state beyond the inventory:
/// the 2×2 grid, row by row, its result (`ResultContainer`, set only where the game's
/// `slotChangedCraftingGrid` runs), the stack on the cursor and a drag under way. Its
/// serial is 0, which no input has.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Crafting {
    pub grid: [Option<ItemStack>; 4],
    pub result: Option<ItemStack>,
    pub carried: Option<ItemStack>,
    pub drag: Option<Drag>,
}

impl Crafting {
    /// What a place keeps of it: all but a drag, which no client that enters by the
    /// place has under way.
    pub fn kept(&self) -> Self {
        Self {
            drag: None,
            ..self.clone()
        }
    }

    /// Whether nothing is in it.
    pub fn is_empty(&self) -> bool {
        self.grid.iter().all(Option::is_none)
            && self.result.is_none()
            && self.carried.is_none()
            && self.drag.is_none()
    }
}

/// A level of an effect as a region's state keeps it: the effect by name, its amplifier,
/// the region's tick it ends in (none for ever) and the instance's flags.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptEffect {
    pub effect: String,
    pub amplifier: u8,
    pub end: Option<u64>,
    pub ambient: bool,
    pub visible: bool,
    pub show_icon: bool,
}

/// A menu a player has open: what it shows, the stack on the cursor and a drag under
/// way. What the region last reported of it is not here: a restored region reports it
/// whole (`docs/groundwork/n6-plan.md`, section 6 item 1 and section 7 item 3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Menu {
    /// The number of the input that opened it, which the edge and the client's clicks
    /// name it by (section 7 item 2).
    pub serial: u64,
    pub of: MenuOf,
    pub kind: MenuKind,
    /// The stack the player carries on the cursor (`AbstractContainerMenu.carried`).
    pub carried: Option<ItemStack>,
    /// A drag under way (`quickcraftStatus`, `quickcraftType`, `quickcraftSlots`).
    pub drag: Option<Drag>,
    /// A merchant's container (`MerchantContainer`): its two payments, its result, and
    /// the trade picked last (`docs/groundwork/n10-plan.md`, section 8.2); none for any
    /// other menu.
    pub trade: Option<Box<TradeSlots>>,
    /// A crafting table's own slots (`ResultContainer` and `TransientCraftingContainer`):
    /// the result, then the 3×3 grid row by row (V3c); none for any other menu.
    pub grid: Option<Box<[Option<ItemStack>; 10]>>,
}

/// What a merchant's window holds of its own: the two payments, the result, and the
/// index of the trade the player picked last (`selectionHint`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradeSlots {
    pub payments: [Option<ItemStack>; 2],
    pub result: Option<ItemStack>,
    pub selected: Option<i32>,
}

/// What a menu shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MenuOf {
    /// A container block at `at`, and the other half of a double chest; slots 0 to 26
    /// are those of `at`, which is the right half (`ChestBlock.getBlockType`).
    Block {
        at: BlockPos,
        other: Option<BlockPos>,
    },
    /// The player's own ender chest, opened at the block `at`, whose openers it counts.
    Ender { at: BlockPos },
    /// A container vehicle: a chest boat, a chest cart or a hopper cart.
    Entity(EntityId),
    /// A villager's or a wandering trader's trades.
    Merchant(EntityId),
    /// The player's own inventory screen (window 0): only ever the menu a click of it
    /// runs over, never the player's open menu.
    Inventory,
    /// A crafting table at `at`, whose grid is the menu's own (V3c).
    CraftingTable { at: BlockPos },
}

/// A drag of the carried stack over slots (the game's quick craft), between its start
/// and its end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Drag {
    pub kind: DragKind,
    /// The slots dragged over so far, in the order they were added.
    pub slots: Vec<u16>,
}

/// How a drag shares out the carried stack (`quickcraftType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DragKind {
    /// Evenly over the slots (a left drag, `QUICKCRAFT_TYPE_CHARITABLE`).
    Split,
    /// One into each (a right drag, `QUICKCRAFT_TYPE_GREEDY`).
    One,
    /// A full stack into each, for a creative player (a middle drag, `CLONE`).
    Clone,
}

/// The most moves of a vehicle kept for a player who waits for it.
pub const KEPT_MOVES: usize = 20;

/// The vehicle a player sits on, and the chunk of its unit, which is the player's
/// holding chunk while they ride: where they belong is judged by it, not by their own
/// position (ADR-0028, section 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerRiding {
    pub vehicle: EntityId,
    pub chunk: ChunkPos,
    /// What the game's connection keeps for the check of the moves of the vehicle the
    /// player drives (`docs/groundwork/n5-brief.md`, finding 12 and section 2.3).
    pub check: VehicleCheck,
    /// For a player handed over to their unit's region before the unit came, the ticks
    /// they wait for it still: seated, vehicle pending (ADR-0028, section 5;
    /// `docs/groundwork/n5-plan.md`, sections 7 and 8). `None` once seated.
    pub pending: Option<u16>,
}

/// The state the game's `ServerGamePacketListenerImpl` keeps for a vehicle's moves:
/// where the root was at the end of the player's last tick, if the player drove it then
/// (`lastVehicle`, `vehicleFirstGood`, and `vehicleLastGood`, which each accepted move
/// carries on), whether the client's last move held it in the air and for how many
/// ticks it has done so, and how long ago a passenger who does not drive was last put
/// back (`vehiclePositionLastResetAt`, as an age: regions number their ticks apart).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VehicleCheck {
    /// `vehicleFirstGood` and `vehicleLastGood`, set while the player drives their root
    /// (`lastVehicle` is the root): `None` while they do not.
    pub good: Option<[[clustine_world::Double; 3]; 2]>,
    /// `clientVehicleIsFloating`.
    pub floating: bool,
    /// `aboveGroundVehicleTickCount`.
    pub floating_ticks: u32,
    /// The player's ticks since a passenger's move was last put back, counted up to
    /// [`VehicleCheck::LONG_AGO`] and no further, so that the record stops changing.
    pub since_reset: u8,
}

impl VehicleCheck {
    /// More ticks since the last reset than the check asks about (`> 20`).
    pub const LONG_AGO: u8 = 21;
}

impl Default for VehicleCheck {
    fn default() -> Self {
        Self {
            good: None,
            floating: false,
            floating_ticks: 0,
            since_reset: Self::LONG_AGO,
        }
    }
}

/// What a region keeps for an edge.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct EdgeState {
    /// Which start of the edge the region knows.
    pub start: u64,
    /// Since when the region has known the edge without a break: the number of the tick
    /// that made this state, when it noted an edge it did not know or reset one for a
    /// higher start. An edge says the number it was last told when it says hello, and
    /// is taken to share the region's numbering of messages and outbox entries only if
    /// it is this one. See `docs/adr/0012-the-tick-on-chunks.md`, section 2.6.
    pub since: u64,
    /// The number of the last message of that edge that was applied.
    pub applied: u64,
    /// The number of the last outbox entry made; the next one is numbered one higher.
    pub sent: u64,
    /// The entries that the edge has not confirmed yet, by number.
    pub outbox: BTreeMap<u64, Durable>,
}

impl RegionState {
    /// The state of a region that has never run, with `entity_ids` as its block.
    pub fn new(entity_ids: EntityIds) -> Self {
        Self {
            tick: 0,
            entity_ids,
            next_entity_id: entity_ids.first,
            players: BTreeMap::new(),
            edges: BTreeMap::new(),
            entering: BTreeMap::new(),
            lends_asked: BTreeMap::new(),
            clock: Default::default(),
            waiting: BTreeMap::new(),
            waiting_travellers: Vec::new(),
            dragon_fight: None,
        }
    }

    /// Makes this the state after the tick that `delta` came from, given that it is the
    /// state before that tick.
    pub fn apply(&mut self, delta: &StateDelta) {
        // Ticks that changed only their number are not stored, so the clock moves on by
        // as many ticks as the delta is after the state.
        match delta.clock {
            Some(clock) => self.clock = clock,
            None => {
                let ticks = delta.tick.saturating_sub(self.tick);
                self.clock.game_time += ticks;
                self.clock.day += ticks;
            }
        }
        self.tick = delta.tick;
        if let Some(next) = delta.next_entity_id {
            self.next_entity_id = next;
        }
        for (id, player) in &delta.players {
            match player {
                Some(player) => {
                    self.players.insert(*id, player.clone());
                }
                None => {
                    self.players.remove(id);
                }
            }
        }
        for (id, entering) in &delta.entering {
            match entering {
                Some(entering) => {
                    self.entering.insert(*id, entering.clone());
                }
                None => {
                    self.entering.remove(id);
                }
            }
        }
        for (position, asked) in &delta.lends_asked {
            match asked {
                Some(tick) => {
                    self.lends_asked.insert(*position, *tick);
                }
                None => {
                    self.lends_asked.remove(position);
                }
            }
        }
        for (id, waiting) in &delta.waiting {
            match waiting {
                Some(waiting) => {
                    self.waiting.insert(*id, waiting.clone());
                }
                None => {
                    self.waiting.remove(id);
                }
            }
        }
        if let Some(travellers) = &delta.waiting_travellers {
            self.waiting_travellers.clone_from(travellers);
        }
        if let Some(fight) = &delta.dragon_fight {
            self.dragon_fight.clone_from(fight);
        }
        for (id, change) in &delta.edges {
            let Some(change) = change else {
                self.edges.remove(id);
                continue;
            };
            let edge = self.edges.entry(*id).or_default();
            edge.start = change.start;
            edge.since = change.since;
            edge.applied = change.applied;
            edge.sent = change.sent;
            if change.cleared {
                edge.outbox.clear();
            }
            edge.confirm(change.confirmed);
            edge.outbox.extend(
                change
                    .added
                    .iter()
                    .map(|(number, entry)| (*number, entry.clone())),
            );
        }
    }
}

impl EdgeState {
    /// Drops the outbox entries up to `number`. Returns whether there were any.
    pub(crate) fn confirm(&mut self, number: u64) -> bool {
        let kept = match number.checked_add(1) {
            Some(first_kept) => self.outbox.split_off(&first_kept),
            None => BTreeMap::new(),
        };
        let dropped = !self.outbox.is_empty();
        self.outbox = kept;
        dropped
    }
}

/// Everything that changed in a [`RegionState`] in one tick, such that
/// `state_before.apply(&delta) == state_after`. It names only what changed: the players
/// and edges that did, and of an outbox the entries added and dropped.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StateDelta {
    /// The number of the tick.
    pub tick: u64,
    /// The world's clock after the tick, when it took a later one the store published;
    /// otherwise each tick moved it on by one, which a delta need not say, so that a
    /// tick at rest still changes only its number.
    pub clock: Option<crate::api::WorldClock>,
    /// The next entity id to give out, if it changed.
    pub next_entity_id: Option<EntityId>,
    /// Each player that changed, in the order of the players, as they are after the tick:
    /// `None` for one who is no longer in the region.
    pub players: Vec<(PlayerId, Option<PlayerState>)>,
    /// Each edge that changed, in the order of the edges: `None` for one the region has
    /// forgotten.
    pub edges: Vec<(EdgeId, Option<EdgeDelta>)>,
    /// Each entering stay that changed, in the order of the players, as it is after
    /// the tick: `None` for a player of whom the region holds none any more.
    pub entering: Vec<(PlayerId, Option<EnteringState>)>,
    /// Each ask for a lend that changed, ascending, as it is after the tick: `None` for
    /// one that ended.
    pub lends_asked: Vec<(ChunkPos, Option<u64>)>,
    /// Each waiting stay that changed, in the order of the players, as it is after the
    /// tick: `None` for one that was placed or ended (`docs/groundwork/p-plan.md`,
    /// section 16.7).
    pub waiting: Vec<(PlayerId, Option<WaitingState>)>,
    /// The waiting travellers whole, if they changed.
    pub waiting_travellers: Option<Vec<WaitingTraveller>>,
    /// The dragon fight, if it changed (none: the region keeps none).
    pub dragon_fight: Option<Option<Box<clustine_world::dragon::DragonFight>>>,
}

impl StateDelta {
    /// Whether nothing changed in the tick but its number.
    pub fn changes_only_the_tick(&self) -> bool {
        self.clock.is_none()
            && self.next_entity_id.is_none()
            && self.players.is_empty()
            && self.edges.is_empty()
            && self.entering.is_empty()
            && self.lends_asked.is_empty()
            && self.waiting.is_empty()
            && self.waiting_travellers.is_none()
            && self.dragon_fight.is_none()
    }
}

/// What changed of an edge the region knows after the tick. Applied in this order: the
/// numbers are set, the outbox is cleared if `cleared`, the entries up to `confirmed` are
/// dropped, and `added` is added.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeDelta {
    pub start: u64,
    /// [`EdgeState::since`] after the tick.
    pub since: u64,
    pub applied: u64,
    pub sent: u64,
    /// Whether the outbox the edge had before the tick was dropped as a whole, as when
    /// the edge was reset, or forgotten and then started again.
    pub cleared: bool,
    /// The outbox entries up to this number were dropped; 0 if none were.
    pub confirmed: u64,
    /// The entries made in the tick, in the order of their numbers.
    pub added: Vec<(u64, Durable)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A body with every tick it keeps set, at a region's tick 1000.
    fn counting() -> Vitals {
        Vitals {
            food: FoodData {
                timer_from: Some(990),
                ..FoodData::default()
            },
            cooldown_until: Some(1012),
            last_hurt: 6.0,
            last_hurt_by: Some(LastHurtBy {
                entity: EntityId(7),
                player: None,
                until: 1080,
            }),
            impulse: Some(Impulse {
                at: [1.0, 2.0, 3.0],
                grace_until: 1005,
            }),
            motion: [0.1, 0.2, 0.3],
            arrows: 2,
            combat: vec![CombatEntry {
                damage_type: "minecraft:fall".to_owned(),
                amount: 3.0,
                killer: None,
                fall_location: None,
                fall_distance: 6.0,
                at: 995,
            }],
            ..Vitals::default()
        }
    }

    #[test]
    fn every_tick_a_body_keeps_moves_by_the_same_and_moves_back() {
        let vitals = counting();
        // Onto tick 0 at a hand-over, then onto the next region's 40.
        let carried = vitals.moved_by(-1000);
        assert_eq!(carried.food.timer_from, Some(-10));
        assert_eq!(carried.cooldown_until, Some(12));
        assert_eq!(carried.last_hurt_by.map(|by| by.until), Some(80));
        assert_eq!(carried.impulse.map(|impulse| impulse.grace_until), Some(5));
        assert_eq!(carried.combat[0].at, -5);
        let arrived = carried.moved_by(40);
        assert_eq!(arrived.cooldown_until, Some(52));
        assert_eq!(arrived.moved_by(960), vitals);
    }

    #[test]
    fn a_place_keeps_what_the_game_saves_of_a_body_and_nothing_a_rejoin_begins_without() {
        let kept = counting().kept_in_place();
        assert_eq!(kept.food, counting().food);
        assert_eq!(kept.impulse, counting().impulse);
        assert_eq!(kept.cooldown_until, None);
        assert_eq!(kept.last_hurt, 0.0);
        assert_eq!(kept.last_hurt_by, None);
        assert_eq!(kept.motion, [0.0; 3]);
        assert_eq!(kept.arrows, 0);
        assert!(kept.combat.is_empty());
    }

    #[test]
    fn an_open_menu_with_a_drag_under_way_reads_back_as_it_was_written() {
        let menus = [
            Menu {
                serial: 41,
                of: MenuOf::Block {
                    at: BlockPos::new(3, -60, 7),
                    other: Some(BlockPos::new(4, -60, 7)),
                },
                kind: MenuKind::Rows6,
                carried: Some(ItemStack::new(12, 9)),
                drag: Some(Drag {
                    kind: DragKind::One,
                    slots: vec![53, 0, 27],
                }),
                trade: None,
                grid: None,
            },
            Menu {
                serial: u64::MAX,
                of: MenuOf::Ender {
                    at: BlockPos::new(-1, 70, -1),
                },
                kind: MenuKind::Rows3,
                carried: None,
                drag: None,
                trade: None,
                grid: None,
            },
            Menu {
                serial: 0,
                of: MenuOf::Entity(EntityId(1 << 40)),
                kind: MenuKind::Hopper,
                carried: None,
                drag: Some(Drag {
                    kind: DragKind::Clone,
                    slots: Vec::new(),
                }),
                trade: None,
                grid: None,
            },
        ];
        for menu in menus {
            let bytes = postcard::to_stdvec(&menu).expect("a menu serialises");
            assert_eq!(postcard::from_bytes::<Menu>(&bytes), Ok(menu));
        }
    }
}
