//! Tests of minecarts on rails (step N5e, `docs/groundwork/n5-plan.md`;
//! `docs/groundwork/n5-brief.md`, section 3). The numbers each test expects are worked
//! out here from the game's formulas as its bytecode has them (`OldMinecartBehavior.
//! moveAlongTrack`, `getPos`, `AbstractMinecart.push`, `MinecartFurnace`), written out
//! afresh in this file rather than taken from what the region computes.
//!
//! The regions are those of the tests of redstone (`redstone.rs`): player 1 at the
//! spawn, (0.5, -60, 0.5), on a stone floor at y = -61, with the 25 chunks within two of
//! chunk (0, 0) loaded so that the nine within one tick. The carts run along z = 8 and
//! beyond, away from the player.

use std::collections::BTreeMap;

use clustine_data::entity_types;
use clustine_world::{
    Double, Due, Effect, EffectId, Effected, Entity, EntityChange, EntityData, EntityUse,
    FiledEffect, Float, Hurt, Seat,
};

use super::redstone::{block_at, click, holding, state, ticking_with};
use super::*;
use crate::api::Tracked;

fn key(n: u64) -> EntityId {
    EntityId((1 << 40) + n)
}

fn rail(shape: &str) -> BlockState {
    state(&format!("minecraft:rail[shape={shape},waterlogged=false]"))
}

fn powered_rail(shape: &str, powered: bool) -> BlockState {
    state(&format!(
        "minecraft:powered_rail[shape={shape},powered={powered},waterlogged=false]"
    ))
}

fn activator_rail(powered: bool) -> BlockState {
    state(&format!(
        "minecraft:activator_rail[shape=east_west,powered={powered},waterlogged=false]"
    ))
}

fn detector_rail(powered: bool) -> BlockState {
    state(&format!(
        "minecraft:detector_rail[shape=east_west,powered={powered},waterlogged=false]"
    ))
}

fn cart_data(contents: Vec<(u8, ItemStack)>) -> EntityData {
    EntityData::Minecart {
        hurt: Hurt::default(),
        display: None,
        display_offset: None,
        flipped: false,
        contents,
        fuel: 0,
        push: [Double(0.0); 2],
        fuse: -1,
        enabled: true,
    }
}

/// A cart of the type `kind` at `position`, moving at `motion`.
fn cart_of(n: u64, kind: i32, position: [f64; 3], motion: [f64; 3]) -> Entity {
    Entity {
        key: key(n),
        kind: kind as u16,
        uuid: uuid::Uuid::from_u128(u128::from(n)),
        position,
        motion,
        yaw: 0.0,
        pitch: 0.0,
        on_ground: false,
        hops: 0,
        lived: 1,
        fresh_at: 0,
        data: cart_data(Vec::new()),
        fall_distance: 0.0,
        fire_ticks: 0,
        riding: Default::default(),
    }
}

fn cart(n: u64, position: [f64; 3], motion: [f64; 3]) -> Entity {
    cart_of(n, entity_types::MINECART, position, motion)
}

/// Lays `entity` in its loaded chunk, as a load would have it.
fn lay(region: &mut Region, entity: &Entity) {
    let chunk = entity.chunk();
    region
        .chunks
        .get_mut(&chunk)
        .unwrap()
        .apply_entity(&EntityChange::Whole(entity.clone()));
    region.entity_chunks.insert(entity.key, chunk);
}

fn record(region: &Region, key: EntityId) -> Option<Entity> {
    region.find_entity(key).map(|(_, entity)| entity.clone())
}

fn bits(values: [f64; 3]) -> [u64; 3] {
    values.map(f64::to_bits)
}

/// The height a cart rides a flat rail at y = -60 at: a sixteenth above the block.
const FLAT: f64 = -60.0 + 0.0625;

/// The game's own reckoning of a plain, empty cart alone on rails in the open, out of
/// water and away from anything it could meet, written from the brief's sections 3.4
/// to 3.6 and 3.6.1: gravity, the rail under it, the slope's push, the speed turned
/// onto the rail, the snap onto its line, the move (free, so its own), the step down
/// a slope's low end, the slowdown, the speed from height and the turn into a new
/// block.
struct Track {
    rails: BTreeMap<(i32, i32, i32), &'static str>,
}

