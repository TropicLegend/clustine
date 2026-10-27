//! What a bot was shown of entities as they came into its view, and the comparison of
//! two servers' entities by type and place, for the comparisons with the official
//! server of what generation makes (`chunk_animals_oracle.rs`,
//! `structure_entities_oracle.rs`) and of what an egg makes (`monsters_oracle.rs`).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use clustine::{Config, Land, Server};
use clustine_botswarm::{Behaviour, Bot, Oracle};
use clustine_data::ENTITY_TYPES;
use clustine_protocol::item::ItemStack;
use clustine_protocol::packet_ids::play::clientbound;
use clustine_protocol::packets::play::{AttributeSnapshot, ClientboundPlay, EntityDataValue};
use tokio::time::Instant;

/// An entity as a server first showed it: its spawn and what came with it.
#[derive(Debug, Clone, PartialEq)]
pub struct FirstShown {
    pub entity_id: i32,
    pub kind: i32,
    pub position: (f64, f64, f64),
    /// The spawn's angles, in 1/256 turns.
    pub yaw: u8,
    pub pitch: u8,
    pub head_yaw: u8,
    pub velocity: [f64; 3],
    /// The spawn's own number (a projectile's owner, a falling block's state).
    pub spawn_data: i32,
    /// The data sent with its spawn (`sendPairingData`): none where every value is the
    /// type's default, as the game sends none then.
    pub values: Vec<(u8, EntityDataValue)>,
    /// Its first `update_attributes`, wherever it came: the game sends it with the
    /// spawn, Clustine may send it a tick later.
    pub attributes: Vec<AttributeSnapshot>,
    /// Its first `set_equipment`, wherever it came; none where it wears nothing.
    pub equipment: Vec<(u8, Option<ItemStack>)>,
    /// When its spawn came.
    pub came: Instant,
}

impl FirstShown {
    /// Its type's name, as the registry has it.
    pub fn name(&self) -> &'static str {
        usize::try_from(self.kind)
            .ok()
            .and_then(|kind| ENTITY_TYPES.get(kind))
            .copied()
            .unwrap_or("an unknown type")
    }

    /// The chunk it stands in, by the game's rule.
    pub fn chunk(&self) -> (i32, i32) {
        (
            (self.position.0.floor() as i32).div_euclid(16),
            (self.position.2.floor() as i32).div_euclid(16),
        )
    }

    /// The value at `index` of the data sent with its spawn.
    pub fn value(&self, index: u8) -> Option<&EntityDataValue> {
        self.values
            .iter()
            .find(|(at, _)| *at == index)
            .map(|(_, value)| value)
    }
}

/// Whether `packet` belongs to the pairing of the entity `id` after its spawn: its data,
/// attributes, equipment, passengers, lead, head and motion, and the delimiters of the
/// bundle the game sends them in.
fn pairs_with(packet: &ClientboundPlay, id: i32) -> bool {
    match packet {
        ClientboundPlay::SetEntityData(data) => data.entity_id == id,
        ClientboundPlay::UpdateAttributes(attributes) => attributes.entity_id == id,
        ClientboundPlay::SetEquipment(worn) => worn.entity_id == id,
        ClientboundPlay::SetPassengers(seated) => seated.vehicle_id == id,
        ClientboundPlay::SetEntityLink(link) => link.leashed_id == id,
        ClientboundPlay::SetHeadRotation(head) => head.entity_id == id,
        ClientboundPlay::SetEntityMotion(motion) => motion.entity_id == id,
        ClientboundPlay::Unhandled { id: packet } => *packet == clientbound::BUNDLE_DELIMITER,
        _ => false,
    }
}

/// Every entity `packets` spawn, as first shown, in the order of their spawns. An
/// entity shown again after it was removed keeps its first showing.
pub fn first_shown(packets: &[(Instant, ClientboundPlay)]) -> Vec<FirstShown> {
    let mut shown: Vec<FirstShown> = Vec::new();
    for (at, (came, packet)) in packets.iter().enumerate() {
        let ClientboundPlay::SpawnEntity(spawn) = packet else {
            continue;
        };
        if shown
            .iter()
            .any(|entity| entity.entity_id == spawn.entity_id)
        {
            continue;
        }
        let id = spawn.entity_id;
        let mut entity = FirstShown {
            entity_id: id,
            kind: spawn.kind,
            position: (spawn.x, spawn.y, spawn.z),
            yaw: spawn.yaw,
            pitch: spawn.pitch,
            head_yaw: spawn.head_yaw,
            velocity: spawn.velocity,
            spawn_data: spawn.data,
            values: Vec::new(),
            attributes: Vec::new(),
            equipment: Vec::new(),
            came: *came,
        };
        let after = &packets[at + 1..];
        // The data counts only as part of the pairing: a later one is a change.
        for (_, packet) in after
            .iter()
            .take_while(|(_, packet)| pairs_with(packet, id))
        {
            if let ClientboundPlay::SetEntityData(data) = packet
                && entity.values.is_empty()
            {
                entity.values = data.values.clone();
            }
        }
        let until_removed = after.iter().take_while(|(_, packet)| {
            !matches!(packet, ClientboundPlay::RemoveEntities(removed)
                if removed.entity_ids.contains(&id))
        });
        for (_, packet) in until_removed {
            match packet {
                ClientboundPlay::UpdateAttributes(attributes)
                    if attributes.entity_id == id && entity.attributes.is_empty() =>
                {
                    entity.attributes = attributes.attributes.clone();
                }
                ClientboundPlay::SetEquipment(worn)
                    if worn.entity_id == id && entity.equipment.is_empty() =>
                {
                    entity.equipment = worn.slots.clone();
                }
                _ => {}
            }
        }
        shown.push(entity);
    }
    shown
}

