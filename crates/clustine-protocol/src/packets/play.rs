//! The play state.
//!
//! Only the packets Clustine uses are modelled; the rest decode as `Unhandled`.

use super::configuration::{Disconnect as ConfigurationDisconnect, empty_packet, keep_alive};
use uuid::Uuid;

use super::login::{MAX_NAME_LENGTH, ProfileProperty};
use super::{Packet, packet_set};
use crate::codec::{Decode, DecodeError, Encode, Position, Reader, Writer};
use crate::item::ItemStack;
use crate::nbt::Nbt;
use crate::packet_ids::play::{clientbound, serverbound};
use crate::text::Text;

/// Game mode ids as used in [`Login`].
pub mod game_mode {
    pub const SURVIVAL: i32 = 0;
    pub const CREATIVE: i32 = 1;
    pub const ADVENTURE: i32 = 2;
    pub const SPECTATOR: i32 = 3;
}

/// Where a player died, shown by the recovery compass.
#[derive(Debug, Clone, PartialEq)]
pub struct DeathLocation {
    pub dimension: String,
    pub position: Position,
}

/// Puts the client into the world: the first packet of the play state.
#[derive(Debug, Clone, PartialEq)]
pub struct Login {
    /// The player's entity id. Clients since 26.2 reject 0.
    pub entity_id: i32,
    pub hardcore: bool,
    /// All dimensions that exist on the server.
    pub dimension_names: Vec<String>,
    /// Unused by the client.
    pub max_players: i32,
    pub view_distance: i32,
    pub simulation_distance: i32,
    pub reduced_debug_info: bool,
    pub enable_respawn_screen: bool,
    pub limited_crafting: bool,
    /// Id in the `minecraft:dimension_type` registry as sent during configuration.
    pub dimension_type: i32,
    /// The dimension the player spawns in.
    pub dimension_name: String,
    pub hashed_seed: i64,
    /// One of the [`game_mode`] ids.
    pub game_mode: i32,
    pub previous_game_mode: Option<i32>,
    pub is_debug: bool,
    /// Flat worlds have their horizon and void fog at a different height.
    pub is_flat: bool,
    pub death_location: Option<DeathLocation>,
    pub portal_cooldown: i32,
    pub sea_level: i32,
    /// Whether the connection was authenticated; false on an offline-mode server.
    pub online_mode: bool,
    pub enforces_secure_chat: bool,
}

impl Packet for Login {
    const ID: i32 = clientbound::LOGIN;
}

impl Encode for Login {
    fn encode(&self, w: &mut Writer) {
        w.put_i32(self.entity_id);
        w.put_bool(self.hardcore);
        w.put_array(&self.dimension_names, |w, name| w.put_string(name));
        w.put_var_int(self.max_players);
        w.put_var_int(self.view_distance);
        w.put_var_int(self.simulation_distance);
        w.put_bool(self.reduced_debug_info);
        w.put_bool(self.enable_respawn_screen);
        w.put_bool(self.limited_crafting);
        w.put_var_int(self.dimension_type);
        w.put_string(&self.dimension_name);
        w.put_i64(self.hashed_seed);
        w.put_var_int(self.game_mode);
        // 0 stands for "none", so real ids are shifted by one.
        w.put_var_int(self.previous_game_mode.map_or(0, |mode| mode + 1));
        w.put_bool(self.is_debug);
        w.put_bool(self.is_flat);
        w.put_option(self.death_location.as_ref(), |w, location| {
            w.put_string(&location.dimension);
            w.put_position(location.position);
        });
        w.put_var_int(self.portal_cooldown);
        w.put_var_int(self.sea_level);
        w.put_bool(self.online_mode);
        w.put_bool(self.enforces_secure_chat);
    }
}

impl Decode for Login {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            entity_id: r.i32()?,
            hardcore: r.bool()?,
            dimension_names: r.array(Reader::identifier)?,
            max_players: r.var_int()?,
            view_distance: r.var_int()?,
            simulation_distance: r.var_int()?,
            reduced_debug_info: r.bool()?,
            enable_respawn_screen: r.bool()?,
            limited_crafting: r.bool()?,
            dimension_type: r.var_int()?,
            dimension_name: r.identifier()?,
            hashed_seed: r.i64()?,
            game_mode: r.var_int()?,
            previous_game_mode: match r.var_int()? {
                0 => None,
                shifted => Some(shifted - 1),
            },
            is_debug: r.bool()?,
            is_flat: r.bool()?,
            death_location: r.option(|r| {
                Ok(DeathLocation {
                    dimension: r.identifier()?,
                    position: r.position()?,
                })
            })?,
            portal_cooldown: r.var_int()?,
            sea_level: r.var_int()?,
            online_mode: r.bool()?,
            enforces_secure_chat: r.bool()?,
        })
    }
}

/// The bits of the flags in [`PlayerAbilities`] and [`ServerboundPlayerAbilities`].
pub mod abilities {
    pub const INVULNERABLE: u8 = 0x01;
    pub const FLYING: u8 = 0x02;
    pub const MAY_FLY: u8 = 0x04;
    /// Breaks blocks instantly, as in creative mode.
    pub const INSTANT_BREAK: u8 = 0x08;
}

/// What the player is allowed to do, mainly depending on the game mode.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerAbilities {
    /// A combination of [`abilities`].
    pub flags: u8,
    pub flying_speed: f32,
    pub field_of_view_modifier: f32,
}

impl Packet for PlayerAbilities {
    const ID: i32 = clientbound::PLAYER_ABILITIES;
}

impl Encode for PlayerAbilities {
    fn encode(&self, w: &mut Writer) {
        w.put_u8(self.flags);
        w.put_f32(self.flying_speed);
        w.put_f32(self.field_of_view_modifier);
    }
}

impl Decode for PlayerAbilities {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            flags: r.u8()?,
            flying_speed: r.f32()?,
            field_of_view_modifier: r.f32()?,
        })
    }
}

impl PlayerAbilities {
    /// Whether the server has the player flying.
    pub fn is_flying(&self) -> bool {
        self.flags & abilities::FLYING != 0
    }
}

/// The client says that its player began or stopped flying: the only ability a client
/// decides for itself. One byte of [`abilities`], of which the official client sets
/// nothing but [`abilities::FLYING`].
///
/// It is not yet among [`ServerboundPlay`], where it decodes as `Unhandled`: the edge
/// matches every packet of that set, and is given this one in the step that reads it
/// (ADR-0020, R1.4).
#[derive(Debug, Clone, PartialEq)]
pub struct ServerboundPlayerAbilities {
    pub flags: u8,
}

impl ServerboundPlayerAbilities {
    /// What a client sends when its player begins (`true`) or stops flying.
    pub fn flying(flying: bool) -> Self {
        Self {
            flags: if flying { abilities::FLYING } else { 0 },
        }
    }

    pub fn is_flying(&self) -> bool {
        self.flags & abilities::FLYING != 0
    }
}

impl Packet for ServerboundPlayerAbilities {
    const ID: i32 = serverbound::PLAYER_ABILITIES;
}

impl Encode for ServerboundPlayerAbilities {
    fn encode(&self, w: &mut Writer) {
        w.put_u8(self.flags);
    }
}

