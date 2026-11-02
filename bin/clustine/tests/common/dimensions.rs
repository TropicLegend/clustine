//! What the comparisons of the rules by dimension with the official server need
//! (`docs/groundwork/p-plan.md`, sections 17.4 and 18.4): bots that go into the Nether
//! the same way on both servers, and a floor there that is the same on both.
//!
//! Neither server can be told to put a player in the Nether by Clustine's means (it has
//! no commands), so a bot gets there as a player does on both: it builds a frame of
//! obsidian beside where it joined, lights it with flint and steel and walks in. The
//! official server's flat world has a Nether of a seed drawn at random, and Clustine's
//! flat world the Nether of seed 0, so what is around the portal a bot comes out of
//! differs. What is above it does not: the Nether's top layer of bedrock is whole at
//! y 127 in every Nether, and nothing is made above it. So the bot digs a shaft from
//! beside its portal straight up through the roof, stands on the roof and plugs the
//! hole behind it with obsidian: a floor of bedrock as far as anyone needs, with air
//! above it, on both servers alike. The scenes of the comparisons are built there.
//!
//! The bots share Clustine's codec ([`clustine_botswarm`]); what they read of the
//! official server is what that server sends a client.
//!
//! A scene with two bots reads the one it is not driving between steps
//! ([`drain`]), so that neither falls behind on its keep-alives or has its packets
//! timed late.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use clustine_botswarm::{Bot, Heard};
use clustine_data::{BLOCKS, BlockState, blocks, items};
use clustine_protocol::packets::play::{ClientboundPlay, face};
use tokio::time::Instant;

/// How long a scene waits for what it is told to wait for.
pub const PATIENCE: Duration = Duration::from_secs(60);

/// The top of the grass on both servers' flat worlds, which a player stands on at -60.
pub const GRASS: i32 = -61;

/// The Nether's whole top layer of bedrock, which a player stands on at 128.
pub const ROOF: i32 = 127;

/// The hotbar slot every scene holds its items in, and the one it keeps empty.
const HELD_SLOT: i16 = 0;
const EMPTY_SLOT: i16 = 8;

/// How long a wait reads one bot before it reads the others again.
const SLICE: Duration = Duration::from_millis(50);

/// The dimensions' names in the game's registries.
pub const OVERWORLD: &str = "minecraft:overworld";
pub const THE_NETHER: &str = "minecraft:the_nether";

/// Runs `run` against the official server and then against Clustine, each on a fresh
/// flat world, and gives what each did, the official server's first.
pub async fn on_both<T>(run: impl AsyncFn(&str) -> Result<T>) -> (Result<T>, Result<T>) {
    let oracle = clustine_botswarm::Oracle::start(false)
        .await
        .expect("the official server starts");
    let theirs = run(oracle.address()).await;
    drop(oracle);
    let (server, address) = crate::common::start().await;
    let ours = run(&address).await;
    server.stop().await;
    (theirs, ours)
}

/// Joins `address` as `name`, waits until the bot has left the loading screen and the
/// nine chunks about it have come, and empties the hotbar slots the scenes use, which
/// each server fills its own way for a new player.
pub async fn join(address: &str, name: &str, others: &mut [Bot]) -> Result<Bot> {
    let mut bot = Bot::join(address, name)
        .await
        .with_context(|| format!("{name} joining {address}"))?;
    wait(
        &mut bot,
        others,
        PATIENCE,
        "the loading screen left",
        |bot| bot.is_loaded() && around_is_held(bot),
    )
    .await?;
    bot.empty_hotbar_slot(HELD_SLOT).await?;
    bot.empty_hotbar_slot(EMPTY_SLOT).await?;
    bot.set_flying(true).await?;
    Ok(bot)
}

/// Whether the bot holds the nine chunks about where it is.
pub fn around_is_held(bot: &Bot) -> bool {
    let x = (bot.location.0.floor() as i32).div_euclid(16);
    let z = (bot.location.2.floor() as i32).div_euclid(16);
    (-1..=1).all(|dx| (-1..=1).all(|dz| bot.chunks.contains_key(&(x + dx, z + dz))))
}

/// Reads what each of `others` has been sent so far, answering as a client does.
pub async fn drain(others: &mut [Bot]) -> Result<()> {
    for other in others.iter_mut() {
        // Not reaching a state is how the read ends each time.
        let _ = other.wait_until(Duration::from_millis(1), |_| false).await;
        if let Some(ending) = &other.ended {
            bail!("{}'s connection ended: {ending:?}", other.info.profile.name);
        }
    }
    Ok(())
}

/// Reads `bot` until `done` holds, at most for `patience`, reading `others` in
/// between.
pub async fn wait(
    bot: &mut Bot,
    others: &mut [Bot],
    patience: Duration,
    what: &str,
    mut done: impl FnMut(&Bot) -> bool,
) -> Result<()> {
    let deadline = Instant::now() + patience;
    // Seen with `--nocapture` (`tools/oracles.sh`): the step a scene that ran out of
    // time was at.
    eprintln!(
        "{}: waiting up to {patience:?} for {what}",
        bot.info.profile.name
    );
    loop {
        if done(bot) {
            return Ok(());
        }
        if let Some(ending) = &bot.ended {
            bail!(
                "{}'s connection ended waiting for {what}: {ending:?}",
                bot.info.profile.name
            );
        }
        let now = Instant::now();
        if now >= deadline {
            bail!(
                "{}: no {what} within {patience:?}, at {:?}",
                bot.info.profile.name,
                bot.location
            );
        }
        let slice = SLICE.min(deadline - now);
        let _ = bot.wait_until(slice, &mut done).await;
        drain(others).await?;
    }
}

/// Waits until the server has acknowledged the click or dig `sequence`.
pub async fn acknowledged(bot: &mut Bot, others: &mut [Bot], sequence: i32) -> Result<()> {
    wait(bot, others, PATIENCE, "an acknowledgement", |bot| {
        bot.acknowledged_sequence >= sequence
    })
    .await
}

/// Waits until the server has handled everything the bot sent before, and sent what
/// the tick that handled it changed: a creative break of the air three blocks above the
/// head, which changes nothing and is acknowledged after the tick's changes. Fails if
/// the server has put the bot back since `teleports` (from [`teleports`]).
pub async fn round_trip(bot: &mut Bot, others: &mut [Bot], teleports: Option<i32>) -> Result<()> {
    let (x, y, z) = feet(bot);
    let sequence = bot.dig(x, y + 3, z).await?;
    acknowledged(bot, others, sequence).await?;
    ensure!(
        teleports == self::teleports(bot),
        "{} was put back, at {:?}",
        bot.info.profile.name,
        bot.location
    );
    Ok(())
}

