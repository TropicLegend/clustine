//! A bot that joins a server the way a vanilla client does.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use clustine_data::{
    BLOCK_STATE_COUNT, DIMENSION_TYPES, DimensionType, GAME_VERSION, entity_types,
};
use clustine_protocol::chunk::{PaletteKind, SectionData, decode_sections};
use clustine_protocol::codec::{Position, Reader};
use clustine_protocol::delta::VecDeltaCodec;
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
    ChunkBatchReceived, ClientTickEnd, ClientboundMoveVehicle, ClientboundPlay,
    ConfirmTeleportation, Interact, LevelChunkWithLight, LightUpdate, Login, PaddleBoat,
    PlayerAbilities, PlayerAction, PlayerLoaded, ServerboundKeepAlive, ServerboundMoveVehicle,
    ServerboundPlayerAbilities, ServerboundPlayerInput, ServerboundUseItem, SetCreativeModeSlot,
    SetDefaultSpawnPosition, SetHeldItem, SetPlayerPosition, SetPlayerPositionAndRotation,
    SynchronizePlayerPosition, TakeItemEntity, UseItemOn, VecDelta, face, inventory,
    movement_flags, player_action, player_input, relative,
};
use tokio::time::{Instant, timeout_at};
use uuid::Uuid;

use crate::kept::Journal;
use crate::light::{ChunkLight, LightAt, LightUpdateSeen, Named};
use crate::{Connection, Ending, Kept, Left, Reason, intention};

/// What a server sent of blocks and of what happens at them, as [`Bot::heard`] keeps it.
#[derive(Debug, Clone, PartialEq)]
pub enum Heard {
    /// Blocks changed: one in a `block_update`, or those of a `section_blocks_update`
    /// (`section`), sorted by position.
    Blocks {
        blocks: Vec<((i32, i32, i32), i32)>,
        section: bool,
    },
    /// A `block_event`: a piston's move, a note block's note, a bell's ring.
    BlockEvent {
        at: (i32, i32, i32),
        action: u8,
        param: u8,
        block: i32,
    },
    /// A `sound` at a place, which is in eighths of a block.
    Sound {
        sound: i32,
        source: i32,
        at: (i32, i32, i32),
        volume: f32,
        pitch: f32,
    },
    /// A `level_event`: a block broken, a torch burnt out.
    LevelEvent {
        event: i32,
        at: (i32, i32, i32),
        data: i32,
    },
}

/// The length of a client tick.
const TICK: Duration = Duration::from_millis(50);

/// How many runs of packets of one kind a bot remembers the order of: enough for
/// everything a server sends on entering, without growing for as long as a bot plays.
const ARRIVALS: usize = 256;

/// How many light updates a bot remembers one by one, of those for chunks it holds and
/// of the others each: enough for a test that looks at them and takes them away as it
/// goes, without growing for as long as a bot plays. [`PlayStats`] counts them all.
const LIGHT_UPDATES: usize = 4096;

/// How many light updates a bot's report spells out.
const LIGHT_UPDATES_REPORTED: usize = 32;

