//! End-to-end tests of light at a pinned line (`docs/groundwork/n8-plan.md`, sections 4,
//! 5 and 10 part B; `docs/adr/0029-light-in-the-simulation.md`), written from the
//! specification by someone who did not write the code: each region lights its own
//! chunks and its copies from the blocks it has, so the light a region reads beside the
//! line is the light one region would read with the same blocks; a torch on one side
//! lights what is planted on the other; grass spreads across the line; and the edge's
//! light, which it makes for clients on its own (ADR-0023), agrees with the region's.
//!
//! A region's light cannot be read from outside, so it is read by what the game decides
//! by it at once: seeds are refused where the light is below 8 (`CropBlock.canSurvive`,
//! the plan's 6.3) and a red mushroom on a solid block where it is 13 or more
//! (`MushroomBlock.canSurvive`, n8 MushroomBlock 144–175). Both are placements, decided
//! by the region that holds the block from its own light (the plan's 4.3), so a
//! placement across the line asks the other region. The probes lie in tunnels dug in
//! the flat world's dirt and covered again, sealed, so that no sky reaches them and a
//! torch's light falls one level a block along them: a torch's 14 gives 8 six blocks
//! away and 7 seven, 13 one block away and 12 two.
//!
//! The worlds are divided at chunk x = 0 and 4 by the tests themselves (the world of
//! `CLUSTINE_TEST_PINS=0,4`), so the line at block x = 0 is beside the spawn whatever the
//! environment says. The player stays in chunk (0, 0), east of the line, so that every
//! placement west of it is passed on to the region there.

mod common;

use std::time::Duration;

use clustine::Config;
use clustine_botswarm::{Bot, LightAt};
use clustine_data::{blocks, items};
use clustine_protocol::item::ItemStack;
use clustine_protocol::packets::play::face;

use common::{VIEW_DISTANCE, config, start_with, view_area};

const PATIENCE: Duration = Duration::from_secs(60);

/// How long grass may take to spread across the line. Each of the strip's dirt blocks
/// is reached by three grass blocks across it, each taking a random tick about every
/// 1,365 ticks and trying four targets of 45: about one in 5,000 a tick for each dirt
/// block, so the strip of 25 turns its first in about ten seconds.
const GRASS_PATIENCE: Duration = Duration::from_secs(300);

const AIR: Option<i32> = Some(blocks::AIR.0 as i32);
const STONE: Option<i32> = Some(blocks::STONE.0 as i32);
const DIRT: Option<i32> = Some(blocks::DIRT.0 as i32);
const GRASS: Option<i32> = Some(blocks::GRASS_BLOCK.0 as i32);
const FARMLAND: Option<i32> = Some(blocks::FARMLAND.0 as i32);
const WHEAT: Option<i32> = Some(blocks::WHEAT.0 as i32);
const TORCH: Option<i32> = Some(blocks::TORCH.0 as i32);
const RED_MUSHROOM: Option<i32> = Some(blocks::RED_MUSHROOM.0 as i32);

/// The flat world's layers: bedrock at −64, dirt at −63 and −62, grass at −61.
const BEDROCK_Y: i32 = -64;
const FLOOR_Y: i32 = -63;
const TUNNEL_Y: i32 = -62;
const TOP_Y: i32 = -61;

/// The tunnel of the seeds, along x at z = 0, from x = −3 to 6, and its torch, five
/// blocks east of the line: 14 at x = 5, 9 at x = 0, 8 at x = −1 and 7 at x = −2.
const SEEDS_Z: i32 = 0;
const SEEDS_TORCH_X: i32 = 5;
/// The tunnel of the mushrooms, along x at z = 4, from x = −3 to 3, and its torch on
/// the line's east side: 13 at x = −1 and 1, 12 at x = −2 and 2.
const MUSHROOMS_Z: i32 = 4;
const MUSHROOMS_TORCH_X: i32 = 0;
const TUNNEL_WEST: i32 = -3;
const SEEDS_EAST: i32 = 6;
const MUSHROOMS_EAST: i32 = 3;