impl Decode for ServerboundPlayerAbilities {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self { flags: r.u8()? })
    }
}

/// Moves the player, which the client has to confirm with [`ConfirmTeleportation`].
#[derive(Debug, Clone, PartialEq)]
pub struct SynchronizePlayerPosition {
    pub teleport_id: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub velocity_x: f64,
    pub velocity_y: f64,
    pub velocity_z: f64,
    pub yaw: f32,
    pub pitch: f32,
    /// Marks components as relative to the current value; 0 means all are absolute.
    pub relative_flags: i32,
}

impl Packet for SynchronizePlayerPosition {
    const ID: i32 = clientbound::PLAYER_POSITION;
}

impl Encode for SynchronizePlayerPosition {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.teleport_id);
        for value in [self.x, self.y, self.z] {
            w.put_f64(value);
        }
        for value in [self.velocity_x, self.velocity_y, self.velocity_z] {
            w.put_f64(value);
        }
        w.put_f32(self.yaw);
        w.put_f32(self.pitch);
        w.put_i32(self.relative_flags);
    }
}

impl Decode for SynchronizePlayerPosition {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            teleport_id: r.var_int()?,
            x: r.f64()?,
            y: r.f64()?,
            z: r.f64()?,
            velocity_x: r.f64()?,
            velocity_y: r.f64()?,
            velocity_z: r.f64()?,
            yaw: r.f32()?,
            pitch: r.f32()?,
            relative_flags: r.i32()?,
        })
    }
}

/// Confirms a [`SynchronizePlayerPosition`] and reports where the client now is.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfirmTeleportation {
    pub teleport_id: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
}

impl Packet for ConfirmTeleportation {
    const ID: i32 = serverbound::ACCEPT_TELEPORTATION;
}

impl Encode for ConfirmTeleportation {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.teleport_id);
        for value in [self.x, self.y, self.z] {
            w.put_f64(value);
        }
        w.put_f32(self.yaw);
        w.put_f32(self.pitch);
    }
}

impl Decode for ConfirmTeleportation {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            teleport_id: r.var_int()?,
            x: r.f64()?,
            y: r.f64()?,
            z: r.f64()?,
            yaw: r.f32()?,
            pitch: r.f32()?,
        })
    }
}

/// Game event ids as used in [`GameEvent`].
pub mod game_event {
    /// The client may leave the loading screen once the chunk it stands in has arrived.
    pub const START_WAITING_FOR_LEVEL_CHUNKS: u8 = 13;
}

/// A state change that is not tied to an entity or block.
#[derive(Debug, Clone, PartialEq)]
pub struct GameEvent {
    /// One of the [`game_event`] ids.
    pub event: u8,
    pub value: f32,
}

impl Packet for GameEvent {
    const ID: i32 = clientbound::GAME_EVENT;
}

impl Encode for GameEvent {
    fn encode(&self, w: &mut Writer) {
        w.put_u8(self.event);
        w.put_f32(self.value);
    }
}

impl Decode for GameEvent {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            event: r.u8()?,
            value: r.f32()?,
        })
    }
}

/// Tells the client which chunk its loaded area is centred on.
#[derive(Debug, Clone, PartialEq)]
pub struct SetCenterChunk {
    pub chunk_x: i32,
    pub chunk_z: i32,
}

impl Packet for SetCenterChunk {
    const ID: i32 = clientbound::SET_CHUNK_CACHE_CENTER;
}

impl Encode for SetCenterChunk {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.chunk_x);
        w.put_var_int(self.chunk_z);
    }
}

impl Decode for SetCenterChunk {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            chunk_x: r.var_int()?,
            chunk_z: r.var_int()?,
        })
    }
}

keep_alive!(
    /// Must be answered with a [`ServerboundKeepAlive`] carrying the same id.
    ClientboundKeepAlive,
    clientbound::KEEP_ALIVE
);
keep_alive!(
    /// The answer to a [`ClientboundKeepAlive`].
    ServerboundKeepAlive,
    serverbound::KEEP_ALIVE
);

empty_packet!(
    /// Sent by the client at the end of each of its ticks.
    ClientTickEnd,
    serverbound::CLIENT_TICK_END
);

empty_packet!(
    /// Sent by the client once it has left the loading screen after joining or
    /// respawning. Until then the official server does not accept its movement.
    PlayerLoaded,
    serverbound::PLAYER_LOADED
);

/// Ends the connection with a message shown to the player.
#[derive(Debug, Clone, PartialEq)]
pub struct Disconnect {
    /// A text component ([`crate::text::Text`]); a plain string tag is the simplest
    /// form.
    pub reason: Nbt,
}

impl Disconnect {
    /// The reason as a text component.
    pub fn text(&self) -> Text {
        Text::from_nbt(self.reason.clone())
    }
}

impl From<Text> for Disconnect {
    fn from(reason: Text) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl Packet for Disconnect {
    const ID: i32 = clientbound::DISCONNECT;
}

impl Encode for Disconnect {
    fn encode(&self, w: &mut Writer) {
        ConfigurationDisconnect {
            reason: self.reason.clone(),
        }
        .encode_fields(w);
    }
}

impl Decode for Disconnect {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let reason = ConfigurationDisconnect::decode_fields(r)?.reason;
        Ok(Self { reason })
    }
}

/// Heightmap kinds the client uses, as ids in [`Heightmap`].
pub mod heightmap {
    /// Highest block that is not air.
    pub const WORLD_SURFACE: i32 = 1;
    /// Highest block that blocks motion or contains a fluid.
    pub const MOTION_BLOCKING: i32 = 4;
    /// Like `MOTION_BLOCKING`, ignoring leaves.
    pub const MOTION_BLOCKING_NO_LEAVES: i32 = 5;
}

/// One heightmap of a chunk; see [`crate::chunk::pack_heightmap`].
#[derive(Debug, Clone, PartialEq)]
pub struct Heightmap {
    /// One of the [`heightmap`] ids.
    pub kind: i32,
    pub data: Vec<u64>,
}

/// A block entity inside a chunk, such as a chest or a sign.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockEntity {
    /// The position within the chunk: `x << 4 | z`.
    pub packed_xz: u8,
    pub y: i16,
    /// Id in the block entity type registry.
    pub kind: i32,
    pub data: Option<Nbt>,
}

/// Light levels of a chunk column.
///
/// Light is stored per section for the sections of the column plus one below and one
/// above, from bottom to top. Bit `i` of a mask refers to the `i`-th of those. A section
/// is either listed in a light mask, with a 2048-byte array of 4-bit levels following in
/// the same order, or in the matching empty mask, meaning all zero, or in neither.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LightData {
    pub sky_mask: Vec<u64>,
    pub block_mask: Vec<u64>,
    pub empty_sky_mask: Vec<u64>,
    pub empty_block_mask: Vec<u64>,
    pub sky: Vec<Vec<u8>>,
    pub block: Vec<Vec<u8>>,
}

