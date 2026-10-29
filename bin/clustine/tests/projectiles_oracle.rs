//! Comparisons with the official server of N9's projectiles (`docs/groundwork/n9-plan.md`,
//! section 11, comparisons 2, 3, 4, 7, 9, 13, 14, 15, 16 and 17, with decision 9 of
//! section 18: neither server is driven by commands, so each scene is made by what a
//! creative player can do on both).
//!
//! Each comparison runs one scene on the official server and then on Clustine, each on
//! a fresh flat world, and compares what a client was sent. **Exact where nothing
//! draws**: the place a dispenser shoots from, the sounds and level events, the data,
//! the crossbow's ticks. **By the rule where draws decide**: a shot's speed against its
//! power and the spread of its uncertainty, a rocket's lifetime within its range, the
//! angle a refused arrow is turned by. Ticks are a client's (twentieths of a second
//! between two arrivals) and are held to one either way, as `monsters_oracle.rs` holds
//! them.
//!
//! Comparisons 2, 4 and 15 are written in their bot-only form: what the official
//! server's packets show is held to what the rules say, within the packets'
//! quantisation, on each server, rather than fed into the simulation
//! (`Region::put_entity_for_test` does not exist yet). The path a projectile is shown
//! along is held to the path its kind's tick gives from its add packet
//! ([`n9_scene::off_the_path`]): a wrong gravity, drag or order of the two bends it away
//! within a few ticks.
//!
//! They need Java, the server jar downloaded by `cargo datagen`, and your agreement to
//! the Minecraft EULA, so they are ignored by default. Run them on a quiet machine:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test projectiles_oracle -- --ignored official_server --nocapture \
//!     --test-threads 1 --no-fail-fast
//! ```

mod common;
#[path = "common/n9_scene.rs"]
#[allow(dead_code)]
mod n9_scene;

use std::time::Duration;

use anyhow::{Context, Result};
use clustine_data::{entity_types, items, sounds};
use clustine_protocol::components::{
    Fireworks, PotionContents, WireComponent, WireComponents, WireStack, potion,
};
use clustine_protocol::item::ItemStack;
use clustine_protocol::packets::play::{ClientboundPlay, EntityDataValue, click, face};
use tokio::time::Instant;

use n9_scene::{
    ARROW_CRIT, Flight, GROUND, PATIENCE, Scene, angle_between, off_the_path, on_both, speed,
    ticks, value_at, yaw_of,
};

/// A stack of one `item` with `components`.
fn with(item: i32, components: Vec<WireComponent>) -> ItemStack {
    ItemStack {
        item,
        count: 1,
        components: WireComponents {
            added: components,
            removed: Vec::new(),
        },
    }
}

/// The id of the potion `name` in the `potion` registry.
fn potion_id(name: &str) -> i32 {
    let at = potion::NAMES
        .iter()
        .position(|known| *known == name)
        .expect("a potion of the registry");
    i32::try_from(at).expect("a registry is small")
}

/// One `item` (a potion, a splash or lingering potion, a tipped arrow) of the potion
/// `name`.
fn potion_of(item: i32, name: &str) -> ItemStack {
    with(
        item,
        vec![WireComponent::PotionContents(PotionContents {
            potion: Some(potion_id(name)),
            ..PotionContents::default()
        })],
    )
}

/// A rocket of `flight`, without explosions.
fn rocket(flight: i32) -> ItemStack {
    with(
        items::FIREWORK_ROCKET,
        vec![WireComponent::Fireworks(Fireworks {
            flight_duration: flight,
            explosions: Vec::new(),
        })],
    )
}

/// The projectiles a stack is charged with, if it is a charged crossbow.
fn charged_with(stack: Option<&ItemStack>) -> Option<Vec<WireStack>> {
    stack?
        .components
        .added
        .iter()
        .find_map(|component| match component {
            WireComponent::ChargedProjectiles(stacks) if !stacks.is_empty() => Some(stacks.clone()),
            _ => None,
        })
}

/// When the player's own inventory was first shown a charged crossbow from `from` on,
/// and what it was charged with.
fn charged(scene: &Scene, from: usize) -> Option<(Instant, Vec<WireStack>)> {
    scene
        .of(from, |packet| match packet {
            ClientboundPlay::ContainerSetSlot(set) if set.window_id == 0 => {
                charged_with(set.stack.as_ref())
            }
            ClientboundPlay::SetPlayerInventory(set) => charged_with(set.stack.as_ref()),
            ClientboundPlay::SetContainerContent(content) if content.window_id == 0 => content
                .slots
                .iter()
                .find_map(|slot| charged_with(slot.as_ref())),
            _ => None,
        })
        .into_iter()
        .next()
}

/// The first time `sound` was heard from `from` on.
fn heard(scene: &Scene, from: usize, sound: i32) -> Result<Instant> {
    scene
        .sounds(from)
        .into_iter()
        .find_map(|(at, heard, _)| (heard == sound).then_some(at))
        .with_context(|| format!("{}: the sound {sound}", scene.name))
}

/// Puts `stack` into a dispenser at `at` (its window's first slot) from the hotbar's
/// slot 1, by a shift-click, and closes it.
async fn fill(scene: &mut Scene, at: (i32, i32, i32), stack: ItemStack) -> Result<()> {
    scene.put(1, stack).await?;
    scene.open(at, 8).await?;
    // A dispenser's window: its nine slots, the inventory's 27, then the hotbar.
    scene
        .click(9 + 27 + 1, 0, click::QUICK_MOVE, Vec::new(), None)
        .await?;
    scene
        .until(PATIENCE, "the dispenser filled", |scene| {
            scene
                .window
                .as_ref()
                .is_some_and(|window| window.slots.iter().take(9).any(Option::is_some))
        })
        .await?;
    scene.close_window().await?;
    scene.round_trip().await
}

/// Places a dispenser on the grass at `(x, z)` while looking as `look` says, which
/// gives its facing (the opposite of the nearest direction looked along).
async fn dispenser(scene: &mut Scene, (x, z): (i32, i32), look: (f32, f32)) -> Result<i32> {
    scene.turn(look.0, look.1).await?;
    let at = (x, GROUND + 1, z);
    scene
        .place(items::DISPENSER, (x, GROUND, z), face::TOP, at)
        .await?;
    scene.block(at).context("the dispenser's state")
}

/// Powers whatever is beside `(x, z)` by a redstone block placed on the grass there.
async fn power(scene: &mut Scene, (x, z): (i32, i32)) -> Result<()> {
    scene
        .place(
            items::REDSTONE_BLOCK,
            (x, GROUND, z),
            face::TOP,
            (x, GROUND + 1, z),
        )
        .await
}

// --- Comparison 7: the crossbow's load ---------------------------------------------

/// What a crossbow held and loaded showed, by ticks from the loading's first sound.
#[derive(Debug)]
struct Loading {
    start_to_middle: i64,
    start_to_charged: i64,
    end_to_charged: i64,
    charged: Vec<WireStack>,
}

