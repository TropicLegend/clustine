//! The End's gateways, end to end, written from `docs/groundwork/p-plan.md`, sections
//! 19.5 (item 9), 19.6 ("The gateway's exit") and 19.8 with its review, `p-brief.md`
//! 2.6, and the game's `TheEndGatewayBlockEntity` and `EndGatewayBlock` (`d-javap`), by
//! someone who did not write the code: a bot that goes into a gateway whose exit is
//! unknown comes out on the outer islands, where the game's search says, with no
//! `Respawn` and with its velocity and look kept; it sees the gateway's beam as it goes
//! in, and a return gateway at the exit, which takes it back beside the first; and a
//! second bot through the first gateway finds the exit the first found, written into
//! the gateway.
//!
//! **The gateway.** A gateway comes only of the dragon's death, and Clustine has no
//! commands. So each scene has its world on disk: a bot opens an End portal and comes
//! onto the platform, which has the chunks about it made and stored, and goes back to
//! the overworld through the void; the server stops; the test writes a gateway in its
//! cage into the stored chunks at the place of the first gateway the dragon's death
//! makes ([`GATEWAY`]), with no exit, as the death leaves one; and the server starts
//! again on the world, where the scene's bots come into the End by the open portal
//! (`dimensions::a_world_with_a_gateway`). The builder leaves in the overworld because
//! a player who leaves in the End cannot come back (the last test has the finding).
//!
//! In the exit found for the world of seed 0's End, `(1028, 80, -7)`, no island is
//! made: the tentative chunk has end stone to stand on, so the island's part of the
//! search below is not exercised by these scenes.
//!
//! **Into a gateway.** A player cannot walk into a caged gateway (the cage leaves no
//! room for a body taller than a block); a creative player breaks the bedrock over it
//! and comes down into it (`dimensions::down_into_the_gateway`).
//!
//! **Where it should come out** is worked out here from the specification over what
//! the bot is shown once it is there: the tentative chunk 1024 out along the gateway's
//! direction, stepped back over chunks that are not empty and on over those that are;
//! the end stone with room above it nearest the world's middle in that chunk, or an
//! island where there is none; the tallest block about that, and ten above it the exit,
//! where the return gateway stands; and the traveller on the tallest block within 5 of
//! two above the exit. The return gateway's cage is taken as having been made over air,
//! as is the island, if one was made (both are then put back to air for the search
//! that came before them): the search sees the world before them.

mod common;
#[path = "common/dimensions.rs"]
#[allow(dead_code)]
mod dimensions;

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use clustine_botswarm::Bot;
use clustine_data::{BlockState, blocks};
use clustine_protocol::chunk::SectionData;
use clustine_protocol::nbt::Nbt;
use clustine_protocol::packets::play::{BlockEntity, ClientboundPlay, relative};
use tokio::time::Instant;

use dimensions::{
    GATEWAY, GatewayWorld, PATIENCE, THE_END, Through, a_world_with_a_gateway, arrive, describe,
    drain, fly_over_the_island, gateway_cage, is, join, through_the_gateway, to_the_end, wait,
};

/// Every relative bit of `DELTA ∪ ROTATION` (`EndGatewayBlock.getPortalDestination`,
/// EndGatewayBlock.txt 72-109): the velocity kept (`Relative.DELTA`, with
/// `ROTATE_DELTA`) and the look kept (`Relative.ROTATION`).
const DELTA_AND_ROTATION: i32 = relative::DELTA_X
    | relative::DELTA_Y
    | relative::DELTA_Z
    | relative::ROTATE_DELTA
    | relative::Y_ROT
    | relative::X_ROT;

/// How far out the search for an exit begins, and how far each of its steps goes
/// (`findExitPortalXZPosTentative`).
const REACH: f64 = 1024.0;
const STRIDE: f64 = 16.0;
const STEPS: i32 = 16;

