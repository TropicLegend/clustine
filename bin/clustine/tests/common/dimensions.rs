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
