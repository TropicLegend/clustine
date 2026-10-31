//! The write-ahead log: what regions committed and the world store has not yet folded
//! into a region's state file and the stored chunks.
//!
//! The log is a sequence of records, each framed as:
//!
//! | Field | Type |
//! |---|---|
//! | Length of the payload | u32 |
//! | CRC-32 of the payload | u32 |
//! | Payload | bytes |
//!
//! Every payload starts with the format version (u8) and the kind of record (u8). What
//! follows depends on the kind:
//!
//! | Kind | Record | Fields |
//! |---|---|---|
//! | 1 | [`LogRecord::Changes`], written before regions had a state | tick u64, epoch u64, changes |
//! | 2 | [`LogRecord::Commit`] | region u32, tick u64, epoch u64, changes, state length u32, state bytes |
//! | 3 | [`LogRecord::Opened`] | region u32, epoch u64, restored u64 |
//! | 4 | [`LogRecord::Granted`] | region u32, tick u64, chunks |
//! | 5 | [`LogRecord::Returned`] | region u32, chunks |
//! | 6 | [`LogRecord::Absorbed`] | region u32, epoch u64, absorbed u32, tick u64, state bytes |
//! | 7 | [`LogRecord::Split`] | region u32, epoch u64, tick u64, state bytes, part u32, part epoch u64, chunks, part state bytes |
//! | 8 | [`LogRecord::Commit`] with stay notes | the fields of kind 2, stays |
//! | 9 | [`LogRecord::Commit`] with records of chunks | the fields of kind 2, stays (whose count may be 0), ticks, marks taken (chunks), seams |
//! | 10 | [`LogRecord::Commit`] with effects | the fields of kind 9, whose counts may all be 0, effects, taken |
//! | 11 | [`LogRecord::EffectsDropped`] | numbers |
//! | 12 | [`LogRecord::EffectsForward`] | filed effects |
//! | 17 | [`LogRecord::ArrivalsDropped`] | numbers |
//! | 18 | [`LogRecord::ArrivalsForward`] | filed arrivals |
//! | 13 | [`LogRecord::Commit`] with block entities | the fields of kind 10, whose counts may all be 0, block entities |
//! | 14 | [`LogRecord::Commit`] with block events | the fields of kind 13, whose counts may all be 0, events |
//! | 15 | [`LogRecord::Commit`] with entity changes | the fields of kind 14, whose counts may all be 0, entity changes |
//! | 16 | [`LogRecord::Commit`] with arrivals | the fields of kind 14, a flag whether entity changes follow, they if so, arrivals, arrivals taken |
//! | 19 | [`LogRecord::Commit`] with claims | the fields of kind 16, whose arrivals may be none, claims |
//! | 20 | [`LogRecord::Commit`] with raids | the fields of kind 19, whose claims may be none, raids |
//!
//! where `changes` is a count (u32) followed, per change, by i32 x, i32 y, i32 z and a
//! u16 block state; `chunks` is a count (u32) followed, per chunk, by i32 x and i32 z;
//! and `bytes` is a length (u32) followed by that many bytes. All integers are
//! big-endian, and so are the floating-point numbers.
//!
//! A commit is written as kind 2 if it has no stay notes and as kind 8 if it has any, so
//! that a commit and its notes are on disk together or not at all; as kind 9 if it
//! carries records of chunks: the scheduled ticks of chunks whose ticks changed in its
//! tick, the chunks whose marks for post-processing were taken in it, or the seam
//! entries of chunks whose entries changed in it
//! (`docs/adr/0024-block-behaviour-in-a-region.md`, sections 4 to 6), whatever its
//! notes. `seams` is a count of chunks (u32), followed per chunk by i32 x, i32 z and the
//! chunk's entries whole, as `seams.rs` writes them.
//! A commit is written as kind 10 if it carries effects or takes any
//! (`docs/adr/0025-effects-between-regions.md`, sections 4 and 5), whatever else it
//! carries: `effects` are those the store keeps of what the tick made, each with when it
//! is due, numbered by the record's sequence and their place in it, and `taken` the
//! numbers of the effects the tick took. A commit is written as kind 13 if any of its
//! changes is to a block entity's data (`docs/adr/0026-redstone.md`, section 6): its
//! `changes` then hold the blocks alone, and `block entities` is a count (u32)
//! followed, per change, by the number of block changes before it (u32), i32 x, i32 y,
//! i32 z and its data as `bytes`, ascending by that number, so that the two together
//! are the tick's changes in the order it made them. A commit is written as kind 14 if
//! it carries the block events kept in chunks (the same record, section 4): `events` is
//! a count of chunks (u32), followed per chunk by i32 x, i32 z and the chunk's events
//! whole, as `kept_events.rs` writes them. A commit is written as kind 15 if it carries
//! changes of entities other than players (`docs/adr/0028-entities.md`, section 3), as
//! `entities.rs` writes them, which are not none. A commit is written as kind 19 if it
//! carries the claims on the points of interest of chunks whose claims changed
//! (`docs/groundwork/n10-plan.md`, section 3.2): `claims` is a count of chunks (u32), not
//! 0, followed per chunk by i32 x, i32 z and the chunk's claims whole, as `chunk.rs`
//! writes them (a count that may be 0, when the last was given back). A commit is written
//! as kind 20 if it carries the raids of chunks whose raids changed (section 9.1 of the same
//! plan): a count of chunks (u32), not 0, then per chunk i32 x, i32 z and the chunk's raids
//! whole, as `raids.rs` writes them (a count that may be 0). Kind 11 says that effects untaken were dropped,
//! and kind 12 carries untaken effects forward out of a segment that is let go, with
//! their numbers. `effects`, `numbers` and `filed effects` are as `effects.rs` writes
//! them.
//! `ticks` is a count of chunks (u32), followed per chunk by i32 x, i32 z and a count of
//! ticks (u32), and per tick by u8 x, i16 y and u8 z within the chunk, u8 1 for a block
//! and 2 for a fluid, u16 its id, i32 the delay left and i8 the priority: the chunk's
//! ticks whole, as the delays left at the end of the tick. `stays` is a count
//! (u32), which is not 0, followed, per note ([`LoggedStay`]), by:
//!
//! | Field | Type |
//! |---|---|
//! | Kind | u8: 1 `Entering`, 2 `Has`, 3 `Has` with a unit, 4 a taking |
//! | Player | u128 |
//! | Entity | u64: the stay's, or a taking's root |
//! | *`Has` only:* hops | u32 |
//! | … the place | see below |
//! | *Kind 3 only:* the unit | a count (u32), not 0, and each member as an entity, root first |
//!
//! and a place ([`LoggedPlace`]) is:
//!
//! | Field | Type |
//! |---|---|
//! | Position | f64 x, f64 y, f64 z |
//! | Yaw, pitch | f32, f32 |
//! | Flags | u8: bit 0 on-ground, bit 1 flying |
//! | Selected slot | u8 |
//! | Inventory | 41 times, in the order of the game's `Inventory` (0 to 8 the hotbar, 9 to
//! 35 the rest, 36 to 39 armour from the feet up, 40 the off hand): u8 0 for an empty
//! slot, or u8 1 and the stack as `stacks.rs` writes one (item i32, count i32, its
//! components) |
//! | Ender chest | 27 times, as the inventory's slots |
//!
//! See `docs/adr/0020-one-stay-per-player.md`, section 8.
//!
//! Kinds 4 to 7 are what decides which region holds which chunk and which regions there
//! are; see `docs/adr/0011-the-world-store-and-regions.md`.
//!
//! A process can die while appending, which leaves a partial record at the end. Reading
//! therefore stops at the first record that is incomplete or fails its checksum, and
//! reports how many bytes were valid.

use clustine_data::BlockState;
use clustine_world::poi::Claim;
use clustine_world::{
    Arrival, BlockPos, Change, ChunkPos, Dimension, Due, Effect, EffectId, Entity, EntityChange,
    FiledEffect, InChunk, ItemStack, KeptEvent, ScheduledTick, Seam, Ticked, Travel, Traveller,
};

use crate::bytes::Input;
use crate::stacks::{put_optional_stack, take_optional_stack};
use crate::{FORMAT_VERSION, FormatError};

const KIND_CHANGES: u8 = 1;
const KIND_COMMIT: u8 = 2;
const KIND_OPENED: u8 = 3;
const KIND_GRANTED: u8 = 4;
const KIND_RETURNED: u8 = 5;
const KIND_ABSORBED: u8 = 6;
const KIND_SPLIT: u8 = 7;
const KIND_COMMIT_WITH_STAYS: u8 = 8;
const KIND_COMMIT_WITH_TICKS: u8 = 9;
const KIND_COMMIT_WITH_EFFECTS: u8 = 10;
const KIND_EFFECTS_DROPPED: u8 = 11;
const KIND_EFFECTS_FORWARD: u8 = 12;
const KIND_COMMIT_WITH_ENTITIES: u8 = 13;
const KIND_COMMIT_WITH_EVENTS: u8 = 14;
const KIND_COMMIT_WITH_ENTITY_CHANGES: u8 = 15;
const KIND_COMMIT_WITH_ARRIVALS: u8 = 16;
const KIND_ARRIVALS_DROPPED: u8 = 17;
const KIND_ARRIVALS_FORWARD: u8 = 18;
const KIND_COMMIT_WITH_CLAIMS: u8 = 19;
const KIND_COMMIT_WITH_RAIDS: u8 = 20;
const KIND_TRADER_CHANCE: u8 = 21;
const KIND_COMMIT_WITH_TRAVELLERS: u8 = 22;
const KIND_TRAVELLERS_FORWARD: u8 = 23;

/// The kinds of a stay note.
const STAY_ENTERING: u8 = 1;
const STAY_HAS: u8 = 2;
const STAY_HAS_WITH_UNIT: u8 = 3;
const STAY_TOOK: u8 = 4;

/// The flags of a place.
const ON_GROUND: u8 = 1;
const FLYING: u8 = 2;

/// Bytes a stay note takes in a payload at the least.
const STAY_LENGTH: usize = 25;

/// The number of slots in a player's inventory, as the game's `Inventory` numbers them.
pub const INVENTORY_SLOTS: usize = 41;

/// The slots of a player's ender chest in a place.
pub const ENDER_SLOTS: usize = 27;

/// Bytes of the length and the checksum in front of every payload.
const FRAME_HEADER_LENGTH: usize = 8;

/// A payload longer than this is taken for damage, not for a record.
const MAX_PAYLOAD_LENGTH: usize = 64 * 1024 * 1024;

/// The longest a record may be as [`LogRecord::encode`] returns it, frame included. A
/// longer one is written like any other and then read as a record that was cut off,
/// which hides it and whatever follows it in its segment.
pub const MAX_RECORD_LENGTH: usize = FRAME_HEADER_LENGTH + MAX_PAYLOAD_LENGTH;

/// Bytes a block change takes in a payload.
const CHANGE_LENGTH: usize = 14;

/// Bytes a chunk takes in a payload.
const CHUNK_LENGTH: usize = 8;

/// What a region said of a stay in a commit, as the log keeps it: in plain numbers, so
/// that the layout is the store's own and does not change with what a region's state
/// looks like. See `docs/adr/0020-one-stay-per-player.md`, sections 3 and 8.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::large_enum_variant,
    reason = "a place carries a whole inventory, and notes are read and written a few at a time"
)]
pub enum LoggedStay {
    /// The home region has given `player` the stay `entity` and holds it as entering.
    Entering { player: u128, entity: u64 },
    /// The stay is in the region after the tick, or was let go in it, with `hops`
    /// hand-overs behind it and this place.
    Has {
        player: u128,
        entity: u64,
        hops: u32,
        place: LoggedPlace,
        /// The vehicle the player left the world on, with all its passengers, root first
        /// (`docs/groundwork/n7h-logout-on-a-mount.md`, section 3), which the record keeps
        /// until the player's next stay takes it.
        unit: Option<LoggedUnit>,
    },
    /// The home region took the unit whose root is `root` out of the player's record, to
    /// put it back under the player's next stay (the same design, section 5). No stay
    /// note: it is kept among the notes so that whatever counts and keeps notes keeps it.
    Took { player: u128, root: u64 },
}

/// A unit kept with a player: its members, root first. Two are equal if their bytes on
/// disk are, as places are.
#[derive(Debug, Clone)]
pub struct LoggedUnit(pub Vec<Entity>);

impl LoggedUnit {
    /// The unit's root, the vehicle the others ride.
    pub fn root(&self) -> &Entity {
        self.0.first().expect("a unit has its root")
    }

    fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        put_unit(&mut out, self);
        out
    }
}

impl PartialEq for LoggedUnit {
    fn eq(&self, other: &Self) -> bool {
        self.bytes() == other.bytes()
    }
}

impl Eq for LoggedUnit {}

/// Appends a unit: a count (u32), which is not 0, and each member as an entity.
pub(crate) fn put_unit(out: &mut Vec<u8>, unit: &LoggedUnit) {
    out.extend_from_slice(&(unit.0.len() as u32).to_be_bytes());
    for member in &unit.0 {
        crate::entities::put_entity(out, member);
    }
}

/// Reads a unit as [`put_unit`] wrote it.
pub(crate) fn take_unit(input: &mut Input<'_>) -> Result<LoggedUnit, FormatError> {
    let count = input.u32()? as usize;
    if count == 0 {
        return Err(FormatError::Corrupt("a unit without its root"));
    }
    // A forged count must not reserve more than the payload can hold.
    let mut members = Vec::with_capacity(count.min(input.0.len() / 64));
    for _ in 0..count {
        members.push(crate::entities::take_entity(input)?);
    }
    Ok(LoggedUnit(members))
}

/// Where a player is and what they hold, in plain numbers.
///
/// Two places are equal if their bytes on disk are: the numbers with a fraction are
/// compared by their bits.
#[derive(Debug, Clone)]
pub struct LoggedPlace {
    /// The position of the feet: x, y, z.
    pub position: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub flying: bool,
    pub selected_slot: u8,
    /// Each slot of the inventory: its stack, if there is anything in it.
    pub inventory: [Option<ItemStack>; INVENTORY_SLOTS],
    /// Each slot of the ender chest, as the inventory's.
    pub ender: [Option<ItemStack>; ENDER_SLOTS],
    /// The player's experience: their level, the progress to the next (0 to 1) and
    /// the points they gathered in all.
    pub experience: (i32, f32, i32),
    /// Where a bed set their respawn point: the bed's head, the yaw and pitch, and the
    /// bed's dimension.
    pub respawn: Option<([i32; 3], f32, f32, Dimension)>,
    /// Their effects, each level with the ticks it has left (`docs/groundwork/n9-plan.md`,
    /// section 5.4), and the ticks since they last slept.
    pub effects: Vec<clustine_world::components::EffectInstance>,
    pub time_since_rest: u32,
    /// Where their raid omen began, where it has (`docs/groundwork/n10-plan.md`,
    /// section 9.5).
    pub raid_omen_position: Option<[i32; 3]>,
    /// The dimension the place is in (`docs/groundwork/p-plan.md`, section 15).
    pub dimension: Dimension,
    /// Where a portal sends them, until a region has placed them.
    pub travel: Option<Travel>,
    pub seen_credits: bool,
    pub portal_cooldown: u32,
    /// Their health, and whether they are dead and shown the respawn screen
    /// (`docs/groundwork/p-plan.md`, sections 19.5 and 19.6).
    pub health: f32,
    pub dead: bool,
}

impl PartialEq for LoggedPlace {
    fn eq(&self, other: &Self) -> bool {
        self.position.map(f64::to_bits) == other.position.map(f64::to_bits)
            && self.yaw.to_bits() == other.yaw.to_bits()
            && self.pitch.to_bits() == other.pitch.to_bits()
            && self.on_ground == other.on_ground
            && self.flying == other.flying
            && self.selected_slot == other.selected_slot
            && self.inventory == other.inventory
            && self.ender == other.ender
            && self.experience.0 == other.experience.0
            && self.experience.1.to_bits() == other.experience.1.to_bits()
            && self.experience.2 == other.experience.2
            && self.respawn.map(|(pos, yaw, pitch, dimension)| {
                (pos, yaw.to_bits(), pitch.to_bits(), dimension)
            }) == other.respawn.map(|(pos, yaw, pitch, dimension)| {
                (pos, yaw.to_bits(), pitch.to_bits(), dimension)
            })
            && self.effects == other.effects
            && self.time_since_rest == other.time_since_rest
            && self.raid_omen_position == other.raid_omen_position
            && self.dimension == other.dimension
            && self.travel == other.travel
            && self.seen_credits == other.seen_credits
            && self.portal_cooldown == other.portal_cooldown
            && self.health.to_bits() == other.health.to_bits()
            && self.dead == other.dead
    }
}

impl Eq for LoggedPlace {}

