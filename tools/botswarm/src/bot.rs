//! A bot that joins a server the way a vanilla client does.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use clustine_data::{
    BLOCK_STATE_COUNT, DIMENSION_TYPES, DimensionType, GAME_VERSION, entity_types,
};
use clustine_protocol::chunk::{PaletteKind, SectionData, decode_sections};
use clustine_protocol::codec::{Position, Reader};
use clustine_protocol::item::ItemStack;
use clustine_protocol::packet_ids;
use clustine_protocol::packets::configuration::{
    AcknowledgeFinishConfiguration, ClientInformation, ClientboundConfiguration, KnownPack,
    ServerboundKeepAlive as ConfigurationKeepAlive, ServerboundKnownPacks,
    ServerboundPluginMessage,
};
use clustine_protocol::packets::handshake::Intent;
use clustine_protocol::packets::login::{
    ClientboundLogin, LoginAcknowledged, LoginStart, LoginSuccess,
};
use clustine_protocol::packets::play::{
    ChunkBatchReceived, ClientTickEnd, ClientboundPlay, ConfirmTeleportation, LevelChunkWithLight,
    Login, PlayerAction, PlayerLoaded, ServerboundKeepAlive, SetCreativeModeSlot, SetHeldItem,
    SetPlayerPosition, SynchronizePlayerPosition, UseItemOn, face, inventory, movement_flags,
    player_action,
};
use tokio::time::{Instant, timeout_at};
use uuid::Uuid;

use crate::{Connection, intention};

/// The length of a client tick.
const TICK: Duration = Duration::from_millis(50);

/// What a server told a bot while it joined.
#[derive(Debug)]
pub struct JoinInfo {
    pub profile: LoginSuccess,
    /// The compression threshold, if the server enabled compression.
    pub compression: Option<i32>,
    pub feature_flags: Vec<String>,
    /// The packs the server offered to rely on.
    pub offered_packs: Vec<KnownPack>,
    /// Entry names of every synchronised registry, in the order sent; the position of an
    /// entry is its numeric id.
    pub registries: Vec<(String, Vec<String>)>,
    /// Registry entries that came with their full definition instead of relying on a pack.
    pub entries_with_data: usize,
    /// Tags by registry and tag name, with members as numeric ids.
    pub tags: BTreeMap<String, BTreeMap<String, Vec<i32>>>,
    pub login: Login,
}

impl JoinInfo {
    /// The entry names of the synchronised registry called `name`, indexed by id.
    pub fn registry(&self, name: &str) -> Result<&[String]> {
        self.registries
            .iter()
            .find(|(registry, _)| registry == name)
            .map(|(_, entries)| entries.as_slice())
            .with_context(|| format!("the server did not send the registry {name}"))
    }

    /// The dimension type of the dimension the bot joined.
    pub fn dimension_type(&self) -> Result<&'static DimensionType> {
        let types = self.registry("minecraft:dimension_type")?;
        let name = types
            .get(self.login.dimension_type as usize)
            .context("the join packet names a dimension type that was not sent")?;
        DIMENSION_TYPES
            .iter()
            .find(|dimension| dimension.name == name)
            .with_context(|| format!("{name} is not a vanilla dimension type"))
    }

    /// The tags with members as sets of names, which is independent of the numeric ids a
    /// particular server assigned. Members of registries that were not synchronised,
    /// whose ids are fixed by the game, stay numeric.
    pub fn tags_by_name(&self) -> BTreeMap<String, BTreeMap<String, BTreeSet<String>>> {
        let mut by_name = BTreeMap::new();
        for (registry, tags) in &self.tags {
            let names = self
                .registries
                .iter()
                .find(|(name, _)| name == registry)
                .map(|(_, entries)| entries);
            let resolved = tags
                .iter()
                .map(|(tag, members)| {
                    let members = members
                        .iter()
                        .map(|&id| match names.and_then(|names| names.get(id as usize)) {
                            Some(name) => name.clone(),
                            None => id.to_string(),
                        })
                        .collect();
                    (tag.clone(), members)
                })
                .collect();
            by_name.insert(registry.clone(), resolved);
        }
        by_name
    }
}

