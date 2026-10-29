//! Comparison 1 of N9 with the official server (`docs/groundwork/n9-plan.md`, section 11):
//! item components on the wire.
//!
//! A creative player (the giver) sets slots of its hotbar to stacks with components:
//! every potion of the registry as a drinkable, a splash and a lingering potion, a
//! tipped arrow, a rocket of flight 3, a crossbow loaded with an arrow, an ominous
//! banner, a suspicious stew and an ominous bottle; and moves them into chests by
//! shift-clicks. A creative slot set echoes nothing to its own player (finding 17), so
//! a **second player** (the looker) opens each chest, and both servers must send it the
//! same `container_set_content` bytes (but the window's id and state, which count what
//! each server did before). The looker then clicks each stack up and back down,
//! guessing the result with the stack's components hashed as the game hashes them
//! (`common/n9_hash.rs`, written from the game's rules); neither server may answer a
//! click with a correction, which it does where its own hash of a component differs.
//! Last, the giver drops one stack of each kind, and the `set_entity_data` bytes the
//! looker is sent for each item (but the entity's id) must be the same.
//!
//! It needs Java, the server jar downloaded by `cargo datagen`, and your agreement to
//! the Minecraft EULA, so it is ignored by default. Run it on a quiet machine:
//!
//! ```text
//! CLUSTINE_ACCEPT_MINECRAFT_EULA=true cargo test -p clustine --locked \
//!     --test components_oracle -- --ignored official_server --nocapture
//! ```

mod common;
#[path = "common/n9_hash.rs"]
mod n9_hash;
#[path = "common/n9_scene.rs"]
#[allow(dead_code)]
mod n9_scene;

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context, Result};
use clustine_data::{entity_types, items};
use clustine_protocol::components::{
    BannerLayer as WireLayer, BannerPattern, Fireworks as WireFireworks,
    PotionContents as WirePotion, Rarity as WireRarity, TooltipDisplay as WireTooltip,
    WireComponent, WireComponents, WireStack, data_component_type, mob_effect, potion,
};
use clustine_protocol::item::ItemStack;
use clustine_protocol::packet_ids::play::clientbound;
use clustine_protocol::packets::play::{
    ClientboundPlay, EntityDataValue, HashedStack, click, face,
};
use clustine_protocol::text::Text;
use clustine_world::Component;
use clustine_world::components::{BannerLayer, Fireworks, PotionContents, Rarity, TooltipDisplay};

use n9_hash::component_hash;
use n9_scene::{GROUND, PATIENCE, Scene, data_bytes, lenient_data, on_both};

/// A stack of the comparison, as the wire carries it and as the world keeps its
/// components, in the same order, which the hash is taken of.
#[derive(Debug, Clone)]
struct Case {
    name: String,
    wire: ItemStack,
    world: Vec<Component>,
}

impl Case {
    fn new(name: impl Into<String>, item: i32, both: Vec<(WireComponent, Component)>) -> Case {
        let (wire, world): (Vec<_>, Vec<_>) = both.into_iter().unzip();
        Case {
            name: name.into(),
            wire: ItemStack {
                item,
                count: 1,
                components: WireComponents {
                    added: wire,
                    removed: Vec::new(),
                },
            },
            world,
        }
    }

    /// The stack as a client guesses it in a click: each component by its kind's id
    /// and the game's hash of it.
    fn hashed(&self) -> HashedStack {
        HashedStack {
            item: self.wire.item,
            count: self.wire.count,
            added: self
                .wire
                .components
                .added
                .iter()
                .zip(&self.world)
                .map(|(wire, world)| (wire.id(), component_hash(world)))
                .collect(),
            removed: Vec::new(),
        }
    }
}

fn index_of(names: &[&str], name: &str) -> i32 {
    let at = names
        .iter()
        .position(|known| *known == name)
        .unwrap_or_else(|| panic!("{name} in its registry"));
    i32::try_from(at).expect("a registry is small")
}

fn potion_contents(name: &str) -> (WireComponent, Component) {
    (
        WireComponent::PotionContents(WirePotion {
            potion: Some(index_of(&potion::NAMES, name)),
            ..WirePotion::default()
        }),
        Component::PotionContents(PotionContents {
            potion: Some(name.to_owned()),
            ..PotionContents::default()
        }),
    )
}

/// The dye colours by id, as the game numbers them.
const DYES: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

