//! Tests of what a client is sent of an item that falls, lands, bounces, floats and is
//! hurt, written from the porting brief's section 9 (`ServerEntity.sendChanges` and
//! `createMovePacket`, read from the 26.3 jar) and the plan of step N4, section 7 (G2:
//! the region's reports), by someone who did not write the edge's tracking: the
//! packets expected in each tick are worked out here by hand from the game's rules,
//! from reports fed as a region makes them, every tick.
//!
//! The rules used: the update of tick `t` runs before the tick's entities, with the
//! entity as its tick `t - 1` left it, and counts the ticks from the first; it is an
//! update tick when that count is a multiple of 20, when the entity needs a sync, or
//! when its data changed; a bounce (`syncPosition`) makes the tick an update tick and
//! the count go on from there; an update sends the motion first if it moved by more
//! than 1e-7 (squared) from the motion sent last, then a move: whole where the entity's
//! being on the ground changed, else relative if it moved by `7.62939453125e-6`
//! (squared) or the count is a multiple of 60; a hurt (`syncVelocity`) sends the motion
//! at the end of the update, whatever it was.

use clustine_protocol::packets::play::{MoveEntityPos, SetEntityMotion, VecDelta};
use clustine_sim::api::Tracked;

use super::*;

const STACK: ItemStack = ItemStack::new(1, 1);

/// `Math.round(v * 4096)`: Java rounds half up, towards positive infinity.
fn steps(value: f64) -> i64 {
    (value * 4096.0 + 0.5).floor() as i64
}

fn delta(from: [f64; 3], to: [f64; 3]) -> [i16; 3] {
    std::array::from_fn(|axis| {
        i16::try_from(steps(to[axis]) - steps(from[axis])).expect("a short delta")
    })
}

/// Two players at the spawn, both shown `HOME` from the west.
async fn watching() -> (Harness, Client) {
    let mut edge = Harness::start().await;
    let mut first = edge.joined(player(1), EntityId(5)).await;
    let mut second = edge.joined(player(2), EntityId(6)).await;
    edge.quiet().await;
    let there = vec![
        EntityState {
            entity: EntityId(5),
            kind: EntityKind::Player {
                player: player(1),
                name: "Player1".to_owned(),
            },
            pose: Pose::at(SPAWN),
        },
        EntityState {
            entity: EntityId(6),
            kind: EntityKind::Player {
                player: player(2),
                name: "Player2".to_owned(),
            },
            pose: Pose::at(SPAWN),
        },
    ];
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot_of(HOME, ask, empty_chunk(), there));
    assert!(edge.shows(&mut first, WEST, HOME).await);
    assert!(edge.shows(&mut second, WEST, HOME).await);
    (edge, first)
}

fn spawn(key: u64, position: [f64; 3], motion: [f64; 3]) -> RegionEvent {
    RegionEvent::EntitySpawned(EntityState {
        entity: EntityId(key),
        kind: EntityKind::Other {
            kind: entity_types::ITEM as u16,
            uuid: Uuid::from_u128(u128::from(key) + 31),
            motion,
            hops: 0,
            data: EntityData::Item {
                stack: STACK,
                pick_up_delay: 10,
                thrower: None,
                health: 5,
                age_offset: 0,
            },
            riding: Default::default(),
        },
        pose: Pose {
            position: Vec3::new(position[0], position[1], position[2]),
            yaw: 45.0,
            pitch: 0.0,
            on_ground: false,
        },
    })
}

fn report(key: u64, position: [f64; 3], motion: [f64; 3], on_ground: bool, flags: u8) -> Tracked {
    Tracked {
        entity: EntityId(key),
        chunk: HOME,
        hops: 0,
        position,
        motion,
        yaw: 45.0,
        pitch: 0.0,
        on_ground,
        flags,
        data: None,
    }
}

/// What `client` is sent of entities' tracking for the region's tick `tick` with
/// `events`.
async fn sent_for(
    edge: &mut Harness,
    client: &mut Client,
    tick: u64,
    events: Vec<RegionEvent>,
) -> Vec<ClientboundPlay> {
    let before = client.tracking.len();
    edge.tell(WEST, WorkerToEdge::TickDelta { tick, events });
    edge.settle(WEST).await;
    edge.sync(client).await;
    client.tracking[before..].to_vec()
}