/// Where the player stands for the tunnels: in chunk (0, 0), within reach of both ends
/// of both tunnels, on the grass between them.
const WORKPLACE: (f64, f64) = (1.5, 2.0);

/// The hotbar slots the tests use.
const STONE_SLOT: i16 = 0;
const FARMLAND_SLOT: i16 = 1;
const SEEDS_SLOT: i16 = 2;
const TORCH_SLOT: i16 = 3;
const MUSHROOM_SLOT: i16 = 4;
const DIRT_SLOT: i16 = 5;

/// A world divided at chunk x = 0 and 4, whose regions stay as they are.
fn pinned() -> Config {
    Config {
        pins: vec![0, 4],
        follow: None,
        ..config()
    }
}

/// A world that is one home region whatever its players do.
fn undivided() -> Config {
    Config {
        pins: Vec::new(),
        follow: None,
        ..config()
    }
}

/// Joins, waits for the chunks around the spawn point and fills the hotbar.
async fn join(address: &str, name: &str) -> Bot {
    let mut bot = Bot::join(address, name).await.unwrap();
    let count = view_area((0, 0), VIEW_DISTANCE).len();
    bot.wait_for_chunks(count, PATIENCE).await.unwrap();
    for (slot, item) in [
        (STONE_SLOT, items::STONE),
        (FARMLAND_SLOT, items::FARMLAND),
        (SEEDS_SLOT, items::WHEAT_SEEDS),
        (TORCH_SLOT, items::TORCH),
        (MUSHROOM_SLOT, items::RED_MUSHROOM),
        (DIRT_SLOT, items::DIRT),
    ] {
        bot.put_in_hotbar(slot, ItemStack::new(item, 64))
            .await
            .unwrap();
    }
    bot
}

/// Waits until `bot` knows the block at `position` to be `state`.
async fn sees_block(bot: &mut Bot, position: (i32, i32, i32), state: Option<i32>) {
    let (x, y, z) = position;
    bot.wait_until(PATIENCE, |bot| bot.block_at(x, y, z).unwrap() == state)
        .await
        .unwrap_or_else(|error| {
            panic!(
                "{error}: the block at {position:?} is {:?}, not {state:?}",
                bot.block_at(x, y, z)
            )
        });
}

/// Waits until the server has acknowledged the action numbered `sequence`.
async fn acknowledged(bot: &mut Bot, sequence: i32) {
    bot.wait_until(PATIENCE, |bot| bot.acknowledged_sequence >= sequence)
        .await
        .unwrap();
}

/// Breaks the block at `position` and waits until it is gone.
async fn dig(bot: &mut Bot, position: (i32, i32, i32)) {
    let (x, y, z) = position;
    let sequence = bot.dig(x, y, z).await.unwrap();
    acknowledged(bot, sequence).await;
    sees_block(bot, position, AIR).await;
}

/// Clicks the face `side` of the block at `against` with what is in `slot`, and returns
/// once the server has acknowledged it.
async fn place(bot: &mut Bot, slot: i16, against: (i32, i32, i32), side: u8) {
    bot.select_slot(slot).await.unwrap();
    let (x, y, z) = against;
    let sequence = bot.use_item_on(x, y, z, side).await.unwrap();
    acknowledged(bot, sequence).await;
}

/// Waits until whatever the region west of the line did before now has reached `bot`:
/// a block placed there and broken again, which the bot sees after anything that
/// region changed earlier. So a placement that was refused is known to have been, and
/// not merely to be on its way.
async fn west_is_settled(bot: &mut Bot) {
    let marker = (-2, TOP_Y + 1, 2);
    place(bot, STONE_SLOT, (-2, TOP_Y, 2), face::TOP).await;
    sees_block(bot, marker, STONE).await;
    dig(bot, marker).await;
}