async fn load_a_crossbow(address: &str) -> Result<Loading> {
    let mut scene = Scene::join(address, "Loader").await?;
    scene.take(ItemStack::new(items::CROSSBOW, 1)).await?;
    scene.round_trip().await?;
    let from = scene.mark();
    scene.use_item(0.0, 0.0).await?;
    scene
        .until(PATIENCE, "the crossbow charged", |scene| {
            charged(scene, from).is_some()
        })
        .await?;
    scene.release().await?;
    scene.round_trip().await?;
    let start = heard(&scene, from, sounds::ITEM_CROSSBOW_LOADING_START)?;
    let middle = heard(&scene, from, sounds::ITEM_CROSSBOW_LOADING_MIDDLE)?;
    let end = heard(&scene, from, sounds::ITEM_CROSSBOW_LOADING_END)?;
    let (charged_at, charged) = charged(&scene, from).context("the charge")?;
    Ok(Loading {
        start_to_middle: ticks(start, middle),
        start_to_charged: ticks(start, charged_at),
        end_to_charged: ticks(end, charged_at),
        charged,
    })
}

/// Comparison 7: a crossbow held in use plays its loading sounds at the 6th and 14th
/// tick of the use (progress 0.2 and 0.5 of the 25 ticks' charge) and is charged on
/// the 26th, with the end's sound: eight ticks from the first sound to the second and
/// twenty to the charge, each held to one.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_crossbow_loads_on_its_ticks_like_on_the_official_server() {
    let (theirs, ours) = on_both(load_a_crossbow).await;
    println!("official {theirs:?}\nClustine {ours:?}");
    let theirs = theirs.expect("the official server's crossbow");
    let ours = ours.expect("Clustine's crossbow");
    for (name, loading) in [("the official server", &theirs), ("Clustine", &ours)] {
        assert!(
            (7..=9).contains(&loading.start_to_middle),
            "{name}: {loading:?}"
        );
        assert!(
            (19..=21).contains(&loading.start_to_charged),
            "{name}: {loading:?}"
        );
        assert!(loading.end_to_charged.abs() <= 1, "{name}: {loading:?}");
        assert_eq!(loading.charged.len(), 1, "{name}: one arrow");
        assert_eq!(loading.charged[0].item, items::ARROW, "{name}");
    }
    assert!(theirs.start_to_middle.abs_diff(ours.start_to_middle) <= 1);
    assert!(theirs.start_to_charged.abs_diff(ours.start_to_charged) <= 1);
    assert_eq!(
        ours.charged, theirs.charged,
        "the charged arrow's components"
    );
}

// --- Comparison 13: the bow's draw -------------------------------------------------

/// `BowItem.getPowerForTime`.
fn power_for(ticks: u32) -> f64 {
    let f = f64::from(ticks) / 20.0;
    ((f * f + f * 2.0) / 3.0).min(1.0)
}

/// The draw, in ticks, whose shot's speed (`3 · power`) is nearest `speed`; the first
/// of those at full power for a full draw.
fn drawn_as(speed: f64) -> u32 {
    (1..=30)
        .min_by(|a, b| {
            let off = |ticks: u32| (speed / (3.0 * power_for(ticks))).ln().abs();
            off(*a).total_cmp(&off(*b))
        })
        .expect("a draw")
}

/// An arrow loosed from a bow.
#[derive(Debug, Clone, Copy)]
struct Loosed {
    speed: f64,
    crit: bool,
    drawn_as: u32,
}

async fn loose(scene: &mut Scene, draw: u64) -> Result<Loosed> {
    scene.take(ItemStack::new(items::BOW, 1)).await?;
    scene.round_trip().await?;
    let from = scene.mark();
    scene.use_item(0.0, 0.0).await?;
    scene.idle_ticks(draw).await?;
    scene.release().await?;
    let arrows = scene
        .wait_added(from, entity_types::ARROW, 1, PATIENCE)
        .await?;
    let (_, add) = arrows.into_iter().next().context("the arrow")?;
    scene.round_trip().await?;
    let track = scene.track(add.entity_id);
    let crit = track.data.iter().any(|(_, values)| {
        matches!(value_at(values, 8), Some(EntityDataValue::Byte(flags)) if flags & ARROW_CRIT != 0)
    });
    let speed = speed(add.velocity);
    Ok(Loosed {
        speed,
        crit,
        drawn_as: drawn_as(speed),
    })
}

/// The draws compared: five and ten ticks, and a full one.
const DRAWS: [u64; 3] = [5, 10, 25];

async fn draw_a_bow(address: &str) -> Result<Vec<Loosed>> {
    let mut scene = Scene::join(address, "Archer").await?;
    let mut loosed = Vec::new();
    for draw in DRAWS {
        loosed.push(loose(&mut scene, draw).await?);
    }
    Ok(loosed)
}

/// Comparison 13 (finding 54): a bow drawn for 5, 10 and 25 ticks shoots at `3 ·
/// getPowerForTime(t)` within the spread of an uncertainty of 1 (a few hundredths), so
/// the speed tells the draw to the tick; the draw a server shot is held to one tick of
/// the client's (whose clock says when it let go) and to one of the other server's.
/// Only the full draw is a crit.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_bows_draw_gives_its_speed_and_crit_like_on_the_official_server() {
    let (theirs, ours) = on_both(draw_a_bow).await;
    println!("official {theirs:?}\nClustine {ours:?}");
    let theirs = theirs.expect("the official server's bow");
    let ours = ours.expect("Clustine's bow");
    for (name, shots) in [("the official server", &theirs), ("Clustine", &ours)] {
        for (draw, shot) in DRAWS.iter().zip(shots) {
            let full = *draw >= 20;
            assert_eq!(shot.crit, full, "{name}: drawn {draw}: {shot:?}");
            if full {
                assert_eq!(shot.drawn_as, 20, "{name}: a full draw: {shot:?}");
                assert!(
                    (2.85..=3.15).contains(&shot.speed),
                    "{name}: a full draw: {shot:?}"
                );
            } else {
                assert!(
                    u64::from(shot.drawn_as).abs_diff(*draw) <= 1,
                    "{name}: drawn {draw}: {shot:?}"
                );
            }
        }
    }
    for ((theirs, ours), draw) in theirs.iter().zip(&ours).zip(DRAWS) {
        assert!(
            theirs.drawn_as.abs_diff(ours.drawn_as) <= 1,
            "drawn {draw}: official {theirs:?}, Clustine {ours:?}"
        );
    }
}

// --- Comparison 14: Multishot ------------------------------------------------------

#[derive(Debug)]
struct Multishot {
    charged: usize,
    /// Each arrow's yaw less the look's, sorted.
    angles: Vec<f64>,
    speeds: Vec<f64>,
    shot_sounds: usize,
}

