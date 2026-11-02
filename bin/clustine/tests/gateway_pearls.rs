//! An ender pearl thrown into one of the End's gateways, end to end, written from
//! `docs/groundwork/p-plan.md` section 19.8 and the game's `EndGatewayBlock`,
//! `TheEndGatewayBlockEntity`, `ThrownEnderpearl`, `ServerPlayer` and `PlayerList`
//! (`/home/claude/reference/d-javap`), by someone who did not write the code.
//!
//! **What the game does.** A gateway's cage leaves no room for a body, so a player in
//! survival reaches the outer islands by throwing an ender pearl into it:
//! - `EndGatewayBlock.entityInside` (EndGatewayBlock.txt, `entityInside` 2-61): any
//!   entity that `canUsePortal(false)`, the gateway not cooling down, is set inside the
//!   portal and the gateway's cooldown is triggered (`triggerCooldown`, whose block
//!   event 1 is the beam the clients draw). A pearl's `onInsideBlock` of a gateway also
//!   tells its owner (ThrownEnderpearl.txt 522-542), for the advancement.
//! - `EndGatewayBlock.getPortalDestination` (EndGatewayBlock.txt 44-71): for a
//!   `ThrownEnderpearl` the transition is to the gateway's portal position with
//!   `Vec3.ZERO` velocity, yaw and pitch nought and **no** relatives (`Set.of()`): the
//!   pearl comes out with its velocity cleared; any other entity keeps its velocity and
//!   look (`DELTA ∪ ROTATION`, 72-109).
//! - The pearl falls from there and hits the ground (`ThrownEnderpearl.onHit`,
//!   137-317): its owner, a player in the same level who is alive and not asleep
//!   (`isAllowedToTeleportOwner`, 320-349) and whose connection accepts messages, is
//!   teleported to the pearl's `oldPosition()` with `Vec3.ZERO` and `Relative.ROTATION`
//!   (258-264: the look kept, the velocity cleared), and hurt by 5 of `enderPearl`
//!   damage (276-279), which a creative player does not take; then the pearl is gone.
//! - A pearl belongs to its owner (`registerToCurrentOwner`, 61-74; `ServerPlayer
//!   .registerEnderPearl`). When the owner leaves, `PlayerList.remove` saves the player
//!   with their pearls (`ServerPlayer.saveEnderPearls`, ServerPlayer.txt 681-714:
//!   `ender_pearls`, each with its `ender_pearl_dimension`) and removes each pearl from
//!   the world with `UNLOADED_WITH_PLAYER` (PlayerList.txt, `remove` 69-107). When the
//!   owner joins again the pearls are put back into their levels
//!   (`loadAndSpawnEnderPearls`, ServerPlayer.txt 726-790, from `PrepareSpawnTask
//!   .Ready`), and fly on. So a pearl thrown into a gateway by a player who leaves
//!   before it gets there neither goes through nor lands while they are away; it does
//!   both when they are back, and carries them.
//!
//! **The scenes** are `gateways.rs`'s, on its world with a gateway at [`GATEWAY`]
//! (`dimensions::a_world_with_a_gateway`), and its working out of where the exit is
//! from what the bot is shown (copied here, since `common/dimensions.rs` is not this
//! file's to change). The bot does not go down into the gateway: it hovers beside the
//! cage, level with the gateway, and throws a pearl into it horizontally, from a place
//! worked out from the pearl's flight (`ThrowableProjectile.tick`: gravity 0.03, then
//! the drag of 0.99, then the move) so that the pearl is in the gateway's block, on its
//! layer of the cage, which is open on every side, at the end of a tick. The bots are in
//! creative mode, which takes no pearl's damage.
//!
//! **Found** while these scenes were written (the scenes are kept, ignored, with what
//! they found):
//! - A pearl thrown into the return gateway goes in (the bot sees the return gateway's
//!   beam and the pearl removed), but its thrower is never carried back beside the
//!   first gateway when no player is near the first gateway; with a second bot on the
//!   End's platform all the while, the same trip carries the thrower back as the game
//!   does. In the game the pearl's travel loads the chunk where it comes out
//!   (`TeleportTransition.PLACE_PORTAL_TICKET`, EndGatewayBlock.txt 65) and the pearl
//!   keeps the chunks about it loaded (`registerAndUpdateEnderPearlTicket`,
//!   ThrownEnderpearl.txt 352-451), so it lands and carries its owner wherever it is.
//! - A pearl whose thrower leaves while it flies is not kept with them: a bot on the
//!   platform sees the player taken off the list, then, 11 ticks later, the gateway's
//!   beam as the pearl goes in, then the pearl removed; and when the thrower joins again
//!   no pearl is shown to them and nothing carries them.

mod common;
#[path = "common/dimensions.rs"]
#[allow(dead_code)]
mod dimensions;

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};
use clustine_botswarm::Bot;
use clustine_data::{BlockState, blocks, entity_types, items};
use clustine_protocol::chunk::SectionData;
use clustine_protocol::nbt::Nbt;
use clustine_protocol::packets::play::{BlockEntity, ClientboundPlay, relative};
use tokio::time::Instant;

use dimensions::{
    END_CRUISE, GATEWAY, GatewayWorld, PATIENCE, THE_END, acknowledged, arrive, describe, drain,
    feet, fly_over_the_island, gateway_cage, is, is_air, join, round_trip, take, teleports, wait,
};

/// How far out the search for an exit begins, and how far each of its steps goes
/// (`findExitPortalXZPosTentative`).
const REACH: f64 = 1024.0;
const STRIDE: f64 = 16.0;
const STEPS: i32 = 16;

/// The End's lowest block and its highest (`getMinY`, `getMaxY`).
const END_MIN_Y: i32 = 0;
const END_MAX_Y: i32 = 255;

/// A player's eyes above their feet, standing (`Player.STANDING_DIMENSIONS`' eye height,
/// a float), and how far below them a thrown item starts (`ThrowableItemProjectile`).
const EYES: f64 = 1.620_000_004_768_371_6;
const BELOW_THE_EYES: f64 = 0.100_000_001_490_116_12;

/// A player's speed of throw (`EnderpearlItem.use`: `shootFromRotation(.., 1.5, 1.0)`),
/// a pearl's gravity (`ThrowableProjectile.getDefaultGravity`) and drag in air (0.99, a
/// float).
const POWER: f64 = 1.5;
const GRAVITY: f64 = 0.03;
const DRAG: f64 = 0.990_000_009_536_743_2;

