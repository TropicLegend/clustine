//! The play state.
//!
//! Only the packets Clustine uses are modelled; the rest decode as `Unhandled`.

use super::configuration::{
    ClientInformation, Disconnect as ConfigurationDisconnect, empty_packet, keep_alive,
};
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

/// The world's difficulty, and whether a client may change it (`ClientboundChangeDifficulty`:
/// the difficulty's id as a byte, then the lock).
#[derive(Debug, Clone, PartialEq)]
pub struct ChangeDifficulty {
    pub difficulty: u8,
    pub locked: bool,
}

impl Packet for ChangeDifficulty {
    const ID: i32 = clientbound::CHANGE_DIFFICULTY;
}

impl Encode for ChangeDifficulty {
    fn encode(&self, w: &mut Writer) {
        w.put_u8(self.difficulty);
        w.put_bool(self.locked);
    }
}

impl Decode for ChangeDifficulty {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            difficulty: r.u8()?,
            locked: r.bool()?,
        })
    }
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
    /// Marks components as relative to the current value ([`relative`]); 0 means all
    /// are absolute.
    pub relative_flags: i32,
}

/// The bits of a teleport's `relative_flags`, the game's `Relative` in the order of its
/// ordinals: each marks a value of the packet as added to the client's own.
pub mod relative {
    pub const X: i32 = 1 << 0;
    pub const Y: i32 = 1 << 1;
    pub const Z: i32 = 1 << 2;
    /// The yaw.
    pub const Y_ROT: i32 = 1 << 3;
    /// The pitch.
    pub const X_ROT: i32 = 1 << 4;
    pub const DELTA_X: i32 = 1 << 5;
    pub const DELTA_Y: i32 = 1 << 6;
    pub const DELTA_Z: i32 = 1 << 7;
    pub const ROTATE_DELTA: i32 = 1 << 8;
    /// `Relative.ROTATION`: the look left as it is.
    pub const ROTATION: i32 = Y_ROT | X_ROT;
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

/// Confirms a [`SynchronizePlayerPosition`] and reports where the client now is and
/// how it looks: the teleport's values with its relative ones added to what the client
/// had. The game takes them as a move of the player (`handleAcceptTeleportPacket` calls
/// `handlePlayerPositionChange` with them), of a rider the look alone.
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

/// The world's time (`ClientboundSetTimePacket`): the game time, and the clocks that
/// changed other than by their ticks, each by its id in the `minecraft:world_clock`
/// registry (`ByteBufCodecs.holderRegistry`, a VarInt) with its state
/// (`ClockNetworkState`: the total ticks a VarLong, the partial tick and the rate as
/// floats). The game sends the game time alone every 20 ticks, and the clocks whole at a
/// join and whenever one is moved (`ServerClockManager.modifyClock`).
#[derive(Debug, Clone, PartialEq)]
pub struct SetTime {
    pub game_time: i64,
    pub clocks: Vec<ClockState>,
}

/// One clock's state in a [`SetTime`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClockState {
    pub clock: i32,
    pub total_ticks: i64,
    pub partial_tick: f32,
    pub rate: f32,
}

impl Packet for SetTime {
    const ID: i32 = clientbound::SET_TIME;
}

impl Encode for SetTime {
    fn encode(&self, w: &mut Writer) {
        w.put_i64(self.game_time);
        w.put_var_int(self.clocks.len() as i32);
        for clock in &self.clocks {
            w.put_var_int(clock.clock);
            w.put_var_long(clock.total_ticks);
            w.put_f32(clock.partial_tick);
            w.put_f32(clock.rate);
        }
    }
}

impl Decode for SetTime {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let game_time = r.i64()?;
        let count = r.var_int()?;
        let count = usize::try_from(count).map_err(|_| DecodeError::InvalidValue {
            what: "count of clocks",
            value: i64::from(count),
        })?;
        let mut clocks = Vec::with_capacity(count.min(16));
        for _ in 0..count {
            clocks.push(ClockState {
                clock: r.var_int()?,
                total_ticks: r.var_long()?,
                partial_tick: r.f32()?,
                rate: r.f32()?,
            });
        }
        Ok(Self { game_time, clocks })
    }
}

/// Where the world's spawn is: the game's `RespawnData`, a dimension and a block with
/// the yaw and pitch a player is turned to there. A compass points to it.
#[derive(Debug, Clone, PartialEq)]
pub struct SetDefaultSpawnPosition {
    pub dimension: String,
    pub position: Position,
    pub yaw: f32,
    pub pitch: f32,
}

impl Packet for SetDefaultSpawnPosition {
    const ID: i32 = clientbound::SET_DEFAULT_SPAWN_POSITION;
}

impl Encode for SetDefaultSpawnPosition {
    fn encode(&self, w: &mut Writer) {
        w.put_string(&self.dimension);
        w.put_position(self.position);
        w.put_f32(self.yaw);
        w.put_f32(self.pitch);
    }
}

impl Decode for SetDefaultSpawnPosition {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            dimension: r.identifier()?,
            position: r.position()?,
            yaw: r.f32()?,
            pitch: r.f32()?,
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

/// The settings from the client's options screen, sent again in play whenever the
/// player changes one of them, such as the render distance.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerboundClientInformation {
    pub information: ClientInformation,
}

impl Packet for ServerboundClientInformation {
    const ID: i32 = serverbound::CLIENT_INFORMATION;
}

impl Encode for ServerboundClientInformation {
    fn encode(&self, w: &mut Writer) {
        self.information.encode_fields(w);
    }
}

impl Decode for ServerboundClientInformation {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            information: ClientInformation::decode_fields(r)?,
        })
    }
}

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

/// Light of a chunk column the client holds, without its blocks.
///
/// The chunk is named as two VarInts, where [`LevelChunkWithLight`] has two Ints; the
/// light follows as in that packet. The masks say which sections the packet speaks of:
/// one in a light mask comes with its array, one in an empty mask is dark. What a client
/// does with a section in neither mask is not said by the layout; see
/// `docs/adr/0023-light-at-the-edge.md`, section 8 (G2).
#[derive(Debug, Clone, PartialEq)]
pub struct LightUpdate {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub light: LightData,
}

impl Packet for LightUpdate {
    const ID: i32 = clientbound::LIGHT_UPDATE;
}

impl Encode for LightUpdate {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.chunk_x);
        w.put_var_int(self.chunk_z);
        self.light.encode(w);
    }
}

impl Decode for LightUpdate {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            chunk_x: r.var_int()?,
            chunk_z: r.var_int()?,
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

/// The player used the item in a hand without aiming at a block the item uses: a
/// bucket, a lily pad. The server finds what it hits by its own ray along `yaw` and
/// `pitch`, which become the player's rotation first.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerboundUseItem {
    /// 0 for the main hand, 1 for the off hand.
    pub hand: i32,
    /// Numbers the client's predicted block changes; see [`AcknowledgeBlockChange`].
    pub sequence: i32,
    pub yaw: f32,
    pub pitch: f32,
}

impl Packet for ServerboundUseItem {
    const ID: i32 = serverbound::USE_ITEM;
}

impl Encode for ServerboundUseItem {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.hand);
        w.put_var_int(self.sequence);
        w.put_f32(self.yaw);
        w.put_f32(self.pitch);
    }
}

impl Decode for ServerboundUseItem {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            hand: r.var_int()?,
            sequence: r.var_int()?,
            yaw: r.f32()?,
            pitch: r.f32()?,
        })
    }
}

/// The movement keys a player holds, sent when one of them changes. The server reads
/// the sneak key from it: a sneaking player who clicks a door with a block in hand
/// places the block instead of opening the door.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerboundPlayerInput {
    /// A combination of [`player_input`].
    pub flags: u8,
}

/// The keys in [`ServerboundPlayerInput::flags`].
pub mod player_input {
    pub const FORWARD: u8 = 0x01;
    pub const BACKWARD: u8 = 0x02;
    pub const LEFT: u8 = 0x04;
    pub const RIGHT: u8 = 0x08;
    pub const JUMP: u8 = 0x10;
    /// The sneak key.
    pub const SHIFT: u8 = 0x20;
    pub const SPRINT: u8 = 0x40;
}

impl Packet for ServerboundPlayerInput {
    const ID: i32 = serverbound::PLAYER_INPUT;
}

impl Encode for ServerboundPlayerInput {
    fn encode(&self, w: &mut Writer) {
        w.put_u8(self.flags);
    }
}

impl Decode for ServerboundPlayerInput {
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

/// An angle in degrees as the game packs it into a byte (`Mth.packDegrees`): the floor
/// of its 256ths of a turn, kept to the low eight bits. The movement packets compare
/// and send angles so.
pub fn pack_degrees(degrees: f32) -> i8 {
    // `Mth.floor` of a float, as an int, of which the byte keeps the low bits.
    (degrees * 256.0 / 360.0).floor() as i32 as i8
}

/// How far a relative move takes an entity (the game's `VecDelta`), in 4096ths of a
/// block from the base both ends keep (`crate::delta::VecDeltaCodec`).
#[derive(Debug, Clone, PartialEq)]
pub enum VecDelta {
    /// Straight there (`VecDelta.Linear`).
    Linear([i16; 3]),
    /// Through intermediate steps, each with the ticks it takes (`VecDelta.Stepped`).
    Stepped(Vec<(i32, [i16; 3])>),
}

impl VecDelta {
    /// The steps the move has: none for a straight one, which is how the packet's
    /// properties tell the two apart.
    fn step_count(&self) -> i32 {
        match self {
            Self::Linear(_) => 0,
            Self::Stepped(steps) => i32::try_from(steps.len()).expect("a move has few steps"),
        }
    }

    fn write(&self, w: &mut Writer) {
        match self {
            Self::Linear(delta) => {
                for axis in delta {
                    w.put_i16(*axis);
                }
            }
            Self::Stepped(steps) => {
                for (ticks, delta) in steps {
                    w.put_var_int(*ticks);
                    for axis in delta {
                        w.put_i16(*axis);
                    }
                }
            }
        }
    }

    /// Reads a delta of `steps` steps (`VecDelta.read`), refusing more steps than the
    /// bytes left could hold, as the game does.
    fn read(r: &mut Reader<'_>, steps: i32) -> Result<Self, DecodeError> {
        let shorts = |r: &mut Reader<'_>| Ok::<_, DecodeError>([r.i16()?, r.i16()?, r.i16()?]);
        if steps <= 0 {
            return Ok(Self::Linear(shorts(r)?));
        }
        let fits = r.remaining() / 7;
        let count = usize::try_from(steps).expect("positive");
        if count > fits {
            return Err(DecodeError::InvalidValue {
                what: "steps of a move",
                value: i64::from(steps),
            });
        }
        let mut read = Vec::with_capacity(count);
        for _ in 0..count {
            let ticks = r.var_int()?;
            read.push((ticks, shorts(r)?));
        }
        Ok(Self::Stepped(read))
    }

    /// Whether the move goes along x at all (`hasDeltaX`), in any step.
    pub fn has_x(&self) -> bool {
        match self {
            Self::Linear(delta) => delta[0] != 0,
            Self::Stepped(steps) => steps.iter().any(|(_, delta)| delta[0] != 0),
        }
    }

    /// Whether the move goes along z at all (`hasDeltaZ`), in any step.
    pub fn has_z(&self) -> bool {
        match self {
            Self::Linear(delta) => delta[2] != 0,
            Self::Stepped(steps) => steps.iter().any(|(_, delta)| delta[2] != 0),
        }
    }
}

/// The on-ground flag and the step count of a move in one VarInt
/// (`ClientboundMoveEntityPacket.packProperties`): the flag in the low bit.
fn move_properties(on_ground: bool, delta: &VecDelta) -> i32 {
    i32::from(on_ground) | delta.step_count() << 1
}

/// Reads [`move_properties`]: the flag and the delta that follows.
fn read_move(r: &mut Reader<'_>) -> Result<(bool, VecDelta), DecodeError> {
    let properties = r.var_int()?;
    let steps = ((properties as u32) >> 1) as i32;
    Ok((properties & 1 != 0, VecDelta::read(r, steps)?))
}

/// Moves an entity relative to where the client last put it
/// (`ClientboundMoveEntityPacket.Pos`).
#[derive(Debug, Clone, PartialEq)]
pub struct MoveEntityPos {
    pub entity_id: i32,
    pub delta: VecDelta,
    pub on_ground: bool,
}

impl Packet for MoveEntityPos {
    const ID: i32 = clientbound::MOVE_ENTITY_POS;
}

impl Encode for MoveEntityPos {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.entity_id);
        w.put_var_int(move_properties(self.on_ground, &self.delta));
        self.delta.write(w);
    }
}

impl Decode for MoveEntityPos {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let entity_id = r.var_int()?;
        let (on_ground, delta) = read_move(r)?;
        Ok(Self {
            entity_id,
            delta,
            on_ground,
        })
    }
}

/// Moves and turns an entity (`ClientboundMoveEntityPacket.PosRot`).
#[derive(Debug, Clone, PartialEq)]
pub struct MoveEntityPosRot {
    pub entity_id: i32,
    pub delta: VecDelta,
    /// Packed as [`pack_degrees`] does.
    pub yaw: i8,
    pub pitch: i8,
    pub on_ground: bool,
}

impl Packet for MoveEntityPosRot {
    const ID: i32 = clientbound::MOVE_ENTITY_POS_ROT;
}

impl Encode for MoveEntityPosRot {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.entity_id);
        w.put_var_int(move_properties(self.on_ground, &self.delta));
        self.delta.write(w);
        w.put_i8(self.yaw);
        w.put_i8(self.pitch);
    }
}

impl Decode for MoveEntityPosRot {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let entity_id = r.var_int()?;
        let (on_ground, delta) = read_move(r)?;
        Ok(Self {
            entity_id,
            delta,
            yaw: r.i8()?,
            pitch: r.i8()?,
            on_ground,
        })
    }
}

/// Turns an entity where it is (`ClientboundMoveEntityPacket.Rot`).
#[derive(Debug, Clone, PartialEq)]
pub struct MoveEntityRot {
    pub entity_id: i32,
    /// Packed as [`pack_degrees`] does.
    pub yaw: i8,
    pub pitch: i8,
    pub on_ground: bool,
}

impl Packet for MoveEntityRot {
    const ID: i32 = clientbound::MOVE_ENTITY_ROT;
}

impl Encode for MoveEntityRot {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.entity_id);
        w.put_bool(self.on_ground);
        w.put_i8(self.yaw);
        w.put_i8(self.pitch);
    }
}

impl Decode for MoveEntityRot {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let entity_id = r.var_int()?;
        let on_ground = r.bool()?;
        Ok(Self {
            entity_id,
            yaw: r.i8()?,
            pitch: r.i8()?,
            on_ground,
        })
    }
}

/// Sets an entity's motion (`ClientboundSetEntityMotionPacket`), in the compact form of
/// [`Writer::put_velocity`].
#[derive(Debug, Clone, PartialEq)]
pub struct SetEntityMotion {
    pub entity_id: i32,
    pub motion: [f64; 3],
}

impl Packet for SetEntityMotion {
    const ID: i32 = clientbound::SET_ENTITY_MOTION;
}

impl Encode for SetEntityMotion {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.entity_id);
        w.put_velocity(self.motion);
    }
}

impl Decode for SetEntityMotion {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            entity_id: r.var_int()?,
            motion: r.velocity()?,
        })
    }
}

/// Shows an item flying to whoever picked `count` of it up
/// (`ClientboundTakeItemEntityPacket`); the client plays the sound itself.
#[derive(Debug, Clone, PartialEq)]
pub struct TakeItemEntity {
    pub item_id: i32,
    pub collector_id: i32,
    pub count: i32,
}

impl Packet for TakeItemEntity {
    const ID: i32 = clientbound::TAKE_ITEM_ENTITY;
}

impl Encode for TakeItemEntity {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.item_id);
        w.put_var_int(self.collector_id);
        w.put_var_int(self.count);
    }
}

impl Decode for TakeItemEntity {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            item_id: r.var_int()?,
            collector_id: r.var_int()?,
            count: r.var_int()?,
        })
    }
}