async fn shoot_multishot(address: &str) -> Result<Multishot> {
    let mut scene = Scene::join(address, "Multishooter").await?;
    let multishot = scene.registered("minecraft:enchantment", "minecraft:multishot")?;
    let crossbow = with(
        items::CROSSBOW,
        vec![WireComponent::Enchantments(vec![(multishot, 1)])],
    );
    scene.take(crossbow).await?;
    scene.round_trip().await?;
    let from = scene.mark();
    scene.use_item(0.0, 0.0).await?;
    scene
        .until(PATIENCE, "the crossbow charged", |scene| {
            charged(scene, from).is_some()
        })
        .await?;
    scene.release().await?;
    scene.round_trip().await?;
    let (_, loaded) = charged(&scene, from).context("the charge")?;
    let shot = scene.mark();
    scene.use_item(0.0, 0.0).await?;
    let arrows = scene
        .wait_added(shot, entity_types::ARROW, 3, PATIENCE)
        .await?;
    scene.round_trip().await?;
    let mut angles: Vec<f64> = arrows
        .iter()
        .map(|(_, add)| angle_between(yaw_of(add.velocity), 0.0))
        .collect();
    angles.sort_by(f64::total_cmp);
    Ok(Multishot {
        charged: loaded.len(),
        angles,
        speeds: arrows.iter().map(|(_, add)| speed(add.velocity)).collect(),
        shot_sounds: scene
            .sounds(shot)
            .iter()
            .filter(|(_, sound, _)| *sound == sounds::ITEM_CROSSBOW_SHOOT)
            .count(),
    })
}

/// Comparison 14: a crossbow with Multishot I loads three arrows and shoots them at
/// 0, −10 and +10 degrees from the look, each within the spread of an uncertainty of 1
/// (held to two degrees), at the crossbow's 3.15 within a few hundredths, with a sound
/// for each.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn multishot_shoots_three_arrows_like_on_the_official_server() {
    let (theirs, ours) = on_both(shoot_multishot).await;
    println!("official {theirs:?}\nClustine {ours:?}");
    let theirs = theirs.expect("the official server's crossbow");
    let ours = ours.expect("Clustine's crossbow");
    for (name, shot) in [("the official server", &theirs), ("Clustine", &ours)] {
        assert_eq!(shot.charged, 3, "{name}: {shot:?}");
        assert_eq!(shot.angles.len(), 3, "{name}: {shot:?}");
        for (angle, wanted) in shot.angles.iter().zip([-10.0, 0.0, 10.0]) {
            assert!((angle - wanted).abs() <= 2.0, "{name}: {shot:?}");
        }
        for speed in &shot.speeds {
            assert!((3.0..=3.3).contains(speed), "{name}: {shot:?}");
        }
        assert_eq!(shot.shot_sounds, 3, "{name}: {shot:?}");
    }
}

// --- Comparison 16: a rocket's lifetime --------------------------------------------

/// The rockets of each flight launched.
const ROCKETS_EACH: usize = 6;

async fn launch_rockets(address: &str) -> Result<Vec<(i32, i64)>> {
    let mut scene = Scene::join(address, "Launcher").await?;
    let (x, z) = scene.column();
    let spots = [(2, 0), (2, 2), (0, 2), (-2, 2), (-2, 0), (-2, -2)];
    let from = scene.mark();
    for flight in 1..=3 {
        scene.take(rocket(flight)).await?;
        for (dx, dz) in spots.iter().take(ROCKETS_EACH) {
            scene.use_on(x + dx, GROUND, z + dz, face::TOP).await?;
        }
        scene.round_trip().await?;
    }
    let launched = scene
        .wait_added(
            from,
            entity_types::FIREWORK_ROCKET,
            3 * ROCKETS_EACH,
            PATIENCE,
        )
        .await?;
    let ids: Vec<i32> = launched.iter().map(|(_, add)| add.entity_id).collect();
    scene
        .until(
            Duration::from_secs(20),
            "every rocket's explosion",
            |scene| {
                ids.iter()
                    .all(|id| scene.track(*id).first_event(17).is_some())
            },
        )
        .await?;
    // Each flight's rockets were launched after the last's had been handled, so the
    // adds come in the flights' order; the stack the data shows is comparison 9's.
    ids.iter()
        .enumerate()
        .map(|(launched, id)| {
            let track = scene.track(*id);
            let (added, _) = track.add.clone().context("the rocket's add")?;
            let flight = 1 + i32::try_from(launched / ROCKETS_EACH).expect("few rockets");
            let exploded = track.first_event(17).context("the explosion")?;
            Ok((flight, ticks(added, exploded)))
        })
        .collect()
}

/// Comparison 16: a rocket of flight `f` used on the grass lives `10 · (1 + f) +
/// nextInt(6) + nextInt(7)` ticks, from its add packet to its entity event 17: each
/// within that range (and a tick of the clock either way), and each flight's mean on
/// the two servers within three ticks (the draws' sum has a deviation of 2.6 ticks,
/// so the means of six differ by 1.5 at one deviation of their difference; three is
/// two of those, with the clock's tick besides).
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_rockets_lifetime_is_drawn_like_on_the_official_server() {
    let (theirs, ours) = on_both(launch_rockets).await;
    println!("official {theirs:?}\nClustine {ours:?}");
    let theirs = theirs.expect("the official server's rockets");
    let ours = ours.expect("Clustine's rockets");
    let mut means = Vec::new();
    for (name, lives) in [("the official server", &theirs), ("Clustine", &ours)] {
        let mut by_flight = Vec::new();
        for flight in 1..=3 {
            let of: Vec<i64> = lives
                .iter()
                .filter(|(of, _)| *of == flight)
                .map(|(_, life)| *life)
                .collect();
            assert_eq!(of.len(), ROCKETS_EACH, "{name}: flight {flight}: {lives:?}");
            let least = 10 * (1 + i64::from(flight));
            for life in &of {
                assert!(
                    (least - 1..=least + 12).contains(life),
                    "{name}: flight {flight} lived {life}: {lives:?}"
                );
            }
            by_flight.push(of.iter().sum::<i64>() as f64 / of.len() as f64);
        }
        means.push(by_flight);
    }
    for (flight, (theirs, ours)) in means[0].iter().zip(&means[1]).enumerate() {
        assert!(
            (theirs - ours).abs() <= 3.0,
            "flight {}: official mean {theirs}, Clustine's {ours}",
            flight + 1,
        );
    }
}

// --- Comparison 17: a dispenser's shot of each kind --------------------------------

/// What a dispenser facing up was shown shooting.
#[derive(Debug, Clone)]
struct Dispensed {
    state: i32,
    kind: i32,
    /// The add packet's place less the dispenser's lowest corner.
    offset: [f64; 3],
    speed: f64,
    upward: bool,
    /// The level events at the dispenser, with their data.
    events: Vec<(i32, i32)>,
}

/// What each dispenser is filled with, the kind it shoots, and its power (the plan's
/// 4.9 table).
fn dispensed_kinds() -> Vec<(ItemStack, i32, f64)> {
    vec![
        (ItemStack::new(items::ARROW, 16), entity_types::ARROW, 1.1),
        (ItemStack::new(items::EGG, 16), entity_types::EGG, 1.1),
        (
            ItemStack::new(items::SNOWBALL, 16),
            entity_types::SNOWBALL,
            1.1,
        ),
        (
            potion_of(items::SPLASH_POTION, "minecraft:water"),
            entity_types::SPLASH_POTION,
            1.375,
        ),
        (
            ItemStack::new(items::EXPERIENCE_BOTTLE, 16),
            entity_types::EXPERIENCE_BOTTLE,
            1.375,
        ),
        (rocket(1), entity_types::FIREWORK_ROCKET, 0.5),
    ]
}

