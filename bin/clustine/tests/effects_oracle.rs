//! Comparisons with the official server of N9's mob effects and of the monsters' attacks
//! that make or carry them (`docs/groundwork/n9-plan.md`, section 11, comparisons 5, 6,
//! 8, 10 and 12, with decision 9 of section 18: neither server is driven by commands,
//! so each scene is made by what a creative player can do on both, mobs by their eggs).
//!
//! Each runs one scene on the official server and then on Clustine, each on a fresh
//! flat world, and compares what a client was sent: exact where nothing draws (an
//! effect's packet, a cloud's radius, the data), by ticks where a rule fixes them (a
//! fang's warmup, the guardian's 80 ticks, the skeleton's rhythm, the pillager's
//! charge), each held to a tick of a client's clock either way (two over spans of
//! several seconds), and by range where a draw decides (the pillager's wait before its
//! shot).
//!
//! They need Java, the server jar downloaded by `cargo datagen`, and your agreement to
//! the Minecraft EULA, so they are ignored by default. Run them on a quiet machine:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test effects_oracle -- --ignored official_server --nocapture \
//!     --test-threads 1 --no-fail-fast
//! ```

mod common;
#[path = "common/n9_scene.rs"]
#[allow(dead_code)]
mod n9_scene;

use std::time::Duration;

use anyhow::{Context, Result, bail};
use clustine_data::{attributes, entity_types, items, sounds};
use clustine_protocol::components::{
    EffectDetails, EffectInstance, PotionContents, WireComponent, WireComponents, mob_effect,
    potion,
};
use clustine_protocol::item::ItemStack;
use clustine_protocol::packets::play::{ClientboundPlay, EntityDataValue, face};
use tokio::time::Instant;

use n9_scene::{GROUND, PATIENCE, Scene, on_both, ticks, value_at};

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

fn index_of(names: &[&str], name: &str) -> i32 {
    let at = names
        .iter()
        .position(|known| *known == name)
        .unwrap_or_else(|| panic!("{name} in its registry"));
    i32::try_from(at).expect("a registry is small")
}

/// Uses the egg `egg` on the top of the block at `at` and returns the mob of `kind` it
/// made.
async fn egg(scene: &mut Scene, egg: i32, kind: i32, at: (i32, i32, i32)) -> Result<i32> {
    let from = scene.mark();
    scene.take(ItemStack::new(egg, 1)).await?;
    scene.use_on(at.0, at.1, at.2, face::TOP).await?;
    let made = scene.wait_added(from, kind, 1, PATIENCE).await?;
    Ok(made.first().context("the mob")?.1.entity_id)
}

/// Reads both scenes by turns, a tick of each at a time, until `done` holds for the
/// first, at most for `patience`: for two players of one scene, each of whom must keep
/// answering its server.
async fn by_turns(
    first: &mut Scene,
    second: &mut Scene,
    patience: Duration,
    what: &str,
    mut done: impl FnMut(&Scene) -> bool,
) -> Result<()> {
    let deadline = Instant::now() + patience;
    while !done(first) {
        if Instant::now() > deadline {
            bail!("{}: no {what} within {patience:?}", first.name);
        }
        first.idle(Duration::from_millis(25)).await?;
        second.idle(Duration::from_millis(25)).await?;
    }
    Ok(())
}

// --- Comparison 5: the lingering cloud ---------------------------------------------

#[derive(Debug)]
struct Bathed {
    /// The ticks from the cloud's add to each effect packet the player was sent.
    applied_at: Vec<i64>,
    /// Each effect packet: the effect, its amplifier, duration and flags.
    effects: Vec<(i32, i32, i32, u8)>,
    /// The ticks from the add to its waiting flag's end.
    waited: Option<i64>,
    /// The radius each time it was sent.
    radii: Vec<f32>,
}