/// Half a pearl's width and its height (`EntityType.ENDER_PEARL`, 0.25 × 0.25).
const PEARL_HALF: f64 = 0.125;
const PEARL_HEIGHT: f64 = 0.25;

/// Half a player's width and their height.
const PLAYER_HALF: f64 = 0.3;
const PLAYER_HEIGHT: f64 = 1.8;

/// Ticks of flight of a pearl thrown from beside a cage: two, so that the pearl is in
/// the gateway's block at the end of its second tick, before the uncertainty of the
/// throw has grown.
const NEAR: u32 = 2;

/// Ticks of flight of a pearl whose thrower leaves while it flies: long enough for the
/// leave to reach the server before the pearl reaches the gateway.
const FAR: u32 = 12;

/// Fails with what the bot was sent.
fn fail(bot: &Bot, what: impl std::fmt::Display) -> ! {
    panic!("{what}\n{}", bot.report())
}

/// Has `name` join the world and come into the End by its open portal, shown the
/// gateway; fails with what it was sent if it does not.
async fn arrive_in(world: &GatewayWorld, name: &str, others: &mut [Bot]) -> Bot {
    arrive(world, name, others)
        .await
        .unwrap_or_else(|error| panic!("{name} did not come to the gateway: {error:#}"))
}

/// Whether the beam's block event of the gateway at `gateway` is among `packets`:
/// `triggerCooldown`'s `blockEvent(pos, end_gateway, 1, 0)`.
fn beam_seen(packets: &[(Instant, ClientboundPlay)], gateway: At) -> bool {
    let block = blocks::END_GATEWAY
        .block_id()
        .expect("the gateway is a block");
    packets.iter().any(|(_, packet)| match packet {
        ClientboundPlay::BlockEvent(event) => {
            (event.position.x, event.position.y, event.position.z) == gateway
                && usize::try_from(event.block).ok() == Some(block)
                && (event.action, event.param) == (1, 0)
        }
        _ => false,
    })
}

/// Checks what the travel sent: no `Respawn` (a gateway changes no level, and the
/// pearl's teleport of its owner is in the level).
fn assert_no_respawn(bot: &Bot, packets: &[(Instant, ClientboundPlay)]) {
    if let Some((_, respawn)) = packets
        .iter()
        .find(|(_, packet)| matches!(packet, ClientboundPlay::Respawn(_)))
    {
        fail(bot, format!("a gateway changes no level, but: {respawn:?}"));
    }
    assert_eq!(bot.info.login.spawn.dimension_name, THE_END);
}

// ---------------------------------------------------------------------------------
// The throw.
// ---------------------------------------------------------------------------------

/// A block's place.
type At = (i32, i32, i32);

/// How far a pearl thrown level at a player's speed goes across, and how far it falls,
/// in `ticks` ticks of `ThrowableProjectile.tick`: gravity, then drag, then the move.
fn flight(ticks: u32) -> (f64, f64) {
    let (mut across, mut up) = (POWER, 0.0);
    let (mut run, mut rise) = (0.0, 0.0);
    for _ in 0..ticks {
        up -= GRAVITY;
        across *= DRAG;
        up *= DRAG;
        run += across;
        rise += up;
    }
    (run, rise)
}

/// Where a bot throws a pearl into a gateway from: where it hovers and the yaw it looks
/// along, level.
#[derive(Debug, Clone, Copy)]
struct Spot {
    feet: (f64, f64, f64),
    yaw: f32,
}

impl Spot {
    /// The spot on `side` of the gateway at `gateway` from which a pearl thrown level is,
    /// after `ticks` ticks, in the middle of the gateway's block across and its box a
    /// little below the block's middle in height.
    fn beside(gateway: At, side: (i32, i32), ticks: u32) -> Spot {
        let (run, rise) = flight(ticks);
        let (gx, gy, gz) = gateway;
        let pearl_y = f64::from(gy) + 0.5 - PEARL_HEIGHT / 2.0 - rise;
        let feet = (
            f64::from(gx) + 0.5 + run * f64::from(side.0),
            pearl_y + BELOW_THE_EYES - EYES,
            f64::from(gz) + 0.5 + run * f64::from(side.1),
        );
        // Looking back along the side, towards the gateway: `-sin(yaw)`, `cos(yaw)`.
        let yaw = match side {
            (1, 0) => 90.0,
            (-1, 0) => -90.0,
            (0, 1) => 180.0,
            _ => 0.0,
        };
        Spot { feet, yaw }
    }

    /// Where the pearl starts.
    fn pearl(self) -> (f64, f64, f64) {
        (
            self.feet.0,
            self.feet.1 + EYES - BELOW_THE_EYES,
            self.feet.2,
        )
    }
}

/// Whether every block a box of half-width `half` about `x`, `z` covers from `low` to
/// `high` is air as the bot is shown it.
fn box_clear(bot: &Bot, (x, z): (f64, f64), half: f64, low: f64, high: f64) -> bool {
    let xs = ((x - half).floor() as i32)..=((x + half).floor() as i32);
    let zs = ((z - half).floor() as i32)..=((z + half).floor() as i32);
    let ys = (low.floor() as i32)..=(high.floor() as i32);
    xs.into_iter().all(|bx| {
        zs.clone().all(|bz| {
            ys.clone()
                .all(|by| is_air(bot.block_at(bx, by, bz).ok().flatten()))
        })
    })
}

/// Whether nothing but the gateway is in the way of the pearl from `spot` into the
/// gateway at `gateway`, nor in the way of the bot hovering there.
fn spot_is_clear(bot: &Bot, spot: Spot, gateway: At) -> bool {
    let (x, y, z) = spot.feet;
    if !box_clear(bot, (x, z), PLAYER_HALF, y, y + PLAYER_HEIGHT) {
        return false;
    }
    let from = spot.pearl();
    let to = (
        f64::from(gateway.0) + 0.5,
        f64::from(gateway.1) + 0.5 - PEARL_HEIGHT / 2.0,
        f64::from(gateway.2) + 0.5,
    );
    (0..=100).all(|step| {
        let t = f64::from(step) / 100.0;
        let (px, py, pz) = (
            from.0 + (to.0 - from.0) * t,
            from.1 + (to.1 - from.1) * t,
            from.2 + (to.2 - from.2) * t,
        );
        let xs = ((px - PEARL_HALF).floor() as i32)..=((px + PEARL_HALF).floor() as i32);
        let zs = ((pz - PEARL_HALF).floor() as i32)..=((pz + PEARL_HALF).floor() as i32);
        let ys = (py.floor() as i32)..=((py + PEARL_HEIGHT).floor() as i32);
        xs.into_iter().all(|bx| {
            zs.clone().all(|bz| {
                ys.clone().all(|by| {
                    let state = bot.block_at(bx, by, bz).ok().flatten();
                    is_air(state) || (bx, by, bz) == gateway && is(state, blocks::END_GATEWAY)
                })
            })
        })
    })
}

