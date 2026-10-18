//! What the edge sends of an entity other than a player as it moves: the game's
//! `ServerEntity`, played from the region's reports (`docs/groundwork/n4-brief.md`,
//! section 9; `docs/groundwork/n4-plan.md`, section 7).
//!
//! The region reports an entity only in a tick that changed what is shown of it or set
//! one of its flags for the edge ([`Tracked`]); the game decides each tick, from its own
//! counters, whether to send the entity's motion, a relative move, its whole position or
//! its changed data. A [`Tracker`] keeps those counters and the last the clients were
//! sent (the base of the position's deltas, the motion, the angles, being on the
//! ground) for one entity, the same for everyone shown it, and is run once for every
//! tick of the region, the ticks nothing was reported in among them.
//!
//! Its count of ticks starts at the entity's first report to this edge, as the game's
//! starts where the entity is first tracked: on another edge, or after a restart of
//! this one, the phase of the 20 ticks between updates may differ. That only moves when
//! a client is corrected, never what it is corrected to.

use bytes::Bytes;
use clustine_data::{ENTITY_TYPE_INFO, entity_types};
use clustine_protocol::codec::Position;
use clustine_protocol::delta::{VecDeltaCodec, precision_loss};
use clustine_protocol::item::ItemStack as WireStack;
use clustine_protocol::packets::{
    self, Packet,
    play::{
        EntityDataValue, MoveEntityPos, MoveEntityPosRot, MoveEntityRot, PositionPath,
        SetEntityData, SetEntityMotion, SyncEntityPosition, VecDelta, entity_data, pack_degrees,
    },
};
use clustine_sim::api::Tracked;
use clustine_world::EntityData;

/// How far an entity has to be from where it was sent last for a move to be sent, the
/// square of a distance (`ServerEntity.TOLERANCE_LEVEL_POSITION`).
const POSITION_TOLERANCE: f64 = 7.629_394_531_25e-6;

/// Every so many ticks the position is sent though it has not changed
/// (`FORCED_POS_UPDATE_PERIOD`).
const FORCED_POSITION_PERIOD: i32 = 60;

/// After this many updates the position is sent whole (`FORCED_TELEPORT_PERIOD`).
const FORCED_TELEPORT_PERIOD: i32 = 400;

/// How much a motion has to change, squared, to be sent again.
const MOTION_TOLERANCE: f64 = 1e-7;

/// The flags of a report that hold until the next report: what the entity's last move
/// did and whether it burns.
const LASTING: u8 = Tracked::HORIZONTAL_COLLISION
    | Tracked::VERTICAL_COLLISION
    | Tracked::PRECISE
    | Tracked::ON_FIRE;

/// The flags of a report that the next update takes and clears.
const ONCE: u8 = Tracked::NEEDS_SYNC | Tracked::SYNC_POSITION | Tracked::SYNC_VELOCITY;

/// Values of [`Tracker::dirty`]: which of the entity's data changed since it was sent.
const DIRTY_FLAGS: u8 = 1;
const DIRTY_STACK: u8 = 2;

/// One of the packets a tracker sends, to everyone shown the entity.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Sent {
    Motion(SetEntityMotion),
    Pos(MoveEntityPos),
    PosRot(MoveEntityPosRot),
    Rot(MoveEntityRot),
    Sync(SyncEntityPosition),
    Data(SetEntityData),
}

impl Sent {
    pub(crate) fn encoded(&self) -> Bytes {
        fn bytes<P: Packet>(packet: &P) -> Bytes {
            packets::encode(packet).into()
        }
        match self {
            Self::Motion(packet) => bytes(packet),
            Self::Pos(packet) => bytes(packet),
            Self::PosRot(packet) => bytes(packet),
            Self::Rot(packet) => bytes(packet),
            Self::Sync(packet) => bytes(packet),
            Self::Data(packet) => bytes(packet),
        }
    }

    /// Whether a client that has had every packet before this one is left as it was by
    /// it: a move to where it has the entity, on the ground or not as it has it, the
    /// angles as it has them.
    fn changes_nothing(&self, held: &Held) -> bool {
        match self {
            Self::Pos(moved) => {
                moved.delta == VecDelta::Linear([0; 3]) && moved.on_ground == held.on_ground
            }
            Self::Sync(moved) => {
                let [x, y, z] = held.position;
                moved.path.end() == Some((x, y, z))
                    && moved.on_ground == held.on_ground
                    && (pack_degrees(moved.yaw), pack_degrees(moved.pitch)) == held.angles
            }
            _ => false,
        }
    }
}

/// What the clients shown an entity hold of it after the packets sent so far.
#[derive(Debug, Clone, PartialEq)]
struct Held {
    position: [f64; 3],
    on_ground: bool,
    angles: (i8, i8),
}

/// What a newly shown client is told of where the entity is and how it moves: what the
/// tracker sent last (`ClientboundAddEntityPacket` from a `ServerEntity`), so that the
/// moves that follow are relative to what it has.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Pairing {
    pub(crate) position: [f64; 3],
    pub(crate) motion: [f64; 3],
    /// In 256ths of a turn.
    pub(crate) yaw: u8,
    pub(crate) pitch: u8,
    /// The values of its data that are not the type's defaults, as sent last
    /// (`getNonDefaultValues`); none, and no packet, if there are none.
    pub(crate) values: Vec<(u8, EntityDataValue)>,
}