async fn stand_in_a_cloud(address: &str) -> Result<Bathed> {
    let mut scene = Scene::join(address, "Bather").await?;
    let lingering = with(
        items::LINGERING_POTION,
        vec![WireComponent::PotionContents(PotionContents {
            potion: Some(index_of(&potion::NAMES, "minecraft:swiftness")),
            ..PotionContents::default()
        })],
    );
    scene.take(lingering).await?;
    scene.round_trip().await?;
    let from = scene.mark();
    // At the feet.
    scene.use_item(0.0, 90.0).await?;
    let clouds = scene
        .wait_added(from, entity_types::AREA_EFFECT_CLOUD, 1, PATIENCE)
        .await?;
    let (added, cloud) = clouds.into_iter().next().context("the cloud")?;
    let id = cloud.entity_id;
    scene
        .until(Duration::from_secs(40), "the cloud gone", |scene| {
            scene.track(id).removed.is_some()
        })
        .await?;
    let me = scene.me;
    let effects = scene.of(from, |packet| match packet {
        ClientboundPlay::UpdateMobEffect(effect) if effect.entity_id == me => Some((
            effect.effect,
            effect.amplifier,
            effect.duration,
            effect.flags,
        )),
        _ => None,
    });
    let track = scene.track(id);
    Ok(Bathed {
        applied_at: effects.iter().map(|(at, _)| ticks(added, *at)).collect(),
        effects: effects.into_iter().map(|(_, effect)| effect).collect(),
        waited: track
            .first_data(9, |value| *value == EntityDataValue::Boolean(false))
            .map(|at| ticks(added, at)),
        radii: track
            .data
            .iter()
            .filter_map(|(_, values)| match value_at(values, 8) {
                Some(EntityDataValue::Float(radius)) => Some(*radius),
                _ => None,
            })
            .collect(),
    })
}

/// Comparison 5: a lingering potion of swiftness thrown at the feet makes a cloud that
/// waits 10 ticks, then gives the player standing in it swiftness for 900 ticks (3600
/// scaled by 0.25) each time it applies, and shrinks by its radius per tick and by half
/// a block at each application until it is gone. The ticks it applied at are held to
/// one of the other server's, the effects and the radius it was sent with are the same
/// (no draw decides either).
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_lingering_cloud_applies_and_shrinks_like_on_the_official_server() {
    let (theirs, ours) = on_both(stand_in_a_cloud).await;
    println!("official {theirs:?}\nClustine {ours:?}");
    let theirs = theirs.expect("the official server's cloud");
    let ours = ours.expect("Clustine's cloud");
    let speed = index_of(&mob_effect::NAMES, "minecraft:speed");
    for (name, bathed) in [("the official server", &theirs), ("Clustine", &ours)] {
        assert!(bathed.effects.len() >= 2, "{name}: {bathed:?}");
        for (effect, amplifier, duration, _) in &bathed.effects {
            assert_eq!(
                (*effect, *amplifier, *duration),
                (speed, 0, 900),
                "{name}: {bathed:?}"
            );
        }
        let waited = bathed.waited.expect("the waiting flag's end");
        assert!((9..=11).contains(&waited), "{name}: {bathed:?}");
    }
    assert_eq!(ours.effects, theirs.effects, "the effects' packets");
    for (theirs_at, ours_at) in theirs.applied_at.iter().zip(&ours.applied_at) {
        assert!(
            theirs_at.abs_diff(*ours_at) <= 1,
            "applied at {:?} on the official server, {:?} on Clustine",
            theirs.applied_at,
            ours.applied_at
        );
    }
    let shared = theirs.radii.len().min(ours.radii.len());
    assert!(
        theirs.radii.len().abs_diff(ours.radii.len()) <= 2,
        "the radius was sent {} times by the official server, {} by Clustine",
        theirs.radii.len(),
        ours.radii.len()
    );
    for (at, (theirs_radius, ours_radius)) in theirs.radii[..shared]
        .iter()
        .zip(&ours.radii[..shared])
        .enumerate()
    {
        assert!(
            (theirs_radius - ours_radius).abs() <= 1e-5,
            "radius {at}: {theirs_radius} on the official server, {ours_radius} on Clustine"
        );
    }
}