/// The first spot beside the gateway at `gateway`, east, south, north or west of it,
/// from which a pearl of `ticks` ticks' flight has a clear way in, and above which the
/// column is clear up to `top`.
fn a_clear_spot(bot: &Bot, gateway: At, ticks: u32, top: f64) -> Result<Spot> {
    for side in [(1, 0), (0, 1), (0, -1), (-1, 0)] {
        let spot = Spot::beside(gateway, side, ticks);
        let (x, y, z) = spot.feet;
        if spot_is_clear(bot, spot, gateway) && box_clear(bot, (x, z), PLAYER_HALF, y, top) {
            return Ok(spot);
        }
    }
    bail!(
        "no side of the gateway at {gateway:?} has a clear way for a pearl of {ticks} ticks \
         and a clear column above it up to {top}: {}",
        dimensions::column_report(bot, gateway.0 + 3, gateway.2, gateway.1 - 3, gateway.1 + 3)
    )
}

/// From over the spot's column at its height `cruise`, down to the spot.
async fn down_to(bot: &mut Bot, others: &mut [Bot], spot: Spot) -> Result<()> {
    let look = (spot.yaw, 0.0);
    let (x, _, z) = spot.feet;
    let here = bot.location;
    ensure!(
        (here.0 - x).abs() < 1.0e-6 && (here.2 - z).abs() < 1.0e-6,
        "the bot is not over the spot {spot:?}: {here:?}"
    );
    bot.fly_to(spot.feet, look, 0.5).await?;
    drain(others).await
}

/// Throws an ender pearl, looking level along the spot's yaw, from where the bot
/// hovers: the look settled and a tick gone by first, so that the server knows the bot
/// as still (a throw adds its thrower's movement to the pearl's); waits until the throw
/// is acknowledged.
async fn throw_a_pearl(bot: &mut Bot, others: &mut [Bot], spot: Spot) -> Result<()> {
    let look = (spot.yaw, 0.0);
    bot.fly_to(spot.feet, look, 0.5).await?;
    round_trip(bot, others, teleports(bot)).await?;
    take(bot, items::ENDER_PEARL).await?;
    let sequence = bot.use_item(look.0, look.1).await?;
    acknowledged(bot, others, sequence).await
}

/// What a bot was sent from just before its throw until it stood where its pearl
/// carried it, among the chunks of its view.
struct Carried {
    packets: Vec<(Instant, ClientboundPlay)>,
    placing: clustine_protocol::packets::play::SynchronizePlayerPosition,
}

/// Waits until the bot is put more than 16 blocks from `gateway`, since `before` (its
/// last teleport then), and then until it holds the chunks of its view.
async fn carried_away(
    bot: &mut Bot,
    others: &mut [Bot],
    gateway: At,
    before: Option<i32>,
) -> Result<Carried> {
    let far = |bot: &Bot| {
        let (x, _, z) = bot.location;
        let (gx, _, gz) = gateway;
        teleports(bot) != before
            && ((x - f64::from(gx)).powi(2) + (z - f64::from(gz)).powi(2)).sqrt() > 16.0
    };
    if let Err(error) = wait(
        bot,
        others,
        PATIENCE,
        "the pearl's teleport of its owner",
        far,
    )
    .await
    {
        let beam = bot
            .packets
            .as_deref()
            .is_some_and(|packets| beam_seen(packets, gateway));
        let onlookers: Vec<String> = others
            .iter()
            .map(|other| format!("{}: {}", other.info.profile.name, pearls(other)))
            .collect();
        bail!(
            "{error:#}; the beam of the gateway at {gateway:?} seen: {beam}; {}; the others \
             saw {onlookers:?}",
            pearls(bot)
        );
    }
    let (x, _, z) = feet(bot);
    let area = common::view_area((x >> 4, z >> 4), common::VIEW_DISTANCE);
    let held = |bot: &Bot| {
        dimensions::around_is_held(bot) && area.iter().all(|chunk| bot.chunks.contains_key(chunk))
    };
    wait(bot, others, PATIENCE, "the chunks where it came out", held).await?;
    let packets = bot.packets.take().unwrap_or_default();
    bot.record_packets();
    let placing = bot.position.clone().context("placed")?;
    Ok(Carried { packets, placing })
}

/// What the pearl's teleport of its owner must be (`ThrownEnderpearl.onHit` 258-264):
/// `Relative.ROTATION` alone, so that the look is kept and the velocity cleared, with no
/// turn.
fn assert_a_pearl_s_teleport(bot: &Bot, carried: &Carried) {
    let placing = &carried.placing;
    assert_eq!(
        placing.relative_flags,
        relative::ROTATION,
        "the pearl's teleport keeps the look and nothing else: {placing:?}"
    );
    assert_eq!(
        (placing.velocity_x, placing.velocity_y, placing.velocity_z),
        (0.0, 0.0, 0.0),
        "the pearl's teleport clears the velocity"
    );
    assert_eq!((placing.yaw, placing.pitch), (0.0, 0.0), "no turn");
    // Creative: the pearl's 5 of damage is not taken.
    if let Some(health) = &bot.health {
        assert!(
            health.health >= 20.0,
            "a creative player takes no pearl's damage: {health:?}"
        );
    }
}

// ---------------------------------------------------------------------------------
// The world as the bot is shown it, and the game's search over it (`gateways.rs`).
// ---------------------------------------------------------------------------------

/// The blocks of the chunks a bot holds, with the changes it was told of since, and
/// places taken to be air (what a cage or an island was made over).
struct View {
    chunks: BTreeMap<(i32, i32), Vec<SectionData>>,
    changed: BTreeMap<At, i32>,
    hidden: BTreeSet<At>,
}

impl View {
    fn of(bot: &Bot) -> Result<View> {
        let mut chunks = BTreeMap::new();
        let mut changed = BTreeMap::new();
        for &position in bot.chunks.keys() {
            let sections = bot.sections(position)?.context("held")?;
            chunks.insert(position, sections);
            changed.extend(bot.changed_blocks(position));
        }
        Ok(View {
            chunks,
            changed,
            hidden: BTreeSet::new(),
        })
    }

    fn holds(&self, chunk: (i32, i32)) -> bool {
        self.chunks.contains_key(&chunk)
    }