/// The data of an entity, by index, for comparing.
fn sorted_values(values: &[(u8, EntityDataValue)]) -> Vec<(u8, EntityDataValue)> {
    let mut values = values.to_vec();
    values.sort_by_key(|(index, _)| *index);
    values
}

/// The attributes of an entity, each with its modifiers sorted, for comparing.
fn sorted_attributes(attributes: &[AttributeSnapshot]) -> Vec<AttributeSnapshot> {
    let mut attributes = attributes.to_vec();
    attributes.sort_by_key(|attribute| attribute.attribute);
    for attribute in &mut attributes {
        attribute
            .modifiers
            .sort_by(|a, b| a.0.cmp(&b.0).then(a.2.cmp(&b.2)));
    }
    attributes
}

/// What wears what, for comparing, with the empty slots left out (the game sends only
/// the slots that hold something with the spawn).
fn worn(equipment: &[(u8, Option<ItemStack>)]) -> Vec<(u8, ItemStack)> {
    let mut worn: Vec<(u8, ItemStack)> = equipment
        .iter()
        .filter_map(|(slot, stack)| stack.clone().map(|stack| (*slot, stack)))
        .collect();
    worn.sort_by_key(|(slot, _)| *slot);
    worn
}

/// How two servers' entities of one area compare.
#[derive(Debug, Default)]
pub struct Compared {
    /// The pairs matched, by type and place.
    pub matched: usize,
    /// Of those, the pairs at the very same place, whose angles were compared too.
    pub in_place: usize,
    /// Everything that differs, one line each, saying what and where.
    pub differences: Vec<String>,
    /// The pairs matched within the tolerance but not at the same place: Clustine's
    /// had begun to move.
    pub moved: Vec<String>,
}

impl Compared {
    /// A report of the comparison, for a test's output.
    pub fn report(&self, title: &str) -> String {
        let mut report = format!(
            "{title}: {} matched, {} of them in place, {} moved before they were shown, {} differences\n",
            self.matched,
            self.in_place,
            self.moved.len(),
            self.differences.len()
        );
        for line in &self.moved {
            let _ = writeln!(report, "  moved: {line}");
        }
        for line in &self.differences {
            let _ = writeln!(report, "  DIFFERS: {line}");
        }
        report
    }
}

/// Where an entity is, said for a report.
fn place(entity: &FirstShown) -> String {
    format!(
        "{} in chunk {:?} at ({:.4}, {:.4}, {:.4})",
        entity.name(),
        entity.chunk(),
        entity.position.0,
        entity.position.1,
        entity.position.2
    )
}