// --- Comparison 6: evoker fangs ----------------------------------------------------

#[derive(Debug)]
struct Fanged {
    count: usize,
    /// Each fang's ticks from its add to its entity event 4, sorted: fixed by its place
    /// in the spell.
    warmups: Vec<i64>,
    /// Each fang's ticks from its event 4 to its removal.
    lives: Vec<i64>,
    /// The ticks from the event 4 of the fang that hurt the villager to the hurt.
    hurt_after: Option<i64>,
    /// The fang's add data against the evoker's id, and its first data.
    owner_off_by: Option<i32>,
}

async fn cast_fangs(address: &str) -> Result<Fanged> {
    let mut scene = Scene::join(address, "Penner").await?;
    let (x, z) = scene.column();
    let feet = scene.feet;
    // The villager in a pit two blocks deep four blocks east, from which it cannot
    // walk out; the evoker two blocks west, six from it: far enough for a line of
    // fangs (the circle is for a target within three).
    scene.dig_pit(&[(x + 4, z)], 2).await?;
    let villager = egg(
        &mut scene,
        items::VILLAGER_SPAWN_EGG,
        entity_types::VILLAGER,
        (x + 4, GROUND - 2, z),
    )
    .await?;
    scene.hover_at(feet).await?;
    let evoker = egg(
        &mut scene,
        items::EVOKER_SPAWN_EGG,
        entity_types::EVOKER,
        (x - 2, GROUND, z),
    )
    .await?;
    let from = scene.mark();
    let first = scene
        .wait_added(from, entity_types::EVOKER_FANGS, 1, Duration::from_secs(60))
        .await?;
    let (_, first_fang) = first.into_iter().next().context("a fang")?;
    // The whole spell: the last fang of a line goes some 40 ticks after the first.
    scene.idle_ticks(60).await?;
    let fangs: Vec<i32> = scene
        .added_of(from, entity_types::EVOKER_FANGS)
        .iter()
        .map(|(_, fang)| fang.entity_id)
        .collect();
    let mut warmups = Vec::new();
    let mut lives = Vec::new();
    for fang in &fangs {
        let track = scene.track(*fang);
        let (added, _) = track.add.clone().context("a fang's add")?;
        if let Some(bit) = track.first_event(4) {
            warmups.push(ticks(added, bit));
            if let Some(gone) = track.removed {
                lives.push(ticks(bit, gone));
            }
        }
    }
    warmups.sort_unstable();
    let hurt = scene.of(from, |packet| match packet {
        ClientboundPlay::DamageEvent(damage)
            if damage.entity_id == villager
                && damage.direct.is_some_and(|direct| fangs.contains(&direct)) =>
        {
            damage.direct
        }
        _ => None,
    });
    let hurt_after = hurt.first().and_then(|(at, fang)| {
        let bit = scene.track(*fang).first_event(4)?;
        Some(ticks(bit, *at))
    });
    Ok(Fanged {
        count: fangs.len(),
        warmups,
        lives,
        hurt_after,
        owner_off_by: Some(first_fang.data - evoker),
    })
}