/// Something that happened to an entity, which each client shows its own way
/// (`ClientboundEntityEventPacket`): its id as a 4-byte int, then the event's byte, as
/// `write` has them (`writeInt`, `writeByte`).
#[derive(Debug, Clone, PartialEq)]
pub struct EntityEvent {
    pub entity_id: i32,
    pub event: i8,
}

impl Packet for EntityEvent {
    const ID: i32 = clientbound::ENTITY_EVENT;
}

impl Encode for EntityEvent {
    fn encode(&self, w: &mut Writer) {
        w.put_i32(self.entity_id);
        w.put_i8(self.event);
    }
}

impl Decode for EntityEvent {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            entity_id: r.i32()?,
            event: r.i8()?,
        })
    }
}

/// What the game's entity events N5 sends mean (`Entity.handleEntityEvent` and its
/// kinds'): a boat's bubble column pushing it down or up, and an armour stand's wobble
/// at a first hit.
pub mod entity_event {
    pub const ARMOR_STAND_HIT: i8 = 32;
    pub const BUBBLE_COLUMN_DOWN: i8 = 71;
    pub const BUBBLE_COLUMN_UP: i8 = 72;
}

/// A block the client shows for a moment where a falling block landed, until the
/// change of the block itself reaches it (`ClientboundAddTransientBlockPacket`).
#[derive(Debug, Clone, PartialEq)]
pub struct AddTransientBlock {
    pub position: Position,
    /// Id in the block state registry.
    pub state: i32,
}

impl Packet for AddTransientBlock {
    const ID: i32 = clientbound::ADD_TRANSIENT_BLOCK;
}

impl Encode for AddTransientBlock {
    fn encode(&self, w: &mut Writer) {
        w.put_position(self.position);
        w.put_var_int(self.state);
    }
}

impl Decode for AddTransientBlock {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            position: r.position()?,
            state: r.var_int()?,
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

/// Several blocks of one section have changed in one tick
/// (`ClientboundSectionBlocksUpdatePacket`). Clustine sends a [`BlockUpdate`] for each
/// instead; the official server sends this, and the bots read it from there.
#[derive(Debug, Clone, PartialEq)]
pub struct SectionBlocksUpdate {
    /// The section, in sections: x, y and z.
    pub section: (i32, i32, i32),
    /// Each changed block: where in the section (x, y, z from 0 to 15) and its new
    /// state id.
    pub blocks: Vec<((u8, u8, u8), i32)>,
}

impl Packet for SectionBlocksUpdate {
    const ID: i32 = clientbound::SECTION_BLOCKS_UPDATE;
}

impl Encode for SectionBlocksUpdate {
    fn encode(&self, w: &mut Writer) {
        let (x, y, z) = self.section;
        // `SectionPos.asLong`.
        let packed = ((i64::from(x) & 0x3f_ffff) << 42)
            | (i64::from(y) & 0xf_ffff)
            | ((i64::from(z) & 0x3f_ffff) << 20);
        w.put_i64(packed);
        w.put_var_int(self.blocks.len() as i32);
        for ((bx, by, bz), state) in &self.blocks {
            // `SectionPos.sectionRelativePos`.
            let at = (i64::from(*bx) << 8) | (i64::from(*bz) << 4) | i64::from(*by);
            w.put_var_long((i64::from(*state) << 12) | at);
        }
    }
}

impl Decode for SectionBlocksUpdate {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let packed = r.i64()?;
        let section = (
            (packed >> 42) as i32,
            ((packed << 44) >> 44) as i32,
            ((packed << 22) >> 42) as i32,
        );
        let count = r.var_int()?;
        let count = usize::try_from(count).map_err(|_| DecodeError::NegativeLength(count))?;
        let mut blocks = Vec::with_capacity(count.min(4096));
        for _ in 0..count {
            let entry = r.var_long()?;
            let at = entry & 0xfff;
            let place = (
                ((at >> 8) & 15) as u8,
                (at & 15) as u8,
                ((at >> 4) & 15) as u8,
            );
            blocks.push((place, (entry >> 12) as i32));
        }
        Ok(Self { section, blocks })
    }
}

/// A block event (`ClientboundBlockEventPacket`): what a block's `triggerEvent` did, which
/// the client does again on its own world, a piston's move or a note block's note
/// (`docs/adr/0026-redstone.md`, sections 4 and 8). `block` is the block's id in the
/// block registry.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockEvent {
    pub position: Position,
    pub action: u8,
    pub param: u8,
    pub block: i32,
}

impl Packet for BlockEvent {
    const ID: i32 = clientbound::BLOCK_EVENT;
}

impl Encode for BlockEvent {
    fn encode(&self, w: &mut Writer) {
        w.put_position(self.position);
        w.put_u8(self.action);
        w.put_u8(self.param);
        w.put_var_int(self.block);
    }
}

impl Decode for BlockEvent {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            position: r.position()?,
            action: r.u8()?,
            param: r.u8()?,
            block: r.var_int()?,
        })
    }
}

/// A sound played at a place (`ClientboundSoundPacket`), named by its id in the sound
/// event registry, as the game names one it has registered. The place is in eighths
/// of a block, as the game sends it.
#[derive(Debug, Clone, PartialEq)]
pub struct Sound {
    pub sound: i32,
    /// The game's `SoundSource`, by its number: 4 for blocks.
    pub source: i32,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub volume: f32,
    pub pitch: f32,
    pub seed: i64,
}

/// The game's `SoundSource.BLOCKS`.
pub const SOUND_SOURCE_BLOCKS: i32 = 4;

impl Packet for Sound {
    const ID: i32 = clientbound::SOUND;
}

impl Encode for Sound {
    fn encode(&self, w: &mut Writer) {
        // A holder of the registry: its id plus one, 0 being a sound sent whole.
        w.put_var_int(self.sound + 1);
        w.put_var_int(self.source);
        w.put_i32(self.x);
        w.put_i32(self.y);
        w.put_i32(self.z);
        w.put_f32(self.volume);
        w.put_f32(self.pitch);
        w.put_i64(self.seed);
    }
}

impl Decode for Sound {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let holder = r.var_int()?;
        if holder == 0 {
            // The server sends none whole, and a test that reads one back needs none.
            return Err(DecodeError::InvalidValue {
                what: "registered sound",
                value: 0,
            });
        }
        Ok(Self {
            sound: holder - 1,
            source: r.var_int()?,
            x: r.i32()?,
            y: r.i32()?,
            z: r.i32()?,
            volume: r.f32()?,
            pitch: r.f32()?,
            seed: r.i64()?,
        })
    }
}

/// Something clients show at a place (`ClientboundLevelEventPacket`): a sound, bits,
/// smoke. `data` says more, as the event needs.
#[derive(Debug, Clone, PartialEq)]
pub struct LevelEvent {
    pub event: i32,
    pub position: Position,
    pub data: i32,
    /// Heard everywhere rather than near the place (the wither's spawn, the dragon's
    /// death).
    pub global: bool,
}

/// The events of [`LevelEvent`] the server sends.
pub mod level_event {
    /// A block broke: its sound and its bits, `data` being its state's id.
    pub const BLOCK_BROKEN: i32 = 2001;
    /// Lava met water or took a block's place: a hiss and smoke, with no data.
    pub const LAVA_FIZZ: i32 = 1501;
}

impl Packet for LevelEvent {
    const ID: i32 = clientbound::LEVEL_EVENT;
}

impl Encode for LevelEvent {
    fn encode(&self, w: &mut Writer) {
        w.put_i32(self.event);
        w.put_position(self.position);
        w.put_i32(self.data);
        w.put_bool(self.global);
    }
}

impl Decode for LevelEvent {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            event: r.i32()?,
            position: r.position()?,
            data: r.i32()?,
            global: r.bool()?,
        })
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
        w.put_item_stack_serverbound(self.stack.as_ref());
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
        w.put_array(&self.slots, |w, stack| w.put_item_stack(stack.as_ref()));
        w.put_item_stack(self.carried.as_ref());
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

/// Sets one slot of the player's inventory, numbered as the game's `Inventory` numbers
/// them (0 to 8 the hotbar, 9 to 35 the rest, 36 to 39 armour, 40 the off hand),
/// without a window or a state id: what the server sends of a slot it changed itself.
#[derive(Debug, Clone, PartialEq)]
pub struct SetPlayerInventory {
    pub slot: i32,
    pub stack: Option<ItemStack>,
}

impl Packet for SetPlayerInventory {
    const ID: i32 = clientbound::SET_PLAYER_INVENTORY;
}

impl Encode for SetPlayerInventory {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.slot);
        w.put_item_stack(self.stack.as_ref());
    }
}

impl Decode for SetPlayerInventory {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            slot: r.var_int()?,
            stack: r.item_stack()?,
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

/// The player hit an entity (`ServerboundAttackPacket`): a vehicle, an armour stand, a
/// frame or a painting to break, or anything else to hurt. Only the entity's network id.
#[derive(Debug, Clone, PartialEq)]
pub struct Attack {
    pub entity_id: i32,
}

impl Packet for Attack {
    const ID: i32 = serverbound::ATTACK;
}

impl Encode for Attack {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.entity_id);
    }
}

impl Decode for Attack {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            entity_id: r.var_int()?,
        })
    }
}

/// The player used a hand on an entity (`ServerboundInteractPacket`): to get into a
/// boat or a cart, to put an item into a frame, to take armour from a stand.
///
/// The hand is `InteractionHand.STREAM_CODEC`: `ByteBufCodecs.idMapper` over the hand's
/// id (MAIN_HAND 0, OFF_HAND 1) with `ByIdMap`'s `ZERO` strategy (InteractionHand's
/// static initialiser), so an id out of range is read as the main hand, here as there.
/// `idMapper`'s body (`ByteBufCodecs$31`) is not dumped; it writes the id as a VarInt.
#[derive(Debug, Clone, PartialEq)]
pub struct Interact {
    pub entity_id: i32,
    /// 0 for the main hand, 1 for the off hand.
    pub hand: i32,
    /// Where the player's aim met the entity's box, relative to the entity's position,
    /// in the compact form of [`Writer::put_velocity`] (`Vec3.LP_STREAM_CODEC`).
    pub location: [f64; 3],
    /// The sneak key was held.
    pub using_secondary_action: bool,
}

impl Packet for Interact {
    const ID: i32 = serverbound::INTERACT;
}

impl Encode for Interact {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.entity_id);
        w.put_var_int(self.hand);
        w.put_velocity(self.location);
        w.put_bool(self.using_secondary_action);
    }
}

impl Decode for Interact {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            entity_id: r.var_int()?,
            hand: read_hand(r)?,
            location: r.velocity()?,
            using_secondary_action: r.bool()?,
        })
    }
}

/// The highest hand id, `InteractionHand.OFF_HAND`'s.
const OFF_HAND: i32 = 1;

/// Reads an `InteractionHand` as its `STREAM_CODEC` does: an id out of range is the main
/// hand, by `ByIdMap`'s `ZERO` strategy, rather than an error.
fn read_hand(r: &mut Reader<'_>) -> Result<i32, DecodeError> {
    let id = r.var_int()?;
    Ok(if (0..=OFF_HAND).contains(&id) { id } else { 0 })
}

/// Where the player who steers a vehicle has moved it (`ServerboundMoveVehiclePacket`):
/// the client moves a boat it controls itself and tells the server so.
///
/// The place and the turn are `PositionAndRotation.STREAM_CODEC`: `Vec3.STREAM_CODEC`
/// (three doubles) and then two `FLOAT`s (PositionAndRotation's static initialiser), in
/// the order of the components of `PositionAndRotation$Immutable`: position, `yRot`,
/// `xRot`.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerboundMoveVehicle {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    /// `yRot`, in degrees.
    pub yaw: f32,
    /// `xRot`, in degrees.
    pub pitch: f32,
    pub on_ground: bool,
}

impl Packet for ServerboundMoveVehicle {
    const ID: i32 = serverbound::MOVE_VEHICLE;
}

impl Encode for ServerboundMoveVehicle {
    fn encode(&self, w: &mut Writer) {
        put_position_and_rotation(w, [self.x, self.y, self.z], self.yaw, self.pitch);
        w.put_bool(self.on_ground);
    }
}

impl Decode for ServerboundMoveVehicle {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let ([x, y, z], yaw, pitch) = read_position_and_rotation(r)?;
        Ok(Self {
            x,
            y,
            z,
            yaw,
            pitch,
            on_ground: r.bool()?,
        })
    }
}

/// Writes the game's `PositionAndRotation`: the position as three doubles, then `yRot`
/// and `xRot`. See [`ServerboundMoveVehicle`] for where the layout comes from.
fn put_position_and_rotation(w: &mut Writer, position: [f64; 3], yaw: f32, pitch: f32) {
    for value in position {
        w.put_f64(value);
    }
    w.put_f32(yaw);
    w.put_f32(pitch);
}

/// Reads what [`put_position_and_rotation`] writes.
fn read_position_and_rotation(r: &mut Reader<'_>) -> Result<([f64; 3], f32, f32), DecodeError> {
    Ok(([r.f64()?, r.f64()?, r.f64()?], r.f32()?, r.f32()?))
}

/// Which of a boat's paddles the player is turning (`ServerboundPaddleBoatPacket`); the
/// server shows the paddles to others through the boat's data.
#[derive(Debug, Clone, PartialEq)]
pub struct PaddleBoat {
    pub left: bool,
    pub right: bool,
}

impl Packet for PaddleBoat {
    const ID: i32 = serverbound::PADDLE_BOAT;
}

impl Encode for PaddleBoat {
    fn encode(&self, w: &mut Writer) {
        w.put_bool(self.left);
        w.put_bool(self.right);
    }
}

impl Decode for PaddleBoat {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            left: r.bool()?,
            right: r.bool()?,
        })
    }
}

/// The player swung their arm at nothing they hit (`ServerboundPunchPacket`): a click at
/// air, or at a block they begin to break. No fields: the game swings the main hand with
/// the held item's attack animation for everyone tracking the player
/// (`handlePunch`).
#[derive(Debug, Clone, PartialEq)]
pub struct Punch;

impl Packet for Punch {
    const ID: i32 = serverbound::PUNCH;
}

impl Encode for Punch {
    fn encode(&self, _: &mut Writer) {}
}

impl Decode for Punch {
    fn decode(_: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self)
    }
}

/// The player picked the item of an entity with the middle button
/// (`ServerboundPickItemFromEntityPacket`): a minecart, a boat, an armour stand, a
/// painting or what a frame holds.
///
/// Its codec is the entity's id as a `VAR_INT` and then `includeData` as a `BOOL`
/// (ServerboundPickItemFromEntityPacket's static initialiser).
#[derive(Debug, Clone, PartialEq)]
pub struct PickItemFromEntity {
    pub entity_id: i32,
    /// The control key was held: a creative player then gets the entity's data with it.
    pub include_data: bool,
}

impl Packet for PickItemFromEntity {
    const ID: i32 = serverbound::PICK_ITEM_FROM_ENTITY;
}

impl Encode for PickItemFromEntity {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.entity_id);
        w.put_bool(self.include_data);
    }
}

impl Decode for PickItemFromEntity {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            entity_id: r.var_int()?,
            include_data: r.bool()?,
        })
    }
}

/// The actions of [`PlayerCommand`]: the ordinals of the game's
/// `ServerboundPlayerCommandPacket$Action`, which `writeEnum` writes.
pub mod player_command {
    pub const STOP_SLEEPING: i32 = 0;
    pub const START_SPRINTING: i32 = 1;
    pub const STOP_SPRINTING: i32 = 2;
    /// The player began to charge a jump of what they ride; `data` is the charge.
    pub const START_RIDING_JUMP: i32 = 3;
    pub const STOP_RIDING_JUMP: i32 = 4;
    /// The player opened the inventory of what they ride, such as a chest boat.
    pub const OPEN_INVENTORY: i32 = 5;
    pub const START_FALL_FLYING: i32 = 6;
}

/// Something the player began or stopped that is not a move
/// (`ServerboundPlayerCommandPacket`): sprinting, a ridden jump, opening what they ride.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerCommand {
    /// The player's own entity id.
    pub entity_id: i32,
    /// One of the [`player_command`] values.
    pub action: i32,
    /// The charge of a [`player_command::START_RIDING_JUMP`], from 0 to 100; 0 for the
    /// others, as the game's two-argument constructor sends.
    pub data: i32,
}