/// The id of the last position the server put the bot at, if any.
pub fn teleports(bot: &Bot) -> Option<i32> {
    bot.position.as_ref().map(|position| position.teleport_id)
}

/// The block the bot's feet are in.
pub fn feet(bot: &Bot) -> (i32, i32, i32) {
    (
        bot.location.0.floor() as i32,
        bot.location.1.floor() as i32,
        bot.location.2.floor() as i32,
    )
}

/// Whether `state` is of the block `block`, whatever its properties.
pub fn is(state: Option<i32>, block: BlockState) -> bool {
    state.is_some_and(|id| {
        u16::try_from(id).is_ok_and(|id| {
            let state = BlockState(id);
            state.block_id().is_some() && state.block_id() == block.block_id()
        })
    })
}

/// Whether `state` is one of the three airs.
pub fn is_air(state: Option<i32>) -> bool {
    [blocks::AIR, blocks::CAVE_AIR, blocks::VOID_AIR]
        .into_iter()
        .any(|air| is(state, air))
}

/// The state as the game prints it, `minecraft:lava[level=2]`.
pub fn describe(state: i32) -> String {
    let Some(id) = u16::try_from(state)
        .ok()
        .map(BlockState)
        .filter(|state| state.block_id().is_some())
    else {
        return format!("state {state}");
    };
    let name = BLOCKS[id.block_id().expect("checked")].name;
    let properties: Vec<String> = id
        .properties()
        .map(|(property, value)| format!("{property}={value}"))
        .collect();
    if properties.is_empty() {
        name.to_owned()
    } else {
        format!("{name}[{}]", properties.join(","))
    }
}

/// The block's name alone.
pub fn block_name(state: i32) -> String {
    let described = describe(state);
    described
        .split_once('[')
        .map_or(described.clone(), |(name, _)| name.to_owned())
}

/// The ticks, rounded, from `from` to `to`, as a client's clock tells them.
pub fn ticks(from: Instant, to: Instant) -> i64 {
    let millis = i64::try_from(to.duration_since(from).as_millis()).unwrap_or(i64::MAX);
    let back = i64::try_from(from.duration_since(to).as_millis()).unwrap_or(i64::MAX);
    (millis - back + 25).div_euclid(50)
}

/// Puts one `item` into the held slot and holds it.
pub async fn take(bot: &mut Bot, item: i32) -> Result<()> {
    bot.take_from_creative_inventory(HELD_SLOT, item).await?;
    bot.select_slot(HELD_SLOT).await
}

/// Holds the slot that is kept empty.
pub async fn empty_hand(bot: &mut Bot) -> Result<()> {
    bot.select_slot(EMPTY_SLOT).await
}

/// Turns the bot to `look` where it is, so that the server has the look before the
/// click that needs it (a bed's facing, a bucket's aim).
pub async fn turn(bot: &mut Bot, look: (f32, f32)) -> Result<()> {
    let here = bot.location;
    bot.fly_to(here, look, 1.0).await
}

/// Clicks the side `side` of the block at `at` with what is held, and waits for the
/// server's acknowledgement.
pub async fn click(bot: &mut Bot, others: &mut [Bot], at: (i32, i32, i32), side: u8) -> Result<()> {
    let sequence = bot.use_item_on(at.0, at.1, at.2, side).await?;
    acknowledged(bot, others, sequence).await
}

/// Places a block of `item` against the side `side` of `against`, and waits until the
/// bot is shown `block` at `at`.
pub async fn place(
    bot: &mut Bot,
    others: &mut [Bot],
    item: i32,
    block: BlockState,
    against: (i32, i32, i32),
    side: u8,
    at: (i32, i32, i32),
) -> Result<()> {
    if is(bot.block_at(at.0, at.1, at.2)?, block) {
        return Ok(());
    }
    take(bot, item).await?;
    let sequence = bot
        .use_item_on(against.0, against.1, against.2, side)
        .await?;
    let what = format!("{} placed at {at:?}", describe(i32::from(block.0)));
    wait(bot, others, PATIENCE, &what, |bot| {
        bot.acknowledged_sequence >= sequence
            && is(bot.block_at(at.0, at.1, at.2).ok().flatten(), block)
    })
    .await
}

/// Uses `item` looking straight down from three blocks above the top of `floor`, as a
/// player empties a bucket onto it, and waits for the server's acknowledgement.
pub async fn pour(
    bot: &mut Bot,
    others: &mut [Bot],
    item: i32,
    floor: (i32, i32, i32),
) -> Result<()> {
    let above = (
        f64::from(floor.0) + 0.5,
        f64::from(floor.1 + 4),
        f64::from(floor.2) + 0.5,
    );
    bot.fly_to(above, (0.0, 90.0), 1.0).await?;
    take(bot, item).await?;
    let sequence = bot.use_item(0.0, 90.0).await?;
    acknowledged(bot, others, sequence).await
}

/// The frame of obsidian a bot builds beside where it joined: across x, from `x` to
/// `x + 3`, in the plane `z`, its inside at `x + 1` and `x + 2` from one above the
/// grass's top up three, lit and walked into from the south.
#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub x: i32,
    pub z: i32,
}

impl Frame {
    /// The frame for a bot standing where `bot` stands: one block west of it to two east,
    /// two blocks south, all within its reach.
    pub fn beside(bot: &Bot) -> Frame {
        let (x, _, z) = feet(bot);
        Frame { x: x - 1, z: z + 2 }
    }

    /// Where a player stands to be in the portal: between its two columns, on its
    /// bottom.
    pub fn entry(self) -> (f64, f64, f64) {
        (
            f64::from(self.x + 2),
            f64::from(GRASS + 2),
            f64::from(self.z) + 0.5,
        )
    }

    fn lit(self, bot: &Bot) -> bool {
        [self.x + 1, self.x + 2].iter().all(|x| {
            (GRASS + 2..=GRASS + 4).all(|y| {
                is(
                    bot.block_at(*x, y, self.z).ok().flatten(),
                    blocks::NETHER_PORTAL,
                )
            })
        })
    }

    /// Builds the frame from the ground up and lights it with flint and steel, unless
    /// it is lit already.
    pub async fn build_and_light(self, bot: &mut Bot, others: &mut [Bot]) -> Result<()> {
        if self.lit(bot) {
            return Ok(());
        }
        let bottom = GRASS + 1;
        let top = bottom + 4;
        // Each block against the one below it or, for the top's inner two, beside it.
        let mut steps = Vec::new();
        for x in self.x..=self.x + 3 {
            steps.push(((x, GRASS, self.z), face::TOP, (x, bottom, self.z)));
        }
        for x in [self.x, self.x + 3] {
            for y in bottom + 1..=top {
                steps.push(((x, y - 1, self.z), face::TOP, (x, y, self.z)));
            }
        }
        steps.push(((self.x, top, self.z), face::EAST, (self.x + 1, top, self.z)));
        steps.push((
            (self.x + 3, top, self.z),
            face::WEST,
            (self.x + 2, top, self.z),
        ));
        for (against, side, at) in steps {
            place(
                bot,
                others,
                items::OBSIDIAN,
                blocks::OBSIDIAN,
                against,
                side,
                at,
            )
            .await?;
        }
        take(bot, items::FLINT_AND_STEEL).await?;
        bot.use_item_on(self.x + 1, bottom, self.z, face::TOP)
            .await?;
        wait(bot, others, PATIENCE, "the frame lit", |bot| self.lit(bot)).await
    }
}