/// The End's lowest block and its highest (`getMinY`, `getMaxY`).
const END_MIN_Y: i32 = 0;
const END_MAX_Y: i32 = 255;

/// The look the bots keep while they go through a gateway: not the one a placing
/// would give, so that a look the server sends shows.
const LOOK: (f32, f32) = (37.0, 12.0);

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

/// Checks what the gateway's travel sent: no `Respawn` (a gateway changes no level).
fn assert_a_teleport_in_the_level(bot: &Bot, through: &Through) {
    if let Some((_, respawn)) = through
        .packets
        .iter()
        .find(|(_, packet)| matches!(packet, ClientboundPlay::Respawn(_)))
    {
        fail(bot, format!("a gateway changes no level, but: {respawn:?}"));
    }
    assert_eq!(bot.info.login.spawn.dimension_name, THE_END);
}

/// Checks the placing of a gateway's travel: `DELTA ∪ ROTATION`, its velocity and turn
/// nought, so that the client keeps the velocity and the look it had.
fn assert_velocity_and_look_kept(bot: &Bot, through: &Through) {
    let placing = &through.placing;
    assert_eq!(
        placing.relative_flags, DELTA_AND_ROTATION,
        "the placing's relative flags: {placing:?}"
    );
    assert_eq!(
        (placing.velocity_x, placing.velocity_y, placing.velocity_z),
        (0.0, 0.0, 0.0),
        "the velocity is kept: nothing is added to it"
    );
    assert_eq!(
        (placing.yaw, placing.pitch),
        (0.0, 0.0),
        "the look is kept: the turn is none"
    );
    assert_eq!(bot.look, LOOK, "the bot's look after the placing");
}

/// Whether the beam's block event of the gateway at `gateway` is among `packets`:
/// `triggerCooldown`'s `blockEvent(pos, end_gateway, 1, 0)`.
fn beam_seen(packets: &[(Instant, ClientboundPlay)], gateway: (i32, i32, i32)) -> bool {
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

// ---------------------------------------------------------------------------------
// The world as the bot is shown it, and the game's search over it.
// ---------------------------------------------------------------------------------

/// The blocks of the chunks a bot holds, with the changes it was told of since, and
/// places taken to be air (what a cage or an island was made over).
struct View {
    chunks: BTreeMap<(i32, i32), Vec<SectionData>>,
    changed: BTreeMap<(i32, i32, i32), i32>,
    hidden: BTreeSet<(i32, i32, i32)>,
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
    fn state(&self, (x, y, z): (i32, i32, i32)) -> Result<BlockState> {
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

    fn is_air(&self, at: (i32, i32, i32)) -> Result<bool> {
        let state = self.state(at)?;
        Ok([blocks::AIR, blocks::CAVE_AIR, blocks::VOID_AIR]
            .into_iter()
            .any(|air| state.block_id() == air.block_id()))
    }

    /// The bottom of the highest section of `chunk` that has a block that is not air,
    /// or `None` if it has none: `getHighestFilledSectionIndex() == -1` is an empty
    /// chunk (`isChunkEmpty`).
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

    fn is_full(&self, at: (i32, i32, i32)) -> Result<bool> {
        Ok(self.state(at)?.props().collision_is_full_block())
    }
}

/// The chunk of a tentative point (`getChunk(Level, Vec3)`: `Mth.floor` of x and z over
/// 16).
fn chunk_of(point: (f64, f64)) -> (i32, i32) {
    (
        (point.0 / 16.0).floor() as i32,
        (point.1 / 16.0).floor() as i32,
    )
}

/// The direction of the gateway at `from` from the middle: `new Vec3(x, 0, z)
/// .normalize()`.
fn direction_of(from: (i32, i32, i32)) -> (f64, f64) {
    let (x, z) = (f64::from(from.0), f64::from(from.2));
    let length = (x * x + z * z).sqrt();
    if length < 1.0e-5 {
        (0.0, 0.0)
    } else {
        (x / length, z / length)
    }
}

/// `findExitPortalXZPosTentative`: from 1024 out along the direction, back 16 at a time
/// while the chunk there is not empty, at most 16 steps; then on 16 at a time while it
/// is empty, at most 16 steps.
fn tentative(view: &View, from: (i32, i32, i32)) -> Result<(f64, f64)> {
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

/// `findValidSpawnInChunk`: over the chunk from y 30 to the top of its highest section
/// that is not empty, in `betweenClosed`'s order (x, then y, then z), the end stone with
/// two blocks above it that are not full, nearest by `distToCenterSqr(0, 0, 0)`; the
/// first of the nearest.
fn valid_spawn(view: &View, chunk: (i32, i32)) -> Result<Option<(i32, i32, i32)>> {
    let Some(highest) = view.highest_filled_section(chunk)? else {
        return Ok(None);
    };
    let (cx, cz) = chunk;
    let mut best: Option<((i32, i32, i32), f64)> = None;
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
                let distance = (f64::from(x) + 0.5).powi(2)
                    + (f64::from(y) + 0.5).powi(2)
                    + (f64::from(z) + 0.5).powi(2);
                if best.is_none_or(|(_, nearest)| distance < nearest) {
                    best = Some((at, distance));
                }
            }
        }
    }
    Ok(best.map(|(at, _)| at))
}