/// What happened while a bot was in the play state.
#[derive(Debug, Default)]
pub struct PlayStats {
    pub keep_alives_answered: u32,
    pub teleports_confirmed: u32,
    /// How many entities the server made appear and disappear. Something the bot sees
    /// without interruption appears once, however far it moves.
    pub entities_spawned: u32,
    pub entities_removed: u32,
    /// How many packets of each kind arrived, by packet name.
    pub received: BTreeMap<&'static str, u32>,
}

/// How a bot behaves where tests need it to differ from a vanilla client.
#[derive(Debug, Clone)]
pub struct Behaviour {
    /// Whether the bot confirms being moved by the server, as every client does.
    pub confirm_teleports: bool,
}

impl Default for Behaviour {
    fn default() -> Self {
        Self {
            confirm_teleports: true,
        }
    }
}

/// An entity as the bot last heard of it.
#[derive(Debug, Clone, PartialEq)]
pub struct SeenEntity {
    /// For a player, the UUID of their profile.
    pub uuid: Uuid,
    /// Id in the entity type registry.
    pub kind: i32,
    /// From the spawn or the latest absolute position. Relative moves, which the
    /// official server uses for small steps, are not followed.
    pub position: (f64, f64, f64),
    /// In degrees.
    pub yaw: f32,
    /// In 1/256 turns.
    pub head_yaw: u8,
    /// How many absolute positions the server has sent since the spawn.
    pub position_syncs: u32,
}

impl Bot {
    /// The entity of the player called `name`, if the bot currently sees it.
    pub fn seen_player(&self, name: &str) -> Option<&SeenEntity> {
        let (uuid, _) = self
            .player_list
            .iter()
            .find(|(_, listed)| *listed == name)?;
        self.entities.values().find(|entity| entity.uuid == *uuid)
    }
}

/// A bot in the play state.
pub struct Bot {
    connection: Connection,
    behaviour: Behaviour,
    pub info: JoinInfo,
    pub stats: PlayStats,
    /// Where the server last put the bot.
    pub position: Option<SynchronizePlayerPosition>,
    /// The chunks the client currently holds, by chunk x and z: those the server sent
    /// and has not told the client to forget.
    pub chunks: BTreeMap<(i32, i32), LevelChunkWithLight>,
    /// The server's player list as the client knows it: names by profile UUID.
    pub player_list: BTreeMap<Uuid, String>,
    /// The entities the server has shown the bot and not removed again, by entity id.
    pub entities: BTreeMap<i32, SeenEntity>,
    /// The entities the server has removed again, as they were last heard of, in the
    /// order they went. A scenario that judges whether an entity was right to vanish
    /// takes them from here.
    pub vanished: Vec<SeenEntity>,
    /// The items in the hotbar as the server last told the bot, by slot from 0 to 8.
    pub hotbar: [Option<ItemStack>; 9],
    /// The hotbar slot the server last selected for the bot.
    pub selected_slot: i32,
    /// Blocks the server reported as changed since it sent their chunk, by x, y and z.
    block_changes: BTreeMap<(i32, i32, i32), i32>,
    /// The number of the bot's latest action that changes blocks.
    sequence: i32,
    /// Up to which of those actions the server has confirmed handling them.
    pub acknowledged_sequence: i32,
    /// Whether the bot has told the server that it left the loading screen.
    loaded: bool,
    /// The chunk the server last centred the bot's view on.
    pub center: Option<(i32, i32)>,
    /// Where the bot is, as x, y and z. Starts where the server put it and changes when
    /// the bot moves or the server moves it.
    pub location: (f64, f64, f64),
}

impl Bot {
    /// Connects to `address` (`host:port`) and goes through login and configuration
    /// until the server has put the bot into the world.
    pub async fn join(address: &str, name: &str) -> Result<Self> {
        Self::join_with(address, name, Behaviour::default()).await
    }