/// How an attempt to join ended, for a scenario in which being refused is an answer
/// and not a failure.
pub enum Entry {
    /// The server put the bot into the world.
    Entered(Box<Bot>),
    /// The server sent a disconnect packet instead. `state` says when, in words that
    /// follow "disconnected".
    Refused { state: &'static str, reason: Reason },
}

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

/// Where a relative move takes an entity from `position`, the base a client keeps for
/// it: the end of the move's last step, each step being from the base.
fn moved_by(position: (f64, f64, f64), delta: &VecDelta) -> (f64, f64, f64) {
    let delta = match delta {
        VecDelta::Linear(delta) => *delta,
        VecDelta::Stepped(steps) => match steps.last() {
            Some((_, delta)) => *delta,
            None => return position,
        },
    };
    let codec = VecDeltaCodec::new([position.0, position.1, position.2]);
    let [x, y, z] = codec.decode(delta);
    (x, y, z)
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
    /// How many packets of the play state arrived, the login packet not counted.
    pub packets: u64,
    /// How many light updates came for chunks the bot held when they came.
    pub light_updates: u32,
    /// How many came for chunks it did not hold, which a server has no reason to send.
    pub stray_light_updates: u32,
}

/// How a bot behaves where tests need it to differ from a vanilla client.
#[derive(Debug, Clone)]
pub struct Behaviour {
    /// Whether the bot confirms being moved by the server, as every client does.
    pub confirm_teleports: bool,
    /// The view distance the bot asks for, in chunks. A server grants the smaller of
    /// this and its own.
    pub view_distance: u8,
}

impl Default for Behaviour {
    fn default() -> Self {
        Self {
            confirm_teleports: true,
            view_distance: 8,
        }
    }
}

/// An entity as the bot last heard of it.
#[derive(Debug, Clone, PartialEq)]
pub struct SeenEntity {
    /// The id the server shows it under. A player has another one in every stay.
    pub entity_id: i32,
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
    /// How many relative moves (`move_entity_pos`, `move_entity_pos_rot`) it has
    /// sent; each is followed from the position before, as a client follows it.
    pub relative_moves: u32,
    /// The motion of the spawn or the latest `set_entity_motion`.
    pub motion: [f64; 3],
    /// Whether the latest move had it on the ground.
    pub on_ground: bool,
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
    /// Where the server last said the world's spawn is.
    pub world_spawn: Option<SetDefaultSpawnPosition>,
    /// The chunks the client currently holds, by chunk x and z: those the server sent
    /// and has not told the client to forget.
    pub chunks: BTreeMap<(i32, i32), LevelChunkWithLight>,
    /// When each chunk in [`Bot::chunks`] came, for a test that measures how long a
    /// view takes to fill: the latest sending of it.
    pub chunks_came: BTreeMap<(i32, i32), Instant>,
    /// The server's player list as the client knows it: names by profile UUID.
    pub player_list: BTreeMap<Uuid, String>,
    /// The entities the server has shown the bot and not removed again, by entity id.
    pub entities: BTreeMap<i32, SeenEntity>,
    /// The entities the server has removed again, as they were last heard of, in the
    /// order they went. A scenario that judges whether an entity was right to vanish
    /// takes them from here.
    pub vanished: Vec<SeenEntity>,
    /// Every item the server showed being picked up (`take_item_entity`), in order.
    pub taken: Vec<TakeItemEntity>,
    /// Every block the server showed for a moment where a falling block landed
    /// (`add_transient_block`), with its state, in order.
    pub transient_blocks: Vec<((i32, i32, i32), i32)>,
    /// The items in the hotbar as the server last told the bot, by slot from 0 to 8.
    pub hotbar: [Option<ItemStack>; 9],
    /// The hotbar slot the server last selected for the bot.
    pub selected_slot: i32,
    /// The items and the slot as the bot has them: as the server last said or as the
    /// bot has changed them since, as a client keeps them.
    own_hotbar: [Option<ItemStack>; 9],
    own_slot: i32,
    /// Every state the bot was in since the server last made sure of one, if the bot
    /// was asked to remember; see [`Bot::remember`].
    journal: Option<Journal>,
    /// Blocks the server reported as changed since it sent their chunk, by x, y and z.
    block_changes: BTreeMap<(i32, i32, i32), i32>,
    /// What the light updates since a chunk's packet have named, laid over each other,
    /// and how many there were: only for chunks the bot holds and that had an update.
    /// The light of the chunk's packet is in [`Bot::chunks`]; [`Bot::chunk_light`]
    /// adds the two up.
    light_since: BTreeMap<(i32, i32), (ChunkLight, u32)>,
    /// The light updates that came for chunks the bot held, in the order they came,
    /// that nobody has taken from here yet; at most [`LIGHT_UPDATES`] are kept.
    pub light_updates: Vec<LightUpdateSeen>,
    /// The light updates that came for chunks the bot did not hold, likewise.
    pub stray_light_updates: Vec<LightUpdateSeen>,
    /// Whether the arrays of a light update are kept with it; see
    /// [`Bot::keep_light_arrays`].
    light_arrays_kept: bool,
    /// What the server sent of blocks changing and of what happens at them, in the
    /// order it came and with when, if the bot was asked to record it; see
    /// [`Bot::record_heard`].
    pub heard: Option<Vec<(Instant, Heard)>>,
    /// The number of the bot's latest action that changes blocks.
    sequence: i32,
    /// Up to which of those actions the server has confirmed handling them.
    pub acknowledged_sequence: i32,
    /// Every acknowledgement that has arrived and that nobody has taken from here yet:
    /// the sequence number it carried and when it arrived. A scenario that measures how
    /// long the server took over something takes them from here.
    pub acknowledgements: Vec<(i32, Instant)>,
    /// Whether the bot has told the server that it left the loading screen.
    loaded: bool,
    /// The chunk the server last centred the bot's view on.
    pub center: Option<(i32, i32)>,
    /// Where the bot is, as x, y and z. Starts where the server put it and changes when
    /// the bot moves or the server moves it.
    pub location: (f64, f64, f64),
    /// Where the bot looks, as yaw and pitch in degrees: as the server last turned it or
    /// as it last turned itself.
    pub look: (f32, f32),
    /// Every abilities packet the server has sent, in the order they came.
    pub abilities: Vec<PlayerAbilities>,
    /// Whether the bot flies: as the server last said or as the bot last told it, as a
    /// client keeps it.
    pub flying: bool,
    /// The names of the packets of the play state in the order they came, the login
    /// packet first, each with how many of it came in a row. Only the first
    /// [`ARRIVALS`] runs are kept: it is for what a server sends on entering.
    pub arrivals: Vec<(&'static str, u32)>,
    /// How the connection ended, once it has.
    pub ended: Option<Ending>,
    /// Who rides each vehicle the bot shows, by the vehicle's id, as the server last
    /// said (`set_passengers`); a client forgets a vehicle's passengers when the
    /// vehicle or a passenger is removed, and so does the bot.
    pub passengers: BTreeMap<i32, Vec<i32>>,
    /// Every `set_passengers` that came, in order: the vehicle's id and its passengers.
    pub passenger_log: Vec<(i32, Vec<i32>)>,
    /// Every clientbound `move_vehicle` that came, in order: where the server put back
    /// the vehicle the bot drives.
    pub vehicle_corrections: Vec<ClientboundMoveVehicle>,
    /// The menu the server has opened and not closed, as it showed it.
    pub menu: Option<ShownMenu>,
    /// The stack on the cursor, as the server last said (`set_cursor_item`, or a whole
    /// window's contents).
    pub cursor: Option<ItemStack>,
    /// The windows the server closed (`container_close`), in order.
    pub menus_closed: Vec<i32>,
    /// Every explosion the server showed, in order.
    pub explosions: Vec<clustine_protocol::packets::play::Explode>,
    /// Every particle the server showed (`level_particles`: a sculk sensor's vibration),
    /// in order.
    pub particles: Vec<clustine_protocol::packets::play::LevelParticles>,
    /// Every arm the server showed swinging (`swing_animation`), in order.
    pub swings: Vec<clustine_protocol::packets::play::SwingAnimation>,
    /// Every time the server told (`set_time`), in order.
    pub times: Vec<clustine_protocol::packets::play::SetTime>,
    /// Every hurt the server showed an entity take, in order.
    pub damage_events: Vec<clustine_protocol::packets::play::DamageEvent>,
    /// Every update of an entity's attributes, in order.
    pub attribute_updates: Vec<clustine_protocol::packets::play::UpdateAttributes>,
    /// The experience the server last told.
    pub experience: Option<clustine_protocol::packets::play::SetExperience>,
    /// Every message of the server, in chat or on the action bar, in order.
    pub messages: Vec<clustine_protocol::packets::play::SystemChat>,
    /// Every packet of a menu's window the server sent, in order: openings, a window's
    /// contents (not the inventory's own, window 0), slots, the cursor and closes.
    pub menu_log: Vec<ClientboundPlay>,
}

/// A menu's window as a client shows it: what `open_screen` said, then the slots and the
/// state id as `container_set_content` and `container_set_slot` set them.
#[derive(Debug, Clone, PartialEq)]
pub struct ShownMenu {
    pub window_id: i32,
    pub menu: i32,
    pub title: clustine_protocol::text::Text,
    pub slots: Vec<Option<ItemStack>>,
    pub state_id: i32,
}

impl Bot {
    /// Connects to `address` (`host:port`) and goes through login and configuration
    /// until the server has put the bot into the world.
    pub async fn join(address: &str, name: &str) -> Result<Self> {
        Self::join_with(address, name, Behaviour::default()).await
    }

    /// Like [`Bot::join`], for a bot that deviates from what a vanilla client does.
    pub async fn join_with(address: &str, name: &str, behaviour: Behaviour) -> Result<Self> {
        match Self::attempt(address, name, behaviour).await? {
            Entry::Entered(bot) => Ok(*bot),
            Entry::Refused { state, reason } => bail!("disconnected {state}: {}", reason.wire),
        }
    }

    /// Like [`Bot::join_with`], but a disconnect packet in place of an entry is an
    /// answer: a second login as a name that is connected gets one from some servers.
    pub async fn attempt(address: &str, name: &str, behaviour: Behaviour) -> Result<Entry> {
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
                    return Ok(Entry::Refused {
                        state: "during login",
                        reason: Reason::from_json(&packet.reason_json),
                    });
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
                view_distance: behaviour.view_distance as i8,
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
            let frame = connection.read_frame().await?;
            match ClientboundConfiguration::decode(&frame)? {
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
                ClientboundConfiguration::Disconnect(_) => {
                    return Ok(Entry::Refused {
                        state: "during configuration",
                        reason: Reason::from_frame(&frame)?,
                    });
                }
                ClientboundConfiguration::FinishConfiguration(_) => {
                    connection.write(&AcknowledgeFinishConfiguration).await?;
                    break;
                }
                ClientboundConfiguration::ClientboundPluginMessage(_)
                | ClientboundConfiguration::Unhandled { .. } => {}
            }
        }