fn reported(tracked: Option<Tracked>) -> Vec<RegionEvent> {
    let mut events = vec![RegionEvent::BlockEventsBegin];
    events.extend(tracked.map(RegionEvent::EntityTracked));
    events
}

/// The packets, as a client reads them.
struct Wire(i32);

impl Wire {
    fn motion(&self, motion: [f64; 3]) -> ClientboundPlay {
        let packet = SetEntityMotion {
            entity_id: self.0,
            motion,
        };
        ClientboundPlay::decode(&packets::encode(&packet)).expect("a packet")
    }

    fn pos(&self, delta: [i16; 3], on_ground: bool) -> ClientboundPlay {
        ClientboundPlay::MoveEntityPos(MoveEntityPos {
            entity_id: self.0,
            delta: VecDelta::Linear(delta),
            on_ground,
        })
    }

    fn whole(&self, at: [f64; 3], on_ground: bool) -> ClientboundPlay {
        ClientboundPlay::SyncEntityPosition(SyncEntityPosition {
            entity_id: self.0,
            path: PositionPath::Linear {
                x: at[0],
                y: at[1],
                z: at[2],
            },
            yaw: 45.0,
            pitch: 0.0,
            on_ground,
        })
    }
}

/// An item's tick in the air over a floor whose top is y -60, as `ItemEntity.tick` has
/// it (the brief, sections 1 and 3): gravity, the move cut at the top, the drag; the
/// position, the motion, the landing and whether the motion changed by more than 0.1.
fn fall(position: &mut [f64; 3], motion: &mut [f64; 3]) -> (bool, bool) {
    let drag = f64::from(0.98_f32);
    let start = *motion;
    motion[1] += -0.04;
    let top = -60.0;
    let cut = motion[1].max(top - position[1]);
    let landed = cut != motion[1];
    position[1] += cut;
    if landed {
        motion[1] = (0.0 - motion[1]) * 1.0 * 0.0;
    }
    motion[1] *= drag;
    let change: f64 = (0..3)
        .map(|axis| (motion[axis] - start[axis]).powi(2))
        .sum();
    (landed, change > 0.01)
}

/// The brief, section 9: an item thrown up from y -55 is sent a move of nothing in its
/// spawn's tick (count 0, a multiple of 60); in tick 30 its motion, changed from the
/// spawn's, and its move from where it was spawned to where tick 29 left it; nothing
/// else while it falls; in the tick after its landing (which needs a sync, its motion
/// cut by more than 0.1) its motion of nothing and, its being on the ground changed,
/// its whole position; after that, at rest and reported no more, nothing until the
/// count of 60, then a move of nothing on the ground every 60 ticks.
#[tokio::test]
async fn a_falling_item_is_sent_its_moves_motion_and_landing_at_the_game_s_ticks() {
    let (mut edge, mut first) = watching().await;
    let key = 1 << 32;
    let start = [1.5, -55.0, 1.5];
    let thrown = [0.0, 0.2, 0.0];
    // The tick of the spawn, in which the item ticks too.
    let mut position = start;
    let mut motion = thrown;
    let mut at = BTreeMap::new();
    let mut landing = None;
    let mut reports = BTreeMap::new();
    for tick in 10..200_u64 {
        if landing.is_some() {
            break;
        }
        let (landed, sync) = fall(&mut position, &mut motion);
        let mut flags = 0;
        if landed {
            flags |= Tracked::VERTICAL_COLLISION;
            landing = Some(tick);
        }
        if sync {
            flags |= Tracked::NEEDS_SYNC;
        }
        reports.insert(tick, report(key, position, motion, landed, flags));
        at.insert(tick, (position, motion));
    }
    let landing = landing.expect("it lands");
    assert!(
        (31..49).contains(&landing),
        "the scenario wants it to land between updates: {landing}"
    );
    assert!(reports[&landing].flags & Tracked::NEEDS_SYNC != 0);
    let mut events = vec![spawn(key, start, thrown)];
    events.extend(reported(reports.get(&10).cloned()));
    let sent = sent_for(&mut edge, &mut first, 10, events).await;
    let wire = Wire(first.naming(key)[0].1);
    let shown = first
        .spawns
        .iter()
        .find(|spawn| spawn.entity_id == wire.0)
        .expect("spawned");
    assert_eq!((shown.x, shown.y, shown.z), (1.5, -55.0, 1.5));
    assert_eq!(sent, [wire.pos([0; 3], false)], "tick 10");
    let (landed_at, _) = at[&landing];
    for tick in 11..=135 {
        let sent = sent_for(
            &mut edge,
            &mut first,
            tick,
            reported(reports.get(&tick).cloned()),
        )
        .await;
        let expected = if tick == 30 {
            let (p, v) = at[&29];
            vec![wire.motion(v), wire.pos(delta(start, p), false)]
        } else if tick == landing + 1 {
            vec![wire.motion([0.0; 3]), wire.whole(landed_at, true)]
        } else if tick == 70 || tick == 130 {
            vec![wire.pos([0; 3], true)]
        } else {
            Vec::new()
        };
        assert_eq!(sent, expected, "tick {tick} (landed in {landing})");
    }
    edge.end().await;
}