/// A record of the log.
#[expect(
    clippy::large_enum_variant,
    reason = "records are read and written one at a time, never kept in numbers"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogRecord {
    /// The blocks that changed during one tick of a region, as logged before regions had
    /// a state of their own: such a log was one per region, so the record does not say
    /// which region it is of. The world store only reads these.
    Changes {
        tick: u64,
        /// The epoch of the owner of the region that made the changes.
        epoch: u64,
        changes: Vec<(BlockPos, BlockState)>,
    },
    /// What one tick of a region committed: the blocks that changed, in the order they
    /// changed, and the region's own record of what else changed, which the store keeps
    /// as it is; and what the region said of stays in that tick, which the store reads.
    Commit {
        region: u32,
        tick: u64,
        /// The epoch of the owner that committed it.
        epoch: u64,
        changes: Vec<Change>,
        state: Vec<u8>,
        /// The stay notes of the tick. A commit without any is written as it was
        /// before there were notes.
        stays: Vec<LoggedStay>,
        /// The scheduled ticks of each chunk whose ticks changed in the tick, whole, as
        /// the delays left at its end, in ascending order of the chunks. A commit
        /// without any is written as it was before there were ticks.
        ticks: Vec<(ChunkPos, Vec<ScheduledTick>)>,
        /// The chunks whose marks for post-processing were taken in the tick, in
        /// ascending order.
        marks_taken: Vec<ChunkPos>,
        /// The seam entries of each chunk whose entries changed in the tick, whole, in
        /// ascending order of the chunks.
        seams: Vec<(ChunkPos, Vec<Seam>)>,
        /// The effects of the tick that the store keeps, each with when it is due, in
        /// the order the tick made them. Their numbers are the record's sequence and
        /// their index here.
        effects: Vec<(Effect, Due)>,
        /// The numbers of the effects the tick took, ascending.
        taken: Vec<EffectId>,
        /// The block events kept in each chunk whose kept events changed in the tick,
        /// whole, in their order, in ascending order of the chunks.
        events: Vec<(ChunkPos, Vec<KeptEvent>)>,
        /// The claims on the points of interest of each chunk whose claims changed in
        /// the tick, whole, in ascending order of the chunks
        /// (`docs/groundwork/n10-plan.md`, section 3.2).
        claims: Vec<(ChunkPos, Vec<Claim>)>,
        /// The raids kept in each chunk whose raids changed in the tick, whole, in
        /// ascending order of the chunks (section 9.1 of the same plan).
        raids: Vec<(ChunkPos, Vec<clustine_world::raid::Raid>)>,
        /// What the tick did to entities other than players, by chunk, in the order it
        /// did it (`docs/adr/0028-entities.md`, section 3). A commit without any is
        /// written as it was before there were entities.
        entity_changes: Vec<(ChunkPos, EntityChange)>,
        /// The entities the tick sent across a line, each with the chunk it is in and when
        /// its holder may take it (`docs/groundwork/n3-plan.md`). A commit without any,
        /// and without arrivals taken, is written as it was before there were arrivals.
        arrivals: Vec<(ChunkPos, Entity, Due)>,
        /// The arrivals the tick took, ascending, the travellers it took among them.
        arrivals_taken: Vec<EffectId>,
        /// The entities other than players its portals sent on, numbered after its
        /// arrivals (`docs/groundwork/p-plan.md`, section 15.4). A commit without any is
        /// written as it was before there were travellers.
        travellers: Vec<Traveller>,
    },
    /// Travellers untaken, written again ahead of the segment they were in, with their
    /// numbers. A traveller is never folded into a chunk (the same plan, section 15.4).
    TravellersForward {
        travellers: Vec<(EffectId, Traveller)>,
    },
    /// These arrivals were taken by a fold at a change of division, which wrote them into
    /// their chunks (the plan, section 6).
    ArrivalsDropped { ids: Vec<EffectId> },
    /// These arrivals of `dimension`, untaken, written again ahead of the segment they
    /// were in.
    ArrivalsForward {
        dimension: Dimension,
        arrivals: Vec<Arrival>,
    },
    /// The effects of these numbers were dropped untaken: the chunk they act on stopped
    /// being used (`docs/adr/0025-effects-between-regions.md`, section 5).
    EffectsDropped { ids: Vec<EffectId> },
    /// Untaken effects of `dimension` written again, with their numbers, so that the
    /// segment they were first written in can be let go (the same record, section 5).
    EffectsForward {
        dimension: Dimension,
        effects: Vec<FiledEffect>,
    },
    /// The region was opened by an owner with `epoch` and restored up to tick
    /// `restored`. A record of the region that comes before this one with a later tick
    /// is not part of the region's history: the owner that opened it here did not read
    /// it, and goes on from `restored` without it.
    Opened {
        region: u32,
        epoch: u64,
        restored: u64,
    },
    /// `region` holds each of `chunks` from its tick `tick` on.
    Granted {
        region: u32,
        tick: u64,
        chunks: Vec<ChunkPos>,
    },
    /// `region` no longer holds `chunks`.
    Returned { region: u32, chunks: Vec<ChunkPos> },
    /// The wandering trader spawner's chance as its firing due at `firing` left it
    /// (`docs/groundwork/n10-plan.md`, decision 12 and section 9.6). The world's: the
    /// record of the latest firing stands.
    TraderChance { firing: u64, chance: i32 },
    /// The merge of two regions. `state` is the whole state of `region` as of `tick`;
    /// every chunk `absorbed` was granted is granted to `region` with that tick, every
    /// area `absorbed` was pinned to is one `region` is pinned to, and `absorbed` is no
    /// region any more.
    Absorbed {
        region: u32,
        /// The epoch of the owner of `region` that asked for it.
        epoch: u64,
        absorbed: u32,
        tick: u64,
        state: Vec<u8>,
    },
    /// The split of a region. `part` is a new region with the whole state `part_state`
    /// as of `tick`; it holds `chunks` from that tick on, which `region` no longer
    /// holds. `state` is the whole state of `region` as of `tick`.
    Split {
        region: u32,
        /// The epoch of the owner of `region` that asked for it.
        epoch: u64,
        tick: u64,
        state: Vec<u8>,
        part: u32,
        /// The highest epoch `part` has been opened with, which is the one its first
        /// owner is to say hello with.
        part_epoch: u64,
        chunks: Vec<ChunkPos>,
        part_state: Vec<u8>,
    },
}

impl LogRecord {
    /// The record as it is appended to the log, frame included.
    pub fn encode(&self) -> Vec<u8> {
        let mut payload = vec![FORMAT_VERSION];
        match self {
            Self::Changes {
                tick,
                epoch,
                changes,
            } => {
                payload.push(KIND_CHANGES);
                payload.extend_from_slice(&tick.to_be_bytes());
                payload.extend_from_slice(&epoch.to_be_bytes());
                put_changes(&mut payload, changes);
            }
            Self::Commit {
                region,
                tick,
                epoch,
                changes,
                state,
                stays,
                ticks,
                marks_taken,
                seams,
                effects,
                taken,
                events,
                claims,
                raids,
                entity_changes,
                arrivals,
                arrivals_taken,
                travellers,
            } => {
                let (blocks, entities) = split_changes(changes);
                let with_travellers = !travellers.is_empty();
                let with_raids = with_travellers || !raids.is_empty();
                let with_claims = with_raids || !claims.is_empty();
                let with_arrivals =
                    with_claims || !arrivals.is_empty() || !arrivals_taken.is_empty();
                let with_entity_changes = with_arrivals || !entity_changes.is_empty();
                let with_events = with_entity_changes || !events.is_empty();
                let with_entities = with_events || !entities.is_empty();
                let effected = with_entities || !effects.is_empty() || !taken.is_empty();
                let records =
                    effected || !ticks.is_empty() || !marks_taken.is_empty() || !seams.is_empty();
                payload.push(if with_travellers {
                    KIND_COMMIT_WITH_TRAVELLERS
                } else if with_raids {
                    KIND_COMMIT_WITH_RAIDS
                } else if with_claims {
                    KIND_COMMIT_WITH_CLAIMS
                } else if with_arrivals {
                    KIND_COMMIT_WITH_ARRIVALS
                } else if with_entity_changes {
                    KIND_COMMIT_WITH_ENTITY_CHANGES
                } else if with_events {
                    KIND_COMMIT_WITH_EVENTS
                } else if with_entities {
                    KIND_COMMIT_WITH_ENTITIES
                } else if effected {
                    KIND_COMMIT_WITH_EFFECTS
                } else if records {
                    KIND_COMMIT_WITH_TICKS
                } else if stays.is_empty() {
                    KIND_COMMIT
                } else {
                    KIND_COMMIT_WITH_STAYS
                });
                payload.extend_from_slice(&region.to_be_bytes());
                payload.extend_from_slice(&tick.to_be_bytes());
                payload.extend_from_slice(&epoch.to_be_bytes());
                put_changes(&mut payload, &blocks);
                put_bytes(&mut payload, state);
                if records {
                    put_stays(&mut payload, stays);
                    put_ticks(&mut payload, ticks);
                    put_chunks(&mut payload, marks_taken);
                    payload.extend_from_slice(&(seams.len() as u32).to_be_bytes());
                    for (chunk, of_chunk) in seams {
                        payload.extend_from_slice(&chunk.x.to_be_bytes());
                        payload.extend_from_slice(&chunk.z.to_be_bytes());
                        crate::seams::put_seams(&mut payload, of_chunk);
                    }
                    if effected {
                        crate::effects::put_effects(&mut payload, effects);
                        crate::effects::put_ids(&mut payload, taken);
                    }
                    if with_entities {
                        put_entities(&mut payload, &entities);
                    }
                    if with_events {
                        payload.extend_from_slice(&(events.len() as u32).to_be_bytes());
                        for (chunk, of_chunk) in events {
                            payload.extend_from_slice(&chunk.x.to_be_bytes());
                            payload.extend_from_slice(&chunk.z.to_be_bytes());
                            crate::kept_events::put_events(&mut payload, of_chunk);
                        }
                    }
                    if with_arrivals {
                        // Its entity changes may be none: a flag says whether they follow.
                        payload.push(u8::from(!entity_changes.is_empty()));
                        if !entity_changes.is_empty() {
                            crate::entities::put_entity_changes(&mut payload, entity_changes);
                        }
                        put_arrivals(&mut payload, arrivals);
                        crate::effects::put_ids(&mut payload, arrivals_taken);
                        if with_claims {
                            payload.extend_from_slice(&(claims.len() as u32).to_be_bytes());
                            for (chunk, of_chunk) in claims {
                                payload.extend_from_slice(&chunk.x.to_be_bytes());
                                payload.extend_from_slice(&chunk.z.to_be_bytes());
                                put_chunk_claims(&mut payload, of_chunk);
                            }
                        }
                        if with_raids {
                            payload.extend_from_slice(&(raids.len() as u32).to_be_bytes());
                            for (chunk, of_chunk) in raids {
                                payload.extend_from_slice(&chunk.x.to_be_bytes());
                                payload.extend_from_slice(&chunk.z.to_be_bytes());
                                crate::raids::put_raids(&mut payload, of_chunk);
                            }
                        }
                        if with_travellers {
                            crate::travel::put_travellers(&mut payload, travellers);
                        }
                    } else if with_entity_changes {
                        crate::entities::put_entity_changes(&mut payload, entity_changes);
                    }
                } else if !stays.is_empty() {
                    put_stays(&mut payload, stays);
                }
            }
            Self::Opened {
                region,
                epoch,
                restored,
            } => {
                payload.push(KIND_OPENED);
                payload.extend_from_slice(&region.to_be_bytes());
                payload.extend_from_slice(&epoch.to_be_bytes());
                payload.extend_from_slice(&restored.to_be_bytes());
            }
            Self::Granted {
                region,
                tick,
                chunks,
            } => {
                payload.push(KIND_GRANTED);
                payload.extend_from_slice(&region.to_be_bytes());
                payload.extend_from_slice(&tick.to_be_bytes());
                put_chunks(&mut payload, chunks);
            }
            Self::Returned { region, chunks } => {
                payload.push(KIND_RETURNED);
                payload.extend_from_slice(&region.to_be_bytes());
                put_chunks(&mut payload, chunks);
            }
            Self::TraderChance { firing, chance } => {
                payload.push(KIND_TRADER_CHANCE);
                payload.extend_from_slice(&firing.to_be_bytes());
                payload.extend_from_slice(&chance.to_be_bytes());
            }
            Self::Absorbed {
                region,
                epoch,
                absorbed,
                tick,
                state,
            } => {
                payload.push(KIND_ABSORBED);
                payload.extend_from_slice(&region.to_be_bytes());
                payload.extend_from_slice(&epoch.to_be_bytes());
                payload.extend_from_slice(&absorbed.to_be_bytes());
                payload.extend_from_slice(&tick.to_be_bytes());
                put_bytes(&mut payload, state);
            }
            Self::Split {
                region,
                epoch,
                tick,
                state,
                part,
                part_epoch,
                chunks,
                part_state,
            } => {
                payload.push(KIND_SPLIT);
                payload.extend_from_slice(&region.to_be_bytes());
                payload.extend_from_slice(&epoch.to_be_bytes());
                payload.extend_from_slice(&tick.to_be_bytes());
                put_bytes(&mut payload, state);
                payload.extend_from_slice(&part.to_be_bytes());
                payload.extend_from_slice(&part_epoch.to_be_bytes());
                put_chunks(&mut payload, chunks);
                put_bytes(&mut payload, part_state);
            }
            Self::EffectsDropped { ids } => {
                payload.push(KIND_EFFECTS_DROPPED);
                crate::effects::put_ids(&mut payload, ids);
            }
            Self::EffectsForward { dimension, effects } => {
                payload.push(KIND_EFFECTS_FORWARD);
                crate::travel::put_dimension(&mut payload, *dimension);
                crate::effects::put_filed(&mut payload, effects);
            }
            Self::ArrivalsDropped { ids } => {
                payload.push(KIND_ARRIVALS_DROPPED);
                crate::effects::put_ids(&mut payload, ids);
            }
            Self::TravellersForward { travellers } => {
                payload.push(KIND_TRAVELLERS_FORWARD);
                payload.extend_from_slice(&(travellers.len() as u32).to_be_bytes());
                for (id, traveller) in travellers {
                    crate::effects::put_id(&mut payload, *id);
                    crate::travel::put_traveller(&mut payload, traveller);
                }
            }
            Self::ArrivalsForward {
                dimension,
                arrivals,
            } => {
                payload.push(KIND_ARRIVALS_FORWARD);
                crate::travel::put_dimension(&mut payload, *dimension);
                payload.extend_from_slice(&(arrivals.len() as u32).to_be_bytes());
                for arrival in arrivals {
                    crate::effects::put_id(&mut payload, arrival.id);
                    put_arrival(&mut payload, arrival.chunk, &arrival.entity, arrival.due);
                }
            }
        }
        frame(&payload)
    }