/// The ominous banner's layers (`Raid.getOminousBannerInstance`), by pattern and dye.
const OMINOUS_LAYERS: [(&str, &str); 8] = [
    ("minecraft:rhombus", "cyan"),
    ("minecraft:stripe_bottom", "light_gray"),
    ("minecraft:stripe_center", "gray"),
    ("minecraft:border", "light_gray"),
    ("minecraft:stripe_middle", "black"),
    ("minecraft:half_horizontal", "light_gray"),
    ("minecraft:circle", "light_gray"),
    ("minecraft:border", "black"),
];

/// Every stack of the comparison, with the banner patterns' ids from the registry the
/// server at hand sent.
fn cases(scene: &Scene) -> Result<Vec<Case>> {
    let mut cases = Vec::new();
    for name in potion::NAMES {
        for (form, item) in [
            ("potion", items::POTION),
            ("splash", items::SPLASH_POTION),
            ("lingering", items::LINGERING_POTION),
        ] {
            cases.push(Case::new(
                format!("{form} {name}"),
                item,
                vec![potion_contents(name)],
            ));
        }
    }
    cases.extend(kinds(scene)?);
    Ok(cases)
}

/// One stack of each kind beyond the potions, and a potion of each form first: what is
/// dropped.
fn kinds(scene: &Scene) -> Result<Vec<Case>> {
    let mut layers = Vec::new();
    for (pattern, dye) in OMINOUS_LAYERS {
        layers.push((
            WireLayer {
                pattern: BannerPattern::Registered(
                    scene.registered("minecraft:banner_pattern", pattern)?,
                ),
                colour: index_of(&DYES, dye),
            },
            BannerLayer {
                pattern: pattern.to_owned(),
                colour: dye.to_owned(),
            },
        ));
    }
    let (wire_layers, world_layers): (Vec<_>, Vec<_>) = layers.into_iter().unzip();
    Ok(vec![
        Case::new(
            "a potion of swiftness",
            items::POTION,
            vec![potion_contents("minecraft:swiftness")],
        ),
        Case::new(
            "a splash potion of swiftness",
            items::SPLASH_POTION,
            vec![potion_contents("minecraft:swiftness")],
        ),
        Case::new(
            "a lingering potion of swiftness",
            items::LINGERING_POTION,
            vec![potion_contents("minecraft:swiftness")],
        ),
        Case::new(
            "a tipped arrow of poison",
            items::TIPPED_ARROW,
            vec![potion_contents("minecraft:poison")],
        ),
        Case::new(
            "a rocket of flight 3",
            items::FIREWORK_ROCKET,
            vec![(
                WireComponent::Fireworks(WireFireworks {
                    flight_duration: 3,
                    explosions: Vec::new(),
                }),
                Component::Fireworks(Fireworks {
                    flight_duration: 3,
                    explosions: Vec::new(),
                }),
            )],
        ),
        Case::new(
            "a loaded crossbow",
            items::CROSSBOW,
            vec![(
                WireComponent::ChargedProjectiles(vec![WireStack {
                    item: items::ARROW,
                    count: 1,
                    components: WireComponents::default(),
                }]),
                Component::ChargedProjectiles(vec![clustine_world::ItemStack::new(
                    items::ARROW,
                    1,
                )]),
            )],
        ),
        Case::new(
            "an ominous banner",
            items::WHITE_BANNER,
            vec![
                (
                    WireComponent::BannerPatterns(wire_layers),
                    Component::BannerPatterns(world_layers),
                ),
                (
                    WireComponent::ItemName(Text::translatable("block.minecraft.ominous_banner")),
                    Component::ItemName(
                        r#"{"translate":"block.minecraft.ominous_banner"}"#.to_owned(),
                    ),
                ),
                (
                    WireComponent::Rarity(WireRarity::Uncommon),
                    Component::Rarity(Rarity::Uncommon),
                ),
                (
                    WireComponent::TooltipDisplay(WireTooltip {
                        hide_tooltip: false,
                        hidden_components: vec![data_component_type::BANNER_PATTERNS],
                    }),
                    Component::TooltipDisplay(TooltipDisplay {
                        hide_tooltip: false,
                        hidden_components: vec!["minecraft:banner_patterns".to_owned()],
                    }),
                ),
            ],
        ),
        Case::new(
            "a suspicious stew of night vision",
            items::SUSPICIOUS_STEW,
            vec![(
                WireComponent::SuspiciousStewEffects(vec![(
                    index_of(&mob_effect::NAMES, "minecraft:night_vision"),
                    100,
                )]),
                Component::SuspiciousStewEffects(vec![("minecraft:night_vision".to_owned(), 100)]),
            )],
        ),
        Case::new(
            "an ominous bottle",
            items::OMINOUS_BOTTLE,
            vec![(
                WireComponent::OminousBottleAmplifier(2),
                Component::OminousBottleAmplifier(2),
            )],
        ),
    ])
}