/// The game's `ServerEntity` of one entity other than a player, and the entity as it
/// was reported last.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Tracker {
    /// The type's ticks between updates (`updateInterval`), whether its motion is sent
    /// (`trackDelta`), and whether it is an item, whose positions after a collision
    /// are sent whole where a delta would round them (`isFullPrecisionEncodingRequired`).
    interval: i32,
    track_delta: bool,
    item: bool,
    tick_count: i32,
    teleport_delay: i32,
    codec: VecDeltaCodec,
    last_motion: [f64; 3],
    last_yaw: i8,
    last_pitch: i8,
    was_on_ground: bool,
    /// Whether every client shown the entity holds it on the ground: the last move sent
    /// said so, and no client has been shown it since. A spawn tells a client nothing of
    /// the ground, so it holds the entity off it until a move says otherwise; a pairing
    /// sets this false, so that a move that tells it is not passed over as one that
    /// changes nothing when the edge catches up quiet ticks.
    sent_on_ground: std::cell::Cell<bool>,
    /// The values of its data that differ from the type's defaults, as sent last.
    pairing: Vec<(u8, EntityDataValue)>,
    /// Where a falling block started (`DATA_START_POS`): the block it was in when this
    /// edge first heard of it.
    start: Option<Position>,

    position: [f64; 3],
    motion: [f64; 3],
    yaw: f32,
    pitch: f32,
    on_ground: bool,
    /// The [`LASTING`] flags of the last report, and the [`ONCE`] flags not yet taken.
    lasting: u8,
    once: u8,
    dirty: u8,
    /// The region's tick whose update ran last.
    pub(crate) ran: u64,
}

/// The entity as a report that is not of its tick shows it: its spawn, a snapshot of
/// its chunk, or the word of the region that took it in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Seen {
    pub(crate) kind: u16,
    pub(crate) position: [f64; 3],
    pub(crate) motion: [f64; 3],
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    pub(crate) on_ground: bool,
}

impl Tracker {
    /// Starts tracking an entity as it is now, with `ran` the region's tick whose update
    /// it is past: as the game's `ServerEntity` begins, with the base of its deltas, its
    /// motion and angles taken as sent.
    pub(crate) fn new(seen: Seen, data: &EntityData, ran: u64) -> Self {
        let info = ENTITY_TYPE_INFO.get(usize::from(seen.kind));
        let (interval, track_delta) =
            info.map_or((20, true), |info| (info.update_interval, info.track_deltas));
        let start = matches!(data, EntityData::FallingBlock { .. }).then(|| Position {
            x: seen.position[0].floor() as i32,
            y: seen.position[1].floor() as i32,
            z: seen.position[2].floor() as i32,
        });
        let mut tracker = Self {
            interval: interval.max(1),
            track_delta,
            item: i32::from(seen.kind) == entity_types::ITEM,
            tick_count: 0,
            teleport_delay: 0,
            codec: VecDeltaCodec::new(seen.position),
            last_motion: seen.motion,
            last_yaw: pack_degrees(seen.yaw),
            last_pitch: pack_degrees(seen.pitch),
            was_on_ground: seen.on_ground,
            // The clients it is first shown to are spawned with it, off the ground.
            sent_on_ground: std::cell::Cell::new(false),
            pairing: Vec::new(),
            start,
            position: seen.position,
            motion: seen.motion,
            yaw: seen.yaw,
            pitch: seen.pitch,
            on_ground: seen.on_ground,
            lasting: 0,
            once: 0,
            dirty: 0,
            ran,
        };
        tracker.pairing = tracker.values(data, true);
        tracker
    }

    /// Takes a report of the entity's tick. `data` is its data before the report, to
    /// see what changed.
    pub(crate) fn report(&mut self, tracked: &Tracked, data: &EntityData) {
        self.position = tracked.position;
        self.motion = tracked.motion;
        self.yaw = tracked.yaw;
        self.pitch = tracked.pitch;
        self.on_ground = tracked.on_ground;
        let burning = tracked.flags & Tracked::ON_FIRE != 0;
        if burning != (self.lasting & Tracked::ON_FIRE != 0) {
            self.dirty |= DIRTY_FLAGS;
        }
        self.lasting = tracked.flags & LASTING;
        self.once |= tracked.flags & ONCE;
        if let (Some(EntityData::Item { stack: new, .. }), EntityData::Item { stack: old, .. }) =
            (&tracked.data, data)
            && new != old
        {
            self.dirty |= DIRTY_STACK;
        }
    }

    /// Takes where the entity is from a report that is not of its tick (it was shown
    /// again, by a snapshot or by the region that holds it now), flags left as they are.
    /// Where that is not where the edge had it, which a tick's report would have told,
    /// the clients are corrected at the next tick, as `needsSync` has it.
    pub(crate) fn moved(&mut self, seen: Seen) {
        if seen.position != self.position || seen.on_ground != self.on_ground {
            self.once |= Tracked::NEEDS_SYNC;
        }
        self.position = seen.position;
        self.motion = seen.motion;
        self.yaw = seen.yaw;
        self.pitch = seen.pitch;
        self.on_ground = seen.on_ground;
    }