impl LightData {
    fn encode(&self, w: &mut Writer) {
        w.put_bit_set(&self.sky_mask);
        w.put_bit_set(&self.block_mask);
        w.put_bit_set(&self.empty_sky_mask);
        w.put_bit_set(&self.empty_block_mask);
        for arrays in [&self.sky, &self.block] {
            w.put_array(arrays, |w, array| {
                w.put_length(array.len());
                w.put_bytes(array);
            });
        }
    }

    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        fn arrays(r: &mut Reader<'_>) -> Result<Vec<Vec<u8>>, DecodeError> {
            r.array(|r| {
                let length = r.length()?;
                Ok(r.bytes(length)?.to_vec())
            })
        }
        Ok(Self {
            sky_mask: r.bit_set()?,
            block_mask: r.bit_set()?,
            empty_sky_mask: r.bit_set()?,
            empty_block_mask: r.bit_set()?,
            sky: arrays(r)?,
            block: arrays(r)?,
        })
    }
}

/// A chunk column with its light.
#[derive(Debug, Clone, PartialEq)]
pub struct LevelChunkWithLight {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub heightmaps: Vec<Heightmap>,
    /// The sections, bottom to top; see [`crate::chunk::encode_sections`].
    pub sections: Vec<u8>,
    pub block_entities: Vec<BlockEntity>,
    pub light: LightData,
}

impl Packet for LevelChunkWithLight {
    const ID: i32 = clientbound::LEVEL_CHUNK_WITH_LIGHT;
}

impl Encode for LevelChunkWithLight {
    fn encode(&self, w: &mut Writer) {
        w.put_i32(self.chunk_x);
        w.put_i32(self.chunk_z);
        w.put_array(&self.heightmaps, |w, heightmap| {
            w.put_var_int(heightmap.kind);
            w.put_array(&heightmap.data, |w, word| w.put_u64(*word));
        });
        w.put_length(self.sections.len());
        w.put_bytes(&self.sections);
        w.put_array(&self.block_entities, |w, entity| {
            w.put_u8(entity.packed_xz);
            w.put_i16(entity.y);
            w.put_var_int(entity.kind);
            match &entity.data {
                Some(data) => w.put_nbt(data),
                None => w.put_u8(0),
            }
        });
        self.light.encode(w);
    }
}

impl Decode for LevelChunkWithLight {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            chunk_x: r.i32()?,
            chunk_z: r.i32()?,
            heightmaps: r.array(|r| {
                Ok(Heightmap {
                    kind: r.var_int()?,
                    data: r.array(Reader::u64)?,
                })
            })?,
            sections: {
                let length = r.length()?;
                r.bytes(length)?.to_vec()
            },
            block_entities: r.array(|r| {
                Ok(BlockEntity {
                    packed_xz: r.u8()?,
                    y: r.i16()?,
                    kind: r.var_int()?,
                    data: r.nbt()?,
                })
            })?,
            light: LightData::decode(r)?,
        })
    }
}

empty_packet!(
    /// Announces a batch of chunk packets, which ends with [`ChunkBatchFinished`].
    ChunkBatchStart,
    clientbound::CHUNK_BATCH_START
);

/// Ends a batch of chunk packets. The client answers with [`ChunkBatchReceived`].
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkBatchFinished {
    /// Number of chunks in the batch.
    pub batch_size: i32,
}

impl Packet for ChunkBatchFinished {
    const ID: i32 = clientbound::CHUNK_BATCH_FINISHED;
}

impl Encode for ChunkBatchFinished {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.batch_size);
    }
}

impl Decode for ChunkBatchFinished {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            batch_size: r.var_int()?,
        })
    }
}

/// Confirms a chunk batch and tells the server how fast the client can take chunks.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkBatchReceived {
    pub chunks_per_tick: f32,
}

impl Packet for ChunkBatchReceived {
    const ID: i32 = serverbound::CHUNK_BATCH_RECEIVED;
}

impl Encode for ChunkBatchReceived {
    fn encode(&self, w: &mut Writer) {
        w.put_f32(self.chunks_per_tick);
    }
}

impl Decode for ChunkBatchReceived {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            chunks_per_tick: r.f32()?,
        })
    }
}

/// Bits of the flags byte that ends every movement packet.
pub mod movement_flags {
    /// The player stands on something.
    pub const ON_GROUND: u8 = 0x01;
    /// The player is pushing against a wall.
    pub const HORIZONTAL_COLLISION: u8 = 0x02;
}

/// The player moved without turning. `y` is the height of the feet.
#[derive(Debug, Clone, PartialEq)]
pub struct SetPlayerPosition {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    /// A combination of [`movement_flags`].
    pub flags: u8,
}

impl Packet for SetPlayerPosition {
    const ID: i32 = serverbound::MOVE_PLAYER_POS;
}

impl Encode for SetPlayerPosition {
    fn encode(&self, w: &mut Writer) {
        for value in [self.x, self.y, self.z] {
            w.put_f64(value);
        }
        w.put_u8(self.flags);
    }
}

impl Decode for SetPlayerPosition {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            x: r.f64()?,
            y: r.f64()?,
            z: r.f64()?,
            flags: r.u8()?,
        })
    }
}

/// The player moved and turned. `y` is the height of the feet.
#[derive(Debug, Clone, PartialEq)]
pub struct SetPlayerPositionAndRotation {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    /// A combination of [`movement_flags`].
    pub flags: u8,
}

impl Packet for SetPlayerPositionAndRotation {
    const ID: i32 = serverbound::MOVE_PLAYER_POS_ROT;
}

impl Encode for SetPlayerPositionAndRotation {
    fn encode(&self, w: &mut Writer) {
        for value in [self.x, self.y, self.z] {
            w.put_f64(value);
        }
        w.put_f32(self.yaw);
        w.put_f32(self.pitch);
        w.put_u8(self.flags);
    }
}

impl Decode for SetPlayerPositionAndRotation {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            x: r.f64()?,
            y: r.f64()?,
            z: r.f64()?,
            yaw: r.f32()?,
            pitch: r.f32()?,
            flags: r.u8()?,
        })
    }
}

/// The player turned without moving.
#[derive(Debug, Clone, PartialEq)]
pub struct SetPlayerRotation {
    pub yaw: f32,
    pub pitch: f32,
    /// A combination of [`movement_flags`].
    pub flags: u8,
}

impl Packet for SetPlayerRotation {
    const ID: i32 = serverbound::MOVE_PLAYER_ROT;
}

impl Encode for SetPlayerRotation {
    fn encode(&self, w: &mut Writer) {
        w.put_f32(self.yaw);
        w.put_f32(self.pitch);
        w.put_u8(self.flags);
    }
}

impl Decode for SetPlayerRotation {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            yaw: r.f32()?,
            pitch: r.f32()?,
            flags: r.u8()?,
        })
    }
}

/// Only the player's movement flags changed. Also sent regularly while standing still.
#[derive(Debug, Clone, PartialEq)]
pub struct SetPlayerMovementFlags {
    /// A combination of [`movement_flags`].
    pub flags: u8,
}

impl Packet for SetPlayerMovementFlags {
    const ID: i32 = serverbound::MOVE_PLAYER_STATUS_ONLY;
}

impl Encode for SetPlayerMovementFlags {
    fn encode(&self, w: &mut Writer) {
        w.put_u8(self.flags);
    }
}

impl Decode for SetPlayerMovementFlags {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self { flags: r.u8()? })
    }
}

/// Tells the client to forget a chunk that left its view.
#[derive(Debug, Clone, PartialEq)]
pub struct UnloadChunk {
    pub chunk_x: i32,
    pub chunk_z: i32,
}