async fn dispense_each(address: &str) -> Result<Vec<Dispensed>> {
    let mut scene = Scene::join(address, "Dispensing").await?;
    let (x, z) = scene.column();
    // Two rows of three, two blocks apart, each with its redstone block placed on its
    // far side, beside no other dispenser.
    let spots = [
        ((x - 2, z + 2), (x - 2, z + 3)),
        ((x, z + 2), (x, z + 3)),
        ((x + 2, z + 2), (x + 2, z + 3)),
        ((x - 2, z - 2), (x - 2, z - 3)),
        ((x, z - 2), (x, z - 3)),
        ((x + 2, z - 2), (x + 2, z - 3)),
    ];
    let kinds = dispensed_kinds();
    let mut states = Vec::new();
    for ((at, _), (stack, _, _)) in spots.iter().zip(&kinds) {
        // Looking straight down, the dispenser faces up.
        states.push(dispenser(&mut scene, *at, (0.0, 90.0)).await?);
        fill(&mut scene, (at.0, GROUND + 1, at.1), stack.clone()).await?;
    }
    let mut shots = Vec::new();
    for (((at, beside), (_, kind, _)), state) in spots.iter().zip(&kinds).zip(states) {
        let from = scene.mark();
        power(&mut scene, *beside).await?;
        let added = scene.wait_added(from, *kind, 1, PATIENCE).await?;
        let (_, add) = added.into_iter().next().context("the shot")?;
        scene.idle_ticks(10).await?;
        let events = scene
            .level_events(from)
            .into_iter()
            .filter(|(_, event)| {
                (event.position.x, event.position.y, event.position.z) == (at.0, GROUND + 1, at.1)
            })
            .map(|(_, event)| (event.event, event.data))
            .collect();
        shots.push(Dispensed {
            state,
            kind: add.kind,
            offset: [
                add.x - f64::from(at.0),
                add.y - f64::from(GROUND + 1),
                add.z - f64::from(at.1),
            ],
            speed: speed(add.velocity),
            upward: add.velocity[1] > 0.0,
            events,
        });
    }
    Ok(shots)
}

/// Comparison 17: six dispensers facing up, each with one kind (arrows, eggs,
/// snowballs, a splash potion, a bottle, a rocket), powered by a redstone block: the
/// projectile's kind, its place to the bit (no draw puts it there), its speed within
/// its power's spread (uncertainty 6 for the default, 3 for the potions, 1 for the
/// rocket: held to a fifth), and the level events at the dispenser (1002 for a shot,
/// 1004 for the rocket, with the smoke's 2000) the same on both.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_dispenser_shoots_each_kind_like_on_the_official_server() {
    let (theirs, ours) = on_both(dispense_each).await;
    println!("official {theirs:#?}\nClustine {ours:#?}");
    let theirs = theirs.expect("the official server's dispensers");
    let ours = ours.expect("Clustine's dispensers");
    let kinds = dispensed_kinds();
    for (name, shots) in [("the official server", &theirs), ("Clustine", &ours)] {
        assert_eq!(shots.len(), kinds.len(), "{name}");
        for (shot, (_, kind, power)) in shots.iter().zip(&kinds) {
            assert_eq!(shot.kind, *kind, "{name}: {shot:?}");
            assert!(shot.upward, "{name}: {shot:?}");
            assert!(
                (power * 0.8..=power * 1.2).contains(&shot.speed),
                "{name}: speed against the power {power}: {shot:?}"
            );
            let sound = if *kind == entity_types::FIREWORK_ROCKET {
                1004
            } else {
                1002
            };
            assert!(
                shot.events.iter().any(|(event, _)| *event == sound),
                "{name}: the level event {sound}: {shot:?}"
            );
        }
    }
    for (theirs, ours) in theirs.iter().zip(&ours) {
        assert_eq!(ours.state, theirs.state, "the dispenser's facing");
        assert_eq!(
            ours.offset, theirs.offset,
            "where {} comes out",
            theirs.kind
        );
        assert_eq!(
            ours.events, theirs.events,
            "the level events of {}",
            theirs.kind
        );
    }
}

// --- Comparison 3: a refused arrow -------------------------------------------------

#[derive(Debug)]
struct Refused {
    /// The motion after the refusal against the motion it was shot with, as a share of
    /// its length, negative: from the first motion sent against the shot's; none where
    /// no such motion was sent.
    turned_back_by: Option<f64>,
    /// Whether its places show it coming back, motion sent or not.
    came_back: bool,
    /// The yaw after the refusal less the yaw before, in degrees.
    turned_by: Option<f64>,
    gone: bool,
    landed: bool,
}

async fn refuse_an_arrow(address: &str) -> Result<Refused> {
    let mut scene = Scene::join(address, "Target").await?;
    let (x, z) = scene.column();
    let at = (x - 3, z);
    // Looking west, the dispenser faces east, at the player.
    dispenser(&mut scene, at, (90.0, 0.0)).await?;
    fill(
        &mut scene,
        (at.0, GROUND + 1, at.1),
        ItemStack::new(items::ARROW, 16),
    )
    .await?;
    scene
        .stand_at(
            f64::from(at.0) + 3.5,
            f64::from(GROUND + 1),
            f64::from(at.1) + 0.5,
        )
        .await?;
    scene.round_trip().await?;
    let from = scene.mark();
    power(&mut scene, (at.0, at.1 - 1)).await?;
    let added = scene
        .wait_added(from, entity_types::ARROW, 1, PATIENCE)
        .await?;
    let (_, add) = added.into_iter().next().context("the arrow")?;
    let id = add.entity_id;
    let before = add.velocity;
    let back = |scene: &Scene| {
        scene
            .track(id)
            .motions
            .iter()
            .skip(1)
            .find_map(|(at, motion)| {
                let dot: f64 = (0..3).map(|axis| motion[axis] * before[axis]).sum();
                (dot < 0.0).then_some((*at, *motion))
            })
    };
    // Where the motion is not sent, the places still show the arrow coming back.
    let came_back = |scene: &Scene| {
        scene.track(id).places.windows(2).any(|pair| {
            let dot: f64 = (0..3)
                .map(|axis| (pair[1].1[axis] - pair[0].1[axis]) * before[axis])
                .sum();
            dot < 0.0
        })
    };
    let _ = scene
        .until(Duration::from_secs(5), "the arrow turned back", |scene| {
            back(scene).is_some() || came_back(scene)
        })
        .await;
    let landed = scene
        .until(
            Duration::from_secs(10),
            "the arrow in the ground",
            |scene| {
                scene
                    .track(id)
                    .first_data(10, |value| *value == EntityDataValue::Boolean(true))
                    .is_some()
            },
        )
        .await
        .is_ok();
    let track = scene.track(id);
    let turned = back(&scene);
    let yaw_before = track.yaws.first().map(|(_, yaw)| f64::from(*yaw));
    let yaw_after = turned.and_then(|(turned_at, _)| {
        track
            .yaws
            .iter()
            .find(|(at, _)| *at >= turned_at)
            .map(|(_, yaw)| f64::from(*yaw))
    });
    Ok(Refused {
        turned_back_by: turned.map(|(_, after)| -speed(after) / speed(before)),
        came_back: came_back(&scene),
        turned_by: yaw_before
            .zip(yaw_after)
            .map(|(before, after)| angle_between(after, before)),
        gone: track.removed.is_some(),
        landed,
    })
}