impl Track {
    fn shape(&self, x: i32, y: i32, z: i32) -> Option<&'static str> {
        self.rails.get(&(x, y, z)).copied()
    }

    /// `exits(shape)`, as the brief's table gives them.
    fn exits(shape: &str) -> ([i32; 3], [i32; 3]) {
        match shape {
            "north_south" => ([0, 0, -1], [0, 0, 1]),
            "east_west" => ([-1, 0, 0], [1, 0, 0]),
            "ascending_east" => ([-1, -1, 0], [1, 0, 0]),
            "ascending_west" => ([-1, 0, 0], [1, -1, 0]),
            "ascending_north" => ([0, 0, -1], [0, -1, 1]),
            "ascending_south" => ([0, -1, -1], [0, 0, 1]),
            "south_east" => ([0, 0, 1], [1, 0, 0]),
            "south_west" => ([0, 0, 1], [-1, 0, 0]),
            "north_west" => ([0, 0, -1], [-1, 0, 0]),
            "north_east" => ([0, 0, -1], [1, 0, 0]),
            _ => unreachable!("{shape}"),
        }
    }

    /// `getPos(x, y, z)`.
    fn get_pos(&self, [x, y, z]: [f64; 3]) -> Option<[f64; 3]> {
        let bx = x.floor() as i32;
        let mut by = y.floor() as i32;
        let bz = z.floor() as i32;
        if self.shape(bx, by - 1, bz).is_some() {
            by -= 1;
        }
        let shape = self.shape(bx, by, bz)?;
        let (a, b) = Self::exits(shape);
        let x0 = bx as f64 + 0.5 + a[0] as f64 * 0.5;
        let y0 = by as f64 + 0.0625 + a[1] as f64 * 0.5;
        let z0 = bz as f64 + 0.5 + a[2] as f64 * 0.5;
        let x1 = bx as f64 + 0.5 + b[0] as f64 * 0.5;
        let y1 = by as f64 + 0.0625 + b[1] as f64 * 0.5;
        let z1 = bz as f64 + 0.5 + b[2] as f64 * 0.5;
        let (ex, ey, ez) = (x1 - x0, (y1 - y0) * 2.0, z1 - z0);
        let t = if ex == 0.0 {
            z - bz as f64
        } else if ez == 0.0 {
            x - bx as f64
        } else {
            ((x - x0) * ex + (z - z0) * ez) * 2.0
        };
        let mut out = [x0 + ex * t, y0 + ey * t, z0 + ez * t];
        if ey < 0.0 {
            out[1] += 1.0;
        } else if ey > 0.0 {
            out[1] += 0.5;
        }
        Some(out)
    }

    /// One tick of the cart at `p` moving at `v`.
    fn tick(&self, p: [f64; 3], v: [f64; 3]) -> ([f64; 3], [f64; 3]) {
        let mut v = [v[0], v[1] + -0.04, v[2]];
        let (bx, mut by, bz) = (
            p[0].floor() as i32,
            p[1].floor() as i32,
            p[2].floor() as i32,
        );
        if self.shape(bx, by - 1, bz).is_some() {
            by -= 1;
        }
        let shape = self.shape(bx, by, bz).expect("the cart stays on the rails");
        let before = self.get_pos(p);
        let mut y = by as f64;
        let slope = 0.0078125;
        let push = match shape {
            "ascending_east" => Some([-slope, 0.0, 0.0]),
            "ascending_west" => Some([slope, 0.0, 0.0]),
            "ascending_north" => Some([0.0, 0.0, slope]),
            "ascending_south" => Some([0.0, 0.0, -slope]),
            _ => None,
        };
        if let Some(push) = push {
            v = [v[0] + push[0], v[1] + push[1], v[2] + push[2]];
            y += 1.0;
        }
        let (a, b) = Self::exits(shape);
        let (mut ex, mut ez) = ((b[0] - a[0]) as f64, (b[2] - a[2]) as f64);
        let length = (ex * ex + ez * ez).sqrt();
        if v[0] * ex + v[2] * ez < 0.0 {
            ex = -ex;
            ez = -ez;
        }
        let h = 2.0_f64.min((v[0] * v[0] + v[2] * v[2]).sqrt());
        v = [h * ex / length, v[1], h * ez / length];
        let x0 = bx as f64 + 0.5 + a[0] as f64 * 0.5;
        let z0 = bz as f64 + 0.5 + a[2] as f64 * 0.5;
        let x1 = bx as f64 + 0.5 + b[0] as f64 * 0.5;
        let z1 = bz as f64 + 0.5 + b[2] as f64 * 0.5;
        let (ex, ez) = (x1 - x0, z1 - z0);
        let t = if ex == 0.0 {
            p[2] - bz as f64
        } else if ez == 0.0 {
            p[0] - bx as f64
        } else {
            ((p[0] - x0) * ex + (p[2] - z0) * ez) * 2.0
        };
        let mut q = [x0 + ex * t, y, z0 + ez * t];
        let clamp = |value: f64| value.clamp(-0.4, 0.4);
        let step = [clamp(1.0 * v[0]), 0.0, clamp(1.0 * v[2])];
        q = [q[0] + step[0], q[1] + step[1], q[2] + step[2]];
        let enters = |exit: [i32; 3]| {
            q[0].floor() as i32 - bx == exit[0] && q[2].floor() as i32 - bz == exit[2]
        };
        if a[1] != 0 && enters(a) {
            q[1] += a[1] as f64;
        } else if b[1] != 0 && enters(b) {
            q[1] += b[1] as f64;
        }
        v = [v[0] * 0.96, v[1] * 0.0, v[2] * 0.96];
        if let (Some(after), Some(before)) = (self.get_pos(q), before) {
            let d = (before[1] - after[1]) * 0.05;
            let h = (v[0] * v[0] + v[2] * v[2]).sqrt();
            if h > 0.0 {
                v = [v[0] * ((h + d) / h), v[1] * 1.0, v[2] * ((h + d) / h)];
            }
            q[1] = after[1];
        }
        let (nx, nz) = (q[0].floor() as i32, q[2].floor() as i32);
        if nx != bx || nz != bz {
            let h = (v[0] * v[0] + v[2] * v[2]).sqrt();
            v = [h * (nx - bx) as f64, v[1], h * (nz - bz) as f64];
        }
        (q, v)
    }
}