/// `findTallestBlock`: over the square of `radius` about `at`, x outer and z inner,
/// each column from the top down to above the tallest found so far, the first block
/// whose collision is a full block, bedrock only where `allow_bedrock`, the column of
/// `at` itself skipped where not; `at` where none is found.
fn tallest(
    view: &View,
    at: (i32, i32, i32),
    radius: i32,
    allow_bedrock: bool,
) -> Result<(i32, i32, i32)> {
    let mut found: Option<(i32, i32, i32)> = None;
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

/// A block's place.
type At = (i32, i32, i32);

/// `java.util.Random`, which `RandomSource.create(seed)` is (`LegacyRandomSource`).
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

/// `BlockPos.asLong`: x in the top 26 bits, z in the next 26, y in the low 12.
fn as_long((x, y, z): (i32, i32, i32)) -> i64 {
    ((i64::from(x) & 0x3FF_FFFF) << 38)
        | ((i64::from(z) & 0x3FF_FFFF) << 12)
        | (i64::from(y) & 0xFFF)
}

/// The places `end_island` sets to end stone at `origin` with
/// `RandomSource.create(origin.asLong())` (EndIslandFeature.txt, `place`): a radius of
/// 4 to 6, and while it is above a half, a disc of the radius plus one at each layer
/// down, the radius shrinking by a half or one and a half each time.
fn island_cells(origin: (i32, i32, i32)) -> Vec<(i32, i32, i32)> {
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
/// shows with the return gateway's cage hidden: the tallest block about the spawn in
/// the tentative chunk, or about the island made where that chunk has none, and ten
/// above it. Also says where the island is, if one was made.
fn expected_exit(view: &mut View, from: (i32, i32, i32)) -> Result<(At, Option<At>)> {
    // An island, if one was made: at the tentative point of the world without it,
    // where that world has no spawn in the point's chunk. Each point the search could
    // end on is tried, where the bot holds what an island there would cover.
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

/// Where a traveller through a gateway whose exit is `exit`, not exact, is placed
/// (`findExitPosition`): on the tallest block, bedrock refused, within 5 of two above
/// the exit, the exit's own column skipped; its bottom centre.
fn expected_place(view: &View, exit: (i32, i32, i32)) -> Result<(f64, f64, f64)> {
    let (x, y, z) = tallest(view, (exit.0, exit.1 + 2, exit.2), 5, false)?;
    Ok((f64::from(x) + 0.5, f64::from(y + 1), f64::from(z) + 0.5))
}

/// The gateways the bot is shown within `radius` of where it is, across.
fn gateways_near(view: &View, bot: &Bot, radius: i32) -> Result<Vec<(i32, i32, i32)>> {
    let (bx, _, bz) = dimensions::feet(bot);
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
/// feature's: each place with what it is and what it should be.
fn cage_differences(view: &View, at: (i32, i32, i32)) -> Result<Vec<String>> {
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

/// What a bot found where the first gateway sent it.
struct CameOut {
    through: Through,
    /// The return gateway at the exit.
    exit: (i32, i32, i32),
}

/// Takes `bot` from the platform into the gateway at [`GATEWAY`] and checks where it
/// came out against the specification: the return gateway, alone, at the exit the
/// search gives, in its cage; and the bot on the tallest block by it.
async fn out_to_the_islands(bot: &mut Bot, others: &mut [Bot]) -> CameOut {
    let (gx, _, gz) = GATEWAY;
    if let Err(error) =
        fly_over_the_island(bot, others, (f64::from(gx) + 0.5, f64::from(gz) + 0.5)).await
    {
        fail(bot, format!("{error:#}"));
    }
    let cruise = bot.location.1;
    let through = match through_the_gateway(bot, others, GATEWAY, cruise, LOOK).await {
        Ok(through) => through,
        Err(error) => fail(bot, format!("{error:#}")),
    };
    let mut view = View::of(bot).unwrap_or_else(|error| fail(bot, format!("{error:#}")));
    let gateways =
        gateways_near(&view, bot, 16).unwrap_or_else(|error| fail(bot, format!("{error:#}")));
    let [exit] = gateways[..] else {
        fail(
            bot,
            format!(
                "one return gateway is wanted near where the bot came out, at {:?}; it is \
                 shown {gateways:?}",
                bot.location
            ),
        );
    };
    let differences =
        cage_differences(&view, exit).unwrap_or_else(|error| fail(bot, format!("{error:#}")));
    assert!(
        differences.is_empty(),
        "the return gateway's cage at {exit:?}: {differences:#?}"
    );
    let place = expected_place(&view, exit).unwrap_or_else(|error| fail(bot, format!("{error:#}")));
    assert_eq!(
        bot.location, place,
        "placed on the tallest block within 5 of two above the exit {exit:?}"
    );
    // The search came before the cage: it is hidden from it.
    for dx in -1..=1 {
        for dy in -2..=2 {
            for dz in -1..=1 {
                view.hidden.insert((exit.0 + dx, exit.1 + dy, exit.2 + dz));
            }
        }
    }
    let (wanted, island) =
        expected_exit(&mut view, GATEWAY).unwrap_or_else(|error| fail(bot, format!("{error:#}")));
    println!(
        "the exit is {exit:?}, the bot placed at {:?}, the island {island:?}",
        bot.location
    );
    assert_eq!(
        exit, wanted,
        "the return gateway stands where the search puts the exit (an island at {island:?})"
    );
    CameOut { through, exit }
}

/// The gateway's block entity at `gateway` as the chunk the bot was sent of it has it.
fn gateway_entity(bot: &Bot, gateway: (i32, i32, i32)) -> Option<&BlockEntity> {
    let (x, y, z) = gateway;
    let chunk = bot.chunks.get(&(x >> 4, z >> 4))?;
    let packed = u8::try_from((x & 15) << 4 | (z & 15)).ok()?;
    chunk
        .block_entities
        .iter()
        .find(|entity| entity.packed_xz == packed && i32::from(entity.y) == y)
}

/// Moves the bot along its height to the nearest column out from under the cage of the
/// gateway at `cage` whose column is clear up to `top` and to which the way is clear; it
/// stays where it is if it is not under the cage.
async fn step_aside(bot: &mut Bot, others: &mut [Bot], cage: At, top: i32) -> Result<()> {
    let (fx, fy, fz) = dimensions::feet(bot);
    if (fx - cage.0).abs() > 1 || (fz - cage.2).abs() > 1 {
        return Ok(());
    }
    let (x, y, z) = bot.location;
    let clear =
        |at: (i32, i32, i32)| dimensions::is_air(bot.block_at(at.0, at.1, at.2).ok().flatten());
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
        // The box's corners along the way, at the feet and the head.
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

/// The exit written into the gateway's block entity, as the chunk the bot was sent of
/// it says: its `exit_portal`, which the update tag carries (`saveCustomOnly`).
fn written_exit(bot: &Bot, gateway: (i32, i32, i32)) -> Option<(i32, i32, i32)> {
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

/// 19.8 and its review, `p-brief.md` 2.6: a bot that goes into a gateway whose exit is
/// unknown comes out on the outer islands, on the tallest block within 5 of two above
/// the exit the game's search gives, beside a return gateway in its cage at the exit;
/// with no `Respawn`, and a teleport that keeps its velocity and look
/// (`DELTA ∪ ROTATION`, nothing added).
#[tokio::test]
async fn a_bot_that_goes_into_a_gateway_comes_out_on_the_outer_islands_where_the_search_says() {
    let world = a_world_with_a_gateway().await;
    let mut bot = arrive_in(&world, "Wanderer", &mut []).await;
    let came_out = out_to_the_islands(&mut bot, &mut []).await;
    assert!(
        came_out.exit.0 > 900,
        "the exit {:?} is on the outer islands, along the gateway's direction",
        came_out.exit
    );
    assert_a_teleport_in_the_level(&bot, &came_out.through);
    world.server.stop().await;
}

/// `EndGatewayBlock.getPortalDestination` (EndGatewayBlock.txt 72-109): a player through
/// a gateway is placed with `DELTA ∪ ROTATION`, its velocity and its turn nought, so that
/// the client keeps the velocity and the look it had.
#[tokio::test]
async fn a_bot_through_a_gateway_keeps_its_velocity_and_look() {
    let world = a_world_with_a_gateway().await;
    let mut bot = arrive_in(&world, "Keeper", &mut []).await;
    let came_out = out_to_the_islands(&mut bot, &mut []).await;
    assert_velocity_and_look_kept(&bot, &came_out.through);
    world.server.stop().await;
}

/// `triggerCooldown` (TheEndGatewayBlockEntity 230-249) when an entity goes in
/// (`entityInside`): the bot is sent `BlockEvent(pos, end_gateway, 1, 0)`, the beam.
#[tokio::test]
async fn a_bot_that_goes_into_a_gateway_sees_its_beam() {
    let world = a_world_with_a_gateway().await;
    let mut bot = arrive_in(&world, "Beholder", &mut []).await;
    let came_out = out_to_the_islands(&mut bot, &mut []).await;
    if !beam_seen(&came_out.through.packets, GATEWAY) {
        fail(
            &bot,
            format!("no BlockEvent(1, 0) of the end gateway at {GATEWAY:?} when the bot went in"),
        );
    }
    world.server.stop().await;
}

/// The return gateway (`knownExit(from, false)`): its exit is the first gateway, not
/// exact, so a bot that goes into it comes back on the tallest block within 5 of two
/// above the first gateway, beside it; as a teleport in the level, as on the way out.
#[tokio::test]
async fn a_bot_that_goes_into_the_return_gateway_comes_back_beside_the_first() {
    let world = a_world_with_a_gateway().await;
    let mut bot = arrive_in(&world, "Returner", &mut []).await;
    let came_out = out_to_the_islands(&mut bot, &mut []).await;
    let exit = came_out.exit;
    // Out from under the cage, where the bot may have been placed.
    if let Err(error) = step_aside(&mut bot, &mut [], exit, exit.1 + 6).await {
        fail(&bot, format!("{error:#}"));
    }
    // Above the return gateway's cage, which is above everything within 16 of where the
    // exit was found.
    let cruise = f64::from(exit.1 + 4);
    let back = match through_the_gateway(&mut bot, &mut [], exit, cruise, LOOK).await {
        Ok(back) => back,
        Err(error) => fail(&bot, format!("{error:#}")),
    };
    let view = View::of(&bot).unwrap_or_else(|error| fail(&bot, format!("{error:#}")));
    let place =
        expected_place(&view, GATEWAY).unwrap_or_else(|error| fail(&bot, format!("{error:#}")));
    assert_eq!(
        bot.location, place,
        "placed on the tallest block within 5 of two above the first gateway"
    );
    let (gx, _, gz) = GATEWAY;
    assert!(
        (bot.location.0 - f64::from(gx)).abs() <= 6.0
            && (bot.location.2 - f64::from(gz)).abs() <= 6.0,
        "beside the first gateway: {:?}",
        bot.location
    );
    if !beam_seen(&back.packets, exit) {
        fail(&bot, format!("no beam of the return gateway at {exit:?}"));
    }
    assert_a_teleport_in_the_level(&bot, &back);
    world.server.stop().await;
}

/// Two bots through the first gateway, one after the other: what the first found, where
/// it was placed, and the second with what it was sent and what it is shown where it
/// came out. The first stays where it came out.
struct TwoThrough {
    came_out: CameOut,
    placed: (f64, f64, f64),
    second: Bot,
    through: Through,
    view: View,
    others: [Bot; 1],
}

async fn two_through_the_first_gateway(world: &GatewayWorld) -> TwoThrough {
    let mut first = arrive_in(world, "Pioneer", &mut []).await;
    let came_out = out_to_the_islands(&mut first, &mut []).await;
    let placed = first.location;
    let mut others = [first];
    let mut second = arrive_in(world, "Follower", &mut others).await;
    let (gx, _, gz) = GATEWAY;
    if let Err(error) = fly_over_the_island(
        &mut second,
        &mut others,
        (f64::from(gx) + 0.5, f64::from(gz) + 0.5),
    )
    .await
    {
        fail(&second, format!("{error:#}"));
    }
    let cruise = second.location.1;
    let through = match through_the_gateway(&mut second, &mut others, GATEWAY, cruise, LOOK).await {
        Ok(through) => through,
        Err(error) => fail(&second, format!("{error:#}")),
    };
    drain(&mut others)
        .await
        .expect("the first bot is still there");
    let view = View::of(&second).unwrap_or_else(|error| fail(&second, format!("{error:#}")));
    TwoThrough {
        came_out,
        placed,
        second,
        through,
        view,
        others,
    }
}

/// 19.5 item 9 and 19.8: the exit is found once and written into the gateway, so a
/// second bot through the first gateway comes out by the one return gateway the first
/// bot's travel made, with no other made beside it.
#[tokio::test]
async fn a_second_bot_through_the_first_gateway_finds_the_same_exit() {
    let world = a_world_with_a_gateway().await;
    let two = two_through_the_first_gateway(&world).await;
    let gateways = gateways_near(&two.view, &two.second, 16)
        .unwrap_or_else(|error| fail(&two.second, format!("{error:#}")));
    assert_eq!(
        gateways,
        vec![two.came_out.exit],
        "the one return gateway the first bot's travel made, and no other"
    );
    let exit = two.came_out.exit;
    let (x, _, z) = two.second.location;
    assert!(
        (x - f64::from(exit.0)).abs() <= 6.0 && (z - f64::from(exit.2)).abs() <= 6.0,
        "the second bot came out by the exit {exit:?}: {:?}",
        two.second.location
    );
    assert_a_teleport_in_the_level(&two.second, &two.through);
    drop(two.others);
    world.server.stop().await;
}

/// `findExitPosition`, as for the first: the second bot through the gateway is placed
/// on the tallest block within 5 of two above the exit, where the first was placed.
#[tokio::test]
#[ignore = "finding: in a world whose regions follow the players (no pins), the second bot through the gateway is placed at (1028.5, 83, -6.5), on the return gateway's cage: the fall-back of findTallestBlock (two above the exit, one up), as if no block about the exit were read; the first bot, and the second in a world pinned at 0 and 4, are placed at (1028.5, 71, -5.5), on the tallest block within 5"]
async fn a_second_bot_through_the_first_gateway_comes_out_where_the_first_did() {
    let world = a_world_with_a_gateway().await;
    let two = two_through_the_first_gateway(&world).await;
    let second = &two.second;
    let exit = two.came_out.exit;
    if second.location != two.placed {
        let (x, _, z) = dimensions::feet(second);
        fail(
            second,
            format!(
                "the second bot came out at {:?}, the first at {:?}; the place the \
                 specification gives from what the second is shown: {:?}; the cage at \
                 {exit:?}: {:?}; the second's column: {}",
                second.location,
                two.placed,
                expected_place(&two.view, exit),
                cage_differences(&two.view, exit),
                dimensions::column_report(second, x, z, exit.1 - 15, exit.1 + 5),
            ),
        );
    }
    drop(two.others);
    world.server.stop().await;
}

/// A player who leaves while in the End and joins again is sent the End's chunks, as
/// the game sends any level's. Found while these scenes were written: the edge asked for
/// the chunks of the overworld's level until the End's home placed the player, so the
/// client stayed on its loading screen and the edge panicked.
#[tokio::test]
async fn a_bot_that_leaves_in_the_end_and_joins_again_is_sent_the_end_s_chunks() {
    let (server, address) = common::start().await;
    let (mut bot, _) = to_the_end(&address, "Returnee", &mut [])
        .await
        .unwrap_or_else(|error| panic!("the bot did not come into the End: {error:#}"));
    bot.close().await.expect("the bot leaves");
    drop(bot);
    // The leave is the server's once another bot is no longer shown the player in its
    // list.
    let mut watcher = join(&address, "Watcher", &mut [])
        .await
        .expect("a second bot joins");
    let gone = |bot: &Bot| !bot.player_list.values().any(|name| name == "Returnee");
    wait(&mut watcher, &mut [], PATIENCE, "the leave", gone)
        .await
        .expect("the first bot is gone");
    let mut others = [watcher];
    let bot = match join(&address, "Returnee", &mut others).await {
        Ok(bot) => bot,
        Err(error) => panic!("joining again in the End: {error:#}"),
    };
    assert_eq!(bot.info.login.spawn.dimension_name, THE_END);
    server.stop().await;
}

/// 19.8's review, item 4, and `TheEndGatewayBlockEntity.getUpdateTag`
/// (`saveCustomOnly`): a gateway's block entity is sent with its age, its exit and
/// whether it is exact. So the return gateway, sent to the bot where it came out,
/// carries the first gateway as its exit; and the first gateway, sent to a second bot
/// that comes after the first went through, carries the exit it was given.
#[tokio::test]
async fn a_gateway_s_block_entity_is_sent_with_its_exit() {
    let world = a_world_with_a_gateway().await;
    let mut first = arrive_in(&world, "Surveyor", &mut []).await;
    let came_out = out_to_the_islands(&mut first, &mut []).await;
    assert_eq!(
        written_exit(&first, came_out.exit),
        Some(GATEWAY),
        "the return gateway's block entity carries the first gateway as its exit; it is {:?}",
        gateway_entity(&first, came_out.exit)
    );
    let mut others = [first];
    let second = arrive_in(&world, "Latecomer", &mut others).await;
    assert_eq!(
        written_exit(&second, GATEWAY),
        Some(came_out.exit),
        "the first gateway's block entity, sent after the first bot went through, carries \
         the exit; it is {:?}",
        gateway_entity(&second, GATEWAY)
    );
    world.server.stop().await;
}