    /// Like [`Bot::join`], for a bot that deviates from what a vanilla client does.
    pub async fn join_with(address: &str, name: &str, behaviour: Behaviour) -> Result<Self> {
        let mut connection = Connection::connect(address).await?;
        connection
            .write(&intention(address, Intent::Login)?)
            .await?;
        connection
            .write(&LoginStart {
                name: name.to_owned(),
                uuid: Uuid::nil(),
            })
            .await?;

        let mut compression = None;
        let profile = loop {
            match ClientboundLogin::decode(&connection.read_frame().await?)? {
                ClientboundLogin::SetCompression(packet) => {
                    connection.enable_compression(packet.threshold)?;
                    compression = Some(packet.threshold);
                }
                ClientboundLogin::LoginSuccess(profile) => break profile,
                ClientboundLogin::LoginDisconnect(packet) => {
                    bail!("disconnected during login: {}", packet.reason_json)
                }
                ClientboundLogin::Unhandled { id } => bail!(
                    "unsupported login packet {}; is the server in online mode?",
                    packet_ids::login::clientbound::NAMES[id as usize]
                ),
            }
        };
        connection.write(&LoginAcknowledged).await?;

        connection
            .write(&ServerboundPluginMessage {
                channel: "minecraft:brand".to_owned(),
                data: b"\x08botswarm".to_vec(),
            })
            .await?;
        connection
            .write(&ClientInformation {
                locale: "en_us".to_owned(),
                view_distance: 8,
                chat_mode: 0,
                chat_colors: true,
                displayed_skin_parts: 0x7F,
                main_hand: 1,
                text_filtering: false,
                allow_server_listing: true,
                particle_status: 0,
            })
            .await?;

        let mut feature_flags = Vec::new();
        let mut offered_packs = Vec::new();
        let mut registries = Vec::new();
        let mut entries_with_data = 0;
        let mut tags = BTreeMap::new();
        loop {
            match ClientboundConfiguration::decode(&connection.read_frame().await?)? {
                ClientboundConfiguration::FeatureFlags(packet) => feature_flags = packet.features,
                ClientboundConfiguration::ClientboundKnownPacks(packet) => {
                    // Like a vanilla client, the bot has the core pack of its own version.
                    let known = packet
                        .packs
                        .iter()
                        .filter(|pack| {
                            pack.namespace == "minecraft"
                                && pack.id == "core"
                                && pack.version == GAME_VERSION
                        })
                        .cloned()
                        .collect();
                    offered_packs = packet.packs;
                    connection
                        .write(&ServerboundKnownPacks { packs: known })
                        .await?;
                }
                ClientboundConfiguration::RegistryData(packet) => {
                    entries_with_data += packet
                        .entries
                        .iter()
                        .filter(|entry| entry.data.is_some())
                        .count();
                    let entries = packet.entries.into_iter().map(|entry| entry.id).collect();
                    registries.push((packet.registry, entries));
                }
                ClientboundConfiguration::UpdateTags(packet) => {
                    for registry in packet.registries {
                        let by_tag = registry
                            .tags
                            .into_iter()
                            .map(|tag| (tag.name, tag.entries))
                            .collect();
                        tags.insert(registry.registry, by_tag);
                    }
                }
                ClientboundConfiguration::ClientboundKeepAlive(packet) => {
                    connection
                        .write(&ConfigurationKeepAlive { id: packet.id })
                        .await?;
                }
                ClientboundConfiguration::Disconnect(packet) => {
                    bail!("disconnected during configuration: {}", packet.reason)
                }
                ClientboundConfiguration::FinishConfiguration(_) => {
                    connection.write(&AcknowledgeFinishConfiguration).await?;
                    break;
                }
                ClientboundConfiguration::ClientboundPluginMessage(_)
                | ClientboundConfiguration::Unhandled { .. } => {}
            }
        }

        let login = match ClientboundPlay::decode(&connection.read_frame().await?)? {
            ClientboundPlay::Login(login) => login,
            ClientboundPlay::Disconnect(packet) => {
                bail!("disconnected on entering the world: {}", packet.reason)
            }
            _ => bail!("the play state did not start with a login packet"),
        };
        ensure!(
            profile.name == name,
            "joined as {} instead of {name}",
            profile.name
        );

        let mut bot = Self {
            connection,
            behaviour,
            info: JoinInfo {
                profile,
                compression,
                feature_flags,
                offered_packs,
                registries,
                entries_with_data,
                tags,
                login,
            },
            stats: PlayStats::default(),
            position: None,
            chunks: BTreeMap::new(),
            player_list: BTreeMap::new(),
            entities: BTreeMap::new(),
            vanished: Vec::new(),
            hotbar: [None; 9],
            selected_slot: 0,
            block_changes: BTreeMap::new(),
            sequence: 0,
            acknowledged_sequence: 0,
            loaded: false,
            center: None,
            location: (0.0, 0.0, 0.0),
        };
        // Every server places a joining player before anything else can happen.
        let deadline = Instant::now() + Duration::from_secs(30);
        while bot.position.is_none() {
            ensure!(
                bot.step(deadline).await?,
                "the server did not send a position"
            );
        }
        Ok(bot)
    }