        let frame = connection.read_frame().await?;
        let login = match ClientboundPlay::decode(&frame)? {
            ClientboundPlay::Login(login) => login,
            ClientboundPlay::Disconnect(_) => {
                return Ok(Entry::Refused {
                    state: "on entering the world",
                    reason: Reason::from_frame(&frame)?,
                });
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
            world_spawn: None,
            chunks: BTreeMap::new(),
            chunks_came: BTreeMap::new(),
            player_list: BTreeMap::new(),
            entities: BTreeMap::new(),
            vanished: Vec::new(),
            taken: Vec::new(),
            transient_blocks: Vec::new(),
            hotbar: [None; 9],
            selected_slot: 0,
            own_hotbar: [None; 9],
            own_slot: 0,
            journal: None,
            block_changes: BTreeMap::new(),
            light_since: BTreeMap::new(),
            light_updates: Vec::new(),
            stray_light_updates: Vec::new(),
            light_arrays_kept: false,
            heard: None,
            sequence: 0,
            acknowledged_sequence: 0,
            acknowledgements: Vec::new(),
            loaded: false,
            center: None,
            location: (0.0, 0.0, 0.0),
            look: (0.0, 0.0),
            abilities: Vec::new(),
            flying: false,
            arrivals: vec![("minecraft:login", 1)],
            ended: None,
            passengers: BTreeMap::new(),
            passenger_log: Vec::new(),
            vehicle_corrections: Vec::new(),
            menu: None,
            cursor: None,
            menus_closed: Vec::new(),
            explosions: Vec::new(),
            particles: Vec::new(),
            swings: Vec::new(),
            times: Vec::new(),
            damage_events: Vec::new(),
            attribute_updates: Vec::new(),
            experience: None,
            messages: Vec::new(),
            menu_log: Vec::new(),
        };
        // Every server places a joining player before anything else can happen.
        let deadline = Instant::now() + Duration::from_secs(30);
        while bot.position.is_none() {
            match bot.step(deadline).await {
                Ok(true) => {}
                Ok(false) => bail!("the server did not send a position"),
                Err(error) => match bot.ended.take() {
                    Some(Ending::Disconnected(reason)) => {
                        return Ok(Entry::Refused {
                            state: "before it was placed",
                            reason,
                        });
                    }
                    _ => return Err(error),
                },
            }
        }
        Ok(Entry::Entered(Box::new(bot)))
    }

    /// Confirms that the bot has been moved by `teleport`, as a client does on its own
    /// unless [`Behaviour::confirm_teleports`] is off: with where the teleport puts it
    /// and how it looks then ([`Bot::teleported`]), which the official server takes as
    /// its move. Echoing a relative look's values would turn a rider who looks east to
    /// yaw 0 there, as mounting is a teleport with the look relative by nothing.
    pub async fn confirm_teleport(&mut self, teleport: &SynchronizePlayerPosition) -> Result<()> {
        let ((x, y, z), (yaw, pitch)) = self.teleported(teleport);
        self.connection
            .write(&ConfirmTeleportation {
                teleport_id: teleport.teleport_id,
                x,
                y,
                z,
                yaw,
                pitch,
            })
            .await?;
        self.stats.teleports_confirmed += 1;
        Ok(())
    }

    /// Where `teleport` puts the bot and how it looks then: each of its values, with
    /// those it marks relative added to the bot's own (the game's
    /// `PositionMoveRotation.calculateAbsolute`, as a client applies it).
    pub fn teleported(
        &self,
        teleport: &SynchronizePlayerPosition,
    ) -> ((f64, f64, f64), (f32, f32)) {
        let flags = teleport.relative_flags;
        let place = |flag: i32, own: f64, given: f64| {
            if flags & flag != 0 {
                own + given
            } else {
                given
            }
        };
        let turn = |flag: i32, own: f32, given: f32| {
            if flags & flag != 0 {
                own + given
            } else {
                given
            }
        };
        let (x, y, z) = self.location;
        let (yaw, pitch) = self.look;
        (
            (
                place(relative::X, x, teleport.x),
                place(relative::Y, y, teleport.y),
                place(relative::Z, z, teleport.z),
            ),
            (
                turn(relative::Y_ROT, yaw, teleport.yaw),
                turn(relative::X_ROT, pitch, teleport.pitch),
            ),
        )
    }

    /// Gives up the bot's behaviour and returns the raw connection, for tests that need
    /// a client that misbehaves.
    pub fn into_connection(self) -> Connection {
        self.connection
    }

    /// Leaves: ends the connection from the bot's side. What the bot sent before is
    /// still read by the server; nothing can be sent after.
    pub async fn close(&mut self) -> Result<()> {
        self.connection.close().await
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
            self.has_sent();
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
        self.has_sent();
        self.connection.write(&ClientTickEnd).await
    }

    /// Says in a single step that the bot is at `x`, `y` and `z`, wherever that is, as
    /// [`Bot::step_to`] does at the current height. A server that checks movement may
    /// refuse it; one that takes a client at its word puts the player there.
    pub async fn stand_at(&mut self, x: f64, y: f64, z: f64) -> Result<()> {
        self.location = (x, y, z);
        self.connection
            .write(&SetPlayerPosition {
                x,
                y,
                z,
                flags: movement_flags::ON_GROUND,
            })
            .await?;
        self.has_sent();
        self.connection.write(&ClientTickEnd).await
    }

    /// Tells the server that the bot begins (`true`) or stops flying, as a client does
    /// when its player does. A server answers nothing; one that keeps a player's place
    /// lets them enter flying again.
    pub async fn set_flying(&mut self, flying: bool) -> Result<()> {
        self.flying = flying;
        self.connection
            .write(&ServerboundPlayerAbilities::flying(flying))
            .await?;
        self.has_sent();
        Ok(())
    }

    /// Moves in a straight line through the air to `to` (x, y and z) while looking
    /// along `look` (yaw and pitch in degrees), `blocks_per_tick` every twentieth of a
    /// second, never on the ground. At least one position is sent, so the look reaches
    /// the server even where the bot already is at `to`.
    ///
    /// A bot has no gravity and stays wherever it says it is. What keeps a real client
    /// up there is that it flies: see [`Bot::set_flying`].
    pub async fn fly_to(
        &mut self,
        to: (f64, f64, f64),
        look: (f32, f32),
        blocks_per_tick: f64,
    ) -> Result<()> {
        self.look = look;
        loop {
            let (dx, dy, dz) = (
                to.0 - self.location.0,
                to.1 - self.location.1,
                to.2 - self.location.2,
            );
            let distance = (dx * dx + dy * dy + dz * dz).sqrt();
            let arrives = distance <= blocks_per_tick + 1e-9;
            if arrives {
                self.location = to;
            } else {
                let fraction = blocks_per_tick / distance;
                self.location.0 += dx * fraction;
                self.location.1 += dy * fraction;
                self.location.2 += dz * fraction;
            }
            self.connection
                .write(&SetPlayerPositionAndRotation {
                    x: self.location.0,
                    y: self.location.1,
                    z: self.location.2,
                    yaw: look.0,
                    pitch: look.1,
                    flags: 0,
                })
                .await?;
            self.has_sent();
            self.connection.write(&ClientTickEnd).await?;
            let next_tick = Instant::now() + TICK;
            while self.step(next_tick).await? {}
            if arrives {
                return Ok(());
            }
        }
    }