/// Comparison 3: an arrow a dispenser shoots at a creative player is refused (a
/// creative player takes no hurt): its motion times −0.1 (held to a share between
/// 0.08 and 0.115 of the motion it was shot with, which the two ticks' drag and
/// gravity before the hit and the packets' quantisation move), its yaw turned by 170
/// to 190 degrees (held to the quantisation of a byte, 1.4 degrees, beyond), the arrow
/// living on and falling to the ground.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn an_arrow_refused_by_a_creative_player_turns_back_like_on_the_official_server() {
    let (theirs, ours) = on_both(refuse_an_arrow).await;
    println!("official {theirs:?}\nClustine {ours:?}");
    let theirs = theirs.expect("the official server's arrow");
    let ours = ours.expect("Clustine's arrow");
    for (name, refused) in [("the official server", &theirs), ("Clustine", &ours)] {
        assert!(refused.came_back, "{name}: {refused:?}");
        assert!(
            refused
                .turned_back_by
                .is_some_and(|share| (-0.115..=-0.08).contains(&share)),
            "{name}: the motion turned back: {refused:?}"
        );
        assert!(
            refused
                .turned_by
                .is_some_and(|turned| turned.abs() >= 168.5),
            "{name}: the yaw turned: {refused:?}"
        );
        assert!(!refused.gone, "{name}: the arrow lives on: {refused:?}");
        assert!(refused.landed, "{name}: the arrow falls: {refused:?}");
    }
}

// --- Comparison 9: the wire of each projectile -------------------------------------

/// What a client is shown of a projectile, as compared.
#[derive(Debug, Clone, PartialEq)]
struct Shown {
    kind: i32,
    /// The add packet's data against the shooter's id.
    owner: String,
    moving: bool,
    /// The first data sent with it, by index.
    data: Vec<(u8, EntityDataValue)>,
    /// The ticks, by the median, between the places sent after the add, and between
    /// the motions; none where fewer than two came.
    place_interval: Option<i64>,
    motion_interval: Option<i64>,
}

/// The add packet's data `data` as said of a shooter `me`.
fn owner(data: i32, me: i32) -> String {
    match data {
        0 => "none".to_owned(),
        _ if data == me => "the shooter".to_owned(),
        _ if data == me + 1 => "the shooter plus one".to_owned(),
        other => format!("{other}"),
    }
}

/// The median of the ticks between the times `times`.
fn interval(times: &[Instant]) -> Option<i64> {
    let mut gaps: Vec<i64> = times
        .windows(2)
        .map(|pair| ticks(pair[0], pair[1]))
        .collect();
    if gaps.is_empty() {
        return None;
    }
    gaps.sort_unstable();
    Some(gaps[gaps.len() / 2])
}

fn shown(scene: &Scene, id: i32) -> Result<Shown> {
    let track = scene.track(id);
    let (_, add) = track.add.clone().context("the add packet")?;
    let mut data = track
        .data
        .first()
        .map(|(_, values)| values.clone())
        .unwrap_or_default();
    data.sort_by_key(|(index, _)| *index);
    let places: Vec<Instant> = track.places.iter().skip(1).map(|(at, _, _)| *at).collect();
    let motions: Vec<Instant> = track.motions.iter().skip(1).map(|(at, _)| *at).collect();
    Ok(Shown {
        kind: add.kind,
        owner: owner(add.data, scene.me),
        moving: add.velocity != [0.0; 3],
        data,
        place_interval: interval(&places),
        motion_interval: interval(&motions),
    })
}

/// How a projectile of the comparison is made.
#[derive(Debug, Clone)]
enum Made {
    /// A bow drawn full, with this stack in the off hand.
    Bow(Option<ItemStack>),
    /// A trident thrown.
    Trident,
    /// This stack used, looking up a little.
    Thrown(ItemStack),
    /// This stack used on the grass.
    UsedOn(ItemStack),
}

/// The projectiles of comparison 9 a player can make, with their kinds; a lingering
/// potion's cloud comes with its potion. The pearl, which moves the player, is last.
fn wired() -> Vec<(Made, i32)> {
    vec![
        (Made::Bow(None), entity_types::ARROW),
        (
            Made::Bow(Some(ItemStack::new(items::SPECTRAL_ARROW, 1))),
            entity_types::SPECTRAL_ARROW,
        ),
        (Made::Trident, entity_types::TRIDENT),
        (
            Made::Thrown(ItemStack::new(items::SNOWBALL, 1)),
            entity_types::SNOWBALL,
        ),
        (
            Made::Thrown(ItemStack::new(items::EGG, 1)),
            entity_types::EGG,
        ),
        (
            Made::Thrown(potion_of(items::SPLASH_POTION, "minecraft:swiftness")),
            entity_types::SPLASH_POTION,
        ),
        (
            Made::Thrown(potion_of(items::LINGERING_POTION, "minecraft:swiftness")),
            entity_types::LINGERING_POTION,
        ),
        (
            Made::Thrown(ItemStack::new(items::EXPERIENCE_BOTTLE, 1)),
            entity_types::EXPERIENCE_BOTTLE,
        ),
        (Made::UsedOn(rocket(1)), entity_types::FIREWORK_ROCKET),
        (
            Made::Thrown(ItemStack::new(items::ENDER_PEARL, 1)),
            entity_types::ENDER_PEARL,
        ),
    ]
}

async fn make(scene: &mut Scene, made: &Made) -> Result<()> {
    match made {
        Made::Bow(off_hand) => {
            scene.put_slot(n9_scene::OFF_HAND, off_hand.clone()).await?;
            scene.take(ItemStack::new(items::BOW, 1)).await?;
            scene.round_trip().await?;
            scene.use_item(0.0, -30.0).await?;
            scene.idle_ticks(25).await?;
            scene.release().await?;
            // Handled after the release, which reads the off hand.
            scene.put_slot(n9_scene::OFF_HAND, None).await?;
        }
        Made::Trident => {
            scene.take(ItemStack::new(items::TRIDENT, 1)).await?;
            scene.round_trip().await?;
            scene.use_item(0.0, -30.0).await?;
            scene.idle_ticks(15).await?;
            scene.release().await?;
        }
        Made::Thrown(stack) => {
            scene.take(stack.clone()).await?;
            scene.round_trip().await?;
            scene.use_item(0.0, -30.0).await?;
        }
        Made::UsedOn(stack) => {
            scene.take(stack.clone()).await?;
            let (x, z) = scene.column();
            scene.use_on(x + 2, GROUND, z, face::TOP).await?;
        }
    }
    Ok(())
}