/// Digs a tunnel along x at `z` from `west` to `east` at the height [`TUNNEL_Y`], and
/// covers it with stone at the grass's height, so that it is sealed.
async fn dig_tunnel(bot: &mut Bot, z: i32, west: i32, east: i32) {
    for x in west..=east {
        dig(bot, (x, TOP_Y, z)).await;
        dig(bot, (x, TUNNEL_Y, z)).await;
    }
    for x in west..=east {
        // Against the grass to the south, which is left as it is.
        place(bot, STONE_SLOT, (x, TOP_Y, z + 1), face::NORTH).await;
        sees_block(bot, (x, TOP_Y, z), STONE).await;
    }
}

/// Makes the floor of the tunnel at `x`, `z` farmland, as it is not where it was laid
/// before: farmland with nothing on it and no water about turns to dirt at a random
/// tick.
async fn lay_farmland(bot: &mut Bot, x: i32, z: i32) {
    if bot.block_at(x, FLOOR_Y, z).unwrap() == FARMLAND {
        return;
    }
    dig(bot, (x, FLOOR_Y, z)).await;
    place(bot, FARMLAND_SLOT, (x, BEDROCK_Y, z), face::TOP).await;
    sees_block(bot, (x, FLOOR_Y, z), FARMLAND).await;
}

/// Plants wheat seeds in the tunnel at `x`, `z` and says whether they were taken: the
/// region that holds the block decides by its light there.
async fn seeds_taken(bot: &mut Bot, x: i32, z: i32) -> bool {
    lay_farmland(bot, x, z).await;
    place(bot, SEEDS_SLOT, (x, FLOOR_Y, z), face::TOP).await;
    west_is_settled(bot).await;
    let planted = bot.block_at(x, TUNNEL_Y, z).unwrap();
    if planted == WHEAT {
        return true;
    }
    assert_eq!(planted, AIR, "the seeds at x = {x}");
    // Refused for the light, and not because the farmland had gone: a refusal for want
    // of farmland would say nothing of the light.
    assert_eq!(
        bot.block_at(x, FLOOR_Y, z).unwrap(),
        FARMLAND,
        "the farmland under the seeds at x = {x}"
    );
    false
}

/// Puts a red mushroom on the tunnel's floor at `x`, `z` and says whether it was taken.
async fn mushroom_taken(bot: &mut Bot, x: i32, z: i32) -> bool {
    place(bot, MUSHROOM_SLOT, (x, FLOOR_Y, z), face::TOP).await;
    west_is_settled(bot).await;
    let planted = bot.block_at(x, TUNNEL_Y, z).unwrap();
    if planted == RED_MUSHROOM {
        return true;
    }
    assert_eq!(planted, AIR, "the mushroom at x = {x}");
    false
}

/// The light the edge has shown `bot` at each block of a tunnel at `z` from `west` to
/// `east`.
fn edge_light(bot: &Bot, z: i32, west: i32, east: i32) -> Vec<LightAt> {
    (west..=east)
        .map(|x| {
            bot.light_at(x, TUNNEL_Y, z)
                .unwrap()
                .expect("the chunk is held")
        })
        .collect()
}

/// The light of a sealed tunnel from `west` to `east` with a torch at `torch`: no sky,
/// and the torch's 14 less one a block.
fn torchlit(west: i32, east: i32, torch: i32) -> Vec<LightAt> {
    (west..=east)
        .map(|x| LightAt {
            sky: Some(0),
            block: Some((14 - (x - torch).abs()).max(0) as u8),
        })
        .collect()
}

/// The dark: a sealed tunnel without a torch.
fn dark(west: i32, east: i32) -> Vec<LightAt> {
    (west..=east)
        .map(|_| LightAt {
            sky: Some(0),
            block: Some(0),
        })
        .collect()
}

/// What the tunnels showed of the light of the region that holds each probe.
#[derive(Debug, PartialEq)]
struct Probed {
    /// Whether seeds were taken at x = 0, −1 and −2 in the dark, before the torch.
    seeds_in_the_dark: Vec<bool>,
    /// The same with the torch at x = 5: light 9, 8 and 7.
    seeds_by_the_torch: Vec<bool>,
    /// Whether a mushroom was taken at x = −2, −1, 1 and 2 by the torch at x = 0:
    /// light 12, 13, 13 and 12.
    mushrooms: Vec<bool>,
}