impl Packet for UnloadChunk {
    const ID: i32 = clientbound::FORGET_LEVEL_CHUNK;
}

impl Encode for UnloadChunk {
    fn encode(&self, w: &mut Writer) {
        // Unlike everywhere else, z comes first.
        w.put_i32(self.chunk_z);
        w.put_i32(self.chunk_x);
    }
}

impl Decode for UnloadChunk {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let chunk_z = r.i32()?;
        let chunk_x = r.i32()?;
        Ok(Self { chunk_x, chunk_z })
    }
}

/// Bits of [`PlayerInfoUpdate::actions`]. They decide which fields the entries carry.
pub mod player_info {
    pub const ADD_PLAYER: u8 = 0x01;
    pub const INITIALIZE_CHAT: u8 = 0x02;
    pub const UPDATE_GAME_MODE: u8 = 0x04;
    pub const UPDATE_LISTED: u8 = 0x08;
    pub const UPDATE_LATENCY: u8 = 0x10;
    pub const UPDATE_DISPLAY_NAME: u8 = 0x20;
    pub const UPDATE_LIST_ORDER: u8 = 0x40;
    pub const UPDATE_HAT: u8 = 0x80;
}

/// The key a player signs chat messages with.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatSession {
    pub session_id: Uuid,
    pub expires_at: i64,
    pub public_key: Vec<u8>,
    pub key_signature: Vec<u8>,
}

/// What a [`PlayerInfoUpdate`] says about one player. A field is present exactly when
/// the packet's actions include the matching bit of [`player_info`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlayerInfoEntry {
    pub uuid: Uuid,
    /// Name and profile properties, with `ADD_PLAYER`.
    pub profile: Option<(String, Vec<ProfileProperty>)>,
    /// With `INITIALIZE_CHAT`; the inner value is absent for a player without a key.
    pub chat_session: Option<Option<ChatSession>>,
    pub game_mode: Option<i32>,
    /// Whether the player appears in the player list.
    pub listed: Option<bool>,
    /// In milliseconds.
    pub latency: Option<i32>,
    /// With `UPDATE_DISPLAY_NAME`; the inner value is absent to show the plain name.
    pub display_name: Option<Option<Nbt>>,
    pub list_order: Option<i32>,
    pub show_hat: Option<bool>,
}

/// Adds players to the client's list of known players or updates them.
///
/// A client only shows a player entity whose player it already knows from this packet.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerInfoUpdate {
    /// A combination of [`player_info`] bits.
    pub actions: u8,
    pub entries: Vec<PlayerInfoEntry>,
}

impl Packet for PlayerInfoUpdate {
    const ID: i32 = clientbound::PLAYER_INFO_UPDATE;
}

impl Encode for PlayerInfoUpdate {
    fn encode(&self, w: &mut Writer) {
        // A missing field is written as its default, so that the packet stays well-formed.
        w.put_u8(self.actions);
        w.put_array(&self.entries, |w, entry| {
            w.put_uuid(entry.uuid);
            if self.actions & player_info::ADD_PLAYER != 0 {
                let (name, properties) = entry.profile.clone().unwrap_or_default();
                w.put_string(&name);
                w.put_array(&properties, |w, property| property.encode(w));
            }
            if self.actions & player_info::INITIALIZE_CHAT != 0 {
                let session = entry.chat_session.clone().flatten();
                w.put_option(session.as_ref(), |w, session| {
                    w.put_uuid(session.session_id);
                    w.put_i64(session.expires_at);
                    w.put_length(session.public_key.len());
                    w.put_bytes(&session.public_key);
                    w.put_length(session.key_signature.len());
                    w.put_bytes(&session.key_signature);
                });
            }
            if self.actions & player_info::UPDATE_GAME_MODE != 0 {
                w.put_var_int(entry.game_mode.unwrap_or_default());
            }
            if self.actions & player_info::UPDATE_LISTED != 0 {
                w.put_bool(entry.listed.unwrap_or_default());
            }
            if self.actions & player_info::UPDATE_LATENCY != 0 {
                w.put_var_int(entry.latency.unwrap_or_default());
            }
            if self.actions & player_info::UPDATE_DISPLAY_NAME != 0 {
                let name = entry.display_name.clone().flatten();
                w.put_option(name.as_ref(), |w, name| w.put_nbt(name));
            }
            if self.actions & player_info::UPDATE_LIST_ORDER != 0 {
                w.put_var_int(entry.list_order.unwrap_or_default());
            }
            if self.actions & player_info::UPDATE_HAT != 0 {
                w.put_bool(entry.show_hat.unwrap_or_default());
            }
        });
    }
}

impl Decode for PlayerInfoUpdate {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let actions = r.u8()?;
        let has = |action: u8| actions & action != 0;
        let entries = r.array(|r| {
            let mut entry = PlayerInfoEntry {
                uuid: r.uuid()?,
                ..PlayerInfoEntry::default()
            };
            if has(player_info::ADD_PLAYER) {
                let name = r.string(MAX_NAME_LENGTH)?;
                entry.profile = Some((name, r.array(ProfileProperty::decode)?));
            }
            if has(player_info::INITIALIZE_CHAT) {
                entry.chat_session = Some(r.option(|r| {
                    Ok(ChatSession {
                        session_id: r.uuid()?,
                        expires_at: r.i64()?,
                        public_key: {
                            let length = r.length()?;
                            r.bytes(length)?.to_vec()
                        },
                        key_signature: {
                            let length = r.length()?;
                            r.bytes(length)?.to_vec()
                        },
                    })
                })?);
            }
            if has(player_info::UPDATE_GAME_MODE) {
                entry.game_mode = Some(r.var_int()?);
            }
            if has(player_info::UPDATE_LISTED) {
                entry.listed = Some(r.bool()?);
            }
            if has(player_info::UPDATE_LATENCY) {
                entry.latency = Some(r.var_int()?);
            }
            if has(player_info::UPDATE_DISPLAY_NAME) {
                entry.display_name = Some(r.option(Reader::nbt)?.flatten());
            }
            if has(player_info::UPDATE_LIST_ORDER) {
                entry.list_order = Some(r.var_int()?);
            }
            if has(player_info::UPDATE_HAT) {
                entry.show_hat = Some(r.bool()?);
            }
            Ok(entry)
        })?;
        Ok(Self { actions, entries })
    }
}

/// Removes players from the client's list of known players.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerInfoRemove {
    pub players: Vec<Uuid>,
}

impl Packet for PlayerInfoRemove {
    const ID: i32 = clientbound::PLAYER_INFO_REMOVE;
}

impl Encode for PlayerInfoRemove {
    fn encode(&self, w: &mut Writer) {
        w.put_array(&self.players, |w, player| w.put_uuid(*player));
    }
}

impl Decode for PlayerInfoRemove {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            players: r.array(Reader::uuid)?,
        })
    }
}

/// Turns an angle in degrees into the 1/256 turns the protocol uses for entities.
pub fn angle(degrees: f32) -> u8 {
    (degrees * 256.0 / 360.0).rem_euclid(256.0) as u8
}