    /// Confirms that the bot has been moved by `teleport`, as a client does on its own
    /// unless [`Behaviour::confirm_teleports`] is off.
    pub async fn confirm_teleport(&mut self, teleport: &SynchronizePlayerPosition) -> Result<()> {
        self.connection
            .write(&ConfirmTeleportation {
                teleport_id: teleport.teleport_id,
                x: teleport.x,
                y: teleport.y,
                z: teleport.z,
                yaw: teleport.yaw,
                pitch: teleport.pitch,
            })
            .await?;
        self.stats.teleports_confirmed += 1;
        Ok(())
    }

    /// Gives up the bot's behaviour and returns the raw connection, for tests that need
    /// a client that misbehaves.
    pub fn into_connection(self) -> Connection {
        self.connection
    }

    /// Stays connected for `duration`, answering what the server expects answers to.
    pub async fn idle(&mut self, duration: Duration) -> Result<()> {
        let deadline = Instant::now() + duration;
        while self.step(deadline).await? {}
        Ok(())
    }

    /// Walks in a straight line to `x` and `z` at the current height, moving
    /// `blocks_per_tick` every twentieth of a second like a client does. For comparison:
    /// walking covers about 0.22 blocks per tick and sprinting about 0.28.
    pub async fn walk_to(&mut self, x: f64, z: f64, blocks_per_tick: f64) -> Result<()> {
        loop {
            let (dx, dz) = (x - self.location.0, z - self.location.2);
            let distance = dx.hypot(dz);
            if distance < 1e-9 {
                return Ok(());
            }
            if distance <= blocks_per_tick + 1e-9 {
                // The last step ends exactly where the walk was meant to end, which
                // adding up the steps does not always do.
                (self.location.0, self.location.2) = (x, z);
            } else {
                let fraction = blocks_per_tick / distance;
                self.location.0 += dx * fraction;
                self.location.2 += dz * fraction;
            }
            self.connection
                .write(&SetPlayerPosition {
                    x: self.location.0,
                    y: self.location.1,
                    z: self.location.2,
                    flags: movement_flags::ON_GROUND,
                })
                .await?;
            // The official server disconnects a client that sends a second position
            // before ending its tick.
            self.connection.write(&ClientTickEnd).await?;
            // Handle what the server sends until the next tick is due.
            let next_tick = Instant::now() + TICK;
            while self.step(next_tick).await? {}
        }
    }

    /// Moves straight to `x` and `z` at the current height in a single step, without
    /// waiting for the tick to pass as [`Bot::walk_to`] does.
    pub async fn step_to(&mut self, x: f64, z: f64) -> Result<()> {
        self.location.0 = x;
        self.location.2 = z;
        self.connection
            .write(&SetPlayerPosition {
                x,
                y: self.location.1,
                z,
                flags: movement_flags::ON_GROUND,
            })
            .await?;
        self.connection.write(&ClientTickEnd).await
    }