    /// With `pairing`, the values of the entity's data that are not the type's defaults,
    /// as a newly shown client is sent them; without, the values the dirty marks name,
    /// defaults or not, as a change sends them.
    fn values(&self, data: &EntityData, pairing: bool) -> Vec<(u8, EntityDataValue)> {
        let mut values = Vec::new();
        let burning = self.lasting & Tracked::ON_FIRE != 0;
        if (pairing && burning) || (!pairing && self.dirty & DIRTY_FLAGS != 0) {
            let flags = if burning { entity_data::ON_FIRE } else { 0 };
            values.push((entity_data::SHARED_FLAGS, EntityDataValue::Byte(flags)));
        }
        match data {
            EntityData::Item { stack, .. } => {
                if pairing || self.dirty & DIRTY_STACK != 0 {
                    let stack = WireStack {
                        item: stack.item,
                        count: stack.count,
                    };
                    values.push((
                        entity_data::ITEM_STACK,
                        EntityDataValue::ItemStack(Some(stack)),
                    ));
                }
            }
            EntityData::FallingBlock { .. } => {
                // The start at the block of the world's origin is the default, not sent.
                if pairing
                    && let Some(start) = self.start
                    && (start.x, start.y, start.z) != (0, 0, 0)
                {
                    values.push((
                        entity_data::START_POSITION,
                        EntityDataValue::BlockPos(start),
                    ));
                }
            }
        }
        values
    }

    /// What a client that is newly shown the entity is sent (`sendPairingData`).
    pub(crate) fn pairing(&self) -> Pairing {
        self.sent_on_ground.set(false);
        Pairing {
            position: self.codec.base,
            motion: self.last_motion,
            yaw: self.last_yaw as u8,
            pitch: self.last_pitch as u8,
            values: self.pairing.clone(),
        }
    }

    /// The changed data, if any, as `sendDirtyEntityData` sends it, the values sent
    /// becoming what a newly shown client is sent.
    fn send_dirty(&mut self, id: i32, data: &EntityData, out: &mut Vec<Sent>) {
        if self.dirty == 0 {
            return;
        }
        let values = self.values(data, false);
        self.dirty = 0;
        self.pairing = self.values(data, true);
        out.push(Sent::Data(SetEntityData {
            entity_id: id,
            values,
        }));
    }

    /// One tick's update, the game's `ServerEntity.sendChanges` for an entity that is no
    /// passenger, minecart or arrow, with `id` its wire id and `data` its data.
    pub(crate) fn send_changes(&mut self, id: i32, data: &EntityData) -> Vec<Sent> {
        let mut out = Vec::new();
        if self.once & Tracked::SYNC_POSITION != 0 {
            // `UpdateInterval.nextInterval`: this tick becomes one of the updates.
            self.tick_count = (self.tick_count / self.interval)
                .wrapping_mul(self.interval)
                .wrapping_add(self.interval);
            self.once &= !Tracked::SYNC_POSITION;
        }
        let needs_sync = self.once & Tracked::NEEDS_SYNC != 0;
        if needs_sync || self.tick_count % self.interval == 0 || self.dirty != 0 {
            let yaw = pack_degrees(self.yaw);
            let pitch = pack_degrees(self.pitch);
            let rotated = yaw != self.last_yaw || pitch != self.last_pitch;
            self.teleport_delay = self.teleport_delay.wrapping_add(1);
            let away = length_squared(self.codec.delta(self.position)) >= POSITION_TOLERANCE;
            let positioned = away || self.tick_count % FORCED_POSITION_PERIOD == 0;
            let packet = self.move_packet(id, yaw, pitch, positioned, rotated);
            if needs_sync || self.track_delta {
                let change = distance_squared(self.motion, self.last_motion);
                if change > MOTION_TOLERANCE || (change > 0.0 && length_squared(self.motion) == 0.0)
                {
                    self.last_motion = self.motion;
                    out.push(Sent::Motion(SetEntityMotion {
                        entity_id: id,
                        motion: self.last_motion,
                    }));
                }
            }
            if let Some(packet) = packet {
                let (has_position, has_rotation) = match &packet {
                    Sent::Pos(_) => (true, false),
                    Sent::PosRot(_) | Sent::Sync(_) => (true, true),
                    Sent::Rot(_) => (false, true),
                    Sent::Motion(_) | Sent::Data(_) => (false, false),
                };
                if let Sent::Pos(MoveEntityPos { on_ground, .. })
                | Sent::PosRot(MoveEntityPosRot { on_ground, .. })
                | Sent::Rot(MoveEntityRot { on_ground, .. })
                | Sent::Sync(SyncEntityPosition { on_ground, .. }) = &packet
                {
                    self.sent_on_ground.set(*on_ground);
                }
                out.push(packet);
                self.send_dirty(id, data, &mut out);
                if has_position {
                    self.codec.base = self.position;
                }
                if has_rotation {
                    self.last_yaw = yaw;
                    self.last_pitch = pitch;
                }
            } else {
                self.send_dirty(id, data, &mut out);
            }
            // A head of its own is only a living entity's: these keep theirs at 0.
            self.once &= !Tracked::NEEDS_SYNC;
        }
        self.tick_count = self.tick_count.wrapping_add(1);
        if self.once & Tracked::SYNC_VELOCITY != 0 {
            self.once &= !Tracked::SYNC_VELOCITY;
            out.push(Sent::Motion(SetEntityMotion {
                entity_id: id,
                motion: self.motion,
            }));
        }
        out
    }