/// The brief, section 9 (`syncPosition`, `nextInterval`): a bounce makes the next
/// update come at once and the updates go on every 20 ticks from there: an item
/// spawned in tick 10 that bounces in tick 15 is updated in tick 16 and then in ticks
/// 36 and 56, not in 30 and 50.
#[tokio::test]
async fn a_bounce_brings_the_update_forward_and_the_next_ones_count_from_it() {
    let (mut edge, mut first) = watching().await;
    let key = (1 << 32) + 1;
    let start = [2.5, -50.0, 2.5];
    let falling = [0.0, -0.1, 0.0];
    let at = |tick: u64| [2.5, -50.0 - 0.1 * (tick - 9) as f64, 2.5];
    let mut events = vec![spawn(key, start, falling)];
    events.extend(reported(Some(report(key, at(10), falling, false, 0))));
    let sent = sent_for(&mut edge, &mut first, 10, events).await;
    let wire = Wire(first.naming(key)[0].1);
    assert_eq!(sent, [wire.pos([0; 3], false)]);
    let mut base = start;
    for tick in 11..=60 {
        let flags = if tick == 15 {
            Tracked::SYNC_POSITION
        } else {
            0
        };
        let tracked = report(key, at(tick), falling, false, flags);
        let sent = sent_for(&mut edge, &mut first, tick, reported(Some(tracked))).await;
        if [16, 36, 56].contains(&tick) {
            let now = at(tick - 1);
            assert_eq!(sent, [wire.pos(delta(base, now), false)], "tick {tick}");
            base = now;
        } else {
            assert_eq!(sent, [], "tick {tick}");
        }
    }
    edge.end().await;
}

/// The brief, section 9 (`needsSync`): an item in water is reported needing a sync
/// every tick, and is updated every tick: a move whenever it is more than the
/// tolerance from where it was sent last, so one drifting 0.002 a tick (4e-6 squared,
/// under the tolerance) every second tick, and its motion only when that changed.
#[tokio::test]
async fn an_item_that_needs_a_sync_is_updated_every_tick_and_moved_when_past_the_tolerance() {
    let (mut edge, mut first) = watching().await;
    let key = (1 << 32) + 2;
    let start = [3.25, -59.5, 3.25];
    let drifting = [0.002, 0.0, 0.0];
    let at = |tick: u64| [3.25 + 0.002 * (tick - 9) as f64, -59.5, 3.25];
    let mut events = vec![spawn(key, start, drifting)];
    events.extend(reported(Some(report(
        key,
        at(10),
        drifting,
        false,
        Tracked::NEEDS_SYNC,
    ))));
    let sent = sent_for(&mut edge, &mut first, 10, events).await;
    let wire = Wire(first.naming(key)[0].1);
    assert_eq!(sent, [wire.pos([0; 3], false)]);
    let mut base = start;
    for tick in 11..=40 {
        let tracked = report(key, at(tick), drifting, false, Tracked::NEEDS_SYNC);
        let sent = sent_for(&mut edge, &mut first, tick, reported(Some(tracked))).await;
        let now = at(tick - 1);
        let far: f64 = (0..3).map(|axis| (now[axis] - base[axis]).powi(2)).sum();
        let count = tick - 10;
        if far >= 7.629_394_531_25e-6 || count % 60 == 0 {
            assert_eq!(sent, [wire.pos(delta(base, now), false)], "tick {tick}");
            base = now;
        } else {
            assert_eq!(sent, [], "tick {tick}");
        }
    }
    // Every second tick, as the drift adds up past the tolerance in two.
    edge.end().await;
}