/// What a bot was sent from the moment it stepped into a portal until the portal's
/// sound (or, where none came, until it stood in the new level's chunks).
#[derive(Debug)]
pub struct Travel {
    /// Every packet but the chunks and their light, in order, with when it came.
    pub packets: Vec<(Instant, ClientboundPlay)>,
    /// Whether the portal's sound came (`LevelEvent` 1032).
    pub portal_sound: bool,
    /// Where the bot was placed.
    pub arrival: (f64, f64, f64),
}

/// Flies the bot into the lit portal of `frame` from the south, and waits until it has
/// been sent into the Nether and placed there, among the new level's chunks.
pub async fn enter(bot: &mut Bot, others: &mut [Bot], frame: Frame) -> Result<Travel> {
    let (x, y, z) = frame.entry();
    let look = (180.0, 0.0);
    let cruise = f64::from(GRASS + 9);
    let from = bot.location;
    bot.fly_to((from.0, cruise, from.2), look, 1.0).await?;
    drain(others).await?;
    bot.fly_to((x, cruise, z + 3.0), look, 1.0).await?;
    drain(others).await?;
    bot.fly_to((x, y, z + 3.0), look, 1.0).await?;
    drain(others).await?;
    // From here every packet is kept, the portal's change of level among them.
    bot.packets = Some(Vec::new());
    // Up to where its box is a little short of the portal's plane, then in at once:
    // a flight's step that reached the portal would take the change of level for a
    // put-back.
    bot.fly_to((x, y, z + 1.5), look, 0.5).await?;
    bot.stand_at(x, y, z).await?;
    wait(
        bot,
        others,
        PATIENCE,
        "the Respawn into the Nether",
        |bot| bot.info.login.spawn.dimension_name == THE_NETHER,
    )
    .await?;
    wait(bot, others, PATIENCE, "the placing in the Nether", |bot| {
        bot.position.is_some() && around_is_held(bot)
    })
    .await?;
    let sounded = |bot: &Bot| {
        bot.packets.as_ref().is_some_and(|packets| {
            packets.iter().any(|(_, packet)| {
                matches!(packet, ClientboundPlay::LevelEvent(event) if event.event == 1032)
            })
        })
    };
    // Its absence is a difference the comparison reports, not a failure of the scene.
    let portal_sound = wait(
        bot,
        others,
        Duration::from_secs(10),
        "the portal's sound",
        sounded,
    )
    .await
    .is_ok();
    let packets = bot.packets.take().unwrap_or_default();
    Ok(Travel {
        packets,
        portal_sound,
        arrival: bot.location,
    })
}

/// Joins as `name`, builds and lights a frame beside where it joined (unless `frame`
/// names one already lit) and goes through it into the Nether.
pub async fn to_the_nether(
    address: &str,
    name: &str,
    frame: Option<Frame>,
    others: &mut [Bot],
) -> Result<(Bot, Frame, Travel)> {
    let mut bot = join(address, name, others).await?;
    let frame = frame.unwrap_or_else(|| Frame::beside(&bot));
    frame.build_and_light(&mut bot, others).await?;
    let travel = enter(&mut bot, others, frame).await?;
    Ok((bot, frame, travel))
}

/// Whether the bot may stand in the block `state`: nothing it collides with that a
/// shaft has to be dug through.
fn passable(state: Option<i32>) -> bool {
    is_air(state)
        || [blocks::LAVA, blocks::WATER, blocks::FIRE, blocks::SOUL_FIRE]
            .into_iter()
            .any(|block| is(state, block))
}

/// Digs the block at `at` unless the bot may already stand in it, and waits for the
/// acknowledgement.
async fn clear(bot: &mut Bot, others: &mut [Bot], at: (i32, i32, i32)) -> Result<()> {
    if passable(bot.block_at(at.0, at.1, at.2)?) {
        return Ok(());
    }
    let sequence = bot.dig(at.0, at.1, at.2).await?;
    acknowledged(bot, others, sequence).await?;
    let what = format!("{at:?} dug");
    wait(bot, others, PATIENCE, &what, |bot| {
        passable(bot.block_at(at.0, at.1, at.2).ok().flatten())
    })
    .await
}

/// From the portal the bot was placed in, steps out sideways and digs straight up
/// through the Nether's roof, until it stands on the roof at y 128 over the hole it
/// made; returns the hole's column. Each block is dug before the bot moves into it, so
/// neither server has reason to put it back.
pub async fn climb_to_the_roof(bot: &mut Bot, others: &mut [Bot]) -> Result<(i32, i32)> {
    let (px, py, pz) = feet(bot);
    let frame_or_portal =
        |state: Option<i32>| is(state, blocks::NETHER_PORTAL) || is(state, blocks::OBSIDIAN);
    let mut column = None;
    for (dx, dz) in [(0, 1), (0, -1), (1, 0), (-1, 0)] {
        let (x, z) = (px + dx, pz + dz);
        let cells = [bot.block_at(x, py, z)?, bot.block_at(x, py + 1, z)?];
        if !cells.into_iter().any(frame_or_portal) {
            column = Some((x, z));
            break;
        }
    }
    let (x, z) = column.with_context(|| {
        format!(
            "{} found no way out of the portal at {:?}",
            bot.info.profile.name, bot.location
        )
    })?;
    eprintln!(
        "{}: climbing from {:?} through the column {x}, {z}",
        bot.info.profile.name, bot.location
    );
    clear(bot, others, (x, py, z)).await?;
    clear(bot, others, (x, py + 1, z)).await?;
    let mut feet = py;
    let put = teleports(bot);
    bot.stand_at(f64::from(x) + 0.5, f64::from(feet), f64::from(z) + 0.5)
        .await?;
    while feet <= ROOF {
        if feet + 2 <= ROOF {
            clear(bot, others, (x, feet + 2, z)).await?;
        }
        feet += 1;
        bot.stand_at(f64::from(x) + 0.5, f64::from(feet), f64::from(z) + 0.5)
            .await?;
        if feet % 8 == 0 {
            round_trip(bot, others, put).await?;
        }
    }
    round_trip(bot, others, put).await?;
    Ok((x, z))
}