    /// The updates of `ticks` ticks in which nothing was reported, which the edge learns
    /// of only when the region next says something: the counters run as in every
    /// tick, but a move that leaves a client where it was, which is all a still entity
    /// is sent at its intervals, is not sent late. The ticks between updates are
    /// counted, not run.
    pub(crate) fn catch_up(&mut self, id: i32, data: &EntityData, mut ticks: u64) -> Vec<Sent> {
        let mut out = Vec::new();
        while ticks > 0 {
            let due = self.once != 0 || self.dirty != 0 || self.tick_count % self.interval == 0;
            if due {
                let held = Held {
                    position: self.codec.base,
                    on_ground: self.sent_on_ground.get(),
                    angles: (self.last_yaw, self.last_pitch),
                };
                for sent in self.send_changes(id, data) {
                    if !sent.changes_nothing(&held) {
                        out.push(sent);
                    }
                }
                ticks -= 1;
                continue;
            }
            let to_next = self.interval - self.tick_count.rem_euclid(self.interval);
            let skipped = u64::try_from(to_next).unwrap_or(1).min(ticks);
            self.tick_count = self
                .tick_count
                .wrapping_add(i32::try_from(skipped).expect("fewer than an interval"));
            ticks -= skipped;
        }
        out
    }

    /// `createMovePacket`.
    fn move_packet(
        &mut self,
        id: i32,
        yaw: i8,
        pitch: i8,
        positioned: bool,
        rotated: bool,
    ) -> Option<Sent> {
        let precise = self.lasting & Tracked::PRECISE != 0;
        if precise
            || self.teleport_delay > FORCED_TELEPORT_PERIOD
            || self.was_on_ground != self.on_ground
        {
            self.was_on_ground = self.on_ground;
            self.teleport_delay = 0;
            return Some(self.sync(id));
        }
        if positioned {
            let Some(delta) = self.codec.try_encode(self.position) else {
                return Some(self.sync(id));
            };
            let delta = VecDelta::Linear(delta);
            if self.full_precision_required(&delta) {
                return Some(self.sync(id));
            }
            let on_ground = self.on_ground;
            return Some(if rotated {
                Sent::PosRot(MoveEntityPosRot {
                    entity_id: id,
                    delta,
                    yaw,
                    pitch,
                    on_ground,
                })
            } else {
                Sent::Pos(MoveEntityPos {
                    entity_id: id,
                    delta,
                    on_ground,
                })
            });
        }
        rotated.then_some(Sent::Rot(MoveEntityRot {
            entity_id: id,
            yaw,
            pitch,
            on_ground: self.on_ground,
        }))
    }

    /// The whole position (`ClientboundEntityPositionSyncPacket.of`), with the angles as
    /// the entity has them.
    fn sync(&self, id: i32) -> Sent {
        let [x, y, z] = self.position;
        Sent::Sync(SyncEntityPosition {
            entity_id: id,
            path: PositionPath::Linear { x, y, z },
            yaw: self.yaw,
            pitch: self.pitch,
            on_ground: self.on_ground,
        })
    }

    /// `isFullPrecisionEncodingRequired`, for items only: after a collision, a move along
    /// x or z is sent whole where a delta would leave the item off where it is, so that
    /// a client does not see it sink into what it lies against. The horizontal case
    /// crosses the axes, as the game's bytecode does.
    fn full_precision_required(&self, delta: &VecDelta) -> bool {
        if !self.item {
            return false;
        }
        let [x, _, z] = self.position;
        let off = |value: f64| precision_loss(value) != 0.0;
        if self.lasting & Tracked::VERTICAL_COLLISION != 0
            && ((delta.has_x() && off(x)) || (delta.has_z() && off(z)))
        {
            return true;
        }
        self.lasting & Tracked::HORIZONTAL_COLLISION != 0
            && ((delta.has_x() && off(z)) || (delta.has_z() && off(x)))
    }
}

fn length_squared(v: [f64; 3]) -> f64 {
    v[0] * v[0] + v[1] * v[1] + v[2] * v[2]
}

/// `Vec3.distanceToSqr`, in the game's order.
fn distance_squared(a: [f64; 3], b: [f64; 3]) -> f64 {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let dz = b[2] - a[2];
    dx * dx + dy * dy + dz * dz
}

#[cfg(test)]
mod tests {
    //! Each sequence of reports is worked through `sendChanges` by hand in its comments:
    //! the brief's section 9, from `ServerEntity` in the 26.3 jar.

    use clustine_world::{ChunkPos, EntityId, ItemStack};

    use super::*;

    const ID: i32 = 1;

    fn stack(count: i32) -> EntityData {
        EntityData::Item {
            stack: ItemStack { item: 1, count },
            pick_up_delay: 40,
            thrower: None,
            health: 5,
        }
    }

    fn falling() -> EntityData {
        EntityData::FallingBlock {
            state: 118,
            time: 0,
            drop_item: true,
            cancel_drop: false,
        }
    }