/// Compares the entities the official server showed, `theirs`, with Clustine's,
/// `ours`: each of theirs is paired with the nearest of ours of its type within
/// `tolerance` blocks in each direction, and the two are compared exactly in the data
/// sent with their spawns, their attributes (bases and modifiers) and what they wear,
/// and, where they stand at the same place to the bit, in their yaw and pitch (an
/// entity that has moved has turned too; the head is never compared, since a mob turns
/// it as it looks about from its first tick). `leave_out` names, for an entity, the
/// indices of its data that depend on draws the two servers make differently, which
/// are not compared. One of theirs with none of ours, or one of ours left over, is a
/// difference, unless `near_the_edge` says it stands where the other server may not
/// have shown it.
pub fn compare(
    theirs: &[FirstShown],
    ours: &[FirstShown],
    tolerance: f64,
    leave_out: impl Fn(&FirstShown) -> Vec<u8>,
    near_the_edge: impl Fn(&FirstShown) -> bool,
) -> Compared {
    let mut compared = Compared::default();
    let mut taken = vec![false; ours.len()];
    for official in theirs {
        let nearest = ours
            .iter()
            .enumerate()
            .filter(|(at, entity)| !taken[*at] && entity.kind == official.kind)
            .map(|(at, entity)| {
                let d = (
                    (entity.position.0 - official.position.0).abs(),
                    (entity.position.1 - official.position.1).abs(),
                    (entity.position.2 - official.position.2).abs(),
                );
                (at, entity, d)
            })
            .filter(|(_, _, d)| d.0 <= tolerance && d.1 <= tolerance && d.2 <= tolerance)
            .min_by(|a, b| {
                let length = |d: &(f64, f64, f64)| d.0 * d.0 + d.1 * d.1 + d.2 * d.2;
                length(&a.2).total_cmp(&length(&b.2))
            });
        let Some((at, clustine, _)) = nearest else {
            if !near_the_edge(official) {
                compared
                    .differences
                    .push(format!("{}: Clustine shows none there", place(official)));
            }
            continue;
        };
        taken[at] = true;
        compared.matched += 1;
        let what = place(official);
        let left_out = leave_out(official);
        let kept = |values: &[(u8, EntityDataValue)]| -> Vec<(u8, EntityDataValue)> {
            sorted_values(values)
                .into_iter()
                .filter(|(index, _)| !left_out.contains(index))
                .collect()
        };
        if kept(&official.values) != kept(&clustine.values) {
            compared.differences.push(format!(
                "{what}: its data: official {:?}, Clustine {:?}",
                kept(&official.values),
                kept(&clustine.values)
            ));
        }
        if sorted_attributes(&official.attributes) != sorted_attributes(&clustine.attributes) {
            compared.differences.push(format!(
                "{what}: its attributes: official {:?}, Clustine {:?}",
                sorted_attributes(&official.attributes),
                sorted_attributes(&clustine.attributes)
            ));
        }
        if worn(&official.equipment) != worn(&clustine.equipment) {
            compared.differences.push(format!(
                "{what}: what it wears: official {:?}, Clustine {:?}",
                worn(&official.equipment),
                worn(&clustine.equipment)
            ));
        }
        let same_bits = |a: f64, b: f64| a.to_bits() == b.to_bits();
        let in_place = same_bits(official.position.0, clustine.position.0)
            && same_bits(official.position.1, clustine.position.1)
            && same_bits(official.position.2, clustine.position.2);
        if in_place {
            compared.in_place += 1;
            if (official.yaw, official.pitch) != (clustine.yaw, clustine.pitch) {
                compared.differences.push(format!(
                    "{what}: its yaw and pitch: official {:?}, Clustine {:?}",
                    (official.yaw, official.pitch),
                    (clustine.yaw, clustine.pitch)
                ));
            }
        } else {
            compared.moved.push(format!(
                "{what}: Clustine's at ({:.4}, {:.4}, {:.4})",
                clustine.position.0, clustine.position.1, clustine.position.2
            ));
        }
    }
    for (at, clustine) in ours.iter().enumerate() {
        if !taken[at] && !near_the_edge(clustine) {
            compared.differences.push(format!(
                "{}: the official server shows none there",
                place(clustine)
            ));
        }
    }
    compared
}

/// How many of each type `shown` holds, by name.
pub fn census(shown: &[FirstShown]) -> BTreeMap<&'static str, usize> {
    let mut census = BTreeMap::new();
    for entity in shown {
        *census.entry(entity.name()).or_default() += 1;
    }
    census
}

/// The height a visiting bot is put at: above any land, where it stays, flying, and
/// out of reach of every mob's looking at players (whose looks would turn their heads
/// and pitches), while the server shows it the area beneath (an entity is shown by its
/// horizontal distance alone, `ChunkMap.TrackedEntity.updatePlayer`).
pub const ABOVE: f64 = 300.0;

/// How long a visit may take: the area is made from nothing on both servers.
pub const VISIT_PATIENCE: Duration = Duration::from_secs(300);

/// The view distance of a visit, in chunks, which both servers grant.
pub const VISIT_VIEW: u32 = 8;

/// The chunks within `reach` of `centre`, in both directions.
pub fn square(centre: (i32, i32), reach: i32) -> Vec<(i32, i32)> {
    let mut chunks = Vec::new();
    for x in centre.0 - reach..=centre.0 + reach {
        for z in centre.1 - reach..=centre.1 + reach {
            chunks.push((x, z));
        }
    }
    chunks
}

/// The bot `name` in `address`'s world, recording what it is sent from its join.
async fn visitor(address: &str, name: &str) -> Result<Bot> {
    let behaviour = Behaviour {
        view_distance: u8::try_from(VISIT_VIEW)?,
        ..Behaviour::default()
    };
    let mut bot = Bot::join_with(address, name, behaviour).await?;
    bot.record_packets();
    Ok(bot)
}