    /// The block at `(x, y, z)`: air outside the End's height and where it is hidden.
    fn state(&self, (x, y, z): At) -> Result<BlockState> {
        if !(END_MIN_Y..=END_MAX_Y).contains(&y) || self.hidden.contains(&(x, y, z)) {
            return Ok(blocks::AIR);
        }
        let chunk = (x >> 4, z >> 4);
        let Some(sections) = self.chunks.get(&chunk) else {
            bail!(
                "the search reads ({x}, {y}, {z}), in chunk {chunk:?}, which the bot does not hold"
            );
        };
        let id = match self.changed.get(&(x, y, z)) {
            Some(state) => *state,
            None => {
                let above = y - END_MIN_Y;
                let index = (above as usize % 16) << 8 | (z as usize & 15) << 4 | (x as usize & 15);
                sections[above as usize / 16].blocks.get(index)
            }
        };
        Ok(BlockState(u16::try_from(id).context("a block state")?))
    }

    fn is_air(&self, at: At) -> Result<bool> {
        let state = self.state(at)?;
        Ok([blocks::AIR, blocks::CAVE_AIR, blocks::VOID_AIR]
            .into_iter()
            .any(|air| state.block_id() == air.block_id()))
    }

    /// The bottom of the highest section of `chunk` that has a block that is not air,
    /// or `None` if it has none (`getHighestFilledSectionIndex() == -1`).
    fn highest_filled_section(&self, chunk: (i32, i32)) -> Result<Option<i32>> {
        let (cx, cz) = chunk;
        for bottom in (END_MIN_Y..=END_MAX_Y)
            .step_by(16)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            for y in bottom..bottom + 16 {
                for x in 0..16 {
                    for z in 0..16 {
                        if !self.is_air((cx * 16 + x, y, cz * 16 + z))? {
                            return Ok(Some(bottom));
                        }
                    }
                }
            }
        }
        Ok(None)
    }

    fn is_empty(&self, chunk: (i32, i32)) -> Result<bool> {
        Ok(self.highest_filled_section(chunk)?.is_none())
    }

    fn is_full(&self, at: At) -> Result<bool> {
        Ok(self.state(at)?.props().collision_is_full_block())
    }
}

/// The chunk of a tentative point.
fn chunk_of(point: (f64, f64)) -> (i32, i32) {
    (
        (point.0 / 16.0).floor() as i32,
        (point.1 / 16.0).floor() as i32,
    )
}

/// The direction of the gateway at `from` from the middle: `new Vec3(x, 0, z)
/// .normalize()`.
fn direction_of(from: At) -> (f64, f64) {
    let (x, z) = (f64::from(from.0), f64::from(from.2));
    let length = (x * x + z * z).sqrt();
    if length < 1.0e-5 {
        (0.0, 0.0)
    } else {
        (x / length, z / length)
    }
}

/// `findExitPortalXZPosTentative`.
fn tentative(view: &View, from: At) -> Result<(f64, f64)> {
    let direction = direction_of(from);
    let mut point = (direction.0 * REACH, direction.1 * REACH);
    let mut steps = STEPS;
    while !view.is_empty(chunk_of(point))? && steps > 0 {
        steps -= 1;
        point = (
            point.0 - direction.0 * STRIDE,
            point.1 - direction.1 * STRIDE,
        );
    }
    let mut steps = STEPS;
    while view.is_empty(chunk_of(point))? && steps > 0 {
        steps -= 1;
        point = (
            point.0 + direction.0 * STRIDE,
            point.1 + direction.1 * STRIDE,
        );
    }
    Ok(point)
}

/// `findValidSpawnInChunk`, as `gateways.rs` reads it: the range's far corner where the
/// chunk has end stone with two blocks above it that are not full.
fn valid_spawn(view: &View, chunk: (i32, i32)) -> Result<Option<At>> {
    let Some(highest) = view.highest_filled_section(chunk)? else {
        return Ok(None);
    };
    let (cx, cz) = chunk;
    for z in cz * 16..cz * 16 + 16 {
        for y in 30..=highest + 15 {
            for x in cx * 16..cx * 16 + 16 {
                let at = (x, y, z);
                if !is(Some(i32::from(view.state(at)?.0)), blocks::END_STONE)
                    || view.is_full((x, y + 1, z))?
                    || view.is_full((x, y + 2, z))?
                {
                    continue;
                }
                return Ok(Some((cx * 16 + 15, (highest + 15).max(30), cz * 16 + 15)));
            }
        }
    }
    Ok(None)
}

/// `findTallestBlock`.
fn tallest(view: &View, at: At, radius: i32, allow_bedrock: bool) -> Result<At> {
    let mut found: Option<At> = None;
    for dx in -radius..=radius {
        for dz in -radius..=radius {
            if (dx, dz) == (0, 0) && !allow_bedrock {
                continue;
            }
            let floor = found.map_or(END_MIN_Y, |(_, y, _)| y);
            let mut y = END_MAX_Y;
            while y > floor {
                let here = (at.0 + dx, y, at.2 + dz);
                let state = view.state(here)?;
                if state.props().collision_is_full_block()
                    && (allow_bedrock || !is(Some(i32::from(state.0)), blocks::BEDROCK))
                {
                    found = Some(here);
                    break;
                }
                y -= 1;
            }
        }
    }
    Ok(found.unwrap_or(at))
}

/// `java.util.Random`, which `RandomSource.create(seed)` is.
struct JavaRandom(i64);

impl JavaRandom {
    const MULTIPLIER: i64 = 0x5_DEEC_E66D;
    const MASK: i64 = (1 << 48) - 1;

    fn new(seed: i64) -> Self {
        Self((seed ^ Self::MULTIPLIER) & Self::MASK)
    }

    fn next(&mut self, bits: u32) -> i32 {
        self.0 = self.0.wrapping_mul(Self::MULTIPLIER).wrapping_add(0xB) & Self::MASK;
        (self.0 >> (48 - bits)) as i32
    }

    fn next_int(&mut self, bound: i32) -> i32 {
        if bound & -bound == bound {
            return ((i64::from(bound) * i64::from(self.next(31))) >> 31) as i32;
        }
        loop {
            let bits = self.next(31);
            let value = bits % bound;
            if bits.wrapping_sub(value).wrapping_add(bound - 1) >= 0 {
                return value;
            }
        }
    }
}

/// `BlockPos.asLong`.
fn as_long((x, y, z): At) -> i64 {
    ((i64::from(x) & 0x3FF_FFFF) << 38)
        | ((i64::from(z) & 0x3FF_FFFF) << 12)
        | (i64::from(y) & 0xFFF)
}