    fn item_at(position: [f64; 3], on_ground: bool) -> Tracker {
        let seen = Seen {
            kind: entity_types::ITEM as u16,
            position,
            motion: [0.0; 3],
            yaw: 200.0,
            pitch: 0.0,
            on_ground,
        };
        Tracker::new(seen, &stack(1), 0)
    }

    /// A report of the item at `position` with `motion`, `flags` and nothing else.
    fn tick(position: [f64; 3], motion: [f64; 3], on_ground: bool, flags: u8) -> Tracked {
        Tracked {
            entity: EntityId(1),
            chunk: ChunkPos::new(0, 0),
            hops: 0,
            position,
            motion,
            yaw: 200.0,
            pitch: 0.0,
            on_ground,
            flags,
            data: None,
        }
    }

    fn pos(delta: [i16; 3], on_ground: bool) -> Sent {
        Sent::Pos(MoveEntityPos {
            entity_id: ID,
            delta: VecDelta::Linear(delta),
            on_ground,
        })
    }

    fn sync(position: [f64; 3], on_ground: bool) -> Sent {
        let [x, y, z] = position;
        Sent::Sync(SyncEntityPosition {
            entity_id: ID,
            path: PositionPath::Linear { x, y, z },
            yaw: 200.0,
            pitch: 0.0,
            on_ground,
        })
    }

    fn motion(motion: [f64; 3]) -> Sent {
        Sent::Motion(SetEntityMotion {
            entity_id: ID,
            motion,
        })
    }

    /// Runs `ticks` updates of `tracker` with nothing reported, each tick's packets in
    /// a list of their own.
    fn run(tracker: &mut Tracker, data: &EntityData, ticks: usize) -> Vec<Vec<Sent>> {
        (0..ticks).map(|_| tracker.send_changes(ID, data)).collect()
    }

    /// The ticks of `sent` in which something was sent, with what.
    fn sent_in(sent: &[Vec<Sent>]) -> Vec<(usize, Vec<Sent>)> {
        let ticks = sent.iter().enumerate();
        ticks
            .filter(|(_, sent)| !sent.is_empty())
            .map(|(tick, sent)| (tick, sent.clone()))
            .collect()
    }

    /// An item lying still: its count of ticks is 0 at the first update, an update every
    /// 20 (`tickCount % 20 == 0`), with nothing to send but on every 60th, where the
    /// position is sent anyway (`tickCount % 60 == 0`), as a delta of nothing.
    #[test]
    fn a_still_item_is_sent_a_move_of_nothing_every_60_ticks_and_nothing_between() {
        let mut tracker = item_at([0.5, 64.0, 0.5], true);
        let sent = run(&mut tracker, &stack(1), 181);
        let still = pos([0; 3], true);
        assert_eq!(
            sent_in(&sent),
            [
                (0, vec![still.clone()]),
                (60, vec![still.clone()]),
                (120, vec![still.clone()]),
                (180, vec![still])
            ]
        );
    }

    /// `teleportDelay` counts updates, not ticks: it passes 400 at the 401st update,
    /// tick 8000 for an item updated every 20 ticks, which is then sent whole and
    /// counted from 0 again.
    #[test]
    fn the_401st_update_sends_the_position_whole() {
        let mut tracker = item_at([0.5, 64.0, 0.5], true);
        let sent = run(&mut tracker, &stack(1), 8001 + 8020);
        let whole: Vec<usize> = sent_in(&sent)
            .into_iter()
            .filter(|(_, sent)| matches!(sent[..], [Sent::Sync(_)]))
            .map(|(tick, _)| tick)
            .collect();
        // Updates 401 and 802: ticks 8000 and 16020.
        assert_eq!(whole, [8000, 16020]);
        assert_eq!(sent[8000], [sync([0.5, 64.0, 0.5], true)]);
    }

    /// An item falling from y 70 with no flags, reported each tick: the clients run its
    /// physics themselves, and are corrected at the updates only, the motion before the
    /// move (`SetEntityMotion` when it changed by more than 1e-7 squared).
    #[test]
    fn a_falling_item_is_corrected_at_the_updates_with_its_motion_first() {
        let mut tracker = item_at([0.5, 70.0, 0.5], false);
        let data = stack(1);
        // Tick 0: the update of the spawn, before the item's first tick.
        assert_eq!(tracker.send_changes(ID, &data), [pos([0; 3], false)]);
        let mut y = 70.0;
        let mut dy = 0.0;
        for tick in 1..=20 {
            dy = (dy - 0.04) * 0.98;
            y += dy;
            tracker.report(&tick_at(y, dy), &data);
            let sent = tracker.send_changes(ID, &data);
            if tick < 20 {
                assert_eq!(sent, [], "tick {tick}");
            } else {
                // From 70 (286720 steps) to y, rounded on its own.
                let steps = clustine_protocol::delta::encode(y) - 286_720;
                let delta = [0, i16::try_from(steps).unwrap(), 0];
                assert_eq!(sent, [motion([0.0, dy, 0.0]), pos(delta, false)]);
            }
        }
        // The base is where it was sent last; a client shown it now is sent that.
        let pairing = tracker.pairing();
        assert_eq!(pairing.position, [0.5, y, 0.5]);
        assert_eq!(pairing.motion, [0.0, dy, 0.0]);
        assert_eq!((pairing.yaw, pairing.pitch), (142, 0));
    }