/// Where the chests stand about the players, none beside another (two side by side
/// would be one double chest).
const CHESTS: [(i32, i32); 7] = [(2, -2), (2, 0), (2, 2), (-2, -2), (-2, 0), (-2, 2), (0, 2)];

/// The hotbar's slot the players keep empty to open a chest with.
const EMPTY: i16 = 8;

/// What the looker was shown and how the servers answered its clicks.
#[derive(Debug, Default)]
struct Shown {
    /// Each chest's content as sent, but the window's id and state.
    contents: Vec<Vec<u8>>,
    /// The clicks a server answered with a correction, by the stack's name and the
    /// click.
    corrected: Vec<String>,
    /// Each dropped item's data as sent, but the entity's id, by the item.
    dropped: BTreeMap<i32, Vec<u8>>,
}

/// The bytes of a `container_set_content` frame after the window's id and its state:
/// the slots and the carried stack.
fn content_bytes(frame: &[u8]) -> Option<Vec<u8>> {
    let mut reader = clustine_protocol::codec::Reader::new(frame);
    reader.var_int().ok()?;
    reader.var_int().ok()?;
    reader.var_int().ok()?;
    Some(reader.bytes(reader.remaining()).ok()?.to_vec())
}

/// Whether a packet corrects a window's slots or the cursor.
fn corrects(packet: &ClientboundPlay) -> bool {
    match packet {
        ClientboundPlay::ContainerSetSlot(_) | ClientboundPlay::SetCursorItem(_) => true,
        ClientboundPlay::SetContainerContent(content) => content.window_id != 0,
        _ => false,
    }
}

/// Reads what the other player was sent meanwhile, so that it keeps up.
async fn keep_up(scene: &mut Scene) -> Result<()> {
    scene.idle(Duration::from_millis(1)).await
}

async fn wire(address: &str) -> Result<Shown> {
    let mut giver = Scene::join(address, "Giver").await?;
    let mut looker = Scene::join(address, "Looker").await?;
    giver.empty_hotbar().await?;
    looker.empty_hotbar().await?;
    giver.round_trip().await?;
    // The official server puts a joining player anywhere within its spawn radius, so
    // the looker may stand out of reach of the giver's chests: it goes to the giver.
    looker
        .stand_at(giver.feet.0, giver.feet.1, giver.feet.2)
        .await?;
    looker.round_trip().await?;
    let all = cases(&giver)?;
    let (x, z) = giver.column();
    let chests: Vec<(i32, i32, i32)> = CHESTS
        .iter()
        .map(|(dx, dz)| (x + dx, GROUND + 1, z + dz))
        .collect();
    let rows: Vec<&[Case]> = all.chunks(27).collect();
    anyhow::ensure!(
        rows.len() <= chests.len(),
        "more stacks than the chests hold"
    );

    // The giver fills each chest, eight stacks at a time from its hotbar.
    for (row, chest) in rows.iter().zip(&chests) {
        giver
            .place(items::CHEST, (chest.0, GROUND, chest.2), face::TOP, *chest)
            .await?;
        let mut moved = 0;
        for batch in row.chunks(8) {
            for (slot, case) in batch.iter().enumerate() {
                giver.put(slot as i16, case.wire.clone()).await?;
            }
            giver.open(*chest, EMPTY).await?;
            for slot in 0..batch.len() {
                // A chest's window: its 27 slots, the inventory's 27, then the hotbar.
                giver
                    .click(54 + slot as i16, 0, click::QUICK_MOVE, Vec::new(), None)
                    .await?;
            }
            moved += batch.len();
            giver
                .until(PATIENCE, "the chest filled", |scene| {
                    scene.window.as_ref().is_some_and(|window| {
                        window.slots.iter().take(27).flatten().count() >= moved
                    })
                })
                .await?;
            giver.close_window().await?;
            giver.round_trip().await?;
            keep_up(&mut looker).await?;
        }
    }

    // The looker opens each, and clicks each stack up and back.
    let mut shown = Shown::default();
    for (row, chest) in rows.iter().zip(&chests) {
        let from = looker.mark();
        looker.open(*chest, EMPTY).await?;
        let content = looker
            .since(from)
            .iter()
            .find(|seen| seen.id == clientbound::CONTAINER_SET_CONTENT)
            .and_then(|seen| content_bytes(&seen.frame))
            .context("the chest's content")?;
        shown.contents.push(content);
        for (slot, case) in row.iter().enumerate() {
            let slot = slot as i16;
            for (what, changed, carried) in [
                ("picked up", None, Some(case.hashed())),
                ("put back", Some(case.hashed()), None),
            ] {
                let click = looker.mark();
                looker
                    .click(slot, 0, click::PICKUP, vec![(slot, changed)], carried)
                    .await?;
                looker.round_trip().await?;
                let answered = looker
                    .since(click)
                    .iter()
                    .filter_map(|seen| seen.packet.as_ref())
                    .any(corrects);
                if answered {
                    shown.corrected.push(format!("{}: {what}", case.name));
                }
            }
            keep_up(&mut giver).await?;
        }
        looker.close_window().await?;
        looker.round_trip().await?;
    }

    // The giver drops one of each kind; the looker is shown each item.
    let dropped = kinds(&giver)?;
    let from = looker.mark();
    for case in &dropped {
        giver.drop_creative(case.wire.clone()).await?;
        giver.idle_ticks(2).await?;
        keep_up(&mut looker).await?;
    }
    let items_shown = looker
        .wait_added(from, entity_types::ITEM, dropped.len(), PATIENCE)
        .await?;
    looker.round_trip().await?;
    for (_, add) in items_shown {
        let frame = looker
            .since(from)
            .iter()
            .filter(|seen| seen.id == clientbound::SET_ENTITY_DATA)
            .find(|seen| {
                lenient_data(&seen.frame).is_some_and(|(entity, _)| entity == add.entity_id)
            })
            .map(|seen| seen.frame.clone())
            .context("an item's data")?;
        let item = lenient_data(&frame)
            .and_then(|(_, values)| {
                values.into_iter().find_map(|(index, value)| match value {
                    EntityDataValue::ItemStack(Some(stack)) if index == 8 => Some(stack.item),
                    _ => None,
                })
            })
            .unwrap_or(-1);
        shown
            .dropped
            .insert(item, data_bytes(&frame).context("the data's bytes")?);
    }
    Ok(shown)
}

