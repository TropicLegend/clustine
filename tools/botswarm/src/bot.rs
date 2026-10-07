//! A bot that joins a server the way a vanilla client does.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use anyhow::{Result, bail, ensure};
use clustine_data::GAME_VERSION;
use clustine_protocol::codec::Reader;
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
    ClientboundPlay, ConfirmTeleportation, Login, ServerboundKeepAlive, SynchronizePlayerPosition,
};
use tokio::time::{Instant, timeout_at};
use uuid::Uuid;

use crate::{Connection, intention};

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
    /// How many packets of each kind arrived, by packet name.
    pub received: BTreeMap<&'static str, u32>,
}

/// A bot in the play state.
pub struct Bot {
    connection: Connection,
    pub info: JoinInfo,
    pub stats: PlayStats,
    /// Where the server last put the bot.
    pub position: Option<SynchronizePlayerPosition>,
}

impl Bot {
    /// Connects to `address` (`host:port`) and goes through login and configuration
    /// until the server has put the bot into the world.
    pub async fn join(address: &str, name: &str) -> Result<Self> {
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

        let ClientboundPlay::Login(login) =
            ClientboundPlay::decode(&connection.read_frame().await?)?
        else {
            bail!("the play state did not start with a login packet");
        };
        ensure!(
            profile.name == name,
            "joined as {} instead of {name}",
            profile.name
        );

        let mut bot = Self {
            connection,
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

    /// Handles the next packet. Returns false if `deadline` passed first.
    async fn step(&mut self, deadline: Instant) -> Result<bool> {
        let Ok(frame) = timeout_at(deadline, self.connection.read_frame()).await else {
            return Ok(false);
        };
        let frame = frame?;
        let packet = ClientboundPlay::decode(&frame)?;
        // Decoding succeeded, so the frame starts with an id that exists in this state.
        let id = Reader::new(&frame).var_int()?;
        let name = packet_ids::play::clientbound::NAMES[id as usize];
        *self.stats.received.entry(name).or_default() += 1;

        match packet {
            ClientboundPlay::ClientboundKeepAlive(packet) => {
                self.connection
                    .write(&ServerboundKeepAlive { id: packet.id })
                    .await?;
                self.stats.keep_alives_answered += 1;
            }
            ClientboundPlay::SynchronizePlayerPosition(packet) => {
                self.connection
                    .write(&ConfirmTeleportation {
                        teleport_id: packet.teleport_id,
                        x: packet.x,
                        y: packet.y,
                        z: packet.z,
                        yaw: packet.yaw,
                        pitch: packet.pitch,
                    })
                    .await?;
                self.stats.teleports_confirmed += 1;
                self.position = Some(packet);
            }
            ClientboundPlay::Disconnect(packet) => {
                bail!("disconnected: {}", packet.reason)
            }
            ClientboundPlay::Login(_) => bail!("received a second login packet"),
            ClientboundPlay::PlayerAbilities(_)
            | ClientboundPlay::GameEvent(_)
            | ClientboundPlay::SetCenterChunk(_)
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