/// The places `end_island` sets to end stone at `origin` with
/// `RandomSource.create(origin.asLong())`.
fn island_cells(origin: At) -> Vec<At> {
    let mut random = JavaRandom::new(as_long(origin));
    let mut radius = random.next_int(3) as f32 + 4.0;
    let mut cells = Vec::new();
    let mut y = 0;
    while radius > 0.5 {
        let reach = (radius + 1.0) * (radius + 1.0);
        for x in (-radius).floor() as i32..=radius.ceil() as i32 {
            for z in (-radius).floor() as i32..=radius.ceil() as i32 {
                if (x * x + z * z) as f32 <= reach {
                    cells.push((origin.0 + x, origin.1 + y, origin.2 + z));
                }
            }
        }
        radius -= random.next_int(2) as f32 + 0.5;
        y -= 1;
    }
    cells
}

/// Where the specification says the gateway at `from` has its exit, from what `view`
/// shows with the return gateway's cage hidden; and the island, if one was made.
fn expected_exit(view: &mut View, from: At) -> Result<(At, Option<At>)> {
    let direction = direction_of(from);
    let mut island = None;
    for step in -STEPS..=STEPS {
        let along = REACH + STRIDE * f64::from(step);
        let point = (direction.0 * along, direction.1 * along);
        let origin = (
            (point.0 + 0.5).floor() as i32,
            75,
            (point.1 + 0.5).floor() as i32,
        );
        let cells = island_cells(origin);
        if !cells
            .iter()
            .all(|cell| view.holds((cell.0 >> 4, cell.2 >> 4)))
        {
            continue;
        }
        let mut made = true;
        for &cell in &cells {
            if !is(Some(i32::from(view.state(cell)?.0)), blocks::END_STONE) {
                made = false;
                break;
            }
        }
        if !made {
            continue;
        }
        let kept = view.hidden.clone();
        view.hidden.extend(cells.iter().copied());
        let without = tentative(view, from);
        let spawn = match &without {
            Ok(found) => valid_spawn(view, chunk_of(*found)),
            Err(_) => Ok(None),
        };
        view.hidden = kept;
        if let (Ok(found), Ok(None)) = (without, spawn)
            && chunk_of(found) == chunk_of(point)
        {
            island = Some(origin);
            break;
        }
    }
    let origin = match island {
        Some(origin) => origin,
        None => {
            let point = tentative(view, from)?;
            valid_spawn(view, chunk_of(point))?.with_context(|| {
                format!(
                    "no end stone to stand on in the tentative chunk {:?}, and no island \
                     there either",
                    chunk_of(point)
                )
            })?
        }
    };
    let top = tallest(view, origin, 16, true)?;
    Ok(((top.0, top.1 + 10, top.2), island))
}

/// Where something through a gateway whose exit is `exit`, not exact, is put
/// (`findExitPosition`): on the tallest block, bedrock refused, within 5 of two above
/// the exit, the exit's own column skipped; its bottom centre. A pearl put there with
/// its velocity cleared falls onto that block at once, and its owner is put where it
/// was as that tick began: here.
fn expected_place(view: &View, exit: At) -> Result<(f64, f64, f64)> {
    let (x, y, z) = tallest(view, (exit.0, exit.1 + 2, exit.2), 5, false)?;
    Ok((f64::from(x) + 0.5, f64::from(y + 1), f64::from(z) + 0.5))
}

/// The gateways the bot is shown within `radius` of where it is, across.
fn gateways_near(view: &View, bot: &Bot, radius: i32) -> Result<Vec<At>> {
    let (bx, _, bz) = feet(bot);
    let mut found = Vec::new();
    for x in bx - radius..=bx + radius {
        for z in bz - radius..=bz + radius {
            if !view.holds((x >> 4, z >> 4)) {
                continue;
            }
            for y in END_MIN_Y..=END_MAX_Y {
                if is(
                    Some(i32::from(view.state((x, y, z))?.0)),
                    blocks::END_GATEWAY,
                ) {
                    found.push((x, y, z));
                }
            }
        }
    }
    Ok(found)
}

/// The cage about the gateway at `at` as the bot is shown it, where it is not the
/// feature's.
fn cage_differences(view: &View, at: At) -> Result<Vec<String>> {
    let mut differences = Vec::new();
    for dx in -1..=1 {
        for dy in -2..=2 {
            for dz in -1..=1 {
                let here = (at.0 + dx, at.1 + dy, at.2 + dz);
                let shown = view.state(here)?;
                let wanted = gateway_cage(dx, dy, dz);
                if shown.block_id() != wanted.block_id() {
                    differences.push(format!(
                        "{here:?}: {} where the cage has {}",
                        describe(i32::from(shown.0)),
                        describe(i32::from(wanted.0))
                    ));
                }
            }
        }
    }
    Ok(differences)
}

/// The gateway's block entity at `gateway` as the chunk the bot was sent of it has it.
fn gateway_entity(bot: &Bot, gateway: At) -> Option<&BlockEntity> {
    let (x, y, z) = gateway;
    let chunk = bot.chunks.get(&(x >> 4, z >> 4))?;
    let packed = u8::try_from((x & 15) << 4 | (z & 15)).ok()?;
    chunk
        .block_entities
        .iter()
        .find(|entity| entity.packed_xz == packed && i32::from(entity.y) == y)
}

/// The exit written into the gateway's block entity, as the chunk the bot was sent of
/// it says (`exit_portal`).
fn written_exit(bot: &Bot, gateway: At) -> Option<At> {
    let entity = gateway_entity(bot, gateway)?;
    let Some(Nbt::Compound(entries)) = &entity.data else {
        return None;
    };
    entries
        .iter()
        .find_map(|(name, value)| match (name.as_str(), value) {
            ("exit_portal", Nbt::IntArray(ints)) => match ints[..] {
                [x, y, z] => Some((x, y, z)),
                _ => None,
            },
            _ => None,
        })
}

/// Whether the bot stands on the ground: the block below its feet has a full block's
/// collision and its feet are on that block's top.
fn on_the_ground(bot: &Bot) -> bool {
    let (x, y, z) = bot.location;
    let below = (
        x.floor() as i32,
        (y - 0.001).floor() as i32,
        z.floor() as i32,
    );
    let state = bot.block_at(below.0, below.1, below.2).ok().flatten();
    let full = state
        .and_then(|state| u16::try_from(state).ok())
        .map(BlockState)
        .filter(|state| state.block_id().is_some())
        .is_some_and(|state| state.props().collision_is_full_block());
    full && (y - f64::from(below.1 + 1)).abs() < 1.0e-6
}