/// Comparison 6: an evoker made by its egg casts fangs at a villager in a pen; when the
/// spell starts is drawn and is not compared. Each fang's ticks from its add packet to
/// its entity event 4 are fixed by its place in the line, and are held to one of the
/// other server's, fang by fang in order; it goes 22 ticks after (held to 20 to 24); the
/// villager is hurt by a fang seven ticks after that fang's event 4 (the hurt at warmup
/// −8, the event at −1). The add packet names the evoker.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn evoker_fangs_bite_on_their_ticks_like_on_the_official_server() {
    let (theirs, ours) = on_both(cast_fangs).await;
    println!("official {theirs:?}\nClustine {ours:?}");
    let theirs = theirs.expect("the official server's fangs");
    let ours = ours.expect("Clustine's fangs");
    for (name, fanged) in [("the official server", &theirs), ("Clustine", &ours)] {
        assert!(fanged.count > 0, "{name}: {fanged:?}");
        assert_eq!(fanged.warmups.len(), fanged.count, "{name}: {fanged:?}");
        for life in &fanged.lives {
            assert!((20..=24).contains(life), "{name}: {fanged:?}");
        }
        let hurt_after = fanged.hurt_after.expect("the villager hurt by a fang");
        assert!((6..=8).contains(&hurt_after), "{name}: {fanged:?}");
    }
    assert_eq!(ours.owner_off_by, theirs.owner_off_by, "the fangs' owner");
    assert_eq!(ours.count, theirs.count, "the fangs of a line");
    for (theirs_warmup, ours_warmup) in theirs.warmups.iter().zip(&ours.warmups) {
        assert!(
            theirs_warmup.abs_diff(*ours_warmup) <= 1,
            "warmups {:?} on the official server, {:?} on Clustine",
            theirs.warmups,
            ours.warmups
        );
    }
}

// --- Comparison 10: a player's effects ---------------------------------------------

#[derive(Debug)]
struct Drunk {
    /// The drinker's effect packet: the effect, its amplifier, duration and flags.
    effect: Option<(i32, i32, i32, u8)>,
    /// The drinker's movement speed's modifiers as sent after the drink.
    speed_modifiers: Vec<String>,
    /// The ticks from the effect's packet to its removal's.
    lasted: Option<i64>,
    /// What the watcher was sent of the drinker's particles (index 10) and whether
    /// they are all ambient (index 11), first; and whether index 10 was emptied at the
    /// end.
    particles: Option<EntityDataValue>,
    ambient: Option<EntityDataValue>,
    cleared: bool,
    drink_sounds: usize,
    drink_crumbs: usize,
    eat_sounds: usize,
    eat_crumbs: usize,
    burped: bool,
}

/// The watcher's sounds of `sound` at or near the other player from `from` on.
fn count_sounds(scene: &Scene, from: usize, sound: i32) -> usize {
    scene
        .sounds(from)
        .iter()
        .filter(|(_, heard, _)| *heard == sound)
        .count()
}