/// Has a player join now and waits until they are shown `expected` of a tunnel at `z`
/// from `west` to `east`: the light of the chunk packets, which the edge makes from the
/// blocks as they are, with the light updates since. (Once a client has a chunk, the
/// edge sends it no light for a block's change where the client has the chunks about
/// it and lights the change itself, so a player who has stayed is shown nothing new:
/// only a latecomer reads the edge's light of now. And a chunk whose light is being
/// made again when the latecomer comes is sent with the light it had, and corrected by
/// a light update once its job is done: ADR-0023, section 5, a stale entry being
/// sendable. So the latecomer is waited for, not read at once.)
async fn latecomer_is_shown(
    address: &str,
    name: &str,
    (z, west, east): (i32, i32, i32),
    expected: Vec<LightAt>,
    what: &str,
) {
    let mut latecomer = Bot::join(address, name).await.unwrap();
    let count = view_area((0, 0), VIEW_DISTANCE).len();
    latecomer.wait_for_chunks(count, PATIENCE).await.unwrap();
    let shown = latecomer
        .wait_until(PATIENCE, |bot| edge_light(bot, z, west, east) == expected)
        .await;
    if let Err(error) = shown {
        let light = edge_light(&latecomer, z, west, east);
        assert_eq!(light, expected, "{what}: {error}");
    }
    latecomer.close().await.unwrap();
}

/// Digs the two tunnels in the world of the server at `address`, probes them, checks
/// that a latecomer is sent the tunnels lit as the probes found them, and says what
/// the probes found.
async fn probe_the_tunnels(address: &str) -> Probed {
    let mut bot = join(address, "Prober").await;
    bot.walk_to(WORKPLACE.0, WORKPLACE.1, 0.5).await.unwrap();

    dig_tunnel(&mut bot, SEEDS_Z, TUNNEL_WEST, SEEDS_EAST).await;
    dig_tunnel(&mut bot, MUSHROOMS_Z, TUNNEL_WEST, MUSHROOMS_EAST).await;
    latecomer_is_shown(
        address,
        "Early",
        (SEEDS_Z, TUNNEL_WEST, SEEDS_EAST),
        dark(TUNNEL_WEST, SEEDS_EAST),
        "the sealed tunnel in a latecomer's chunk packets",
    )
    .await;

    let mut seeds_in_the_dark = Vec::new();
    for x in [0, -1, -2] {
        seeds_in_the_dark.push(seeds_taken(&mut bot, x, SEEDS_Z).await);
    }

    // The torch east of the line. Once it is shown, the region west of the line has
    // had it in its copy for at least the tick it took to come round: the seeds planted
    // after are judged by it.
    place(
        &mut bot,
        TORCH_SLOT,
        (SEEDS_TORCH_X, FLOOR_Y, SEEDS_Z),
        face::TOP,
    )
    .await;
    sees_block(&mut bot, (SEEDS_TORCH_X, TUNNEL_Y, SEEDS_Z), TORCH).await;
    let mut seeds_by_the_torch = Vec::new();
    for x in [0, -1, -2] {
        seeds_by_the_torch.push(seeds_taken(&mut bot, x, SEEDS_Z).await);
    }

    place(
        &mut bot,
        TORCH_SLOT,
        (MUSHROOMS_TORCH_X, FLOOR_Y, MUSHROOMS_Z),
        face::TOP,
    )
    .await;
    sees_block(&mut bot, (MUSHROOMS_TORCH_X, TUNNEL_Y, MUSHROOMS_Z), TORCH).await;
    let mut mushrooms = Vec::new();
    for x in [-2, -1, 1, 2] {
        mushrooms.push(mushroom_taken(&mut bot, x, MUSHROOMS_Z).await);
    }

    // Someone who joins now is sent the tunnels' chunks lit as the edge lights them,
    // which is the light the regions judged by.
    latecomer_is_shown(
        address,
        "Late",
        (SEEDS_Z, TUNNEL_WEST, SEEDS_EAST),
        torchlit(TUNNEL_WEST, SEEDS_EAST, SEEDS_TORCH_X),
        "the seeds' tunnel in a latecomer's chunk packets",
    )
    .await;
    latecomer_is_shown(
        address,
        "Later",
        (MUSHROOMS_Z, TUNNEL_WEST, MUSHROOMS_EAST),
        torchlit(TUNNEL_WEST, MUSHROOMS_EAST, MUSHROOMS_TORCH_X),
        "the mushrooms' tunnel in a latecomer's chunk packets",
    )
    .await;

    Probed {
        seeds_in_the_dark,
        seeds_by_the_torch,
        mushrooms,
    }
}