    /// Breaks the block at `x`, `y`, `z` the way a creative-mode client does, and returns
    /// the sequence number the server will acknowledge.
    pub async fn dig(&mut self, x: i32, y: i32, z: i32) -> Result<i32> {
        self.sequence += 1;
        self.connection
            .write(&PlayerAction {
                status: player_action::START_DESTROY_BLOCK,
                position: Position { x, y, z },
                face: face::TOP,
                sequence: self.sequence,
            })
            .await?;
        Ok(self.sequence)
    }

    /// Selects a hotbar slot, from 0 to 8.
    pub async fn select_slot(&mut self, slot: i16) -> Result<()> {
        self.connection.write(&SetHeldItem { slot }).await
    }

    /// Puts one `item` into a hotbar slot, from 0 to 8, the way a creative-mode client
    /// does when the player picks an item from the creative inventory.
    pub async fn take_from_creative_inventory(&mut self, slot: i16, item: i32) -> Result<()> {
        self.connection
            .write(&SetCreativeModeSlot {
                slot: inventory::HOTBAR_START as i16 + slot,
                stack: Some(ItemStack { item, count: 1 }),
            })
            .await
    }

    /// Uses the held item on a side of the block at `x`, `y`, `z`, which places a held
    /// block against it. `side` is one of the `face` values of the protocol. Returns the
    /// sequence number the server will acknowledge.
    pub async fn use_item_on(&mut self, x: i32, y: i32, z: i32, side: u8) -> Result<i32> {
        self.sequence += 1;
        self.connection
            .write(&UseItemOn {
                hand: 0,
                position: Position { x, y, z },
                face: side.into(),
                cursor: [0.5, 0.5, 0.5],
                inside_block: false,
                world_border_hit: false,
                sequence: self.sequence,
            })
            .await?;
        Ok(self.sequence)
    }

    /// The block state at `x`, `y`, `z` as far as the server has told the bot: from the
    /// chunk it sent and the changes it reported since. `None` if the bot does not hold
    /// the chunk or the height is outside the world.
    pub fn block_at(&self, x: i32, y: i32, z: i32) -> Result<Option<i32>> {
        let Some(sections) = self.sections((x >> 4, z >> 4))? else {
            return Ok(None);
        };
        if let Some(state) = self.block_changes.get(&(x, y, z)) {
            return Ok(Some(*state));
        }
        let above_bottom = y - self.info.dimension_type()?.min_y;
        let Some(section) = usize::try_from(above_bottom / 16)
            .ok()
            .filter(|_| above_bottom >= 0)
            .and_then(|index| sections.get(index))
        else {
            return Ok(None);
        };
        let index = (above_bottom as usize % 16) << 8 | (z as usize & 15) << 4 | (x as usize & 15);
        Ok(Some(section.blocks.get(index)))
    }

    fn forget_block_changes(&mut self, chunk: (i32, i32)) {
        self.block_changes
            .retain(|(x, _, z), _| (x >> 4, z >> 4) != chunk);
    }

    /// Stays connected until `done` holds, or fails once `patience` has run out.
    pub async fn wait_until(
        &mut self,
        patience: Duration,
        mut done: impl FnMut(&Self) -> bool,
    ) -> Result<()> {
        let deadline = Instant::now() + patience;
        while !done(self) {
            ensure!(
                self.step(deadline).await?,
                "the expected state was not reached in time"
            );
        }
        Ok(())
    }

    /// Stays connected until at least `count` chunks have arrived.
    pub async fn wait_for_chunks(&mut self, count: usize, patience: Duration) -> Result<()> {
        let deadline = Instant::now() + patience;
        while self.chunks.len() < count {
            ensure!(
                self.step(deadline).await?,
                "only {} of {count} chunks arrived",
                self.chunks.len()
            );
        }
        Ok(())
    }