/// The brief, sections 3.3 and 9 (`markHurt`, `syncVelocity`): an item hurt is sent
/// its motion in the next update, at its end, though that motion is the one sent last
/// and the tick is no update tick.
#[tokio::test]
async fn a_hurt_item_is_sent_its_motion_whatever_was_sent_before() {
    let (mut edge, mut first) = watching().await;
    let key = (1 << 32) + 3;
    let lying = [4.5, -60.0, 4.5];
    let still = [0.0, 0.0, 0.0];
    let mut events = vec![spawn(key, lying, still)];
    events.extend(reported(None));
    let sent = sent_for(&mut edge, &mut first, 10, events).await;
    let wire = Wire(first.naming(key)[0].1);
    assert_eq!(sent, [wire.pos([0; 3], false)]);
    for tick in 11..=20 {
        let tracked =
            (tick == 14).then(|| report(key, lying, still, false, Tracked::SYNC_VELOCITY));
        let sent = sent_for(&mut edge, &mut first, tick, reported(tracked)).await;
        if tick == 15 {
            assert_eq!(sent, [wire.motion(still)], "tick {tick}");
        } else {
            assert_eq!(sent, [], "tick {tick}");
        }
    }
    edge.end().await;
}

/// The brief, section 9 (`isFullPrecisionEncodingRequired`, for items only): a
/// relative move is sent whole instead where the item's last move collided and the
/// delta would lose precision: after a vertical collision, where it moved along x and
/// its x is off the 1/4096 grid; after a horizontal one, crossed, where it moved along
/// x and its z is off the grid. Otherwise the relative move.
#[tokio::test]
async fn an_item_s_move_after_a_collision_is_sent_whole_where_the_delta_would_lose_precision() {
    let (mut edge, mut first) = watching().await;
    let still = [0.0; 3];
    // (key, start, where tick 11 leaves it, its flags, whether it is sent whole)
    let cases = [
        (
            (1 << 32) + 10,
            [5.5, -60.0, 5.5],
            [5.5 + 1.0 / 3.0, -60.0, 5.5],
            Tracked::VERTICAL_COLLISION,
            true,
        ),
        (
            (1 << 32) + 11,
            [6.5, -60.0, 6.5],
            [6.75, -60.0, 6.5],
            Tracked::VERTICAL_COLLISION,
            false,
        ),
        (
            (1 << 32) + 12,
            [7.5, -60.0, 7.1],
            [7.75, -60.0, 7.1],
            Tracked::HORIZONTAL_COLLISION,
            true,
        ),
        (
            (1 << 32) + 13,
            [8.5, -60.0, 8.5],
            [8.5 + 1.0 / 3.0, -60.0, 8.5],
            Tracked::HORIZONTAL_COLLISION,
            false,
        ),
    ];
    let mut events = Vec::new();
    for (key, start, _, _, _) in cases {
        let RegionEvent::EntitySpawned(mut state) = spawn(key, start, still) else {
            unreachable!()
        };
        state.pose.on_ground = true;
        events.push(RegionEvent::EntitySpawned(state));
    }
    events.push(RegionEvent::BlockEventsBegin);
    sent_for(&mut edge, &mut first, 10, events).await;
    let moved = cases
        .iter()
        .map(|(key, _, to, flags, _)| {
            RegionEvent::EntityTracked(report(*key, *to, still, true, *flags))
        })
        .collect();
    let mut events = vec![RegionEvent::BlockEventsBegin];
    events.extend::<Vec<RegionEvent>>(moved);
    sent_for(&mut edge, &mut first, 11, events).await;
    for tick in 12..30 {
        sent_for(&mut edge, &mut first, tick, reported(None)).await;
    }
    let sent = sent_for(&mut edge, &mut first, 30, reported(None)).await;
    for (key, start, to, flags, whole) in cases {
        let wire = Wire(first.naming(key)[0].1);
        let mine: Vec<&ClientboundPlay> = sent
            .iter()
            .filter(|packet| match packet {
                ClientboundPlay::MoveEntityPos(moved) => moved.entity_id == wire.0,
                ClientboundPlay::SyncEntityPosition(moved) => moved.entity_id == wire.0,
                ClientboundPlay::SetEntityMotion(moved) => moved.entity_id == wire.0,
                _ => false,
            })
            .collect();
        let expected = if whole {
            wire.whole(to, true)
        } else {
            wire.pos(delta(start, to), true)
        };
        assert_eq!(
            mine,
            [&expected],
            "flags {flags:#b}, from {start:?} to {to:?}"
        );
    }
    edge.end().await;
}