// ---------------------------------------------------------------------------------
// The scenes.
// ---------------------------------------------------------------------------------

/// What a bot found where its pearl through the first gateway carried it.
struct CameOut {
    carried: Carried,
    /// The return gateway at the exit.
    exit: At,
    view: View,
    /// Where the search puts the exit, and the island, if one was made.
    wanted: At,
    island: Option<At>,
}

/// From the platform, over the island to the spot beside the first gateway and down to
/// it; returns the spot.
async fn beside_the_first_gateway(bot: &mut Bot, others: &mut [Bot], ticks: u32) -> Spot {
    let top = f64::from(END_CRUISE + 2);
    let spot = a_clear_spot(bot, GATEWAY, ticks, top).unwrap_or_else(|error| fail(bot, error));
    let (x, _, z) = spot.feet;
    if let Err(error) = fly_over_the_island(bot, others, (x, z)).await {
        fail(bot, format!("{error:#}"));
    }
    if let Err(error) = down_to(bot, others, spot).await {
        fail(bot, format!("{error:#}"));
    }
    spot
}

/// Checks where the bot came out after its pearl went through the first gateway: on the
/// outer islands, on the ground by the exit; and works out from what it is shown the
/// return gateway and where the search puts the exit.
fn came_out(bot: &Bot, carried: Carried) -> CameOut {
    if bot.location.0 < 900.0 {
        fail(
            bot,
            format!(
                "the pearl did not carry the bot through the gateway at {GATEWAY:?} to the \
                 outer islands: it was put at {:?}",
                bot.location
            ),
        );
    }
    let mut view = View::of(bot).unwrap_or_else(|error| fail(bot, format!("{error:#}")));
    let gateways =
        gateways_near(&view, bot, 16).unwrap_or_else(|error| fail(bot, format!("{error:#}")));
    let [exit] = gateways[..] else {
        fail(
            bot,
            format!(
                "one return gateway is wanted near where the pearl carried the bot, at {:?}; \
                 it is shown {gateways:?}",
                bot.location
            ),
        );
    };
    // The search came before the cage: it is hidden from it.
    let mut search = View {
        chunks: view.chunks.clone(),
        changed: view.changed.clone(),
        hidden: view.hidden.clone(),
    };
    for dx in -1..=1 {
        for dy in -2..=2 {
            for dz in -1..=1 {
                search
                    .hidden
                    .insert((exit.0 + dx, exit.1 + dy, exit.2 + dz));
            }
        }
    }
    let (wanted, island) =
        expected_exit(&mut search, GATEWAY).unwrap_or_else(|error| fail(bot, format!("{error:#}")));
    view.hidden.clear();
    println!(
        "the return gateway is at {exit:?}, the search's exit {wanted:?}, the island \
         {island:?}; the bot at {:?}",
        bot.location
    );
    CameOut {
        carried,
        exit,
        view,
        wanted,
        island,
    }
}

/// Has `bot`, which has arrived in the End, throw a pearl into the first gateway from
/// beside its cage, and checks where it came out.
async fn pearl_through_the_first_gateway(bot: &mut Bot, others: &mut [Bot]) -> CameOut {
    let spot = beside_the_first_gateway(bot, others, NEAR).await;
    let before = teleports(bot);
    bot.packets = Some(Vec::new());
    if let Err(error) = throw_a_pearl(bot, others, spot).await {
        fail(bot, format!("{error:#}"));
    }
    let carried = match carried_away(bot, others, GATEWAY, before).await {
        Ok(carried) => carried,
        Err(error) => fail(bot, format!("{error:#}")),
    };
    came_out(bot, carried)
}

/// Checks that the bot was put where its pearl landed by the exit: on the tallest block
/// within 5 of two above the exit, on the ground.
fn assert_landed_by_the_exit(bot: &Bot, out: &CameOut) {
    let place =
        expected_place(&out.view, out.exit).unwrap_or_else(|error| fail(bot, format!("{error:#}")));
    let (x, y, z) = bot.location;
    if (x - place.0).abs() > 1.0 || (z - place.2).abs() > 1.0 || (y - place.1).abs() > 1.0 {
        fail(
            bot,
            format!(
                "the pearl lands where it comes out, on the tallest block within 5 of two \
                 above the exit {:?}, {place:?}; the bot was put at {:?}",
                out.exit, bot.location
            ),
        );
    }
    if !on_the_ground(bot) {
        fail(
            bot,
            format!(
                "the pearl lands on the ground, and the bot with it; the bot is at {:?}: {}",
                bot.location,
                dimensions::column_report(
                    bot,
                    x.floor() as i32,
                    z.floor() as i32,
                    y as i32 - 5,
                    y as i32 + 2
                )
            ),
        );
    }
}

/// 19.8, `EndGatewayBlock.getPortalDestination` and `ThrownEnderpearl.onHit`: a bot
/// beside the gateway's cage throws a pearl into it; the pearl goes through and comes
/// out by the exit on the outer islands with its velocity cleared, falls onto the
/// tallest block within 5 of two above the exit, and the bot is put there, on the
/// ground, with its look kept and no `Respawn`; the bot sees the gateway's beam as the
/// pearl goes in.
#[tokio::test]
async fn a_pearl_thrown_into_a_gateway_carries_its_thrower_to_the_outer_islands() {
    let world = dimensions::a_world_with_a_gateway().await;
    let mut bot = arrive_in(&world, "Pitcher", &mut []).await;
    let out = pearl_through_the_first_gateway(&mut bot, &mut []).await;
    assert_landed_by_the_exit(&bot, &out);
    if !beam_seen(&out.carried.packets, GATEWAY) {
        fail(
            &bot,
            format!("no BlockEvent(1, 0) of the end gateway at {GATEWAY:?} when the pearl went in"),
        );
    }
    assert_no_respawn(&bot, &out.carried.packets);
    assert_a_pearl_s_teleport(&bot, &out.carried);
    world.server.stop().await;
}