    fn tick_at(y: f64, dy: f64) -> Tracked {
        tick([0.5, y, 0.5], [0.0, dy, 0.0], false, 0)
    }

    /// Landing changes `onGround`, which the next update sends whole
    /// (`wasOnGround != onGround`), not at once: the 20 ticks hold.
    #[test]
    fn landing_is_sent_whole_at_the_next_update() {
        let mut tracker = item_at([0.5, 65.0, 0.5], false);
        let data = stack(1);
        tracker.send_changes(ID, &data);
        let landed = [0.5, 64.0, 0.5];
        tracker.report(
            &tick(landed, [0.0; 3], true, Tracked::VERTICAL_COLLISION),
            &data,
        );
        let sent = run(&mut tracker, &data, 20);
        assert_eq!(sent_in(&sent), [(19, vec![sync(landed, true)])]);
        // And from then on a move of nothing every 60, the first at tick 60.
        let sent = run(&mut tracker, &data, 40);
        assert_eq!(sent_in(&sent), [(39, vec![pos([0; 3], true)])]);
    }

    /// `needsSync` (an item touching water, or whose motion changed by more than 0.1)
    /// makes any tick an update, once.
    #[test]
    fn needs_sync_makes_the_tick_an_update_once() {
        let mut tracker = item_at([0.5, 64.0, 0.5], false);
        let data = stack(1);
        tracker.send_changes(ID, &data);
        run(&mut tracker, &data, 4);
        let moved = [0.5, 63.9, 0.5];
        let flowing = [0.01, -0.1, 0.0];
        tracker.report(&tick(moved, flowing, false, Tracked::NEEDS_SYNC), &data);
        // 64 is 262144 steps, 63.9 is 261734.4, rounded to 261734: 410 down.
        assert_eq!(
            tracker.send_changes(ID, &data),
            [motion(flowing), pos([0, -410, 0], false)]
        );
        assert_eq!(tracker.send_changes(ID, &data), []);
        // In water every tick is an update: the 401st comes at tick 401 of it.
        let mut tracker = item_at([0.5, 64.0, 0.5], false);
        let mut whole = None;
        for tick_number in 0..=401 {
            tracker.report(
                &tick([0.5, 64.0, 0.5], [0.0; 3], false, Tracked::NEEDS_SYNC),
                &data,
            );
            let sent = tracker.send_changes(ID, &data);
            if matches!(sent[..], [Sent::Sync(_)]) && whole.is_none() {
                whole = Some(tick_number);
            }
        }
        assert_eq!(whole, Some(400));
    }

    /// `syncPosition` (a bounce) moves the count of ticks on to the next multiple of 20
    /// (`nextInterval`), so that this tick is an update and the next is 20 on.
    #[test]
    fn sync_position_makes_the_tick_an_update_and_the_next_twenty_on() {
        let mut tracker = item_at([0.5, 64.0, 0.5], false);
        let data = stack(1);
        run(&mut tracker, &data, 5);
        let up = [0.5, 64.25, 0.5];
        tracker.report(
            &tick(up, [0.0, 0.2, 0.0], false, Tracked::SYNC_POSITION),
            &data,
        );
        // 0.25 is 1024 steps; the count is 20, not a multiple of 60.
        assert_eq!(
            tracker.send_changes(ID, &data),
            [motion([0.0, 0.2, 0.0]), pos([0, 1024, 0], false)]
        );
        let sent = run(&mut tracker, &data, 40);
        // Counts 21 to 60: an update at 40, with nothing to send, and at 60 a move of
        // nothing.
        assert_eq!(sent_in(&sent), [(39, vec![pos([0; 3], false)])]);
    }

    /// `syncVelocity` sends the motion as it is after the update, to the entity's
    /// trackers, without making it the motion last sent.
    #[test]
    fn sync_velocity_sends_the_motion_after_the_tick_s_update() {
        let mut tracker = item_at([0.5, 64.0, 0.5], true);
        let data = stack(1);
        tracker.send_changes(ID, &data);
        let hit = [0.2, 0.3, 0.0];
        tracker.report(
            &tick([0.5, 64.0, 0.5], hit, true, Tracked::SYNC_VELOCITY),
            &data,
        );
        assert_eq!(tracker.send_changes(ID, &data), [motion(hit)]);
        // At the update the motion is sent as changed from the one sent last.
        let sent = run(&mut tracker, &data, 19);
        assert_eq!(sent_in(&sent), [(18, vec![motion(hit)])]);
    }