/// Stands the bot on the roof `east` blocks east of `hole` and fills the hole with
/// obsidian, so that the roof is whole again: nothing poured or blown up on it reaches
/// below.
pub async fn plug(bot: &mut Bot, others: &mut [Bot], hole: (i32, i32)) -> Result<()> {
    let (x, z) = hole;
    bot.fly_to(
        (f64::from(x) + 3.5, f64::from(ROOF + 1), f64::from(z) + 0.5),
        (90.0, 30.0),
        1.0,
    )
    .await?;
    place(
        bot,
        others,
        items::OBSIDIAN,
        blocks::OBSIDIAN,
        (x + 1, ROOF, z),
        face::WEST,
        (x, ROOF, z),
    )
    .await?;
    round_trip(bot, others, teleports(bot)).await
}

/// A bot on the Nether's roof, which it reached by a portal it built: the scene's
/// floor.
pub struct OnTheRoof {
    pub bot: Bot,
    pub frame: Frame,
    pub travel: Travel,
    /// Where the bot stands: three blocks east of the plugged hole, on the roof.
    pub stand: (i32, i32, i32),
}

/// Joins as `name`, goes into the Nether through a portal it builds, climbs onto the
/// roof and plugs the hole behind it.
pub async fn onto_the_roof(address: &str, name: &str) -> Result<OnTheRoof> {
    let (mut bot, frame, travel) = to_the_nether(address, name, None, &mut []).await?;
    let hole = climb_to_the_roof(&mut bot, &mut []).await?;
    plug(&mut bot, &mut [], hole).await?;
    let stand = feet(&bot);
    Ok(OnTheRoof {
        bot,
        frame,
        travel,
        stand,
    })
}

/// The block changes `bot` heard from `from` on, each with when it came: the bot must
/// have been told to record them ([`Bot::record_heard`]).
pub fn changes(bot: &Bot, from: usize) -> Vec<(Instant, (i32, i32, i32), i32)> {
    let Some(heard) = &bot.heard else {
        return Vec::new();
    };
    heard[from.min(heard.len())..]
        .iter()
        .flat_map(|(at, heard)| match heard {
            Heard::Blocks { blocks, .. } => blocks
                .iter()
                .map(|(position, state)| (*at, *position, *state))
                .collect(),
            _ => Vec::new(),
        })
        .collect()
}

/// A sound heard at a place, without its seed, its place relative to `origin` in
/// eighths of a block.
#[derive(Debug, Clone, PartialEq)]
pub struct HeardSound {
    pub sound: i32,
    pub source: i32,
    pub at: (i32, i32, i32),
    pub volume: f32,
    pub pitch: f32,
}

/// The sounds `bot` heard at places from `from` on, relative to `origin`.
pub fn sounds_heard(bot: &Bot, from: usize, origin: (i32, i32, i32)) -> Vec<(Instant, HeardSound)> {
    let Some(heard) = &bot.heard else {
        return Vec::new();
    };
    heard[from.min(heard.len())..]
        .iter()
        .filter_map(|(when, heard)| match heard {
            Heard::Sound {
                sound,
                source,
                at,
                volume,
                pitch,
            } => Some((
                *when,
                HeardSound {
                    sound: *sound,
                    source: *source,
                    at: (
                        at.0 - origin.0 * 8,
                        at.1 - origin.1 * 8,
                        at.2 - origin.2 * 8,
                    ),
                    volume: *volume,
                    pitch: *pitch,
                },
            )),
            _ => None,
        })
        .collect()
}

/// Where the record of what `bot` heard ends now.
pub fn mark(bot: &Bot) -> usize {
    bot.heard.as_ref().map_or(0, Vec::len)
}

/// The blocks changed from `from` on, last state first-come order kept, by place
/// relative to `origin`: each place with every state it was sent, in order.
pub fn states_by_place(
    bot: &Bot,
    from: usize,
    origin: (i32, i32, i32),
) -> BTreeMap<(i32, i32, i32), Vec<(Instant, i32)>> {
    let mut by_place: BTreeMap<_, Vec<(Instant, i32)>> = BTreeMap::new();
    for (when, (x, y, z), state) in changes(bot, from) {
        let place = (x - origin.0, y - origin.1, z - origin.2);
        let states = by_place.entry(place).or_default();
        // A section's update can repeat a state already sent; only changes count.
        if states.last().map(|(_, last)| *last) != Some(state) {
            states.push((when, state));
        }
    }
    by_place
}

// ---------------------------------------------------------------------------------
// The End (`docs/groundwork/p-plan.md`, sections 19.3 and 19.5; `p-brief.md`, 2.4 and
// 4.3).
// ---------------------------------------------------------------------------------

/// The End's name in the game's registries.
pub const THE_END: &str = "minecraft:the_end";

/// The block a player who comes into the End by its portal stands in: the platform's
/// middle, one above its floor of obsidian (`ServerLevel.END_SPAWN_POINT` (100, 50, 0)
/// less one for a player, `p-brief.md` 2.4).
pub const END_PLATFORM: (i32, i32, i32) = (100, 49, 0);

/// Where such a player is placed: the bottom centre of [`END_PLATFORM`], turned west
/// (`Direction.WEST.toYRot()`).
pub const END_ARRIVAL: (f64, f64, f64) = (100.5, 49.0, 0.5);
pub const END_ARRIVAL_YAW: f32 = 90.0;

/// The End's lowest y, below which less 64 the void hurts (`Entity.checkBelowWorld`).
pub const END_FLOOR: i32 = 0;

/// The ring of twelve end portal frames a bot builds on the grass beside where it
/// joined, around a 3 × 3 whose lowest corner is `x`, `z`, at one above the grass's
/// top. Its frames face inwards, as the ring the eyes open must
/// (`EndPortalFrameBlock.getOrCreatePortalShape`); a frame placed by a player faces
/// them (`getHorizontalDirection().getOpposite()`), so the bot, hovering over the
/// middle, looks out at each frame's place as it places it.
#[derive(Debug, Clone, Copy)]
pub struct Ring {
    pub x: i32,
    pub z: i32,
}

impl Ring {
    /// The ring for a bot standing where `bot` stands: its inside from three to five
    /// blocks east and from one north to one south, so that the bot is not in it.
    pub fn beside(bot: &Bot) -> Ring {
        let (x, _, z) = feet(bot);
        Ring { x: x + 3, z: z - 1 }
    }

    /// The middle of the inside, at the frames' height: where the portal's opening is
    /// heard (`EnderEyeItem.useOn`, `globalLevelEvent(1038)`).
    pub fn centre(self) -> (i32, i32, i32) {
        (self.x + 1, GRASS + 1, self.z + 1)
    }