/// 19.8 and its review, item 2: the return gateway is made at the exit by a pearl's
/// travel as by a player's, in its cage where the game's search puts the exit, the only
/// gateway there, with the first gateway as its exit (`knownExit(from, false)`).
#[tokio::test]
async fn a_pearl_s_travel_makes_the_return_gateway_at_the_exit() {
    let world = dimensions::a_world_with_a_gateway().await;
    let mut bot = arrive_in(&world, "Mason", &mut []).await;
    let out = pearl_through_the_first_gateway(&mut bot, &mut []).await;
    let differences = cage_differences(&out.view, out.exit)
        .unwrap_or_else(|error| fail(&bot, format!("{error:#}")));
    assert!(
        differences.is_empty(),
        "the return gateway's cage at {:?}: {differences:#?}",
        out.exit
    );
    assert_eq!(
        out.exit, out.wanted,
        "the return gateway stands where the search puts the exit (an island at {:?})",
        out.island
    );
    assert_eq!(
        written_exit(&bot, out.exit),
        Some(GATEWAY),
        "the return gateway's exit is the first gateway; its block entity is {:?}",
        gateway_entity(&bot, out.exit)
    );
    world.server.stop().await;
}

/// Moves the bot along its height to the nearest column out from under the cage of the
/// gateway at `cage` whose column is clear up to `top` and to which the way is clear
/// (`gateways.rs`' `step_aside`).
async fn step_aside(bot: &mut Bot, others: &mut [Bot], cage: At, top: i32) -> Result<()> {
    let (fx, fy, fz) = feet(bot);
    if (fx - cage.0).abs() > 1 || (fz - cage.2).abs() > 1 {
        return Ok(());
    }
    let (x, y, z) = bot.location;
    let clear = |at: At| is_air(bot.block_at(at.0, at.1, at.2).ok().flatten());
    let mut candidates = Vec::new();
    for dx in -4..=4_i32 {
        for dz in -4..=4_i32 {
            let column = (fx + dx, fz + dz);
            if (column.0 - cage.0).abs() <= 1 && (column.1 - cage.2).abs() <= 1 {
                continue;
            }
            candidates.push((dx * dx + dz * dz, column));
        }
    }
    candidates.sort();
    for (_, (cx, cz)) in candidates {
        if !dimensions::column_is_clear(bot, cx, cz, fy, top) {
            continue;
        }
        let to = (f64::from(cx) + 0.5, y, f64::from(cz) + 0.5);
        let way_is_clear = (0..=40).all(|step| {
            let t = f64::from(step) / 40.0;
            let (px, pz) = (x + (to.0 - x) * t, z + (to.2 - z) * t);
            [(-0.3, -0.3), (-0.3, 0.3), (0.3, -0.3), (0.3, 0.3)]
                .into_iter()
                .all(|(ox, oz)| {
                    let (bx, bz) = ((px + ox).floor() as i32, (pz + oz).floor() as i32);
                    clear((bx, fy, bz)) && clear((bx, fy + 1, bz))
                })
        });
        if way_is_clear {
            let look = bot.look;
            bot.fly_to(to, look, 0.25).await?;
            return drain(others).await;
        }
    }
    bail!(
        "no clear column near {:?} out from under the cage at {cage:?}",
        bot.location
    )
}

/// From where the bot stands by the return gateway at `exit`: out from under its cage,
/// up above it, across over the spot beside it and down to the spot.
async fn beside_the_return_gateway(bot: &mut Bot, others: &mut [Bot], exit: At) -> Result<Spot> {
    let cruise = f64::from(exit.1 + 4);
    let spot = a_clear_spot(bot, exit, NEAR, cruise + 2.0)?;
    step_aside(bot, others, exit, exit.1 + 6).await?;
    let (x, y, z) = bot.location;
    ensure!(
        box_clear(bot, (x, z), PLAYER_HALF, y, cruise + 2.0),
        "the column over the bot is not clear: {}",
        dimensions::column_report(
            bot,
            x.floor() as i32,
            z.floor() as i32,
            y as i32,
            exit.1 + 6
        )
    );
    let look = bot.look;
    bot.fly_to((x, cruise, z), look, 1.0).await?;
    drain(others).await?;
    bot.fly_to((spot.feet.0, cruise, spot.feet.2), look, 1.0)
        .await?;
    drain(others).await?;
    down_to(bot, others, spot).await?;
    Ok(spot)
}

/// The return gateway's exit is the first gateway, not exact: a pearl thrown into it
/// comes out on the tallest block within 5 of two above the first gateway, and its
/// thrower is put there, beside the first gateway; the return gateway's beam is seen,
/// and there is no `Respawn`.
#[tokio::test]
#[ignore = "finding: the pearl goes into the return gateway (its beam is seen) but its thrower is never carried back where nobody is by the first gateway; with a player there it is"]
async fn a_pearl_thrown_into_the_return_gateway_brings_its_thrower_back_beside_the_first() {
    let world = dimensions::a_world_with_a_gateway().await;
    let mut bot = arrive_in(&world, "Boomerang", &mut []).await;
    there_and_back(&mut bot, &mut []).await;
    world.server.stop().await;
}

/// As above, with a second bot on the End's platform by the first gateway all the
/// while, so that the place where the pearl comes out on its way back is held by a
/// region with a player in it: what tells whether the return trip fails for the
/// pearl's way back as such or for where it comes out being held by nobody.
#[tokio::test]
async fn a_pearl_thrown_into_the_return_gateway_with_someone_by_the_first_brings_its_thrower_back()
{
    let world = dimensions::a_world_with_a_gateway().await;
    let onlooker = arrive_in(&world, "Lookout", &mut []).await;
    let mut others = [onlooker];
    let mut bot = arrive_in(&world, "Yoyo", &mut others).await;
    there_and_back(&mut bot, &mut others).await;
    world.server.stop().await;
}

/// The return trip of `bot`, which has arrived in the End: a pearl through the first
/// gateway, then one through the return gateway, which must bring it back beside
/// the first.
async fn there_and_back(bot: &mut Bot, others: &mut [Bot]) {
    let out = pearl_through_the_first_gateway(bot, others).await;
    let exit = out.exit;
    let spot = match beside_the_return_gateway(bot, others, exit).await {
        Ok(spot) => spot,
        Err(error) => fail(bot, format!("{error:#}")),
    };
    let before = teleports(bot);
    bot.packets = Some(Vec::new());
    if let Err(error) = throw_a_pearl(bot, others, spot).await {
        fail(bot, format!("{error:#}"));
    }
    let back = match carried_away(bot, others, exit, before).await {
        Ok(back) => back,
        Err(error) => fail(bot, format!("{error:#}")),
    };
    let view = View::of(bot).unwrap_or_else(|error| fail(bot, format!("{error:#}")));
    let place =
        expected_place(&view, GATEWAY).unwrap_or_else(|error| fail(bot, format!("{error:#}")));
    let (x, y, z) = bot.location;
    if (x - place.0).abs() > 1.0 || (z - place.2).abs() > 1.0 || (y - place.1).abs() > 1.0 {
        fail(
            bot,
            format!(
                "the pearl lands on the tallest block within 5 of two above the first gateway, \
                 {place:?}; the bot was put at {:?}",
                bot.location
            ),
        );
    }
    let (gx, _, gz) = GATEWAY;
    assert!(
        (x - f64::from(gx)).abs() <= 6.0 && (z - f64::from(gz)).abs() <= 6.0,
        "beside the first gateway: {:?}",
        bot.location
    );
    if !beam_seen(&back.packets, exit) {
        fail(bot, format!("no beam of the return gateway at {exit:?}"));
    }
    assert_no_respawn(bot, &back.packets);
    assert_a_pearl_s_teleport(bot, &back);
}