async fn drink_and_eat(address: &str) -> Result<Drunk> {
    let mut drinker = Scene::join(address, "Drinker").await?;
    let mut watcher = Scene::join(address, "Watcher").await?;
    watcher
        .until(PATIENCE, "the drinker shown", |scene| {
            !scene.shown_of(entity_types::PLAYER).is_empty()
        })
        .await?;
    let shown_as = *watcher
        .shown_of(entity_types::PLAYER)
        .first()
        .context("the drinker shown")?;
    // A potion of swiftness for 100 ticks, so that its end comes within the test; the
    // tab's lasts three minutes.
    let speed = index_of(&mob_effect::NAMES, "minecraft:speed");
    let potion = with(
        items::POTION,
        vec![WireComponent::PotionContents(PotionContents {
            custom_effects: vec![EffectInstance {
                effect: speed,
                details: EffectDetails {
                    amplifier: 0,
                    duration: 100,
                    ambient: false,
                    visible: true,
                    show_icon: true,
                    hidden: None,
                },
            }],
            ..PotionContents::default()
        })],
    );
    drinker.take(potion).await?;
    drinker.round_trip().await?;
    let (from, watched) = (drinker.mark(), watcher.mark());
    drinker.use_item(0.0, 0.0).await?;
    let me = drinker.me;
    let given = |packet: &ClientboundPlay| match packet {
        ClientboundPlay::UpdateMobEffect(effect) if effect.entity_id == me => Some((
            effect.effect,
            effect.amplifier,
            effect.duration,
            effect.flags,
        )),
        _ => None,
    };
    let taken = |packet: &ClientboundPlay| match packet {
        ClientboundPlay::RemoveMobEffect(effect) if effect.entity_id == me => Some(()),
        _ => None,
    };
    by_turns(
        &mut drinker,
        &mut watcher,
        PATIENCE,
        "the effect",
        |scene| !scene.of(from, given).is_empty(),
    )
    .await?;
    by_turns(
        &mut drinker,
        &mut watcher,
        PATIENCE,
        "the effect's end",
        |scene| !scene.of(from, taken).is_empty(),
    )
    .await?;
    watcher.round_trip().await?;
    let effect = drinker.of(from, given).first().cloned();
    let removed = drinker.of(from, taken).first().map(|(at, _)| *at);
    let speed_modifiers = drinker
        .of(from, |packet| match packet {
            ClientboundPlay::UpdateAttributes(update) if update.entity_id == me => update
                .attributes
                .iter()
                .find(|attribute| attribute.attribute == attributes::MOVEMENT_SPEED)
                .map(|attribute| {
                    attribute
                        .modifiers
                        .iter()
                        .map(|(id, _, _)| id.clone())
                        .collect::<Vec<_>>()
                }),
            _ => None,
        })
        .into_iter()
        .map(|(_, modifiers)| modifiers)
        .find(|modifiers| !modifiers.is_empty())
        .unwrap_or_default();
    let track = watcher.track(shown_as);
    let particles = track
        .data
        .iter()
        .find_map(|(_, values)| value_at(values, 10).cloned());
    let ambient = track
        .data
        .iter()
        .find_map(|(_, values)| value_at(values, 11).cloned());
    let cleared = track.data.iter().any(|(at, values)| {
        removed.is_some_and(|removed| *at >= removed - Duration::from_millis(100))
            && value_at(values, 10) == Some(&EntityDataValue::Particles(Vec::new()))
    });
    let drink_sounds = count_sounds(&watcher, watched, sounds::ENTITY_GENERIC_DRINK);
    let drink_crumbs = watcher.particles(watched).len();

    // Rotten flesh, eaten in 32 ticks: crumbs and sounds, and a burp at the end.
    drinker.take(ItemStack::new(items::ROTTEN_FLESH, 1)).await?;
    drinker.round_trip().await?;
    let eating = watcher.mark();
    drinker.use_item(0.0, 0.0).await?;
    by_turns(&mut watcher, &mut drinker, PATIENCE, "the burp", |scene| {
        count_sounds(scene, eating, sounds::ENTITY_PLAYER_BURP) > 0
    })
    .await?;
    watcher.idle_ticks(5).await?;
    Ok(Drunk {
        effect: effect.map(|(_, effect)| effect),
        speed_modifiers,
        lasted: effect
            .zip(removed)
            .map(|((given, _), removed)| ticks(given, removed)),
        particles,
        ambient,
        cleared,
        drink_sounds,
        drink_crumbs,
        eat_sounds: count_sounds(&watcher, eating, sounds::ENTITY_GENERIC_EAT),
        eat_crumbs: watcher.particles(eating).len(),
        burped: count_sounds(&watcher, eating, sounds::ENTITY_PLAYER_BURP) > 0,
    })
}