impl Packet for PlayerCommand {
    const ID: i32 = serverbound::PLAYER_COMMAND;
}

impl Encode for PlayerCommand {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.entity_id);
        w.put_var_int(self.action);
        w.put_var_int(self.data);
    }
}

impl Decode for PlayerCommand {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let entity_id = r.var_int()?;
        let action = r.var_int()?;
        // `readEnum` indexes the enum's constants, so the game refuses any other
        // ordinal; so does this.
        if !(player_command::STOP_SLEEPING..=player_command::START_FALL_FLYING).contains(&action) {
            return Err(DecodeError::InvalidValue {
                what: "player command",
                value: i64::from(action),
            });
        }
        Ok(Self {
            entity_id,
            action,
            data: r.var_int()?,
        })
    }
}

/// Menus' kinds by their id in the `minecraft:menu` registry (`docs/groundwork/n6-brief.md`,
/// section 9.3).
pub mod menu {
    pub const GENERIC_9X3: i32 = 2;
    pub const GENERIC_9X6: i32 = 5;
    pub const GENERIC_3X3: i32 = 6;
    pub const CRAFTER_3X3: i32 = 7;
    pub const HOPPER: i32 = 16;
    pub const SHULKER_BOX: i32 = 20;
}

/// The kinds of click (`ContainerInput`, the game's `ClickType` before 26.3), as a click
/// carries them.
pub mod click {
    pub const PICKUP: i32 = 0;
    pub const QUICK_MOVE: i32 = 1;
    pub const SWAP: i32 = 2;
    pub const CLONE: i32 = 3;
    pub const THROW: i32 = 4;
    pub const QUICK_CRAFT: i32 = 5;
    pub const PICKUP_ALL: i32 = 6;
}

/// The most slots a click says the client changed (`ByteBufCodecs.map` with 128).
pub const MAX_CHANGED_SLOTS: usize = 128;
/// The most components a hashed stack names as added, or as removed.
pub const MAX_HASHED_COMPONENTS: usize = 256;

/// A stack as a client believes a slot holds it, with its components by hashes alone
/// (`HashedStack`): an item, a count, and per component added its type and the CRC32C of
/// its value, per component removed its type. What a client sends of an empty slot is
/// `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HashedStack {
    pub item: i32,
    pub count: i32,
    /// The components added, as (type, hash).
    pub added: Vec<(i32, i32)>,
    /// The types of the components removed.
    pub removed: Vec<i32>,
}

impl Writer {
    /// Writes a hashed stack, or an empty slot.
    pub fn put_hashed_stack(&mut self, stack: Option<&HashedStack>) {
        self.put_option(stack, |w, stack| {
            w.put_var_int(stack.item);
            w.put_var_int(stack.count);
            w.put_array(&stack.added, |w, (kind, hash)| {
                w.put_var_int(*kind);
                w.put_i32(*hash);
            });
            w.put_array(&stack.removed, |w, kind| w.put_var_int(*kind));
        });
    }
}

impl Reader<'_> {
    /// Reads a hashed stack, or an empty slot.
    pub fn hashed_stack(&mut self) -> Result<Option<HashedStack>, DecodeError> {
        self.option(|r| {
            let item = r.var_int()?;
            let count = r.var_int()?;
            let added = r.array_at_most(MAX_HASHED_COMPONENTS, "components added", |r| {
                Ok((r.var_int()?, r.i32()?))
            })?;
            let removed =
                r.array_at_most(MAX_HASHED_COMPONENTS, "components removed", Reader::var_int)?;
            Ok(HashedStack {
                item,
                count,
                added,
                removed,
            })
        })
    }

    /// Reads an element count of at most `max`, refused above it as the game's
    /// `readCount` refuses it, and that many elements.
    fn array_at_most<T>(
        &mut self,
        max: usize,
        what: &'static str,
        mut item: impl FnMut(&mut Self) -> Result<T, DecodeError>,
    ) -> Result<Vec<T>, DecodeError> {
        let count = self.length()?;
        if count > max {
            return Err(DecodeError::InvalidValue {
                what,
                value: i64::try_from(count).unwrap_or(i64::MAX),
            });
        }
        (0..count).map(|_| item(self)).collect()
    }
}

/// A click in a menu (`container_click`): the client's view, after its own prediction,
/// of every slot it changed and of the carried stack, which the server compares with
/// what it holds (`docs/groundwork/n6-brief.md`, sections 3.15 to 3.17).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerClick {
    pub window_id: i32,
    pub state_id: i32,
    /// The menu's slot; -999 outside the window, -1 its margin.
    pub slot: i16,
    pub button: i8,
    /// One of [`click`]; the game reads any other as a pickup.
    pub input: i32,
    pub changed: Vec<(i16, Option<HashedStack>)>,
    pub carried: Option<HashedStack>,
}

impl Packet for ContainerClick {
    const ID: i32 = serverbound::CONTAINER_CLICK;
}

impl Encode for ContainerClick {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.window_id);
        w.put_var_int(self.state_id);
        w.put_i16(self.slot);
        w.put_i8(self.button);
        w.put_var_int(self.input);
        w.put_array(&self.changed, |w, (slot, stack)| {
            w.put_i16(*slot);
            w.put_hashed_stack(stack.as_ref());
        });
        w.put_hashed_stack(self.carried.as_ref());
    }
}

impl Decode for ContainerClick {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            window_id: r.var_int()?,
            state_id: r.var_int()?,
            slot: r.i16()?,
            button: r.i8()?,
            input: r.var_int()?,
            changed: r.array_at_most(MAX_CHANGED_SLOTS, "slots changed", |r| {
                Ok((r.i16()?, r.hashed_stack()?))
            })?,
            carried: r.hashed_stack()?,
        })
    }
}

/// The client closed the window `window_id` (`container_close`, serverbound).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundContainerClose {
    pub window_id: i32,
}

impl Packet for ServerboundContainerClose {
    const ID: i32 = serverbound::CONTAINER_CLOSE;
}

impl Encode for ServerboundContainerClose {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.window_id);
    }
}

impl Decode for ServerboundContainerClose {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            window_id: r.var_int()?,
        })
    }
}

/// The server closed the window `window_id` (`container_close`, clientbound).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientboundContainerClose {
    pub window_id: i32,
}

impl Packet for ClientboundContainerClose {
    const ID: i32 = clientbound::CONTAINER_CLOSE;
}

impl Encode for ClientboundContainerClose {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.window_id);
    }
}

impl Decode for ClientboundContainerClose {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            window_id: r.var_int()?,
        })
    }
}

/// Opens a menu's window (`open_screen`): its id, its kind (one of [`menu`]) and its
/// title, a text component.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenScreen {
    pub window_id: i32,
    pub menu: i32,
    pub title: Nbt,
}

impl Packet for OpenScreen {
    const ID: i32 = clientbound::OPEN_SCREEN;
}

impl Encode for OpenScreen {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.window_id);
        w.put_var_int(self.menu);
        w.put_nbt(&self.title);
    }
}

impl Decode for OpenScreen {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            window_id: r.var_int()?,
            menu: r.var_int()?,
            title: r.nbt()?.ok_or(DecodeError::InvalidValue {
                what: "text component",
                value: 0,
            })?,
        })
    }
}

/// Opens a horse's inventory window (`mount_screen_open`,
/// `ClientboundMountScreenOpenPacket.write`): its window id, the columns of its chest
/// (none without one) and the horse's entity id, which the client shows the window of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MountScreenOpen {
    pub window_id: i32,
    pub columns: i32,
    pub entity_id: i32,
}

impl Packet for MountScreenOpen {
    const ID: i32 = clientbound::MOUNT_SCREEN_OPEN;
}

impl Encode for MountScreenOpen {
    fn encode(&self, w: &mut Writer) {
        // `writeContainerId`, a VarInt, then an `int`.
        w.put_var_int(self.window_id);
        w.put_var_int(self.columns);
        w.put_i32(self.entity_id);
    }
}

impl Decode for MountScreenOpen {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            window_id: r.var_int()?,
            columns: r.var_int()?,
            entity_id: r.i32()?,
        })
    }
}

/// Sets one slot of a window (`container_set_slot`).
#[derive(Debug, Clone, PartialEq)]
pub struct ContainerSetSlot {
    pub window_id: i32,
    pub state_id: i32,
    pub slot: i16,
    pub stack: Option<ItemStack>,
}

impl Packet for ContainerSetSlot {
    const ID: i32 = clientbound::CONTAINER_SET_SLOT;
}

impl Encode for ContainerSetSlot {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.window_id);
        w.put_var_int(self.state_id);
        w.put_i16(self.slot);
        w.put_item_stack(self.stack.as_ref());
    }
}

impl Decode for ContainerSetSlot {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            window_id: r.var_int()?,
            state_id: r.var_int()?,
            slot: r.i16()?,
            stack: r.item_stack()?,
        })
    }
}

/// Sets one data slot of a window (`container_set_data`), such as a crafter's slot
/// being disabled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerSetData {
    pub window_id: i32,
    pub id: i16,
    pub value: i16,
}

impl Packet for ContainerSetData {
    const ID: i32 = clientbound::CONTAINER_SET_DATA;
}

impl Encode for ContainerSetData {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.window_id);
        w.put_i16(self.id);
        w.put_i16(self.value);
    }
}

impl Decode for ContainerSetData {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            window_id: r.var_int()?,
            id: r.i16()?,
            value: r.i16()?,
        })
    }
}

/// Sets the stack a player carries on the cursor (`set_cursor_item`), with no window and
/// no state id.
#[derive(Debug, Clone, PartialEq)]
pub struct SetCursorItem {
    pub stack: Option<ItemStack>,
}

impl Packet for SetCursorItem {
    const ID: i32 = clientbound::SET_CURSOR_ITEM;
}

impl Encode for SetCursorItem {
    fn encode(&self, w: &mut Writer) {
        w.put_item_stack(self.stack.as_ref());
    }
}

impl Decode for SetCursorItem {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            stack: r.item_stack()?,
        })
    }
}

/// The particles an explosion shows, by their ids in the `minecraft:particle_type`
/// registry; none of them has options.
pub mod particle {
    pub const EXPLOSION_EMITTER: i32 = 29;
    pub const EXPLOSION: i32 = 30;
    pub const POOF: i32 = 69;
    pub const SMOKE: i32 = 72;
}

/// One of the particles an explosion scatters over the blocks it broke
/// (`ExplosionParticleInfo` with its weight in a `WeightedList`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExplosionParticle {
    /// One of [`particle`], a type without options.
    pub particle: i32,
    pub scaling: f32,
    pub speed: f32,
    pub weight: i32,
}

/// The block particles of every explosion the game makes (`ServerExplosion`'s default):
/// poof and smoke, equally likely.
pub const EXPLOSION_BLOCK_PARTICLES: [ExplosionParticle; 2] = [
    ExplosionParticle {
        particle: particle::POOF,
        scaling: 0.5,
        speed: 1.0,
        weight: 1,
    },
    ExplosionParticle {
        particle: particle::SMOKE,
        scaling: 1.0,
        speed: 1.0,
        weight: 1,
    },
];

/// An explosion (`explode`, `docs/groundwork/n6-brief.md`, section 9.1): where, how far,
/// how many positions its rays reached, the push to the receiving player, the particle
/// at the centre, the sound by its id, the particles over the blocks and whether the
/// sound plays. The layouts of the block particles' entries are inferred, and checked by
/// the comparison with the official server.
#[derive(Debug, Clone, PartialEq)]
pub struct Explode {
    pub centre: [f64; 3],
    pub radius: f32,
    pub block_count: i32,
    pub knockback: Option<[f64; 3]>,
    /// One of [`particle`]'s two explosions.
    pub particle: i32,
    /// An id of the `minecraft:sound_event` registry.
    pub sound: i32,
    pub block_particles: Vec<ExplosionParticle>,
    pub play_sound: bool,
}

impl Packet for Explode {
    const ID: i32 = clientbound::EXPLODE;
}

impl Encode for Explode {
    fn encode(&self, w: &mut Writer) {
        for coordinate in self.centre {
            w.put_f64(coordinate);
        }
        w.put_f32(self.radius);
        w.put_i32(self.block_count);
        w.put_option(self.knockback.as_ref(), |w, push| {
            for part in push {
                w.put_f64(*part);
            }
        });
        w.put_var_int(self.particle);
        // A holder of the registry: its id plus one, 0 being a sound sent whole.
        w.put_var_int(self.sound + 1);
        w.put_array(&self.block_particles, |w, entry| {
            w.put_var_int(entry.particle);
            w.put_f32(entry.scaling);
            w.put_f32(entry.speed);
            w.put_var_int(entry.weight);
        });
        w.put_bool(self.play_sound);
    }
}

impl Decode for Explode {
    /// Reads what [`Explode::encode`] writes: particles without options and a sound of
    /// the registry, as every explosion of the game's has them.
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let centre = [r.f64()?, r.f64()?, r.f64()?];
        let radius = r.f32()?;
        let block_count = r.i32()?;
        let knockback = r.option(|r| Ok([r.f64()?, r.f64()?, r.f64()?]))?;
        let particle = r.var_int()?;
        let sound = r.var_int()? - 1;
        if sound < 0 {
            return Err(DecodeError::InvalidValue {
                what: "sound of the registry",
                value: i64::from(sound),
            });
        }
        let block_particles = r.array(|r| {
            Ok(ExplosionParticle {
                particle: r.var_int()?,
                scaling: r.f32()?,
                speed: r.f32()?,
                weight: r.var_int()?,
            })
        })?;
        Ok(Self {
            centre,
            radius,
            block_count,
            knockback,
            particle,
            sound,
            block_particles,
            play_sound: r.bool()?,
        })
    }
}

/// The id of the `minecraft:vibration` particle type, and of the `minecraft:block`
/// position source a vibration travels to.
pub const PARTICLE_VIBRATION: i32 = 58;
pub const POSITION_SOURCE_BLOCK: i32 = 0;

/// A particle a `level_particles` packet shows, with its options. Only the vibration a
/// sculk sensor takes up is sent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Particle {
    /// `VibrationParticleOption`: travelling to the block `to` (a `BlockPositionSource`)
    /// and arriving in `arrival` ticks.
    Vibration { to: Position, arrival: i32 },
}

/// Particles shown at a point (`level_particles`, read from
/// `ClientboundLevelParticlesPacket`'s codec: the particle, two flags, the position, the
/// spread and the most speed along each axis, the count and how they are scattered).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelParticles {
    pub particle: Particle,
    pub override_limiter: bool,
    pub always_show: bool,
    pub position: [f64; 3],
    pub spread: [f32; 3],
    pub max_speed: [f32; 3],
    pub count: i32,
    /// `RandomizationType`, by its id: 0 `DEFAULT`.
    pub randomization: i32,
}

impl Packet for LevelParticles {
    const ID: i32 = clientbound::LEVEL_PARTICLES;
}

impl Encode for LevelParticles {
    fn encode(&self, w: &mut Writer) {
        match self.particle {
            Particle::Vibration { to, arrival } => {
                w.put_var_int(PARTICLE_VIBRATION);
                w.put_var_int(POSITION_SOURCE_BLOCK);
                w.put_position(to);
                w.put_var_int(arrival);
            }
        }
        w.put_bool(self.override_limiter);
        w.put_bool(self.always_show);
        for coordinate in self.position {
            w.put_f64(coordinate);
        }
        for part in self.spread.iter().chain(&self.max_speed) {
            w.put_f32(*part);
        }
        w.put_var_int(self.count);
        w.put_var_int(self.randomization);
    }
}

impl Decode for LevelParticles {
    /// Reads what [`LevelParticles::encode`] writes: a vibration to a block, which is the
    /// one particle a server sends here.
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let kind = r.var_int()?;
        if kind != PARTICLE_VIBRATION {
            return Err(DecodeError::InvalidValue {
                what: "vibration particle",
                value: i64::from(kind),
            });
        }
        let source = r.var_int()?;
        if source != POSITION_SOURCE_BLOCK {
            return Err(DecodeError::InvalidValue {
                what: "block position source",
                value: i64::from(source),
            });
        }
        let particle = Particle::Vibration {
            to: r.position()?,
            arrival: r.var_int()?,
        };
        Ok(Self {
            particle,
            override_limiter: r.bool()?,
            always_show: r.bool()?,
            position: [r.f64()?, r.f64()?, r.f64()?],
            spread: [r.f32()?, r.f32()?, r.f32()?],
            max_speed: [r.f32()?, r.f32()?, r.f32()?],
            count: r.var_int()?,
            randomization: r.var_int()?,
        })
    }
}