/// What the onlooker was sent of the thrower's leave, the pearl and the gateway, in
/// order, with the ticks from the first of them: the player taken off its list, the
/// pearl's removal and the gateway's beam.
fn events_while_away(onlooker: &Bot) -> Vec<(i64, &'static str)> {
    let pearls: Vec<i32> = onlooker
        .vanished
        .iter()
        .chain(onlooker.entities.values())
        .filter(|entity| entity.kind == entity_types::ENDER_PEARL)
        .map(|entity| entity.entity_id)
        .collect();
    let packets = onlooker.packets.as_deref().unwrap_or_default();
    let mut events = Vec::new();
    for (when, packet) in packets {
        let what = match packet {
            ClientboundPlay::PlayerInfoRemove(_) => "the player taken off the list",
            ClientboundPlay::RemoveEntities(removed)
                if removed.entity_ids.iter().any(|id| pearls.contains(id)) =>
            {
                "the pearl removed"
            }
            ClientboundPlay::BlockEvent(_) if beam_seen(&[(*when, packet.clone())], GATEWAY) => {
                "the gateway's beam"
            }
            _ => continue,
        };
        events.push((*when, what));
    }
    let first = events.first().map(|(when, _)| *when);
    events
        .into_iter()
        .map(|(when, what)| {
            (
                first.map_or(0, |first| dimensions::ticks(first, when)),
                what,
            )
        })
        .collect()
}

/// The pearls among what `bot` sees now, and among what it has seen go.
fn pearls(bot: &Bot) -> String {
    let now: Vec<_> = bot
        .entities
        .values()
        .filter(|entity| entity.kind == entity_types::ENDER_PEARL)
        .map(|entity| entity.position)
        .collect();
    let gone: Vec<_> = bot
        .vanished
        .iter()
        .filter(|entity| entity.kind == entity_types::ENDER_PEARL)
        .map(|entity| entity.position)
        .collect();
    format!("pearls seen now at {now:?}, gone at {gone:?}")
}

/// `PlayerList.remove` and `ServerPlayer.loadAndSpawnEnderPearls`: a pearl is kept with
/// its owner. A bot throws a pearl into the gateway from far enough that it flies for
/// twelve ticks, and leaves as soon as the throw is acknowledged; the pearl is taken
/// out of the world with it (a bot on the platform sees it go) and neither goes through
/// nor lands while its owner is away. When the bot joins again the pearl is put back
/// and flies on, through the gateway, and carries the bot to the outer islands, on the
/// ground by the exit, beside the return gateway its travel made.
#[tokio::test]
#[ignore = "finding: a pearl whose thrower leaves stays in the world and flies on into the gateway while they are away (its beam 11 ticks after the leave), and nothing carries them when they join again"]
async fn a_pearl_whose_thrower_leaves_before_it_lands_carries_them_when_they_come_back() {
    let world = dimensions::a_world_with_a_gateway().await;
    let watcher = arrive_in(&world, "Onlooker", &mut []).await;
    let mut others = [watcher];
    let mut bot = arrive_in(&world, "Truant", &mut others).await;
    let spot = beside_the_first_gateway(&mut bot, &mut others, FAR).await;
    let thrown_from = bot.location;
    // What the onlooker is sent from the throw on: whether the gateway took the pearl
    // in while its owner was away (its beam).
    others[0].packets = Some(Vec::new());
    if let Err(error) = throw_a_pearl(&mut bot, &mut others, spot).await {
        fail(&bot, format!("{error:#}"));
    }
    bot.close().await.expect("the bot leaves");
    drop(bot);
    let [watcher] = &mut others;
    // The leave is the server's once the watcher no longer has the player in its list;
    // the pearl went with them.
    let gone = |bot: &Bot| {
        !bot.player_list.values().any(|name| name == "Truant")
            && !bot
                .entities
                .values()
                .any(|entity| entity.kind == entity_types::ENDER_PEARL)
    };
    if let Err(error) = wait(
        watcher,
        &mut [],
        PATIENCE,
        "the leave, and the pearl gone",
        gone,
    )
    .await
    {
        fail(watcher, format!("{error:#}; {}", pearls(watcher)));
    }
    let beam_while_away = watcher
        .packets
        .as_deref()
        .is_some_and(|packets| beam_seen(packets, GATEWAY));
    let away = format!(
        "while the thrower was away the onlooker saw {}, and the gateway's beam: \
         {beam_while_away}; in order: {:?}",
        pearls(watcher),
        events_while_away(watcher)
    );
    println!("{away}");
    let mut bot = match join(&world.address, "Truant", &mut others).await {
        Ok(bot) => bot,
        Err(error) => panic!("the thrower joins again: {error:#}"),
    };
    assert_eq!(bot.info.login.spawn.dimension_name, THE_END);
    bot.record_packets();
    let outer = |bot: &Bot| bot.location.0 > 900.0;
    if let Err(error) = wait(
        &mut bot,
        &mut others,
        PATIENCE,
        "the kept pearl's teleport to the outer islands",
        outer,
    )
    .await
    {
        let [watcher] = &others;
        fail(
            &bot,
            format!(
                "{error:#}: the bot left at {thrown_from:?} with its pearl in the air; {away}; \
                 since, the onlooker saw {}, and the bot since it joined again {}",
                pearls(watcher),
                pearls(&bot)
            ),
        );
    }
    let carried = match carried_away(&mut bot, &mut others, GATEWAY, None).await {
        Ok(carried) => carried,
        Err(error) => fail(&bot, format!("{error:#}")),
    };
    let out = came_out(&bot, carried);
    assert_landed_by_the_exit(&bot, &out);
    assert_eq!(
        out.exit, out.wanted,
        "the return gateway stands where the search puts the exit"
    );
    assert_no_respawn(&bot, &out.carried.packets);
    world.server.stop().await;
}