/// The brief, section 9: a motion is sent again only when it is more than 1e-7
/// (squared) from the one sent last, but a motion of nothing is sent whenever it
/// differs at all: of two items spawned moving at 0.0002 along x, the one reported
/// still is sent its motion of nothing in tick 30, and the one reported at 0.0001 is
/// sent nothing.
#[tokio::test]
async fn a_motion_of_nothing_is_sent_however_little_it_differs_and_others_by_the_tolerance() {
    let (mut edge, mut first) = watching().await;
    let (stopping, slowing) = ((1 << 32) + 20, (1 << 32) + 21);
    let at = [5.5, -60.0, 1.5];
    let moving = [0.0002, 0.0, 0.0];
    let mut events = vec![
        spawn(stopping, at, moving),
        spawn(slowing, [6.5, -60.0, 1.5], moving),
    ];
    events.push(RegionEvent::BlockEventsBegin);
    events.push(RegionEvent::EntityTracked(report(
        stopping, at, [0.0; 3], false, 0,
    )));
    events.push(RegionEvent::EntityTracked(report(
        slowing,
        [6.5, -60.0, 1.5],
        [0.0001, 0.0, 0.0],
        false,
        0,
    )));
    sent_for(&mut edge, &mut first, 10, events).await;
    for tick in 11..30 {
        sent_for(&mut edge, &mut first, tick, reported(None)).await;
    }
    let sent = sent_for(&mut edge, &mut first, 30, reported(None)).await;
    let stopped = Wire(first.naming(stopping)[0].1);
    let motions: Vec<&ClientboundPlay> = sent
        .iter()
        .filter(|packet| matches!(packet, ClientboundPlay::SetEntityMotion(_)))
        .collect();
    assert_eq!(motions, [&stopped.motion([0.0; 3])], "{sent:?}");
    edge.end().await;
}

/// The brief, section 9 (`teleportDelay`): every update counts, and the 401st since the
/// last whole position sends the position whole instead of a relative move: an item
/// that needs a sync every tick, and moves 0.01 a tick, is sent relative moves for 400
/// updates and its whole position in the 401st, tick 410.
#[tokio::test]
async fn the_four_hundred_and_first_update_sends_the_position_whole() {
    let (mut edge, mut first) = watching().await;
    let key = (1 << 32) + 22;
    let flowing = [0.01, 0.0, 0.0];
    let at = |tick: u64| [1.5, -59.5, 2.0 + 0.0001 * (tick - 9) as f64 + 0.0];
    let at = |tick: u64| {
        let mut p = at(tick);
        p[0] += 0.01 * (tick - 9) as f64;
        p
    };
    let mut events = vec![spawn(key, at(9), flowing)];
    events.extend(reported(Some(report(
        key,
        at(10),
        flowing,
        false,
        Tracked::NEEDS_SYNC,
    ))));
    let sent = sent_for(&mut edge, &mut first, 10, events).await;
    let wire = Wire(first.naming(key)[0].1);
    assert_eq!(sent, [wire.pos([0; 3], false)]);
    let mut base = at(9);
    for tick in 11..=420 {
        let tracked = report(key, at(tick), flowing, false, Tracked::NEEDS_SYNC);
        let sent = sent_for(&mut edge, &mut first, tick, reported(Some(tracked))).await;
        let now = at(tick - 1);
        if tick == 410 {
            assert_eq!(sent, [wire.whole(now, false)], "tick {tick}");
        } else {
            assert_eq!(sent, [wire.pos(delta(base, now), false)], "tick {tick}");
        }
        base = now;
    }
    edge.end().await;
}