async fn show_each(address: &str) -> Result<Vec<Shown>> {
    let mut scene = Scene::join(address, "Shooter").await?;
    let mut shown_each = Vec::new();
    for (made, kind) in wired() {
        let from = scene.mark();
        make(&mut scene, &made).await?;
        let added = scene.wait_added(from, kind, 1, PATIENCE).await?;
        let (_, add) = added.into_iter().next().context("the projectile")?;
        // Its flight, or as much of it as three seconds show.
        let id = add.entity_id;
        let _ = scene
            .until(Duration::from_secs(3), "the projectile gone", |scene| {
                scene.track(id).removed.is_some()
            })
            .await;
        shown_each.push(shown(&scene, id)?);
        if kind == entity_types::LINGERING_POTION {
            let clouds = scene
                .wait_added(from, entity_types::AREA_EFFECT_CLOUD, 1, PATIENCE)
                .await?;
            let (_, cloud) = clouds.into_iter().next().context("the cloud")?;
            scene.idle_ticks(40).await?;
            shown_each.push(shown(&scene, cloud.entity_id)?);
        }
    }
    Ok(shown_each)
}

/// Comparison 9: each projectile a player makes (an arrow, a spectral arrow, a trident,
/// each thrown kind, a rocket, a lingering potion's cloud) is added with the same kind,
/// the same data in the add packet (the shooter), motion or none, the same entity data
/// to the value, and its places and motions sent at the same interval (the type's
/// update interval: 20 for arrows and tridents, 10 for thrown items and rockets, none
/// for the cloud), held to a tick. The fangs are in `effects_oracle.rs`, with the
/// evoker; the llama's spit is the next test.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn each_projectile_is_sent_like_on_the_official_server() {
    let (theirs, ours) = on_both(show_each).await;
    println!("official {theirs:#?}\nClustine {ours:#?}");
    let theirs = theirs.expect("the official server's projectiles");
    let ours = ours.expect("Clustine's projectiles");
    assert_eq!(ours.len(), theirs.len());
    let mut differ = Vec::new();
    for (theirs, ours) in theirs.iter().zip(&ours) {
        let name = clustine_data::ENTITY_TYPES[theirs.kind as usize];
        let close = |a: Option<i64>, b: Option<i64>| match (a, b) {
            (Some(a), Some(b)) => a.abs_diff(b) <= 1,
            (a, b) => a == b,
        };
        // The trident flies past its type's tracking range (64 blocks), where the official
        // server stops sending its motion and Clustine, which shows an entity as far as
        // its chunk is in view, does not: named in the roadmap (2026-10-29).
        let motion_compared = name != "minecraft:trident";
        let same = ours.kind == theirs.kind
            && ours.owner == theirs.owner
            && ours.moving == theirs.moving
            && ours.data == theirs.data
            && close(ours.place_interval, theirs.place_interval)
            && (!motion_compared || close(ours.motion_interval, theirs.motion_interval));
        if !same {
            differ.push(name);
        }
    }
    assert!(differ.is_empty(), "these differ, printed above: {differ:?}");
}

async fn spit(address: &str) -> Result<Shown> {
    let mut scene = Scene::join(address, "Herder").await?;
    let (x, z) = scene.column();
    let feet = scene.feet;
    // A llama attacks a wolf that is not tamed (`Llama.LlamaAttackWolfGoal`) by
    // spitting at it, but only one within a quarter of its follow range of 16, and a
    // wolf runs from llamas. So both are held in a corridor of fences three cells long
    // east of the player, the llama at one end and the wolf two blocks from it at the
    // other, in sight of each other down the corridor. The player builds it from above
    // its middle.
    scene.hover_at((feet.0 + 3.0, feet.1 + 2.0, feet.2)).await?;
    for dx in 1..=5 {
        for dz in -1..=1 {
            if dz == 0 && (2..=4).contains(&dx) {
                continue;
            }
            let at = (x + dx, GROUND + 1, z + dz);
            scene
                .place(items::OAK_FENCE, (at.0, GROUND, at.2), face::TOP, at)
                .await?;
        }
    }
    let from = scene.mark();
    scene
        .take(ItemStack::new(items::LLAMA_SPAWN_EGG, 1))
        .await?;
    scene.use_on(x + 2, GROUND, z, face::TOP).await?;
    scene
        .wait_added(from, entity_types::LLAMA, 1, PATIENCE)
        .await?;
    scene.take(ItemStack::new(items::WOLF_SPAWN_EGG, 1)).await?;
    scene.use_on(x + 4, GROUND, z, face::TOP).await?;
    // Back over the corridor's west end first: straight down to the feet runs through
    // its fence, and the official server puts a player who flies into one back.
    scene.hover_at((feet.0, feet.1 + 2.0, feet.2)).await?;
    scene.hover_at(feet).await?;
    let spat = scene
        .wait_added(from, entity_types::LLAMA_SPIT, 1, Duration::from_secs(60))
        .await?;
    let (_, add) = spat.into_iter().next().context("the spit")?;
    let id = add.entity_id;
    let _ = scene
        .until(Duration::from_secs(3), "the spit gone", |scene| {
            scene.track(id).removed.is_some()
        })
        .await;
    let llama = scene
        .added_of(from, entity_types::LLAMA)
        .first()
        .map(|(_, llama)| llama.entity_id)
        .context("the llama")?;
    let mut shown = shown(&scene, id)?;
    // The spit's owner is the llama, not the player.
    shown.owner = owner(add.data, llama).replace("shooter", "llama");
    Ok(shown)
}

/// Comparison 9, the llama's spit: added with the llama as its owner, moving, with no
/// data, and its places sent at the same interval.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_llamas_spit_is_sent_like_on_the_official_server() {
    let (theirs, ours) = on_both(spit).await;
    println!("official {theirs:?}\nClustine {ours:?}");
    let theirs = theirs.expect("the official server's spit");
    let ours = ours.expect("Clustine's spit");
    assert_eq!(ours.kind, theirs.kind);
    assert_eq!(ours.owner, theirs.owner);
    assert_eq!(ours.moving, theirs.moving);
    assert_eq!(ours.data, theirs.data);
}

// --- Comparison 2, bot-only: a dispensed arrow's flight ----------------------------

#[derive(Debug)]
struct Flown {
    /// The farthest a place shown in the air lies from the rule's path.
    worst_off_the_path: f64,
    places: usize,
    /// Whether its data said it was in the ground (index 10).
    landed: bool,
    /// For the arrow shot up: whether its places fell when the block it stuck in was
    /// broken, whether its data said it was out of the ground and then in it again,
    /// and how fast it began to fall.
    fell: Option<bool>,
    out_of_the_ground: Option<bool>,
    landed_again: Option<bool>,
    fall_motion: Option<[f64; 3]>,
}

fn in_ground(value: &EntityDataValue) -> bool {
    *value == EntityDataValue::Boolean(true)
}