packet_set! {
    /// Packets a client can send in the play state.
    pub enum ServerboundPlay in crate::packet_ids::play::serverbound {
        ConfirmTeleportation,
        ServerboundKeepAlive,
        ClientTickEnd,
        ChunkBatchReceived,
        ServerboundClientInformation,
        SetPlayerPosition,
        SetPlayerPositionAndRotation,
        SetPlayerRotation,
        SetPlayerMovementFlags,
        ServerboundPlayerInput,
        PlayerLoaded,
        PlayerAction,
        UseItemOn,
        ServerboundUseItem,
        SetHeldItem,
        SetCreativeModeSlot,
        ServerboundPlayerAbilities,
        Attack,
        Interact,
        ServerboundMoveVehicle,
        PaddleBoat,
        PickItemFromEntity,
        Punch,
        PlayerCommand,
        ContainerClick,
        ServerboundContainerClose,
    }
}

/// A value of an entity's synchronised data (the game's `SynchedEntityData`), by the
/// serializer that writes it. Only those Clustine sends.
#[derive(Debug, Clone, PartialEq)]
pub enum EntityDataValue {
    /// `EntityDataSerializers.BYTE`: the shared flags of every entity, at index 0.
    Byte(u8),
    /// `EntityDataSerializers.INT`, a VarInt: a vehicle's hurt time, a frame's rotation.
    Int(i32),
    /// `EntityDataSerializers.FLOAT`: a vehicle's damage, a living entity's health.
    Float(f32),
    /// `EntityDataSerializers.LONG`: a wolf's end of anger in game time. The codec,
    /// `ByteBufCodecs.VAR_LONG`, is an anonymous class not dumped; a VarLong, as its
    /// name says (**Inferred**, `docs/groundwork/n7-brief.md`, section 9.5).
    Long(i64),
    /// `EntityDataSerializers.OPTIONAL_LIVING_ENTITY_REFERENCE`: a pet's owner, by UUID.
    /// `EntityReference.streamCodec()` (the UUID's) made optional: a presence flag, then
    /// the UUID as two longs, most significant first (**Inferred**, the brief's 9.5).
    OptionalLivingEntityReference(Option<Uuid>),
    /// `EntityDataSerializers.ITEM_STACK`: a slot, empty or not.
    ItemStack(Option<ItemStack>),
    /// `EntityDataSerializers.BOOLEAN`, one byte: a boat's paddles, a furnace cart's fuel.
    Boolean(bool),
    /// `EntityDataSerializers.ROTATIONS`: the angles of an armour stand's limb in
    /// degrees, x, y and z. `Rotations.STREAM_CODEC` is not dumped; three floats in that
    /// order are what the reference servers write.
    Rotations([f32; 3]),
    /// `EntityDataSerializers.BLOCK_POS`: a falling block's start, as a long.
    BlockPos(Position),
    /// `EntityDataSerializers.OPTIONAL_BLOCK_POS`: a sleeping position, `BlockPos`'s
    /// codec made optional, which is a presence flag and then the long.
    OptionalBlockPos(Option<Position>),
    /// `EntityDataSerializers.DIRECTION`: the face a hanging entity hangs on, as a
    /// VarInt of the direction's 3D value (down 0, up 1, north 2, south 3, west 4,
    /// east 5); see [`direction`].
    Direction(i32),
    /// `EntityDataSerializers.POSE`: an entity's pose, as a VarInt of its ordinal; see
    /// [`pose`].
    Pose(i32),
    /// `EntityDataSerializers.BLOCK_STATE`: the block primed TNT shows, as a VarInt of the
    /// state's id (`docs/groundwork/n6-brief.md`, section 9.1).
    BlockState(i32),
    /// `EntityDataSerializers.OPTIONAL_BLOCK_STATE`: a minecart's custom display block,
    /// as a VarInt of the state's id, with 0 for none. Air is state 0, so a cart told to
    /// show air shows none. The codec, `EntityDataSerializers$2`, is not dumped; this is
    /// how the reference servers write it.
    OptionalBlockState(i32),
    /// `EntityDataSerializers.PAINTING_VARIANT`: a painting's variant, by its id in the
    /// registry the client was sent. `PaintingVariant.STREAM_CODEC` is
    /// `ByteBufCodecs.holder` with the direct codec (PaintingVariant's static
    /// initialiser), which writes a registered value as its id plus one, 0 being a
    /// variant sent whole, which the server never sends. The body of `holder`
    /// (`ByteBufCodecs$33`) is not dumped; `Sound` writes its holder the same way.
    PaintingVariant(i32),
    /// `EntityDataSerializers.DYE_COLOR`: a cushion's colour, as a VarInt of the dye's
    /// id (white 0 to black 15).
    DyeColor(i32),
    /// One of the serializers of a farm animal's variant or sound variant
    /// (`COW_VARIANT`, `PIG_SOUND_VARIANT` and the like, of [`entity_data_serializer`]):
    /// `ByteBufCodecs.holderRegistry`, a VarInt of the entry's id in the registry the
    /// client was sent (no whole entry, so no one added). The serializers of an
    /// enumeration (`COPPER_GOLEM_STATE`, `WEATHERING_COPPER_STATE` and the like) write
    /// a VarInt of its ordinal, and are carried so too.
    Registered { serializer: i32, id: i32 },
    /// `EntityDataSerializers.VILLAGER_DATA`: a villager's or a zombie villager's type,
    /// profession and level. `VillagerData.STREAM_CODEC` (VillagerData.txt 265–275) is
    /// `StreamCodec.composite` of `ByteBufCodecs.holderRegistry` of the type's registry,
    /// the same of the profession's, and `ByteBufCodecs.VAR_INT`: three VarInts, the
    /// type's and the profession's ids in their built-in registries (no one added, as
    /// for a farm animal's variant), then the level.
    VillagerData {
        kind: i32,
        profession: i32,
        level: i32,
    },
}

/// The serializer ids of the game's `EntityDataSerializers`, in the order it registers
/// them (its static initialiser), which is not the order of their fields.
pub mod entity_data_serializer {
    pub const BYTE: i32 = 0;
    pub const INT: i32 = 1;
    pub const LONG: i32 = 2;
    pub const FLOAT: i32 = 3;
    pub const ITEM_STACK: i32 = 7;
    pub const BOOLEAN: i32 = 8;
    pub const ROTATIONS: i32 = 9;
    pub const BLOCK_POS: i32 = 10;
    pub const OPTIONAL_BLOCK_POS: i32 = 11;
    pub const DIRECTION: i32 = 12;
    pub const OPTIONAL_LIVING_ENTITY_REFERENCE: i32 = 13;
    pub const BLOCK_STATE: i32 = 14;
    pub const OPTIONAL_BLOCK_STATE: i32 = 15;
    /// The nineteenth registered, after `PARTICLES` (EntityDataSerializers.txt 317).
    pub const VILLAGER_DATA: i32 = 18;
    pub const POSE: i32 = 20;
    pub const CAT_VARIANT: i32 = 21;
    pub const CAT_SOUND_VARIANT: i32 = 22;
    pub const COW_VARIANT: i32 = 23;
    pub const COW_SOUND_VARIANT: i32 = 24;
    pub const WOLF_VARIANT: i32 = 25;
    pub const WOLF_SOUND_VARIANT: i32 = 26;
    pub const FROG_VARIANT: i32 = 27;
    pub const PIG_VARIANT: i32 = 28;
    pub const PIG_SOUND_VARIANT: i32 = 29;
    pub const CHICKEN_VARIANT: i32 = 30;
    pub const CHICKEN_SOUND_VARIANT: i32 = 31;
    pub const ZOMBIE_NAUTILUS_VARIANT: i32 = 32;
    pub const PAINTING_VARIANT: i32 = 34;
    pub const SNIFFER_STATE: i32 = 35;
    pub const ARMADILLO_STATE: i32 = 36;
    pub const COPPER_GOLEM_STATE: i32 = 37;
    pub const WEATHERING_COPPER_STATE: i32 = 38;
    pub const DYE_COLOR: i32 = 43;
}

/// The ordinals of the game's `Pose` that [`EntityDataValue::Pose`] carries
/// (`docs/groundwork/n7d-brief.md`, section 2.3).
pub mod pose {
    pub const STANDING: i32 = 0;
    pub const SLEEPING: i32 = 2;
    pub const CROUCHING: i32 = 5;
}

/// The 3D values of the game's `Direction`, which [`EntityDataValue::Direction`] and the
/// data of a hanging entity's spawn carry.
pub mod direction {
    pub const DOWN: i32 = 0;
    pub const UP: i32 = 1;
    pub const NORTH: i32 = 2;
    pub const SOUTH: i32 = 3;
    pub const WEST: i32 = 4;
    pub const EAST: i32 = 5;
}

/// The indices of entity data fields, as the game's classes define them one after the
/// other from `Entity`'s eight.
pub mod entity_data {
    /// `Entity.DATA_SHARED_FLAGS_ID`, a byte of flags: on fire in its lowest bit.
    pub const SHARED_FLAGS: u8 = 0;
    /// The shared flag of an entity on fire (`Entity.FLAG_ONFIRE`, bit 0).
    pub const ON_FIRE: u8 = 1;
    /// `ItemEntity.DATA_ITEM`, the stack an item entity is.
    pub const ITEM_STACK: u8 = 8;
    /// `FallingBlockEntity.DATA_START_POS`, where a falling block began to fall.
    pub const START_POSITION: u8 = 8;
    /// The shared flag of an invisible entity (`Entity.FLAG_INVISIBLE`, bit 5).
    pub const INVISIBLE: u8 = 0x20;
    /// `Entity.DATA_AIR_SUPPLY_ID`, the air left under water, an int.
    pub const AIR_SUPPLY: u8 = 1;
    /// `Entity.DATA_TICKS_FROZEN`, the ticks spent in powder snow, an int.
    pub const TICKS_FROZEN: u8 = 7;
    /// `Entity.DATA_POSE`, a [`super::pose`] ordinal.
    pub const POSE: u8 = 6;
    /// `LivingEntity.DATA_HEALTH_ID`, a float.
    pub const HEALTH: u8 = 9;
    /// `LivingEntity.SLEEPING_POS_ID`, an optional block position: the head of the bed a
    /// player sleeps in.
    pub const SLEEPING_POSITION: u8 = 14;
    /// `ArmorStand.DATA_CLIENT_FLAGS`: small, arms, no base plate, marker.
    pub const ARMOR_STAND_FLAGS: u8 = 15;
    /// `ArmorStand.DATA_HEAD_POSE`, the first of the six rotations of its parts (head,
    /// body, left arm, right arm, left leg, right leg), one index each.
    pub const ARMOR_STAND_POSES: u8 = 16;
}

/// The byte that ends a list of entity data values.
const ENTITY_DATA_END: u8 = 0xFF;

/// Sets fields of an entity's synchronised data (the game's
/// `ClientboundSetEntityDataPacket`): each an index, the serializer's id and the value,
/// the list ended by `0xFF`.
#[derive(Debug, Clone, PartialEq)]
pub struct SetEntityData {
    pub entity_id: i32,
    pub values: Vec<(u8, EntityDataValue)>,
}

impl Packet for SetEntityData {
    const ID: i32 = clientbound::SET_ENTITY_DATA;
}

impl Encode for SetEntityData {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.entity_id);
        for (index, value) in &self.values {
            w.put_u8(*index);
            match value {
                EntityDataValue::Byte(byte) => {
                    w.put_var_int(entity_data_serializer::BYTE);
                    w.put_u8(*byte);
                }
                EntityDataValue::ItemStack(stack) => {
                    w.put_var_int(entity_data_serializer::ITEM_STACK);
                    w.put_item_stack(stack.as_ref());
                }
                EntityDataValue::BlockPos(position) => {
                    w.put_var_int(entity_data_serializer::BLOCK_POS);
                    w.put_position(*position);
                }
                EntityDataValue::Int(value) => {
                    w.put_var_int(entity_data_serializer::INT);
                    w.put_var_int(*value);
                }
                EntityDataValue::Float(value) => {
                    w.put_var_int(entity_data_serializer::FLOAT);
                    w.put_f32(*value);
                }
                EntityDataValue::Long(value) => {
                    w.put_var_int(entity_data_serializer::LONG);
                    w.put_var_long(*value);
                }
                EntityDataValue::OptionalLivingEntityReference(uuid) => {
                    w.put_var_int(entity_data_serializer::OPTIONAL_LIVING_ENTITY_REFERENCE);
                    w.put_option(uuid.as_ref(), |w, uuid| w.put_uuid(*uuid));
                }
                EntityDataValue::Boolean(value) => {
                    w.put_var_int(entity_data_serializer::BOOLEAN);
                    w.put_bool(*value);
                }
                EntityDataValue::Rotations(angles) => {
                    w.put_var_int(entity_data_serializer::ROTATIONS);
                    for angle in angles {
                        w.put_f32(*angle);
                    }
                }
                EntityDataValue::OptionalBlockPos(position) => {
                    w.put_var_int(entity_data_serializer::OPTIONAL_BLOCK_POS);
                    w.put_option(position.as_ref(), |w, position| w.put_position(*position));
                }
                EntityDataValue::Direction(direction) => {
                    w.put_var_int(entity_data_serializer::DIRECTION);
                    w.put_var_int(*direction);
                }
                EntityDataValue::Pose(pose) => {
                    w.put_var_int(entity_data_serializer::POSE);
                    w.put_var_int(*pose);
                }
                EntityDataValue::BlockState(state) => {
                    w.put_var_int(entity_data_serializer::BLOCK_STATE);
                    w.put_var_int(*state);
                }
                EntityDataValue::OptionalBlockState(state) => {
                    w.put_var_int(entity_data_serializer::OPTIONAL_BLOCK_STATE);
                    w.put_var_int(*state);
                }
                EntityDataValue::PaintingVariant(variant) => {
                    w.put_var_int(entity_data_serializer::PAINTING_VARIANT);
                    w.put_var_int(*variant + 1);
                }
                EntityDataValue::DyeColor(colour) => {
                    w.put_var_int(entity_data_serializer::DYE_COLOR);
                    w.put_var_int(*colour);
                }
                EntityDataValue::Registered { serializer, id } => {
                    w.put_var_int(*serializer);
                    w.put_var_int(*id);
                }
                EntityDataValue::VillagerData {
                    kind,
                    profession,
                    level,
                } => {
                    w.put_var_int(entity_data_serializer::VILLAGER_DATA);
                    w.put_var_int(*kind);
                    w.put_var_int(*profession);
                    w.put_var_int(*level);
                }
            }
        }
        w.put_u8(ENTITY_DATA_END);
    }
}