    /// The motion is sent when it moved by more than 1e-7 squared, or by anything at all
    /// to a standstill.
    #[test]
    fn motion_is_sent_by_the_rule_of_1e_7() {
        let at = [0.5, 64.0, 0.5];
        let data = stack(1);
        let update = |tracker: &mut Tracker, moving: [f64; 3]| {
            tracker.report(&tick(at, moving, true, Tracked::NEEDS_SYNC), &data);
            tracker.send_changes(ID, &data)
        };
        let mut tracker = item_at(at, true);
        // The spawn's update: a move of nothing (count 0).
        assert_eq!(update(&mut tracker, [0.0; 3]), [pos([0; 3], true)]);
        // 0.0003 squared is 9e-8: kept.
        assert_eq!(update(&mut tracker, [0.0003, 0.0, 0.0]), []);
        // 0.00032 squared is 1.024e-7: sent.
        assert_eq!(
            update(&mut tracker, [0.00032, 0.0, 0.0]),
            [motion([0.00032, 0.0, 0.0])]
        );
        assert_eq!(update(&mut tracker, [0.0003, 0.0, 0.0]), []);
        // To nothing: sent though it moved by only 0.00002.
        assert_eq!(update(&mut tracker, [0.0; 3]), [motion([0.0; 3])]);
        assert_eq!(update(&mut tracker, [0.0; 3]), []);
    }

    /// A move of eight blocks or more on an axis is past a short's worth of 4096ths, and
    /// is sent whole; that does not count as the teleport of every 400 updates.
    #[test]
    fn a_move_past_the_delta_s_range_is_sent_whole() {
        let mut tracker = item_at([0.5, 64.0, 0.5], false);
        let data = stack(1);
        tracker.send_changes(ID, &data);
        run(&mut tracker, &data, 19);
        let far = [8.5, 64.0, 0.5];
        tracker.report(&tick(far, [0.0; 3], false, 0), &data);
        assert_eq!(tracker.send_changes(ID, &data), [sync(far, false)]);
        let near = [8.5 - 1.0 / 4096.0, 64.0, 0.5];
        tracker.report(
            &tick([0.5, 64.0, 0.5], [0.0; 3], false, Tracked::NEEDS_SYNC),
            &data,
        );
        tracker.send_changes(ID, &data);
        tracker.report(&tick(near, [0.0; 3], false, Tracked::NEEDS_SYNC), &data);
        assert_eq!(tracker.send_changes(ID, &data), [pos([32767, 0, 0], false)]);
    }

    /// `getRequiresPrecisePosition` (a falling block whose move collided) sends every
    /// update whole.
    #[test]
    fn a_precise_position_is_sent_whole() {
        let seen = Seen {
            kind: entity_types::FALLING_BLOCK as u16,
            position: [0.5, 70.0, 0.5],
            motion: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
        };
        let data = falling();
        let mut tracker = Tracker::new(seen, &data, 0);
        tracker.send_changes(ID, &data);
        let at = [0.5, 69.96, 0.5];
        tracker.report(
            &tick(
                at,
                [0.0, -0.04, 0.0],
                false,
                Tracked::PRECISE | Tracked::NEEDS_SYNC,
            ),
            &data,
        );
        let sent = tracker.send_changes(ID, &data);
        let [Sent::Motion(_), Sent::Sync(whole)] = &sent[..] else {
            panic!("{sent:?}");
        };
        assert_eq!(
            whole.path,
            PositionPath::Linear {
                x: 0.5,
                y: 69.96,
                z: 0.5
            }
        );
    }

    /// An item that collided is sent whole where a delta along x or z would round it off
    /// where it is: after a vertical collision by the axis moved, after a horizontal one
    /// by the other axis, as the game's bytecode has it.
    #[test]
    fn an_item_that_collided_is_sent_whole_where_a_delta_would_round_it() {
        let data = stack(1);
        let update = |flags: u8, to: [f64; 3]| {
            let mut tracker = item_at([0.5, 64.0, 0.5], true);
            tracker.send_changes(ID, &data);
            tracker.report(
                &tick(to, [0.0; 3], true, flags | Tracked::NEEDS_SYNC),
                &data,
            );
            tracker.send_changes(ID, &data)
        };
        let off = 0.6;
        assert!(precision_loss(off) != 0.0);
        let vertical = Tracked::VERTICAL_COLLISION;
        let horizontal = Tracked::HORIZONTAL_COLLISION;
        // Along x to a coordinate off the steps, after landing: whole.
        assert!(matches!(
            update(vertical, [off, 64.0, 0.5])[..],
            [Sent::Sync(_)]
        ));
        // With no collision, a delta.
        assert!(matches!(update(0, [off, 64.0, 0.5])[..], [Sent::Pos(_)]));
        // Along x to a step after landing, a delta.
        assert!(matches!(
            update(vertical, [0.75, 64.0, 0.5])[..],
            [Sent::Pos(_)]
        ));
        // After a horizontal collision a move along x is judged by z: on a step, a
        // delta, though x is off; off a step, whole.
        assert!(matches!(
            update(horizontal, [off, 64.0, 0.5])[..],
            [Sent::Pos(_)]
        ));
        assert!(matches!(
            update(horizontal, [0.75, 64.0, off])[..],
            [Sent::Sync(_)]
        ));
    }

