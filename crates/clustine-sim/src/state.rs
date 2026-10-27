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
}

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
            },
        ];
        for menu in menus {
            let bytes = postcard::to_stdvec(&menu).expect("a menu serialises");
            assert_eq!(postcard::from_bytes::<Menu>(&bytes), Ok(menu));
        }
    }
}