    /// What the bot has that a server keeps across a leave, as a client has it: where
    /// it is and looks, whether it flies, and what it holds, each as the server last
    /// said or as the bot has changed it since.
    pub fn kept(&self) -> Kept {
        Kept {
            location: self.location,
            look: self.look,
            flying: self.flying,
            hotbar: self.own_hotbar,
            selected_slot: self.own_slot,
        }
    }

    /// From now on the bot remembers every state it was in since the server last made
    /// sure of one, so that [`Bot::left`] says what the server can be held to however
    /// the connection ends. To be asked for before the bot sends anything that changes
    /// what it has. A bot that only walks, and never does anything a server
    /// acknowledges, remembers every step: this is for bots that act, or that send a
    /// pulse now and then.
    pub fn remember(&mut self) {
        self.journal = Some(Journal::new(self.kept()));
    }

    /// What the bot would leave with, were its connection to end now: what a server
    /// that keeps a player's place may give it when it joins again; see
    /// [`Left::fault`]. A bot that does not remember is taken at its word, which holds
    /// only after [`Bot::stand_until_kept`].
    pub fn left(&self) -> Left {
        match &self.journal {
            Some(journal) => journal.left(),
            None => Left::exactly(self.kept()),
        }
    }

    /// From now on no acknowledgement makes sure of anything the bot sent. To be said
    /// before the bot's player logs in a second time: what the first connection is
    /// acknowledged after the server has the second is no longer kept.
    pub fn hold_what_is_sure(&mut self) {
        if let Some(journal) = &mut self.journal {
            journal.hold();
        }
    }

    /// Waits until the server has said everything it says a player has on entering,
    /// and returns it: the place and the look of the position packet, the flying bit
    /// of the abilities packet, the hotbar's contents and the held slot.
    pub async fn entered(&mut self, patience: Duration) -> Result<Kept> {
        const SAID: [&str; 4] = [
            "minecraft:player_abilities",
            "minecraft:player_position",
            "minecraft:container_set_content",
            "minecraft:set_held_slot",
        ];
        let unsaid = |bot: &Self| -> Vec<&str> {
            let unsaid = |name: &&str| !bot.stats.received.contains_key(*name);
            SAID.into_iter().filter(unsaid).collect()
        };
        if self
            .wait_until(patience, |bot| unsaid(bot).is_empty())
            .await
            .is_err()
        {
            bail!(
                "the server did not say everything a player has on entering: {} did not come",
                unsaid(self).join(", ")
            );
        }
        let position = self
            .position
            .as_ref()
            .context("a position packet has come")?;
        ensure!(
            position.relative_flags == 0,
            "the server placed the player relative to where a client that has just \
             entered is, which is nowhere: {position:?}"
        );
        Ok(Kept {
            location: (position.x, position.y, position.z),
            look: (position.yaw, position.pitch),
            flying: self
                .abilities
                .last()
                .is_some_and(PlayerAbilities::is_flying),
            hotbar: self.hotbar,
            selected_slot: self.selected_slot,
        })
    }

    /// Stands still until the server has kept everything the bot has: sends an action
    /// that changes nothing, breaking a block far above its head, and waits for the
    /// acknowledgement, which a region gives once it has applied everything sent
    /// before the action and committed it. Unless the bot sends something after this,
    /// [`Bot::left`] is one state then, and a server that keeps places gives exactly
    /// that back.
    pub async fn stand_until_kept(&mut self, patience: Duration) -> Result<()> {
        let (x, y, z) = self.location;
        let sequence = self
            .dig(x.floor() as i32, y.floor() as i32 + 64, z.floor() as i32)
            .await?;
        self.wait_until(patience, |bot| bot.acknowledged_sequence >= sequence)
            .await
            .context("the server did not acknowledge what the bot sent last")
    }

    /// Notes the state the bot is in after something it has just sent.
    fn has_sent(&mut self) {
        let now = self.kept();
        if let Some(journal) = &mut self.journal {
            journal.sent(self.sequence, now);
        }
    }

    /// Notes what the server said the bot has.
    fn was_told(&mut self, change: impl Fn(&mut Kept)) {
        if let Some(journal) = &mut self.journal {
            journal.told(change);
        }
    }

    /// Whether the bot has left the loading screen and told the server so. Before that
    /// the official server does not take its movement.
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// Stays connected until the connection ends, and says how it did; `None` if it is
    /// still there when `patience` has run out.
    pub async fn wait_for_end(&mut self, patience: Duration) -> Option<Ending> {
        let deadline = Instant::now() + patience;
        loop {
            if let Some(ending) = &self.ended {
                return Some(ending.clone());
            }
            // An error leaves its ending behind, which the next round returns.
            if let Ok(false) = self.step(deadline).await {
                return None;
            }
        }
    }