impl Decode for SetEntityData {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let entity_id = r.var_int()?;
        let mut values = Vec::new();
        loop {
            let index = r.u8()?;
            if index == ENTITY_DATA_END {
                break;
            }
            let value = match r.var_int()? {
                entity_data_serializer::BYTE => EntityDataValue::Byte(r.u8()?),
                entity_data_serializer::ITEM_STACK => EntityDataValue::ItemStack(r.item_stack()?),
                entity_data_serializer::BLOCK_POS => EntityDataValue::BlockPos(r.position()?),
                entity_data_serializer::INT => EntityDataValue::Int(r.var_int()?),
                entity_data_serializer::FLOAT => EntityDataValue::Float(r.f32()?),
                entity_data_serializer::LONG => EntityDataValue::Long(r.var_long()?),
                entity_data_serializer::OPTIONAL_LIVING_ENTITY_REFERENCE => {
                    EntityDataValue::OptionalLivingEntityReference(r.option(Reader::uuid)?)
                }
                entity_data_serializer::BOOLEAN => EntityDataValue::Boolean(r.bool()?),
                entity_data_serializer::ROTATIONS => {
                    EntityDataValue::Rotations([r.f32()?, r.f32()?, r.f32()?])
                }
                entity_data_serializer::OPTIONAL_BLOCK_POS => {
                    EntityDataValue::OptionalBlockPos(r.option(Reader::position)?)
                }
                entity_data_serializer::DIRECTION => EntityDataValue::Direction(r.var_int()?),
                entity_data_serializer::POSE => EntityDataValue::Pose(r.var_int()?),
                entity_data_serializer::BLOCK_STATE => EntityDataValue::BlockState(r.var_int()?),
                entity_data_serializer::OPTIONAL_BLOCK_STATE => {
                    EntityDataValue::OptionalBlockState(r.var_int()?)
                }
                entity_data_serializer::PAINTING_VARIANT => match r.var_int()? {
                    // As for a sound: the server sends no variant whole.
                    0 => {
                        return Err(DecodeError::InvalidValue {
                            what: "registered painting variant",
                            value: 0,
                        });
                    }
                    holder => EntityDataValue::PaintingVariant(holder - 1),
                },
                entity_data_serializer::DYE_COLOR => EntityDataValue::DyeColor(r.var_int()?),
                entity_data_serializer::VILLAGER_DATA => EntityDataValue::VillagerData {
                    kind: r.var_int()?,
                    profession: r.var_int()?,
                    level: r.var_int()?,
                },
                serializer @ (entity_data_serializer::COW_VARIANT
                | entity_data_serializer::COW_SOUND_VARIANT
                | entity_data_serializer::CAT_VARIANT
                | entity_data_serializer::CAT_SOUND_VARIANT
                | entity_data_serializer::WOLF_VARIANT
                | entity_data_serializer::WOLF_SOUND_VARIANT
                | entity_data_serializer::PIG_VARIANT
                | entity_data_serializer::PIG_SOUND_VARIANT
                | entity_data_serializer::CHICKEN_VARIANT
                | entity_data_serializer::CHICKEN_SOUND_VARIANT
                | entity_data_serializer::ZOMBIE_NAUTILUS_VARIANT
                | entity_data_serializer::SNIFFER_STATE
                | entity_data_serializer::ARMADILLO_STATE
                | entity_data_serializer::COPPER_GOLEM_STATE
                | entity_data_serializer::WEATHERING_COPPER_STATE) => EntityDataValue::Registered {
                    serializer,
                    id: r.var_int()?,
                },
                other => {
                    return Err(DecodeError::InvalidValue {
                        what: "entity data serializer",
                        value: i64::from(other),
                    });
                }
            };
            values.push((index, value));
        }
        Ok(Self { entity_id, values })
    }
}

/// What a living entity wears and holds (`ClientboundSetEquipmentPacket`): each slot
/// named by the ordinal of the game's `EquipmentSlot` (0 the main hand, 1 the off
/// hand, 2 the feet, 3 the legs, 4 the chest, 5 the head), with its stack, empty to
/// clear it.
///
/// **Inferred**: the packet's class is not in the dump. It is written as the reference
/// servers write it, and as the game has written it since slots came in one packet: a
/// byte of the slot's ordinal, its high bit set where another slot follows, then the
/// stack as a slot (`ItemStack.OPTIONAL_STREAM_CODEC`).
#[derive(Debug, Clone, PartialEq)]
pub struct SetEquipment {
    pub entity_id: i32,
    pub slots: Vec<(u8, Option<ItemStack>)>,
}

impl Packet for SetEquipment {
    const ID: i32 = clientbound::SET_EQUIPMENT;
}

/// The bit of a slot's byte in [`SetEquipment`] that says another slot follows.
const MORE_EQUIPMENT: u8 = 0x80;

impl Encode for SetEquipment {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.entity_id);
        for (at, (slot, stack)) in self.slots.iter().enumerate() {
            let more = if at + 1 < self.slots.len() {
                MORE_EQUIPMENT
            } else {
                0
            };
            w.put_u8((slot & !MORE_EQUIPMENT) | more);
            w.put_item_stack(stack.as_ref());
        }
    }
}

impl Decode for SetEquipment {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let entity_id = r.var_int()?;
        let mut slots = Vec::new();
        loop {
            let byte = r.u8()?;
            slots.push((byte & !MORE_EQUIPMENT, r.item_stack()?));
            if byte & MORE_EQUIPMENT == 0 {
                break;
            }
        }
        Ok(Self { entity_id, slots })
    }
}

/// The actions of [`Animate`], as the game's `ClientboundAnimatePacket` numbers them in
/// 26.3. A swing of the arm is no longer one of them: it is [`SwingAnimation`].
pub mod animation {
    pub const WAKE_UP: u8 = 0;
    pub const CRITICAL_HIT: u8 = 1;
    pub const MAGIC_CRITICAL_HIT: u8 = 2;
}

/// Shows an entity doing something brief: a player waking, the sparks of a critical hit
/// (the game's `ClientboundAnimatePacket`).
#[derive(Debug, Clone, PartialEq)]
pub struct Animate {
    pub entity_id: i32,
    /// One of the [`animation`] values.
    pub action: u8,
}

impl Packet for Animate {
    const ID: i32 = clientbound::ANIMATE;
}

impl Encode for Animate {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.entity_id);
        w.put_u8(self.action);
    }
}

impl Decode for Animate {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            entity_id: r.var_int()?,
            action: r.u8()?,
        })
    }
}

/// Puts the vehicle the player steers where the server says it is
/// (`ClientboundMoveVehiclePacket`): the answer to a [`ServerboundMoveVehicle`] the
/// server would not take. The layout is that of [`ServerboundMoveVehicle`] without the
/// on-ground flag.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientboundMoveVehicle {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    /// `yRot`, in degrees.
    pub yaw: f32,
    /// `xRot`, in degrees.
    pub pitch: f32,
}

impl Packet for ClientboundMoveVehicle {
    const ID: i32 = clientbound::MOVE_VEHICLE;
}

impl Encode for ClientboundMoveVehicle {
    fn encode(&self, w: &mut Writer) {
        put_position_and_rotation(w, [self.x, self.y, self.z], self.yaw, self.pitch);
    }
}

impl Decode for ClientboundMoveVehicle {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let ([x, y, z], yaw, pitch) = read_position_and_rotation(r)?;
        Ok(Self {
            x,
            y,
            z,
            yaw,
            pitch,
        })
    }
}

/// Who rides an entity (`ClientboundSetPassengersPacket`): its direct passengers in the
/// order they got on, the first of them the one who steers. An empty list puts everyone
/// off.
///
/// The list is `writeVarIntArray`, of `FriendlyByteBuf`, which is not dumped: a VarInt
/// count and then each id as a VarInt, as `readVarIntArray` reads it back in the
/// packet's constructor. The comparisons with the official server check it.
#[derive(Debug, Clone, PartialEq)]
pub struct SetPassengers {
    pub vehicle_id: i32,
    pub passengers: Vec<i32>,
}

impl Packet for SetPassengers {
    const ID: i32 = clientbound::SET_PASSENGERS;
}

impl Encode for SetPassengers {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.vehicle_id);
        w.put_array(&self.passengers, |w, id| w.put_var_int(*id));
    }
}

impl Decode for SetPassengers {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            vehicle_id: r.var_int()?,
            passengers: r.array(Reader::var_int)?,
        })
    }
}

/// Ties a lead from one entity to another, or cuts it (`ClientboundSetEntityLinkPacket`).
/// Unlike most entity packets the ids are four-byte ints, not VarInts.
#[derive(Debug, Clone, PartialEq)]
pub struct SetEntityLink {
    /// The leashed entity.
    pub leashed_id: i32,
    /// What holds the lead: a player or a fence's knot; 0 when nothing does.
    pub holder_id: i32,
}

impl Packet for SetEntityLink {
    const ID: i32 = clientbound::SET_ENTITY_LINK;
}

impl Encode for SetEntityLink {
    fn encode(&self, w: &mut Writer) {
        w.put_i32(self.leashed_id);
        w.put_i32(self.holder_id);
    }
}

impl Decode for SetEntityLink {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            leashed_id: r.i32()?,
            holder_id: r.i32()?,
        })
    }
}

/// A sound that follows an entity (`ClientboundSoundEntityPacket`): a minecart rolling,
/// a frame's item turned. Named, like [`Sound`], by its id in the sound event registry.
///
/// ClientboundSoundEntityPacket's `write`: the sound by `SoundEvent.STREAM_CODEC` (the
/// holder, its id plus one), the source by `writeEnum` (a VarInt of its ordinal), the
/// entity by `writeVarInt`, then volume and pitch by `writeFloat` and the seed by
/// `writeLong`.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundEntity {
    pub sound: i32,
    /// The game's `SoundSource`, by its number.
    pub source: i32,
    pub entity_id: i32,
    pub volume: f32,
    pub pitch: f32,
    pub seed: i64,
}

impl Packet for SoundEntity {
    const ID: i32 = clientbound::SOUND_ENTITY;
}

impl Encode for SoundEntity {
    fn encode(&self, w: &mut Writer) {
        // A holder of the registry, as in `Sound`.
        w.put_var_int(self.sound + 1);
        w.put_var_int(self.source);
        w.put_var_int(self.entity_id);
        w.put_f32(self.volume);
        w.put_f32(self.pitch);
        w.put_i64(self.seed);
    }
}

impl Decode for SoundEntity {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let holder = r.var_int()?;
        if holder == 0 {
            // As for `Sound`: the server sends none whole.
            return Err(DecodeError::InvalidValue {
                what: "registered sound",
                value: 0,
            });
        }
        Ok(Self {
            sound: holder - 1,
            source: r.var_int()?,
            entity_id: r.var_int()?,
            volume: r.f32()?,
            pitch: r.f32()?,
            seed: r.i64()?,
        })
    }
}

/// The kinds of a swing, by the ids of the game's `SwingAnimationType` (its static
/// initialiser gives each the id of its ordinal).
pub mod swing_animation {
    pub const NONE: i32 = 0;
    /// The ordinary swing, the game's `SwingAnimation.DEFAULT` with a duration of 6.
    pub const WHACK: i32 = 1;
    /// The thrust of a spear.
    pub const STAB: i32 = 2;
    /// The duration of `SwingAnimation.DEFAULT`, in ticks.
    pub const DEFAULT_DURATION: i32 = 6;
}

/// Shows an entity swing a hand in a given way (`ClientboundSwingAnimationPacket`):
/// what a player's interaction with an entity shows to the others.
///
/// Its codec (ClientboundSwingAnimationPacket's static initialiser) is the entity as a
/// `VAR_INT`, the hand by `InteractionHand.STREAM_CODEC` (see [`Interact`]), then
/// `SwingAnimation.STREAM_CODEC`, the item component: the kind by
/// `SwingAnimationType.STREAM_CODEC`, an `idMapper` over its id with the `ZERO`
/// strategy, and the duration in ticks as a `VAR_INT`. So four VarInts, of which an
/// out-of-range hand or kind is read as id 0.
#[derive(Debug, Clone, PartialEq)]
pub struct SwingAnimation {
    pub entity_id: i32,
    /// 0 for the main hand, 1 for the off hand.
    pub hand: i32,
    /// One of the [`swing_animation`] kinds.
    pub kind: i32,
    pub duration: i32,
}

impl Packet for SwingAnimation {
    const ID: i32 = clientbound::SWING_ANIMATION;
}

impl Encode for SwingAnimation {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.entity_id);
        w.put_var_int(self.hand);
        w.put_var_int(self.kind);
        w.put_var_int(self.duration);
    }
}

impl Decode for SwingAnimation {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            entity_id: r.var_int()?,
            hand: read_hand(r)?,
            kind: match r.var_int()? {
                kind @ swing_animation::NONE..=swing_animation::STAB => kind,
                _ => swing_animation::NONE,
            },
            duration: r.var_int()?,
        })
    }
}