    fn decode(payload: &[u8]) -> Result<Self, FormatError> {
        let mut input = Input(payload);
        let version = input.u8()?;
        if version != FORMAT_VERSION {
            return Err(FormatError::UnsupportedVersion(version));
        }
        let record = match input.u8()? {
            KIND_CHANGES => Self::Changes {
                tick: input.u64()?,
                epoch: input.u64()?,
                changes: take_changes(&mut input)?,
            },
            KIND_COMMIT => Self::Commit {
                region: input.u32()?,
                tick: input.u64()?,
                epoch: input.u64()?,
                changes: blocks_only(take_changes(&mut input)?),
                state: take_bytes(&mut input)?,
                stays: Vec::new(),
                ticks: Vec::new(),
                marks_taken: Vec::new(),
                seams: Vec::new(),
                effects: Vec::new(),
                taken: Vec::new(),
                events: Vec::new(),
                claims: Vec::new(),
                raids: Vec::new(),
                entity_changes: Vec::new(),
                arrivals: Vec::new(),
                arrivals_taken: Vec::new(),
                travellers: Vec::new(),
            },
            KIND_COMMIT_WITH_STAYS => Self::Commit {
                region: input.u32()?,
                tick: input.u64()?,
                epoch: input.u64()?,
                changes: blocks_only(take_changes(&mut input)?),
                state: take_bytes(&mut input)?,
                stays: take_stays(&mut input)?,
                ticks: Vec::new(),
                marks_taken: Vec::new(),
                seams: Vec::new(),
                effects: Vec::new(),
                taken: Vec::new(),
                events: Vec::new(),
                claims: Vec::new(),
                raids: Vec::new(),
                entity_changes: Vec::new(),
                arrivals: Vec::new(),
                arrivals_taken: Vec::new(),
                travellers: Vec::new(),
            },
            kind @ (KIND_COMMIT_WITH_TICKS
            | KIND_COMMIT_WITH_EFFECTS
            | KIND_COMMIT_WITH_ENTITIES
            | KIND_COMMIT_WITH_EVENTS
            | KIND_COMMIT_WITH_ENTITY_CHANGES
            | KIND_COMMIT_WITH_ARRIVALS
            | KIND_COMMIT_WITH_CLAIMS
            | KIND_COMMIT_WITH_RAIDS
            | KIND_COMMIT_WITH_TRAVELLERS) => {
                let (region, tick, epoch) = (input.u32()?, input.u64()?, input.u64()?);
                let blocks = take_changes(&mut input)?;
                let state = take_bytes(&mut input)?;
                let stays = if input.peek_u32()? == 0 {
                    input.u32()?;
                    Vec::new()
                } else {
                    take_stays(&mut input)?
                };
                let ticks = take_ticks(&mut input)?;
                let marks_taken = take_chunks(&mut input)?;
                let chunks = input.u32()? as usize;
                // Each is eight bytes and a count at least.
                let mut seams = Vec::with_capacity(chunks.min(input.0.len() / 12));
                for _ in 0..chunks {
                    let chunk = ChunkPos::new(input.i32()?, input.i32()?);
                    seams.push((chunk, crate::seams::take_seams(&mut input)?));
                }
                let (effects, taken) = if kind == KIND_COMMIT_WITH_TICKS {
                    (Vec::new(), Vec::new())
                } else {
                    let effects = crate::effects::take_effects(&mut input)?;
                    (effects, crate::effects::take_ids(&mut input)?)
                };
                let changes = if kind >= KIND_COMMIT_WITH_ENTITIES {
                    let entities = take_entities(&mut input)?;
                    if kind == KIND_COMMIT_WITH_ENTITIES && entities.is_empty() {
                        return Err(FormatError::Corrupt(
                            "a commit with block entities that has none",
                        ));
                    }
                    join_changes(blocks, entities)?
                } else {
                    blocks_only(blocks)
                };
                let mut events = Vec::new();
                if kind >= KIND_COMMIT_WITH_EVENTS {
                    let chunks = input.u32()? as usize;
                    // Each is eight bytes and a count at least.
                    events.reserve(chunks.min(input.0.len() / 12));
                    for _ in 0..chunks {
                        let chunk = ChunkPos::new(input.i32()?, input.i32()?);
                        events.push((chunk, crate::kept_events::take_events(&mut input)?));
                    }
                    if kind == KIND_COMMIT_WITH_EVENTS && events.is_empty() {
                        return Err(FormatError::Corrupt(
                            "a commit with block events that has none",
                        ));
                    }
                }
                let mut arrivals = Vec::new();
                let mut arrivals_taken = Vec::new();
                let entity_changes = if kind == KIND_COMMIT_WITH_ENTITY_CHANGES {
                    crate::entities::take_entity_changes(&mut input)?
                } else if kind >= KIND_COMMIT_WITH_ARRIVALS {
                    let changes = if take_bool(&mut input, "whether entity changes follow")? {
                        crate::entities::take_entity_changes(&mut input)?
                    } else {
                        Vec::new()
                    };
                    arrivals = take_arrivals(&mut input)?;
                    arrivals_taken = crate::effects::take_ids(&mut input)?;
                    if kind == KIND_COMMIT_WITH_ARRIVALS
                        && arrivals.is_empty()
                        && arrivals_taken.is_empty()
                    {
                        return Err(FormatError::Corrupt("a commit with arrivals that has none"));
                    }
                    changes
                } else {
                    Vec::new()
                };
                let mut claims = Vec::new();
                if kind >= KIND_COMMIT_WITH_CLAIMS {
                    let chunks = input.u32()? as usize;
                    // Each is eight bytes and a count at least.
                    claims.reserve(chunks.min(input.0.len() / 10));
                    for _ in 0..chunks {
                        let chunk = ChunkPos::new(input.i32()?, input.i32()?);
                        claims.push((chunk, take_chunk_claims(&mut input)?));
                    }
                    if kind == KIND_COMMIT_WITH_CLAIMS && claims.is_empty() {
                        return Err(FormatError::Corrupt("a commit with claims that has none"));
                    }
                }
                let mut raids = Vec::new();
                if kind >= KIND_COMMIT_WITH_RAIDS {
                    let chunks = input.u32()? as usize;
                    // Each is eight bytes and a count at least.
                    raids.reserve(chunks.min(input.0.len() / 12));
                    for _ in 0..chunks {
                        let chunk = ChunkPos::new(input.i32()?, input.i32()?);
                        raids.push((chunk, crate::raids::take_raids(&mut input)?));
                    }
                    if kind == KIND_COMMIT_WITH_RAIDS && raids.is_empty() {
                        return Err(FormatError::Corrupt("a commit with raids that has none"));
                    }
                }
                let mut travellers = Vec::new();
                if kind == KIND_COMMIT_WITH_TRAVELLERS {
                    travellers = crate::travel::take_travellers(&mut input)?;
                    if travellers.is_empty() {
                        return Err(FormatError::Corrupt(
                            "a commit with travellers that has none",
                        ));
                    }
                }
                // A commit is written in one form only: without effects as kind 9, and
                // without records of chunks as kind 2 or 8.
                if kind == KIND_COMMIT_WITH_EFFECTS && effects.is_empty() && taken.is_empty() {
                    return Err(FormatError::Corrupt("a commit with effects that has none"));
                }
                if kind == KIND_COMMIT_WITH_TICKS
                    && ticks.is_empty()
                    && marks_taken.is_empty()
                    && seams.is_empty()
                {
                    return Err(FormatError::Corrupt(
                        "a commit with records of chunks that has none",
                    ));
                }
                Self::Commit {
                    region,
                    tick,
                    epoch,
                    changes,
                    state,
                    stays,
                    ticks,
                    marks_taken,
                    seams,
                    effects,
                    taken,
                    events,
                    claims,
                    raids,
                    entity_changes,
                    arrivals,
                    arrivals_taken,
                    travellers,
                }
            }
            KIND_OPENED => Self::Opened {
                region: input.u32()?,
                epoch: input.u64()?,
                restored: input.u64()?,
            },
            KIND_GRANTED => Self::Granted {
                region: input.u32()?,
                tick: input.u64()?,
                chunks: take_chunks(&mut input)?,
            },
            KIND_RETURNED => Self::Returned {
                region: input.u32()?,
                chunks: take_chunks(&mut input)?,
            },
            KIND_TRADER_CHANCE => Self::TraderChance {
                firing: input.u64()?,
                chance: input.i32()?,
            },
            KIND_ABSORBED => Self::Absorbed {
                region: input.u32()?,
                epoch: input.u64()?,
                absorbed: input.u32()?,
                tick: input.u64()?,
                state: take_bytes(&mut input)?,
            },
            KIND_SPLIT => Self::Split {
                region: input.u32()?,
                epoch: input.u64()?,
                tick: input.u64()?,
                state: take_bytes(&mut input)?,
                part: input.u32()?,
                part_epoch: input.u64()?,
                chunks: take_chunks(&mut input)?,
                part_state: take_bytes(&mut input)?,
            },
            KIND_EFFECTS_DROPPED => Self::EffectsDropped {
                ids: crate::effects::take_ids(&mut input)?,
            },
            KIND_EFFECTS_FORWARD => Self::EffectsForward {
                dimension: crate::travel::take_dimension(&mut input)?,
                effects: crate::effects::take_filed(&mut input)?,
            },
            KIND_ARRIVALS_DROPPED => Self::ArrivalsDropped {
                ids: crate::effects::take_ids(&mut input)?,
            },
            KIND_TRAVELLERS_FORWARD => {
                let count = input.u32()? as usize;
                // Each is far longer than a byte: no forged count reserves more.
                let mut travellers = Vec::with_capacity(count.min(input.0.len()));
                for _ in 0..count {
                    let id = crate::effects::take_id(&mut input)?;
                    travellers.push((id, crate::travel::take_traveller(&mut input)?));
                }
                Self::TravellersForward { travellers }
            }
            KIND_ARRIVALS_FORWARD => {
                let dimension = crate::travel::take_dimension(&mut input)?;
                let count = input.u32()? as usize;
                // Each is far longer than a byte: no forged count reserves more.
                let mut arrivals = Vec::with_capacity(count.min(input.0.len()));
                for _ in 0..count {
                    let id = crate::effects::take_id(&mut input)?;
                    let (chunk, entity, due) = take_arrival(&mut input)?;
                    arrivals.push(Arrival {
                        id,
                        chunk,
                        entity,
                        due,
                    });
                }
                Self::ArrivalsForward {
                    dimension,
                    arrivals,
                }
            }
            _ => return Err(FormatError::Corrupt("record kind")),
        };
        input.finish()?;
        Ok(record)
    }
}

/// Frames `payload` as a record of the log.
fn frame(payload: &[u8]) -> Vec<u8> {
    let mut record = Vec::with_capacity(FRAME_HEADER_LENGTH + payload.len());
    record.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    record.extend_from_slice(&crc32fast::hash(payload).to_be_bytes());
    record.extend_from_slice(payload);
    record
}

fn put_changes(payload: &mut Vec<u8>, changes: &[(BlockPos, BlockState)]) {
    payload.extend_from_slice(&(changes.len() as u32).to_be_bytes());
    for (position, state) in changes {
        payload.extend_from_slice(&position.x.to_be_bytes());
        payload.extend_from_slice(&position.y.to_be_bytes());
        payload.extend_from_slice(&position.z.to_be_bytes());
        payload.extend_from_slice(&state.0.to_be_bytes());
    }
}

fn take_changes(input: &mut Input<'_>) -> Result<Vec<(BlockPos, BlockState)>, FormatError> {
    let count = input.u32()? as usize;
    // A forged count must not reserve more than the payload can hold.
    let mut changes = Vec::with_capacity(count.min(input.0.len() / CHANGE_LENGTH));
    for _ in 0..count {
        let position = BlockPos::new(input.i32()?, input.i32()?, input.i32()?);
        changes.push((position, BlockState(input.u16()?)));
    }
    Ok(changes)
}

/// A commit's changes as kind 13 writes them: the blocks alone, and each change of a
/// block entity with the number of block changes before it.
#[allow(clippy::type_complexity)]
fn split_changes(changes: &[Change]) -> (Vec<(BlockPos, BlockState)>, Vec<(u32, BlockPos, &[u8])>) {
    let mut blocks = Vec::with_capacity(changes.len());
    let mut entities = Vec::new();
    for change in changes {
        match change {
            Change::Block(position, state) => blocks.push((*position, *state)),
            Change::BlockEntity(position, data) => {
                entities.push((blocks.len() as u32, *position, data.as_slice()));
            }
        }
    }
    (blocks, entities)
}

/// The changes of a commit written before block entities were.
fn blocks_only(blocks: Vec<(BlockPos, BlockState)>) -> Vec<Change> {
    blocks.into_iter().map(Change::from).collect()
}

/// The changes of a commit of kind 13, in the order its tick made them.
fn join_changes(
    blocks: Vec<(BlockPos, BlockState)>,
    entities: Vec<(u32, BlockPos, Vec<u8>)>,
) -> Result<Vec<Change>, FormatError> {
    let mut changes = Vec::with_capacity(blocks.len() + entities.len());
    let mut blocks = blocks.into_iter();
    let mut before = 0;
    for (place, position, data) in entities {
        if place < before || place as usize > before as usize + blocks.len() {
            return Err(FormatError::Corrupt(
                "a block entity's place among a commit's blocks is out of order",
            ));
        }
        changes.extend(
            blocks
                .by_ref()
                .take((place - before) as usize)
                .map(Change::from),
        );
        before = place;
        changes.push(Change::BlockEntity(position, data));
    }
    changes.extend(blocks.map(Change::from));
    Ok(changes)
}

fn put_entities(payload: &mut Vec<u8>, entities: &[(u32, BlockPos, &[u8])]) {
    payload.extend_from_slice(&(entities.len() as u32).to_be_bytes());
    for (place, position, data) in entities {
        payload.extend_from_slice(&place.to_be_bytes());
        payload.extend_from_slice(&position.x.to_be_bytes());
        payload.extend_from_slice(&position.y.to_be_bytes());
        payload.extend_from_slice(&position.z.to_be_bytes());
        put_bytes(payload, data);
    }
}

#[allow(clippy::type_complexity)]
fn take_entities(input: &mut Input<'_>) -> Result<Vec<(u32, BlockPos, Vec<u8>)>, FormatError> {
    let count = input.u32()? as usize;
    // Each is twenty bytes at least: a forged count must not reserve more.
    let mut entities = Vec::with_capacity(count.min(input.0.len() / 20));
    for _ in 0..count {
        let place = input.u32()?;
        let position = BlockPos::new(input.i32()?, input.i32()?, input.i32()?);
        entities.push((place, position, take_bytes(input)?));
    }
    Ok(entities)
}

fn put_chunks(payload: &mut Vec<u8>, chunks: &[ChunkPos]) {
    payload.extend_from_slice(&(chunks.len() as u32).to_be_bytes());
    for chunk in chunks {
        payload.extend_from_slice(&chunk.x.to_be_bytes());
        payload.extend_from_slice(&chunk.z.to_be_bytes());
    }
}

fn take_chunks(input: &mut Input<'_>) -> Result<Vec<ChunkPos>, FormatError> {
    let count = input.u32()? as usize;
    // A forged count must not reserve more than the payload can hold.
    let mut chunks = Vec::with_capacity(count.min(input.0.len() / CHUNK_LENGTH));
    for _ in 0..count {
        chunks.push(ChunkPos::new(input.i32()?, input.i32()?));
    }
    Ok(chunks)
}

fn put_bytes(payload: &mut Vec<u8>, bytes: &[u8]) {
    payload.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    payload.extend_from_slice(bytes);
}

fn take_bytes(input: &mut Input<'_>) -> Result<Vec<u8>, FormatError> {
    let length = input.u32()? as usize;
    Ok(input.take(length)?.to_vec())
}

fn put_stays(payload: &mut Vec<u8>, stays: &[LoggedStay]) {
    payload.extend_from_slice(&(stays.len() as u32).to_be_bytes());
    for stay in stays {
        match stay {
            LoggedStay::Entering { player, entity } => {
                payload.push(STAY_ENTERING);
                payload.extend_from_slice(&player.to_be_bytes());
                payload.extend_from_slice(&entity.to_be_bytes());
            }
            LoggedStay::Has {
                player,
                entity,
                hops,
                place,
                unit,
            } => {
                payload.push(match unit {
                    Some(_) => STAY_HAS_WITH_UNIT,
                    None => STAY_HAS,
                });
                payload.extend_from_slice(&player.to_be_bytes());
                payload.extend_from_slice(&entity.to_be_bytes());
                payload.extend_from_slice(&hops.to_be_bytes());
                put_place(payload, place);
                if let Some(unit) = unit {
                    put_unit(payload, unit);
                }
            }
            LoggedStay::Took { player, root } => {
                payload.push(STAY_TOOK);
                payload.extend_from_slice(&player.to_be_bytes());
                payload.extend_from_slice(&root.to_be_bytes());
            }
        }
    }
}

/// The length of one tick in a commit's ticks.
const TICK_LENGTH: usize = 1 + 2 + 1 + 1 + 2 + 4 + 1;

fn put_ticks(payload: &mut Vec<u8>, ticks: &[(ChunkPos, Vec<ScheduledTick>)]) {
    payload.extend_from_slice(&(ticks.len() as u32).to_be_bytes());
    for (chunk, of_chunk) in ticks {
        payload.extend_from_slice(&chunk.x.to_be_bytes());
        payload.extend_from_slice(&chunk.z.to_be_bytes());
        payload.extend_from_slice(&(of_chunk.len() as u32).to_be_bytes());
        for tick in of_chunk {
            payload.push(tick.at.x);
            payload.extend_from_slice(&tick.at.y.to_be_bytes());
            payload.push(tick.at.z);
            let (kind, id) = match tick.of {
                Ticked::Block(block) => (1_u8, block),
                Ticked::Fluid(fluid) => (2, u16::from(fluid)),
            };
            payload.push(kind);
            payload.extend_from_slice(&id.to_be_bytes());
            payload.extend_from_slice(&tick.delay.to_be_bytes());
            payload.extend_from_slice(&tick.priority.to_be_bytes());
        }
    }
}