    /// Where the bot hovers while it builds: over the middle, two blocks above the
    /// frames, so that its box does not reach the portal when it opens.
    fn hover(self) -> (f64, f64, f64) {
        let (x, y, z) = self.centre();
        (f64::from(x) + 0.5, f64::from(y + 2), f64::from(z) + 0.5)
    }

    /// Each frame's place, the facing it must have, and the yaw of a look out at it from
    /// the middle: north row first, then east, south and west.
    fn frames(self) -> Vec<((i32, i32, i32), &'static str, f32)> {
        let y = GRASS + 1;
        let mut frames = Vec::new();
        for i in 0..3 {
            frames.push(((self.x + i, y, self.z - 1), "south", 180.0));
        }
        for i in 0..3 {
            frames.push(((self.x + 3, y, self.z + i), "west", -90.0));
        }
        for i in (0..3).rev() {
            frames.push(((self.x + i, y, self.z + 3), "north", 0.0));
        }
        for i in (0..3).rev() {
            frames.push(((self.x - 1, y, self.z + i), "east", 90.0));
        }
        frames
    }

    /// Whether the bot is shown the inside as nine blocks of the End's portal.
    pub fn is_open(self, bot: &Bot) -> bool {
        (0..3).all(|i| {
            (0..3).all(|j| {
                is(
                    bot.block_at(self.x + i, GRASS + 1, self.z + j)
                        .ok()
                        .flatten(),
                    blocks::END_PORTAL,
                )
            })
        })
    }
}

/// The value of the property `name` of `state`, if it has one.
pub fn property(state: Option<i32>, name: &str) -> Option<&'static str> {
    let state = BlockState(u16::try_from(state?).ok()?);
    state.block_id()?;
    state
        .properties()
        .find(|(property, _)| *property == name)
        .map(|(_, value)| value)
}

/// What a bot was sent on its way into the End, and where it began.
#[derive(Debug)]
pub struct EndTravel {
    /// Every packet but the chunks and their light, in order, with when it came, from
    /// just before the last eye was used until the bot stood placed in the End among
    /// its chunks.
    pub packets: Vec<(Instant, ClientboundPlay)>,
    /// Where the server placed the bot when it joined, which is where it respawns.
    pub entered: (f64, f64, f64),
    /// The ring it built and opened.
    pub ring: Ring,
}

/// Places the end portal frame of the ring at `at`, looking along `yaw`, against the
/// grass below it, and makes sure it faces `facing`.
async fn place_frame(
    bot: &mut Bot,
    others: &mut [Bot],
    at: (i32, i32, i32),
    facing: &str,
    yaw: f32,
) -> Result<()> {
    turn(bot, (yaw, 45.0)).await?;
    take(bot, items::END_PORTAL_FRAME).await?;
    let sequence = bot.use_item_on(at.0, at.1 - 1, at.2, face::TOP).await?;
    let what = format!("an end portal frame placed at {at:?}");
    wait(bot, others, PATIENCE, &what, |bot| {
        bot.acknowledged_sequence >= sequence
            && is(
                bot.block_at(at.0, at.1, at.2).ok().flatten(),
                blocks::END_PORTAL_FRAME,
            )
    })
    .await?;
    let state = bot.block_at(at.0, at.1, at.2)?;
    ensure!(
        property(state, "facing") == Some(facing) && property(state, "eye") == Some("false"),
        "the frame at {at:?}, placed looking along yaw {yaw}, is {}; it should face {facing} \
         without its eye",
        state.map_or_else(|| "unknown".to_owned(), describe)
    );
    Ok(())
}

/// Uses an eye of ender on the frame at `at`, and waits until it is shown with its eye.
async fn fill_frame(bot: &mut Bot, others: &mut [Bot], at: (i32, i32, i32)) -> Result<()> {
    take(bot, items::ENDER_EYE).await?;
    let sequence = bot.use_item_on(at.0, at.1, at.2, face::TOP).await?;
    let what = format!("the frame at {at:?} filled with its eye");
    wait(bot, others, PATIENCE, &what, |bot| {
        bot.acknowledged_sequence >= sequence
            && property(bot.block_at(at.0, at.1, at.2).ok().flatten(), "eye") == Some("true")
    })
    .await
}

/// Joins as `name`, places a ring of end portal frames beside where it joined and
/// fills each with an eye of ender, hovering over its middle; waits until the bot is
/// shown the portal the last eye opens. The packets are kept, in [`Bot::packets`], from
/// just before the last eye. Returns the bot, where it joined and the ring.
pub async fn open_an_end_portal(
    address: &str,
    name: &str,
    others: &mut [Bot],
) -> Result<(Bot, (f64, f64, f64), Ring)> {
    let mut bot = join(address, name, others).await?;
    let entered = bot.location;
    let ring = Ring::beside(&bot);
    let hover = ring.hover();
    let from = bot.location;
    bot.fly_to((from.0, hover.1, from.2), (0.0, 45.0), 1.0)
        .await?;
    drain(others).await?;
    bot.fly_to(hover, (0.0, 45.0), 1.0).await?;
    drain(others).await?;
    let frames = ring.frames();
    for (at, facing, yaw) in &frames {
        place_frame(&mut bot, others, *at, facing, *yaw).await?;
    }
    let (last, eleven) = frames.split_last().expect("twelve frames");
    for (at, _, _) in eleven {
        fill_frame(&mut bot, others, *at).await?;
    }
    ensure!(
        !ring.is_open(&bot),
        "the portal opened before its twelfth eye, at {ring:?}"
    );
    // From here every packet is kept: the portal's opening, the change of level.
    bot.packets = Some(Vec::new());
    fill_frame(&mut bot, others, last.0).await?;
    wait(
        &mut bot,
        others,
        PATIENCE,
        "the End's portal opened",
        |bot| ring.is_open(bot),
    )
    .await?;
    Ok((bot, entered, ring))
}

/// Joins as `name`, opens an End portal beside where it joined
/// ([`open_an_end_portal`]) and drops into it as soon as it is shown open; waits until
/// it is placed in the End among its chunks. The packets are kept from just before the
/// last eye; once they are taken, the bot goes on keeping what comes after in
/// [`Bot::packets`], for the scenes that read the End.
pub async fn to_the_end(address: &str, name: &str, others: &mut [Bot]) -> Result<(Bot, EndTravel)> {
    let (mut bot, entered, ring) = open_an_end_portal(address, name, others).await?;
    let hover = ring.hover();
    // In at once: standing on the portal block's bottom, its box reaches the portal's
    // shape (from 6 to 12 sixteenths up), which a flight's step there would take the
    // change of level for a put-back.
    let (x, _, z) = hover;
    bot.stand_at(x, f64::from(GRASS + 1), z).await?;
    wait(
        &mut bot,
        others,
        PATIENCE,
        "the Respawn into the End",
        |bot| bot.info.login.spawn.dimension_name == THE_END,
    )
    .await?;
    wait(
        &mut bot,
        others,
        PATIENCE,
        "the placing in the End",
        |bot| bot.position.is_some() && around_is_held(bot),
    )
    .await?;
    let packets = bot.packets.take().unwrap_or_default();
    bot.record_packets();
    Ok((
        bot,
        EndTravel {
            packets,
            entered,
            ring,
        },
    ))
}