/// Comparison 10: a player drinks a potion of swiftness: their client is sent the
/// effect (`update_mob_effect`) and their movement speed with the effect's modifier,
/// another player is sent the drinker's effect particles (index 10) and whether they
/// are ambient (index 11), and hears the drinking without crumbs; the effect ends after
/// its 100 ticks (held to one), the particles emptied. Then the player eats rotten
/// flesh: the other hears the eating and sees its crumbs, as many as on the official
/// server (held to one), and the burp. The hunger rotten flesh gives four times in five
/// is drawn and not compared.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_players_effects_are_sent_like_on_the_official_server() {
    let (theirs, ours) = on_both(drink_and_eat).await;
    println!("official {theirs:?}\nClustine {ours:?}");
    let theirs = theirs.expect("the official server's drink");
    let ours = ours.expect("Clustine's drink");
    for (name, drunk) in [("the official server", &theirs), ("Clustine", &ours)] {
        assert!(drunk.effect.is_some(), "{name}: {drunk:?}");
        assert!(
            drunk
                .speed_modifiers
                .iter()
                .any(|id| id == "minecraft:effect.speed"),
            "{name}: {drunk:?}"
        );
        let lasted = drunk.lasted.expect("the effect's end");
        assert!((99..=101).contains(&lasted), "{name}: {drunk:?}");
        assert!(
            matches!(&drunk.particles, Some(EntityDataValue::Particles(list)) if !list.is_empty()),
            "{name}: {drunk:?}"
        );
        assert!(drunk.cleared, "{name}: the particles emptied: {drunk:?}");
        assert!(drunk.drink_sounds > 0, "{name}: {drunk:?}");
        assert_eq!(drunk.drink_crumbs, 0, "{name}: {drunk:?}");
        assert!(drunk.eat_sounds > 0, "{name}: {drunk:?}");
        assert!(drunk.eat_crumbs > 0, "{name}: {drunk:?}");
        assert!(drunk.burped, "{name}: {drunk:?}");
    }
    assert_eq!(ours.effect, theirs.effect, "the effect's packet");
    assert_eq!(ours.particles, theirs.particles, "the particles shown");
    assert_eq!(ours.ambient, theirs.ambient, "index 11");
    assert!(ours.drink_sounds.abs_diff(theirs.drink_sounds) <= 1);
    assert!(ours.eat_sounds.abs_diff(theirs.eat_sounds) <= 1);
    assert!(ours.eat_crumbs.abs_diff(theirs.eat_crumbs) <= 1);
}

// --- Comparison 12: the guardian's beam --------------------------------------------

#[derive(Debug)]
struct Beamed {
    /// Whether the beam's target (index 17) named the squid.
    at_the_squid: bool,
    attack_sound_event: bool,
    /// The ticks from the beam's target being sent to the squid's hurt.
    struck_after: Option<i64>,
}

async fn beam(address: &str) -> Result<Beamed> {
    let mut scene = Scene::join(address, "Aquarist").await?;
    let (x, z) = scene.column();
    // A trench five long, two deep, its floor under water: the guardian at one end,
    // the squid at the other, four blocks apart (a guardian targets a squid farther
    // than three).
    let cells: Vec<(i32, i32)> = (z - 2..=z + 2).map(|zz| (x + 3, zz)).collect();
    scene.dig_pit(&cells, 2).await?;
    // Poured straight down into the middle, from above it (the ends are out of a
    // bucket's reach), it flows along the floor to both ends.
    scene.pour((x + 3, GROUND - 2, z)).await?;
    scene
        .until(PATIENCE, "the water along the trench", |scene| {
            cells.iter().all(|(cx, cz)| {
                scene
                    .block((*cx, GROUND - 1, *cz))
                    .is_some_and(|state| state != 0)
            })
        })
        .await?;
    let guardian = egg(
        &mut scene,
        items::GUARDIAN_SPAWN_EGG,
        entity_types::GUARDIAN,
        (x + 3, GROUND - 2, z - 2),
    )
    .await?;
    let squid = egg(
        &mut scene,
        items::SQUID_SPAWN_EGG,
        entity_types::SQUID,
        (x + 3, GROUND - 2, z + 2),
    )
    .await?;
    let aiming = |value: &EntityDataValue| matches!(value, EntityDataValue::Int(id) if *id != 0);
    scene
        .until(Duration::from_secs(60), "the guardian's beam", |scene| {
            scene.track(guardian).first_data(17, aiming).is_some()
        })
        .await?;
    let struck = |scene: &Scene| {
        scene
            .of(0, |packet| match packet {
                ClientboundPlay::DamageEvent(damage) if damage.entity_id == squid => Some(()),
                _ => None,
            })
            .first()
            .map(|(at, _)| *at)
    };
    let _ = scene
        .until(Duration::from_secs(10), "the squid struck", |scene| {
            struck(scene).is_some()
        })
        .await;
    let track = scene.track(guardian);
    let aimed = track.first_data(17, aiming).context("the beam")?;
    let target = track
        .data
        .iter()
        .find_map(|(_, values)| match value_at(values, 17) {
            Some(EntityDataValue::Int(id)) if *id != 0 => Some(*id),
            _ => None,
        });
    Ok(Beamed {
        at_the_squid: target == Some(squid),
        attack_sound_event: track.first_event(21).is_some(),
        struck_after: struck(&scene).map(|at| ticks(aimed, at)),
    })
}