packet_set! {
    /// Packets a server can send in the play state.
    pub enum ClientboundPlay in crate::packet_ids::play::clientbound {
        Login,
        PlayerAbilities,
        SynchronizePlayerPosition,
        GameEvent,
        SetDefaultSpawnPosition,
        SetTime,
        SetCenterChunk,
        ClientboundKeepAlive,
        Disconnect,
        LevelChunkWithLight,
        LightUpdate,
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
        SectionBlocksUpdate,
        LevelEvent,
        BlockEvent,
        Sound,
        SetContainerContent,
        OpenScreen,
        MountScreenOpen,
        ContainerSetSlot,
        ContainerSetData,
        SetCursorItem,
        ClientboundContainerClose,
        Explode,
        LevelParticles,
        SetHeldSlot,
        SetPlayerInventory,
        SetEntityData,
        SetEquipment,
        Animate,
        MoveEntityPos,
        MoveEntityPosRot,
        MoveEntityRot,
        SetEntityMotion,
        TakeItemEntity,
        AddTransientBlock,
        EntityEvent,
        ClientboundMoveVehicle,
        SetPassengers,
        SetEntityLink,
        SoundEntity,
        SwingAnimation,
        DamageEvent,
        UpdateAttributes,
        SetExperience,
        SystemChat,
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::packets::testing::assert_round_trip;

    #[test]
    fn equipment_is_written_slot_by_slot_with_the_high_bit_on_all_but_the_last() {
        let worn = SetEquipment {
            entity_id: 3,
            slots: vec![(5, Some(ItemStack::new(7, 1))), (0, None)],
        };
        // The head with one of item 7 and no components changed, then an empty hand.
        assert_eq!(
            crate::packets::encode(&worn),
            [SetEquipment::ID as u8, 3, 0x85, 1, 7, 0, 0, 0x00, 0]
        );
        assert_eq!(SetEquipment::ID, 104);
        assert_round_trip(worn, ClientboundPlay::decode, ClientboundPlay::SetEquipment);
    }

    #[test]
    fn a_damage_event_and_attributes_are_written_as_the_game_writes_them() {
        let hurt = DamageEvent {
            entity_id: 9,
            damage_type: 3,
            cause: Some(0),
            direct: None,
            position: Some([1.5, -2.0, 0.25]),
        };
        let bytes = crate::packets::encode(&hurt);
        // The id, the entity, the type, the cause plus one, no direct entity, a position.
        assert_eq!(&bytes[..6], [DamageEvent::ID as u8, 9, 3, 1, 0, 1]);
        assert_eq!(DamageEvent::ID, 25);
        assert_round_trip(hurt, ClientboundPlay::decode, ClientboundPlay::DamageEvent);

        let attributes = UpdateAttributes {
            entity_id: 9,
            attributes: vec![AttributeSnapshot {
                attribute: 13,
                base: 16.0,
                modifiers: vec![("minecraft:random_spawn_bonus".to_owned(), 0.05, 1)],
            }],
        };
        assert_eq!(UpdateAttributes::ID, 134);
        assert_round_trip(
            attributes,
            ClientboundPlay::decode,
            ClientboundPlay::UpdateAttributes,
        );
    }

    #[test]
    fn a_section_blocks_update_round_trips_far_out_and_below_zero() {
        assert_round_trip(
            SectionBlocksUpdate {
                section: (-1_875_000, -4, 1_874_999),
                blocks: vec![((0, 0, 0), 1), ((15, 7, 3), 27_000), ((1, 15, 15), 0)],
            },
            ClientboundPlay::decode,
            ClientboundPlay::SectionBlocksUpdate,
        );
    }

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
    fn an_item_entity_s_spawn_and_its_stack_round_trip() {
        use ClientboundPlay as Set;
        // With a velocity at the protocol's steps, which reads back as it was written.
        assert_round_trip(
            SpawnEntity {
                entity_id: 7,
                uuid: Uuid::from_u128(0x1234),
                kind: 72,
                x: 0.5,
                y: -58.680_000_007_152_56,
                z: 0.5,
                velocity: [1.0, 0.0, -1.0],
                pitch: 0,
                yaw: angle(200.0),
                head_yaw: 0,
                data: 0,
            },
            Set::decode,
            Set::SpawnEntity,
        );
        let stack = ItemStack::new(1, 64);
        assert_round_trip(
            SetEntityData {
                entity_id: 7,
                values: vec![(
                    entity_data::ITEM_STACK,
                    EntityDataValue::ItemStack(Some(stack)),
                )],
            },
            Set::decode,
            Set::SetEntityData,
        );
        assert_round_trip(
            Animate {
                entity_id: 3,
                action: animation::CRITICAL_HIT,
            },
            Set::decode,
            Set::Animate,
        );
    }

    /// The bytes of the item's data as the game writes them: the entity's id, the index
    /// of `ItemEntity.DATA_ITEM` (8), the id of the serializer `ITEM_STACK` (7), the
    /// stack as count, item and two empty lists of components, and the end of the list.
    #[test]
    fn an_item_s_stack_is_written_as_the_game_writes_its_entity_data() {
        let packet = SetEntityData {
            entity_id: 300,
            values: vec![(
                entity_data::ITEM_STACK,
                EntityDataValue::ItemStack(Some(ItemStack::new(5, 2))),
            )],
        };
        let bytes = crate::packets::encode(&packet);
        let id = u8::try_from(clientbound::SET_ENTITY_DATA).unwrap();
        assert_eq!(bytes, [id, 0xAC, 0x02, 8, 7, 2, 5, 0, 0, 0xFF]);
        // A list of none is the end alone; and a serializer not known is refused.
        let none = SetEntityData {
            entity_id: 1,
            values: Vec::new(),
        };
        assert_eq!(crate::packets::encode(&none), [id, 1, 0xFF]);
        // `STRING`, 4, is not one Clustine sends.
        assert!(ClientboundPlay::decode(&[id, 1, 8, 4, 0, 0xFF]).is_err());
    }

    #[test]
    fn clientbound_packets_round_trip() {
        use ClientboundPlay as Set;
        assert_round_trip(login(), Set::decode, Set::Login);
        assert_round_trip(
            LevelEvent {
                event: level_event::BLOCK_BROKEN,
                position: Position {
                    x: -3,
                    y: 70,
                    z: 12,
                },
                data: 1,
                global: false,
            },
            Set::decode,
            Set::LevelEvent,
        );
        assert_round_trip(
            BlockEvent {
                position: Position {
                    x: -30_000_000,
                    y: -64,
                    z: 29_999_999,
                },
                action: 1,
                param: 5,
                block: 1200,
            },
            Set::decode,
            Set::BlockEvent,
        );
        assert_round_trip(
            Sound {
                sound: 1850,
                source: SOUND_SOURCE_BLOCKS,
                x: -20,
                y: 524,
                z: 100,
                volume: 1.0,
                pitch: 0.93,
                seed: -7,
            },
            Set::decode,
            Set::Sound,
        );
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
            SetDefaultSpawnPosition {
                dimension: "minecraft:overworld".to_owned(),
                position: Position {
                    x: -96,
                    y: 66,
                    z: 32,
                },
                yaw: 0.0,
                pitch: 0.0,
            },
            Set::decode,
            Set::SetDefaultSpawnPosition,
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
    fn the_packets_of_menus_round_trip() {
        use ClientboundPlay as Client;
        use ServerboundPlay as Server;
        let named = HashedStack {
            item: 7,
            count: 3,
            added: vec![(5, -0x1234_5678)],
            removed: vec![9],
        };
        assert_round_trip(
            ContainerClick {
                window_id: 100,
                state_id: 4,
                slot: -999,
                button: 1,
                input: click::QUICK_CRAFT,
                changed: vec![(0, None), (53, Some(named.clone()))],
                carried: Some(named),
            },
            Server::decode,
            Server::ContainerClick,
        );
        assert_round_trip(
            ServerboundContainerClose { window_id: 3 },
            Server::decode,
            Server::ServerboundContainerClose,
        );
        assert_round_trip(
            OpenScreen {
                window_id: 1,
                menu: menu::GENERIC_9X6,
                title: Text::translatable("container.chestDouble").to_nbt(),
            },
            Client::decode,
            Client::OpenScreen,
        );
        assert_round_trip(
            MountScreenOpen {
                window_id: 2,
                columns: 5,
                entity_id: 70_000,
            },
            Client::decode,
            Client::MountScreenOpen,
        );
        assert_round_trip(
            ContainerSetSlot {
                window_id: 1,
                state_id: 9,
                slot: 26,
                stack: Some(ItemStack::new(1, 64)),
            },
            Client::decode,
            Client::ContainerSetSlot,
        );
        assert_round_trip(
            ContainerSetData {
                window_id: 1,
                id: 2,
                value: 1,
            },
            Client::decode,
            Client::ContainerSetData,
        );
        assert_round_trip(
            SetCursorItem { stack: None },
            Client::decode,
            Client::SetCursorItem,
        );
        assert_round_trip(
            ClientboundContainerClose { window_id: 100 },
            Client::decode,
            Client::ClientboundContainerClose,
        );
    }

    #[test]
    fn a_vibrations_particle_round_trips() {
        assert_round_trip(
            LevelParticles {
                particle: Particle::Vibration {
                    to: Position {
                        x: -3,
                        y: -60,
                        z: 17,
                    },
                    arrival: 6,
                },
                override_limiter: false,
                always_show: false,
                position: [0.5, -59.0, 12.25],
                spread: [0.0; 3],
                max_speed: [0.0; 3],
                count: 1,
                randomization: 0,
            },
            ClientboundPlay::decode,
            ClientboundPlay::LevelParticles,
        );
    }

    #[test]
    fn an_explosion_round_trips_with_a_push_and_without() {
        for knockback in [None, Some([0.5, 0.25, -1.0])] {
            assert_round_trip(
                Explode {
                    centre: [0.5, -59.0, 16.5],
                    radius: 4.0,
                    block_count: 1061,
                    knockback,
                    particle: particle::EXPLOSION_EMITTER,
                    sound: 703,
                    block_particles: EXPLOSION_BLOCK_PARTICLES.to_vec(),
                    play_sound: true,
                },
                ClientboundPlay::decode,
                ClientboundPlay::Explode,
            );
        }
    }

    #[test]
    fn a_click_naming_more_than_128_slots_is_refused() {
        let click = |slots: i16| ContainerClick {
            window_id: 1,
            state_id: 0,
            slot: 0,
            button: 0,
            input: click::PICKUP,
            changed: (0..slots).map(|slot| (slot, None)).collect(),
            carried: None,
        };
        let read = |slots| ServerboundPlay::decode(&crate::packets::encode(&click(slots)));
        assert_eq!(read(128), Ok(ServerboundPlay::ContainerClick(click(128))));
        assert!(read(129).is_err());
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
        assert_round_trip(
            ServerboundClientInformation {
                information: ClientInformation {
                    locale: "en_gb".to_owned(),
                    view_distance: 5,
                    chat_mode: 0,
                    chat_colors: true,
                    displayed_skin_parts: 0x7F,
                    main_hand: 1,
                    text_filtering: false,
                    allow_server_listing: true,
                    particle_status: 0,
                },
            },
            Set::decode,
            Set::ServerboundClientInformation,
        );
    }

    #[test]
    fn a_client_information_packet_in_play_is_decoded_by_its_id_from_bytes_a_client_sends() {
        // Written out by hand from the protocol, so that the id and the order of the
        // fields are checked against something other than the encoder: id 14, then
        // the locale, the render distance as one signed byte, the chat mode, chat
        // colours, the skin parts, the main hand, text filtering, listing and
        // particles.
        let mut bytes = vec![14, 5];
        bytes.extend(b"en_gb");
        bytes.extend([12, 0, 1, 0x7F, 1, 0, 1, 2]);
        let decoded = ServerboundPlay::decode(&bytes).expect("the packet decodes");
        let ServerboundPlay::ServerboundClientInformation(packet) = decoded else {
            panic!("decoded as {decoded:?}");
        };
        let information = packet.information;
        assert_eq!(information.locale, "en_gb");
        assert_eq!(information.view_distance, 12);
        assert_eq!(information.main_hand, 1);
        assert!(information.allow_server_listing);
        assert_eq!(information.particle_status, 2);
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

        // Whatever else a client sets in the byte comes back as it was sent.
        for packet in [begin, stop, ServerboundPlayerAbilities { flags: 0xFF }] {
            let bytes = crate::packets::encode(&packet);
            assert_eq!(
                ServerboundPlay::decode(&bytes),
                Ok(ServerboundPlay::ServerboundPlayerAbilities(packet))
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

    /// The bytes are written out by hand from `docs/protocol-26.3.md`: the id, the chunk
    /// as two VarInts, four bit sets (each a count of bytes and the bytes, lowest first,
    /// without zero bytes at the end), then the sky arrays and the block arrays, each a
    /// count and each array with its length of 2,048 before it.
    #[test]
    fn a_light_update_is_two_var_ints_and_the_light_of_a_chunk_packet() {
        let packet = LightUpdate {
            chunk_x: 2,
            chunk_z: -3,
            light: LightData {
                // Sections 1 and 9 of the 26 have sky light, the one below the world is
                // dark; section 2 has block light and none is dark.
                sky_mask: vec![0x202],
                block_mask: vec![0b100],
                empty_sky_mask: vec![0b1],
                empty_block_mask: vec![],
                sky: vec![vec![0xFF; 2048], vec![0xFE; 2048]],
                block: vec![vec![0x0E; 2048]],
            },
        };
        let mut expected = vec![
            // Packet 49; x 2; z -3, which as a VarInt is the five bytes of 0xFFFFFFFD.
            0x31, 0x02, 0xFD, 0xFF, 0xFF, 0xFF, 0x0F,
            // Sky: two bytes, bits 1 and 9. Block: one byte, bit 2.
            0x02, 0x02, 0x02, 0x01, 0x04,
            // Dark for the sky: one byte, bit 0. Dark for blocks: no bytes.
            0x01, 0x01, 0x00,
        ];
        // Two sky arrays, each after its length, 2,048, which is the VarInt 0x80 0x10.
        expected.push(0x02);
        for level in [0xFF, 0xFE] {
            expected.extend([0x80, 0x10]);
            expected.extend([level; 2048]);
        }
        // One block array.
        expected.extend([0x01, 0x80, 0x10]);
        expected.extend([0x0E; 2048]);

        assert_eq!(LightUpdate::ID, 49);
        assert_eq!(crate::packets::encode(&packet), expected);
        assert_eq!(
            ClientboundPlay::decode(&expected),
            Ok(ClientboundPlay::LightUpdate(packet))
        );
    }

    /// An update that names nothing is the id, the chunk and six zeros: no mask has a
    /// byte and neither list an array.
    #[test]
    fn a_light_update_that_names_no_section_is_six_zeros_after_the_chunk() {
        let packet = LightUpdate {
            chunk_x: -1,
            chunk_z: 300,
            light: LightData::default(),
        };
        assert_eq!(
            crate::packets::encode(&packet),
            [
                0x31, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F, 0xAC, 0x02, 0, 0, 0, 0, 0, 0
            ]
        );
    }

    #[test]
    fn light_updates_round_trip() {
        use ClientboundPlay as Set;
        assert_round_trip(
            LightUpdate {
                chunk_x: i32::MIN,
                chunk_z: i32::MAX,
                light: LightData {
                    sky_mask: vec![0b110],
                    block_mask: vec![1 << 25],
                    empty_sky_mask: vec![0b001],
                    empty_block_mask: vec![0x1FF_FFFE],
                    sky: vec![vec![0xFF; 2048], vec![0x0F; 2048]],
                    block: vec![vec![0x5A; 2048]],
                },
            },
            Set::decode,
            Set::LightUpdate,
        );
        assert_round_trip(
            LightUpdate {
                chunk_x: 0,
                chunk_z: 0,
                light: LightData::default(),
            },
            Set::decode,
            Set::LightUpdate,
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
        let keys = player_input::SHIFT | player_input::FORWARD;
        let packet = ServerboundPlayerInput { flags: keys };
        assert_eq!(crate::packets::encode(&packet), [43, 0x21]);
        assert_round_trip(packet, Set::decode, Set::ServerboundPlayerInput);
        let packet = ServerboundUseItem {
            hand: 0,
            sequence: 300,
            yaw: -90.0,
            pitch: 45.0,
        };
        // The id, the hand, the sequence as two bytes, then two floats.
        let bytes = crate::packets::encode(&packet);
        assert_eq!(bytes[..4], [67, 0, 0xac, 0x02]);
        assert_eq!(bytes[4..], [0xc2, 0xb4, 0, 0, 0x42, 0x34, 0, 0]);
        assert_round_trip(packet, Set::decode, Set::ServerboundUseItem);
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
        let stone = Some(ItemStack::new(1, 1));
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
        for stack in [stone.clone(), None] {
            assert_round_trip(
                SetCreativeModeSlot { slot: 36, stack },
                ServerboundPlay::decode,
                ServerboundPlay::SetCreativeModeSlot,
            );
        }
        let mut slots = vec![None; inventory::SLOT_COUNT];
        slots[inventory::HOTBAR_START] = stone.clone();
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
        for (slot, stack) in [(40, stone), (8, None)] {
            assert_round_trip(
                SetPlayerInventory { slot, stack },
                ClientboundPlay::decode,
                ClientboundPlay::SetPlayerInventory,
            );
        }
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

    /// The sound is a holder of the registry, its id plus one, as the game writes one
    /// it names; then the source, the place in eighths, volume, pitch and seed.
    #[test]
    fn a_sound_is_written_as_the_game_writes_it() {
        let mut w = Writer::default();
        Sound {
            sound: 1850,
            source: SOUND_SOURCE_BLOCKS,
            x: 20,
            y: -4,
            z: 8,
            volume: 1.0,
            pitch: 1.0,
            seed: 1,
        }
        .encode(&mut w);
        let bytes = w.into_bytes();
        assert_eq!(bytes[..3], [0xBB, 0x0E, 4], "1851 as a VarInt, then 4");
        assert_eq!(
            bytes[3..15],
            [0, 0, 0, 20, 0xFF, 0xFF, 0xFF, 0xFC, 0, 0, 0, 8]
        );
        assert_eq!(bytes.len(), 3 + 12 + 4 + 4 + 8);
    }

    /// The bytes of the moves of an entity as the game writes them
    /// (`ClientboundMoveEntityPacket.Pos`, `PosRot` and `Rot`): the id, then for a move
    /// the on-ground flag and the count of steps in one VarInt and the shorts of the
    /// delta, a step's ticks before its shorts; the angles a byte each.
    #[test]
    fn an_entity_s_moves_are_written_as_the_game_writes_them() {
        let linear = MoveEntityPos {
            entity_id: 300,
            delta: VecDelta::Linear([1, -2, 32767]),
            on_ground: true,
        };
        assert_eq!(
            crate::packets::encode(&linear),
            [0x36, 0xAC, 0x02, 0x01, 0x00, 0x01, 0xFF, 0xFE, 0x7F, 0xFF]
        );
        let turned = MoveEntityPosRot {
            entity_id: 5,
            delta: VecDelta::Linear([0, 4096, -1]),
            yaw: pack_degrees(200.0),
            pitch: pack_degrees(-90.0),
            on_ground: false,
        };
        assert_eq!(
            crate::packets::encode(&turned),
            [0x37, 5, 0, 0, 0, 0x10, 0, 0xFF, 0xFF, 0x8E, 0xC0]
        );
        let stepped = MoveEntityPos {
            entity_id: 1,
            delta: VecDelta::Stepped(vec![(2, [1, 2, 3]), (3, [0, 0, -1])]),
            on_ground: true,
        };
        assert_eq!(
            crate::packets::encode(&stepped),
            [0x36, 1, 5, 2, 0, 1, 0, 2, 0, 3, 3, 0, 0, 0, 0, 0xFF, 0xFF]
        );
        let rot = MoveEntityRot {
            entity_id: 1,
            yaw: 64,
            pitch: 0,
            on_ground: true,
        };
        assert_eq!(crate::packets::encode(&rot), [0x39, 1, 1, 64, 0]);
        // The ids are the game's.
        assert_eq!(
            [
                MoveEntityPos::ID,
                MoveEntityPosRot::ID,
                MoveEntityRot::ID,
                SetEntityMotion::ID,
                TakeItemEntity::ID,
                AddTransientBlock::ID
            ],
            [54, 55, 57, 103, 127, 37]
        );
        // More steps than the bytes left could hold are refused.
        assert!(ClientboundPlay::decode(&[0x36, 1, 0x0B, 2, 0, 1, 0, 2, 0, 3]).is_err());
    }

    #[test]
    fn an_angle_is_packed_as_the_floor_of_its_256ths_in_a_byte() {
        assert_eq!(pack_degrees(0.0), 0);
        assert_eq!(pack_degrees(200.0), -114);
        assert_eq!(pack_degrees(359.0), -1);
        assert_eq!(pack_degrees(360.0), 0);
        assert_eq!(pack_degrees(-1.0), -1);
        assert_eq!(pack_degrees(-90.0), -64);
        assert_eq!(pack_degrees(720.5), 0);
    }

    /// `ClientboundSetEntityMotionPacket`, `ClientboundTakeItemEntityPacket` and
    /// `ClientboundAddTransientBlockPacket`, and the byte of shared flags and the start
    /// of a falling block in an entity's data, byte for byte.
    #[test]
    fn motion_pick_up_a_transient_block_and_the_new_data_are_written_as_the_game_writes_them() {
        let still = SetEntityMotion {
            entity_id: 1,
            motion: [0.0; 3],
        };
        assert_eq!(crate::packets::encode(&still), [0x67, 1, 0]);
        let taken = TakeItemEntity {
            item_id: 7,
            collector_id: 300,
            count: 2,
        };
        assert_eq!(crate::packets::encode(&taken), [0x7F, 7, 0xAC, 0x02, 2]);
        // An entity event: its id as a 4-byte int, then the event's byte.
        let wobble = EntityEvent {
            entity_id: 258,
            event: entity_event::ARMOR_STAND_HIT,
        };
        assert_eq!(crate::packets::encode(&wobble), [0x22, 0, 0, 1, 2, 32]);
        let at = Position { x: 1, y: -60, z: 2 };
        let long = [0, 0, 0, 0x40, 0, 0, 0x2F, 0xC4];
        let transient = AddTransientBlock {
            position: at,
            state: 300,
        };
        let mut expected = vec![0x25];
        expected.extend(long);
        expected.extend([0xAC, 0x02]);
        assert_eq!(crate::packets::encode(&transient), expected);
        let burning = SetEntityData {
            entity_id: 1,
            values: vec![(
                entity_data::SHARED_FLAGS,
                EntityDataValue::Byte(entity_data::ON_FIRE),
            )],
        };
        assert_eq!(crate::packets::encode(&burning), [0x65, 1, 0, 0, 1, 0xFF]);
        let start = SetEntityData {
            entity_id: 1,
            values: vec![(entity_data::START_POSITION, EntityDataValue::BlockPos(at))],
        };
        let mut expected = vec![0x65, 1, 8, 10];
        expected.extend(long);
        expected.push(0xFF);
        assert_eq!(crate::packets::encode(&start), expected);
    }

    #[test]
    fn the_packets_of_an_entity_s_tracking_round_trip() {
        use ClientboundPlay as Set;
        assert_round_trip(
            MoveEntityPos {
                entity_id: 9,
                delta: VecDelta::Linear([-32768, 0, 12]),
                on_ground: false,
            },
            Set::decode,
            Set::MoveEntityPos,
        );
        assert_round_trip(
            MoveEntityPos {
                entity_id: 9,
                delta: VecDelta::Stepped(vec![(1, [5, 6, 7])]),
                on_ground: true,
            },
            Set::decode,
            Set::MoveEntityPos,
        );
        assert_round_trip(
            MoveEntityPosRot {
                entity_id: 9,
                delta: VecDelta::Linear([1, 2, 3]),
                yaw: -5,
                pitch: 7,
                on_ground: true,
            },
            Set::decode,
            Set::MoveEntityPosRot,
        );
        assert_round_trip(
            MoveEntityRot {
                entity_id: 9,
                yaw: 100,
                pitch: -100,
                on_ground: false,
            },
            Set::decode,
            Set::MoveEntityRot,
        );
        // At the protocol's steps, which read back as written.
        assert_round_trip(
            SetEntityMotion {
                entity_id: 9,
                motion: [1.0, 0.0, -1.0],
            },
            Set::decode,
            Set::SetEntityMotion,
        );
        assert_round_trip(
            TakeItemEntity {
                item_id: 9,
                collector_id: 10,
                count: 64,
            },
            Set::decode,
            Set::TakeItemEntity,
        );
        assert_round_trip(
            AddTransientBlock {
                position: Position {
                    x: -30_000_000,
                    y: 319,
                    z: 29_999_999,
                },
                state: 27_000,
            },
            Set::decode,
            Set::AddTransientBlock,
        );
        assert_round_trip(
            SetEntityData {
                entity_id: 9,
                values: vec![
                    (entity_data::SHARED_FLAGS, EntityDataValue::Byte(0)),
                    (
                        entity_data::START_POSITION,
                        EntityDataValue::BlockPos(Position {
                            x: -1,
                            y: -64,
                            z: 5,
                        }),
                    ),
                ],
            },
            Set::decode,
            Set::SetEntityData,
        );
    }

    /// A position and a turn as `PositionAndRotation` is written: x, y and z as
    /// doubles, then `yRot` and `xRot` as floats (0.5, 64, -2.25, 90 and -10 here).
    const AT_AND_TURNED: [u8; 32] = [
        0x3F, 0xE0, 0, 0, 0, 0, 0, 0, 0x40, 0x50, 0, 0, 0, 0, 0, 0, 0xC0, 0x02, 0, 0, 0, 0, 0, 0,
        0x42, 0xB4, 0, 0, 0xC1, 0x20, 0, 0,
    ];

    /// The serverbound packets of riding and of using an entity, byte for byte in the
    /// order of the fields of each class's codec (`ServerboundAttackPacket`,
    /// `ServerboundInteractPacket`, `ServerboundMoveVehiclePacket`,
    /// `ServerboundPaddleBoatPacket`, `ServerboundPickItemFromEntityPacket`,
    /// `ServerboundPlayerCommandPacket`).
    #[test]
    fn the_packets_a_player_sends_about_entities_are_read_as_the_game_writes_them() {
        assert_eq!(
            [
                Attack::ID,
                Interact::ID,
                ServerboundMoveVehicle::ID,
                PaddleBoat::ID,
                PickItemFromEntity::ID,
                PlayerCommand::ID
            ],
            [1, 26, 34, 35, 37, 42]
        );
        let attack = [0x01, 0xAC, 0x02];
        assert_eq!(
            ServerboundPlay::decode(&attack),
            Ok(ServerboundPlay::Attack(Attack { entity_id: 300 }))
        );
        // The entity, the hand, a location of zero as its one byte, the sneak key.
        let interact = [0x1A, 42, 1, 0, 1];
        assert_eq!(
            ServerboundPlay::decode(&interact),
            Ok(ServerboundPlay::Interact(Interact {
                entity_id: 42,
                hand: 1,
                location: [0.0; 3],
                using_secondary_action: true,
            }))
        );
        // `LpVec3` of (1, 0, -1): a scale of 1 in the low bits, then 32766, 16383 and 0
        // as 15-bit fractions, in six bytes.
        let interact = [0x1A, 5, 0, 0xF1, 0xFF, 0, 0, 0xFF, 0xFF, 0];
        assert_eq!(
            ServerboundPlay::decode(&interact),
            Ok(ServerboundPlay::Interact(Interact {
                entity_id: 5,
                hand: 0,
                location: [1.0, 0.0, -1.0],
                using_secondary_action: false,
            }))
        );
        // A hand out of range is the main hand, by `ByIdMap`'s `ZERO` strategy.
        assert_eq!(
            ServerboundPlay::decode(&[0x1A, 42, 2, 0, 0]),
            Ok(ServerboundPlay::Interact(Interact {
                entity_id: 42,
                hand: 0,
                location: [0.0; 3],
                using_secondary_action: false,
            }))
        );
        let mut moved = vec![0x22];
        moved.extend(AT_AND_TURNED);
        moved.push(1);
        assert_eq!(
            ServerboundPlay::decode(&moved),
            Ok(ServerboundPlay::ServerboundMoveVehicle(
                ServerboundMoveVehicle {
                    x: 0.5,
                    y: 64.0,
                    z: -2.25,
                    yaw: 90.0,
                    pitch: -10.0,
                    on_ground: true,
                }
            ))
        );
        assert_eq!(
            ServerboundPlay::decode(&[0x23, 1, 0]),
            Ok(ServerboundPlay::PaddleBoat(PaddleBoat {
                left: true,
                right: false,
            }))
        );
        assert_eq!(
            ServerboundPlay::decode(&[0x25, 0xAC, 0x02, 1]),
            Ok(ServerboundPlay::PickItemFromEntity(PickItemFromEntity {
                entity_id: 300,
                include_data: true,
            }))
        );
        // The player, the action's ordinal, the jump's charge.
        assert_eq!(
            ServerboundPlay::decode(&[0x2A, 7, 3, 90]),
            Ok(ServerboundPlay::PlayerCommand(PlayerCommand {
                entity_id: 7,
                action: player_command::START_RIDING_JUMP,
                data: 90,
            }))
        );
        assert_eq!(
            [
                player_command::STOP_SLEEPING,
                player_command::START_SPRINTING,
                player_command::STOP_SPRINTING,
                player_command::START_RIDING_JUMP,
                player_command::STOP_RIDING_JUMP,
                player_command::OPEN_INVENTORY,
                player_command::START_FALL_FLYING,
            ],
            [0, 1, 2, 3, 4, 5, 6]
        );
        // There is no eighth action, and the game refuses one.
        assert!(ServerboundPlay::decode(&[0x2A, 7, 7, 0]).is_err());
        assert!(ServerboundPlay::decode(&[0x2A, 7, 0x7F, 0]).is_err());
        // The bots write what the server reads.
        assert_eq!(crate::packets::encode(&Attack { entity_id: 300 }), attack);
        assert_eq!(
            crate::packets::encode(&Interact {
                entity_id: 5,
                hand: 0,
                location: [1.0, 0.0, -1.0],
                using_secondary_action: false,
            }),
            interact
        );
    }

    /// The clientbound packets of riding, of leads, of an entity's sound and of a swing,
    /// byte for byte in the order of the fields of `ClientboundMoveVehiclePacket`,
    /// `ClientboundSetPassengersPacket`, `ClientboundSetEntityLinkPacket`,
    /// `ClientboundSoundEntityPacket` and `ClientboundSwingAnimationPacket` (whose last
    /// field is the `SwingAnimation` component: its kind, then its duration).
    #[test]
    fn the_packets_of_riding_leads_sounds_and_swings_are_written_as_the_game_writes_them() {
        assert_eq!(
            [
                ClientboundMoveVehicle::ID,
                SetPassengers::ID,
                SetEntityLink::ID,
                SoundEntity::ID,
                SwingAnimation::ID
            ],
            [58, 109, 102, 118, 123]
        );
        let mut moved = vec![0x3A];
        moved.extend(AT_AND_TURNED);
        assert_eq!(
            crate::packets::encode(&ClientboundMoveVehicle {
                x: 0.5,
                y: 64.0,
                z: -2.25,
                yaw: 90.0,
                pitch: -10.0,
            }),
            moved
        );
        // The vehicle, then the count and each passenger as VarInts.
        assert_eq!(
            crate::packets::encode(&SetPassengers {
                vehicle_id: 9,
                passengers: vec![300, 2],
            }),
            [0x6D, 9, 2, 0xAC, 0x02, 2]
        );
        assert_eq!(
            crate::packets::encode(&SetPassengers {
                vehicle_id: 9,
                passengers: Vec::new(),
            }),
            [0x6D, 9, 0]
        );
        // Two four-byte ints, the holder 0 for a lead cut.
        assert_eq!(
            crate::packets::encode(&SetEntityLink {
                leashed_id: 300,
                holder_id: 0,
            }),
            [0x66, 0, 0, 0x01, 0x2C, 0, 0, 0, 0]
        );
        assert_eq!(
            crate::packets::encode(&SetEntityLink {
                leashed_id: 1,
                holder_id: -1,
            }),
            [0x66, 0, 0, 0, 1, 0xFF, 0xFF, 0xFF, 0xFF]
        );
        // The sound's id plus one (1851), the source, the entity, volume 1, pitch 0.5
        // and the seed.
        assert_eq!(
            crate::packets::encode(&SoundEntity {
                sound: 1850,
                source: 6,
                entity_id: 300,
                volume: 1.0,
                pitch: 0.5,
                seed: 1,
            }),
            [
                0x76, 0xBB, 0x0E, 6, 0xAC, 0x02, 0x3F, 0x80, 0, 0, 0x3F, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                0, 1
            ]
        );
        // A sound sent whole is not modelled.
        assert!(ClientboundPlay::decode(&[0x76, 0, 6, 1, 0, 0, 0, 0]).is_err());
        // The entity, the off hand, a stab of 13 ticks.
        assert_eq!(
            crate::packets::encode(&SwingAnimation {
                entity_id: 7,
                hand: 1,
                kind: swing_animation::STAB,
                duration: 13,
            }),
            [0x7B, 7, 1, 2, 13]
        );
        // A hand or a kind out of range is id 0, by `ByIdMap`'s `ZERO` strategy.
        assert_eq!(
            ClientboundPlay::decode(&[0x7B, 7, 2, 3, 13]),
            Ok(ClientboundPlay::SwingAnimation(SwingAnimation {
                entity_id: 7,
                hand: 0,
                kind: swing_animation::NONE,
                duration: 13,
            }))
        );
    }

    /// Each serializer N5's entities need, as `SetEntityData` writes it: the index, the
    /// serializer's id in the order `EntityDataSerializers` registers them, the value.
    #[test]
    fn the_entity_data_of_vehicles_stands_and_hanging_things_is_written_as_the_game_writes_it() {
        let one = |index: u8, value: EntityDataValue| {
            crate::packets::encode(&SetEntityData {
                entity_id: 1,
                values: vec![(index, value)],
            })
        };
        let with = |body: &[u8]| {
            let mut expected = vec![0x65, 1];
            expected.extend(body);
            expected.push(0xFF);
            expected
        };
        // `INT`, 1: a VarInt.
        assert_eq!(one(8, EntityDataValue::Int(300)), with(&[8, 1, 0xAC, 0x02]));
        assert_eq!(
            one(8, EntityDataValue::Int(-1)),
            with(&[8, 1, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F])
        );
        // `FLOAT`, 3: four bytes.
        assert_eq!(
            one(10, EntityDataValue::Float(1.0)),
            with(&[10, 3, 0x3F, 0x80, 0, 0])
        );
        // `BOOLEAN`, 8: one byte.
        assert_eq!(one(11, EntityDataValue::Boolean(true)), with(&[11, 8, 1]));
        // `ROTATIONS`, 9: x, y and z as floats (a left arm's default pose).
        assert_eq!(
            one(18, EntityDataValue::Rotations([-10.0, 0.0, 10.0])),
            with(&[18, 9, 0xC1, 0x20, 0, 0, 0, 0, 0, 0, 0x41, 0x20, 0, 0])
        );
        // `OPTIONAL_BLOCK_POS`, 11: a presence flag, then the long.
        assert_eq!(
            one(14, EntityDataValue::OptionalBlockPos(None)),
            with(&[14, 11, 0])
        );
        let at = Position { x: 1, y: -60, z: 2 };
        assert_eq!(
            one(14, EntityDataValue::OptionalBlockPos(Some(at))),
            with(&[14, 11, 1, 0, 0, 0, 0x40, 0, 0, 0x2F, 0xC4])
        );
        // `DIRECTION`, 12: the 3D value.
        assert_eq!(
            one(8, EntityDataValue::Direction(direction::SOUTH)),
            with(&[8, 12, 3])
        );
        // `BLOCK_STATE`, 14: the state's id.
        assert_eq!(
            one(9, EntityDataValue::BlockState(2141)),
            with(&[9, 14, 0xDD, 0x10])
        );
        // `OPTIONAL_BLOCK_STATE`, 15: the state's id, 0 for none.
        assert_eq!(
            one(11, EntityDataValue::OptionalBlockState(0)),
            with(&[11, 15, 0])
        );
        assert_eq!(
            one(11, EntityDataValue::OptionalBlockState(300)),
            with(&[11, 15, 0xAC, 0x02])
        );
        // `PAINTING_VARIANT`, 34: a holder, the variant's id plus one.
        assert_eq!(
            one(9, EntityDataValue::PaintingVariant(4)),
            with(&[9, 34, 5])
        );
        // `DYE_COLOR`, 43: the dye's id.
        assert_eq!(one(8, EntityDataValue::DyeColor(15)), with(&[8, 43, 15]));
        // `COW_VARIANT`, 23: the entry's id in the registry sent, as it is.
        assert_eq!(
            one(
                18,
                EntityDataValue::Registered {
                    serializer: entity_data_serializer::COW_VARIANT,
                    id: 1
                }
            ),
            with(&[18, 23, 1])
        );
        // A painting variant sent whole is not modelled.
        assert!(ClientboundPlay::decode(&[0x65, 1, 9, 34, 0, 0xFF]).is_err());
    }

    /// The serializers of the tamed animals (`docs/groundwork/n7-brief.md`, section 9.5):
    /// a wolf's end of anger a VarLong, an owner a presence flag and the UUID, the wolf's
    /// and cat's variants an entry's id; each read back as written.
    #[test]
    fn the_entity_data_of_tamed_animals_is_written_as_the_game_writes_it_and_read_back() {
        let one = |index: u8, value: EntityDataValue| {
            crate::packets::encode(&SetEntityData {
                entity_id: 1,
                values: vec![(index, value)],
            })
        };
        let with = |body: &[u8]| {
            let mut expected = vec![0x65, 1];
            expected.extend(body);
            expected.push(0xFF);
            expected
        };
        // `LONG`, 2: a VarLong; no anger is -1, ten bytes.
        assert_eq!(
            one(22, EntityDataValue::Long(-1)),
            with(&[
                22, 2, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01
            ])
        );
        assert_eq!(
            one(22, EntityDataValue::Long(300)),
            with(&[22, 2, 0xAC, 0x02])
        );
        // `OPTIONAL_LIVING_ENTITY_REFERENCE`, 13: none, or the UUID's two longs.
        assert_eq!(
            one(19, EntityDataValue::OptionalLivingEntityReference(None)),
            with(&[19, 13, 0])
        );
        let owner = Uuid::from_u128(0x0102_0304_0506_0708_090A_0B0C_0D0E_0F10);
        let mut body = vec![19, 13, 1];
        body.extend(owner.as_bytes());
        assert_eq!(
            one(
                19,
                EntityDataValue::OptionalLivingEntityReference(Some(owner))
            ),
            with(&body)
        );
        // `CAT_VARIANT` 21, `CAT_SOUND_VARIANT` 22, `WOLF_VARIANT` 25,
        // `WOLF_SOUND_VARIANT` 26: the entry's id as it is.
        for (serializer, index) in [
            (entity_data_serializer::CAT_VARIANT, 20),
            (entity_data_serializer::CAT_SOUND_VARIANT, 24),
            (entity_data_serializer::WOLF_VARIANT, 23),
            (entity_data_serializer::WOLF_SOUND_VARIANT, 24),
        ] {
            let value = EntityDataValue::Registered { serializer, id: 3 };
            assert_eq!(one(index, value), with(&[index, serializer as u8, 3]));
        }
        assert_eq!(
            [
                entity_data_serializer::LONG,
                entity_data_serializer::OPTIONAL_LIVING_ENTITY_REFERENCE,
                entity_data_serializer::CAT_VARIANT,
                entity_data_serializer::CAT_SOUND_VARIANT,
                entity_data_serializer::WOLF_VARIANT,
                entity_data_serializer::WOLF_SOUND_VARIANT,
            ],
            [2, 13, 21, 22, 25, 26]
        );
        let packet = SetEntityData {
            entity_id: 7,
            values: vec![
                (
                    19,
                    EntityDataValue::OptionalLivingEntityReference(Some(owner)),
                ),
                (22, EntityDataValue::Long(123_456_789_012)),
                (
                    23,
                    EntityDataValue::Registered {
                        serializer: entity_data_serializer::WOLF_VARIANT,
                        id: 8,
                    },
                ),
            ],
        };
        let bytes = crate::packets::encode(&packet);
        assert_eq!(
            ClientboundPlay::decode(&bytes),
            Ok(ClientboundPlay::SetEntityData(packet))
        );
    }

    /// A punch is its id alone (`ServerboundPunchPacket`'s codec is `StreamCodec.unit`),
    /// and a swing of the arm is four VarInts: the entity, the hand, the kind and the
    /// duration (`ClientboundSwingAnimationPacket`).
    #[test]
    fn a_punch_is_its_id_alone_and_a_swing_four_numbers() {
        assert_eq!(Punch::ID, 46);
        assert_eq!(
            ServerboundPlay::decode(&[46]),
            Ok(ServerboundPlay::Punch(Punch))
        );
        assert_eq!(crate::packets::encode(&Punch), [46]);
        let swing = SwingAnimation {
            entity_id: 300,
            hand: 0,
            kind: swing_animation::STAB,
            duration: 19,
        };
        assert_eq!(
            crate::packets::encode(&swing),
            [SwingAnimation::ID as u8, 0xAC, 0x02, 0, 2, 19]
        );
    }

    /// `VILLAGER_DATA` (`docs/groundwork/n7k-brief.md`, section 0.15): serializer 18,
    /// then `VillagerData.STREAM_CODEC`'s three VarInts in its order, the type's id, the
    /// profession's id and the level.
    #[test]
    fn a_villager_s_data_is_written_as_the_game_writes_it_and_read_back() {
        assert_eq!(entity_data_serializer::VILLAGER_DATA, 18);
        // A snow villager (type 4), a weaponsmith (profession 14), of level 1, at the
        // villager's index 19: the packet's id, the entity, the index, the serializer,
        // the type, the profession, the level, the end.
        let snow_weaponsmith = SetEntityData {
            entity_id: 1,
            values: vec![(
                19,
                EntityDataValue::VillagerData {
                    kind: 4,
                    profession: 14,
                    level: 1,
                },
            )],
        };
        let bytes = crate::packets::encode(&snow_weaponsmith);
        assert_eq!(bytes, [0x65, 1, 19, 18, 4, 14, 1, 0xFF]);
        assert_eq!(
            ClientboundPlay::decode(&bytes),
            Ok(ClientboundPlay::SetEntityData(snow_weaponsmith))
        );
        // The level is a VarInt as the ids are, so one past 127 takes two bytes; the
        // default villager (plains 2, none 0, level 1) at a zombie villager's index 20.
        let values = vec![
            (
                20,
                EntityDataValue::VillagerData {
                    kind: 2,
                    profession: 0,
                    level: 1,
                },
            ),
            (
                19,
                EntityDataValue::VillagerData {
                    kind: 6,
                    profession: 11,
                    level: 200,
                },
            ),
        ];
        let packet = SetEntityData {
            entity_id: 300,
            values,
        };
        let bytes = crate::packets::encode(&packet);
        assert_eq!(
            bytes,
            [
                0x65, 0xAC, 0x02, 20, 18, 2, 0, 1, 19, 18, 6, 11, 0xC8, 0x01, 0xFF
            ]
        );
        assert_eq!(
            ClientboundPlay::decode(&bytes),
            Ok(ClientboundPlay::SetEntityData(packet))
        );
    }

    #[test]
    fn the_packets_and_the_entity_data_of_riding_round_trip() {
        use ClientboundPlay as Client;
        use ServerboundPlay as Server;
        assert_round_trip(Attack { entity_id: -5 }, Server::decode, Server::Attack);
        assert_round_trip(
            Interact {
                entity_id: 9,
                hand: 1,
                location: [1.0, 0.0, -1.0],
                using_secondary_action: true,
            },
            Server::decode,
            Server::Interact,
        );
        assert_round_trip(
            ServerboundMoveVehicle {
                x: -1.5e7,
                y: 319.99,
                z: 0.1,
                yaw: -179.5,
                pitch: 12.25,
                on_ground: false,
            },
            Server::decode,
            Server::ServerboundMoveVehicle,
        );
        assert_round_trip(
            PaddleBoat {
                left: false,
                right: true,
            },
            Server::decode,
            Server::PaddleBoat,
        );
        assert_round_trip(
            PickItemFromEntity {
                entity_id: 70_000,
                include_data: false,
            },
            Server::decode,
            Server::PickItemFromEntity,
        );
        for action in player_command::STOP_SLEEPING..=player_command::START_FALL_FLYING {
            assert_round_trip(
                PlayerCommand {
                    entity_id: 12,
                    action,
                    data: 0,
                },
                Server::decode,
                Server::PlayerCommand,
            );
        }
        assert_round_trip(
            ClientboundMoveVehicle {
                x: 3.0,
                y: -64.0,
                z: 1e6,
                yaw: 45.0,
                pitch: -90.0,
            },
            Client::decode,
            Client::ClientboundMoveVehicle,
        );
        assert_round_trip(
            SetPassengers {
                vehicle_id: 4,
                passengers: vec![5, 600_000, -1],
            },
            Client::decode,
            Client::SetPassengers,
        );
        assert_round_trip(
            SetEntityLink {
                leashed_id: 8,
                holder_id: 9,
            },
            Client::decode,
            Client::SetEntityLink,
        );
        assert_round_trip(
            SoundEntity {
                sound: 0,
                source: 6,
                entity_id: 3,
                volume: 0.8,
                pitch: 1.2,
                seed: -7,
            },
            Client::decode,
            Client::SoundEntity,
        );
        assert_round_trip(
            SwingAnimation {
                entity_id: 3,
                hand: 0,
                kind: swing_animation::WHACK,
                duration: swing_animation::DEFAULT_DURATION,
            },
            Client::decode,
            Client::SwingAnimation,
        );
        assert_round_trip(
            SetEntityData {
                entity_id: 9,
                values: vec![
                    (8, EntityDataValue::Int(-3)),
                    (9, EntityDataValue::Float(40.5)),
                    (11, EntityDataValue::Boolean(false)),
                    (16, EntityDataValue::Rotations([1.0, -2.0, 3.5])),
                    (14, EntityDataValue::OptionalBlockPos(None)),
                    (
                        14,
                        EntityDataValue::OptionalBlockPos(Some(Position {
                            x: -7,
                            y: 300,
                            z: 9,
                        })),
                    ),
                    (8, EntityDataValue::Direction(direction::EAST)),
                    (11, EntityDataValue::OptionalBlockState(27_000)),
                    (9, EntityDataValue::PaintingVariant(0)),
                    (8, EntityDataValue::DyeColor(0)),
                    (
                        19,
                        EntityDataValue::Registered {
                            serializer: entity_data_serializer::PIG_SOUND_VARIANT,
                            id: 2,
                        },
                    ),
                ],
            },
            Client::decode,
            Client::SetEntityData,
        );
    }
}

/// A hurt that counts, which the client shows (`ClientboundDamageEventPacket`): the
/// entity, the damage type by its id in the registry the server sent, the entities that
/// caused it and dealt it (each its id plus one, 0 for none), and the position the
/// source names, if any.
#[derive(Debug, Clone, PartialEq)]
pub struct DamageEvent {
    pub entity_id: i32,
    pub damage_type: i32,
    pub cause: Option<i32>,
    pub direct: Option<i32>,
    pub position: Option<[f64; 3]>,
}

impl Packet for DamageEvent {
    const ID: i32 = clientbound::DAMAGE_EVENT;
}

impl Encode for DamageEvent {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.entity_id);
        w.put_var_int(self.damage_type);
        w.put_var_int(self.cause.map_or(0, |id| id + 1));
        w.put_var_int(self.direct.map_or(0, |id| id + 1));
        w.put_bool(self.position.is_some());
        if let Some([x, y, z]) = self.position {
            w.put_f64(x);
            w.put_f64(y);
            w.put_f64(z);
        }
    }
}

impl Decode for DamageEvent {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let entity_id = r.var_int()?;
        let damage_type = r.var_int()?;
        let optional = |id: i32| (id > 0).then(|| id - 1);
        let cause = optional(r.var_int()?);
        let direct = optional(r.var_int()?);
        let position = if r.bool()? {
            Some([r.f64()?, r.f64()?, r.f64()?])
        } else {
            None
        };
        Ok(Self {
            entity_id,
            damage_type,
            cause,
            direct,
            position,
        })
    }
}

/// One attribute of [`UpdateAttributes`]: its id in the game's registry, its base value
/// and its modifiers (id, amount, the operation's id).
#[derive(Debug, Clone, PartialEq)]
pub struct AttributeSnapshot {
    pub attribute: i32,
    pub base: f64,
    pub modifiers: Vec<(String, f64, i32)>,
}

/// The attributes a client keeps of a living entity
/// (`ClientboundUpdateAttributesPacket`): the entity, then each attribute.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateAttributes {
    pub entity_id: i32,
    pub attributes: Vec<AttributeSnapshot>,
}

impl Packet for UpdateAttributes {
    const ID: i32 = clientbound::UPDATE_ATTRIBUTES;
}

/// The longest identifier the game reads (`Identifier.STREAM_CODEC`, a string of up to
/// 32767 characters).
const IDENTIFIER_LENGTH: usize = 32767;

impl Encode for UpdateAttributes {
    fn encode(&self, w: &mut Writer) {
        w.put_var_int(self.entity_id);
        w.put_var_int(self.attributes.len() as i32);
        for snapshot in &self.attributes {
            w.put_var_int(snapshot.attribute);
            w.put_f64(snapshot.base);
            w.put_var_int(snapshot.modifiers.len() as i32);
            for (id, amount, operation) in &snapshot.modifiers {
                w.put_string(id);
                w.put_f64(*amount);
                w.put_var_int(*operation);
            }
        }
    }
}

impl Decode for UpdateAttributes {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let entity_id = r.var_int()?;
        let count = r.var_int()?;
        let mut attributes = Vec::new();
        for _ in 0..count {
            let attribute = r.var_int()?;
            let base = r.f64()?;
            let modifiers = r.var_int()?;
            let mut list = Vec::new();
            for _ in 0..modifiers {
                list.push((r.string(IDENTIFIER_LENGTH)?, r.f64()?, r.var_int()?));
            }
            attributes.push(AttributeSnapshot {
                attribute,
                base,
                modifiers: list,
            });
        }
        Ok(Self {
            entity_id,
            attributes,
        })
    }
}

/// A player's experience bar (`ClientboundSetExperiencePacket`): the progress towards the
/// next level as a float, then the level and the points in all.
#[derive(Debug, Clone, PartialEq)]
pub struct SetExperience {
    pub progress: f32,
    pub level: i32,
    pub total: i32,
}

impl Packet for SetExperience {
    const ID: i32 = clientbound::SET_EXPERIENCE;
}

impl Encode for SetExperience {
    fn encode(&self, w: &mut Writer) {
        w.put_f32(self.progress);
        w.put_var_int(self.level);
        w.put_var_int(self.total);
    }
}

impl Decode for SetExperience {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            progress: r.f32()?,
            level: r.var_int()?,
            total: r.var_int()?,
        })
    }
}