/// Whether every block of the column `x`, `z` from `low` to `high` is air as the bot
/// is shown it; not where it holds no chunk there.
pub fn column_is_clear(bot: &Bot, x: i32, z: i32, low: i32, high: i32) -> bool {
    (low..=high).all(|y| is_air(bot.block_at(x, y, z).ok().flatten()))
}

/// What the bot is shown of the column `x`, `z` from `low` to `high` that is not air,
/// for a person to read.
pub fn column_report(bot: &Bot, x: i32, z: i32, low: i32, high: i32) -> String {
    let found: Vec<String> = (low..=high)
        .filter_map(|y| match bot.block_at(x, y, z).ok().flatten() {
            None => Some(format!("{y}: not held")),
            Some(state) if !is_air(Some(state)) => Some(format!("{y}: {}", describe(state))),
            Some(_) => None,
        })
        .collect();
    format!("({x}, {z}) from {low} to {high}: {found:?}")
}

/// The height above every block of the End's island and its pillars, where the bot
/// flies across the island.
pub const END_CRUISE: i32 = 140;

/// From the platform, walks off its east side, away from the island's middle, to the
/// first column east of it with nothing in it from `low` to `high`, along the
/// platform's height; returns the column's x. The platform lies under the island's
/// rim, which hangs over it, and beside the island's underside.
async fn east_off_the_platform(
    bot: &mut Bot,
    others: &mut [Bot],
    low: i32,
    high: i32,
) -> Result<i32> {
    let (_, y, z) = feet(bot);
    let (px, _, _) = END_PLATFORM;
    // The platform's east edge is x 102; the bot holds a chunk or more beyond it.
    let column = (px + 3..px + 3 + 16)
        .find(|x| column_is_clear(bot, *x, z, low, high))
        .with_context(|| {
            format!(
                "no clear column east of the platform: {}",
                column_report(bot, px + 3, z, low, high)
            )
        })?;
    // Along the platform's height every block on the way must let the bot through.
    for x in px + 3..=column {
        ensure!(
            column_is_clear(bot, x, z, y, y + 1),
            "something in the way off the platform: {}",
            column_report(bot, x, z, y, y + 1)
        );
    }
    let (_, here_y, here_z) = bot.location;
    bot.fly_to((f64::from(column) + 0.5, here_y, here_z), (-90.0, 0.0), 0.5)
        .await?;
    drain(others).await?;
    Ok(column)
}

/// From the platform, goes east off it, rises to [`END_CRUISE`] and flies over the
/// island to `to`, so that the dragon made at (0, 128, 0) comes within the bot's view
/// whatever its view distance.
pub async fn fly_over_the_island(bot: &mut Bot, others: &mut [Bot], to: (f64, f64)) -> Result<()> {
    let (_, y, _) = feet(bot);
    east_off_the_platform(bot, others, y, END_CRUISE + 1).await?;
    let (x, _, z) = bot.location;
    let cruise = f64::from(END_CRUISE);
    bot.fly_to((x, cruise, z), (END_ARRIVAL_YAW, 0.0), 1.0)
        .await?;
    drain(others).await?;
    bot.fly_to((to.0, cruise, to.1), (END_ARRIVAL_YAW, 0.0), 1.0)
        .await?;
    drain(others).await
}

/// From the platform, goes east off it to the first column with nothing in it from
/// the platform's height down to the End's floor, and down that column to `bottom`,
/// below the void's line. Returns the column.
pub async fn off_the_platform(
    bot: &mut Bot,
    others: &mut [Bot],
    bottom: f64,
) -> Result<(i32, i32)> {
    let (_, y, z) = feet(bot);
    let column = east_off_the_platform(bot, others, END_FLOOR, y + 2).await?;
    let (x, _, here_z) = bot.location;
    bot.fly_to((x, bottom, here_z), (-90.0, 0.0), 4.0).await?;
    drain(others).await?;
    Ok((column, z))
}

// ---------------------------------------------------------------------------------
// The End's gateways (`gateways.rs`, `gateways_oracle.rs`).
// ---------------------------------------------------------------------------------

/// Where the scenes of the gateways have one: the place of the first of the twenty
/// gateways the dragon's death makes (`EnderDragonFight`: 96 × the cosine and sine of
/// the first angle, at y 75), west of the platform and above it. Its direction from the
/// middle is due east, so its exit is looked for along z 0 from x 1024
/// (`findExitPortalXZPosTentative`).
pub const GATEWAY: (i32, i32, i32) = (96, 75, 0);

/// The block a gateway's feature puts at `(dx, dy, dz)` from its middle
/// (`EndGatewayFeature.place`, `p-brief.md` 2.6): the gateway in the middle, air round
/// it on its layer, a plus of bedrock one above and one below, and bedrock on the
/// middle two above and two below with air round it.
pub fn gateway_cage(dx: i32, dy: i32, dz: i32) -> BlockState {
    if (dx, dy, dz) == (0, 0, 0) {
        blocks::END_GATEWAY
    } else if dy == 0 {
        blocks::AIR
    } else if dy.abs() == 2 {
        if (dx, dz) == (0, 0) {
            blocks::BEDROCK
        } else {
            blocks::AIR
        }
    } else if dx == 0 || dz == 0 {
        blocks::BEDROCK
    } else {
        blocks::AIR
    }
}

/// Joins as `name` and drops into the End's portal `ring` another bot opened, as
/// [`to_the_end`] does; waits until it is placed in the End among its chunks.
pub async fn through_the_open_portal(
    address: &str,
    name: &str,
    ring: Ring,
    others: &mut [Bot],
) -> Result<Bot> {
    let mut bot = join(address, name, others).await?;
    let hover = ring.hover();
    let from = bot.location;
    bot.fly_to((from.0, hover.1, from.2), (0.0, 45.0), 1.0)
        .await?;
    drain(others).await?;
    bot.fly_to(hover, (0.0, 45.0), 1.0).await?;
    drain(others).await?;
    bot.stand_at(hover.0, f64::from(GRASS + 1), hover.2).await?;
    wait(
        &mut bot,
        others,
        PATIENCE,
        "the Respawn into the End",
        |bot| bot.info.login.spawn.dimension_name == THE_END,
    )
    .await?;
    wait(
        &mut bot,
        others,
        PATIENCE,
        "the placing in the End",
        |bot| bot.position.is_some() && around_is_held(bot),
    )
    .await?;
    Ok(bot)
}