/// What the game decides by the light the tunnels have.
fn as_the_light_says() -> Probed {
    Probed {
        // No light at all: no seeds anywhere.
        seeds_in_the_dark: vec![false, false, false],
        // 9 and 8 take seeds, 7 does not.
        seeds_by_the_torch: vec![true, true, false],
        // 12 keeps a mushroom on a solid block, 13 does not.
        mushrooms: vec![true, false, false, true],
    }
}

/// The plan's 5 and 10 part B: the light a region reads beside a pinned line is the
/// light of one region with the same blocks. The tunnels are probed in a world divided
/// at the line and in one that is a single region, and both find what the light says
/// at the thresholds of 8 and 13, a block either side of each; the probes west of the
/// line are judged by the region there, from its copy of the chunk east of the line
/// that holds the torch.
#[tokio::test]
async fn the_light_beside_a_pinned_line_is_the_light_of_one_region_with_the_same_blocks() {
    let (server, address) = start_with(pinned()).await;
    let divided = probe_the_tunnels(&address).await;
    server.stop().await;

    let (server, address) = start_with(undivided()).await;
    let whole = probe_the_tunnels(&address).await;
    server.stop().await;

    assert_eq!(whole, as_the_light_says(), "in one region");
    assert_eq!(divided, as_the_light_says(), "beside the pinned line");
}

/// The plan's 1.1, 6.3 and 10 part B: a torch on one side of the line lights a crop on
/// the other at once, and a change of light is no update. Seeds are refused at x = −1 in
/// the dark; a torch is placed east of the line, and as soon as it is shown the seeds
/// are taken there, judged by the region west of the line from the torch in its copy. A
/// neighbour's change in light 8 leaves the crop. The torch broken, the crop stays in
/// the dark until a neighbour changes, and then breaks.
#[tokio::test]
async fn a_torch_east_of_a_pinned_line_lights_seeds_west_of_it_and_its_going_breaks_them_only_at_a_change()
 {
    let (server, address) = start_with(pinned()).await;
    let mut bot = join(&address, "Planter").await;
    bot.walk_to(WORKPLACE.0, WORKPLACE.1, 0.5).await.unwrap();
    dig_tunnel(&mut bot, SEEDS_Z, TUNNEL_WEST, SEEDS_EAST).await;

    assert!(
        !seeds_taken(&mut bot, -1, SEEDS_Z).await,
        "seeds in the dark west of the line"
    );
    let torch = (SEEDS_TORCH_X, TUNNEL_Y, SEEDS_Z);
    place(
        &mut bot,
        TORCH_SLOT,
        (SEEDS_TORCH_X, FLOOR_Y, SEEDS_Z),
        face::TOP,
    )
    .await;
    // The first moment the player can know of the torch: it is shown to them.
    sees_block(&mut bot, torch, TORCH).await;
    assert!(
        seeds_taken(&mut bot, -1, SEEDS_Z).await,
        "seeds in light 8 west of the line, planted once the torch east of it is seen"
    );

    // A neighbour's change asks the crop whether it may stay: it may, in light 8. The
    // tunnel is widened by a block beside it, still sealed.
    dig(&mut bot, (-1, TUNNEL_Y, SEEDS_Z + 1)).await;
    west_is_settled(&mut bot).await;
    assert_eq!(bot.block_at(-1, TUNNEL_Y, SEEDS_Z).unwrap(), WHEAT);

    // The torch goes: the crop's light is 0 now, which is no update to it.
    dig(&mut bot, torch).await;
    latecomer_is_shown(
        &address,
        "Late",
        (SEEDS_Z, -1, -1),
        dark(-1, -1),
        "the crop's place in a latecomer's chunk packets once the torch is gone",
    )
    .await;
    west_is_settled(&mut bot).await;
    assert_eq!(
        bot.block_at(-1, TUNNEL_Y, SEEDS_Z).unwrap(),
        WHEAT,
        "a crop in the dark stays until something beside it changes"
    );
    // Something beside it changes: the widened block is filled again.
    place(&mut bot, STONE_SLOT, (-1, FLOOR_Y, SEEDS_Z + 1), face::TOP).await;
    sees_block(&mut bot, (-1, TUNNEL_Y, SEEDS_Z), AIR).await;

    server.stop().await;
}