    /// The sections of the chunk at `position`, decoded, if the server has sent it.
    pub fn sections(&self, position: (i32, i32)) -> Result<Option<Vec<SectionData>>> {
        let Some(chunk) = self.chunks.get(&position) else {
            return Ok(None);
        };
        let biome_count = self.info.registry("minecraft:worldgen/biome")?.len();
        let dimension = self.info.dimension_type()?;
        let sections = decode_sections(
            &chunk.sections,
            dimension.height as usize / 16,
            PaletteKind::blocks(BLOCK_STATE_COUNT as usize),
            PaletteKind::biomes(biome_count),
        )?;
        Ok(Some(sections))
    }

    /// Handles the next packet. Returns false if `deadline` passed first.
    async fn step(&mut self, deadline: Instant) -> Result<bool> {
        let Ok(frame) = timeout_at(deadline, self.connection.read_frame()).await else {
            return Ok(false);
        };
        let frame = frame?;
        let id = Reader::new(&frame).var_int()?;
        let name = usize::try_from(id)
            .ok()
            .and_then(|id| packet_ids::play::clientbound::NAMES.get(id))
            .copied()
            .unwrap_or("unknown packet");
        let packet = ClientboundPlay::decode(&frame)
            .with_context(|| format!("decoding {name} ({} bytes)", frame.len()))?;
        *self.stats.received.entry(name).or_default() += 1;

        match packet {
            ClientboundPlay::ClientboundKeepAlive(packet) => {
                self.connection
                    .write(&ServerboundKeepAlive { id: packet.id })
                    .await?;
                self.stats.keep_alives_answered += 1;
            }
            ClientboundPlay::SynchronizePlayerPosition(packet) => {
                if self.behaviour.confirm_teleports {
                    self.confirm_teleport(&packet).await?;
                }
                self.location = (packet.x, packet.y, packet.z);
                self.position = Some(packet);
            }
            ClientboundPlay::SetCenterChunk(packet) => {
                self.center = Some((packet.chunk_x, packet.chunk_z));
            }
            ClientboundPlay::PlayerInfoUpdate(packet) => {
                for entry in packet.entries {
                    if let Some((name, _)) = entry.profile {
                        self.player_list.insert(entry.uuid, name);
                    }
                }
            }
            ClientboundPlay::PlayerInfoRemove(packet) => {
                for player in packet.players {
                    self.player_list.remove(&player);
                }
            }
            ClientboundPlay::SpawnEntity(packet) => {
                // A client ignores a player entity whose player it has not been told of.
                ensure!(
                    packet.kind != entity_types::PLAYER
                        || self.player_list.contains_key(&packet.uuid),
                    "player entity {} spawned before its player list entry",
                    packet.entity_id
                );
                // Nor does a server show the same entity twice.
                ensure!(
                    !self.entities.contains_key(&packet.entity_id),
                    "entity {} spawned while it is already there",
                    packet.entity_id
                );
                ensure!(
                    !self
                        .entities
                        .values()
                        .any(|entity| entity.uuid == packet.uuid),
                    "a second entity with the UUID {} spawned",
                    packet.uuid
                );
                self.stats.entities_spawned += 1;
                self.entities.insert(
                    packet.entity_id,
                    SeenEntity {
                        uuid: packet.uuid,
                        kind: packet.kind,
                        position: (packet.x, packet.y, packet.z),
                        yaw: f32::from(packet.yaw) * 360.0 / 256.0,
                        head_yaw: packet.head_yaw,
                        position_syncs: 0,
                    },
                );
            }
            ClientboundPlay::SyncEntityPosition(packet) => {
                if let Some(entity) = self.entities.get_mut(&packet.entity_id) {
                    if let Some(position) = packet.path.end() {
                        entity.position = position;
                    }
                    entity.yaw = packet.yaw;
                    entity.position_syncs += 1;
                }
            }
            ClientboundPlay::SetHeadRotation(packet) => {
                if let Some(entity) = self.entities.get_mut(&packet.entity_id) {
                    entity.head_yaw = packet.head_yaw;
                }
            }
            ClientboundPlay::RemoveEntities(packet) => {
                for entity in packet.entity_ids {
                    if let Some(gone) = self.entities.remove(&entity) {
                        self.stats.entities_removed += 1;
                        self.vanished.push(gone);
                    }
                }
            }
            ClientboundPlay::UnloadChunk(packet) => {
                self.chunks.remove(&(packet.chunk_x, packet.chunk_z));
                self.forget_block_changes((packet.chunk_x, packet.chunk_z));
            }
            ClientboundPlay::BlockUpdate(packet) => {
                let position = packet.position;
                self.block_changes
                    .insert((position.x, position.y, position.z), packet.state);
            }
            ClientboundPlay::SetContainerContent(packet) => {
                if packet.window_id == inventory::PLAYER_WINDOW {
                    for (slot, held) in self.hotbar.iter_mut().enumerate() {
                        *held = packet
                            .slots
                            .get(inventory::HOTBAR_START + slot)
                            .copied()
                            .flatten();
                    }
                }
            }
            ClientboundPlay::SetHeldSlot(packet) => self.selected_slot = packet.slot,
            ClientboundPlay::AcknowledgeBlockChange(packet) => {
                self.acknowledged_sequence = self.acknowledged_sequence.max(packet.sequence);
            }
            ClientboundPlay::Disconnect(packet) => {
                bail!("disconnected: {}", packet.reason)
            }
            ClientboundPlay::LevelChunkWithLight(packet) => {
                // A chunk that is sent again replaces what was known about it.
                self.forget_block_changes((packet.chunk_x, packet.chunk_z));
                self.chunks.insert((packet.chunk_x, packet.chunk_z), packet);
                // A client leaves the loading screen once the chunk it is in has
                // arrived, and tells the server.
                let own_chunk = (
                    (self.location.0.floor() as i32) >> 4,
                    (self.location.2.floor() as i32) >> 4,
                );
                if !self.loaded && self.position.is_some() && self.chunks.contains_key(&own_chunk) {
                    self.loaded = true;
                    self.connection.write(&PlayerLoaded).await?;
                }
            }
            ClientboundPlay::ChunkBatchFinished(_) => {
                // The server waits for this before it sends the next batch.
                self.connection
                    .write(&ChunkBatchReceived {
                        chunks_per_tick: 20.0,
                    })
                    .await?;
            }
            ClientboundPlay::Login(_) => bail!("received a second login packet"),
            ClientboundPlay::ChunkBatchStart(_)
            | ClientboundPlay::PlayerAbilities(_)
            | ClientboundPlay::GameEvent(_)
            | ClientboundPlay::Unhandled { .. } => {}
        }
        Ok(true)
    }
}

impl JoinInfo {
    /// A short human-readable description for the command line.
    pub fn summary(&self) -> String {
        let entries: usize = self
            .registries
            .iter()
            .map(|(_, entries)| entries.len())
            .sum();
        let tag_count: usize = self.tags.values().map(BTreeMap::len).sum();
        format!(
            "joined as {} ({})\n\
             compression threshold: {:?}\n\
             feature flags: {:?}\n\
             offered packs: {}\n\
             registries: {} with {entries} entries, {} sent in full\n\
             tags: {tag_count} in {} registries\n\
             entity id {}, game mode {}, dimension {}, view distance {}",
            self.profile.name,
            self.profile.uuid,
            self.compression,
            self.feature_flags,
            self.offered_packs
                .iter()
                .map(|pack| format!("{}:{}@{}", pack.namespace, pack.id, pack.version))
                .collect::<Vec<_>>()
                .join(", "),
            self.registries.len(),
            self.entries_with_data,
            self.tags.len(),
            self.login.entity_id,
            self.login.game_mode,
            self.login.dimension_name,
            self.login.view_distance,
        )
    }
}