/// Breaks the block at `at`, as a creative player breaks anything, bedrock too, and
/// waits until the bot is shown air there.
pub async fn break_block(bot: &mut Bot, others: &mut [Bot], at: (i32, i32, i32)) -> Result<()> {
    empty_hand(bot).await?;
    let sequence = bot.dig(at.0, at.1, at.2).await?;
    let what = format!("the block at {at:?} broken");
    wait(bot, others, PATIENCE, &what, |bot| {
        bot.acknowledged_sequence >= sequence
            && is_air(bot.block_at(at.0, at.1, at.2).ok().flatten())
    })
    .await
}

/// From where the bot is, its column clear up to `cruise`, flies up to `cruise`, across
/// to over the gateway at `gateway`, its column clear from `cruise` down to three above
/// it, and down to two above its cage; breaks the cage's two blocks of bedrock over the
/// gateway, and steps down into it.
///
/// A player cannot walk into a gateway in its cage: on its own layer the cage is open
/// on every side, but a player is taller than a block, and the bedrock above and below
/// the gateway and on the plus round them leaves no room. A player in creative breaks
/// bedrock, and from above, with the two blocks over the gateway gone, the player's box
/// comes into the gateway's block and touches no other. Only the stepping in is sent:
/// the bot does not wait for what the gateway does.
pub async fn down_into_the_gateway(
    bot: &mut Bot,
    others: &mut [Bot],
    gateway: (i32, i32, i32),
    cruise: f64,
) -> Result<()> {
    let (x, y, z) = gateway;
    let top = cruise as i32 + 2;
    let (fx, fy, fz) = feet(bot);
    ensure!(
        column_is_clear(bot, fx, fz, fy, top),
        "the column over the bot is not clear: {}",
        column_report(bot, fx, fz, fy, top)
    );
    ensure!(
        column_is_clear(bot, x, z, y + 3, top),
        "the column over the gateway is not clear: {}",
        column_report(bot, x, z, y + 3, top)
    );
    let (cx, cz) = (f64::from(x) + 0.5, f64::from(z) + 0.5);
    let look = bot.look;
    let (here_x, _, here_z) = bot.location;
    bot.fly_to((here_x, cruise, here_z), look, 1.0).await?;
    drain(others).await?;
    bot.fly_to((cx, cruise, cz), look, 1.0).await?;
    drain(others).await?;
    // Feet a block above the cage's top: the box reaches neither the bedrock nor the
    // gateway, and both blocks to break are within reach.
    bot.fly_to((cx, f64::from(y + 3), cz), look, 0.5).await?;
    drain(others).await?;
    let gateway_state = bot.block_at(x, y, z)?;
    ensure!(
        is(gateway_state, blocks::END_GATEWAY),
        "the gateway at {gateway:?} is {}",
        gateway_state.map_or_else(|| "not held".to_owned(), describe)
    );
    // The cage's bedrock over the gateway, which another bot may have broken before.
    for dy in [2, 1] {
        let state = bot.block_at(x, y + dy, z)?;
        if is_air(state) {
            continue;
        }
        ensure!(
            is(state, blocks::BEDROCK),
            "{dy} above the gateway at {gateway:?} is {}",
            state.map_or_else(|| "not held".to_owned(), describe)
        );
        break_block(bot, others, (x, y + dy, z)).await?;
    }
    // Down to where the box is just above the gateway, then in with one step: a
    // flight would go on after the gateway's teleport, back towards here.
    bot.fly_to((cx, f64::from(y) + 1.05, cz), look, 0.5).await?;
    drain(others).await?;
    bot.stand_at(cx, f64::from(y) + 0.25, cz).await?;
    Ok(())
}

/// Takes a bot standing on the End's platform back to the overworld the one way there
/// is without the dragon's death: off the platform into the void, where it dies; then,
/// as the bots have no way to ask for a respawn, on its raw connection the client's
/// command (`PERFORM_RESPAWN`), answering keep-alives, until the server has sent the
/// `Respawn` into the overworld and placed it there. The connection is then closed: the
/// player has left in the overworld.
pub async fn back_through_the_void(mut bot: Bot, others: &mut [Bot]) -> Result<()> {
    use clustine_protocol::packets::play::{
        ClientCommand, ClientboundKeepAlive, ServerboundKeepAlive, client_command,
    };
    off_the_platform(&mut bot, others, f64::from(END_FLOOR - 80)).await?;
    wait(&mut bot, others, PATIENCE, "the void's death", |bot| {
        !bot.deaths.is_empty()
    })
    .await?;
    let mut connection = bot.into_connection();
    connection
        .write(&ClientCommand {
            action: client_command::PERFORM_RESPAWN,
        })
        .await?;
    let deadline = Instant::now() + PATIENCE;
    let mut respawned = false;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let frame = tokio::time::timeout(left, connection.read_frame())
            .await
            .context("the respawn did not come in time")??;
        match ClientboundPlay::decode(&frame)? {
            ClientboundPlay::ClientboundKeepAlive(ClientboundKeepAlive { id }) => {
                connection.write(&ServerboundKeepAlive { id }).await?;
            }
            ClientboundPlay::Respawn(respawn) => {
                ensure!(
                    respawn.spawn.dimension_name == OVERWORLD,
                    "the respawn is into {}",
                    respawn.spawn.dimension_name
                );
                respawned = true;
            }
            ClientboundPlay::SynchronizePlayerPosition(_) if respawned => break,
            _ => {}
        }
        drain(others).await?;
    }
    connection.close().await
}

/// A server on a world on disk whose End has a gateway at [`GATEWAY`], with the open
/// End portal a bot made in the overworld.
pub struct GatewayWorld {
    pub server: clustine::Server,
    pub address: String,
    pub ring: Ring,
    _directory: tempfile::TempDir,
}