/// The strip of dirt west of the line that grass reaches only from across it: from
/// z = −12 to 12.
const STRIP: std::ops::RangeInclusive<i32> = -12..=12;

/// The plan's 1.3, 5 and 10 part B: grass east of a pinned line spreads to dirt west of
/// it, by day, as a conditional set the region west of the line makes. The dirt is a
/// strip at x = −1, with stone at x = −2 beside it and stone at both its ends, so no
/// grass west of the line is within a spread's reach of it (one across, three down and
/// one up): only the grass at x = 0 is. The region east of the line decides by its own
/// light above its grass and its copy of the dirt; the region west of it sets the grass.
#[tokio::test]
async fn grass_spreads_across_a_pinned_line_to_dirt_only_it_can_reach() {
    let (server, address) = start_with(pinned()).await;
    let mut bot = join(&address, "Gardener").await;

    let ends = [*STRIP.start() - 1, *STRIP.end() + 1];
    for z in STRIP.clone().chain(ends) {
        bot.walk_to(0.5, f64::from(z) + 0.5, 1.0).await.unwrap();
        // Stone at x = −2, and at the strip's ends at x = −1 too.
        dig(&mut bot, (-2, TOP_Y, z)).await;
        place(&mut bot, STONE_SLOT, (-2, TOP_Y - 1, z), face::TOP).await;
        sees_block(&mut bot, (-2, TOP_Y, z), STONE).await;
        dig(&mut bot, (-1, TOP_Y, z)).await;
        let (slot, block) = if ends.contains(&z) {
            (STONE_SLOT, STONE)
        } else {
            (DIRT_SLOT, DIRT)
        };
        place(&mut bot, slot, (-1, TOP_Y - 1, z), face::TOP).await;
        // The dirt may have taken the grass already, as that is what is tested.
        bot.wait_until(PATIENCE, |bot| {
            let now = bot.block_at(-1, TOP_Y, z).unwrap();
            now == block || (block == DIRT && now == GRASS)
        })
        .await
        .unwrap();
    }
    // Back to the middle of the strip, so that all of it stays in view.
    bot.walk_to(0.5, 0.5, 1.0).await.unwrap();

    let spread = |bot: &Bot| {
        STRIP
            .clone()
            .find(|z| bot.block_at(-1, TOP_Y, *z).unwrap() == GRASS)
    };
    bot.wait_until(GRASS_PATIENCE, |bot| spread(bot).is_some())
        .await
        .unwrap_or_else(|error| panic!("{error}: no grass spread across the line"));
    // Nothing west of the line was grass about the strip, and nothing was refused.
    for z in ends {
        assert_eq!(bot.block_at(-1, TOP_Y, z).unwrap(), STONE);
    }
    for z in STRIP.clone().chain(ends) {
        assert_eq!(bot.block_at(-2, TOP_Y, z).unwrap(), STONE);
    }

    server.stop().await;
}