/// Makes an entity appear.
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnEntity {
    pub entity_id: i32,
    /// For a player, the UUID of their profile.
    pub uuid: Uuid,
    /// Id in the entity type registry.
    pub kind: i32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub velocity: [f64; 3],
    /// Angles in 1/256 turns; see [`angle`].
    pub pitch: u8,
    pub yaw: u8,
    pub head_yaw: u8,
    /// Meaning depends on the entity type; 0 for players.
    pub data: i32,
}

impl Packet for SpawnEntity {
    const ID: i32 = clientbound::ADD_ENTITY;
}

impl Encode for SpawnEntity {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.entity_id);
        w.put_uuid(self.uuid);
        w.put_var_int(self.kind);
        for value in [self.x, self.y, self.z] {
            w.put_f64(value);
        }
        w.put_velocity(self.velocity);
        w.put_u8(self.pitch);
        w.put_u8(self.yaw);
        w.put_u8(self.head_yaw);
        w.put_var_int(self.data);
    }
}

impl Decode for SpawnEntity {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            entity_id: r.var_int()?,
            uuid: r.uuid()?,
            kind: r.var_int()?,
            x: r.f64()?,
            y: r.f64()?,
            z: r.f64()?,
            velocity: r.velocity()?,
            pitch: r.u8()?,
            yaw: r.u8()?,
            head_yaw: r.u8()?,
            data: r.var_int()?,
        })
    }
}

/// The way an entity gets to the position of a [`SyncEntityPosition`].
#[derive(Debug, Clone, PartialEq)]
pub enum PositionPath {
    /// Straight to the position.
    Linear { x: f64, y: f64, z: f64 },
    /// Through intermediate positions, each with the number of ticks it takes.
    Stepped(Vec<(f64, f64, f64, i32)>),
}

impl PositionPath {
    /// Where the entity ends up.
    pub fn end(&self) -> Option<(f64, f64, f64)> {
        match self {
            Self::Linear { x, y, z } => Some((*x, *y, *z)),
            Self::Stepped(steps) => steps.last().map(|(x, y, z, _)| (*x, *y, *z)),
        }
    }
}

/// Puts an entity at an absolute position.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncEntityPosition {
    pub entity_id: i32,
    pub path: PositionPath,
    /// In degrees.
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
}

impl Packet for SyncEntityPosition {
    const ID: i32 = clientbound::ENTITY_POSITION_SYNC;
}

impl Encode for SyncEntityPosition {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.entity_id);
        match &self.path {
            PositionPath::Linear { x, y, z } => {
                w.put_var_int(0);
                for value in [x, y, z] {
                    w.put_f64(*value);
                }
            }
            PositionPath::Stepped(steps) => {
                w.put_var_int(1);
                w.put_array(steps, |w, (x, y, z, ticks)| {
                    for value in [x, y, z] {
                        w.put_f64(*value);
                    }
                    w.put_var_int(*ticks);
                });
            }
        }
        w.put_f32(self.yaw);
        w.put_f32(self.pitch);
        w.put_bool(self.on_ground);
    }
}

impl Decode for SyncEntityPosition {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            entity_id: r.var_int()?,
            path: match r.var_int()? {
                0 => PositionPath::Linear {
                    x: r.f64()?,
                    y: r.f64()?,
                    z: r.f64()?,
                },
                1 => PositionPath::Stepped(
                    r.array(|r| Ok((r.f64()?, r.f64()?, r.f64()?, r.var_int()?)))?,
                ),
                other => {
                    return Err(DecodeError::InvalidValue {
                        what: "position path type",
                        value: other.into(),
                    });
                }
            },
            yaw: r.f32()?,
            pitch: r.f32()?,
            on_ground: r.bool()?,
        })
    }
}

/// Turns an entity's head, which the position packets do not do.
#[derive(Debug, Clone, PartialEq)]
pub struct SetHeadRotation {
    pub entity_id: i32,
    /// In 1/256 turns; see [`angle`].
    pub head_yaw: u8,
}

impl Packet for SetHeadRotation {
    const ID: i32 = clientbound::ROTATE_HEAD;
}

impl Encode for SetHeadRotation {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.entity_id);
        w.put_u8(self.head_yaw);
    }
}

impl Decode for SetHeadRotation {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            entity_id: r.var_int()?,
            head_yaw: r.u8()?,
        })
    }
}

/// Makes entities disappear.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoveEntities {
    pub entity_ids: Vec<i32>,
}

impl Packet for RemoveEntities {
    const ID: i32 = clientbound::REMOVE_ENTITIES;
}

impl Encode for RemoveEntities {
    fn encode(&self, w: &mut Writer) {
        w.put_array(&self.entity_ids, |w, id| w.put_var_int(*id));
    }
}

impl Decode for RemoveEntities {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            entity_ids: r.array(Reader::var_int)?,
        })
    }
}

/// Values of [`PlayerAction::status`] in 26.3.
pub mod player_action {
    /// Started breaking a block. In creative mode this is the whole break.
    pub const START_DESTROY_BLOCK: i32 = 0;
    pub const CHANGE_DESTROY_DIRECTION: i32 = 1;
    pub const ABORT_DESTROY_BLOCK: i32 = 2;
    /// Finished breaking a block in survival mode.
    pub const STOP_DESTROY_BLOCK: i32 = 3;
    pub const DROP_ALL_ITEMS: i32 = 4;
    pub const DROP_ITEM: i32 = 5;
    pub const RELEASE_USE_ITEM: i32 = 6;
    pub const SWAP_ITEM_WITH_OFFHAND: i32 = 7;
    pub const STAB: i32 = 8;
}

/// Block faces as used by [`PlayerAction`] and block placement.
pub mod face {
    pub const BOTTOM: u8 = 0;
    pub const TOP: u8 = 1;
    pub const NORTH: u8 = 2;
    pub const SOUTH: u8 = 3;
    pub const WEST: u8 = 4;
    pub const EAST: u8 = 5;
}

/// Something the player does with their hands that is not placing a block: mostly
/// breaking blocks.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerAction {
    /// One of the [`player_action`] values.
    pub status: i32,
    /// The block concerned; zero for actions without one.
    pub position: Position,
    /// One of the [`face`] values: the side of the block that was hit.
    pub face: u8,
    /// Numbers the client's predicted block changes; see [`AcknowledgeBlockChange`].
    pub sequence: i32,
}

impl Packet for PlayerAction {
    const ID: i32 = serverbound::PLAYER_ACTION;
}

impl Encode for PlayerAction {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.status);
        w.put_position(self.position);
        w.put_u8(self.face);
        w.put_var_int(self.sequence);
    }
}

impl Decode for PlayerAction {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            status: r.var_int()?,
            position: r.position()?,
            face: r.u8()?,
            sequence: r.var_int()?,
        })
    }
}

/// Tells the client that every block change it predicted up to `sequence` has been
/// handled. The client then drops those predictions and shows what the server said.
#[derive(Debug, Clone, PartialEq)]
pub struct AcknowledgeBlockChange {
    pub sequence: i32,
}

impl Packet for AcknowledgeBlockChange {
    const ID: i32 = clientbound::BLOCK_CHANGED_ACK;
}

impl Encode for AcknowledgeBlockChange {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.sequence);
    }
}

impl Decode for AcknowledgeBlockChange {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            sequence: r.var_int()?,
        })
    }
}