/// Has a bot open an End portal and come into the End, which makes and stores the
/// chunks about the platform, and go back to the overworld through the void; stops the
/// server, writes the gateway into the world ([`write_a_gateway`]) and starts the server
/// again on it.
///
/// The bot leaves in the overworld, not in the End: a player who leaves in the End and
/// joins again is sent no chunks (`gateways.rs` has the finding).
pub async fn a_world_with_a_gateway() -> GatewayWorld {
    let directory = tempfile::tempdir().expect("a directory for the world");
    let config = clustine::Config {
        world: Some(directory.path().join("world")),
        ..crate::common::config()
    };
    let (server, address) = crate::common::start_with(config.clone()).await;
    let (bot, travel) = to_the_end(&address, "Builder", &mut [])
        .await
        .unwrap_or_else(|error| panic!("the builder did not come into the End: {error:#}"));
    back_through_the_void(bot, &mut [])
        .await
        .unwrap_or_else(|error| panic!("the builder did not come back: {error:#}"));
    server.stop().await;
    let world = directory.path().join("world");
    write_a_gateway(&world).unwrap_or_else(|error| panic!("the gateway: {error:#}"));
    let (server, address) = crate::common::start_with(config).await;
    GatewayWorld {
        server,
        address,
        ring: travel.ring,
        _directory: directory,
    }
}

/// Waits until the bot is shown the gateway at [`GATEWAY`].
pub async fn gateway_shown(bot: &mut Bot, others: &mut [Bot]) -> Result<()> {
    let (x, y, z) = GATEWAY;
    let shown = |bot: &Bot| is(bot.block_at(x, y, z).ok().flatten(), blocks::END_GATEWAY);
    wait(bot, others, PATIENCE, "the gateway", shown).await
}

/// Joins the world as `name` and goes into the End by the open portal, and waits until
/// the bot is shown the gateway.
pub async fn arrive(world: &GatewayWorld, name: &str, others: &mut [Bot]) -> Result<Bot> {
    let mut bot = through_the_open_portal(&world.address, name, world.ring, others).await?;
    gateway_shown(&mut bot, others).await?;
    Ok(bot)
}

/// What a bot was sent from the moment it stepped into a gateway until it stood placed
/// among its chunks where the gateway sent it.
pub struct Through {
    /// Every packet but the chunks and their light, in order.
    pub packets: Vec<(Instant, ClientboundPlay)>,
    /// The teleport that placed it.
    pub placing: clustine_protocol::packets::play::SynchronizePlayerPosition,
}

/// Goes from where the bot is into the gateway at `gateway` (from `cruise`, over it),
/// looking along `look` all the while, and waits until it is placed more than 16 blocks
/// away among the chunks of its view.
pub async fn through_the_gateway(
    bot: &mut Bot,
    others: &mut [Bot],
    gateway: (i32, i32, i32),
    cruise: f64,
    look: (f32, f32),
) -> Result<Through> {
    bot.look = look;
    down_into_the_gateway(bot, others, gateway, cruise).await?;
    // Kept from the step into the gateway on: the beam and the placing come after it.
    bot.packets = Some(Vec::new());
    let stepped = teleports(bot);
    let far = |bot: &Bot| {
        let (x, _, z) = bot.location;
        let (gx, _, gz) = gateway;
        teleports(bot) != stepped
            && ((x - f64::from(gx)).powi(2) + (z - f64::from(gz)).powi(2)).sqrt() > 16.0
    };
    wait(bot, others, PATIENCE, "the gateway's teleport", far).await?;
    // Every chunk of its view: what is read about the exit reaches 16 blocks and more.
    let (x, _, z) = feet(bot);
    let area = crate::common::view_area((x >> 4, z >> 4), crate::common::VIEW_DISTANCE);
    let held =
        |bot: &Bot| around_is_held(bot) && area.iter().all(|chunk| bot.chunks.contains_key(chunk));
    wait(bot, others, PATIENCE, "the chunks where it came out", held).await?;
    let packets = bot.packets.take().unwrap_or_default();
    bot.record_packets();
    let placing = bot.position.clone().context("placed")?;
    Ok(Through { packets, placing })
}

/// The file of the End's chunk `(x, z)` in the stopped world in `world`, as the store
/// names it.
pub fn end_chunk_file(world: &std::path::Path, x: i32, z: i32) -> std::path::PathBuf {
    world
        .join("chunks")
        .join("the_end")
        .join(format!("{}.{}", x >> 5, z >> 5))
        .join(format!("{x}.{z}.chunk"))
}

/// Puts a gateway in its cage at [`GATEWAY`] into the stopped world in `world`, with no
/// exit, as the dragon's death leaves one (`end_gateway_delayed`): into the stored End
/// chunk that holds it, which a bot must have had made by standing near it. Its block
/// entity is left without data, which the game reads as a gateway of age 0 with no exit
/// (`TheEndGatewayBlockEntity.loadAdditional`). Clustine has no commands, and a gateway
/// comes only of the dragon's death, so the scenes write it into the world before the
/// server starts on it.
///
/// The light of that chunk and of the eight about it is taken out, so that the light
/// the gateway gives is made afresh with the chunks when they are next loaded.
pub fn write_a_gateway(world: &std::path::Path) -> Result<()> {
    use clustine_format::ChunkFile;
    let (gx, gy, gz) = GATEWAY;
    // The cage's blocks by the chunk they are in.
    type Cells = Vec<((i32, i32, i32), BlockState)>;
    let mut cage: BTreeMap<(i32, i32), Cells> = BTreeMap::new();
    for ox in -1..=1 {
        for oy in -2..=2 {
            for oz in -1..=1 {
                let (x, y, z) = (gx + ox, gy + oy, gz + oz);
                cage.entry((x >> 4, z >> 4))
                    .or_default()
                    .push(((x, y, z), gateway_cage(ox, oy, oz)));
            }
        }
    }
    // The chunks the cage is in, which [`GATEWAY`]'s place spreads over four, and those
    // about them, whose light the gateway's reaches.
    let low = (
        cage.keys().map(|c| c.0).min(),
        cage.keys().map(|c| c.1).min(),
    );
    let high = (
        cage.keys().map(|c| c.0).max(),
        cage.keys().map(|c| c.1).max(),
    );
    let (Some(low_x), Some(low_z), Some(high_x), Some(high_z)) = (low.0, low.1, high.0, high.1)
    else {
        unreachable!("a cage has blocks");
    };
    for chunk_x in low_x - 1..=high_x + 1 {
        for chunk_z in low_z - 1..=high_z + 1 {
            let path = end_chunk_file(world, chunk_x, chunk_z);
            let cells = cage.get(&(chunk_x, chunk_z));
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(_) if cells.is_none() => continue,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("reading a chunk of the gateway's cage, {}", path.display())
                    });
                }
            };
            let (mut file, _) = ChunkFile::decode(&bytes)
                .map_err(|error| anyhow::anyhow!("{error}"))
                .with_context(|| format!("decoding {}", path.display()))?;
            file.chunk.take_light();
            for &((x, y, z), state) in cells.into_iter().flatten() {
                file.chunk
                    .set((x & 15) as usize, y, (z & 15) as usize, state)
                    .context("the cage is within the End's height")?;
            }
            std::fs::write(&path, file.encode())
                .with_context(|| format!("writing {}", path.display()))?;
        }
    }
    Ok(())
}