    /// The data goes in the update after its change, after the move; a stack's change
    /// is index 8 alone, fire is the shared flags at index 0, and both together go in
    /// the order of their indices. A client shown the entity after is sent what was sent.
    #[test]
    fn changed_data_is_sent_after_the_move_by_its_indices() {
        let mut tracker = item_at([0.5, 64.0, 0.5], true);
        let one = stack(1);
        tracker.send_changes(ID, &one);
        let mut merged = tick([0.5, 64.0, 0.5], [0.0; 3], true, 0);
        merged.data = Some(stack(5));
        tracker.report(&merged, &one);
        let five = stack(5);
        let stack_value = |count| {
            (
                entity_data::ITEM_STACK,
                EntityDataValue::ItemStack(Some(WireStack { item: 1, count })),
            )
        };
        let data = |values| {
            Sent::Data(SetEntityData {
                entity_id: ID,
                values,
            })
        };
        // A dirty change makes the tick an update with nothing else to send.
        assert_eq!(
            tracker.send_changes(ID, &five),
            [data(vec![stack_value(5)])]
        );
        assert_eq!(tracker.pairing().values, [stack_value(5)]);
        let mut burning = tick([0.5, 64.0, 0.5], [0.0; 3], true, Tracked::ON_FIRE);
        burning.data = Some(stack(4));
        tracker.report(&burning, &five);
        let four = stack(4);
        let fire = |flags| (entity_data::SHARED_FLAGS, EntityDataValue::Byte(flags));
        assert_eq!(
            tracker.send_changes(ID, &four),
            [data(vec![fire(1), stack_value(4)])]
        );
        assert_eq!(tracker.pairing().values, [fire(1), stack_value(4)]);
        // Burning on is no change; going out is, to the default, which a newly shown
        // client is not sent.
        tracker.report(
            &tick([0.5, 64.0, 0.5], [0.0; 3], true, Tracked::ON_FIRE),
            &four,
        );
        assert_eq!(tracker.send_changes(ID, &four), []);
        tracker.report(&tick([0.5, 64.0, 0.5], [0.0; 3], true, 0), &four);
        assert_eq!(tracker.send_changes(ID, &four), [data(vec![fire(0)])]);
        assert_eq!(tracker.pairing().values, [stack_value(4)]);
    }

    /// A turn of a 256th or more is sent with the move (`PosRot`) where a move is sent,
    /// and alone (`Rot`) where none is; the angles sent become the ones compared with.
    #[test]
    fn a_turn_goes_with_the_move_or_alone() {
        let data = stack(1);
        let mut tracker = item_at([0.5, 64.0, 0.5], true);
        tracker.send_changes(ID, &data);
        let mut turned = tick([0.75, 64.0, 0.5], [0.0; 3], true, Tracked::NEEDS_SYNC);
        turned.yaw = 90.0;
        tracker.report(&turned, &data);
        assert_eq!(
            tracker.send_changes(ID, &data),
            [Sent::PosRot(MoveEntityPosRot {
                entity_id: ID,
                delta: VecDelta::Linear([1024, 0, 0]),
                yaw: 64,
                pitch: 0,
                on_ground: true,
            })]
        );
        turned.yaw = 180.0;
        tracker.report(&turned, &data);
        assert_eq!(
            tracker.send_changes(ID, &data),
            [Sent::Rot(MoveEntityRot {
                entity_id: ID,
                yaw: -128,
                pitch: 0,
                on_ground: true,
            })]
        );
        assert_eq!(tracker.pairing().yaw, 128);
        // Less than a 256th is no turn.
        turned.yaw = 181.0;
        tracker.report(&turned, &data);
        assert_eq!(tracker.send_changes(ID, &data), []);
    }

    /// A falling block is shown with its start, the block it fell from, at index 8.
    #[test]
    fn a_falling_block_is_shown_with_where_it_started() {
        let seen = Seen {
            kind: entity_types::FALLING_BLOCK as u16,
            position: [-3.5, 70.0, 2.5],
            motion: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
        };
        let tracker = Tracker::new(seen, &falling(), 7);
        let pairing = tracker.pairing();
        assert_eq!(
            pairing.values,
            [(
                entity_data::START_POSITION,
                EntityDataValue::BlockPos(Position { x: -4, y: 70, z: 2 })
            )]
        );
        assert_eq!(
            (pairing.position, pairing.motion),
            ([-3.5, 70.0, 2.5], [0.0; 3])
        );
        assert_eq!(tracker.ran, 7);
    }

    /// Ticks the edge hears of late run the counters as every tick does, and send only
    /// what changes a client: a still item's moves of nothing and its teleport of every
    /// 400 updates are not sent late.
    #[test]
    fn ticks_caught_up_count_but_send_only_what_changes_a_client() {
        let data = stack(1);
        let mut caught = item_at([0.5, 64.0, 0.5], true);
        let mut every = caught.clone();
        // The clients were spawned with it, which tells them nothing of the ground: the
        // first update tells them it lies on it, and nothing is sent after that.
        let told = caught.catch_up(ID, &data, 8100);
        assert_eq!(told.len(), 1, "{told:?}");
        assert!(
            matches!(&told[0], Sent::Pos(moved) if moved.on_ground)
                || matches!(&told[0], Sent::Sync(moved) if moved.on_ground),
            "{told:?}"
        );
        run(&mut every, &data, 8100);
        assert_eq!(caught, every);
        // A landing reported before the silence is sent at the first update in it.
        let mut caught = item_at([0.5, 65.0, 0.5], false);
        caught.send_changes(ID, &data);
        let landed = [0.5, 64.0, 0.5];
        caught.report(
            &tick(landed, [0.0; 3], true, Tracked::VERTICAL_COLLISION),
            &data,
        );
        assert_eq!(caught.catch_up(ID, &data, 1000), [sync(landed, true)]);
    }
}