/// A message of the server to the player (`ClientboundSystemChatPacket`): a text
/// component, in their chat or, with `overlay`, on their action bar.
#[derive(Debug, Clone, PartialEq)]
pub struct SystemChat {
    pub content: Nbt,
    pub overlay: bool,
}

impl SystemChat {
    /// A sentence of the game by its key, with whole numbers for its arguments.
    pub fn translated(key: &str, args: &[i32], overlay: bool) -> Self {
        let mut entries = vec![("translate".to_owned(), Nbt::String(key.to_owned()))];
        if !args.is_empty() {
            let with = args.iter().map(|arg| Nbt::Int(*arg)).collect();
            entries.push(("with".to_owned(), Nbt::List(with)));
        }
        Self {
            content: Nbt::Compound(entries),
            overlay,
        }
    }
}

impl Packet for SystemChat {
    const ID: i32 = clientbound::SYSTEM_CHAT;
}

impl Encode for SystemChat {
    fn encode(&self, w: &mut Writer) {
        w.put_nbt(&self.content);
        w.put_bool(self.overlay);
    }
}

impl Decode for SystemChat {
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let content = r.nbt()?.ok_or(DecodeError::InvalidValue {
            what: "text component",
            value: 0,
        })?;
        Ok(Self {
            content,
            overlay: r.bool()?,
        })
    }
}