fn take_ticks(input: &mut Input<'_>) -> Result<Vec<(ChunkPos, Vec<ScheduledTick>)>, FormatError> {
    let chunks = input.u32()? as usize;
    let mut ticks = Vec::with_capacity(chunks.min(input.0.len() / 12));
    for _ in 0..chunks {
        let chunk = ChunkPos::new(input.i32()?, input.i32()?);
        let count = input.u32()? as usize;
        // A forged count must not reserve more than the payload can hold.
        let mut of_chunk = Vec::with_capacity(count.min(input.0.len() / TICK_LENGTH));
        for _ in 0..count {
            let (x, y, z) = (input.u8()?, input.i16()?, input.u8()?);
            if x > 15 || z > 15 {
                return Err(FormatError::Corrupt("a tick outside its chunk"));
            }
            let kind = input.u8()?;
            let id = input.u16()?;
            let of = match kind {
                1 => Ticked::Block(id),
                2 => Ticked::Fluid(u8::try_from(id).map_err(|_| FormatError::Corrupt("fluid"))?),
                _ => return Err(FormatError::Corrupt("tick kind")),
            };
            of_chunk.push(ScheduledTick {
                at: InChunk { y, z, x },
                of,
                delay: input.i32()?,
                priority: input.i8()?,
            });
        }
        ticks.push((chunk, of_chunk));
    }
    Ok(ticks)
}

fn take_stays(input: &mut Input<'_>) -> Result<Vec<LoggedStay>, FormatError> {
    let count = input.u32()? as usize;
    // A commit without notes is of the other kind, so that one commit has one form.
    if count == 0 {
        return Err(FormatError::Corrupt("a commit with stays that has none"));
    }
    // A forged count must not reserve more than the payload can hold.
    let mut stays = Vec::with_capacity(count.min(input.0.len() / STAY_LENGTH));
    for _ in 0..count {
        let kind = input.u8()?;
        let (player, entity) = (input.u128()?, input.u64()?);
        stays.push(match kind {
            STAY_ENTERING => LoggedStay::Entering { player, entity },
            STAY_HAS => LoggedStay::Has {
                player,
                entity,
                hops: input.u32()?,
                place: take_place(input)?,
                unit: None,
            },
            STAY_HAS_WITH_UNIT => LoggedStay::Has {
                player,
                entity,
                hops: input.u32()?,
                place: take_place(input)?,
                unit: Some(take_unit(input)?),
            },
            STAY_TOOK => LoggedStay::Took {
                player,
                root: entity,
            },
            _ => return Err(FormatError::Corrupt("stay kind")),
        });
    }
    Ok(stays)
}

/// Appends a place as the log and the players' file have it.
pub(crate) fn put_place(out: &mut Vec<u8>, place: &LoggedPlace) {
    for coordinate in place.position {
        out.extend_from_slice(&coordinate.to_be_bytes());
    }
    out.extend_from_slice(&place.yaw.to_be_bytes());
    out.extend_from_slice(&place.pitch.to_be_bytes());
    let on_ground = if place.on_ground { ON_GROUND } else { 0 };
    let flying = if place.flying { FLYING } else { 0 };
    out.push(on_ground | flying);
    out.push(place.selected_slot);
    for slot in place.inventory.iter().chain(&place.ender) {
        put_optional_stack(out, slot.as_ref());
    }
    let (level, progress, total) = place.experience;
    out.extend_from_slice(&level.to_be_bytes());
    out.extend_from_slice(&progress.to_be_bytes());
    out.extend_from_slice(&total.to_be_bytes());
    match place.respawn {
        None => out.push(0),
        Some((pos, yaw, pitch, dimension)) => {
            out.push(1);
            for coordinate in pos {
                out.extend_from_slice(&coordinate.to_be_bytes());
            }
            out.extend_from_slice(&yaw.to_be_bytes());
            out.extend_from_slice(&pitch.to_be_bytes());
            crate::travel::put_dimension(out, dimension);
        }
    }
    let count = u32::try_from(place.effects.len()).expect("fewer effects than a u32 counts");
    out.extend_from_slice(&count.to_be_bytes());
    for effect in &place.effects {
        crate::stacks::put_effect(out, effect);
    }
    out.extend_from_slice(&place.time_since_rest.to_be_bytes());
    match place.raid_omen_position {
        None => out.push(0),
        Some(position) => {
            out.push(1);
            for coordinate in position {
                out.extend_from_slice(&coordinate.to_be_bytes());
            }
        }
    }
    crate::travel::put_dimension(out, place.dimension);
    out.push(u8::from(place.seen_credits));
    out.extend_from_slice(&place.portal_cooldown.to_be_bytes());
    crate::travel::put_optional_travel(out, place.travel.as_ref());
    out.extend_from_slice(&place.health.to_be_bytes());
    out.push(u8::from(place.dead));
}

/// Reads what [`put_place`] wrote.
pub(crate) fn take_place(input: &mut Input<'_>) -> Result<LoggedPlace, FormatError> {
    let position = [input.f64()?, input.f64()?, input.f64()?];
    let (yaw, pitch) = (input.f32()?, input.f32()?);
    let flags = input.u8()?;
    if flags & !(ON_GROUND | FLYING) != 0 {
        return Err(FormatError::Corrupt("place flags"));
    }
    let selected_slot = input.u8()?;
    let mut inventory = [const { None }; INVENTORY_SLOTS];
    let mut ender = [const { None }; ENDER_SLOTS];
    for slot in inventory.iter_mut().chain(&mut ender) {
        *slot = take_optional_stack(input, "inventory slot")?;
    }
    let experience = (input.i32()?, input.f32()?, input.i32()?);
    let respawn = match input.u8()? {
        0 => None,
        1 => Some((
            [input.i32()?, input.i32()?, input.i32()?],
            input.f32()?,
            input.f32()?,
            crate::travel::take_dimension(input)?,
        )),
        _ => return Err(FormatError::Corrupt("a place's respawn flag")),
    };
    let count = input.u32()?;
    // Every effect takes more than a byte: a count beyond what is left is corrupt.
    if usize::try_from(count).map_or(true, |count| count > input.0.len()) {
        return Err(FormatError::Corrupt("a place's count of effects"));
    }
    let mut effects = Vec::new();
    for _ in 0..count {
        effects.push(crate::stacks::take_effect(input)?);
    }
    let time_since_rest = input.u32()?;
    let raid_omen_position = match input.u8()? {
        0 => None,
        1 => Some([input.i32()?, input.i32()?, input.i32()?]),
        _ => return Err(FormatError::Corrupt("a place's raid omen flag")),
    };
    let dimension = crate::travel::take_dimension(input)?;
    let seen_credits = take_bool(input, "a place's seen credits flag")?;
    let portal_cooldown = input.u32()?;
    let travel = crate::travel::take_optional_travel(input)?;
    let health = input.f32()?;
    let dead = take_bool(input, "a place's dead flag")?;
    Ok(LoggedPlace {
        dimension,
        seen_credits,
        portal_cooldown,
        travel,
        health,
        dead,
        position,
        yaw,
        pitch,
        on_ground: flags & ON_GROUND != 0,
        flying: flags & FLYING != 0,
        selected_slot,
        inventory,
        ender,
        experience,
        respawn,
        effects,
        time_since_rest,
        raid_omen_position,
    })
}

/// A record read from a log, with where it is in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Logged {
    /// The offset of the record's frame in the log.
    pub offset: usize,
    /// The length of the frame, header included.
    pub length: usize,
    pub record: LogRecord,
}

/// Reads the records at the start of `log`. Returns them with the number of bytes they
/// occupy; anything after that is a record that was not written completely.
///
/// A record that is complete and passes its checksum but cannot be understood is an
/// error: that is not what dying in the middle of a write looks like.
pub fn read_log(log: &[u8]) -> Result<(Vec<LogRecord>, usize), FormatError> {
    let (logged, valid) = read_log_with_offsets(log)?;
    Ok((
        logged.into_iter().map(|logged| logged.record).collect(),
        valid,
    ))
}

/// Does what [`read_log`] does, and says for each record where it is.
pub fn read_log_with_offsets(log: &[u8]) -> Result<(Vec<Logged>, usize), FormatError> {
    let mut records = Vec::new();
    let mut valid = 0;
    loop {
        let rest = &log[valid..];
        let Some((header, body)) = rest.split_at_checked(FRAME_HEADER_LENGTH) else {
            break;
        };
        let length = u32::from_be_bytes(header[..4].try_into().expect("four bytes")) as usize;
        let checksum = u32::from_be_bytes(header[4..].try_into().expect("four bytes"));
        if length > MAX_PAYLOAD_LENGTH {
            break;
        }
        let Some(payload) = body.get(..length) else {
            break;
        };
        if crc32fast::hash(payload) != checksum {
            break;
        }
        records.push(Logged {
            offset: valid,
            length: FRAME_HEADER_LENGTH + length,
            record: LogRecord::decode(payload)?,
        });
        valid += FRAME_HEADER_LENGTH + length;
    }
    Ok((records, valid))
}

/// Appends an arrival without its number: its chunk, the entity and when it is due.
fn put_arrival(out: &mut Vec<u8>, chunk: ChunkPos, entity: &Entity, due: Due) {
    out.extend_from_slice(&chunk.x.to_be_bytes());
    out.extend_from_slice(&chunk.z.to_be_bytes());
    crate::entities::put_entity(out, entity);
    crate::effects::put_due(out, due);
}

/// Reads what [`put_arrival`] wrote; an entity not in the chunk it is sent to is refused,
/// but for a passenger, which goes to its root's.
fn take_arrival(input: &mut Input<'_>) -> Result<(ChunkPos, Entity, Due), FormatError> {
    let chunk = ChunkPos::new(input.i32()?, input.i32()?);
    let entity = crate::entities::take_entity(input)?;
    if entity.riding.vehicle.is_none() && entity.chunk() != chunk {
        return Err(FormatError::Corrupt("an arrival outside its chunk"));
    }
    Ok((chunk, entity, crate::effects::take_due(input)?))
}

use crate::chunk::{put_claims as put_chunk_claims, take_claims as take_chunk_claims};

fn put_arrivals(out: &mut Vec<u8>, arrivals: &[(ChunkPos, Entity, Due)]) {
    out.extend_from_slice(&(arrivals.len() as u32).to_be_bytes());
    for (chunk, entity, due) in arrivals {
        put_arrival(out, *chunk, entity, *due);
    }
}

fn take_arrivals(input: &mut Input<'_>) -> Result<Vec<(ChunkPos, Entity, Due)>, FormatError> {
    let count = input.u32()? as usize;
    let mut arrivals = Vec::with_capacity(count.min(input.0.len()));
    for _ in 0..count {
        arrivals.push(take_arrival(input)?);
    }
    Ok(arrivals)
}

fn take_bool(input: &mut Input<'_>, what: &'static str) -> Result<bool, FormatError> {
    match input.u8()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(FormatError::Corrupt(what)),
    }
}

#[cfg(test)]
mod tests {
    use clustine_data::blocks;
    use proptest::prelude::*;

    use super::*;

    fn record(tick: u64, count: i32) -> LogRecord {
        LogRecord::Commit {
            travellers: Vec::new(),
            region: 3,
            tick,
            epoch: 1,
            changes: (0..count)
                .map(|index| (BlockPos::new(index, -61, -index), blocks::STONE).into())
                .collect(),
            state: vec![tick as u8; count as usize],
            stays: Vec::new(),
            ticks: Vec::new(),
            marks_taken: Vec::new(),
            seams: Vec::new(),
            effects: Vec::new(),
            taken: Vec::new(),
            events: Vec::new(),
            entity_changes: Vec::new(),
            arrivals: Vec::new(),
            arrivals_taken: Vec::new(),
            claims: Vec::new(),
            raids: Vec::new(),
        }
    }