    /// What the server sent the bot and how the connection ended, for a person to read:
    /// the packets in the order they came, every abilities packet, and where and with
    /// which look the server last put the bot.
    pub fn report(&self) -> String {
        use std::fmt::Write as _;

        let mut text = format!(
            "{} ({}), entity {}\n  packets in order:",
            self.info.profile.name, self.info.profile.uuid, self.info.login.entity_id
        );
        for (name, count) in &self.arrivals {
            let name = name.trim_start_matches("minecraft:");
            if *count == 1 {
                let _ = write!(text, " {name}");
            } else {
                let _ = write!(text, " {name} x{count}");
            }
        }
        if self.arrivals.len() == ARRIVALS {
            text.push_str(" (and what came after)");
        }
        text.push_str("\n  abilities packets:");
        if self.abilities.is_empty() {
            text.push_str(" none");
        }
        for packet in &self.abilities {
            let _ = write!(
                text,
                " [flags {:#04x}, flying {}, flying speed {}, field of view {}]",
                packet.flags,
                packet.is_flying(),
                packet.flying_speed,
                packet.field_of_view_modifier
            );
        }
        let _ = write!(
            text,
            "\n  light updates: {} for chunks held, {} for chunks not held",
            self.stats.light_updates, self.stats.stray_light_updates
        );
        let updates = self.light_updates.iter().chain(&self.stray_light_updates);
        for update in updates.clone().take(LIGHT_UPDATES_REPORTED) {
            let _ = write!(text, "\n    {update}");
        }
        let unsaid = updates.count().saturating_sub(LIGHT_UPDATES_REPORTED);
        if unsaid > 0 {
            let _ = write!(text, "\n    (and {unsaid} more that are kept)");
        }
        let _ = write!(
            text,
            "\n  the bot flies: {}\n  last position packet: {:?}\n  positions confirmed: {}\n  \
             the bot is at {:?} looking along {:?}\n  the connection: ",
            self.flying, self.position, self.stats.teleports_confirmed, self.location, self.look
        );
        match &self.ended {
            Some(ending) => {
                let _ = write!(text, "ended with {ending}");
            }
            None => text.push_str("has not ended"),
        }
        text
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

    /// Clicks the slot `slot` of the window shown (`container_click`) with `button` and
    /// the kind of click `input`, saying what the client guesses the slots it changed
    /// (`changed`) and the cursor (`carried`) hold now, as a vanilla client does; the bot
    /// shows its guess at once, as a client does, until the server says otherwise.
    pub async fn click_window(
        &mut self,
        slot: i16,
        button: i8,
        input: i32,
        changed: Vec<(i16, Option<ItemStack>)>,
        carried: Option<ItemStack>,
    ) -> Result<()> {
        let Some(menu) = self.menu.as_mut() else {
            bail!("no window is shown");
        };
        let hashed = |stack: Option<ItemStack>| {
            stack.map(|stack| clustine_protocol::packets::play::HashedStack {
                item: stack.item,
                count: stack.count,
                added: Vec::new(),
                removed: Vec::new(),
            })
        };
        let packet = clustine_protocol::packets::play::ContainerClick {
            window_id: menu.window_id,
            state_id: menu.state_id,
            slot,
            button,
            input,
            changed: changed
                .iter()
                .map(|(slot, stack)| (*slot, hashed(*stack)))
                .collect(),
            carried: hashed(carried),
        };
        for (at, stack) in &changed {
            if let Some(place) = usize::try_from(*at)
                .ok()
                .and_then(|at| menu.slots.get_mut(at))
            {
                *place = *stack;
            }
        }
        self.cursor = carried;
        self.connection.write(&packet).await?;
        self.has_sent();
        Ok(())
    }

    /// Closes the window shown (`container_close`), as a client does when its player
    /// presses escape.
    pub async fn close_window(&mut self) -> Result<()> {
        let Some(menu) = self.menu.take() else {
            bail!("no window is shown");
        };
        self.connection
            .write(
                &clustine_protocol::packets::play::ServerboundContainerClose {
                    window_id: menu.window_id,
                },
            )
            .await?;
        self.has_sent();
        Ok(())
    }

    /// Selects a hotbar slot, from 0 to 8.
    /// Swings the main arm at nothing, as a client does at a click that hits no entity
    /// (`punch`).
    pub async fn punch(&mut self) -> Result<()> {
        self.connection
            .write(&clustine_protocol::packets::play::Punch)
            .await?;
        self.has_sent();
        Ok(())
    }

    pub async fn select_slot(&mut self, slot: i16) -> Result<()> {
        self.connection.write(&SetHeldItem { slot }).await?;
        self.own_slot = slot.into();
        self.has_sent();
        Ok(())
    }

    /// Puts one `item` into a hotbar slot, from 0 to 8, the way a creative-mode client
    /// does when the player picks an item from the creative inventory.
    pub async fn take_from_creative_inventory(&mut self, slot: i16, item: i32) -> Result<()> {
        self.connection
            .write(&SetCreativeModeSlot {
                slot: inventory::HOTBAR_START as i16 + slot,
                stack: Some(ItemStack { item, count: 1 }),
            })
            .await?;
        if let Some(held) = usize::try_from(slot)
            .ok()
            .and_then(|slot| self.own_hotbar.get_mut(slot))
        {
            *held = Some(ItemStack { item, count: 1 });
        }
        self.has_sent();
        Ok(())
    }

    /// Puts `stack` into a hotbar slot, from 0 to 8, the way a creative-mode client does
    /// when its player takes a stack of it from the creative inventory.
    pub async fn put_in_hotbar(&mut self, slot: i16, stack: ItemStack) -> Result<()> {
        self.connection
            .write(&SetCreativeModeSlot {
                slot: inventory::HOTBAR_START as i16 + slot,
                stack: Some(stack),
            })
            .await?;
        if let Some(held) = usize::try_from(slot)
            .ok()
            .and_then(|slot| self.own_hotbar.get_mut(slot))
        {
            *held = Some(stack);
        }
        self.has_sent();
        Ok(())
    }

    /// Empties a hotbar slot, from 0 to 8, the way a creative-mode client does when the
    /// player puts its stack back into the creative inventory. Each server fills a new
    /// player's hotbar its own way, so a test that wants an empty hand empties a slot.
    pub async fn empty_hotbar_slot(&mut self, slot: i16) -> Result<()> {
        self.connection
            .write(&SetCreativeModeSlot {
                slot: inventory::HOTBAR_START as i16 + slot,
                stack: None,
            })
            .await?;
        if let Some(held) = usize::try_from(slot)
            .ok()
            .and_then(|slot| self.own_hotbar.get_mut(slot))
        {
            *held = None;
        }
        self.has_sent();
        Ok(())
    }

    /// Uses the held item on a side of the block at `x`, `y`, `z`, which places a held
    /// block against it. `side` is one of the `face` values of the protocol. Returns the
    /// sequence number the server will acknowledge.
    pub async fn use_item_on(&mut self, x: i32, y: i32, z: i32, side: u8) -> Result<i32> {
        self.click(x, y, z, side, [0.5, 0.5, 0.5]).await
    }

    /// As [`Bot::use_item_on`], with where on the block the click lands, each from 0
    /// to 1: a slab or a trapdoor goes to the half the cursor is in.
    pub async fn click(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        side: u8,
        cursor: [f32; 3],
    ) -> Result<i32> {
        self.sequence += 1;
        self.connection
            .write(&UseItemOn {
                hand: 0,
                position: Position { x, y, z },
                face: side.into(),
                cursor,
                inside_block: false,
                world_border_hit: false,
                sequence: self.sequence,
            })
            .await?;
        Ok(self.sequence)
    }

    /// Uses the held item without aiming at a block for it, looking along `yaw` and
    /// `pitch`: what a client sends for a bucket. Returns the sequence number the
    /// server will acknowledge.
    pub async fn use_item(&mut self, yaw: f32, pitch: f32) -> Result<i32> {
        self.sequence += 1;
        self.connection
            .write(&ServerboundUseItem {
                hand: 0,
                sequence: self.sequence,
                yaw,
                pitch,
            })
            .await?;
        Ok(self.sequence)
    }

    /// Holds down or lets go of the sneak key, and no other.
    pub async fn sneak(&mut self, sneaking: bool) -> Result<()> {
        self.hold_keys(if sneaking { player_input::SHIFT } else { 0 })
            .await
    }

    /// Holds down the movement keys `flags` (a combination of the protocol's
    /// `player_input` values) and lets go of every other, as a client says when one of
    /// them changes. A player who rides a cart pushes it with them.
    pub async fn hold_keys(&mut self, flags: u8) -> Result<()> {
        self.connection
            .write(&ServerboundPlayerInput { flags })
            .await?;
        self.has_sent();
        Ok(())
    }

    /// Uses the main hand on the entity `entity_id`, the aim meeting its box at
    /// `location` relative to the entity's position, without sneaking: what a client
    /// sends to get into a cart or a boat.
    /// Hits the entity shown under `entity_id` (`ServerboundAttackPacket`), from where
    /// the bot stands and looking where it looked last.
    pub async fn attack(&mut self, entity_id: i32) -> Result<()> {
        self.connection
            .write(&clustine_protocol::packets::play::Attack { entity_id })
            .await?;
        self.has_sent();
        Ok(())
    }

    pub async fn interact(&mut self, entity_id: i32, location: [f64; 3]) -> Result<()> {
        self.connection
            .write(&Interact {
                entity_id,
                hand: 0,
                location,
                using_secondary_action: false,
            })
            .await?;
        self.has_sent();
        Ok(())
    }

    /// Says where the bot has moved the vehicle it drives, and ends its tick, as a client
    /// does every tick while it steers a boat.
    pub async fn move_vehicle(
        &mut self,
        position: (f64, f64, f64),
        yaw: f32,
        pitch: f32,
        on_ground: bool,
    ) -> Result<()> {
        self.connection
            .write(&ServerboundMoveVehicle {
                x: position.0,
                y: position.1,
                z: position.2,
                yaw,
                pitch,
                on_ground,
            })
            .await?;
        self.has_sent();
        self.connection.write(&ClientTickEnd).await
    }

    /// Says which of a driven boat's paddles move.
    pub async fn paddle_boat(&mut self, left: bool, right: bool) -> Result<()> {
        self.connection.write(&PaddleBoat { left, right }).await?;
        self.has_sent();
        Ok(())
    }

    /// The vehicle the bot rides as the server last said, if any: the one whose
    /// passengers name the bot's own entity.
    pub fn riding(&self) -> Option<i32> {
        let own = self.info.login.entity_id;
        self.passengers
            .iter()
            .find(|(_, riders)| riders.contains(&own))
            .map(|(vehicle, _)| *vehicle)
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

    /// The blocks of `chunk` the server reported as changed since it sent the chunk,
    /// each as x, y, z and its state: what [`Bot::sections`] does not show.
    pub fn changed_blocks(&self, chunk: (i32, i32)) -> Vec<((i32, i32, i32), i32)> {
        self.block_changes
            .iter()
            .filter(|((x, _, z), _)| (x >> 4, z >> 4) == chunk)
            .map(|(position, state)| (*position, *state))
            .collect()
    }

    /// From now on the bot keeps the arrays of every light update it remembers
    /// ([`LightUpdateSeen::light`]), about 2 KB a section named. For a test that
    /// compares them; a bot that plays for long keeps what was named and no more.
    pub fn keep_light_arrays(&mut self) {
        self.light_arrays_kept = true;
    }

    /// From now on the bot records every block change, block event, sound and level
    /// event it is sent, in [`Bot::heard`], for a test that compares when and in which
    /// order a server sends them. It grows for as long as it is not taken.
    pub fn record_heard(&mut self) {
        self.heard.get_or_insert_with(Vec::new);
    }

    fn hear(&mut self, heard: Heard) {
        if let Some(record) = &mut self.heard {
            record.push((Instant::now(), heard));
        }
    }

    /// How many light sections a chunk of the bot's dimension has: its sections and
    /// one below and one above.
    fn light_sections(&self) -> Result<usize> {
        Ok(self.info.dimension_type()?.height as usize / 16 + 2)
    }

    /// The light of the chunk at `position` as this client has it: what its packets
    /// add up to, the light of the chunk's packet with every light update since laid
    /// over it (`crate::light` says how, and what of that is a guess). `None` if the
    /// bot does not hold the chunk.
    pub fn chunk_light(&self, position: (i32, i32)) -> Result<Option<ChunkLight>> {
        let Some(chunk) = self.chunks.get(&position) else {
            return Ok(None);
        };
        let light = ChunkLight::of(&chunk.light, self.light_sections()?)
            .with_context(|| format!("the light of the chunk packet for {position:?}"))?;
        Ok(Some(match self.light_since.get(&position) {
            Some((since, _)) => light.with(since),
            None => light,
        }))
    }

    /// The sky and block light at the block `x`, `y`, `z` as this client has it, as
    /// far as its packets stated them. `None` if the bot does not hold the chunk or
    /// the height is outside the chunk's light sections, which reach one section
    /// below the world and one above it.
    pub fn light_at(&self, x: i32, y: i32, z: i32) -> Result<Option<LightAt>> {
        let Some(light) = self.chunk_light((x >> 4, z >> 4))? else {
            return Ok(None);
        };
        let above_bottom = y - (self.info.dimension_type()?.min_y - 16);
        let Some(height) = usize::try_from(above_bottom)
            .ok()
            .filter(|height| height / 16 < light.sky.len())
        else {
            return Ok(None);
        };
        let (x, z) = (x as usize & 15, z as usize & 15);
        Ok(Some(LightAt {
            sky: light.sky(x, height, z),
            block: light.block(x, height, z),
        }))
    }

    /// How many light updates have come for the chunk at `position` since the bot was
    /// last sent the chunk. Nothing for a chunk the bot does not hold.
    pub fn light_update_count(&self, position: (i32, i32)) -> u32 {
        self.light_since
            .get(&position)
            .map_or(0, |(_, count)| *count)
    }

    /// The light updates for the chunk at `position` that are still kept in
    /// [`Bot::light_updates`], in the order they came; each says which sections it
    /// named.
    pub fn light_updates_for(
        &self,
        position: (i32, i32),
    ) -> impl Iterator<Item = &LightUpdateSeen> {
        self.light_updates
            .iter()
            .filter(move |update| update.chunk == position)
    }

    /// Takes a light update: lays it over what the bot holds of its chunk and
    /// remembers that it came.
    fn take_light_update(&mut self, packet: LightUpdate) -> Result<()> {
        let chunk = (packet.chunk_x, packet.chunk_z);
        let light_sections = self.light_sections()?;
        let held = self.chunks.contains_key(&chunk);
        let named = if held {
            let (since, count) = self
                .light_since
                .entry(chunk)
                .or_insert_with(|| (ChunkLight::unsaid(light_sections), 0));
            *count += 1;
            since.lay_over(&packet.light)
        } else {
            Named::of(&packet.light, light_sections)
        }
        .with_context(|| format!("a light update for the chunk {chunk:?}"))?;
        let (count, kept) = if held {
            (&mut self.stats.light_updates, &mut self.light_updates)
        } else {
            (
                &mut self.stats.stray_light_updates,
                &mut self.stray_light_updates,
            )
        };
        *count += 1;
        if kept.len() < LIGHT_UPDATES {
            kept.push(LightUpdateSeen {
                chunk,
                held,
                named,
                // This packet has been counted already.
                packets_before: self.stats.packets - 1,
                block_updates_before: self
                    .stats
                    .received
                    .get("minecraft:block_update")
                    .copied()
                    .unwrap_or(0),
                light: self.light_arrays_kept.then_some(packet.light),
            });
        }
        Ok(())
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

    /// Handles the next packet. Returns false if `deadline` passed first. An error is
    /// the end of the connection as far as the bot goes, and [`Bot::ended`] says which.
    async fn step(&mut self, deadline: Instant) -> Result<bool> {
        let handled = self.handle_next(deadline).await;
        if let Err(error) = &handled {
            // An answer that could not be written says that the connection is going,
            // not how: the server may have said why before it closed, and that is
            // still to be read. Reading on finds it, or finds the end.
            let unwritten = error
                .chain()
                .any(|cause| cause.downcast_ref::<std::io::Error>().is_some());
            if !unwritten {
                self.ended
                    .get_or_insert_with(|| Ending::Failed(format!("{error:#}")));
            }
        }
        handled
    }

    async fn handle_next(&mut self, deadline: Instant) -> Result<bool> {
        let Ok(frame) = timeout_at(deadline, self.connection.read_frame()).await else {
            return Ok(false);
        };
        let frame = match frame {
            Ok(frame) => frame,
            Err(error) => {
                self.ended = Some(Ending::Closed(format!("{error:#}")));
                return Err(error);
            }
        };
        let id = Reader::new(&frame).var_int()?;
        let name = usize::try_from(id)
            .ok()
            .and_then(|id| packet_ids::play::clientbound::NAMES.get(id))
            .copied()
            .unwrap_or("unknown packet");
        let packet = ClientboundPlay::decode(&frame)
            .with_context(|| format!("decoding {name} ({} bytes)", frame.len()))?;
        *self.stats.received.entry(name).or_default() += 1;
        self.stats.packets += 1;
        let room = self.arrivals.len() < ARRIVALS;
        match self.arrivals.last_mut() {
            Some((last, count)) if *last == name => *count += 1,
            _ if room => self.arrivals.push((name, 1)),
            _ => {}
        }

        let of_a_menu = match &packet {
            ClientboundPlay::OpenScreen(_)
            | ClientboundPlay::ContainerSetSlot(_)
            | ClientboundPlay::SetCursorItem(_)
            | ClientboundPlay::ClientboundContainerClose(_) => true,
            ClientboundPlay::SetContainerContent(content) => {
                content.window_id != inventory::PLAYER_WINDOW
            }
            _ => false,
        };
        if of_a_menu {
            self.menu_log.push(packet.clone());
        }
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
                (self.location, self.look) = self.teleported(&packet);
                let (location, look) = (self.location, self.look);
                self.was_told(|state| (state.location, state.look) = (location, look));
                self.position = Some(packet);
            }
            ClientboundPlay::SetCenterChunk(packet) => {
                self.center = Some((packet.chunk_x, packet.chunk_z));
            }
            ClientboundPlay::SetDefaultSpawnPosition(packet) => {
                self.world_spawn = Some(packet);
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
                // A client is its own entity and is never shown it: a server that does
                // has lost track of who the player is.
                ensure!(
                    packet.entity_id != self.info.login.entity_id
                        && packet.uuid != self.info.profile.uuid,
                    "the bot was shown its own player as entity {} (it is entity {})",
                    packet.entity_id,
                    self.info.login.entity_id
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
                        entity_id: packet.entity_id,
                        uuid: packet.uuid,
                        kind: packet.kind,
                        position: (packet.x, packet.y, packet.z),
                        yaw: f32::from(packet.yaw) * 360.0 / 256.0,
                        head_yaw: packet.head_yaw,
                        position_syncs: 0,
                        relative_moves: 0,
                        motion: packet.velocity,
                        on_ground: false,
                    },
                );
            }
            ClientboundPlay::SyncEntityPosition(packet) => {
                if let Some(entity) = self.entities.get_mut(&packet.entity_id) {
                    if let Some(position) = packet.path.end() {
                        entity.position = position;
                    }
                    entity.yaw = packet.yaw;
                    entity.on_ground = packet.on_ground;
                    entity.position_syncs += 1;
                }
            }
            ClientboundPlay::MoveEntityPos(packet) => {
                if let Some(entity) = self.entities.get_mut(&packet.entity_id) {
                    entity.position = moved_by(entity.position, &packet.delta);
                    entity.on_ground = packet.on_ground;
                    entity.relative_moves += 1;
                }
            }
            ClientboundPlay::MoveEntityPosRot(packet) => {
                if let Some(entity) = self.entities.get_mut(&packet.entity_id) {
                    entity.position = moved_by(entity.position, &packet.delta);
                    entity.yaw = f32::from(packet.yaw) * 360.0 / 256.0;
                    entity.on_ground = packet.on_ground;
                    entity.relative_moves += 1;
                }
            }
            ClientboundPlay::MoveEntityRot(packet) => {
                if let Some(entity) = self.entities.get_mut(&packet.entity_id) {
                    entity.yaw = f32::from(packet.yaw) * 360.0 / 256.0;
                    entity.on_ground = packet.on_ground;
                }
            }
            ClientboundPlay::SetEntityMotion(packet) => {
                if let Some(entity) = self.entities.get_mut(&packet.entity_id) {
                    entity.motion = packet.motion;
                }
            }
            ClientboundPlay::TakeItemEntity(packet) => self.taken.push(packet),
            ClientboundPlay::SetPassengers(packet) => {
                self.passenger_log
                    .push((packet.vehicle_id, packet.passengers.clone()));
                self.passengers.insert(packet.vehicle_id, packet.passengers);
            }
            ClientboundPlay::ClientboundMoveVehicle(packet) => {
                self.vehicle_corrections.push(packet);
            }
            ClientboundPlay::AddTransientBlock(packet) => {
                let at = packet.position;
                self.transient_blocks
                    .push(((at.x, at.y, at.z), packet.state));
            }
            ClientboundPlay::SetHeadRotation(packet) => {
                if let Some(entity) = self.entities.get_mut(&packet.entity_id) {
                    entity.head_yaw = packet.head_yaw;
                }
            }
            ClientboundPlay::RemoveEntities(packet) => {
                ensure!(
                    !packet.entity_ids.contains(&self.info.login.entity_id),
                    "the bot's own entity {} was removed",
                    self.info.login.entity_id
                );
                for entity in packet.entity_ids {
                    self.passengers.remove(&entity);
                    for riders in self.passengers.values_mut() {
                        riders.retain(|rider| *rider != entity);
                    }
                    if let Some(gone) = self.entities.remove(&entity) {
                        self.stats.entities_removed += 1;
                        self.vanished.push(gone);
                    }
                }
            }
            ClientboundPlay::UnloadChunk(packet) => {
                self.chunks.remove(&(packet.chunk_x, packet.chunk_z));
                self.chunks_came.remove(&(packet.chunk_x, packet.chunk_z));
                self.forget_block_changes((packet.chunk_x, packet.chunk_z));
                self.light_since.remove(&(packet.chunk_x, packet.chunk_z));
            }
            ClientboundPlay::LightUpdate(packet) => self.take_light_update(packet)?,
            ClientboundPlay::BlockUpdate(packet) => {
                let position = packet.position;
                let at = (position.x, position.y, position.z);
                self.block_changes.insert(at, packet.state);
                self.hear(Heard::Blocks {
                    blocks: vec![(at, packet.state)],
                    section: false,
                });
            }
            ClientboundPlay::SectionBlocksUpdate(packet) => {
                let (sx, sy, sz) = packet.section;
                let mut blocks: Vec<_> = packet
                    .blocks
                    .into_iter()
                    .map(|((x, y, z), state)| {
                        let at = (
                            sx * 16 + i32::from(x),
                            sy * 16 + i32::from(y),
                            sz * 16 + i32::from(z),
                        );
                        (at, state)
                    })
                    .collect();
                self.block_changes.extend(blocks.iter().copied());
                if self.heard.is_some() {
                    // A client applies a packet's blocks all at once, so their order in
                    // it says nothing; the official server's is that of a hash set.
                    blocks.sort_unstable();
                    self.hear(Heard::Blocks {
                        blocks,
                        section: true,
                    });
                }
            }
            ClientboundPlay::OpenScreen(packet) => {
                self.menu = Some(ShownMenu {
                    window_id: packet.window_id,
                    menu: packet.menu,
                    title: clustine_protocol::text::Text::from_nbt(packet.title),
                    slots: Vec::new(),
                    state_id: 0,
                });
            }
            ClientboundPlay::ContainerSetSlot(packet) => {
                if let Some(menu) = self
                    .menu
                    .as_mut()
                    .filter(|menu| menu.window_id == packet.window_id)
                    && let Ok(slot) = usize::try_from(packet.slot)
                {
                    if menu.slots.len() <= slot {
                        menu.slots.resize(slot + 1, None);
                    }
                    menu.slots[slot] = packet.stack;
                    menu.state_id = packet.state_id;
                }
            }
            ClientboundPlay::SetCursorItem(packet) => self.cursor = packet.stack,
            ClientboundPlay::ClientboundContainerClose(packet) => {
                if self
                    .menu
                    .as_ref()
                    .is_some_and(|menu| menu.window_id == packet.window_id)
                {
                    self.menu = None;
                }
                self.menus_closed.push(packet.window_id);
            }
            // No menu of N6 has data slots but the crafter's.
            ClientboundPlay::ContainerSetData(_) => {}
            ClientboundPlay::Explode(packet) => self.explosions.push(packet),
            ClientboundPlay::LevelParticles(packet) => self.particles.push(packet),
            ClientboundPlay::SwingAnimation(packet) => self.swings.push(packet),
            ClientboundPlay::SetTime(packet) => self.times.push(packet),
            ClientboundPlay::DamageEvent(packet) => self.damage_events.push(packet),
            ClientboundPlay::UpdateAttributes(packet) => self.attribute_updates.push(packet),
            ClientboundPlay::SetExperience(packet) => self.experience = Some(packet),
            ClientboundPlay::SystemChat(packet) => self.messages.push(packet),
            ClientboundPlay::SetContainerContent(packet) => {
                if let Some(menu) = self
                    .menu
                    .as_mut()
                    .filter(|menu| menu.window_id == packet.window_id)
                {
                    menu.slots.clone_from(&packet.slots);
                    menu.state_id = packet.state_id;
                    self.cursor = packet.carried;
                }
                if packet.window_id == inventory::PLAYER_WINDOW {
                    for (slot, held) in self.hotbar.iter_mut().enumerate() {
                        *held = packet
                            .slots
                            .get(inventory::HOTBAR_START + slot)
                            .copied()
                            .flatten();
                    }
                    let hotbar = self.hotbar;
                    self.own_hotbar = hotbar;
                    self.was_told(|state| state.hotbar = hotbar);
                }
            }
            // A slot the server changed of its own accord, numbered as the game's
            // inventory numbers them: the hotbar is its first nine.
            ClientboundPlay::SetPlayerInventory(packet) => {
                if let Some(slot) = usize::try_from(packet.slot)
                    .ok()
                    .filter(|slot| *slot < self.hotbar.len())
                {
                    self.hotbar[slot] = packet.stack;
                    self.own_hotbar[slot] = packet.stack;
                    let hotbar = self.hotbar;
                    self.was_told(|state| state.hotbar = hotbar);
                }
            }
            ClientboundPlay::SetHeldSlot(packet) => {
                let slot = packet.slot;
                (self.selected_slot, self.own_slot) = (slot, slot);
                self.was_told(|state| state.selected_slot = slot);
            }
            ClientboundPlay::AcknowledgeBlockChange(packet) => {
                self.acknowledged_sequence = self.acknowledged_sequence.max(packet.sequence);
                if let Some(journal) = &mut self.journal {
                    journal.acknowledged(packet.sequence);
                }
                self.acknowledgements
                    .push((packet.sequence, Instant::now()));
            }
            ClientboundPlay::Disconnect(packet) => {
                self.ended = Some(Ending::Disconnected(Reason::from_frame(&frame)?));
                bail!("disconnected: {}", packet.reason)
            }
            ClientboundPlay::PlayerAbilities(packet) => {
                // A client takes the server's word for whether it flies.
                self.flying = packet.is_flying();
                let flying = self.flying;
                self.was_told(|state| state.flying = flying);
                self.abilities.push(packet);
            }
            ClientboundPlay::LevelChunkWithLight(packet) => {
                // A chunk that is sent again replaces what was known about it.
                self.forget_block_changes((packet.chunk_x, packet.chunk_z));
                // Its light too begins anew: the updates before it were for the chunk
                // of before.
                self.light_since.remove(&(packet.chunk_x, packet.chunk_z));
                self.chunks_came
                    .insert((packet.chunk_x, packet.chunk_z), Instant::now());
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
            ClientboundPlay::BlockEvent(packet) => {
                let position = packet.position;
                self.hear(Heard::BlockEvent {
                    at: (position.x, position.y, position.z),
                    action: packet.action,
                    param: packet.param,
                    block: packet.block,
                });
            }
            ClientboundPlay::Sound(packet) => self.hear(Heard::Sound {
                sound: packet.sound,
                source: packet.source,
                at: (packet.x, packet.y, packet.z),
                volume: packet.volume,
                pitch: packet.pitch,
            }),
            ClientboundPlay::LevelEvent(packet) => {
                let position = packet.position;
                self.hear(Heard::LevelEvent {
                    event: packet.event,
                    at: (position.x, position.y, position.z),
                    data: packet.data,
                });
            }
            ClientboundPlay::ChunkBatchStart(_)
            | ClientboundPlay::GameEvent(_)
            | ClientboundPlay::SetEntityData(_)
            // What an armour stand wears is nothing a bot keeps.
            | ClientboundPlay::SetEquipment(_)
            | ClientboundPlay::Animate(_)
            // The bots lead nothing yet, so what is said about leads is not theirs to
            // keep.
            | ClientboundPlay::SetEntityLink(_)
            | ClientboundPlay::SoundEntity(_)
            | ClientboundPlay::EntityEvent(_)
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