/// A single block has changed.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockUpdate {
    pub position: Position,
    /// The new block state id.
    pub state: i32,
}

impl Packet for BlockUpdate {
    const ID: i32 = clientbound::BLOCK_UPDATE;
}

impl Encode for BlockUpdate {
    fn encode(&self, w: &mut Writer) {
        w.put_position(self.position);
        w.put_var_int(self.state);
    }
}

impl Decode for BlockUpdate {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            position: r.position()?,
            state: r.var_int()?,
        })
    }
}

/// The player used the item in their hand on a block, which for a block item means
/// placing it against that block.
#[derive(Debug, Clone, PartialEq)]
pub struct UseItemOn {
    /// 0 for the main hand, 1 for the off hand.
    pub hand: i32,
    /// The block that was clicked.
    pub position: Position,
    /// One of the [`face`] values: the side of the block that was clicked.
    pub face: i32,
    /// Where on the block the click landed, each from 0 to 1.
    pub cursor: [f32; 3],
    /// Whether the player's head is inside a block.
    pub inside_block: bool,
    pub world_border_hit: bool,
    /// Numbers the client's predicted block changes; see [`AcknowledgeBlockChange`].
    pub sequence: i32,
}

impl Packet for UseItemOn {
    const ID: i32 = serverbound::USE_ITEM_ON;
}

impl Encode for UseItemOn {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.hand);
        w.put_position(self.position);
        w.put_var_int(self.face);
        for value in self.cursor {
            w.put_f32(value);
        }
        w.put_bool(self.inside_block);
        w.put_bool(self.world_border_hit);
        w.put_var_int(self.sequence);
    }
}

impl Decode for UseItemOn {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            hand: r.var_int()?,
            position: r.position()?,
            face: r.var_int()?,
            cursor: [r.f32()?, r.f32()?, r.f32()?],
            inside_block: r.bool()?,
            world_border_hit: r.bool()?,
            sequence: r.var_int()?,
        })
    }
}

/// The player selected another slot of their hotbar, from 0 to 8.
#[derive(Debug, Clone, PartialEq)]
pub struct SetHeldItem {
    pub slot: i16,
}

impl Packet for SetHeldItem {
    const ID: i32 = serverbound::SET_CARRIED_ITEM;
}

impl Encode for SetHeldItem {
    fn encode(&self, w: &mut Writer) {
        w.put_i16(self.slot);
    }
}

impl Decode for SetHeldItem {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self { slot: r.i16()? })
    }
}

/// Slots of the player's inventory window.
pub mod inventory {
    /// The number of slots, including crafting, armour and the off hand.
    pub const SLOT_COUNT: usize = 46;
    /// The first of the nine hotbar slots.
    pub const HOTBAR_START: usize = 36;
    /// The id of the player's own inventory window.
    pub const PLAYER_WINDOW: i32 = 0;
}

/// A creative-mode player put an item into a slot of their inventory, or emptied it.
#[derive(Debug, Clone, PartialEq)]
pub struct SetCreativeModeSlot {
    /// A slot of the inventory window; see [`inventory`]. -1 drops the item instead.
    pub slot: i16,
    pub stack: Option<ItemStack>,
}

impl Packet for SetCreativeModeSlot {
    const ID: i32 = serverbound::SET_CREATIVE_MODE_SLOT;
}

impl Encode for SetCreativeModeSlot {
    fn encode(&self, w: &mut Writer) {
        w.put_i16(self.slot);
        w.put_item_stack(self.stack);
    }
}

impl Decode for SetCreativeModeSlot {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            slot: r.i16()?,
            stack: r.untrusted_item_stack()?,
        })
    }
}

/// The whole contents of a window, such as the player's inventory.
#[derive(Debug, Clone, PartialEq)]
pub struct SetContainerContent {
    /// [`inventory::PLAYER_WINDOW`] for the player's own inventory.
    pub window_id: i32,
    /// Counts the server's changes to the window, so that both sides can tell whether
    /// they agree on its contents.
    pub state_id: i32,
    pub slots: Vec<Option<ItemStack>>,
    /// The stack the player is dragging with the cursor.
    pub carried: Option<ItemStack>,
}

impl Packet for SetContainerContent {
    const ID: i32 = clientbound::CONTAINER_SET_CONTENT;
}

impl Encode for SetContainerContent {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.window_id);
        w.put_var_int(self.state_id);
        w.put_array(&self.slots, |w, stack| w.put_item_stack(*stack));
        w.put_item_stack(self.carried);
    }
}

impl Decode for SetContainerContent {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            window_id: r.var_int()?,
            state_id: r.var_int()?,
            slots: r.array(Reader::item_stack)?,
            carried: r.item_stack()?,
        })
    }
}

/// Tells the client which hotbar slot is selected, from 0 to 8.
#[derive(Debug, Clone, PartialEq)]
pub struct SetHeldSlot {
    pub slot: i32,
}

impl Packet for SetHeldSlot {
    const ID: i32 = clientbound::SET_HELD_SLOT;
}

impl Encode for SetHeldSlot {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.slot);
    }
}

impl Decode for SetHeldSlot {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self { slot: r.var_int()? })
    }
}

packet_set! {
    /// Packets a client can send in the play state.
    pub enum ServerboundPlay in crate::packet_ids::play::serverbound {
        ConfirmTeleportation,
        ServerboundKeepAlive,
        ClientTickEnd,
        ChunkBatchReceived,
        SetPlayerPosition,
        SetPlayerPositionAndRotation,
        SetPlayerRotation,
        SetPlayerMovementFlags,
        PlayerLoaded,
        PlayerAction,
        UseItemOn,
        SetHeldItem,
        SetCreativeModeSlot,
    }
}