    #[test]
    fn known_answer() {
        let record = LogRecord::Commit {
            travellers: Vec::new(),
            region: 4,
            tick: 2,
            epoch: 1,
            changes: vec![(BlockPos::new(1, -1, 3), BlockState(9)).into()],
            state: vec![7, 8],
            stays: Vec::new(),
            ticks: Vec::new(),
            marks_taken: Vec::new(),
            seams: Vec::new(),
            effects: Vec::new(),
            taken: Vec::new(),
            events: Vec::new(),
            entity_changes: Vec::new(),
            arrivals: Vec::new(),
            arrivals_taken: Vec::new(),
            claims: Vec::new(),
            raids: Vec::new(),
        };
        let bytes = record.encode();
        let payload = [
            FORMAT_VERSION,
            2, // version, kind
            0,
            0,
            0,
            4, // region
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            2, // tick
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            1, // epoch
            0,
            0,
            0,
            1, // one change
            0,
            0,
            0,
            1,
            0xFF,
            0xFF,
            0xFF,
            0xFF,
            0,
            0,
            0,
            3,
            0,
            9, // the change
            0,
            0,
            0,
            2,
            7,
            8, // the state
        ];
        assert_eq!(bytes[..4], (payload.len() as u32).to_be_bytes());
        assert_eq!(bytes[4..8], crc32fast::hash(&payload).to_be_bytes());
        assert_eq!(bytes[8..], payload);
        assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));

        let opened = LogRecord::Opened {
            region: 4,
            epoch: 9,
            restored: 300,
        };
        let bytes = opened.encode();
        let payload = [
            FORMAT_VERSION,
            3, // version, kind
            0,
            0,
            0,
            4, // region
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            9, // epoch
            0,
            0,
            0,
            0,
            0,
            0,
            1,
            44, // restored
        ];
        assert_eq!(bytes[8..], payload);
        assert_eq!(read_log(&bytes), Ok((vec![opened], bytes.len())));
    }

    /// A record of block changes alone, as the world store wrote them before regions
    /// had a state, is still a record of the log. No world this server opens has one:
    /// those worlds are of format 1.
    #[test]
    fn records_of_block_changes_alone_are_read() {
        let payload = [
            FORMAT_VERSION,
            1, // version, kind
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            2, // tick
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            1, // epoch
            0,
            0,
            0,
            1, // one change
            0,
            0,
            0,
            1,
            0xFF,
            0xFF,
            0xFF,
            0xFF,
            0,
            0,
            0,
            3,
            0,
            9,
        ];
        let bytes = frame(&payload);
        let expected = LogRecord::Changes {
            tick: 2,
            epoch: 1,
            changes: vec![(BlockPos::new(1, -1, 3), BlockState(9))],
        };
        assert_eq!(expected.encode(), bytes);
        assert_eq!(read_log(&bytes), Ok((vec![expected], bytes.len())));
    }

    /// A record of format 1 is not one this build reads, whatever its kind: the log of
    /// a world from before format 2 is not taken for a log with nothing in it.
    #[test]
    fn a_record_of_format_1_is_refused() {
        for kind in 1..=8 {
            assert_eq!(
                read_log(&frame(&[1, kind, 0, 0, 0, 4])),
                Err(FormatError::UnsupportedVersion(1)),
                "kind {kind}"
            );
        }
    }

    #[test]
    fn an_empty_log_has_no_records() {
        assert_eq!(read_log(&[]), Ok((Vec::new(), 0)));
    }

    /// Whatever byte the process died at while appending, exactly the records written
    /// in full before are read back, each where it was written.
    #[test]
    fn a_log_cut_off_anywhere_yields_the_complete_records() {
        let records = [record(1, 3), record(2, 0), record(5, 40)];
        let mut log = Vec::new();
        let mut ends = Vec::new();
        for record in &records {
            log.extend_from_slice(&record.encode());
            ends.push(log.len());
        }

        for length in 0..=log.len() {
            let complete = ends.iter().filter(|end| **end <= length).count();
            let valid = ends[..complete].last().copied().unwrap_or(0);
            assert_eq!(
                read_log(&log[..length]),
                Ok((records[..complete].to_vec(), valid)),
                "cut at {length}"
            );
        }
        let (logged, _) = read_log_with_offsets(&log).unwrap();
        for (index, logged) in logged.iter().enumerate() {
            let start = if index == 0 { 0 } else { ends[index - 1] };
            assert_eq!((logged.offset, logged.length), (start, ends[index] - start));
            assert_eq!(logged.record, records[index]);
        }
    }

    #[test]
    fn a_damaged_record_ends_the_log() {
        let mut log = record(1, 2).encode();
        let first = log.len();
        log.extend_from_slice(&record(2, 2).encode());
        log.extend_from_slice(&record(3, 2).encode());

        // Damage anywhere in the second record: only the first is read, and the third
        // is not trusted either.
        for index in first..first + record(2, 2).encode().len() {
            let mut damaged = log.clone();
            damaged[index] ^= 0x10;
            let (records, valid) = read_log(&damaged).unwrap();
            assert_eq!(records, [record(1, 2)], "byte {index}");
            assert_eq!(valid, first);
        }
    }

    #[test]
    fn a_record_that_checks_out_but_makes_no_sense_is_an_error() {
        let later = FORMAT_VERSION + 1;
        assert_eq!(
            read_log(&frame(&[later, 1])),
            Err(FormatError::UnsupportedVersion(later))
        );
        assert!(read_log(&frame(&[1, 7])).is_err());
        assert!(read_log(&frame(&[])).is_err());
        // A state longer than what is left.
        let mut payload = record(1, 1).encode()[FRAME_HEADER_LENGTH..].to_vec();
        payload.pop();
        assert!(read_log(&frame(&payload)).is_err());
    }

    /// One of each record that says which region holds what.
    fn records_of_regions() -> [LogRecord; 4] {
        [
            LogRecord::Granted {
                region: 4,
                tick: 2,
                chunks: vec![ChunkPos::new(1, -1), ChunkPos::new(-2, 3)],
            },
            LogRecord::Returned {
                region: 4,
                chunks: vec![ChunkPos::new(-2, 3)],
            },
            LogRecord::Absorbed {
                region: 4,
                epoch: 9,
                absorbed: 5,
                tick: 300,
                state: vec![7, 8],
            },
            LogRecord::Split {
                region: 4,
                epoch: 9,
                tick: 301,
                state: vec![7],
                part: 6,
                part_epoch: 2,
                chunks: vec![ChunkPos::new(1, -1)],
                part_state: vec![8, 9, 10],
            },
        ]
    }

    #[test]
    fn known_answers_of_the_records_of_regions() {
        let payloads: [&[u8]; 4] = [
            &[
                FORMAT_VERSION,
                4, // version, kind
                0,
                0,
                0,
                4, // region
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                2, // tick
                0,
                0,
                0,
                2, // two chunks
                0,
                0,
                0,
                1,
                0xFF,
                0xFF,
                0xFF,
                0xFF, // the first
                0xFF,
                0xFF,
                0xFF,
                0xFE,
                0,
                0,
                0,
                3, // the second
            ],
            &[
                FORMAT_VERSION,
                5, // version, kind
                0,
                0,
                0,
                4, // region
                0,
                0,
                0,
                1, // one chunk
                0xFF,
                0xFF,
                0xFF,
                0xFE,
                0,
                0,
                0,
                3, // the chunk
            ],
            &[
                FORMAT_VERSION,
                6, // version, kind
                0,
                0,
                0,
                4, // region
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                9, // epoch
                0,
                0,
                0,
                5, // absorbed
                0,
                0,
                0,
                0,
                0,
                0,
                1,
                44, // tick
                0,
                0,
                0,
                2,
                7,
                8, // the state
            ],
            &[
                FORMAT_VERSION,
                7, // version, kind
                0,
                0,
                0,
                4, // region
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                9, // epoch
                0,
                0,
                0,
                0,
                0,
                0,
                1,
                45, // tick
                0,
                0,
                0,
                1,
                7, // the state
                0,
                0,
                0,
                6, // part
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                2, // part epoch
                0,
                0,
                0,
                1, // one chunk
                0,
                0,
                0,
                1,
                0xFF,
                0xFF,
                0xFF,
                0xFF, // the chunk
                0,
                0,
                0,
                3,
                8,
                9,
                10, // the part's state
            ],
        ];
        for (record, payload) in records_of_regions().into_iter().zip(payloads) {
            let bytes = record.encode();
            assert_eq!(bytes[..4], (payload.len() as u32).to_be_bytes());
            assert_eq!(bytes[4..8], crc32fast::hash(payload).to_be_bytes());
            assert_eq!(bytes[8..], *payload, "{record:?}");
            assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));
        }
    }

    /// The records of regions are cut off and damaged like any other: whatever byte the
    /// process died at, the records written in full before are read back, and a record
    /// with a wrong bit ends the log.
    #[test]
    fn records_of_regions_cut_off_or_damaged_anywhere_end_the_log() {
        let records = records_of_regions();
        let mut log = Vec::new();
        let mut ends = Vec::new();
        for record in &records {
            log.extend_from_slice(&record.encode());
            ends.push(log.len());
        }
        for length in 0..=log.len() {
            let complete = ends.iter().filter(|end| **end <= length).count();
            let valid = ends[..complete].last().copied().unwrap_or(0);
            assert_eq!(
                read_log(&log[..length]),
                Ok((records[..complete].to_vec(), valid)),
                "cut at {length}"
            );
        }
        for index in 0..log.len() {
            let mut damaged = log.clone();
            damaged[index] ^= 0x10;
            let complete = ends.iter().filter(|end| **end <= index).count();
            let (read, valid) = read_log(&damaged).unwrap();
            assert_eq!(read, records[..complete], "byte {index}");
            assert_eq!(valid, ends[..complete].last().copied().unwrap_or(0));
        }
    }

    #[test]
    fn an_arrival_outside_the_chunk_it_is_sent_to_and_a_commit_of_arrivals_with_none_are_refused() {
        let entity = crate::entities::tests::item(1, [20.5, -60.0, 3.5], None);
        let commit =
            |arrivals: Vec<(ChunkPos, Entity, Due)>, taken: Vec<EffectId>| LogRecord::Commit {
                travellers: Vec::new(),
                region: 1,
                tick: 4,
                epoch: 1,
                changes: Vec::new(),
                state: Vec::new(),
                stays: Vec::new(),
                ticks: Vec::new(),
                marks_taken: Vec::new(),
                seams: Vec::new(),
                effects: Vec::new(),
                taken: Vec::new(),
                events: Vec::new(),
                claims: Vec::new(),
                raids: Vec::new(),
                entity_changes: Vec::new(),
                arrivals,
                arrivals_taken: taken,
            };
        let elsewhere = commit(
            vec![(ChunkPos::new(0, 0), entity.clone(), Due::FromTaking)],
            Vec::new(),
        );
        assert_eq!(
            read_log(&elsewhere.encode()),
            Err(FormatError::Corrupt("an arrival outside its chunk"))
        );
        // One taken arrival makes a commit of kind 16; with both lists emptied in its
        // bytes it is refused, as a commit has one form.
        let taken = commit(
            Vec::new(),
            vec![EffectId {
                sequence: 7,
                index: 0,
            }],
        );
        let mut bytes = taken.encode();
        assert_eq!(bytes[FRAME_HEADER_LENGTH + 1], KIND_COMMIT_WITH_ARRIVALS);
        let end = bytes.len();
        // The ids' count is the last u32 before the id: written as 1, made 0, and the id cut.
        let ids_at = end - 12 - 4;
        bytes[ids_at..ids_at + 4].copy_from_slice(&0u32.to_be_bytes());
        bytes.truncate(ids_at + 4);
        let payload = bytes[FRAME_HEADER_LENGTH..].to_vec();
        assert_eq!(
            read_log(&frame(&payload)),
            Err(FormatError::Corrupt("a commit with arrivals that has none"))
        );
    }

    #[test]
    fn a_record_of_regions_that_checks_out_but_makes_no_sense_is_an_error() {
        // Kinds there are none of. 8 to 15 were among them until a commit with stay
        // notes, one with the ticks of chunks, one with effects, the store's records
        // of effects, a commit with block entities, one with block events, one with
        // entity changes, one with arrivals, the store's records of arrivals and one with
        // claims and one with raids, the trader's chance, and a commit with travellers and
        // the store's record of them, became kinds of their own; 24 stands in for them.
        for kind in [0, 24, 255] {
            assert_eq!(
                read_log(&frame(&[FORMAT_VERSION, kind])),
                Err(FormatError::Corrupt("record kind"))
            );
        }
        for record in records_of_regions() {
            let payload = record.encode()[FRAME_HEADER_LENGTH..].to_vec();
            // A byte too few, a byte too many, and nothing but the version and the kind.
            assert_eq!(
                read_log(&frame(&payload[..payload.len() - 1])),
                Err(FormatError::Truncated),
                "{record:?}"
            );
            let mut longer = payload.clone();
            longer.push(0);
            assert_eq!(
                read_log(&frame(&longer)),
                Err(FormatError::Corrupt("trailing bytes")),
                "{record:?}"
            );
            assert_eq!(read_log(&frame(&payload[..2])), Err(FormatError::Truncated));
        }
        // More chunks than the payload holds, which must not be reserved either.
        let mut payload = vec![FORMAT_VERSION, KIND_RETURNED, 0, 0, 0, 4];
        payload.extend_from_slice(&u32::MAX.to_be_bytes());
        payload.extend_from_slice(&[0; 8]);
        assert_eq!(read_log(&frame(&payload)), Err(FormatError::Truncated));
    }

    /// A commit with the ticks of its chunks is written as kind 9, with its notes or
    /// none, and read back as it was; one without ticks keeps the kind it had.
    #[test]
    fn a_commit_with_the_ticks_of_chunks_is_read_back_as_it_was() {
        let ticks = vec![(
            ChunkPos::new(-3, 7),
            vec![
                ScheduledTick {
                    at: InChunk {
                        y: -61,
                        z: 15,
                        x: 0,
                    },
                    of: Ticked::Block(12),
                    delay: -1,
                    priority: 0,
                },
                ScheduledTick {
                    at: InChunk {
                        y: 300,
                        z: 0,
                        x: 15,
                    },
                    of: Ticked::Fluid(2),
                    delay: 4,
                    priority: -3,
                },
            ],
        )];
        for stays in [
            Vec::new(),
            vec![LoggedStay::Entering {
                player: 9,
                entity: 4,
            }],
        ] {
            let record = LogRecord::Commit {
                travellers: Vec::new(),
                region: 2,
                tick: 40,
                epoch: 3,
                changes: vec![(BlockPos::new(1, 2, 3), BlockState(5)).into()],
                state: b"delta".to_vec(),
                stays,
                ticks: ticks.clone(),
                marks_taken: Vec::new(),
                seams: Vec::new(),
                effects: Vec::new(),
                taken: Vec::new(),
                events: Vec::new(),
                claims: Vec::new(),
                raids: Vec::new(),
                entity_changes: Vec::new(),
                arrivals: Vec::new(),
                arrivals_taken: Vec::new(),
            };
            let bytes = record.encode();
            assert_eq!(bytes[FRAME_HEADER_LENGTH + 1], KIND_COMMIT_WITH_TICKS);
            assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));
        }
    }

    // Tests of the commit with the ticks of chunks (ADR-0024, section 4, "The format"),
    // written from the record by someone who did not write the code.

    /// A tick as a chunk keeps it, of a block or of a fluid.
    fn kept_tick(x: u8, y: i16, z: u8, of: Ticked, delay: i32, priority: i8) -> ScheduledTick {
        ScheduledTick {
            at: InChunk { y, z, x },
            of,
            delay,
            priority,
        }
    }

    /// The ticks of three chunks: one with ticks of both queues, one whose list became
    /// empty in the tick, and one with a tick of delay 0 scheduled in the tick, which is
    /// kept as −1.
    fn ticks_of_three_chunks() -> Vec<(ChunkPos, Vec<ScheduledTick>)> {
        vec![
            (
                ChunkPos::new(-4, 2),
                vec![
                    kept_tick(0, -64, 15, Ticked::Block(200), -1, 0),
                    kept_tick(7, 0, 7, Ticked::Fluid(2), 5, -3),
                    kept_tick(15, 319, 0, Ticked::Block(u16::MAX), i32::MAX, i8::MIN),
                ],
            ),
            (ChunkPos::new(-1, 0), Vec::new()),
            (
                ChunkPos::new(30_000, -30_000),
                vec![kept_tick(3, -61, 4, Ticked::Block(1), -1, i8::MAX)],
            ),
        ]
    }

    /// A commit of `tick` with these ticks, without notes of stays.
    fn commit_with_ticks(tick: u64, ticks: Vec<(ChunkPos, Vec<ScheduledTick>)>) -> LogRecord {
        LogRecord::Commit {
            travellers: Vec::new(),
            region: 6,
            tick,
            epoch: 2,
            changes: vec![
                (BlockPos::new(-60, -61, 40), blocks::AIR).into(),
                (BlockPos::new(-60, -60, 40), blocks::SAND).into(),
            ],
            state: b"a delta".to_vec(),
            stays: Vec::new(),
            ticks,
            marks_taken: Vec::new(),
            seams: Vec::new(),
            effects: Vec::new(),
            taken: Vec::new(),
            events: Vec::new(),
            entity_changes: Vec::new(),
            arrivals: Vec::new(),
            arrivals_taken: Vec::new(),
            claims: Vec::new(),
            raids: Vec::new(),
        }
    }

    /// The ticks of several chunks are read back as they were written: every field of
    /// every tick, the order of the chunks and of each chunk's list, a list that is
    /// empty, and delays of −1.
    #[test]
    fn a_commit_with_the_ticks_of_several_chunks_with_an_empty_list_and_negative_delays_round_trips()
     {
        let record = commit_with_ticks(12, ticks_of_three_chunks());
        let bytes = record.encode();
        assert_eq!(
            bytes[FRAME_HEADER_LENGTH..FRAME_HEADER_LENGTH + 2],
            [FORMAT_VERSION, 9]
        );
        assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));
    }

    /// A list that became empty is committed as empty (section 4): a commit whose only
    /// ticks are one empty list is still a commit with ticks, and that list is read
    /// back. Written as a commit without ticks, the chunk's ticks would not be cleared
    /// when the commit is replayed.
    #[test]
    fn a_commit_whose_only_ticks_are_an_empty_list_keeps_that_list() {
        let record = commit_with_ticks(3, vec![(ChunkPos::new(0, 0), Vec::new())]);
        let bytes = record.encode();
        assert_eq!(bytes[FRAME_HEADER_LENGTH + 1], 9);
        assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));
    }

    /// The commit with ticks is the commit of kind 2 with the notes of stays and the
    /// ticks after it, as the table at the top of this file says: what comes before
    /// the notes is written as in a commit without ticks, and a commit without notes
    /// says that it has none.
    #[test]
    fn a_commit_with_ticks_has_the_fields_of_a_commit_without_them_first() {
        let with = commit_with_ticks(12, ticks_of_three_chunks());
        let LogRecord::Commit {
            region,
            tick,
            epoch,
            changes,
            state,
            ..
        } = with.clone()
        else {
            unreachable!()
        };
        let without = LogRecord::Commit {
            travellers: Vec::new(),
            region,
            tick,
            epoch,
            changes,
            state,
            stays: Vec::new(),
            ticks: Vec::new(),
            marks_taken: Vec::new(),
            seams: Vec::new(),
            effects: Vec::new(),
            taken: Vec::new(),
            events: Vec::new(),
            entity_changes: Vec::new(),
            arrivals: Vec::new(),
            arrivals_taken: Vec::new(),
            claims: Vec::new(),
            raids: Vec::new(),
        };
        let with = with.encode()[FRAME_HEADER_LENGTH..].to_vec();
        let without = without.encode()[FRAME_HEADER_LENGTH..].to_vec();
        assert_eq!(without[..2], [FORMAT_VERSION, 2]);
        assert_eq!(with[..2], [FORMAT_VERSION, 9]);
        assert_eq!(with[2..without.len()], without[2..]);
        // No notes.
        assert_eq!(with[without.len()..without.len() + 4], [0, 0, 0, 0]);
        // Three chunks.
        assert_eq!(with[without.len() + 4..without.len() + 8], [0, 0, 0, 3]);
    }

    /// A commit with ticks and notes of stays keeps both.
    #[test]
    fn a_commit_with_ticks_and_notes_of_stays_keeps_both() {
        let LogRecord::Commit { stays, .. } = commit_with_stays() else {
            unreachable!()
        };
        let LogRecord::Commit {
            region,
            tick,
            epoch,
            changes,
            state,
            ticks,
            ..
        } = commit_with_ticks(12, ticks_of_three_chunks())
        else {
            unreachable!()
        };
        let record = LogRecord::Commit {
            travellers: Vec::new(),
            region,
            tick,
            epoch,
            changes,
            state,
            stays,
            ticks,
            marks_taken: Vec::new(),
            seams: Vec::new(),
            effects: Vec::new(),
            taken: Vec::new(),
            events: Vec::new(),
            entity_changes: Vec::new(),
            arrivals: Vec::new(),
            arrivals_taken: Vec::new(),
            claims: Vec::new(),
            raids: Vec::new(),
        };
        let bytes = record.encode();
        assert_eq!(bytes[FRAME_HEADER_LENGTH + 1], 9);
        assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));
    }

    /// A commit without ticks is written byte for byte as it was before there were
    /// any, with notes of stays or without: the known answers of kinds 2 and 8.
    #[test]
    fn a_commit_without_ticks_is_written_byte_for_byte_as_before() {
        let plain = LogRecord::Commit {
            travellers: Vec::new(),
            region: 4,
            tick: 2,
            epoch: 1,
            changes: vec![(BlockPos::new(1, -1, 3), BlockState(9)).into()],
            state: vec![7, 8],
            stays: Vec::new(),
            ticks: Vec::new(),
            marks_taken: Vec::new(),
            seams: Vec::new(),
            effects: Vec::new(),
            taken: Vec::new(),
            events: Vec::new(),
            entity_changes: Vec::new(),
            arrivals: Vec::new(),
            arrivals_taken: Vec::new(),
            claims: Vec::new(),
            raids: Vec::new(),
        };
        let payload = [
            FORMAT_VERSION,
            2, // version, kind
            0,
            0,
            0,
            4, // region
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            2, // tick
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            1, // epoch
            0,
            0,
            0,
            1, // one change
            0,
            0,
            0,
            1,
            0xFF,
            0xFF,
            0xFF,
            0xFF,
            0,
            0,
            0,
            3,
            0,
            9, // the change
            0,
            0,
            0,
            2,
            7,
            8, // the state
        ];
        assert_eq!(plain.encode(), frame(&payload));
        // The notes of the commit with stays follow its state directly, as in kind 8.
        let bytes = commit_with_stays().encode();
        assert_eq!(
            bytes[FRAME_HEADER_LENGTH..FRAME_HEADER_LENGTH + 2],
            [FORMAT_VERSION, 8]
        );
        assert_eq!(
            bytes[FRAME_HEADER_LENGTH + 26..FRAME_HEADER_LENGTH + 36],
            [0, 0, 0, 2, 7, 8, 0, 0, 0, 2]
        );
    }

    /// A log written before there were commits with ticks, of records of kinds 1, 2,
    /// 3 and 8 in bytes as they were written then, is read as it was, its commits
    /// without ticks; and a commit with ticks appended to it is read after them.
    #[test]
    fn an_older_log_without_commits_with_ticks_reads_unchanged() {
        let changes = [
            FORMAT_VERSION,
            1, // version, kind
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            2, // tick
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            1, // epoch
            0,
            0,
            0,
            1, // one change
            0,
            0,
            0,
            1,
            0xFF,
            0xFF,
            0xFF,
            0xFF,
            0,
            0,
            0,
            3,
            0,
            9,
        ];
        let commit = [
            FORMAT_VERSION,
            2, // version, kind
            0,
            0,
            0,
            4, // region
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            2, // tick
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            1, // epoch
            0,
            0,
            0,
            1, // one change
            0,
            0,
            0,
            1,
            0xFF,
            0xFF,
            0xFF,
            0xFF,
            0,
            0,
            0,
            3,
            0,
            9, // the change
            0,
            0,
            0,
            2,
            7,
            8, // the state
        ];
        let opened = [
            FORMAT_VERSION,
            3, // version, kind
            0,
            0,
            0,
            4, // region
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            9, // epoch
            0,
            0,
            0,
            0,
            0,
            0,
            1,
            44, // restored
        ];
        let mut log = frame(&changes);
        log.extend(frame(&commit));
        log.extend(commit_with_stays().encode());
        log.extend(frame(&opened));
        let older = vec![
            LogRecord::Changes {
                tick: 2,
                epoch: 1,
                changes: vec![(BlockPos::new(1, -1, 3), BlockState(9))],
            },
            LogRecord::Commit {
                region: 4,
                tick: 2,
                epoch: 1,
                changes: vec![(BlockPos::new(1, -1, 3), BlockState(9)).into()],
                state: vec![7, 8],
                stays: Vec::new(),
                ticks: Vec::new(),
                marks_taken: Vec::new(),
                seams: Vec::new(),
                effects: Vec::new(),
                taken: Vec::new(),
                events: Vec::new(),
                claims: Vec::new(),
                raids: Vec::new(),
                entity_changes: Vec::new(),
                arrivals: Vec::new(),
                arrivals_taken: Vec::new(),
                travellers: Vec::new(),
            },
            commit_with_stays(),
            LogRecord::Opened {
                region: 4,
                epoch: 9,
                restored: 300,
            },
        ];
        assert_eq!(read_log(&log), Ok((older.clone(), log.len())));

        let newer = commit_with_ticks(3, ticks_of_three_chunks());
        log.extend(newer.encode());
        let mut all = older;
        all.push(newer);
        assert_eq!(read_log(&log), Ok((all, log.len())));
    }

    /// A commit with ticks is cut off and damaged like any other record: what is left
    /// of it is not read, and what came before it is.
    #[test]
    fn a_commit_with_ticks_cut_off_or_damaged_anywhere_ends_the_log() {
        let first = record(1, 2);
        let mut log = first.encode();
        let start = log.len();
        let with_ticks = commit_with_ticks(2, ticks_of_three_chunks());
        log.extend_from_slice(&with_ticks.encode());
        for length in start..log.len() {
            assert_eq!(
                read_log(&log[..length]),
                Ok((vec![first.clone()], start)),
                "cut at {length}"
            );
        }
        for index in start..log.len() {
            let mut damaged = log.clone();
            damaged[index] ^= 0x10;
            assert_eq!(
                read_log(&damaged),
                Ok((vec![first.clone()], start)),
                "byte {index}"
            );
        }
        assert_eq!(read_log(&log), Ok((vec![first, with_ticks], log.len())));
    }

    /// A commit whose changes are blocks and block entities' data, interleaved.
    fn commit_with_entities(changes: Vec<Change>) -> LogRecord {
        LogRecord::Commit {
            travellers: Vec::new(),
            region: 2,
            tick: 7,
            epoch: 3,
            changes,
            state: vec![1, 2, 3],
            stays: Vec::new(),
            ticks: Vec::new(),
            marks_taken: Vec::new(),
            seams: Vec::new(),
            effects: Vec::new(),
            taken: Vec::new(),
            events: Vec::new(),
            entity_changes: Vec::new(),
            arrivals: Vec::new(),
            arrivals_taken: Vec::new(),
            claims: Vec::new(),
            raids: Vec::new(),
        }
    }

    /// `docs/adr/0026-redstone.md`, section 6: the tick's changes are read back in the
    /// order it made them, with a block entity's data before every block, between
    /// them, after them all, twice at one place, or with no blocks at all.
    #[test]
    fn a_commit_with_block_entities_is_read_back_in_the_order_of_its_changes() {
        let at = |x| BlockPos::new(x, -60, 4);
        let entity = |x, data: &[u8]| Change::BlockEntity(at(x), data.to_vec());
        let block = |x| Change::Block(at(x), BlockState(5544));
        for changes in [
            vec![entity(0, b"first"), block(1), block(2)],
            vec![block(1), entity(1, b"between"), block(2)],
            vec![block(1), block(2), entity(2, b"last")],
            vec![block(1), entity(1, b""), entity(1, b"again"), block(2)],
            vec![entity(3, b"alone")],
        ] {
            let record = commit_with_entities(changes.clone());
            let bytes = record.encode();
            assert_eq!(
                bytes[FRAME_HEADER_LENGTH + 1],
                KIND_COMMIT_WITH_ENTITIES,
                "{changes:?}"
            );
            assert_eq!(
                read_log(&bytes),
                Ok((vec![record], bytes.len())),
                "{changes:?}"
            );
        }
    }

    /// A commit of blocks alone is written as it was before block entities were.
    #[test]
    fn a_commit_of_blocks_alone_is_not_written_with_block_entities() {
        let record =
            commit_with_entities(vec![Change::Block(BlockPos::new(1, 2, 3), BlockState(9))]);
        assert_eq!(record.encode()[FRAME_HEADER_LENGTH + 1], KIND_COMMIT);
    }

    /// A block entity's place that runs backwards or past the blocks is corrupt, and
    /// so is a commit of kind 13 with none: neither is ever written.
    #[test]
    fn a_commit_with_block_entities_out_of_place_is_an_error() {
        let blocks = vec![
            (BlockPos::new(1, 2, 3), BlockState(9)),
            (BlockPos::new(1, 2, 4), BlockState(9)),
        ];
        let entity = |place: u32| (place, BlockPos::new(1, 2, 3), b"data".to_vec());
        assert!(join_changes(blocks.clone(), vec![entity(1), entity(0)]).is_err());
        assert!(join_changes(blocks.clone(), vec![entity(3)]).is_err());
        assert_eq!(
            join_changes(blocks.clone(), vec![entity(2), entity(2)]).map(|changes| changes.len()),
            Ok(4)
        );
        // Kind 13 with no block entities: the fields of kind 10 with every count 0,
        // and a count of 0 for the block entities.
        let mut payload = vec![FORMAT_VERSION, KIND_COMMIT_WITH_ENTITIES];
        payload.extend_from_slice(&2u32.to_be_bytes());
        payload.extend_from_slice(&7u64.to_be_bytes());
        payload.extend_from_slice(&3u64.to_be_bytes());
        put_changes(&mut payload, &blocks);
        put_bytes(&mut payload, &[1]);
        for _ in 0..4 {
            payload.extend_from_slice(&0u32.to_be_bytes());
        }
        crate::effects::put_effects(&mut payload, &[]);
        crate::effects::put_ids(&mut payload, &[]);
        payload.extend_from_slice(&0u32.to_be_bytes());
        assert!(LogRecord::decode(&payload).is_err());
    }

    /// `docs/adr/0026-redstone.md`, section 4: the block events chunks keep are read
    /// back with the commit, an empty list among them, with or without block entities.
    #[test]
    fn a_commit_with_block_events_kept_in_chunks_is_read_back_as_it_was() {
        let event = |data| KeptEvent {
            at: InChunk::new(3, -61, 9),
            block: 1200,
            kind: 0,
            data,
        };
        let events = vec![
            (ChunkPos::new(-2, 5), vec![event(1), event(2)]),
            (ChunkPos::new(0, 0), Vec::new()),
        ];
        for changes in [
            Vec::new(),
            vec![Change::BlockEntity(
                BlockPos::new(1, 2, 3),
                b"data".to_vec(),
            )],
        ] {
            let mut record = commit_with_entities(changes);
            if let LogRecord::Commit { events: with, .. } = &mut record {
                *with = events.clone();
            }
            let bytes = record.encode();
            assert_eq!(bytes[FRAME_HEADER_LENGTH + 1], KIND_COMMIT_WITH_EVENTS);
            assert_eq!(read_log(&bytes), Ok((vec![record.clone()], bytes.len())));
            for length in 0..bytes.len() {
                assert_eq!(
                    read_log(&bytes[..length]),
                    Ok((Vec::new(), 0)),
                    "cut at {length}"
                );
            }
            for index in 0..bytes.len() {
                let mut damaged = bytes.clone();
                damaged[index] ^= 0x10;
                assert_eq!(read_log(&damaged), Ok((Vec::new(), 0)), "byte {index}");
            }
        }
    }

    /// The claims of chunks whose claims changed are read back with the commit, a chunk
    /// whose last claim went (an empty list) among them, with or without arrivals,
    /// written as kind 19 (`docs/groundwork/n10-plan.md`, section 3.2).
    #[test]
    fn a_commit_with_claims_is_read_back_as_it_was() {
        use clustine_world::poi::PoiType;
        let claims = vec![
            (
                ChunkPos::new(-2, 5),
                vec![Claim {
                    at: InChunk::new(3, -61, 9),
                    kind: PoiType::Home,
                    free: 0,
                }],
            ),
            (ChunkPos::new(0, 0), Vec::new()),
        ];
        for changes in [
            Vec::new(),
            vec![Change::BlockEntity(
                BlockPos::new(1, 2, 3),
                b"data".to_vec(),
            )],
        ] {
            let mut record = commit_with_entities(changes);
            if let LogRecord::Commit { claims: with, .. } = &mut record {
                *with = claims.clone();
            }
            let bytes = record.encode();
            assert_eq!(bytes[FRAME_HEADER_LENGTH + 1], KIND_COMMIT_WITH_CLAIMS);
            assert_eq!(read_log(&bytes), Ok((vec![record.clone()], bytes.len())));
            for length in 0..bytes.len() {
                assert_eq!(
                    read_log(&bytes[..length]),
                    Ok((Vec::new(), 0)),
                    "cut at {length}"
                );
            }
            for index in 0..bytes.len() {
                let mut damaged = bytes.clone();
                damaged[index] ^= 0x10;
                assert_eq!(read_log(&damaged), Ok((Vec::new(), 0)), "byte {index}");
            }
        }
    }

    /// The raids of chunks whose raids changed are read back with the commit, written as
    /// kind 20 (`docs/groundwork/n10-plan.md`, section 9.1).
    #[test]
    fn a_commit_with_raids_is_read_back_as_it_was() {
        let mut record = commit_with_entities(Vec::new());
        if let LogRecord::Commit { raids, .. } = &mut record {
            *raids = vec![
                (ChunkPos::new(-3, 0), vec![crate::raids::tests::raid(4)]),
                (ChunkPos::new(2, 2), Vec::new()),
            ];
        }
        let bytes = record.encode();
        assert_eq!(bytes[FRAME_HEADER_LENGTH + 1], KIND_COMMIT_WITH_RAIDS);
        assert_eq!(read_log(&bytes), Ok((vec![record.clone()], bytes.len())));
        for length in 0..bytes.len() {
            assert_eq!(
                read_log(&bytes[..length]),
                Ok((Vec::new(), 0)),
                "cut at {length}"
            );
        }
    }

    /// A commit with nothing but one entity change, as section 4 of
    /// `docs/groundwork/n2-plan.md` lays it out: the fields of kind 14 with every count
    /// 0, then the changes by chunk.
    #[test]
    fn known_answer_of_a_commit_with_entity_changes() {
        let key = clustine_world::EntityId(0x2_0000_0001);
        let record = LogRecord::Commit {
            travellers: Vec::new(),
            region: 1,
            tick: 3,
            epoch: 2,
            changes: Vec::new(),
            state: Vec::new(),
            stays: Vec::new(),
            ticks: Vec::new(),
            marks_taken: Vec::new(),
            seams: Vec::new(),
            effects: Vec::new(),
            taken: Vec::new(),
            events: Vec::new(),
            entity_changes: vec![
                (
                    ChunkPos::new(-2, 0),
                    EntityChange::Set(
                        key,
                        vec![
                            clustine_world::EntityField::OnGround(true),
                            clustine_world::EntityField::Lived(258),
                        ],
                    ),
                ),
                (ChunkPos::new(-2, 0), EntityChange::Gone(key)),
            ],
            arrivals: Vec::new(),
            arrivals_taken: Vec::new(),
            claims: Vec::new(),
            raids: Vec::new(),
        };
        let bytes = record.encode();
        let mut payload = vec![FORMAT_VERSION, 15];
        payload.extend_from_slice(&[0, 0, 0, 1]); // region
        payload.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 3]); // tick
        payload.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 2]); // epoch
        payload.extend_from_slice(&[0, 0, 0, 0]); // no block changes
        payload.extend_from_slice(&[0, 0, 0, 0]); // an empty state
        // Stays, ticks, marks taken, seams, effects, taken, block entities, events.
        for _ in 0..8 {
            payload.extend_from_slice(&[0, 0, 0, 0]);
        }
        payload.extend_from_slice(&[0, 0, 0, 1]); // one chunk
        payload.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFE, 0, 0, 0, 0]); // -2, 0
        payload.extend_from_slice(&[0, 0, 0, 2]); // two changes
        payload.push(3); // set
        payload.extend_from_slice(&[0, 0, 0, 2, 0, 0, 0, 1]); // the key
        payload.extend_from_slice(&[0, 0, 0, 2]); // two fields
        payload.extend_from_slice(&[4, 1]); // on the ground
        payload.extend_from_slice(&[6, 0, 0, 1, 2]); // age 258
        payload.push(2); // gone
        payload.extend_from_slice(&[0, 0, 0, 2, 0, 0, 0, 1]); // the key
        assert_eq!(bytes[FRAME_HEADER_LENGTH..], payload);
        assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));
    }

    /// Kind 15 reads the block entities and block events of kind 14, which may be none
    /// there, and refuses a list of entity changes that is empty: a commit without any
    /// is of the kinds it always was.
    #[test]
    fn a_commit_with_entity_changes_has_one_form() {
        let change = (
            ChunkPos::new(0, 0),
            EntityChange::Gone(clustine_world::EntityId(9)),
        );
        let event = KeptEvent {
            at: InChunk::new(3, -61, 9),
            block: 1200,
            kind: 0,
            data: 4,
        };
        for (changes, events) in [
            (Vec::new(), Vec::new()),
            (
                vec![Change::BlockEntity(
                    BlockPos::new(1, 2, 3),
                    b"chest".to_vec(),
                )],
                Vec::new(),
            ),
            (Vec::new(), vec![(ChunkPos::new(0, 0), vec![event])]),
        ] {
            let mut record = commit_with_entities(changes);
            if let LogRecord::Commit {
                events: with,
                entity_changes,
                ..
            } = &mut record
            {
                *with = events;
                *entity_changes = vec![change.clone()];
            }
            let bytes = record.encode();
            assert_eq!(
                bytes[FRAME_HEADER_LENGTH + 1],
                KIND_COMMIT_WITH_ENTITY_CHANGES
            );
            assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));
        }
        // Without them, of the kind it always was.
        assert_eq!(
            commit_with_entities(Vec::new()).encode()[FRAME_HEADER_LENGTH + 1],
            2
        );
        // The fields of kind 14 with every count 0, and no entity changes.
        let mut payload = vec![FORMAT_VERSION, KIND_COMMIT_WITH_ENTITY_CHANGES];
        payload.extend_from_slice(&2u32.to_be_bytes());
        payload.extend_from_slice(&7u64.to_be_bytes());
        payload.extend_from_slice(&3u64.to_be_bytes());
        for _ in 0..11 {
            payload.extend_from_slice(&0u32.to_be_bytes());
        }
        assert!(LogRecord::decode(&payload).is_err());
    }

    /// Cut off or damaged anywhere, a commit with entity changes ends the log there.
    #[test]
    fn a_commit_with_entity_changes_cut_off_or_damaged_anywhere_ends_the_log() {
        let first = record(1, 2);
        let mut log = first.encode();
        let start = log.len();
        let mut with_entities = commit_with_entities(vec![Change::Block(
            BlockPos::new(1, 2, 3),
            BlockState(5544),
        )]);
        if let LogRecord::Commit { entity_changes, .. } = &mut with_entities {
            *entity_changes = vec![(
                ChunkPos::new(0, 0),
                EntityChange::Whole(crate::entities::tests::item(4, [1.5, 3.0, 2.5], Some(1))),
            )];
        }
        log.extend_from_slice(&with_entities.encode());
        for length in start..log.len() {
            assert_eq!(
                read_log(&log[..length]),
                Ok((vec![first.clone()], start)),
                "cut at {length}"
            );
        }
        for index in start..log.len() {
            let mut damaged = log.clone();
            damaged[index] ^= 0x10;
            assert_eq!(
                read_log(&damaged),
                Ok((vec![first.clone()], start)),
                "byte {index}"
            );
        }
        assert_eq!(read_log(&log), Ok((vec![first, with_entities], log.len())));
    }

    /// Cut off or damaged anywhere, a commit with block entities ends the log there.
    #[test]
    fn a_commit_with_block_entities_cut_off_or_damaged_anywhere_ends_the_log() {
        let first = record(1, 2);
        let mut log = first.encode();
        let start = log.len();
        let with_entities = commit_with_entities(vec![
            Change::Block(BlockPos::new(1, 2, 3), BlockState(5544)),
            Change::BlockEntity(BlockPos::new(1, 2, 3), b"chest".to_vec()),
        ]);
        log.extend_from_slice(&with_entities.encode());
        for length in start..log.len() {
            assert_eq!(
                read_log(&log[..length]),
                Ok((vec![first.clone()], start)),
                "cut at {length}"
            );
        }
        for index in start..log.len() {
            let mut damaged = log.clone();
            damaged[index] ^= 0x10;
            assert_eq!(
                read_log(&damaged),
                Ok((vec![first.clone()], start)),
                "byte {index}"
            );
        }
        assert_eq!(read_log(&log), Ok((vec![first, with_entities], log.len())));
    }

    /// What the store compares a record with before it writes it.
    #[test]
    fn a_record_longer_than_the_limit_is_read_as_cut_off() {
        let absorbed = |state: usize| {
            LogRecord::Absorbed {
                region: 1,
                epoch: 1,
                absorbed: 2,
                tick: 1,
                state: vec![0; state],
            }
            .encode()
        };
        // The payload has 30 bytes besides the state.
        let fits = absorbed(MAX_PAYLOAD_LENGTH - 30);
        assert_eq!(fits.len(), MAX_RECORD_LENGTH);
        let (records, valid) = read_log(&fits).unwrap();
        assert_eq!((records.len(), valid), (1, fits.len()));

        let mut too_long = absorbed(MAX_PAYLOAD_LENGTH - 29);
        assert_eq!(too_long.len(), MAX_RECORD_LENGTH + 1);
        // And whatever follows it is hidden with it.
        too_long.extend(records_of_regions()[0].encode());
        assert_eq!(read_log(&too_long), Ok((Vec::new(), 0)));
    }

    /// A place in which nothing is as it is by default: something in the hotbar, on the
    /// head and in the off hand.
    fn place() -> LoggedPlace {
        let mut inventory = [const { None }; INVENTORY_SLOTS];
        inventory[0] = Some(ItemStack::new(1, 64));
        inventory[8] = Some(ItemStack::new(-2, 258));
        inventory[39] = Some(ItemStack::new(3, 1));
        inventory[40] = Some(ItemStack::new(5, 16));
        let mut ender = [const { None }; ENDER_SLOTS];
        ender[0] = Some(ItemStack::new(6, 1));
        LoggedPlace {
            health: 20.0,
            dead: false,
            dimension: Default::default(),
            travel: None,
            seen_credits: false,
            portal_cooldown: 0,
            respawn: None,
            experience: (5, 0.25, 87),
            position: [1.5, -2.0, 0.0],
            yaw: 90.0,
            pitch: -45.0,
            on_ground: false,
            flying: true,
            selected_slot: 8,
            inventory,
            ender,
            effects: Vec::new(),
            time_since_rest: 0,
            raid_omen_position: None,
        }
    }

    /// A commit with one note of each kind.
    fn commit_with_stays() -> LogRecord {
        LogRecord::Commit {
            travellers: Vec::new(),
            region: 4,
            tick: 2,
            epoch: 1,
            changes: Vec::new(),
            state: vec![7, 8],
            stays: vec![
                LoggedStay::Entering {
                    player: 0x0102_0304_0506_0708_090A_0B0C_0D0E_0F10,
                    entity: 258,
                },
                LoggedStay::Has {
                    unit: None,
                    player: 3,
                    entity: u64::MAX,
                    hops: 5,
                    place: place(),
                },
            ],
            ticks: Vec::new(),
            marks_taken: Vec::new(),
            seams: Vec::new(),
            effects: Vec::new(),
            taken: Vec::new(),
            events: Vec::new(),
            entity_changes: Vec::new(),
            arrivals: Vec::new(),
            arrivals_taken: Vec::new(),
            claims: Vec::new(),
            raids: Vec::new(),
        }
    }

    /// A `Has` with the unit its player left the world on, and a taking of a unit, come
    /// back as they went (`docs/groundwork/n7h-logout-on-a-mount.md`, section 9), beside
    /// the kinds there were before.
    #[test]
    fn a_commit_with_a_unit_in_a_note_and_a_taking_round_trips() {
        let LogRecord::Commit { mut stays, .. } = commit_with_stays() else {
            unreachable!("a commit")
        };
        let unit = LoggedUnit(vec![
            crate::entities::tests::item(40, [1.5, -60.0, 2.5], None),
            crate::entities::tests::item(41, [1.5, -59.0, 2.5], Some(7)),
        ]);
        stays.push(LoggedStay::Has {
            player: 9,
            entity: 12,
            hops: 1,
            place: place(),
            unit: Some(unit.clone()),
        });
        stays.push(LoggedStay::Took {
            player: 9,
            root: 40,
        });
        let LogRecord::Commit {
            region,
            tick,
            epoch,
            changes,
            state,
            ticks,
            marks_taken,
            seams,
            effects,
            taken,
            events,
            entity_changes,
            arrivals,
            arrivals_taken,
            ..
        } = commit_with_stays()
        else {
            unreachable!("a commit")
        };
        let record = LogRecord::Commit {
            travellers: Vec::new(),
            region,
            tick,
            epoch,
            changes,
            state,
            stays,
            ticks,
            marks_taken,
            seams,
            effects,
            taken,
            events,
            entity_changes,
            arrivals,
            arrivals_taken,
            claims: Vec::new(),
            raids: Vec::new(),
        };
        let bytes = record.encode();
        assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));
        // A unit without its root is no unit.
        let mut empty = Vec::new();
        put_unit(&mut empty, &LoggedUnit(Vec::new()));
        assert_eq!(
            take_unit(&mut Input(&empty)),
            Err(FormatError::Corrupt("a unit without its root"))
        );
        assert_eq!(unit.root().key.0, 40);
    }

    /// A commit with stay notes is a record of a kind of its own: the fields of a
    /// commit, and then the notes.
    #[test]
    fn known_answer_of_a_commit_with_stays() {
        let record = commit_with_stays();
        let bytes = record.encode();
        let mut payload = vec![
            FORMAT_VERSION,
            8, // version, kind
            0,
            0,
            0,
            4, // region
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            2, // tick
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            1, // epoch
            0,
            0,
            0,
            0, // no changes
            0,
            0,
            0,
            2,
            7,
            8, // the state
            0,
            0,
            0,
            2, // two notes
            1, // entering
            1,
            2,
            3,
            4,
            5,
            6,
            7,
            8,
            9,
            10,
            11,
            12,
            13,
            14,
            15,
            16, // the player
            0,
            0,
            0,
            0,
            0,
            0,
            1,
            2, // the entity
            2, // has
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            3, // the player
            0xFF,
            0xFF,
            0xFF,
            0xFF,
            0xFF,
            0xFF,
            0xFF,
            0xFF, // the entity
            0,
            0,
            0,
            5, // hops
            0x3F,
            0xF8,
            0,
            0,
            0,
            0,
            0,
            0, // x: 1.5
            0xC0,
            0,
            0,
            0,
            0,
            0,
            0,
            0, // y: -2
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0, // z: 0
            0x42,
            0xB4,
            0,
            0, // yaw: 90
            0xC2,
            0x34,
            0,
            0, // pitch: -45
            2, // flying, not on the ground
            8, // the selected slot
            1,
            0,
            0,
            0,
            1,
            0,
            0,
            0,
            64,
            0,
            0,
            0,
            0, // the first slot: no components added and none removed
            0,
            0,
            0,
            0,
            0,
            0,
            0, // seven empty slots
            1,
            0xFF,
            0xFF,
            0xFF,
            0xFE,
            0,
            0,
            1,
            2,
            0,
            0,
            0,
            0, // the last slot of the hotbar
        ];
        // Thirty empty slots: the rest of the hotbar's and the armour's from the feet up
        // to the chest.
        payload.extend_from_slice(&[0; 30]);
        // The head and the off hand.
        payload.extend_from_slice(&[1, 0, 0, 0, 3, 0, 0, 0, 1, 0, 0, 0, 0]);
        payload.extend_from_slice(&[1, 0, 0, 0, 5, 0, 0, 0, 16, 0, 0, 0, 0]);
        // The ender chest: one of item 6 in its first slot, and 26 empty.
        payload.extend_from_slice(&[1, 0, 0, 0, 6, 0, 0, 0, 1, 0, 0, 0, 0]);
        payload.extend_from_slice(&[0; 26]);
        // The experience: level 5, a quarter of the way to the next, 87 points.
        payload.extend_from_slice(&[0, 0, 0, 5, 0x3E, 0x80, 0, 0, 0, 0, 0, 87]);
        // No respawn point, no effects, no time since rest and no raid omen's place.
        payload.push(0);
        payload.extend_from_slice(&[0; 4]);
        payload.extend_from_slice(&[0; 4]);
        payload.push(0);
        // The overworld, the credits not seen, no portal cooldown and no travel.
        payload.push(0);
        payload.push(0);
        payload.extend_from_slice(&[0; 4]);
        payload.push(0);
        // Full health, 20, and not dead.
        payload.extend_from_slice(&20.0_f32.to_be_bytes());
        payload.push(0);
        assert_eq!(bytes[..4], (payload.len() as u32).to_be_bytes());
        assert_eq!(bytes[4..8], crc32fast::hash(&payload).to_be_bytes());
        assert_eq!(bytes[8..], payload);
        assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));
    }

    /// A commit without notes is written as it was before there were any, and such a
    /// record is read as a commit without notes.
    #[test]
    fn a_commit_without_stays_is_of_the_kind_it_always_was() {
        let LogRecord::Commit {
            region,
            tick,
            epoch,
            changes,
            state,
            ..
        } = commit_with_stays()
        else {
            unreachable!()
        };
        let without = LogRecord::Commit {
            travellers: Vec::new(),
            region,
            tick,
            epoch,
            changes,
            state,
            stays: Vec::new(),
            ticks: Vec::new(),
            marks_taken: Vec::new(),
            seams: Vec::new(),
            effects: Vec::new(),
            taken: Vec::new(),
            events: Vec::new(),
            entity_changes: Vec::new(),
            arrivals: Vec::new(),
            arrivals_taken: Vec::new(),
            claims: Vec::new(),
            raids: Vec::new(),
        };
        let bytes = without.encode();
        assert_eq!(bytes[8..10], [FORMAT_VERSION, KIND_COMMIT]);
        // Nothing follows the state.
        assert_eq!(bytes[bytes.len() - 6..], [0, 0, 0, 2, 7, 8]);
        assert_eq!(read_log(&bytes), Ok((vec![without], bytes.len())));
        assert_eq!(commit_with_stays().encode()[8..10], [FORMAT_VERSION, 8]);
    }

    /// A commit with stays is cut off and damaged like any other record.
    #[test]
    fn a_commit_with_stays_cut_off_or_damaged_anywhere_ends_the_log() {
        let first = record(1, 2);
        let mut log = first.encode();
        let start = log.len();
        log.extend_from_slice(&commit_with_stays().encode());
        for length in start..log.len() {
            assert_eq!(
                read_log(&log[..length]),
                Ok((vec![first.clone()], start)),
                "cut at {length}"
            );
        }
        for index in start..log.len() {
            let mut damaged = log.clone();
            damaged[index] ^= 0x10;
            assert_eq!(
                read_log(&damaged),
                Ok((vec![first.clone()], start)),
                "byte {index}"
            );
        }
        assert_eq!(
            read_log(&log),
            Ok((vec![first, commit_with_stays()], log.len()))
        );
    }

    #[test]
    fn a_commit_with_stays_that_checks_out_but_makes_no_sense_is_an_error() {
        let payload = commit_with_stays().encode()[FRAME_HEADER_LENGTH..].to_vec();
        assert_eq!(
            read_log(&frame(&payload[..payload.len() - 1])),
            Err(FormatError::Truncated)
        );
        let mut longer = payload.clone();
        longer.push(0);
        assert_eq!(
            read_log(&frame(&longer)),
            Err(FormatError::Corrupt("trailing bytes"))
        );
        // The payload up to the count of the notes, which begin 32 bytes in.
        let notes = 32;
        assert_eq!(payload[notes..notes + 4], [0, 0, 0, 2]);
        // No notes at all: such a commit is of the other kind.
        let mut none = payload[..notes].to_vec();
        none.extend_from_slice(&[0; 4]);
        assert_eq!(
            read_log(&frame(&none)),
            Err(FormatError::Corrupt("a commit with stays that has none"))
        );
        // More notes than the payload holds, which must not be reserved either.
        let mut many = payload.clone();
        many[notes..notes + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(read_log(&frame(&many)), Err(FormatError::Truncated));
        // A note of no kind.
        let mut unknown = payload.clone();
        unknown[notes + 4] = 5;
        assert_eq!(
            read_log(&frame(&unknown)),
            Err(FormatError::Corrupt("stay kind"))
        );
        // The second note begins behind the first, which is 25 bytes; its place begins
        // behind its kind, player, entity and hops, with 32 bytes of position and look.
        let flags = notes + 4 + 25 + 29 + 32;
        assert_eq!(payload[flags], 2);
        let mut flagged = payload.clone();
        flagged[flags] = 4;
        assert_eq!(
            read_log(&frame(&flagged)),
            Err(FormatError::Corrupt("place flags"))
        );
        let mut slot = payload.clone();
        assert_eq!(slot[flags + 2], 1);
        slot[flags + 2] = 2;
        assert_eq!(
            read_log(&frame(&slot)),
            Err(FormatError::Corrupt("inventory slot"))
        );
    }

    /// A place whose stacks carry components, in the inventory and the ender chest alike,
    /// is read back with them.
    #[test]
    fn a_place_whose_stacks_carry_a_component_of_every_kind_reads_as_it_was_written() {
        let mut place = place();
        place.inventory[40] = Some(crate::stacks::tests::with_every_component());
        place.ender[26] = Some(crate::stacks::tests::with_every_component());
        let mut out = Vec::new();
        put_place(&mut out, &place);
        let mut input = Input(&out);
        assert_eq!(take_place(&mut input), Ok(place));
        input.finish().unwrap();
    }

    /// Records of format 13, written before stacks had components, of 14, before
    /// places kept effects, of 15, before chunks kept structures, of 16, before chunks
    /// kept claims, of 17, before they kept raids, of 18, before places named their
    /// dimension, of 19, before the records written forward named theirs, and of 20,
    /// before a place carried the player's health, are refused as any record of another
    /// format is.
    #[test]
    fn records_of_formats_13_to_20_are_refused() {
        assert_eq!(FORMAT_VERSION, 21);
        for earlier in [13, 14, 15, 16, 17, 18, 19, 20] {
            let mut payload = commit_with_stays().encode()[FRAME_HEADER_LENGTH..].to_vec();
            assert_eq!(payload[0], FORMAT_VERSION);
            payload[0] = earlier;
            assert_eq!(
                read_log(&frame(&payload)),
                Err(FormatError::UnsupportedVersion(earlier))
            );
        }
    }

    /// A place keeps its effects, the levels of one effect in their order, and the time
    /// since its player last slept.
    #[test]
    fn a_place_keeps_its_effects_and_its_time_since_rest() {
        use clustine_world::components::EffectInstance;
        let effect = |name: &str, amplifier: i32, duration: i32| EffectInstance {
            effect: name.to_owned(),
            amplifier,
            duration,
            ambient: amplifier == 0,
            visible: duration > 0,
            show_icon: true,
        };
        let mut place = place();
        place.effects = vec![
            effect("minecraft:speed", 2, 40),
            effect("minecraft:speed", 0, 600),
            effect("minecraft:night_vision", 0, -1),
        ];
        place.time_since_rest = 72_000;
        let mut bytes = Vec::new();
        put_place(&mut bytes, &place);
        let mut input = Input(&bytes);
        assert_eq!(take_place(&mut input), Ok(place));
        assert!(input.0.is_empty());
    }

    /// Places are the same if their bytes are, so that a record that was read is the
    /// record that was written whatever numbers a region put into it.
    #[test]
    fn places_are_compared_by_their_bytes() {
        let nan = LoggedPlace {
            position: [f64::NAN, 0.0, -0.0],
            yaw: f32::NAN,
            ..place()
        };
        assert_eq!(nan, nan);
        let zero = LoggedPlace {
            position: [f64::NAN, 0.0, 0.0],
            ..nan.clone()
        };
        assert_ne!(nan, zero);
        let record = LogRecord::Commit {
            travellers: Vec::new(),
            region: 1,
            tick: 1,
            epoch: 1,
            changes: Vec::new(),
            state: Vec::new(),
            stays: vec![LoggedStay::Has {
                unit: None,
                player: 1,
                entity: 1,
                hops: 0,
                place: nan,
            }],
            ticks: Vec::new(),
            marks_taken: Vec::new(),
            seams: Vec::new(),
            effects: Vec::new(),
            taken: Vec::new(),
            events: Vec::new(),
            entity_changes: Vec::new(),
            arrivals: Vec::new(),
            arrivals_taken: Vec::new(),
            claims: Vec::new(),
            raids: Vec::new(),
        };
        let bytes = record.encode();
        assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));
    }

    /// Inventories of every kind: proptest makes arrays of up to 32 by itself.
    fn inventories() -> impl Strategy<Value = [Option<ItemStack>; INVENTORY_SLOTS]> {
        proptest::collection::vec(stacks(), INVENTORY_SLOTS)
            .prop_map(|slots| slots.try_into().expect("as many slots as an inventory has"))
    }

    /// Plain stacks or none, and now and then one with a component of every kind.
    fn stacks() -> impl Strategy<Value = Option<ItemStack>> {
        prop_oneof![
            8 => any::<Option<(i32, i32)>>()
                .prop_map(|slot| slot.map(|(item, count)| ItemStack::new(item, count))),
            1 => any::<i32>().prop_map(|count| Some(ItemStack {
                count,
                ..crate::stacks::tests::with_every_component()
            })),
        ]
    }

    fn dimensions() -> impl Strategy<Value = Dimension> {
        (0..3u8).prop_map(|number| Dimension::numbered(number).expect("three dimensions"))
    }

    /// Travels of every way.
    pub(crate) fn travels() -> impl Strategy<Value = Travel> {
        (
            dimensions(),
            any::<(i32, i32, i32)>(),
            0..7u8,
            any::<[f64; 3]>(),
            any::<(i32, i32, i32)>(),
            any::<bool>(),
        )
            .prop_map(|(to, (x, y, z), way, relative, (fx, fy, fz), exact)| {
                use clustine_world::{Double, PortalAxis, Way};
                let way = match way {
                    0..=2 => Way::NetherPortal {
                        axis: [None, Some(PortalAxis::X), Some(PortalAxis::Z)][usize::from(way)],
                        relative: relative.map(Double),
                    },
                    3 => Way::EndPortal,
                    4 => Way::Gateway {
                        from: BlockPos::new(fx, fy, fz),
                        exact,
                    },
                    5 => Way::Credits,
                    _ => Way::Respawn,
                };
                Travel {
                    to,
                    target: BlockPos::new(x, y, z),
                    way,
                }
            })
    }

    /// Travellers of one to four entities.
    fn travellers() -> impl Strategy<Value = Traveller> {
        (
            travels(),
            dimensions(),
            any::<(i32, i32)>(),
            proptest::collection::vec(crate::entities::tests::entities(), 1..4),
        )
            .prop_map(|(travel, dimension, (x, z), entities)| Traveller {
                travel,
                chunk: clustine_world::LevelChunk::new(dimension, ChunkPos::new(x, z)),
                entities,
            })
    }

    fn places() -> impl Strategy<Value = LoggedPlace> {
        (
            any::<[f64; 3]>(),
            any::<(f32, f32)>(),
            any::<(bool, bool, u8)>(),
            inventories(),
            proptest::collection::vec(stacks(), ENDER_SLOTS)
                .prop_map(|slots| <[_; ENDER_SLOTS]>::try_from(slots).expect("27 slots")),
            any::<(i32, f32, i32)>(),
            (
                dimensions(),
                proptest::option::of(travels()),
                any::<(bool, u32)>(),
                proptest::option::of((any::<[i32; 3]>(), any::<(f32, f32)>(), dimensions())),
                any::<(f32, bool)>(),
            ),
        )
            .prop_map(
                |(
                    position,
                    (yaw, pitch),
                    (on_ground, flying, selected_slot),
                    inventory,
                    ender,
                    experience,
                    (dimension, travel, (seen_credits, portal_cooldown), respawn, (health, dead)),
                )| {
                    LoggedPlace {
                        dimension,
                        travel,
                        seen_credits,
                        portal_cooldown,
                        health,
                        dead,
                        respawn: respawn
                            .map(|(pos, (yaw, pitch), dimension)| (pos, yaw, pitch, dimension)),
                        position,
                        yaw,
                        pitch,
                        on_ground,
                        flying,
                        selected_slot,
                        inventory,
                        ender,
                        experience,
                        effects: Vec::new(),
                        time_since_rest: 0,
                        raid_omen_position: None,
                    }
                },
            )
    }

    fn stays() -> impl Strategy<Value = Vec<LoggedStay>> {
        let stay = prop_oneof![
            (any::<u128>(), any::<u64>())
                .prop_map(|(player, entity)| LoggedStay::Entering { player, entity }),
            (any::<u128>(), any::<u64>(), any::<u32>(), places()).prop_map(
                |(player, entity, hops, place)| LoggedStay::Has {
                    unit: None,
                    player,
                    entity,
                    hops,
                    place,
                }
            ),
        ];
        prop::collection::vec(stay, 0..20)
    }

    fn scheduled_ticks() -> impl Strategy<Value = ScheduledTick> {
        let of = prop_oneof![
            any::<u16>().prop_map(Ticked::Block),
            any::<u8>().prop_map(Ticked::Fluid),
        ];
        (
            0..16_u8,
            any::<i16>(),
            0..16_u8,
            of,
            any::<i32>(),
            any::<i8>(),
        )
            .prop_map(|(x, y, z, of, delay, priority)| ScheduledTick {
                at: InChunk { y, z, x },
                of,
                delay,
                priority,
            })
    }

    fn chunks() -> impl Strategy<Value = Vec<ChunkPos>> {
        prop::collection::vec(
            (any::<i32>(), any::<i32>()).prop_map(|(x, z)| ChunkPos::new(x, z)),
            0..50,
        )
    }

    proptest! {
        #[test]
        fn travellers_round_trip_in_a_commit_and_written_forward(
            region: u32,
            tick: u64,
            travellers in proptest::collection::vec(travellers(), 1..3),
            ids in proptest::collection::vec(any::<(u64, u32)>(), 1..3),
        ) {
            let forward: Vec<(EffectId, Traveller)> = ids
                .iter()
                .zip(&travellers)
                .map(|((sequence, index), traveller)| (EffectId { sequence: *sequence, index: *index }, traveller.clone()))
                .collect();
            let records = vec![
                LogRecord::Commit {
                    region,
                    tick,
                    epoch: 1,
                    changes: Vec::new(),
                    state: Vec::new(),
                    stays: Vec::new(),
                    ticks: Vec::new(),
                    marks_taken: Vec::new(),
                    seams: Vec::new(),
                    effects: Vec::new(),
                    taken: Vec::new(),
                    events: Vec::new(),
                    claims: Vec::new(),
                    raids: Vec::new(),
                    entity_changes: Vec::new(),
                    arrivals: Vec::new(),
                    arrivals_taken: Vec::new(),
                    travellers,
                },
                LogRecord::TravellersForward { travellers: forward },
            ];
            let bytes: Vec<u8> = records.iter().flat_map(LogRecord::encode).collect();
            prop_assert_eq!(bytes[FRAME_HEADER_LENGTH + 1], KIND_COMMIT_WITH_TRAVELLERS);
            prop_assert_eq!(read_log(&bytes), Ok((records, bytes.len())));
        }

        #[test]
        fn records_of_regions_round_trip(
            region: u32,
            other: u32,
            tick: u64,
            epoch: u64,
            other_epoch: u64,
            granted in chunks(),
            returned in chunks(),
            state in prop::collection::vec(any::<u8>(), 0..200),
            other_state in prop::collection::vec(any::<u8>(), 0..200),
            chance: i32,
        ) {
            let records = vec![
                LogRecord::TraderChance { firing: tick, chance },
                LogRecord::Granted { region, tick, chunks: granted },
                LogRecord::Returned { region, chunks: returned.clone() },
                LogRecord::Absorbed {
                    region,
                    epoch,
                    absorbed: other,
                    tick,
                    state: state.clone(),
                },
                LogRecord::Split {
                    region,
                    epoch,
                    tick,
                    state,
                    part: other,
                    part_epoch: other_epoch,
                    chunks: returned,
                    part_state: other_state,
                },
            ];
            let bytes: Vec<u8> = records.iter().flat_map(LogRecord::encode).collect();
            prop_assert_eq!(read_log(&bytes), Ok((records, bytes.len())));
        }

        #[test]
        fn records_round_trip(
            region: u32,
            tick: u64,
            epoch: u64,
            changes in prop::collection::vec((any::<i32>(), any::<i32>(), any::<i32>(), any::<u16>()), 0..50),
            state in prop::collection::vec(any::<u8>(), 0..200),
            restored: u64,
            stays in stays(),
        ) {
            let commit = LogRecord::Commit { travellers: Vec::new(),
                region,
                tick,
                epoch,
                changes: changes
                    .into_iter()
                    .map(|(x, y, z, state)| (BlockPos::new(x, y, z), BlockState(state)).into())
                    .collect(),
                state,
                stays,
                ticks: Vec::new(),
                marks_taken: Vec::new(),
                seams: Vec::new(),
                effects: Vec::new(),
                taken: Vec::new(), events: Vec::new(),
                entity_changes: Vec::new(),
                arrivals: Vec::new(),
                arrivals_taken: Vec::new(),
                claims: Vec::new(),
                raids: Vec::new(),
            };
            let opened = LogRecord::Opened { region, epoch, restored };
            let mut bytes = commit.encode();
            bytes.extend(opened.encode());
            prop_assert_eq!(read_log(&bytes), Ok((vec![commit, opened], bytes.len())));
        }

        /// Commits with the ticks of any chunks, of any number, are read back as they
        /// were written.
        #[test]
        fn commits_with_ticks_round_trip(
            tick: u64,
            ticks in prop::collection::vec(
                (
                    (any::<i32>(), any::<i32>()).prop_map(|(x, z)| ChunkPos::new(x, z)),
                    prop::collection::vec(scheduled_ticks(), 0..20),
                ),
                1..10,
            ),
            stays in stays(),
        ) {
            let commit = LogRecord::Commit { travellers: Vec::new(),
                region: 1,
                tick,
                epoch: 3,
                changes: Vec::new(),
                state: Vec::new(),
                stays,
                ticks,
                marks_taken: Vec::new(),
                seams: Vec::new(),
                effects: Vec::new(),
                taken: Vec::new(), events: Vec::new(),
                entity_changes: Vec::new(),
                arrivals: Vec::new(),
                arrivals_taken: Vec::new(),
                claims: Vec::new(),
                raids: Vec::new(),
            };
            let bytes = commit.encode();
            prop_assert_eq!(bytes[FRAME_HEADER_LENGTH + 1], 9);
            prop_assert_eq!(read_log(&bytes), Ok((vec![commit], bytes.len())));
        }

        /// Commits with entity changes of any kind, among block changes, block entities
        /// and block events or without them, are read back as they were written.
        #[test]
        fn commits_with_entity_changes_round_trip(
            tick: u64,
            entity_changes in crate::entities::tests::entity_changes(),
            blocks in prop::collection::vec((any::<i32>(), any::<u16>()), 0..5),
            with_block_entity: bool,
            stays in stays(),
        ) {
            let mut changes: Vec<Change> = blocks
                .into_iter()
                .map(|(x, state)| (BlockPos::new(x, -60, 0), BlockState(state)).into())
                .collect();
            if with_block_entity {
                changes.push(Change::BlockEntity(BlockPos::new(0, -60, 0), b"data".to_vec()));
            }
            let commit = LogRecord::Commit { travellers: Vec::new(),
                region: 1,
                tick,
                epoch: 3,
                changes,
                state: vec![1],
                stays,
                ticks: Vec::new(),
                marks_taken: Vec::new(),
                seams: Vec::new(),
                effects: Vec::new(),
                taken: Vec::new(),
                events: Vec::new(),
                claims: Vec::new(),
                raids: Vec::new(),
                entity_changes,
                arrivals: Vec::new(),
                arrivals_taken: Vec::new(),
            };
            let bytes = commit.encode();
            prop_assert_eq!(bytes[FRAME_HEADER_LENGTH + 1], KIND_COMMIT_WITH_ENTITY_CHANGES);
            prop_assert_eq!(read_log(&bytes), Ok((vec![commit], bytes.len())));
        }

        /// Commits with arrivals sent or taken, with entity changes or none, and the store's
        /// records of arrivals, are read back as they were written.
        #[test]
        fn commits_with_arrivals_and_the_records_of_arrivals_round_trip(
            tick: u64,
            entity_changes in prop::option::of(crate::entities::tests::entity_changes()),
            sent in prop::collection::vec(crate::entities::tests::entities(), 0..3),
            taken in prop::collection::vec((any::<u64>(), any::<u32>()), 0..3),
            due in prop::option::of(any::<u64>()),
        ) {
            let due = due.map_or(Due::FromTaking, |tick| Due::At { tick });
            let arrivals: Vec<(ChunkPos, Entity, Due)> =
                sent.iter().map(|entity| (entity.chunk(), entity.clone(), due)).collect();
            let mut arrivals_taken: Vec<EffectId> = taken
                .iter()
                .map(|(sequence, index)| EffectId {
                    sequence: *sequence,
                    index: *index,
                })
                .collect();
            // Ascending, as a commit has them.
            arrivals_taken.sort();
            arrivals_taken.dedup();
            let commit = LogRecord::Commit { travellers: Vec::new(),
                region: 1,
                tick,
                epoch: 3,
                changes: Vec::new(),
                state: vec![1],
                stays: Vec::new(),
                ticks: Vec::new(),
                marks_taken: Vec::new(),
                seams: Vec::new(),
                effects: Vec::new(),
                taken: Vec::new(),
                events: Vec::new(),
                claims: Vec::new(),
                raids: Vec::new(),
                entity_changes: entity_changes.unwrap_or_default(),
                arrivals: arrivals.clone(),
                arrivals_taken: arrivals_taken.clone(),
            };
            let bytes = commit.encode();
            let with_arrivals = !arrivals.is_empty() || !arrivals_taken.is_empty();
            if with_arrivals {
                prop_assert_eq!(bytes[FRAME_HEADER_LENGTH + 1], KIND_COMMIT_WITH_ARRIVALS);
            }
            prop_assert_eq!(read_log(&bytes), Ok((vec![commit], bytes.len())));
            let forward = LogRecord::ArrivalsForward {
                dimension: Dimension::Nether,
                arrivals: arrivals
                    .iter()
                    .zip(&arrivals_taken)
                    .map(|((chunk, entity, due), id)| Arrival {
                        id: *id,
                        chunk: *chunk,
                        entity: entity.clone(),
                        due: *due,
                    })
                    .collect(),
            };
            let dropped = LogRecord::ArrivalsDropped { ids: arrivals_taken };
            for record in [forward, dropped] {
                let bytes = record.encode();
                prop_assert_eq!(read_log(&bytes), Ok((vec![record], bytes.len())));
            }
        }

        #[test]
        fn arbitrary_bytes_never_panic(bytes: Vec<u8>) {
            let _ = read_log(&bytes);
        }
    }
}