/// The brief, section 9: the game runs `sendChanges` for every entity it tracks, every
/// tick, whether it changed or not, and sends a move of nothing (with its being on the
/// ground) every 60 ticks: so a client shown an item lying in a chunk it was sent is
/// told it lies on the ground within 60 ticks, and every 60 ticks from then, though the
/// region never reports the item, which does not move. Where the count starts is the
/// edge's; that it runs is the game's.
#[tokio::test]
async fn an_item_shown_in_a_chunk_s_snapshot_is_sent_its_periodic_moves_though_never_reported() {
    let mut edge = Harness::start().await;
    let mut first = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    let key = (1 << 32) + 30;
    let RegionEvent::EntitySpawned(mut lying) = spawn(key, [2.5, -60.0, 2.5], [0.0; 3]) else {
        unreachable!()
    };
    lying.pose.on_ground = true;
    let there = vec![
        EntityState {
            entity: EntityId(5),
            kind: EntityKind::Player {
                player: player(1),
                name: "Player1".to_owned(),
            },
            pose: Pose::at(SPAWN),
        },
        lying,
    ];
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot_of(HOME, ask, empty_chunk(), there));
    assert!(edge.shows(&mut first, WEST, HOME).await);
    assert!(first.entities.contains(&key));
    let wire = Wire(first.naming(key)[0].1);
    let mut moves = Vec::new();
    for tick in 2..=200 {
        let sent = sent_for(&mut edge, &mut first, tick, Vec::new()).await;
        for packet in sent {
            assert_eq!(packet, wire.pos([0; 3], true), "tick {tick}");
            moves.push(tick);
        }
    }
    assert!(
        moves.first().is_some_and(|tick| *tick <= 62),
        "the first move of nothing came in {moves:?}"
    );
    for pair in moves.windows(2) {
        assert_eq!(pair[1] - pair[0], 60, "{moves:?}");
    }
    edge.end().await;
}

/// The brief, section 9: a client is told an entity's being on the ground only by its
/// moves (`AddEntity` carries none, so a client shows a new entity off the ground), and
/// the game sends a move of nothing, with the ground, within 60 ticks of tracking an
/// entity: so a client shown a lying item in a chunk's snapshot is told it lies on the
/// ground by the time the region next speaks, though the region was quiet for a while
/// and the edge catches its ticks up late.
#[tokio::test]
async fn a_client_shown_a_lying_item_is_told_it_lies_on_the_ground_after_a_quiet_spell() {
    let mut edge = Harness::start().await;
    let mut first = edge.joined(player(1), EntityId(5)).await;
    edge.quiet().await;
    let key = (1 << 32) + 31;
    let RegionEvent::EntitySpawned(mut lying) = spawn(key, [2.5, -60.0, 2.5], [0.0; 3]) else {
        unreachable!()
    };
    lying.pose.on_ground = true;
    let there = vec![
        EntityState {
            entity: EntityId(5),
            kind: EntityKind::Player {
                player: player(1),
                name: "Player1".to_owned(),
            },
            pose: Pose::at(SPAWN),
        },
        lying,
    ];
    let ask = edge.ask(WEST, HOME);
    edge.tell(WEST, snapshot_of(HOME, ask, empty_chunk(), there));
    assert!(edge.shows(&mut first, WEST, HOME).await);
    let wire = Wire(first.naming(key)[0].1);
    // The region says nothing for 150 ticks, then speaks of something else.
    let sent = sent_for(&mut edge, &mut first, 151, Vec::new()).await;
    assert!(
        sent.contains(&wire.pos([0; 3], true)),
        "never told it lies on the ground: {sent:?}"
    );
    edge.end().await;
}