async fn fly_an_arrow(scene: &mut Scene, beside: (i32, i32), then_break: bool) -> Result<Flown> {
    let from = scene.mark();
    power(scene, beside).await?;
    let added = scene
        .wait_added(from, entity_types::ARROW, 1, PATIENCE)
        .await?;
    let (_, add) = added.into_iter().next().context("the arrow")?;
    let id = add.entity_id;
    // Its data says when it is in the ground; where a server does not say so, it is
    // given the time an arrow takes to come down.
    let landed = scene
        .until(
            Duration::from_secs(10),
            "the arrow in the ground",
            |scene| scene.track(id).first_data(10, in_ground).is_some(),
        )
        .await
        .is_ok();
    let track = scene.track(id);
    // Where it lies is its last place; the places in the air are those before the
    // first that was already there (the place of the hit is not on a tick's path).
    let (_, rest, _) = *track.places.last().context("where it lies")?;
    let lying = |place: &[f64; 3]| (0..3).all(|axis| (place[axis] - rest[axis]).abs() < 1e-6);
    let came_to_rest = track
        .places
        .iter()
        .find(|(_, place, _)| lying(place))
        .map(|(at, _, _)| *at);
    let off = off_the_path(&track, Flight::Arrow { gravity: 0.05 }, came_to_rest);
    let mut flown = Flown {
        worst_off_the_path: off.iter().copied().fold(0.0, f64::max),
        places: off.len(),
        landed,
        fell: None,
        out_of_the_ground: None,
        landed_again: None,
        fall_motion: None,
    };
    if then_break {
        // It lies 0.05 above the top it struck, moved back along its motion.
        let under = (
            rest[0].floor() as i32,
            (rest[1] - 0.1).floor() as i32,
            rest[2].floor() as i32,
        );
        scene.break_block(under).await?;
        let broken = Instant::now();
        let fallen = |scene: &Scene| {
            scene
                .track(id)
                .places
                .iter()
                .any(|(at, place, _)| *at >= broken && place[1] < rest[1] - 0.3)
        };
        flown.fell = Some(
            scene
                .until(Duration::from_secs(5), "the arrow falling", fallen)
                .await
                .is_ok(),
        );
        let after_the_break = |scene: &Scene, wanted: bool| {
            scene.track(id).data.iter().any(|(at, values)| {
                *at >= broken && value_at(values, 10) == Some(&EntityDataValue::Boolean(wanted))
            })
        };
        let _ = scene
            .until(
                Duration::from_secs(10),
                "the arrow landing again",
                |scene| after_the_break(scene, true),
            )
            .await;
        flown.out_of_the_ground = Some(after_the_break(scene, false));
        flown.landed_again = Some(after_the_break(scene, true));
        flown.fall_motion = scene
            .track(id)
            .motions
            .iter()
            .find(|(at, _)| *at >= broken)
            .map(|(_, motion)| *motion);
    }
    Ok(flown)
}

async fn dispense_arrows(address: &str) -> Result<Vec<Flown>> {
    let mut scene = Scene::join(address, "Watcher").await?;
    let (x, z) = scene.column();
    // One facing up four blocks south, out of the way of its arrow's fall, powered from
    // its west; one facing west two blocks off, powered from its north.
    let up = (x, z + 4);
    let west = (x - 2, z - 2);
    dispenser(&mut scene, up, (0.0, 90.0)).await?;
    fill(
        &mut scene,
        (up.0, GROUND + 1, up.1),
        ItemStack::new(items::ARROW, 16),
    )
    .await?;
    // Looking east, the dispenser faces west.
    dispenser(&mut scene, west, (-90.0, 0.0)).await?;
    fill(
        &mut scene,
        (west.0, GROUND + 1, west.1),
        ItemStack::new(items::ARROW, 16),
    )
    .await?;
    let upward = fly_an_arrow(&mut scene, (up.0 - 1, up.1), true).await?;
    let sideways = fly_an_arrow(&mut scene, (west.0, west.1 - 1), false).await?;
    Ok(vec![upward, sideways])
}

/// Comparison 2 in its bot-only form: an arrow from a dispenser facing up and one from
/// a dispenser facing west fly along the path `AbstractArrow.tick` gives from the add
/// packet (the move, then the drag of 0.99 and the gravity of 0.05), every place shown
/// within five hundredths of it (the packets' 1/4096 and the quantised motion's error
/// summed over the flight), stick in the ground, and the one shot up falls when the
/// block it stuck in is broken (its fall's motion each at most 0.2 of what it had) and
/// lands again. The ticks and the block it sticks in to the tick, the water, and the
/// fall's three draws to the bit need the simulation fed the official server's state
/// (the plan's form), which waits for a test-only input of the region.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_dispensed_arrow_flies_by_the_rules_like_on_the_official_server() {
    let (theirs, ours) = on_both(dispense_arrows).await;
    println!("official {theirs:#?}\nClustine {ours:#?}");
    let theirs = theirs.expect("the official server's arrows");
    let ours = ours.expect("Clustine's arrows");
    for (name, flights) in [("the official server", &theirs), ("Clustine", &ours)] {
        for (flown, upward) in flights.iter().zip([true, false]) {
            assert!(flown.landed, "{name}: {flown:?}");
            // The arrow shot west from a dispenser on the ground lands within some four
            // ticks, before the game sends an arrow's first place in the air (every 20
            // ticks): only the one shot up is shown in flight.
            assert!(
                !upward || flown.places >= 1,
                "{name}: no place in the air: {flown:?}"
            );
            assert!(flown.worst_off_the_path <= 0.05, "{name}: {flown:?}");
        }
        let upward = &flights[0];
        assert_eq!(upward.fell, Some(true), "{name}: {upward:?}");
        assert_eq!(upward.out_of_the_ground, Some(true), "{name}: {upward:?}");
        assert_eq!(upward.landed_again, Some(true), "{name}: {upward:?}");
        if let Some(motion) = upward.fall_motion {
            assert!(
                motion[0].abs() <= 0.2 && motion[2].abs() <= 0.2,
                "{name}: {upward:?}"
            );
        }
    }
}

// --- Comparison 4, bot-only: thrown kinds' flight ----------------------------------

#[derive(Debug)]
struct ThrownFlight {
    kind: i32,
    /// The add packet's place less the thrower's eyes.
    from_the_eyes: [f64; 3],
    speed: f64,
    worst_off_the_path: f64,
    places: usize,
    /// The level events from the throw until a second after the projectile went.
    events: Vec<(i32, i32)>,
}

/// The kinds thrown, each with its gravity and power (the plan's 4.4).
fn thrown_kinds() -> Vec<(ItemStack, i32, f64, f64)> {
    vec![
        (ItemStack::new(items::EGG, 1), entity_types::EGG, 0.03, 1.5),
        (
            potion_of(items::SPLASH_POTION, "minecraft:swiftness"),
            entity_types::SPLASH_POTION,
            0.05,
            0.5,
        ),
        (
            ItemStack::new(items::EXPERIENCE_BOTTLE, 1),
            entity_types::EXPERIENCE_BOTTLE,
            0.07,
            0.7,
        ),
        (
            ItemStack::new(items::ENDER_PEARL, 1),
            entity_types::ENDER_PEARL,
            0.03,
            1.5,
        ),
    ]
}

async fn throw_each(address: &str) -> Result<Vec<ThrownFlight>> {
    let mut scene = Scene::join(address, "Thrower").await?;
    let mut flights = Vec::new();
    for (stack, kind, gravity, _) in thrown_kinds() {
        scene.take(stack).await?;
        scene.round_trip().await?;
        let feet = scene.feet;
        let from = scene.mark();
        scene.use_item(-90.0, -30.0).await?;
        let added = scene.wait_added(from, kind, 1, PATIENCE).await?;
        let (_, add) = added.into_iter().next().context("the throw")?;
        let id = add.entity_id;
        scene
            .until(Duration::from_secs(10), "the projectile gone", |scene| {
                scene.track(id).removed.is_some()
            })
            .await?;
        scene.idle_ticks(20).await?;
        let track = scene.track(id);
        let off = off_the_path(&track, Flight::Thrown { gravity }, track.removed);
        flights.push(ThrownFlight {
            kind,
            from_the_eyes: [
                add.x - feet.0,
                add.y - (feet.1 + f64::from(1.62_f32)),
                add.z - feet.2,
            ],
            speed: speed(add.velocity),
            worst_off_the_path: off.iter().copied().fold(0.0, f64::max),
            places: off.len(),
            events: scene
                .level_events(from)
                .into_iter()
                .map(|(_, event)| (event.event, event.data))
                .collect(),
        });
    }
    Ok(flights)
}