/// Comparison 12: a guardian and a squid made by their eggs in a trench of water: the
/// guardian's beam names the squid (its data 17, the squid's id on that server), its
/// entity event 21 starts the attack's sound, and the squid is struck 80 ticks after
/// the beam was sent (held to two, a span of four seconds, and to one of the other
/// server's).
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_guardians_beam_strikes_like_on_the_official_server() {
    let (theirs, ours) = on_both(beam).await;
    println!("official {theirs:?}\nClustine {ours:?}");
    let theirs = theirs.expect("the official server's guardian");
    let ours = ours.expect("Clustine's guardian");
    for (name, beamed) in [("the official server", &theirs), ("Clustine", &ours)] {
        assert!(beamed.at_the_squid, "{name}: {beamed:?}");
        assert!(beamed.attack_sound_event, "{name}: {beamed:?}");
        let after = beamed.struck_after.expect("the strike");
        assert!((78..=82).contains(&after), "{name}: {beamed:?}");
    }
    let (theirs, ours) = (
        theirs.struck_after.expect("checked"),
        ours.struck_after.expect("checked"),
    );
    assert!(
        theirs.abs_diff(ours) <= 1,
        "struck after {theirs} and {ours}"
    );
}

// --- Comparison 8: a skeleton's rhythm and a pillager's charge ---------------------

/// Makes an iron golem in a pit three blocks wide and two deep east of the player,
/// which it cannot climb out of, and returns to the ground: a target that cannot come
/// at the shooter.
async fn golem_in_a_pit(scene: &mut Scene) -> Result<i32> {
    let (x, z) = scene.column();
    let feet = scene.feet;
    let cells: Vec<(i32, i32)> = (x + 3..=x + 5)
        .flat_map(|cx| (z - 1..=z + 1).map(move |cz| (cx, cz)))
        .collect();
    scene.dig_pit(&cells, 2).await?;
    let golem = egg(
        scene,
        items::IRON_GOLEM_SPAWN_EGG,
        entity_types::IRON_GOLEM,
        (x + 4, GROUND - 2, z),
    )
    .await?;
    scene.hover_at(feet).await?;
    Ok(golem)
}

/// The arrows added from `from` on whose add packet names `owner`.
fn arrows_of(scene: &Scene, from: usize, owner: i32) -> Vec<Instant> {
    scene
        .added_of(from, entity_types::ARROW)
        .into_iter()
        .filter(|(_, arrow)| arrow.data == owner || arrow.data == owner + 1)
        .map(|(at, _)| at)
        .collect()
}

async fn skeleton_rhythm(address: &str) -> Result<Vec<i64>> {
    let mut scene = Scene::join(address, "Keeper").await?;
    golem_in_a_pit(&mut scene).await?;
    let (x, z) = scene.column();
    let from = scene.mark();
    let skeleton = egg(
        &mut scene,
        items::SKELETON_SPAWN_EGG,
        entity_types::SKELETON,
        (x - 3, GROUND, z),
    )
    .await?;
    scene
        .until(Duration::from_secs(40), "four arrows", |scene| {
            arrows_of(scene, from, skeleton).len() >= 4
        })
        .await?;
    let shots = arrows_of(&scene, from, skeleton);
    Ok(shots
        .windows(2)
        .map(|pair| ticks(pair[0], pair[1]))
        .collect())
}