/// Waits until the bot has every chunk of `wanted` and then for the entities' showing
/// that follows a chunk's sending, which the game does at the tracker's next tick, and
/// returns every entity first shown in `wanted`.
async fn shown_in(mut bot: Bot, wanted: &[(i32, i32)]) -> Result<Vec<FirstShown>> {
    bot.wait_until(VISIT_PATIENCE, |bot| {
        wanted.iter().all(|chunk| bot.chunks.contains_key(chunk))
    })
    .await
    .with_context(|| {
        format!(
            "the chunks of the area, of which the bot has {}",
            bot.chunks.len()
        )
    })?;
    // The game shows a chunk's entities a tick after it sends the chunk, Clustine as
    // its regions take them; three seconds is plenty for both.
    bot.idle(Duration::from_secs(3)).await?;
    let packets = bot.packets.take().context("the bot recorded its packets")?;
    bot.close().await?;
    let shown = first_shown(&packets)
        .into_iter()
        .filter(|entity| wanted.contains(&entity.chunk()) && !NOT_GENERATED.contains(&entity.kind))
        .collect();
    Ok(shown)
}

/// The kinds a visit does not compare: players, and what the land's own ticks make once
/// it runs. Clustine's land is not frozen: sand that generation left over air falls
/// and what lost its support drops as an item in the first ticks (a first visit of seed
/// 13579 at chunk (-40, -40) was shown eight falling blocks and ten items), while the
/// frozen official server ticks no block.
const NOT_GENERATED: [i32; 4] = [
    clustine_data::entity_types::PLAYER,
    clustine_data::entity_types::ITEM,
    clustine_data::entity_types::FALLING_BLOCK,
    clustine_data::entity_types::EXPERIENCE_ORB,
];

/// What a bot is shown of the area of `reach` chunks about the chunk `centre` on the
/// official server, frozen: it joins, is moved above the centre by the console's `tp`,
/// and records the first showing of every entity in the area.
pub async fn visit_official(
    oracle: &Oracle,
    name: &str,
    centre: (i32, i32),
    reach: i32,
) -> Result<Vec<FirstShown>> {
    let mut bot = visitor(oracle.address(), name).await?;
    let (x, z) = (
        f64::from(centre.0 * 16) + 8.5,
        f64::from(centre.1 * 16) + 8.5,
    );
    bot.set_flying(true).await?;
    oracle
        .command(&format!("tp {name} {x} {ABOVE} {z}"), &["Teleported"])
        .await?;
    bot.wait_until(VISIT_PATIENCE, |bot| {
        (bot.location.0 - x).abs() < 1e-6 && (bot.location.2 - z).abs() < 1e-6
    })
    .await
    .context("the official server's teleport")?;
    shown_in(bot, &square(centre, reach)).await
}

/// What a bot is shown of the same area on Clustine: it joins and says it is above
/// the centre, which Clustine takes a client's word for, and records as on the
/// official server.
pub async fn visit_clustine(
    address: &str,
    name: &str,
    centre: (i32, i32),
    reach: i32,
) -> Result<Vec<FirstShown>> {
    let mut bot = visitor(address, name).await?;
    let (x, z) = (
        f64::from(centre.0 * 16) + 8.5,
        f64::from(centre.1 * 16) + 8.5,
    );
    bot.set_flying(true).await?;
    bot.stand_at(x, ABOVE, z).await?;
    shown_in(bot, &square(centre, reach)).await
}

/// The structure templates' blocks from the jar `cargo datagen` downloads, which the
/// comparisons need anyway: without them Clustine builds no village house, igloo or
/// outpost, and a world of a seed is not the game's.
pub fn templates() -> Result<std::sync::Arc<clustine_structures::template_blocks::TemplateLibrary>>
{
    let jar = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/datagen")
        .join(clustine_data::GAME_VERSION)
        .join("server.jar");
    ensure!(
        jar.exists(),
        "the server jar is missing at {}; run `cargo datagen` to download it",
        jar.display()
    );
    let templates = clustine_worldgen::templates::read_jar(&jar)
        .context("reading the structure templates of the server jar")?;
    Ok(std::sync::Arc::new(templates))
}

/// Starts Clustine with the world of `seed`, its structures built of the jar's
/// templates, in memory, with the view distance of a visit and regions that follow
/// their players.
pub async fn start_seeded(seed: i64) -> Result<(Server, String)> {
    let config = Config {
        view_distance: i32::try_from(VISIT_VIEW)?,
        ..super::config()
    };
    let land = Land::of_seed_with(seed, Some(templates()?));
    let server = Server::start_in(config, land)
        .await
        .context("starting Clustine")?;
    let address = server.address().to_string();
    Ok((server, address))
}