/// Comparison 4 in its bot-only form: an egg, a splash potion, a bottle and a pearl
/// thrown east and up by a player leave a tenth below the eyes, at the kind's power
/// within the spread of an uncertainty of 1 (held to a twentieth), fly along the path
/// `ThrowableProjectile.tick` gives with the kind's gravity (each place within five
/// hundredths), and land with the same level events (the potion's 2002 with its colour,
/// the bottle's). The effects' durations on mobs by their distance to the splash need
/// the simulation fed the official server's hit, which waits for a test-only input.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn thrown_kinds_fly_by_the_rules_like_on_the_official_server() {
    let (theirs, ours) = on_both(throw_each).await;
    println!("official {theirs:#?}\nClustine {ours:#?}");
    let theirs = theirs.expect("the official server's throws");
    let ours = ours.expect("Clustine's throws");
    let kinds = thrown_kinds();
    for (name, flights) in [("the official server", &theirs), ("Clustine", &ours)] {
        for (flight, (_, kind, _, power)) in flights.iter().zip(&kinds) {
            assert_eq!(flight.kind, *kind, "{name}");
            assert!(
                flight.from_the_eyes[0].abs() < 1e-9
                    && flight.from_the_eyes[2].abs() < 1e-9
                    && (flight.from_the_eyes[1] + 0.1).abs() < 1e-6,
                "{name}: {flight:?}"
            );
            assert!(
                (power * 0.95..=power * 1.05).contains(&flight.speed),
                "{name}: {flight:?}"
            );
            assert!(
                flight.places >= 1,
                "{name}: no place in the air: {flight:?}"
            );
            assert!(flight.worst_off_the_path <= 0.05, "{name}: {flight:?}");
        }
    }
    for (theirs, ours) in theirs.iter().zip(&ours) {
        assert_eq!(ours.from_the_eyes, theirs.from_the_eyes, "{}", theirs.kind);
        assert_eq!(
            ours.events, theirs.events,
            "the level events of {}",
            theirs.kind
        );
    }
}

// --- Comparison 15, bot-only: a loyal trident's return -----------------------------

#[derive(Debug)]
struct Returned {
    loyalty: Option<EntityDataValue>,
    /// Ticks from its stick in the ground to the return's sound.
    in_ground_for: Option<i64>,
    /// Whether the first motion after the return's sound points at the thrower.
    towards_the_thrower: Option<bool>,
    taken: bool,
}

async fn throw_a_loyal_trident(address: &str) -> Result<Returned> {
    let mut scene = Scene::join(address, "Thrower").await?;
    let loyalty = scene.registered("minecraft:enchantment", "minecraft:loyalty")?;
    let trident = with(
        items::TRIDENT,
        vec![WireComponent::Enchantments(vec![(loyalty, 3)])],
    );
    scene.take(trident).await?;
    scene.round_trip().await?;
    let from = scene.mark();
    // Down at the ground a few blocks ahead.
    scene.use_item(0.0, 30.0).await?;
    scene.idle_ticks(15).await?;
    scene.release().await?;
    let added = scene
        .wait_added(from, entity_types::TRIDENT, 1, PATIENCE)
        .await?;
    let (_, add) = added.into_iter().next().context("the trident")?;
    let id = add.entity_id;
    let _ = scene
        .until(Duration::from_secs(15), "the trident taken", |scene| {
            scene.track(id).removed.is_some()
        })
        .await;
    let track = scene.track(id);
    let landed = track.first_data(10, in_ground);
    let returning = heard(&scene, from, sounds::ITEM_TRIDENT_RETURN).ok();
    let eyes = [scene.feet.0, scene.feet.1 + 1.62, scene.feet.2];
    let towards_the_thrower = returning.and_then(|returning| {
        let place = track
            .places
            .iter()
            .rev()
            .find(|(at, _, _)| *at <= returning)
            .map(|(_, place, _)| *place)?;
        let (_, motion) = track.motions.iter().find(|(at, _)| *at >= returning)?;
        let dot: f64 = (0..3)
            .map(|axis| motion[axis] * (eyes[axis] - place[axis]))
            .sum();
        Some(dot > 0.0)
    });
    let taken = scene.of(from, |packet| match packet {
        ClientboundPlay::TakeItemEntity(taken) if taken.item_id == id => Some(()),
        _ => None,
    });
    Ok(Returned {
        loyalty: track
            .data
            .iter()
            .find_map(|(_, values)| value_at(values, 11).cloned()),
        in_ground_for: landed
            .zip(returning)
            .map(|(landed, returning)| ticks(landed, returning)),
        towards_the_thrower,
        taken: !taken.is_empty(),
    })
}

/// Comparison 15 in its bot-only form: a trident with Loyalty III thrown at the ground
/// shows its loyalty (data 11), sticks, and after its four ticks in the ground plays
/// the return's sound and flies back at the thrower, who takes it. The ticks in the
/// ground are held to one of the other server's; the return's motion to the tick needs
/// the simulation fed the official server's state.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_loyal_trident_returns_like_on_the_official_server() {
    let (theirs, ours) = on_both(throw_a_loyal_trident).await;
    println!("official {theirs:?}\nClustine {ours:?}");
    let theirs = theirs.expect("the official server's trident");
    let ours = ours.expect("Clustine's trident");
    for (name, returned) in [("the official server", &theirs), ("Clustine", &ours)] {
        assert_eq!(
            returned.loyalty,
            Some(EntityDataValue::Byte(3)),
            "{name}: {returned:?}"
        );
        assert!(returned.in_ground_for.is_some(), "{name}: {returned:?}");
        assert_eq!(
            returned.towards_the_thrower,
            Some(true),
            "{name}: {returned:?}"
        );
        assert!(returned.taken, "{name}: {returned:?}");
    }
    let (theirs, ours) = (
        theirs.in_ground_for.expect("checked"),
        ours.in_ground_for.expect("checked"),
    );
    assert!(
        theirs.abs_diff(ours) <= 1,
        "in the ground for {theirs} ticks on the official server, {ours} on Clustine"
    );
}

/// The helpers' own sense, which runs without Java.
#[test]
fn a_draw_is_told_by_its_speed() {
    assert_eq!(drawn_as(3.0 * power_for(5)), 5);
    assert_eq!(drawn_as(3.0 * power_for(10) * 1.02), 10);
    assert_eq!(drawn_as(3.0), 20);
    assert_eq!(
        potion::NAMES[potion_id("minecraft:swiftness") as usize],
        "minecraft:swiftness"
    );
}