/// Comparison 8, the skeleton: made by its egg ten blocks from an iron golem in a pit,
/// it shoots at the golem every 60 ticks at easy (a draw of 20 and an interval of 40),
/// each interval held to two of the rule and to two of the other server's. The time to
/// its first shot depends on when it sees the golem, and is not compared.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_skeleton_shoots_at_a_golem_in_its_rhythm_like_on_the_official_server() {
    let (theirs, ours) = on_both(skeleton_rhythm).await;
    println!("official {theirs:?}\nClustine {ours:?}");
    let theirs = theirs.expect("the official server's skeleton");
    let ours = ours.expect("Clustine's skeleton");
    for (name, intervals) in [("the official server", &theirs), ("Clustine", &ours)] {
        for interval in intervals {
            assert!((58..=62).contains(interval), "{name}: {intervals:?}");
        }
    }
    for (theirs_interval, ours_interval) in theirs.iter().zip(&ours) {
        assert!(
            theirs_interval.abs_diff(*ours_interval) <= 2,
            "official {theirs:?}, Clustine {ours:?}"
        );
    }
}

#[derive(Debug)]
struct Charged {
    /// Ticks from each charge's start (index 17 true) to its end (false).
    charges: Vec<i64>,
    /// Ticks from each charge's end to the next arrow.
    waits: Vec<i64>,
    shot_sounds: usize,
}

async fn pillager_charge(address: &str) -> Result<Charged> {
    let mut scene = Scene::join(address, "Keeper").await?;
    golem_in_a_pit(&mut scene).await?;
    let (x, z) = scene.column();
    let from = scene.mark();
    let pillager = egg(
        &mut scene,
        items::PILLAGER_SPAWN_EGG,
        entity_types::PILLAGER,
        (x - 3, GROUND, z),
    )
    .await?;
    scene
        .until(Duration::from_secs(40), "two arrows", |scene| {
            arrows_of(scene, from, pillager).len() >= 2
        })
        .await?;
    let track = scene.track(pillager);
    let mut flips: Vec<(Instant, bool)> = track
        .data
        .iter()
        .filter_map(|(at, values)| match value_at(values, 17) {
            Some(EntityDataValue::Boolean(charging)) => Some((*at, *charging)),
            _ => None,
        })
        .collect();
    flips.dedup_by_key(|(_, charging)| *charging);
    let shots = arrows_of(&scene, from, pillager);
    let mut charges = Vec::new();
    let mut waits = Vec::new();
    for pair in flips.windows(2) {
        if let [(started, true), (ended, false)] = pair {
            charges.push(ticks(*started, *ended));
            if let Some(shot) = shots.iter().find(|shot| **shot >= *ended) {
                waits.push(ticks(*ended, *shot));
            }
        }
    }
    Ok(Charged {
        charges,
        waits,
        shot_sounds: scene
            .sounds(from)
            .iter()
            .filter(|(_, sound, _)| *sound == sounds::ITEM_CROSSBOW_SHOOT)
            .count(),
    })
}

/// Comparison 8, the pillager: made by its egg ten blocks from an iron golem in a pit,
/// it charges its crossbow (index 17) for its 25 ticks (held to 24 to 27, and to one
/// of the other server's), then waits `20 + nextInt(20)` ticks (drawn: held to that
/// range and a tick either way) and shoots, with the crossbow's sound.
#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn a_pillager_charges_and_shoots_like_on_the_official_server() {
    let (theirs, ours) = on_both(pillager_charge).await;
    println!("official {theirs:?}\nClustine {ours:?}");
    let theirs = theirs.expect("the official server's pillager");
    let ours = ours.expect("Clustine's pillager");
    for (name, charged) in [("the official server", &theirs), ("Clustine", &ours)] {
        assert!(!charged.charges.is_empty(), "{name}: {charged:?}");
        for charge in &charged.charges {
            assert!((24..=27).contains(charge), "{name}: {charged:?}");
        }
        for wait in &charged.waits {
            assert!((19..=40).contains(wait), "{name}: {charged:?}");
        }
        assert!(charged.shot_sounds >= 1, "{name}: {charged:?}");
    }
    assert!(
        theirs.charges[0].abs_diff(ours.charges[0]) <= 1,
        "official {theirs:?}, Clustine {ours:?}"
    );
}