#[tokio::test]
#[ignore = "needs Java, the server jar and agreement to the Minecraft EULA"]
async fn components_are_sent_and_hashed_like_on_the_official_server() {
    let (theirs, ours) = on_both(wire).await;
    let theirs = theirs.expect("the official server");
    let ours = ours.expect("Clustine");
    println!(
        "corrections: official {:?}\nClustine {:?}",
        theirs.corrected, ours.corrected
    );
    // The official server's own answers check the hashes this test sends: a
    // correction there is the test's mistake, not Clustine's.
    assert!(
        theirs.corrected.is_empty(),
        "the official server corrected the test's own guesses: {:?}",
        theirs.corrected
    );
    assert!(
        ours.corrected.is_empty(),
        "Clustine corrected clicks the official server took: {:?}",
        ours.corrected
    );
    assert_eq!(ours.contents.len(), theirs.contents.len());
    for (chest, (theirs, ours)) in theirs.contents.iter().zip(&ours.contents).enumerate() {
        assert_eq!(ours, theirs, "the content of chest {chest}");
    }
    assert_eq!(
        ours.dropped.keys().collect::<Vec<_>>(),
        theirs.dropped.keys().collect::<Vec<_>>(),
        "the items dropped"
    );
    for (item, theirs) in &theirs.dropped {
        assert_eq!(
            ours.dropped.get(item),
            Some(theirs),
            "the data of the dropped item {item}"
        );
    }
}

/// The cases themselves, which need no server: each stack's wire and world forms name
/// the same kinds in the same order, so that each hash goes with its kind.
#[test]
fn each_case_names_its_kinds_alike_on_the_wire_and_in_the_world() {
    let wire_kind = |component: &WireComponent| component.id();
    let world_kind =
        |component: &Component| index_of(&data_component_type::NAMES, component.kind().name());
    let check = |case: &Case| {
        assert_eq!(
            case.wire.components.added.len(),
            case.world.len(),
            "{}",
            case.name
        );
        for (wire, world) in case.wire.components.added.iter().zip(&case.world) {
            assert_eq!(wire_kind(wire), world_kind(world), "{}", case.name);
        }
    };
    for name in potion::NAMES {
        check(&Case::new(
            "a potion",
            items::POTION,
            vec![potion_contents(name)],
        ));
    }
    // The banner's patterns need a registry; their kind is checked with any id.
    check(&Case::new(
        "an ominous banner",
        items::WHITE_BANNER,
        vec![
            (
                WireComponent::BannerPatterns(Vec::new()),
                Component::BannerPatterns(Vec::new()),
            ),
            (
                WireComponent::Rarity(WireRarity::Uncommon),
                Component::Rarity(Rarity::Uncommon),
            ),
        ],
    ));
    assert!(potion::NAMES.len() * 3 + 9 <= CHESTS.len() * 27);
}