packet_set! {
    /// Packets a server can send in the play state.
    pub enum ClientboundPlay in crate::packet_ids::play::clientbound {
        Login,
        PlayerAbilities,
        SynchronizePlayerPosition,
        GameEvent,
        SetCenterChunk,
        ClientboundKeepAlive,
        Disconnect,
        LevelChunkWithLight,
        ChunkBatchStart,
        ChunkBatchFinished,
        UnloadChunk,
        PlayerInfoUpdate,
        PlayerInfoRemove,
        SpawnEntity,
        SyncEntityPosition,
        SetHeadRotation,
        RemoveEntities,
        AcknowledgeBlockChange,
        BlockUpdate,
        SetContainerContent,
        SetHeldSlot,
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::packets::testing::assert_round_trip;

    fn login() -> Login {
        Login {
            entity_id: 42,
            hardcore: false,
            dimension_names: vec!["minecraft:overworld".to_owned()],
            max_players: 20,
            view_distance: 10,
            simulation_distance: 10,
            reduced_debug_info: false,
            enable_respawn_screen: true,
            limited_crafting: false,
            dimension_type: 0,
            dimension_name: "minecraft:overworld".to_owned(),
            hashed_seed: 0x1234_5678_9ABC_DEF0,
            game_mode: game_mode::CREATIVE,
            previous_game_mode: None,
            is_debug: false,
            is_flat: true,
            death_location: None,
            portal_cooldown: 0,
            sea_level: 63,
            online_mode: false,
            enforces_secure_chat: false,
        }
    }

    #[test]
    fn clientbound_packets_round_trip() {
        use ClientboundPlay as Set;
        assert_round_trip(login(), Set::decode, Set::Login);
        assert_round_trip(
            Login {
                previous_game_mode: Some(game_mode::SURVIVAL),
                death_location: Some(DeathLocation {
                    dimension: "minecraft:the_nether".to_owned(),
                    position: Position { x: -5, y: 70, z: 9 },
                }),
                ..login()
            },
            Set::decode,
            Set::Login,
        );
        assert_round_trip(
            PlayerAbilities {
                flags: 0x0F,
                flying_speed: 0.05,
                field_of_view_modifier: 0.1,
            },
            Set::decode,
            Set::PlayerAbilities,
        );
        assert_round_trip(
            SynchronizePlayerPosition {
                teleport_id: 1,
                x: 0.5,
                y: -60.0,
                z: 0.5,
                velocity_x: 0.0,
                velocity_y: 0.0,
                velocity_z: 0.0,
                yaw: 90.0,
                pitch: 0.0,
                relative_flags: 0,
            },
            Set::decode,
            Set::SynchronizePlayerPosition,
        );
        assert_round_trip(
            GameEvent {
                event: game_event::START_WAITING_FOR_LEVEL_CHUNKS,
                value: 0.0,
            },
            Set::decode,
            Set::GameEvent,
        );
        assert_round_trip(
            SetCenterChunk {
                chunk_x: -3,
                chunk_z: 7,
            },
            Set::decode,
            Set::SetCenterChunk,
        );
        assert_round_trip(
            ClientboundKeepAlive { id: 99 },
            Set::decode,
            Set::ClientboundKeepAlive,
        );
        assert_round_trip(
            Disconnect {
                reason: Nbt::String("bye".to_owned()),
            },
            Set::decode,
            Set::Disconnect,
        );
    }

    #[test]
    fn serverbound_packets_round_trip() {
        use ServerboundPlay as Set;
        assert_round_trip(
            ConfirmTeleportation {
                teleport_id: 1,
                x: 0.5,
                y: -60.0,
                z: 0.5,
                yaw: 90.0,
                pitch: 0.0,
            },
            Set::decode,
            Set::ConfirmTeleportation,
        );
        assert_round_trip(
            ServerboundKeepAlive { id: 99 },
            Set::decode,
            Set::ServerboundKeepAlive,
        );
        assert_round_trip(ClientTickEnd, Set::decode, Set::ClientTickEnd);
        assert_round_trip(PlayerLoaded, Set::decode, Set::PlayerLoaded);
    }

    #[test]
    fn the_serverbound_abilities_packet_is_its_id_and_one_byte_of_flags() {
        let (begin, stop) = (
            ServerboundPlayerAbilities::flying(true),
            ServerboundPlayerAbilities::flying(false),
        );
        assert!(begin.is_flying() && !stop.is_flying());
        assert_eq!(crate::packets::encode(&begin), [40, 0x02]);
        assert_eq!(crate::packets::encode(&stop), [40, 0x00]);

        // It is not in the set of serverbound packets yet, so it is read directly.
        for packet in [begin, stop, ServerboundPlayerAbilities { flags: 0xFF }] {
            let bytes = crate::packets::encode(&packet);
            let mut reader = Reader::new(&bytes);
            assert_eq!(reader.var_int(), Ok(ServerboundPlayerAbilities::ID));
            assert_eq!(ServerboundPlayerAbilities::decode(&mut reader), Ok(packet));
            assert_eq!(reader.finish(), Ok(()));
            assert_eq!(
                ServerboundPlay::decode(&bytes),
                Ok(ServerboundPlay::Unhandled { id: 40 })
            );
        }
    }

    #[test]
    fn the_clientbound_abilities_packet_says_whether_the_player_flies() {
        let with = |flags| PlayerAbilities {
            flags,
            flying_speed: 0.05,
            field_of_view_modifier: 0.1,
        };
        let creative = abilities::INVULNERABLE | abilities::MAY_FLY | abilities::INSTANT_BREAK;
        assert!(!with(creative).is_flying());
        assert!(with(creative | abilities::FLYING).is_flying());
        assert_eq!(
            crate::packets::encode(&with(abilities::FLYING))[..2],
            [65, 0x02]
        );
    }

    #[test]
    fn a_disconnect_carries_a_reason_in_either_form() {
        use ClientboundPlay as Set;
        for reason in [
            Text::literal("bye"),
            Text::translatable("multiplayer.disconnect.kicked"),
        ] {
            let packet = Disconnect::from(reason.clone());
            assert_eq!(packet.text(), reason);
            let bytes = crate::packets::encode(&packet);
            let Ok(Set::Disconnect(read)) = Set::decode(&bytes) else {
                panic!("not a disconnect: {bytes:?}");
            };
            assert_eq!(read.text(), reason);
            assert_eq!(read, packet);
        }
    }

    #[test]
    fn chunk_packets_round_trip() {
        use ClientboundPlay as Set;
        assert_round_trip(
            LevelChunkWithLight {
                chunk_x: -2,
                chunk_z: 5,
                heightmaps: vec![Heightmap {
                    kind: heightmap::MOTION_BLOCKING,
                    data: vec![1, 2, 3],
                }],
                sections: vec![0, 1, 2, 3, 4],
                block_entities: vec![
                    BlockEntity {
                        packed_xz: 0x4A,
                        y: -60,
                        kind: 7,
                        data: Some(Nbt::Compound(vec![("a".to_owned(), Nbt::Int(1))])),
                    },
                    BlockEntity {
                        packed_xz: 0,
                        y: 0,
                        kind: 1,
                        data: None,
                    },
                ],
                light: LightData {
                    sky_mask: vec![0b110],
                    block_mask: vec![],
                    empty_sky_mask: vec![0b001],
                    empty_block_mask: vec![0b111],
                    sky: vec![vec![0xFF; 2048], vec![0x0F; 2048]],
                    block: vec![],
                },
            },
            Set::decode,
            Set::LevelChunkWithLight,
        );
        assert_round_trip(ChunkBatchStart, Set::decode, Set::ChunkBatchStart);
        assert_round_trip(
            ChunkBatchFinished { batch_size: 9 },
            Set::decode,
            Set::ChunkBatchFinished,
        );
        assert_round_trip(
            ChunkBatchReceived {
                chunks_per_tick: 20.0,
            },
            ServerboundPlay::decode,
            ServerboundPlay::ChunkBatchReceived,
        );
    }

    #[test]
    fn movement_packets_round_trip() {
        use ServerboundPlay as Set;
        let flags = movement_flags::ON_GROUND | movement_flags::HORIZONTAL_COLLISION;
        assert_round_trip(
            SetPlayerPosition {
                x: 1.5,
                y: -60.0,
                z: -2.25,
                flags,
            },
            Set::decode,
            Set::SetPlayerPosition,
        );
        assert_round_trip(
            SetPlayerPositionAndRotation {
                x: 1.5,
                y: -60.0,
                z: -2.25,
                yaw: 180.0,
                pitch: -45.0,
                flags,
            },
            Set::decode,
            Set::SetPlayerPositionAndRotation,
        );
        assert_round_trip(
            SetPlayerRotation {
                yaw: 180.0,
                pitch: -45.0,
                flags: 0,
            },
            Set::decode,
            Set::SetPlayerRotation,
        );
        assert_round_trip(
            SetPlayerMovementFlags { flags },
            Set::decode,
            Set::SetPlayerMovementFlags,
        );
    }

    #[test]
    fn unload_chunk_puts_z_first() {
        let packet = UnloadChunk {
            chunk_x: 1,
            chunk_z: 2,
        };
        let bytes = crate::packets::encode(&packet);
        assert_eq!(bytes[1..], [0, 0, 0, 2, 0, 0, 0, 1]);
        assert_round_trip(
            packet,
            ClientboundPlay::decode,
            ClientboundPlay::UnloadChunk,
        );
    }

    #[test]
    fn entity_packets_round_trip() {
        use ClientboundPlay as Set;
        let uuid = Uuid::from_u128(0x1234_5678_9ABC_DEF0);
        assert_round_trip(
            PlayerInfoUpdate {
                actions: player_info::ADD_PLAYER
                    | player_info::UPDATE_GAME_MODE
                    | player_info::UPDATE_LISTED,
                entries: vec![PlayerInfoEntry {
                    uuid,
                    profile: Some(("Notch".to_owned(), Vec::new())),
                    game_mode: Some(game_mode::CREATIVE),
                    listed: Some(true),
                    ..PlayerInfoEntry::default()
                }],
            },
            Set::decode,
            Set::PlayerInfoUpdate,
        );
        // Every action at once, as the official server sends for a joining player.
        assert_round_trip(
            PlayerInfoUpdate {
                actions: 0xFF,
                entries: vec![PlayerInfoEntry {
                    uuid,
                    profile: Some((
                        "Notch".to_owned(),
                        vec![ProfileProperty {
                            name: "textures".to_owned(),
                            value: "abc".to_owned(),
                            signature: Some("def".to_owned()),
                        }],
                    )),
                    chat_session: Some(Some(ChatSession {
                        session_id: uuid,
                        expires_at: 99,
                        public_key: vec![1, 2, 3],
                        key_signature: vec![4, 5],
                    })),
                    game_mode: Some(game_mode::SURVIVAL),
                    listed: Some(false),
                    latency: Some(42),
                    display_name: Some(Some(Nbt::String("The Notch".to_owned()))),
                    list_order: Some(-1),
                    show_hat: Some(true),
                }],
            },
            Set::decode,
            Set::PlayerInfoUpdate,
        );
        assert_round_trip(
            PlayerInfoRemove {
                players: vec![uuid],
            },
            Set::decode,
            Set::PlayerInfoRemove,
        );
        assert_round_trip(
            SpawnEntity {
                entity_id: 300,
                uuid,
                kind: 159,
                x: 0.5,
                y: -60.0,
                z: -7.25,
                velocity: [0.0; 3],
                pitch: angle(-45.0),
                yaw: angle(90.0),
                head_yaw: angle(90.0),
                data: 0,
            },
            Set::decode,
            Set::SpawnEntity,
        );
        assert_round_trip(
            SyncEntityPosition {
                entity_id: 300,
                path: PositionPath::Linear {
                    x: 1.0,
                    y: 2.0,
                    z: 3.0,
                },
                yaw: 90.0,
                pitch: -45.0,
                on_ground: true,
            },
            Set::decode,
            Set::SyncEntityPosition,
        );
        assert_round_trip(
            SyncEntityPosition {
                entity_id: 300,
                path: PositionPath::Stepped(vec![(1.0, 2.0, 3.0, 1), (4.0, 5.0, 6.0, 2)]),
                yaw: 0.0,
                pitch: 0.0,
                on_ground: false,
            },
            Set::decode,
            Set::SyncEntityPosition,
        );
        assert_round_trip(
            SetHeadRotation {
                entity_id: 300,
                head_yaw: 64,
            },
            Set::decode,
            Set::SetHeadRotation,
        );
        assert_round_trip(
            RemoveEntities {
                entity_ids: vec![1, 300],
            },
            Set::decode,
            Set::RemoveEntities,
        );
    }

    #[test]
    fn block_packets_round_trip() {
        let position = Position {
            x: -12,
            y: -61,
            z: 300,
        };
        assert_round_trip(
            PlayerAction {
                status: player_action::START_DESTROY_BLOCK,
                position,
                face: face::TOP,
                sequence: 7,
            },
            ServerboundPlay::decode,
            ServerboundPlay::PlayerAction,
        );
        assert_round_trip(
            AcknowledgeBlockChange { sequence: 7 },
            ClientboundPlay::decode,
            ClientboundPlay::AcknowledgeBlockChange,
        );
        assert_round_trip(
            BlockUpdate { position, state: 0 },
            ClientboundPlay::decode,
            ClientboundPlay::BlockUpdate,
        );
    }

    #[test]
    fn item_packets_round_trip() {
        let stone = Some(ItemStack { item: 1, count: 1 });
        assert_round_trip(
            UseItemOn {
                hand: 0,
                position: Position {
                    x: 3,
                    y: -61,
                    z: -4,
                },
                face: i32::from(face::TOP),
                cursor: [0.5, 1.0, 0.25],
                inside_block: false,
                world_border_hit: false,
                sequence: 12,
            },
            ServerboundPlay::decode,
            ServerboundPlay::UseItemOn,
        );
        assert_round_trip(
            SetHeldItem { slot: 8 },
            ServerboundPlay::decode,
            ServerboundPlay::SetHeldItem,
        );
        for stack in [stone, None] {
            assert_round_trip(
                SetCreativeModeSlot { slot: 36, stack },
                ServerboundPlay::decode,
                ServerboundPlay::SetCreativeModeSlot,
            );
        }
        let mut slots = vec![None; inventory::SLOT_COUNT];
        slots[inventory::HOTBAR_START] = stone;
        assert_round_trip(
            SetContainerContent {
                window_id: inventory::PLAYER_WINDOW,
                state_id: 3,
                slots,
                carried: None,
            },
            ClientboundPlay::decode,
            ClientboundPlay::SetContainerContent,
        );
        assert_round_trip(
            SetHeldSlot { slot: 4 },
            ClientboundPlay::decode,
            ClientboundPlay::SetHeldSlot,
        );
    }

    #[test]
    fn angles_wrap_into_a_byte() {
        assert_eq!(angle(0.0), 0);
        assert_eq!(angle(90.0), 64);
        assert_eq!(angle(180.0), 128);
        assert_eq!(angle(-90.0), 192);
        assert_eq!(angle(360.0), 0);
        assert_eq!(angle(450.0), 64);
    }

    #[test]
    fn previous_game_mode_is_shifted_by_one() {
        let none = crate::packets::encode(&login());
        let survival = crate::packets::encode(&Login {
            previous_game_mode: Some(game_mode::SURVIVAL),
            ..login()
        });
        // The two encodings differ in exactly that one byte: 0 for none, 1 for survival.
        let differing: Vec<_> = none.iter().zip(&survival).filter(|(a, b)| a != b).collect();
        assert_eq!(differing, [(&0, &1)]);
    }

    proptest! {
        #[test]
        fn arbitrary_bytes_never_panic(bytes: Vec<u8>) {
            let _ = ServerboundPlay::decode(&bytes);
            let _ = ClientboundPlay::decode(&bytes);
        }
    }
}