/// Lays `rails` and the blocks `held` that hold the slopes' upper ends, lets a cart go
/// at `start`, and checks it every tick of `ticks` against the game's own reckoning, to
/// the bit. Returns where it ends and how it moves then.
fn roll_against_the_game(
    rails: &[((i32, i32, i32), &'static str)],
    held: &[BlockPos],
    start: [f64; 3],
    ticks: u32,
) -> ([f64; 3], [f64; 3]) {
    let track = Track {
        rails: rails.iter().copied().collect(),
    };
    let mut blocks: Vec<(BlockPos, BlockState)> = rails
        .iter()
        .map(|&((x, y, z), shape)| (BlockPos::new(x, y, z), rail(shape)))
        .collect();
    blocks.extend(held.iter().map(|at| (*at, blocks::STONE)));
    let mut region = ticking_with(&blocks);
    let rolling = cart(1, start, [0.0; 3]);
    lay(&mut region, &rolling);
    let (mut p, mut v) = (start, [0.0; 3]);
    for tick in 1..=ticks {
        (p, v) = track.tick(p, v);
        idle(&mut region);
        let now = record(&region, rolling.key).unwrap();
        assert_eq!(bits(now.position), bits(p), "tick {tick}: {now:?}");
        assert_eq!(bits(now.motion), bits(v), "tick {tick}: {now:?}");
    }
    (p, v)
}

/// Where `MinecartItem` puts a cart on the slope at `(x, -60, z)`: half a block higher
/// than on a flat rail.
fn on_slope(x: i32, z: i32) -> [f64; 3] {
    [f64::from(x) + 0.5, -60.0 + 0.0625 + 0.5, f64::from(z) + 0.5]
}

/// A cart let go at the top of a slope of rails rolls down it, along the rail at its
/// foot and round a curve onto rails running south, and is where the game's own
/// reckoning has it, to the bit, every tick. Its first tick is worked out by hand too.
#[test]
fn a_cart_let_go_on_a_slope_rolls_down_and_round_a_curve_where_the_game_has_it() {
    let mut rails = vec![((4, -60, 8), "ascending_east"), ((3, -60, 8), "south_east")];
    rails.extend((9..=14).map(|z| ((3, -60, z), "north_south")));
    let held = [BlockPos::new(5, -60, 8)];
    let start = on_slope(4, 8);

    // The first tick, by hand: the slope's push of 1/128 west, the snap at the
    // rail's middle a block up, the move of that push, the slowdown, and the speed of
    // the height lost: from (4.5, -59.4375) at the slope's top to x = 4.4921875, whose
    // point of the slope is 0.0078125 lower.
    let (first, first_motion) = roll_against_the_game(&rails, &held, start, 1);
    let x = 4.5 + -0.0078125;
    let y = (-60.0 + 0.0625 - 0.5) + 1.0 * (x - 4.0) + 0.5;
    assert_eq!(bits(first), bits([x, y, 8.5]));
    let slowed: f64 = -0.0078125 * 0.96;
    let h = (slowed * slowed + 0.0 * 0.0).sqrt();
    let gained = (start[1] - y) * 0.05;
    assert_eq!(
        first_motion[0].to_bits(),
        (slowed * ((h + gained) / h)).to_bits()
    );

    let (p, v) = roll_against_the_game(&rails, &held, start, 40);
    // It ends going south along the rails beyond the curve, at their middle.
    assert!(p[2] > 9.0, "it went round the curve: {p:?}");
    assert_eq!(p[0], 3.5);
    assert!(v[2] > 0.0 && v[0] == 0.0, "{v:?}");
}

/// The other three slopes and curves: down a slope rising west into a curve turning
/// north, down one rising north into a curve turning east, and down one rising south
/// into a curve turning west, each where the game has it every tick.
#[test]
fn carts_roll_down_every_slope_and_round_every_curve_where_the_game_has_them() {
    // Rising west, the cart rolls east and turns north.
    let mut rails = vec![
        ((10, -60, 8), "ascending_west"),
        ((11, -60, 8), "north_west"),
    ];
    rails.extend((2..=7).map(|z| ((11, -60, z), "north_south")));
    let (p, v) = roll_against_the_game(&rails, &[BlockPos::new(9, -60, 8)], on_slope(10, 8), 40);
    assert!(p[2] < 8.0 && v[2] < 0.0 && p[0] == 11.5, "{p:?} {v:?}");

    // Rising north, it rolls south and turns east.
    let mut rails = vec![
        ((4, -60, 18), "ascending_north"),
        ((4, -60, 19), "north_east"),
    ];
    rails.extend((5..=10).map(|x| ((x, -60, 19), "east_west")));
    let (p, v) = roll_against_the_game(&rails, &[BlockPos::new(4, -60, 17)], on_slope(4, 18), 40);
    assert!(p[0] > 5.0 && v[0] > 0.0 && p[2] == 19.5, "{p:?} {v:?}");

    // Rising south, it rolls north and turns west.
    let mut rails = vec![
        ((12, -60, 24), "ascending_south"),
        ((12, -60, 23), "south_west"),
    ];
    rails.extend((6..=11).map(|x| ((x, -60, 23), "east_west")));
    let (p, v) = roll_against_the_game(&rails, &[BlockPos::new(12, -60, 25)], on_slope(12, 24), 40);
    assert!(p[0] < 12.0 && v[0] < 0.0 && p[2] == 23.5, "{p:?} {v:?}");
}

/// A cart at rest on a powered rail beside a block that conducts is kicked away from
/// it, 0.02, and then gains 0.06 a tick after the slowdown, as the game adds it.
#[test]
fn a_powered_rail_kicks_a_cart_away_from_a_wall_and_speeds_it_up() {
    let mut blocks = vec![(BlockPos::new(1, -60, 8), blocks::STONE)];
    for x in 2..=12 {
        // Each on a block of redstone, which powers it from below.
        blocks.push((BlockPos::new(x, -61, 8), blocks::REDSTONE_BLOCK));
        blocks.push((BlockPos::new(x, -60, 8), powered_rail("east_west", true)));
    }
    let mut region = ticking_with(&blocks);
    let resting = cart(1, [2.5, FLAT, 8.5], [0.0; 3]);
    lay(&mut region, &resting);
    idle(&mut region);
    let first = record(&region, resting.key).unwrap();
    // Nowhere to go yet: from rest it is only kicked, away from the stone to the west.
    assert_eq!(bits(first.position), bits([2.5, FLAT, 8.5]));
    assert_eq!(first.motion[0], 0.02);
    // The speed along the rail is the length of the motion, turned onto it.
    let along = |v: f64| (v * v + 0.0 * 0.0).sqrt() * 2.0 / 2.0;
    idle(&mut region);
    let second = record(&region, resting.key).unwrap();
    assert_eq!(second.position[0], 2.5 + along(0.02));
    let slowed = along(0.02) * 0.96;
    let h = (slowed * slowed + 0.0 * 0.0).sqrt();
    let pushed = slowed + slowed / h * 0.06;
    assert_eq!(second.motion[0].to_bits(), pushed.to_bits());
    idle(&mut region);
    let third = record(&region, resting.key).unwrap();
    assert_eq!(third.position[0], 2.5 + along(0.02) + along(pushed));
    let slowed = along(pushed) * 0.96;
    let h = (slowed * slowed + 0.0 * 0.0).sqrt();
    assert_eq!(
        third.motion[0].to_bits(),
        (slowed + slowed / h * 0.06).to_bits()
    );
    // It goes on gaining towards the top speed, and never past 0.4 a tick.
    let mut last = third.position[0];
    for _ in 0..12 {
        idle(&mut region);
        let now = record(&region, resting.key).unwrap();
        assert!(now.position[0] - last <= 0.4 + 1e-12);
        assert!(now.position[0] > last);
        last = now.position[0];
    }
}

/// An unpowered powered rail halves a cart's speed before each move and stops it once it
/// is under 0.03 a tick.
#[test]
fn an_unpowered_powered_rail_brakes_a_cart_to_a_stop() {
    let mut blocks = Vec::new();
    for x in 2..=12 {
        blocks.push((BlockPos::new(x, -60, 8), powered_rail("east_west", false)));
    }
    let mut region = ticking_with(&blocks);
    let moving = cart(1, [3.5, FLAT, 8.5], [0.3, 0.0, 0.0]);
    lay(&mut region, &moving);
    idle(&mut region);
    let first = record(&region, moving.key).unwrap();
    let h = (0.3_f64 * 0.3 + 0.0 * 0.0).sqrt();
    let braked = h * 2.0 / 2.0 * 0.5;
    assert_eq!(first.position[0], 3.5 + braked);
    assert_eq!(first.motion[0].to_bits(), (braked * 0.96).to_bits());
    let mut stopped_at = None;
    for tick in 2..=10 {
        idle(&mut region);
        let now = record(&region, moving.key).unwrap();
        if now.motion[0] == 0.0 && stopped_at.is_none() {
            stopped_at = Some((tick, now.position));
        }
    }
    let (tick, at) = stopped_at.expect("it stops");
    assert!(tick <= 5, "{tick}");
    for _ in 0..5 {
        idle(&mut region);
        assert_eq!(
            bits(record(&region, moving.key).unwrap().position),
            bits(at)
        );
    }
}

/// A powered activator rail lets a plain cart's riders off, and the cart rocks with its
/// damage set to 50, once each time its wobble has run down; an unpowered one does
/// nothing.
#[test]
fn a_powered_activator_rail_ejects_a_carts_rider_and_rocks_the_cart() {
    let mut region = ticking_with(&[
        (BlockPos::new(6, -60, 8), activator_rail(true)),
        (BlockPos::new(6, -61, 8), blocks::REDSTONE_BLOCK),
        (BlockPos::new(6, -60, 12), activator_rail(false)),
    ]);
    let stand = |n: u64, position: [f64; 3]| Entity {
        kind: entity_types::ARMOR_STAND as u16,
        data: EntityData::ArmorStand {
            flags: 0,
            disabled_slots: 0,
            pose: Box::new([[Float(0.0); 3]; 6]),
            equipment: Vec::new(),
            health: Float(20.0),
            last_hit: None,
            air: 300,
            frozen: 0,
        },
        ..cart(n, position, [0.0; 3])
    };
    let mut riders = Vec::new();
    for (n, z) in [(1, 8.5), (3, 12.5)] {
        let mut vehicle = cart(n, [6.5, FLAT, z], [0.0; 3]);
        let mut rider = stand(n + 1, [6.5, FLAT, z]);
        rider.riding.vehicle = Some(vehicle.key);
        vehicle.riding.passengers.push(Seat {
            key: rider.key,
            player: false,
            pending: None,
        });
        lay(&mut region, &rider);
        lay(&mut region, &vehicle);
        riders.push((vehicle.key, rider.key));
    }
    let shaken = idle(&mut region);
    let (powered_cart, ejected) = riders[0];
    assert_eq!(record(&region, ejected).unwrap().riding.vehicle, None);
    let EntityData::Minecart { hurt, .. } = record(&region, powered_cart).unwrap().data else {
        unreachable!()
    };
    assert_eq!(
        hurt,
        Hurt {
            time: 10,
            direction: -1,
            damage: Float(50.0)
        }
    );
    assert!(shaken.events.iter().any(|event| matches!(
        event,
        RegionEvent::EntityTracked(report)
            if report.entity == powered_cart && report.flags & crate::api::Tracked::SYNC_VELOCITY != 0
    )));
    // The next tick its wobble falls and nothing more happens until it has run down.
    idle(&mut region);
    let EntityData::Minecart { hurt, .. } = record(&region, powered_cart).unwrap().data else {
        unreachable!()
    };
    assert_eq!((hurt.time, hurt.damage), (9, Float(49.0)));
    // The cart on the unpowered rail keeps its rider and is whole.
    let (quiet_cart, kept) = riders[1];
    assert_eq!(
        record(&region, kept).unwrap().riding.vehicle,
        Some(quiet_cart)
    );
    let EntityData::Minecart { hurt, .. } = record(&region, quiet_cart).unwrap().data else {
        unreachable!()
    };
    assert_eq!(hurt, Hurt::default());
}

/// A moving cart running into one at rest stops against its box and pushes it: the two
/// meet end to end along the resting cart's yaw and take the mean of their motions, the
/// one at rest going on the way the other came.
#[test]
fn a_cart_running_into_another_pushes_it_along() {
    let mut blocks = Vec::new();
    for x in 2..=14 {
        blocks.push((BlockPos::new(x, -60, 8), rail("east_west")));
    }
    let mut region = ticking_with(&blocks);
    let running = cart(1, [3.5, FLAT, 8.5], [0.3, 0.0, 0.0]);
    let resting = cart(2, [6.5, FLAT, 8.5], [0.0; 3]);
    lay(&mut region, &running);
    lay(&mut region, &resting);
    let mut pushed = false;
    for _ in 0..20 {
        idle(&mut region);
        let other = record(&region, resting.key).unwrap();
        pushed |= other.position[0] > 6.5;
        let first = record(&region, running.key).unwrap();
        // The running cart never goes into the other's box.
        assert!(
            first.position[0] + 0.49 <= other.position[0] - 0.49 + 1e-9,
            "{first:?} {other:?}"
        );
    }
    assert!(pushed, "the cart at rest was pushed along");
}

/// A plain empty cart moving at a tenth a tick or more picks up a boat it meets, which
/// rides it from then on; it never picks up what is not pushable (an item).
#[test]
fn a_moving_empty_cart_picks_up_a_boat_it_meets() {
    let mut blocks = Vec::new();
    for x in 2..=14 {
        blocks.push((BlockPos::new(x, -60, 8), rail("east_west")));
    }
    let mut region = ticking_with(&blocks);
    let running = cart(1, [3.5, FLAT, 8.5], [0.15, 0.0, 0.0]);
    lay(&mut region, &running);
    let boat = Entity {
        kind: entity_types::OAK_BOAT as u16,
        data: EntityData::Boat {
            hurt: Hurt::default(),
            paddles: [false; 2],
            bubble_time: 0,
            out_of_control: Float(0.0),
            turning: Float(0.0),
            status: [0; 2],
            contents: Vec::new(),
        },
        ..cart(2, [5.3, -60.0, 8.5], [0.0; 3])
    };
    lay(&mut region, &boat);
    for _ in 0..6 {
        idle(&mut region);
    }
    assert_eq!(
        record(&region, boat.key).unwrap().riding.vehicle,
        Some(running.key)
    );
    assert_eq!(
        record(&region, running.key).unwrap().riding.passengers[0].key,
        boat.key
    );
}

/// A furnace cart takes a piece of coal for 3600 ticks, pushed away from the player who
/// gave it by as far as they stood; it burns a tick of fuel each tick, rolls at half a
/// plain cart's top speed, and takes no more than 32000 ticks of fuel.
#[test]
fn a_furnace_cart_takes_coal_and_pushes_away_from_the_player() {
    let mut blocks = Vec::new();
    for x in 2..=40 {
        blocks.push((BlockPos::new(x, -60, 8), rail("east_west")));
    }
    let mut region = ticking_with(&blocks);
    let furnace = cart_of(
        1,
        entity_types::FURNACE_MINECART,
        [4.5, FLAT, 8.5],
        [0.0; 3],
    );
    lay(&mut region, &furnace);
    let coal = ItemStack {
        item: items::COAL,
        count: 1,
    };
    let fuel = |region: &mut Region, from: [f64; 3]| {
        let mut output = TickOutput::default();
        let entity = record(region, furnace.key).unwrap();
        let by = EntityUse::Interact {
            player: EntityId(1),
            creative: true,
            held: Some(coal),
            location: [Double(0.0); 3],
            sneaking: false,
            from: from.map(Double),
            edge: None,
        };
        region.minecart_used(&entity, ChunkPos::new(0, 0), by, &mut output);
    };
    fuel(&mut region, [1.5, -60.0, 8.5]);
    let EntityData::Minecart { fuel: f, push, .. } = record(&region, furnace.key).unwrap().data
    else {
        unreachable!()
    };
    assert_eq!(f, 3600);
    assert_eq!(push, [Double(3.0), Double(0.0)]);

    idle(&mut region);
    let first = record(&region, furnace.key).unwrap();
    // At rest the push becomes the motion (0.8 of nothing and the push, then the plain
    // slowdown of an empty cart), and it has not moved yet.
    assert_eq!(first.position[0], 4.5);
    assert_eq!(
        first.motion[0].to_bits(),
        ((0.0_f64 * 0.8 + 3.0) * 0.96).to_bits()
    );
    let EntityData::Minecart { fuel: f, .. } = first.data else {
        unreachable!()
    };
    assert_eq!(f, 3599);
    // Then it rolls at its top speed, 0.4 halved.
    idle(&mut region);
    assert_eq!(record(&region, furnace.key).unwrap().position[0], 4.5 + 0.2);
    idle(&mut region);
    assert_eq!(
        record(&region, furnace.key).unwrap().position[0],
        4.5 + 0.2 + 0.2
    );
    // Eight pieces in all (with the one that has burned three ticks) are the most.
    for _ in 0..7 {
        fuel(&mut region, [1.5, -60.0, 8.5]);
    }
    fuel(&mut region, [1.5, -60.0, 8.5]);
    let EntityData::Minecart { fuel: f, .. } = record(&region, furnace.key).unwrap().data else {
        unreachable!()
    };
    assert_eq!(f, 3597 + 7 * 3600);
    // Anything but coal or charcoal fuels nothing.
    let mut output = TickOutput::default();
    let entity = record(&region, furnace.key).unwrap();
    let by = EntityUse::Interact {
        player: EntityId(1),
        creative: true,
        held: Some(ItemStack {
            item: items::DIRT,
            count: 1,
        }),
        location: [Double(0.0); 3],
        sneaking: false,
        from: [Double(0.0); 3],
        edge: None,
    };
    region.minecart_used(&entity, ChunkPos::new(0, 0), by, &mut output);
    assert!(output.entity_changes.is_empty());
}

/// A chest cart rolls according to what it holds: an empty one keeps 0.98 + 15
/// thousandths of its speed each tick, a full one 0.98, in floats; the full one is soon
/// behind.
#[test]
fn a_full_chest_cart_rolls_less_far_than_an_empty_one() {
    let mut blocks = Vec::new();
    for x in 2..=20 {
        blocks.push((BlockPos::new(x, -60, 8), rail("east_west")));
        blocks.push((BlockPos::new(x, -60, 11), rail("east_west")));
    }
    let mut region = ticking_with(&blocks);
    let empty = cart_of(
        1,
        entity_types::CHEST_MINECART,
        [3.5, FLAT, 8.5],
        [0.3, 0.0, 0.0],
    );
    let full = Entity {
        data: cart_data(
            (0..27)
                .map(|slot| {
                    (
                        slot,
                        ItemStack {
                            item: items::DIRT,
                            count: 64,
                        },
                    )
                })
                .collect(),
        ),
        ..cart_of(
            2,
            entity_types::CHEST_MINECART,
            [3.5, FLAT, 11.5],
            [0.3, 0.0, 0.0],
        )
    };
    lay(&mut region, &empty);
    lay(&mut region, &full);
    idle(&mut region);
    let h = (0.3_f64 * 0.3 + 0.0 * 0.0).sqrt();
    let empty_factor = f64::from(0.98_f32 + 15.0_f32 * 0.001_f32);
    let full_factor = f64::from(0.98_f32 + 0.0_f32 * 0.001_f32);
    assert_eq!(
        record(&region, empty.key).unwrap().motion[0].to_bits(),
        (h * 2.0 / 2.0 * empty_factor).to_bits()
    );
    assert_eq!(
        record(&region, full.key).unwrap().motion[0].to_bits(),
        (h * 2.0 / 2.0 * full_factor).to_bits()
    );
    for _ in 0..20 {
        idle(&mut region);
    }
    let gone = |key| record(&region, key).unwrap().position[0];
    assert!(
        gone(full.key) < gone(empty.key) - 0.1,
        "{} {}",
        gone(full.key),
        gone(empty.key)
    );
}

/// A detector rail is pressed by a cart on it: powered, with a comparator beside it
/// reading the fill of the chest cart there; it looks again every 20 ticks, and goes
/// once the cart is gone, here broken by a creative player's hit from another region,
/// which spills what the cart held and drops no cart.
#[test]
fn a_detector_rail_signals_a_cart_and_a_comparator_reads_its_chest() {
    let detector = BlockPos::new(6, -60, 8);
    let comparator = BlockPos::new(6, -60, 9);
    let mut region = ticking_with(&[
        (BlockPos::new(5, -60, 8), rail("east_west")),
        (detector, detector_rail(false)),
        (BlockPos::new(7, -60, 8), rail("east_west")),
        (
            comparator,
            state("minecraft:comparator[facing=north,mode=compare,powered=false]"),
        ),
    ]);
    let stack = ItemStack {
        item: items::DIRT,
        count: 64,
    };
    let chest = Entity {
        data: cart_data(vec![(0, stack), (1, stack)]),
        ..cart_of(1, entity_types::CHEST_MINECART, [6.5, FLAT, 8.5], [0.0; 3])
    };
    lay(&mut region, &chest);
    idle(&mut region);
    assert_eq!(block_at(&region, detector), Some(detector_rail(true)));
    // It is a source of 15 around it, and powers the block below it directly.
    assert_eq!(clustine_blocks::signal::own_signal(detector_rail(true)), 15);
    let direct = |region: &mut Region, direction| {
        clustine_blocks::signal::block_direct_signal(
            &mut crate::region::updates::RegionView(region),
            detector_rail(true),
            detector,
            direction,
            clustine_blocks::signal::Query::ALL,
        )
    };
    assert_eq!(direct(&mut region, clustine_data::Face::Up), 15);
    assert_eq!(direct(&mut region, clustine_data::Face::North), 0);
    for _ in 0..3 {
        idle(&mut region);
    }
    // Two full stacks of 27 slots: `floor(2/27 * 14) + 1`, in floats.
    let share = (1.0_f32 + 1.0) / 27.0;
    let signal = (share * 14.0).floor() as u8 + 1;
    assert_eq!(signal, 2);
    assert_eq!(region.comparator_output(comparator), signal);
    // Still there 20 ticks on, it stays pressed.
    for _ in 0..25 {
        idle(&mut region);
    }
    assert_eq!(block_at(&region, detector), Some(detector_rail(true)));

    let hit = TickInputs {
        effects: vec![FiledEffect {
            id: EffectId {
                sequence: 1,
                index: 0,
            },
            effect: Effect {
                at: crate::physics::containing(chest.position),
                what: Effected::OnEntity {
                    key: chest.key,
                    by: EntityUse::Attack {
                        attacker: EntityId(9),
                        creative: true,
                        sprinting: false,
                        damage: Float(1.0),
                        from: [Double(30.5), Double(-60.0), Double(8.5)],
                    },
                },
            },
            due: Due::FromTaking,
        }],
        ..TickInputs::default()
    };
    region.tick(&hit);
    assert!(record(&region, chest.key).is_none());
    // The two stacks spill in pieces, and no chest cart is dropped.
    let spilled: Vec<ItemStack> = region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter_map(|entity| match entity.data {
            EntityData::Item { stack, .. } => Some(stack),
            _ => None,
        })
        .collect();
    assert_eq!(spilled.iter().map(|stack| stack.count).sum::<i32>(), 128);
    assert!(spilled.iter().all(|stack| stack.item == items::DIRT));
    for _ in 0..25 {
        idle(&mut region);
    }
    assert_eq!(block_at(&region, detector), Some(detector_rail(false)));
    assert_eq!(region.comparator_output(comparator), 0);
}

/// A survival hit of more than 40 breaks a cart, which drops its own item where it is;
/// a smaller one only makes it wobble.
#[test]
fn a_hard_hit_breaks_a_cart_into_its_item() {
    let mut region = ticking_with(&[(BlockPos::new(6, -60, 8), rail("east_west"))]);
    let furnace = cart_of(
        1,
        entity_types::FURNACE_MINECART,
        [6.5, FLAT, 8.5],
        [0.0; 3],
    );
    lay(&mut region, &furnace);
    let mut output = TickOutput::default();
    let hit = |damage: f32| EntityUse::Attack {
        attacker: EntityId(9),
        creative: false,
        sprinting: false,
        damage: Float(damage),
        from: [Double(0.0); 3],
    };
    let entity = record(&region, furnace.key).unwrap();
    region.minecart_used(&entity, ChunkPos::new(0, 0), hit(4.0), &mut output);
    let EntityData::Minecart { hurt, .. } = record(&region, furnace.key).unwrap().data else {
        unreachable!()
    };
    assert_eq!(hurt.damage, Float(40.0), "exactly 40 does not break it");
    let entity = record(&region, furnace.key).unwrap();
    region.minecart_used(&entity, ChunkPos::new(0, 0), hit(1.0), &mut output);
    assert!(record(&region, furnace.key).is_none());
    let dropped: Vec<ItemStack> = region
        .chunks
        .values()
        .flat_map(|chunk| chunk.entities().iter())
        .filter_map(|entity| match entity.data {
            EntityData::Item { stack, .. } => Some(stack),
            _ => None,
        })
        .collect();
    assert_eq!(
        dropped,
        [ItemStack {
            item: items::FURNACE_MINECART,
            count: 1
        }]
    );
}

/// A minecart item used on a rail places a cart of its kind at the rail's middle and a
/// sixteenth up, half a block higher on a slope, facing yaw 0; used on anything else it
/// places nothing. A cart placed ticks in the tick it is placed, as in the game, where
/// the player's click comes before the entities: one on a slope has rolled its first
/// tick by the tick's end.
#[test]
fn a_minecart_is_placed_on_rails_and_not_off_them() {
    let flat = BlockPos::new(1, -60, 2);
    let slope = BlockPos::new(3, -60, 2);
    let mut region = ticking_with(&[(flat, rail("east_west")), (slope, rail("ascending_east"))]);
    let carts = |region: &Region| -> Vec<Entity> {
        let mut found: Vec<Entity> = region
            .chunks
            .values()
            .flat_map(|chunk| chunk.entities().iter())
            .filter(|entity| matches!(entity.data, EntityData::Minecart { .. }))
            .cloned()
            .collect();
        found.sort_by(|a, b| a.position[0].total_cmp(&b.position[0]));
        found
    };
    region.tick(&holding(items::MINECART));
    region.tick(&click(BlockPos::new(2, -61, 2)));
    assert!(carts(&region).is_empty(), "nothing on stone");
    region.tick(&click(flat));
    let placed = carts(&region);
    assert_eq!(placed.len(), 1);
    assert_eq!(bits(placed[0].position), bits([1.5, FLAT, 2.5]));
    assert_eq!(i32::from(placed[0].kind), entity_types::MINECART);
    assert_eq!(placed[0].yaw, 0.0);
    // The creative player keeps the item.
    let held = region.players[&player(1)].held();
    assert_eq!(held.map(|stack| stack.item), Some(items::MINECART));

    region.tick(&holding(items::HOPPER_MINECART));
    region.tick(&click(slope));
    let placed = carts(&region);
    assert_eq!(placed.len(), 2);
    let on_slope = &placed[1];
    assert_eq!(i32::from(on_slope.kind), entity_types::HOPPER_MINECART);
    // Placed at (3.5, -59.4375, 2.5), it rolled 1/128 west in the tick it was placed.
    let x = 3.5 + -0.0078125;
    let y = (-60.0 + 0.0625 - 0.5) + 1.0 * (x - 3.0) + 0.5;
    assert_eq!(bits(on_slope.position), bits([x, y, 2.5]));
}

/// A player standing in a cart's box pushes it away (`AbstractMinecart.push(player)`),
/// in the player's own tick.
#[test]
fn a_player_standing_in_a_cart_pushes_it_away() {
    let mut region = ticking_with(&[(BlockPos::new(1, -60, 0), rail("east_west"))]);
    // The player stands at (0.5, -60, 0.5): the cart just east of them.
    let near = cart(1, [1.2, FLAT, 0.5], [0.0; 3]);
    lay(&mut region, &near);
    idle(&mut region);
    let pushed = record(&region, near.key).unwrap();
    // `push`: dx = 0.5 - 1.2 = -0.7 along x alone, of length one, by 1/0.7 capped at 1,
    // a tenth (the float) and halved; the cart gets the opposite, +x.
    let d = ((0.5_f64 - 1.2) * (0.5 - 1.2)).sqrt();
    let dx = (0.5 - 1.2) / d * 1.0 * 0.100_000_001_490_116_12 * 0.5;
    assert_eq!(pushed.motion[0].to_bits(), (0.0 + -dx).to_bits());
}

#[test]
fn a_player_who_uses_a_cart_rides_it_and_walking_forwards_starts_it_on_its_way() {
    let rails: Vec<(BlockPos, BlockState)> = (2..12)
        .map(|x| (BlockPos::new(x, -60, 4), rail("east_west")))
        .collect();
    let mut region = ticking_with(&rails);
    let ridden = cart(1, [3.5, FLAT, 4.5], [0.0; 3]);
    lay(&mut region, &ridden);
    let used = moves(vec![numbered(
        1,
        PlayerInput::Interact {
            entity: ridden.key,
            location: [0.0, 0.3, 0.0],
            sneaking: false,
        },
    )]);
    checked(&mut region, &used);
    let seated = region.players[&player(1)].pose.position;
    let on = record(&region, ridden.key).unwrap();
    // The cart's passenger point is 0.1875 up, the player's vehicle point 0.6.
    assert_eq!(
        [seated.x, seated.y, seated.z].map(f64::to_bits),
        [
            on.position[0],
            (on.position[1] + 0.1875) - 0.6,
            on.position[2]
        ]
        .map(f64::to_bits)
    );
    // Facing east (yaw -90) and holding forward: the rider's push, the keys' intent
    // times 0.001 while the cart is slower than 0.1 a tick.
    let walk = moves(vec![
        numbered(
            1,
            PlayerInput::Move {
                position: None,
                rotation: Some((-90.0, 0.0)),
                on_ground: true,
            },
        ),
        numbered(
            1,
            PlayerInput::Keys(crate::api::Keys {
                forward: true,
                ..Default::default()
            }),
        ),
    ]);
    checked(&mut region, &walk);
    let moving = record(&region, ridden.key).unwrap();
    assert!(moving.motion[0] > 0.0, "{:?}", moving.motion);
    assert!(moving.position[0] > on.position[0]);
}

/// A motion as a client is sent it: the game's `LpVec3`, three 15-bit fractions of a
/// shared whole scale (the largest component rounded up), read back as the codec reads
/// them (`clustine_protocol::codec`). What the official server's comparisons record is
/// this, not the doubles.
fn on_the_wire(motion: [f64; 3]) -> [f64; 3] {
    let largest = motion.iter().fold(0.0_f64, |most, c| most.max(c.abs()));
    if largest < 3.051_944_088_384_301e-5 {
        return [0.0; 3];
    }
    let scale = largest.ceil();
    motion.map(|component| {
        let packed = ((component / scale * 0.5 + 0.5) * 32766.0).round();
        (packed * 2.0 / 32766.0 - 1.0) * scale
    })
}

/// How far the cart is from `start`, in the 4096ths of a block a move is sent in.
fn moved_in_4096ths(position: [f64; 3], start: [f64; 3]) -> [i64; 3] {
    let steps = |value: f64| (value * 4096.0).round() as i64;
    [0, 1, 2].map(|axis| steps(position[axis]) - steps(start[axis]))
}

/// The official server's run of the slope in `minecarts_oracle.rs` (2026-10-18): its
/// cart gained some 0.06 a tick where the slope gives 0.008, and was sent every tick
/// from the second to the sixth, where Clustine's was sent every third. The slope's own
/// rules were not what differed. The builder hovered three blocks above the cart's
/// place, at (20.5, 4.5), without flying; the official server moves a player in the
/// player's own tick (`Player.getMoveSimulationType` is `AUTHORITATIVE_SIDE_AND_SERVER`,
/// so `LivingEntity.aiStep` travels it, gravity and all) before
/// `LivingEntity.pushEntities`, and puts it back where its client said
/// (`tickPlayer`'s `absSnapTo`) after; so the builder's box met the cart's there, and
/// only there: the cart's own `pushAndPickupEntities` found nobody. Each such tick gave
/// the cart one `AbstractMinecart.push(player)` from (20.5, 4.5), from the second tick,
/// when the cart is first a hundredth of a block away, a full 0.05 west (closer than a
/// block it is not scaled down), and `needsSync`, which made the tick an update.
///
/// The same slope, curve and run, with that push alone given to the cart after each of
/// its ticks while the player's box (at the cart's starting place) meets it, gives what
/// the official server sent, to the bit.
#[test]
fn a_cart_pushed_off_a_hovering_players_place_as_it_rolls_down_a_slope_is_what_the_official_server_sent()
 {
    let mut blocks = vec![
        (BlockPos::new(8, -60, 8), rail("ascending_east")),
        (BlockPos::new(9, -60, 8), blocks::STONE),
        (BlockPos::new(9, -59, 8), rail("east_west")),
        (BlockPos::new(7, -60, 8), rail("south_east")),
    ];
    blocks.extend((9..=16).map(|z| (BlockPos::new(7, -60, z), rail("north_south"))));
    let mut region = ticking_with(&blocks);
    let start = on_slope(8, 8);
    let rolling = cart(1, start, [0.0; 3]);
    lay(&mut region, &rolling);
    let builder = [8.5, -60.0, 8.5];
    let mut seen = Vec::new();
    for tick in 1..=15 {
        idle(&mut region);
        // The builder's push in their own tick, while a player's box (0.6 wide) about
        // their place meets the cart's (0.98).
        let at = record(&region, rolling.key).unwrap().position;
        let meets = (at[0] - builder[0]).abs() < 0.79 && (at[2] - builder[2]).abs() < 0.79;
        let mut output = TickOutput::default();
        if meets {
            region.push_minecart(rolling.key, builder, None, &mut output);
        }
        let synced = output.events.iter().any(|event| {
            matches!(event, RegionEvent::EntityTracked(report)
                if report.entity == rolling.key && report.flags & Tracked::NEEDS_SYNC != 0)
        });
        let now = record(&region, rolling.key).unwrap();
        seen.push((
            tick,
            on_the_wire(now.motion),
            moved_in_4096ths(now.position, start),
            synced,
        ));
    }
    // The official server's packets: each motion as sent, and each move added up from
    // the spawn.
    let official: [(i64, [f64; 3], [i64; 3]); 8] = [
        (2, [-0.06586095342733322, 0.0, 0.0], [-96, -96, 0]),
        (3, [-0.12439724104254413, 0.0, 0.0], [-398, -398, 0]),
        (4, [-0.18354391747543186, 0.0, 0.0], [-940, -940, 0]),
        (5, [-0.24323994384422876, 0.0, 0.0], [-1723, -1723, 0]),
        (6, [-0.2950009155832265, 0.0, 0.0], [-2752, -2048, 0]),
        (9, [0.0, 0.0, 0.26100225843862535], [-4862, -2048, 2814]),
        (12, [0.0, 0.0, 0.2309100897271561], [-4096, -2048, 5895]),
        (15, [0.0, 0.0, 0.20429713727644505], [-4096, -2048, 8620]),
    ];
    for (tick, motion, moved) in official {
        let (_, ours, our_move, _) = seen[usize::try_from(tick - 1).unwrap()];
        assert_eq!(bits(ours), bits(motion), "tick {tick}: {seen:?}");
        assert_eq!(our_move, moved, "tick {tick}: {seen:?}");
    }
    // Pushed, and so sent at once, on the ticks the official server sent every tick.
    let pushed: Vec<i64> = seen
        .iter()
        .filter(|(_, _, _, synced)| *synced)
        .map(|(tick, _, _, _)| *tick)
        .collect();
    assert_eq!(pushed, [2, 3, 4, 5, 6]);
}

/// A player who really stands where the cart rolls pushes it more than that: once the
/// cart runs at a tenth of a block a tick, its own `pushAndPickupEntities` finds them in
/// its box (grown by 0.2 across) and pushes them away, `Entity.push(cart)`, which pushes
/// the cart too, by the push of `Entity.push` (a twentieth of the way, over the root of
/// the larger distance), on top of the player's own push of it.
#[test]
fn a_player_standing_where_a_cart_rolls_down_a_slope_pushes_it_from_both_ticks() {
    let mut blocks = vec![
        (BlockPos::new(0, -60, 0), rail("ascending_east")),
        (BlockPos::new(1, -60, 0), blocks::STONE),
        (BlockPos::new(-1, -60, 0), rail("south_east")),
    ];
    blocks.extend((1..=8).map(|z| (BlockPos::new(-1, -60, z), rail("north_south"))));
    let mut region = ticking_with(&blocks);
    // Player 1 stands at the spawn, (0.5, -60, 0.5): in the cart's box.
    let rolling = cart(1, on_slope(0, 0), [0.0; 3]);
    lay(&mut region, &rolling);
    let mut motions = Vec::new();
    for _ in 1..=4 {
        idle(&mut region);
        motions.push(record(&region, rolling.key).unwrap());
    }
    // Ticks 2 and 3 as the official server's: the cart runs under a tenth a tick, and
    // only the player's own push counts.
    assert_eq!(
        on_the_wire(motions[1].motion),
        [-0.06586095342733322, 0.0, 0.0]
    );
    assert_eq!(
        on_the_wire(motions[2].motion),
        [-0.12439724104254413, 0.0, 0.0]
    );
    // Tick 4: the slope's tick from tick 3's motion, the cart's push of the player, and
    // the player's push of the cart.
    let third = &motions[2];
    let fourth = &motions[3];
    let mut v = third.motion[0] + -0.0078125;
    let moved = v;
    v *= 0.96;
    let h = v.abs();
    v *= (h + (third.position[1] - fourth.position[1]) * 0.05) / h;
    let x = third.position[0] + moved;
    let dx = x - 0.5;
    let d = dx.abs().sqrt();
    let by_the_cart = dx / d * 1.0 * 0.050_000_000_745_058_06;
    let by_the_player = 1.0 * 0.100_000_001_490_116_12 * 0.5;
    assert_eq!(fourth.position[0].to_bits(), x.to_bits());
    assert_eq!(
        fourth.motion[0].to_bits(),
        (v + by_the_cart + -by_the_player).to_bits()
    );
}

/// Without the player there, the same cart gains what the slope gives, as Clustine sent
/// it in the same run: a cart let go alone is not pushed.
#[test]
fn a_cart_let_go_on_a_slope_alone_gains_an_eighth_of_a_sixteenth_a_tick() {
    let mut blocks = vec![
        (BlockPos::new(8, -60, 8), rail("ascending_east")),
        (BlockPos::new(9, -60, 8), blocks::STONE),
        (BlockPos::new(7, -60, 8), rail("south_east")),
    ];
    blocks.extend((9..=16).map(|z| (BlockPos::new(7, -60, z), rail("north_south"))));
    let mut region = ticking_with(&blocks);
    let start = on_slope(8, 8);
    let rolling = cart(1, start, [0.0; 3]);
    lay(&mut region, &rolling);
    let mut motions = Vec::new();
    for _ in 1..=6 {
        let output = idle(&mut region);
        assert!(!output.events.iter().any(|event| {
            matches!(event, RegionEvent::EntityTracked(report)
                if report.entity == rolling.key && report.flags & Tracked::NEEDS_SYNC != 0)
        }));
        motions.push(on_the_wire(record(&region, rolling.key).unwrap().motion));
    }
    // Clustine's packets of ticks 3 and 6.
    assert_eq!(motions[2], [-0.023927241652932918, 0.0, 0.0]);
    assert_eq!(motions[5], [-0.04852591100531034, 0.0, 0.0]);
}

/// The official server's run of the driven cart in `minecarts_oracle.rs` (2026-10-18):
/// its rider faced east when they got in, but their client's confirmation of the
/// teleport onto the seat said yaw 0, which the game takes as the rider's look
/// (`handleAcceptTeleportPacket` → `handlePlayerPositionChange`, which for a passenger
/// takes only the rotations), so `getLastClientMoveIntent` turned the forward key south,
/// (0, 0, 1). `moveAlongTrack` adds a thousandth of it after turning the speed onto the
/// east-west rail, and turns the whole of it onto the rail the next tick: the cart goes
/// east by the length of what it had, and south by a thousandth again. A rider of yaw 0
/// gives what the official server sent, to the bit; one facing east (yaw -90) what
/// Clustine sent.
#[test]
fn a_rider_looking_south_on_an_east_west_rail_drives_the_cart_as_the_official_server_had_it() {
    let drive = |yaw: f32| -> Vec<([f64; 3], [i64; 3])> {
        let rails: Vec<(BlockPos, BlockState)> = (2..12)
            .map(|x| (BlockPos::new(x, -60, 4), rail("east_west")))
            .collect();
        let mut region = ticking_with(&rails);
        let start = [3.5, FLAT, 4.5];
        let ridden = cart(1, start, [0.0; 3]);
        lay(&mut region, &ridden);
        let used = moves(vec![numbered(
            1,
            PlayerInput::Interact {
                entity: ridden.key,
                location: [0.0, 0.3, 0.0],
                sneaking: false,
            },
        )]);
        checked(&mut region, &used);
        let walk = moves(vec![
            numbered(
                1,
                PlayerInput::Move {
                    position: None,
                    rotation: Some((yaw, 0.0)),
                    on_ground: false,
                },
            ),
            numbered(
                1,
                PlayerInput::Keys(crate::api::Keys {
                    forward: true,
                    ..Default::default()
                }),
            ),
        ]);
        checked(&mut region, &walk);
        let mut seen = vec![];
        for _ in 0..12 {
            let now = record(&region, ridden.key).unwrap();
            seen.push((
                on_the_wire(now.motion),
                moved_in_4096ths(now.position, start),
            ));
            idle(&mut region);
        }
        seen
    };
    // The official server's motions of ticks 3, 6, 9 and 12 after the rider was shown
    // seated, its move of tick 6, and each the cart's tick 2, 5, 8 and 11 of being driven.
    let south = drive(0.0);
    let thousandth = 0.000_976_622_108_282_976_2;
    assert_eq!(south[1].0, [thousandth, 0.0, thousandth]);
    assert_eq!(
        south[4],
        ([0.001_953_244_216_565_952_5, 0.0, thousandth], [19, 0, 3])
    );
    assert_eq!(south[7].0, [0.002_624_671_916_010_568, 0.0, thousandth]);
    assert_eq!(south[10].0, [0.003_112_982_970_151_945, 0.0, thousandth]);
    // Facing east: Clustine's motions of the same ticks, and its move of tick 6.
    let east = drive(-90.0);
    assert_eq!(east[1].0, [0.002_014_283_098_333_624_6, 0.0, 0.0]);
    assert_eq!(east[4], ([0.004_944_149_423_182_553, 0.0, 0.0], [46, 0, 0]));
    assert_eq!(east[7].0, [0.007_874_015_748_031_482, 0.0, 0.0]);
}
