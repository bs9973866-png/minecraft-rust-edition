//! Minecraft Java protocol primitives for protocol 47 (1.8.9).

use std::error::Error as StdError;
use std::fmt;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;

use crate::nbt::{self, NamedTag, Tag};
use crate::world::{BlockPos, BlockState, ChunkColumnData, ChunkPos, ChunkSectionData, World};

pub const PROTOCOL_VERSION: i32 = 47;
pub const DEFAULT_MAX_FRAME_SIZE: usize = 2 * 1024 * 1024;

#[derive(Debug)]
pub enum ProtocolError {
    Io(io::Error), Compression(String), UnexpectedEof, VarIntTooLong, InvalidLength(i32),
    FrameTooLarge { length: usize, maximum: usize }, InvalidUtf8,
    StringTooLong { length: usize, maximum: usize },
    InvalidChunkData(&'static str), InvalidBlockUpdate(&'static str),
    UnexpectedPacket { expected: i32, actual: i32 }, InvalidAddress(String),
    OnlineModeRequired, LoginRejected(String), InvalidLoginPacket(i32),
    MissingJoinGame, WorldUpdate(String), InvalidHotbarSlot(i32),
    InvalidItemStack(&'static str), Nbt(String), InvalidWindowData(&'static str),
    InvalidBlockPlacement(&'static str), InvalidChatMessage,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "network I/O error: {error}"),
            Self::Compression(error) => write!(f, "zlib compression error: {error}"),
            Self::UnexpectedEof => write!(f, "unexpected end of packet"),
            Self::VarIntTooLong => write!(f, "VarInt is longer than five bytes"),
            Self::InvalidLength(length) => write!(f, "invalid packet length: {length}"),
            Self::FrameTooLarge { length, maximum } => write!(f, "packet frame is too large ({length} bytes; maximum {maximum})"),
            Self::InvalidUtf8 => write!(f, "packet string is not valid UTF-8"),
            Self::InvalidChunkData(reason) => write!(f, "invalid chunk data: {reason}"),
            Self::InvalidBlockUpdate(reason) => write!(f, "invalid block update: {reason}"),
            Self::StringTooLong { length, maximum } => write!(f, "packet string is too long ({length} bytes; maximum {maximum})"),
            Self::UnexpectedPacket { expected, actual } => write!(f, "unexpected packet id {actual}; expected {expected}"),
            Self::InvalidAddress(address) => write!(f, "invalid server address: {address}"),
            Self::OnlineModeRequired => write!(f, "server requires online-mode encryption/authentication; this client only supports offline login"),
            Self::LoginRejected(reason) => write!(f, "server rejected login: {reason}"),
            Self::InvalidLoginPacket(id) => write!(f, "unexpected login-state packet id: {id}"),
            Self::MissingJoinGame => write!(f, "received chunk data before Join Game"),
            Self::WorldUpdate(error) => write!(f, "failed to update world: {error}"),
            Self::InvalidHotbarSlot(slot) => write!(f, "hotbar slot must be between 0 and 8, got {slot}"),
            Self::InvalidItemStack(reason) => write!(f, "invalid item stack: {reason}"),
            Self::Nbt(error) => write!(f, "invalid item NBT: {error}"),
            Self::InvalidWindowData(reason) => write!(f, "invalid inventory window data: {reason}"),
            Self::InvalidBlockPlacement(reason) => write!(f, "invalid block placement: {reason}"),
            Self::InvalidChatMessage => write!(f, "chat message exceeds 100 characters"),
        }
    }
}
impl StdError for ProtocolError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self { Self::Io(error) => Some(error), _ => None }
    }
}
impl From<io::Error> for ProtocolError {
    fn from(value: io::Error) -> Self {
        if value.kind() == io::ErrorKind::UnexpectedEof { Self::UnexpectedEof } else { Self::Io(value) }
    }
}

/// Read a signed Minecraft VarInt from a byte stream.
pub fn read_varint<R: Read>(reader: &mut R) -> Result<i32, ProtocolError> {
    let mut result = 0_u32;
    for index in 0..5 {
        let mut byte = [0_u8; 1];
        reader.read_exact(&mut byte)?;
        result |= u32::from(byte[0] & 0x7f) << (7 * index);
        if byte[0] & 0x80 == 0 { return Ok(result as i32); }
    }
    Err(ProtocolError::VarIntTooLong)
}

/// Append a signed Minecraft VarInt to a byte vector.
pub fn write_varint(value: i32, output: &mut Vec<u8>) {
    let mut remaining = value as u32;
    loop {
        if remaining & !0x7f == 0 { output.push(remaining as u8); return; }
        output.push(((remaining & 0x7f) as u8) | 0x80);
        remaining >>= 7;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ItemStack {
    pub item_id: i16,
    pub count: i8,
    pub damage: i16,
    pub tag: Option<NamedTag>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SetSlotData {
    pub window_id: i8,
    pub slot: i16,
    pub item: Option<ItemStack>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowItemsData {
    pub window_id: u8,
    pub items: Vec<Option<ItemStack>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenWindowData {
    pub window_id: u8,
    pub inventory_type: String,
    pub title_json: String,
    pub slot_count: u8,
    pub horse_entity_id: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CloseWindowData { pub window_id: u8 }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowPropertyData {
    pub window_id: u8,
    pub property: i16,
    pub value: i16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfirmTransactionData {
    pub window_id: u8,
    pub action_number: i16,
    pub accepted: bool,
}
/// A bounded cursor for decoding packet payloads.
#[derive(Clone, Debug)]
pub struct PacketCursor<'a> { bytes: &'a [u8], offset: usize }

impl<'a> PacketCursor<'a> {
    pub fn new(bytes: &'a [u8]) -> Self { Self { bytes, offset: 0 } }
    pub fn remaining(&self) -> usize { self.bytes.len().saturating_sub(self.offset) }
    pub fn is_empty(&self) -> bool { self.remaining() == 0 }

    pub fn read_u8(&mut self) -> Result<u8, ProtocolError> {
        let value = *self.bytes.get(self.offset).ok_or(ProtocolError::UnexpectedEof)?;
        self.offset += 1;
        Ok(value)
    }
    pub fn read_i8(&mut self) -> Result<i8, ProtocolError> { Ok(self.read_u8()? as i8) }
    pub fn read_u16(&mut self) -> Result<u16, ProtocolError> {
        let bytes = self.read_exact_bytes(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }
    pub fn read_i16(&mut self) -> Result<i16, ProtocolError> { Ok(self.read_u16()? as i16) }
    pub fn read_i32(&mut self) -> Result<i32, ProtocolError> {
        let bytes = self.read_exact_bytes(4)?;
        Ok(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
    pub fn read_i64(&mut self) -> Result<i64, ProtocolError> {
        let bytes = self.read_exact_bytes(8)?;
        Ok(i64::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]]))
    }
    pub fn read_f32(&mut self) -> Result<f32, ProtocolError> { Ok(f32::from_bits(self.read_i32()? as u32)) }
    pub fn read_f64(&mut self) -> Result<f64, ProtocolError> { Ok(f64::from_bits(self.read_i64()? as u64)) }
    pub fn read_varint(&mut self) -> Result<i32, ProtocolError> {
        let mut result = 0_u32;
        for index in 0..5 {
            let byte = self.read_u8()?;
            result |= u32::from(byte & 0x7f) << (7 * index);
            if byte & 0x80 == 0 { return Ok(result as i32); }
        }
        Err(ProtocolError::VarIntTooLong)
    }
    pub fn read_bytes(&mut self, length: usize) -> Result<&'a [u8], ProtocolError> {
        let end = self.offset.checked_add(length).ok_or(ProtocolError::UnexpectedEof)?;
        let slice = self.bytes.get(self.offset..end).ok_or(ProtocolError::UnexpectedEof)?;
        self.offset = end;
        Ok(slice)
    }
    pub fn read_string(&mut self, maximum_bytes: usize) -> Result<String, ProtocolError> {
        let signed_length = self.read_varint()?;
        if signed_length < 0 { return Err(ProtocolError::InvalidLength(signed_length)); }
        let length = signed_length as usize;
        if length > maximum_bytes {
            return Err(ProtocolError::StringTooLong { length, maximum: maximum_bytes });
        }
        let bytes = self.read_bytes(length)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| ProtocolError::InvalidUtf8)
    }
    pub fn read_exact_bytes(&mut self, length: usize) -> Result<Vec<u8>, ProtocolError> {
        Ok(self.read_bytes(length)?.to_vec())
    }
    /// Read a protocol-47 Slot/ItemStack, including optional uncompressed root-compound NBT.
    pub fn read_item_stack(&mut self) -> Result<Option<ItemStack>, ProtocolError> {
        let item_id = self.read_i16()?;
        if item_id == -1 { return Ok(None); }
        if item_id < 0 { return Err(ProtocolError::InvalidItemStack("negative item ID")); }
        let count = self.read_i8()?;
        let damage = self.read_i16()?;
        let tag_id = *self.bytes.get(self.offset).ok_or(ProtocolError::UnexpectedEof)?;
        let tag = if tag_id == 0 {
            self.offset += 1;
            None
        } else {
            if tag_id != 10 { return Err(ProtocolError::InvalidItemStack("item NBT root must be a compound or TAG_End")); }
            let remaining = self.bytes.get(self.offset..).ok_or(ProtocolError::UnexpectedEof)?;
            let limits = nbt::Limits {
                max_depth: 64,
                max_elements: 262_144,
                max_array_len: 2 * 1024 * 1024,
                max_string_bytes: 65_535,
            };
            let (root, consumed) = nbt::parse_prefix_with_limits(remaining, limits)
                .map_err(|error| ProtocolError::Nbt(error.to_string()))?;
            if !matches!(root.value, Tag::Compound(_)) {
                return Err(ProtocolError::InvalidItemStack("item NBT root is not a compound"));
            }
            self.offset = self.offset.checked_add(consumed).ok_or(ProtocolError::UnexpectedEof)?;
            Some(root)
        };
        Ok(Some(ItemStack { item_id, count, damage, tag }))
    }
}

/// Serialize a protocol-47 Slot/ItemStack, including optional uncompressed root-compound NBT.
pub fn write_item_stack(item: Option<&ItemStack>, output: &mut Vec<u8>) -> Result<(), ProtocolError> {
    let Some(item) = item else {
        output.extend_from_slice(&(-1_i16).to_be_bytes());
        return Ok(());
    };
    if item.item_id < 0 { return Err(ProtocolError::InvalidItemStack("item ID must be non-negative")); }
    output.extend_from_slice(&item.item_id.to_be_bytes());
    output.push(item.count as u8);
    output.extend_from_slice(&item.damage.to_be_bytes());
    if let Some(tag) = &item.tag {
        if !matches!(tag.value, Tag::Compound(_)) {
            return Err(ProtocolError::InvalidItemStack("item NBT root must be a compound"));
        }
        let encoded = nbt::encode(tag).map_err(|error| ProtocolError::Nbt(error.to_string()))?;
        output.extend_from_slice(&encoded);
    } else {
        output.push(0);
    }
    Ok(())
}

/// Build the C08 payload. Face 255 means use the held item without targeting a block.
pub fn block_placement_payload(position: BlockPos, face: u8, item: Option<&ItemStack>, cursor: [f32; 3]) -> Result<Vec<u8>, ProtocolError> {
    if face > 5 && face != 255 { return Err(ProtocolError::InvalidBlockPlacement("face must be 0..=5 or 255")); }
    if cursor.iter().any(|value| !value.is_finite() || !(0.0..=1.0).contains(value)) {
        return Err(ProtocolError::InvalidBlockPlacement("cursor coordinates must be finite and between 0 and 1"));
    }
    let mut payload = Vec::with_capacity(32);
    payload.extend_from_slice(&encode_packed_position(position).to_be_bytes());
    payload.push(face);
    write_item_stack(item, &mut payload)?;
    for coordinate in cursor { payload.push((coordinate * 16.0) as u8); }
    Ok(payload)
}

/// Encode C08 Player Block Placement (protocol 47).
pub fn encode_block_placement(position: BlockPos, face: u8, item: Option<&ItemStack>, cursor: [f32; 3]) -> Result<Vec<u8>, ProtocolError> {
    Ok(encode_packet(0x08, &block_placement_payload(position, face, item, cursor)?))
}
/// Encode a packet body, including the packet ID but excluding the outer frame length.
pub fn encode_packet(packet_id: i32, payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(5 + payload.len());
    write_varint(packet_id, &mut body);
    body.extend_from_slice(payload);
    let mut frame = Vec::with_capacity(5 + body.len());
    write_varint(body.len() as i32, &mut frame);
    frame.extend_from_slice(&body);
    frame
}

/// Encode a packet using the compression framing enabled by Set Compression.
pub fn encode_compressed_packet(packet_id: i32, payload: &[u8], threshold: usize) -> Result<Vec<u8>, ProtocolError> {
    let mut packet_data = Vec::with_capacity(5 + payload.len());
    write_varint(packet_id, &mut packet_data);
    packet_data.extend_from_slice(payload);

    let mut body = Vec::new();
    if packet_data.len() >= threshold {
        write_varint(packet_data.len() as i32, &mut body);
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&packet_data).map_err(|e| ProtocolError::Compression(e.to_string()))?;
        let compressed = encoder.finish().map_err(|e| ProtocolError::Compression(e.to_string()))?;
        body.extend_from_slice(&compressed);
    } else {
        write_varint(0, &mut body);
        body.extend_from_slice(&packet_data);
    }

    let mut frame = Vec::with_capacity(5 + body.len());
    write_varint(body.len() as i32, &mut frame);
    frame.extend_from_slice(&body);
    Ok(frame)
}

/// Read one packet after compression has been negotiated. Returns packet ID plus payload.
pub fn read_compressed_frame<R: Read>(reader: &mut R, maximum_uncompressed: usize) -> Result<Vec<u8>, ProtocolError> {
    let frame = read_frame(reader, maximum_uncompressed.saturating_add(5))?;
    let mut cursor = PacketCursor::new(&frame);
    let declared_length = cursor.read_varint()?;
    if declared_length < 0 { return Err(ProtocolError::InvalidLength(declared_length)); }
    let content = cursor.read_bytes(cursor.remaining())?;
    if declared_length == 0 {
        if content.len() > maximum_uncompressed {
            return Err(ProtocolError::FrameTooLarge { length: content.len(), maximum: maximum_uncompressed });
        }
        return Ok(content.to_vec());
    }
    let expected = declared_length as usize;
    if expected > maximum_uncompressed {
        return Err(ProtocolError::FrameTooLarge { length: expected, maximum: maximum_uncompressed });
    }
    let decoder = ZlibDecoder::new(content);
    let mut decoded = Vec::with_capacity(expected);
    decoder.take(maximum_uncompressed as u64 + 1).read_to_end(&mut decoded)
        .map_err(|e| ProtocolError::Compression(e.to_string()))?;
    if decoded.len() != expected {
        return Err(ProtocolError::Compression(format!("declared uncompressed length {expected}, got {}", decoded.len())));
    }
    Ok(decoded)
}

/// Decode clientbound S2F Set Slot (0x2F). Window -1, slot -1 updates the carried cursor stack.
pub fn decode_set_slot(payload: &[u8]) -> Result<SetSlotData, ProtocolError> {
    let mut cursor = PacketCursor::new(payload);
    let data = SetSlotData { window_id: cursor.read_i8()?, slot: cursor.read_i16()?, item: cursor.read_item_stack()? };
    if !cursor.is_empty() { return Err(ProtocolError::InvalidWindowData("trailing bytes in Set Slot")); }
    Ok(data)
}

/// Decode clientbound S30 Window Items (0x30).
pub fn decode_window_items(payload: &[u8]) -> Result<WindowItemsData, ProtocolError> {
    let mut cursor = PacketCursor::new(payload);
    let window_id = cursor.read_u8()?;
    let count = cursor.read_i16()?;
    if !(0..=4096).contains(&count) { return Err(ProtocolError::InvalidWindowData("slot count must be between 0 and 4096")); }
    let mut items = Vec::with_capacity(count as usize);
    for _ in 0..count { items.push(cursor.read_item_stack()?); }
    if !cursor.is_empty() { return Err(ProtocolError::InvalidWindowData("trailing bytes in Window Items")); }
    Ok(WindowItemsData { window_id, items })
}

/// Decode S2D Open Window (0x2D).
pub fn decode_open_window(payload: &[u8]) -> Result<OpenWindowData, ProtocolError> {
    let mut cursor = PacketCursor::new(payload);
    let window_id = cursor.read_u8()?;
    let inventory_type = cursor.read_string(64)?;
    let title_json = cursor.read_string(32_767)?;
    let slot_count = cursor.read_u8()?;
    let horse_entity_id = if inventory_type == "EntityHorse" { Some(cursor.read_i32()?) } else { None };
    if !cursor.is_empty() { return Err(ProtocolError::InvalidWindowData("trailing bytes in Open Window")); }
    Ok(OpenWindowData { window_id, inventory_type, title_json, slot_count, horse_entity_id })
}

pub fn decode_close_window(payload: &[u8]) -> Result<CloseWindowData, ProtocolError> {
    let mut cursor = PacketCursor::new(payload);
    let data = CloseWindowData { window_id: cursor.read_u8()? };
    if !cursor.is_empty() { return Err(ProtocolError::InvalidWindowData("trailing bytes in Close Window")); }
    Ok(data)
}

pub fn decode_window_property(payload: &[u8]) -> Result<WindowPropertyData, ProtocolError> {
    let mut cursor = PacketCursor::new(payload);
    let data = WindowPropertyData { window_id: cursor.read_u8()?, property: cursor.read_i16()?, value: cursor.read_i16()? };
    if !cursor.is_empty() { return Err(ProtocolError::InvalidWindowData("trailing bytes in Window Property")); }
    Ok(data)
}

pub fn decode_confirm_transaction(payload: &[u8]) -> Result<ConfirmTransactionData, ProtocolError> {
    let mut cursor = PacketCursor::new(payload);
    let window_id = cursor.read_u8()?;
    let action_number = cursor.read_i16()?;
    let raw_accepted = cursor.read_u8()?;
    if raw_accepted > 1 { return Err(ProtocolError::InvalidWindowData("invalid transaction acceptance flag")); }
    if !cursor.is_empty() { return Err(ProtocolError::InvalidWindowData("trailing bytes in Confirm Transaction")); }
    Ok(ConfirmTransactionData { window_id, action_number, accepted: raw_accepted != 0 })
}

/// Clientbound S32 may be rejected; the client answers with C0F using accepted=true.
pub fn encode_confirm_transaction_reply(window_id: u8, action_number: i16, accepted: bool) -> Vec<u8> {
    let mut payload = Vec::with_capacity(4);
    payload.push(window_id);
    payload.extend_from_slice(&action_number.to_be_bytes());
    payload.push(u8::from(accepted));
    encode_packet(0x0F, &payload)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginSuccess {
    pub uuid: String,
    pub username: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginDisconnect { pub reason_json: String }

pub fn encode_login_start(username: &str) -> Result<Vec<u8>, ProtocolError> {
    if username.is_empty() || username.len() > 16 || !username.is_ascii() {
        return Err(ProtocolError::InvalidBlockUpdate("offline username must be 1-16 ASCII bytes"));
    }
    let mut payload = Vec::new();
    write_string(username, &mut payload);
    Ok(encode_packet(0x00, &payload))
}

pub fn decode_login_success(payload: &[u8]) -> Result<LoginSuccess, ProtocolError> {
    let mut cursor = PacketCursor::new(payload);
    let uuid = cursor.read_string(36)?;
    let username = cursor.read_string(16)?;
    if !cursor.is_empty() { return Err(ProtocolError::InvalidBlockUpdate("trailing bytes in Login Success")); }
    Ok(LoginSuccess { uuid, username })
}

pub fn decode_login_disconnect(payload: &[u8]) -> Result<LoginDisconnect, ProtocolError> {
    let mut cursor = PacketCursor::new(payload);
    let reason_json = cursor.read_string(32_767)?;
    if !cursor.is_empty() { return Err(ProtocolError::InvalidBlockUpdate("trailing bytes in Login Disconnect")); }
    Ok(LoginDisconnect { reason_json })
}

pub fn decode_set_compression(payload: &[u8]) -> Result<usize, ProtocolError> {
    let mut cursor = PacketCursor::new(payload);
    let threshold = cursor.read_varint()?;
    if threshold < 0 { return Err(ProtocolError::InvalidLength(threshold)); }
    if !cursor.is_empty() { return Err(ProtocolError::InvalidBlockUpdate("trailing bytes in Set Compression")); }
    Ok(threshold as usize)
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinGameInfo {
    pub entity_id: i32,
    pub game_mode: u8,
    pub hardcore_mode: bool,
    pub dimension: i8,
    pub difficulty: u8,
    pub max_players: u8,
    pub level_type: String,
    pub reduced_debug_info: bool,
}
impl JoinGameInfo {
    pub fn has_sky_light(&self) -> bool { self.dimension == 0 }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeepAlive { pub id: i32 }

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerPositionLook {
    pub x: f64, pub y: f64, pub z: f64,
    pub yaw: f32, pub pitch: f32, pub flags: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerMove {
    pub x: f64, pub y: f64, pub z: f64,
    pub yaw: f32, pub pitch: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeUpdate { pub world_age: i64, pub time_of_day: i64 }

pub fn decode_join_game(payload: &[u8]) -> Result<JoinGameInfo, ProtocolError> {
    let mut cursor = PacketCursor::new(payload);
    let entity_id = cursor.read_i32()?;
    let raw_game_mode = cursor.read_u8()?;
    let hardcore_mode = raw_game_mode & 8 != 0;
    let game_mode = raw_game_mode & !8;
    let dimension = cursor.read_i8()?;
    let difficulty = cursor.read_u8()?;
    let max_players = cursor.read_u8()?;
    let level_type = cursor.read_string(16)?;
    let reduced = cursor.read_u8()?;
    if reduced > 1 { return Err(ProtocolError::InvalidBlockUpdate("invalid reduced-debug flag")); }
    if !cursor.is_empty() { return Err(ProtocolError::InvalidBlockUpdate("trailing bytes in Join Game")); }
    Ok(JoinGameInfo { entity_id, game_mode, hardcore_mode, dimension, difficulty, max_players, level_type, reduced_debug_info: reduced != 0 })
}

pub fn decode_keep_alive(payload: &[u8]) -> Result<KeepAlive, ProtocolError> {
    let mut cursor = PacketCursor::new(payload);
    let id = cursor.read_varint()?;
    if !cursor.is_empty() { return Err(ProtocolError::InvalidBlockUpdate("trailing bytes in Keep Alive")); }
    Ok(KeepAlive { id })
}

pub fn decode_player_position_look(payload: &[u8]) -> Result<PlayerPositionLook, ProtocolError> {
    let mut cursor = PacketCursor::new(payload);
    let update = PlayerPositionLook {
        x: cursor.read_f64()?, y: cursor.read_f64()?, z: cursor.read_f64()?,
        yaw: cursor.read_f32()?, pitch: cursor.read_f32()?, flags: cursor.read_u8()?,
    };
    if !cursor.is_empty() { return Err(ProtocolError::InvalidBlockUpdate("trailing bytes in Player Position & Look")); }
    Ok(update)
}

pub fn decode_time_update(payload: &[u8]) -> Result<TimeUpdate, ProtocolError> {
    let mut cursor = PacketCursor::new(payload);
    let update = TimeUpdate { world_age: cursor.read_i64()?, time_of_day: cursor.read_i64()? };
    if !cursor.is_empty() { return Err(ProtocolError::InvalidBlockUpdate("trailing bytes in Time Update")); }
    Ok(update)
}

pub fn decode_spawn_position(payload: &[u8]) -> Result<BlockPos, ProtocolError> {
    let mut cursor = PacketCursor::new(payload);
    let position = decode_packed_position(cursor.read_i64()? as u64);
    if !cursor.is_empty() { return Err(ProtocolError::InvalidBlockUpdate("trailing bytes in Spawn Position")); }
    Ok(position)
}

pub fn encode_keep_alive_response(id: i32) -> Vec<u8> {
    let mut payload = Vec::new();
    write_varint(id, &mut payload);
    encode_packet(0x00, &payload)
}

/// Serverbound Player Position & Look (packet ID 0x06).
/// Digging actions sent in C07 Player Digging (0x07).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum DiggingStatus {
    Start = 0, Cancel = 1, Finish = 2, DropStack = 3, DropItem = 4, ReleaseUseItem = 5,
}

/// Block face ordinals used by protocol 47.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum BlockFace { Down = 0, Up = 1, North = 2, South = 3, West = 4, East = 5 }

fn encode_packed_position(position: BlockPos) -> i64 {
    (((position.x as i64) & 0x03ff_ffff) << 38)
        | (((position.y as i64) & 0x0fff) << 26)
        | ((position.z as i64) & 0x03ff_ffff)
}

pub fn decode_held_item_change(payload: &[u8]) -> Result<u8, ProtocolError> {
    if payload.len() != 1 { return Err(ProtocolError::InvalidBlockUpdate("held-item change must contain one byte")); }
    if payload[0] > 8 { return Err(ProtocolError::InvalidHotbarSlot(payload[0] as i32)); }
    Ok(payload[0])
}

pub fn encode_held_item_change(slot: u8) -> Result<Vec<u8>, ProtocolError> {
    if slot > 8 { return Err(ProtocolError::InvalidHotbarSlot(slot as i32)); }
    Ok(encode_packet(0x09, &(slot as i16).to_be_bytes()))
}

pub fn encode_player_position(x: f64, y: f64, z: f64, on_ground: bool) -> Vec<u8> {
    let mut payload = Vec::with_capacity(25);
    payload.extend_from_slice(&x.to_be_bytes());
    payload.extend_from_slice(&y.to_be_bytes());
    payload.extend_from_slice(&z.to_be_bytes());
    payload.push(u8::from(on_ground));
    encode_packet(0x04, &payload)
}

pub fn encode_player_look(yaw: f32, pitch: f32, on_ground: bool) -> Vec<u8> {
    let mut payload = Vec::with_capacity(9);
    payload.extend_from_slice(&yaw.to_be_bytes());
    payload.extend_from_slice(&pitch.to_be_bytes());
    payload.push(u8::from(on_ground));
    encode_packet(0x05, &payload)
}

pub fn encode_player_ground(on_ground: bool) -> Vec<u8> {
    encode_packet(0x03, &[u8::from(on_ground)])
}

pub fn encode_player_digging(status: DiggingStatus, position: BlockPos, face: BlockFace) -> Vec<u8> {
    let mut payload = Vec::with_capacity(10);
    write_varint(status as i32, &mut payload);
    payload.extend_from_slice(&encode_packed_position(position).to_be_bytes());
    payload.push(face as u8);
    encode_packet(0x07, &payload)
}

pub fn player_position_look_payload(update: PlayerMove, on_ground: bool) -> Vec<u8> {
    let mut payload = Vec::with_capacity(33);
    payload.extend_from_slice(&update.x.to_be_bytes());
    payload.extend_from_slice(&update.y.to_be_bytes());
    payload.extend_from_slice(&update.z.to_be_bytes());
    payload.extend_from_slice(&update.yaw.to_be_bytes());
    payload.extend_from_slice(&update.pitch.to_be_bytes());
    payload.push(u8::from(on_ground));
    payload
}

pub fn encode_player_position_look(update: PlayerMove, on_ground: bool) -> Vec<u8> {
    encode_packet(0x06, &player_position_look_payload(update, on_ground))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapChunkBulkData {
    pub sky_light_sent: bool,
    pub columns: Vec<ChunkColumnData>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockChange {
    pub position: BlockPos,
    pub state: BlockState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MultiBlockChangeRecord {
    pub position: BlockPos,
    pub state: BlockState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MultiBlockChange {
    pub chunk: ChunkPos,
    pub records: Vec<MultiBlockChangeRecord>,
}

fn sign_extend(value: u64, bits: u32) -> i32 {
    let shift = 64 - bits;
    ((value << shift) as i64 >> shift) as i32
}

fn decode_packed_position(value: u64) -> BlockPos {
    let x = sign_extend((value >> 38) & 0x03ff_ffff, 26);
    let y = sign_extend((value >> 26) & 0x0fff, 12);
    let z = sign_extend(value & 0x03ff_ffff, 26);
    BlockPos::new(x, y, z)
}

fn read_block_state(cursor: &mut PacketCursor<'_>) -> Result<BlockState, ProtocolError> {
    let state = cursor.read_varint()?;
    if !(0..=u16::MAX as i32).contains(&state) {
        return Err(ProtocolError::InvalidBlockUpdate("block state is outside the legacy 16-bit range"));
    }
    Ok(BlockState(state as u16))
}

/// Decode clientbound S23 (0x23): one block position and its new global block-state ID.
pub fn decode_block_change(payload: &[u8]) -> Result<BlockChange, ProtocolError> {
    let mut cursor = PacketCursor::new(payload);
    let packed_position = cursor.read_i64()? as u64;
    let state = read_block_state(&mut cursor)?;
    if !cursor.is_empty() { return Err(ProtocolError::InvalidBlockUpdate("trailing bytes in single block change")); }
    Ok(BlockChange { position: decode_packed_position(packed_position), state })
}

/// Decode clientbound S22 (0x22): chunk coordinates and short-packed block changes.
pub fn decode_multi_block_change(payload: &[u8]) -> Result<MultiBlockChange, ProtocolError> {
    let mut cursor = PacketCursor::new(payload);
    let chunk_x = cursor.read_i32()?;
    let chunk_z = cursor.read_i32()?;
    let count = cursor.read_varint()?;
    if count < 0 || count > 4096 {
        return Err(ProtocolError::InvalidBlockUpdate("record count must be between 0 and 4096"));
    }
    let base_x = chunk_x.checked_mul(16).ok_or(ProtocolError::InvalidBlockUpdate("chunk X coordinate overflow"))?;
    let base_z = chunk_z.checked_mul(16).ok_or(ProtocolError::InvalidBlockUpdate("chunk Z coordinate overflow"))?;
    let mut records = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let packed = cursor.read_u16()?;
        let local_x = ((packed >> 12) & 0x0f) as i32;
        let local_z = ((packed >> 8) & 0x0f) as i32;
        let y = (packed & 0xff) as i32;
        let state = read_block_state(&mut cursor)?;
        records.push(MultiBlockChangeRecord {
            position: BlockPos::new(base_x + local_x, y, base_z + local_z),
            state,
        });
    }
    if !cursor.is_empty() { return Err(ProtocolError::InvalidBlockUpdate("trailing bytes in multi block change")); }
    Ok(MultiBlockChange { chunk: ChunkPos::new(chunk_x, chunk_z), records })
}

/// Decode clientbound S26 (0x26). The packet stores all column headers first,
/// followed by each complete column data blob in order.
pub fn decode_map_chunk_bulk(payload: &[u8]) -> Result<MapChunkBulkData, ProtocolError> {
    let mut cursor = PacketCursor::new(payload);
    let sky_raw = cursor.read_u8()?;
    if sky_raw > 1 { return Err(ProtocolError::InvalidChunkData("invalid bulk skylight flag")); }
    let sky_light_sent = sky_raw != 0;
    let count = cursor.read_varint()?;
    if count < 0 || count > 1024 {
        return Err(ProtocolError::InvalidChunkData("bulk column count must be between 0 and 1024"));
    }
    let mut headers = Vec::with_capacity(count as usize);
    for _ in 0..count {
        headers.push((cursor.read_i32()?, cursor.read_i32()?, cursor.read_u16()?));
    }
    let mut columns = Vec::with_capacity(count as usize);
    for (x, z, bitmap) in headers {
        let section_count = bitmap.count_ones() as usize;
        let bytes_per_section = 8192 + 2048 + if sky_light_sent { 2048 } else { 0 };
        let data_length = section_count.checked_mul(bytes_per_section)
            .and_then(|n| n.checked_add(256))
            .ok_or(ProtocolError::InvalidChunkData("bulk chunk data length overflow"))?;
        let data = cursor.read_bytes(data_length)?;
        let mut single = Vec::with_capacity(16 + data_length);
        single.extend_from_slice(&x.to_be_bytes());
        single.extend_from_slice(&z.to_be_bytes());
        single.push(1);
        single.extend_from_slice(&bitmap.to_be_bytes());
        write_varint(data_length as i32, &mut single);
        single.extend_from_slice(data);
        columns.push(decode_chunk_data_packet(&single, sky_light_sent)?);
    }
    if !cursor.is_empty() { return Err(ProtocolError::InvalidChunkData("trailing bytes in map chunk bulk")); }
    Ok(MapChunkBulkData { sky_light_sent, columns })
}

/// Decode a protocol-47 Chunk Data packet payload (packet ID 0x21 excluded).
/// `has_sky_light` is true only for dimensions that have skylight.
/// Decode a protocol-47 Chunk Data packet payload (packet ID 0x21 excluded).
/// `has_sky_light` must match the current dimension.
pub fn decode_chunk_data_packet(payload: &[u8], has_sky_light: bool) -> Result<ChunkColumnData, ProtocolError> {
    let mut packet = PacketCursor::new(payload);
    let x = packet.read_i32()?;
    let z = packet.read_i32()?;
    let ground_up_raw = packet.read_u8()?;
    if ground_up_raw > 1 { return Err(ProtocolError::InvalidChunkData("invalid ground-up flag")); }
    let ground_up = ground_up_raw != 0;
    let primary_bitmap = packet.read_u16()?;
    let data_length = packet.read_varint()?;
    if data_length < 0 { return Err(ProtocolError::InvalidChunkData("negative chunk data length")); }
    let data_length = data_length as usize;
    let data = packet.read_bytes(data_length)?;
    if !packet.is_empty() { return Err(ProtocolError::InvalidChunkData("trailing packet bytes")); }
    if ground_up && primary_bitmap == 0 && data_length == 0 {
        return Ok(ChunkColumnData { position: ChunkPos::new(x, z), ground_up, unload: true, sections: std::array::from_fn(|_| None), biomes: None });
    }
    let section_count = primary_bitmap.count_ones() as usize;
    let bytes_per_section = 8192 + 2048 + if has_sky_light { 2048 } else { 0 };
    let expected = section_count.checked_mul(bytes_per_section)
        .and_then(|n| n.checked_add(if ground_up { 256 } else { 0 }))
        .ok_or(ProtocolError::InvalidChunkData("chunk data length overflow"))?;
    if data_length != expected { return Err(ProtocolError::InvalidChunkData("chunk data length does not match section mask")); }
    let mut cursor = PacketCursor::new(data);
    let mut block_arrays: [Option<Vec<u16>>; 16] = std::array::from_fn(|_| None);
    let mut block_lights: [Option<Vec<u8>>; 16] = std::array::from_fn(|_| None);
    let mut sky_lights: [Option<Vec<u8>>; 16] = std::array::from_fn(|_| None);
    // Protocol 47 groups arrays by type: all blocks, then all block light, then all skylight.
    for section_y in 0..16 {
        if primary_bitmap & (1 << section_y) == 0 { continue; }
        let raw = cursor.read_bytes(8192)?;
        block_arrays[section_y] = Some(raw.chunks_exact(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]])).collect());
    }
    for section_y in 0..16 {
        if primary_bitmap & (1 << section_y) == 0 { continue; }
        block_lights[section_y] = Some(cursor.read_exact_bytes(2048)?);
    }
    if has_sky_light {
        for section_y in 0..16 {
            if primary_bitmap & (1 << section_y) == 0 { continue; }
            sky_lights[section_y] = Some(cursor.read_exact_bytes(2048)?);
        }
    }
    let biomes = if ground_up {
        let mut values = [0_u8; 256];
        values.copy_from_slice(cursor.read_bytes(256)?);
        Some(values)
    } else { None };
    if !cursor.is_empty() { return Err(ProtocolError::InvalidChunkData("unconsumed chunk data")); }
    let mut sections: [Option<ChunkSectionData>; 16] = std::array::from_fn(|_| None);
    for section_y in 0..16 {
        if let Some(blocks) = block_arrays[section_y].take() {
            sections[section_y] = Some(ChunkSectionData {
                blocks,
                block_light: block_lights[section_y].take().unwrap_or_default(),
                sky_light: sky_lights[section_y].take(),
            });
        }
    }
    Ok(ChunkColumnData { position: ChunkPos::new(x, z), ground_up, unload: false, sections, biomes })
}

/// Result of a successful offline-mode login. This is a connection in the Play state
/// only after the server has sent Login Success; the game loop is built on top of it.
pub struct LoginSession {
    pub identity: LoginSuccess,
    pub stream: PacketStream<TcpStream>,
}

/// Connect to a Minecraft Java server and perform offline-mode login.
/// Online-mode encryption is deliberately rejected rather than bypassed.
pub fn login_offline(host: &str, port: u16, username: &str, timeout: Duration) -> Result<LoginSession, ProtocolError> {
    let login_start = encode_login_start(username)?;
    let mut addresses = (host, port).to_socket_addrs()?;
    let address = addresses.next().ok_or_else(|| ProtocolError::InvalidAddress(format!("{host}:{port}")))?;
    let mut socket = TcpStream::connect_timeout(&address, timeout)?;
    socket.set_nodelay(true)?;
    socket.set_read_timeout(Some(timeout))?;
    socket.set_write_timeout(Some(timeout))?;
    socket.write_all(&handshake_packet(host, port, 2))?;
    socket.write_all(&login_start)?;
    socket.flush()?;

    let mut stream = PacketStream::new(socket);
    for _ in 0..64 {
        let packet = stream.read_packet()?;
        let mut cursor = PacketCursor::new(&packet);
        let packet_id = cursor.read_varint()?;
        let payload = cursor.read_bytes(cursor.remaining())?;
        match packet_id {
            0x00 => {
                let disconnect = decode_login_disconnect(payload)?;
                return Err(ProtocolError::LoginRejected(disconnect.reason_json));
            }
            0x01 => return Err(ProtocolError::OnlineModeRequired),
            0x02 => {
                let identity = decode_login_success(payload)?;
                return Ok(LoginSession { identity, stream });
            }
            0x03 => {
                let threshold = decode_set_compression(payload)?;
                stream.set_compression_threshold(Some(threshold));
            }
            other => return Err(ProtocolError::InvalidLoginPacket(other)),
        }
    }
    Err(ProtocolError::InvalidLoginPacket(-1))
}

/// Stateful packet framing for a single Minecraft TCP connection.
pub struct PacketStream<S> {
    inner: S,
    compression_threshold: Option<usize>,
    max_frame_size: usize,
    max_uncompressed_size: usize,
}

impl<S: Read + Write> PacketStream<S> {
    pub fn new(inner: S) -> Self {
        Self { inner, compression_threshold: None, max_frame_size: DEFAULT_MAX_FRAME_SIZE, max_uncompressed_size: DEFAULT_MAX_FRAME_SIZE }
    }
    pub fn with_limits(inner: S, max_frame_size: usize, max_uncompressed_size: usize) -> Self {
        Self { inner, compression_threshold: None, max_frame_size, max_uncompressed_size }
    }
    pub fn set_compression_threshold(&mut self, threshold: Option<usize>) { self.compression_threshold = threshold; }
    pub fn compression_threshold(&self) -> Option<usize> { self.compression_threshold }
    pub fn read_packet(&mut self) -> Result<Vec<u8>, ProtocolError> {
        match self.compression_threshold {
            Some(_) => read_compressed_frame(&mut self.inner, self.max_uncompressed_size),
            None => read_frame(&mut self.inner, self.max_frame_size),
        }
    }
    pub fn write_packet(&mut self, packet_id: i32, payload: &[u8]) -> Result<(), ProtocolError> {
        let frame = match self.compression_threshold {
            Some(threshold) => encode_compressed_packet(packet_id, payload, threshold)?,
            None => encode_packet(packet_id, payload),
        };
        self.inner.write_all(&frame)?;
        self.inner.flush()?;
        Ok(())
    }
    pub fn into_inner(self) -> S { self.inner }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RespawnInfo {
    pub dimension: i32,
    pub difficulty: u8,
    pub game_mode: u8,
    pub level_type: String,
}

pub fn decode_respawn(payload: &[u8]) -> Result<RespawnInfo, ProtocolError> {
    let mut cursor = PacketCursor::new(payload);
    let info = RespawnInfo {
        dimension: cursor.read_i32()?,
        difficulty: cursor.read_u8()?,
        game_mode: cursor.read_u8()?,
        level_type: cursor.read_string(16)?,
    };
    if !cursor.is_empty() { return Err(ProtocolError::InvalidBlockUpdate("trailing bytes in Respawn")); }
    Ok(info)
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlayEvent {
    JoinGame(JoinGameInfo),
    ChatMessage(String),
    TimeUpdate(TimeUpdate),
    SpawnPosition(BlockPos),
    Respawn(RespawnInfo),
    PlayerPositionLook(PlayerPositionLook),
    Chunk(ChunkColumnData),
    ChunkBulk(MapChunkBulkData),
    BlockChange(BlockChange),
    MultiBlockChange(MultiBlockChange),
    HeldItemChange(u8),
    SetSlot(SetSlotData),
    WindowItems(WindowItemsData),
    OpenWindow(OpenWindowData),
    CloseWindow(CloseWindowData),
    WindowProperty(WindowPropertyData),
    ConfirmTransaction(ConfirmTransactionData),
    KeepAliveResponded(i32),
    Unknown { packet_id: i32, payload: Vec<u8> },
}

/// A logged-in protocol-47 play connection. Keep Alive packets are answered automatically.
pub struct PlaySession {
    pub identity: LoginSuccess,
    pub stream: PacketStream<TcpStream>,
    pub entity_id: i32,
    pub dimension: Option<i8>,
    pub selected_hotbar_slot: u8,
    pub inventory_windows: std::collections::HashMap<i16, Vec<Option<ItemStack>>>,
    pub carried_item: Option<ItemStack>,
    pub open_windows: std::collections::HashMap<u8, OpenWindowData>,
    pub window_properties: std::collections::HashMap<(u8, i16), i16>,
    pub transaction_results: std::collections::HashMap<(u8, i16), bool>,
}

impl LoginSession {
    pub fn into_play(self) -> PlaySession {
        PlaySession { identity: self.identity, stream: self.stream, entity_id: 0, dimension: None, selected_hotbar_slot: 0, inventory_windows: std::collections::HashMap::new(), carried_item: None, open_windows: std::collections::HashMap::new(), window_properties: std::collections::HashMap::new(), transaction_results: std::collections::HashMap::new() }
    }
}

impl PlaySession {
    /// Read and decode one serverbound Play packet. Unknown packet IDs are preserved verbatim.
    pub fn read_event(&mut self) -> Result<PlayEvent, ProtocolError> {
        let packet = self.stream.read_packet()?;
        let mut cursor = PacketCursor::new(&packet);
        let packet_id = cursor.read_varint()?;
        let payload = cursor.read_bytes(cursor.remaining())?;
        match packet_id {
            0x00 => {
                let keep_alive = decode_keep_alive(payload)?;
                let mut response = Vec::new();
                write_varint(keep_alive.id, &mut response);
                self.stream.write_packet(0x00, &response)?;
                Ok(PlayEvent::KeepAliveResponded(keep_alive.id))
            }
            0x01 => {
                let join = decode_join_game(payload)?;
                self.entity_id = join.entity_id;
                self.dimension = Some(join.dimension);
                Ok(PlayEvent::JoinGame(join))
            }
            0x02 => {
                let mut chat = PacketCursor::new(payload);
                let message = chat.read_string(32_767)?;
                if !chat.is_empty() { return Err(ProtocolError::InvalidBlockUpdate("trailing bytes in Chat Message")); }
                Ok(PlayEvent::ChatMessage(message))
            }
            0x03 => Ok(PlayEvent::TimeUpdate(decode_time_update(payload)?)),
            0x05 => Ok(PlayEvent::SpawnPosition(decode_spawn_position(payload)?)),
            0x07 => {
                let respawn = decode_respawn(payload)?;
                self.dimension = Some(respawn.dimension as i8);
                Ok(PlayEvent::Respawn(respawn))
            }
            0x08 => Ok(PlayEvent::PlayerPositionLook(decode_player_position_look(payload)?)),
            0x2F => {
                let data = decode_set_slot(payload)?;
                if data.window_id == -1 && data.slot == -1 {
                    self.carried_item = data.item.clone();
                } else if data.slot >= 0 && data.slot < 4096 {
                    let slots = self.inventory_windows.entry(data.window_id as i16).or_default();
                    let index = data.slot as usize;
                    if slots.len() <= index { slots.resize(index + 1, None); }
                    slots[index] = data.item.clone();
                }
                Ok(PlayEvent::SetSlot(data))
            }
            0x30 => {
                let data = decode_window_items(payload)?;
                self.inventory_windows.insert(data.window_id as i16, data.items.clone());
                Ok(PlayEvent::WindowItems(data))
            }
            0x2D => {
                let data = decode_open_window(payload)?;
                self.inventory_windows.entry(data.window_id as i16).or_default();
                self.open_windows.insert(data.window_id, data.clone());
                Ok(PlayEvent::OpenWindow(data))
            }
            0x2E => {
                let data = decode_close_window(payload)?;
                self.open_windows.remove(&data.window_id);
                self.inventory_windows.remove(&(data.window_id as i16));
                self.window_properties.retain(|(window, _), _| *window != data.window_id);
                self.carried_item = None;
                Ok(PlayEvent::CloseWindow(data))
            }
            0x31 => {
                let data = decode_window_property(payload)?;
                self.window_properties.insert((data.window_id, data.property), data.value);
                Ok(PlayEvent::WindowProperty(data))
            }
            0x32 => {
                let data = decode_confirm_transaction(payload)?;
                self.transaction_results.insert((data.window_id, data.action_number), data.accepted);
                if !data.accepted {
                    let mut response = Vec::with_capacity(4);
                    response.push(data.window_id);
                    response.extend_from_slice(&data.action_number.to_be_bytes());
                    response.push(1);
                    self.stream.write_packet(0x0F, &response)?;
                }
                Ok(PlayEvent::ConfirmTransaction(data))
            }
            0x09 => {
                let slot = decode_held_item_change(payload)?;
                self.selected_hotbar_slot = slot;
                Ok(PlayEvent::HeldItemChange(slot))
            }
            0x21 => {
                let dimension = self.dimension.ok_or(ProtocolError::MissingJoinGame)?;
                Ok(PlayEvent::Chunk(decode_chunk_data_packet(payload, dimension == 0)?))
            }
            0x22 => Ok(PlayEvent::MultiBlockChange(decode_multi_block_change(payload)?)),
            0x23 => Ok(PlayEvent::BlockChange(decode_block_change(payload)?)),
            0x26 => Ok(PlayEvent::ChunkBulk(decode_map_chunk_bulk(payload)?)),
            other => Ok(PlayEvent::Unknown { packet_id: other, payload: payload.to_vec() }),
        }
    }

    /// Send a full position + rotation update (C06).
    pub fn send_player_movement(&mut self, movement: PlayerMove, on_ground: bool) -> Result<(), ProtocolError> {
        self.stream.write_packet(0x06, &player_position_look_payload(movement, on_ground))
    }

    /// Send a position-only update (C04).
    pub fn send_player_position(&mut self, x: f64, y: f64, z: f64, on_ground: bool) -> Result<(), ProtocolError> {
        let mut payload = Vec::with_capacity(25);
        payload.extend_from_slice(&x.to_be_bytes());
        payload.extend_from_slice(&y.to_be_bytes());
        payload.extend_from_slice(&z.to_be_bytes());
        payload.push(u8::from(on_ground));
        self.stream.write_packet(0x04, &payload)
    }

    /// Send a rotation-only update (C05).
    pub fn send_player_look(&mut self, yaw: f32, pitch: f32, on_ground: bool) -> Result<(), ProtocolError> {
        let mut payload = Vec::with_capacity(9);
        payload.extend_from_slice(&yaw.to_be_bytes());
        payload.extend_from_slice(&pitch.to_be_bytes());
        payload.push(u8::from(on_ground));
        self.stream.write_packet(0x05, &payload)
    }

    pub fn send_player_ground(&mut self, on_ground: bool) -> Result<(), ProtocolError> {
        self.stream.write_packet(0x03, &[u8::from(on_ground)])
    }

    pub fn send_digging(&mut self, status: DiggingStatus, position: BlockPos, face: BlockFace) -> Result<(), ProtocolError> {
        let mut payload = Vec::with_capacity(10);
        write_varint(status as i32, &mut payload);
        payload.extend_from_slice(&encode_packed_position(position).to_be_bytes());
        payload.push(face as u8);
        self.stream.write_packet(0x07, &payload)
    }

    pub fn send_held_item_change(&mut self, slot: u8) -> Result<(), ProtocolError> {
        if slot > 8 { return Err(ProtocolError::InvalidHotbarSlot(slot as i32)); }
        self.stream.write_packet(0x09, &(slot as i16).to_be_bytes())?;
        self.selected_hotbar_slot = slot;
        Ok(())
    }

    /// Send C08 block placement with the server-synchronized selected hotbar stack.
    pub fn send_block_placement(&mut self, position: BlockPos, face: u8, cursor: [f32; 3]) -> Result<(), ProtocolError> {
        let slot_index = 36 + self.selected_hotbar_slot as usize;
        let held = self.inventory_windows.get(&0)
            .and_then(|slots| slots.get(slot_index))
            .and_then(Option::as_ref)
            .cloned();
        let payload = block_placement_payload(position, face, held.as_ref(), cursor)?;
        self.stream.write_packet(0x08, &payload)
    }

    /// Send the C08 item-use variant (no block face targeted).
    pub fn send_use_item(&mut self) -> Result<(), ProtocolError> {
        self.send_block_placement(BlockPos::new(-1, -1, -1), 255, [0.0, 0.0, 0.0])
    }

    /// Send a chat message to the server (C01).
    pub fn send_chat_message(&mut self, message: &str) -> Result<(), ProtocolError> {
        if message.chars().count() > 100 { return Err(ProtocolError::InvalidChatMessage); }
        let mut payload = Vec::with_capacity(message.len() + 5);
        write_string(message, &mut payload);
        self.stream.write_packet(0x01, &payload)
    }
    pub fn send_animation(&mut self) -> Result<(), ProtocolError> {
        self.stream.write_packet(0x0A, &[])
    }

    /// Read the next event and apply authoritative world changes before returning it.
    pub fn read_world_event(&mut self, world: &mut World) -> Result<PlayEvent, ProtocolError> {
        let event = self.read_event()?;
        let result = match &event {
            PlayEvent::Chunk(chunk) => world.apply_chunk_data(chunk.clone()),
            PlayEvent::ChunkBulk(bulk) => world.apply_chunk_batch(bulk.columns.clone()),
            PlayEvent::BlockChange(change) => {
                world.apply_block_change(change.position, change.state);
                return Ok(event);
            }
            PlayEvent::MultiBlockChange(change) => {
                world.apply_block_changes(change.records.iter().map(|record| (record.position, record.state)));
                return Ok(event);
            }
            _ => return Ok(event),
        };
        result.map_err(|error| ProtocolError::WorldUpdate(error.to_string()))?;
        Ok(event)
    }
}

/// Read one complete Minecraft packet frame from a stream.
pub fn read_frame<R: Read>(reader: &mut R, maximum: usize) -> Result<Vec<u8>, ProtocolError> {
    let length = read_varint(reader)?;
    if length < 0 { return Err(ProtocolError::InvalidLength(length)); }
    let length = length as usize;
    if length > maximum { return Err(ProtocolError::FrameTooLarge { length, maximum }); }
    let mut frame = vec![0_u8; length];
    reader.read_exact(&mut frame)?;
    Ok(frame)
}

/// Write a complete packet frame to a stream.
pub fn write_frame<W: Write>(writer: &mut W, packet_id: i32, payload: &[u8]) -> Result<(), ProtocolError> {
    writer.write_all(&encode_packet(packet_id, payload))?;
    writer.flush()?;
    Ok(())
}

/// Build the protocol-47 handshake packet.
pub fn handshake_packet(server_host: &str, server_port: u16, next_state: i32) -> Vec<u8> {
    let mut payload = Vec::with_capacity(server_host.len() + 16);
    write_varint(PROTOCOL_VERSION, &mut payload);
    write_string(server_host, &mut payload);
    payload.extend_from_slice(&server_port.to_be_bytes());
    write_varint(next_state, &mut payload);
    encode_packet(0x00, &payload)
}

/// Build the status request packet (packet ID 0, empty payload).
pub fn status_request_packet() -> Vec<u8> { encode_packet(0x00, &[]) }

pub fn write_string(value: &str, output: &mut Vec<u8>) {
    write_varint(value.len() as i32, output);
    output.extend_from_slice(value.as_bytes());
}

/// Query status while preserving the server hostname and port in the handshake.
pub fn query_status_host(host: &str, port: u16) -> Result<String, ProtocolError> {
    let mut addresses = (host, port).to_socket_addrs()?;
    let address = addresses.next().ok_or_else(|| ProtocolError::InvalidAddress(format!("{host}:{port}")))?;
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(8))?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(8)))?;
    stream.set_write_timeout(Some(Duration::from_secs(8)))?;
    stream.write_all(&handshake_packet(host, port, 1))?;
    stream.write_all(&status_request_packet())?;
    stream.flush()?;
    let frame = read_frame(&mut stream, DEFAULT_MAX_FRAME_SIZE)?;
    let mut cursor = PacketCursor::new(&frame);
    let packet_id = cursor.read_varint()?;
    if packet_id != 0x00 {
        return Err(ProtocolError::UnexpectedPacket { expected: 0x00, actual: packet_id });
    }
    cursor.read_string(DEFAULT_MAX_FRAME_SIZE)
}

/// Parse a host and port, accepting bracketed IPv6 addresses.
pub fn parse_host_port(input: &str) -> Result<(String, u16), ProtocolError> {
    let (host, port_text) = if let Some(rest) = input.strip_prefix('[') {
        let (host, suffix) = rest.split_once(']').ok_or_else(|| ProtocolError::InvalidAddress(input.to_owned()))?;
        let port_text = suffix.strip_prefix(':').ok_or_else(|| ProtocolError::InvalidAddress(input.to_owned()))?;
        (host, port_text)
    } else {
        input.rsplit_once(':').ok_or_else(|| ProtocolError::InvalidAddress(input.to_owned()))?
    };
    let port = port_text.parse::<u16>().map_err(|_| ProtocolError::InvalidAddress(input.to_owned()))?;
    if host.is_empty() || port == 0 { return Err(ProtocolError::InvalidAddress(input.to_owned())); }
    Ok((host.to_owned(), port))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn varint_round_trip_boundaries() {
        for value in [0, 1, 127, 128, 255, 25565, i32::MAX, -1, i32::MIN] {
            let mut encoded = Vec::new();
            write_varint(value, &mut encoded);
            assert_eq!(read_varint(&mut Cursor::new(encoded)).unwrap(), value);
        }
    }
    #[test]
    fn rejects_six_byte_varint() {
        let input = [0x80, 0x80, 0x80, 0x80, 0x80, 0x00];
        assert!(matches!(read_varint(&mut Cursor::new(input)), Err(ProtocolError::VarIntTooLong)));
    }
    #[test]
    fn packet_frame_round_trip() {
        let frame = encode_packet(0x23, &[1, 2, 3, 4]);
        let body = read_frame(&mut Cursor::new(frame), 64).unwrap();
        let mut cursor = PacketCursor::new(&body);
        assert_eq!(cursor.read_varint().unwrap(), 0x23);
        assert_eq!(cursor.read_exact_bytes(4).unwrap(), [1, 2, 3, 4]);
        assert!(cursor.is_empty());
    }
    #[test]
    fn handshake_has_protocol_47_and_status_state() {
        let packet = handshake_packet("example.org", 25565, 1);
        let mut cursor = PacketCursor::new(&packet);
        let length = cursor.read_varint().unwrap() as usize;
        assert_eq!(length, cursor.remaining());
        assert_eq!(cursor.read_varint().unwrap(), 0);
        assert_eq!(cursor.read_varint().unwrap(), 47);
        assert_eq!(cursor.read_string(255).unwrap(), "example.org");
        assert_eq!(cursor.read_u16().unwrap(), 25565);
        assert_eq!(cursor.read_varint().unwrap(), 1);
    }
    #[test]
    fn strings_are_bounded_and_utf8_checked() {
        let mut encoded = Vec::new();
        write_string("Привет", &mut encoded);
        assert_eq!(PacketCursor::new(&encoded).read_string(32).unwrap(), "Привет");
        assert!(matches!(PacketCursor::new(&encoded).read_string(2), Err(ProtocolError::StringTooLong { .. })));
        let bad = vec![1, 0xff];
        assert!(matches!(PacketCursor::new(&bad).read_string(4), Err(ProtocolError::InvalidUtf8)));
    }
    #[test]
    fn compressed_and_uncompressed_frames_round_trip() {
        let payload = vec![b'R'; 1024];
        for threshold in [0, 1, 4096] {
            let frame = encode_compressed_packet(0x21, &payload, threshold).unwrap();
            let decoded = read_compressed_frame(&mut Cursor::new(frame), 4096).unwrap();
            let mut cursor = PacketCursor::new(&decoded);
            assert_eq!(cursor.read_varint().unwrap(), 0x21);
            assert_eq!(cursor.read_exact_bytes(1024).unwrap(), payload);
        }
    }
    fn make_chunk_payload(ground_up: bool, bitmap: u16, sky: bool) -> Vec<u8> {
        let mut data = Vec::new();
        for section_y in 0..16 {
            if bitmap & (1 << section_y) == 0 { continue; }
            let mut blocks = vec![0_u8; 8192];
            if section_y == 2 {
                let index = (1 << 8) | (2 << 4) | 3;
                blocks[index * 2..index * 2 + 2].copy_from_slice(&0x1234_u16.to_le_bytes());
            }
            data.extend_from_slice(&blocks);
        }
        for section_y in 0..16 {
            if bitmap & (1 << section_y) != 0 { data.extend(std::iter::repeat(0xAA).take(2048)); }
        }
        if sky {
            for section_y in 0..16 {
                if bitmap & (1 << section_y) != 0 { data.extend(std::iter::repeat(0xBB).take(2048)); }
            }
        }
        if ground_up { data.extend(std::iter::repeat(0xCC).take(256)); }
        let mut out = Vec::new();
        out.extend_from_slice(&(-3_i32).to_be_bytes());
        out.extend_from_slice(&7_i32.to_be_bytes());
        out.push(u8::from(ground_up));
        out.extend_from_slice(&bitmap.to_be_bytes());
        write_varint(data.len() as i32, &mut out);
        out.extend_from_slice(&data);
        out
    }
    #[test]
    fn decodes_chunk_sections_lights_and_biomes() {
        let parsed = decode_chunk_data_packet(&make_chunk_payload(true, (1 << 2) | (1 << 4), true), true).unwrap();
        assert_eq!(parsed.position, ChunkPos::new(-3, 7));
        let section = parsed.sections[2].as_ref().unwrap();
        assert_eq!(section.blocks[(1 << 8) | (2 << 4) | 3], 0x1234);
        assert_eq!(section.block_light[0], 0xAA);
        assert_eq!(section.sky_light.as_ref().unwrap()[0], 0xBB);
        assert_eq!(parsed.sections[4].as_ref().unwrap().block_light[0], 0xAA);
        assert_eq!(parsed.biomes.unwrap()[255], 0xCC);
    }

    #[test]
    fn decodes_nether_chunk_without_sky_light() {
        let parsed = decode_chunk_data_packet(&make_chunk_payload(false, 1, false), false).unwrap();
        assert!(!parsed.ground_up);
        assert!(parsed.biomes.is_none());
        assert!(parsed.sections[0].as_ref().unwrap().sky_light.is_none());
    }

    #[test]
    fn rejects_chunk_with_wrong_dimension_length_or_trailing_packet_bytes() {
        let valid = make_chunk_payload(true, 1, true);
        assert!(matches!(decode_chunk_data_packet(&valid, false), Err(ProtocolError::InvalidChunkData(_))));
        let mut with_trailing = valid;
        with_trailing.push(0);
        assert!(matches!(decode_chunk_data_packet(&with_trailing, true), Err(ProtocolError::InvalidChunkData(_))));
    }
    #[test]
    fn recognizes_empty_ground_up_packet_as_chunk_unload() {
        let mut packet = Vec::new();
        packet.extend_from_slice(&4_i32.to_be_bytes());
        packet.extend_from_slice(&(-2_i32).to_be_bytes());
        packet.push(1);
        packet.extend_from_slice(&0_u16.to_be_bytes());
        write_varint(0, &mut packet);
        let parsed = decode_chunk_data_packet(&packet, true).unwrap();
        assert!(parsed.unload);
        assert_eq!(parsed.position, ChunkPos::new(4, -2));
    }
    fn encode_position(x: i32, y: i32, z: i32) -> i64 {
        (((x as i64 & 0x03ff_ffff) << 38) | ((y as i64 & 0x0fff) << 26) | (z as i64 & 0x03ff_ffff)) as i64
    }

    #[test]
    fn decodes_single_block_change_with_negative_coordinates() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&encode_position(-12345, 64, -54321).to_be_bytes());
        write_varint(0x123, &mut payload);
        let change = decode_block_change(&payload).unwrap();
        assert_eq!(change.position, BlockPos::new(-12345, 64, -54321));
        assert_eq!(change.state, BlockState(0x123));
    }

    #[test]
    fn decodes_multi_block_change_packed_records() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&(-1_i32).to_be_bytes());
        payload.extend_from_slice(&3_i32.to_be_bytes());
        write_varint(2, &mut payload);
        let first = (15_u16 << 12) | (7_u16 << 8) | 255_u16;
        payload.extend_from_slice(&first.to_be_bytes());
        write_varint(0x45, &mut payload);
        let second = (2_u16 << 12) | (4_u16 << 8) | 10_u16;
        payload.extend_from_slice(&second.to_be_bytes());
        write_varint(0x91, &mut payload);
        let decoded = decode_multi_block_change(&payload).unwrap();
        assert_eq!(decoded.chunk, ChunkPos::new(-1, 3));
        assert_eq!(decoded.records.len(), 2);
        assert_eq!(decoded.records[0].position, BlockPos::new(-1, 255, 55));
        assert_eq!(decoded.records[0].state, BlockState(0x45));
        assert_eq!(decoded.records[1].position, BlockPos::new(-14, 10, 52));
        assert_eq!(decoded.records[1].state, BlockState(0x91));
    }

    #[test]
    fn rejects_invalid_multi_block_change_count_and_trailing_bytes() {
        let mut too_many = Vec::new();
        too_many.extend_from_slice(&0_i32.to_be_bytes());
        too_many.extend_from_slice(&0_i32.to_be_bytes());
        write_varint(4097, &mut too_many);
        assert!(matches!(decode_multi_block_change(&too_many), Err(ProtocolError::InvalidBlockUpdate(_))));
        let mut trailing = Vec::new();
        trailing.extend_from_slice(&0_i32.to_be_bytes());
        trailing.extend_from_slice(&0_i32.to_be_bytes());
        write_varint(0, &mut trailing);
        trailing.push(1);
        assert!(matches!(decode_multi_block_change(&trailing), Err(ProtocolError::InvalidBlockUpdate(_))));
    }
    fn append_bulk_column(data: &mut Vec<u8>, marker: u16, sky: bool) {
        let mut blocks = vec![0_u8; 8192];
        blocks[0..2].copy_from_slice(&marker.to_le_bytes());
        data.extend_from_slice(&blocks);
        data.extend(std::iter::repeat(0xA1).take(2048));
        if sky { data.extend(std::iter::repeat(0xB2).take(2048)); }
        data.extend(std::iter::repeat(0xC3).take(256));
    }

    #[test]
    fn decodes_map_chunk_bulk_headers_and_column_blobs() {
        let mut packet = vec![1];
        write_varint(2, &mut packet);
        packet.extend_from_slice(&(-1_i32).to_be_bytes());
        packet.extend_from_slice(&2_i32.to_be_bytes());
        packet.extend_from_slice(&1_u16.to_be_bytes());
        packet.extend_from_slice(&3_i32.to_be_bytes());
        packet.extend_from_slice(&(-4_i32).to_be_bytes());
        packet.extend_from_slice(&2_u16.to_be_bytes());
        append_bulk_column(&mut packet, 0x1234, true);
        append_bulk_column(&mut packet, 0x5678, true);
        let decoded = decode_map_chunk_bulk(&packet).unwrap();
        assert!(decoded.sky_light_sent);
        assert_eq!(decoded.columns.len(), 2);
        assert_eq!(decoded.columns[0].position, ChunkPos::new(-1, 2));
        assert_eq!(decoded.columns[0].sections[0].as_ref().unwrap().blocks[0], 0x1234);
        assert_eq!(decoded.columns[1].position, ChunkPos::new(3, -4));
        assert_eq!(decoded.columns[1].sections[1].as_ref().unwrap().blocks[0], 0x5678);
        assert_eq!(decoded.columns[1].sections[1].as_ref().unwrap().block_light[0], 0xA1);
        assert_eq!(decoded.columns[1].sections[1].as_ref().unwrap().sky_light.as_ref().unwrap()[0], 0xB2);
        assert_eq!(decoded.columns[1].biomes.unwrap()[0], 0xC3);
    }

    #[test]
    fn rejects_invalid_bulk_count_and_truncated_column() {
        let mut too_many = vec![0];
        write_varint(1025, &mut too_many);
        assert!(matches!(decode_map_chunk_bulk(&too_many), Err(ProtocolError::InvalidChunkData(_))));
        let mut truncated = vec![0];
        write_varint(1, &mut truncated);
        truncated.extend_from_slice(&0_i32.to_be_bytes());
        truncated.extend_from_slice(&0_i32.to_be_bytes());
        truncated.extend_from_slice(&1_u16.to_be_bytes());
        assert!(matches!(decode_map_chunk_bulk(&truncated), Err(ProtocolError::UnexpectedEof)));
    }
    #[test]
    fn decodes_join_game_dimension_hardcore_and_sky_light() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&42_i32.to_be_bytes());
        payload.push(9); payload.push(0); payload.push(2); payload.push(20);
        write_string("default", &mut payload);
        payload.push(1);
        let join = decode_join_game(&payload).unwrap();
        assert_eq!(join.entity_id, 42);
        assert_eq!(join.game_mode, 1);
        assert!(join.hardcore_mode);
        assert_eq!(join.dimension, 0);
        assert!(join.has_sky_light());
        assert_eq!(join.level_type, "default");
        assert!(join.reduced_debug_info);
        payload[5] = 1;
        assert!(!decode_join_game(&payload).unwrap().has_sky_light());
    }

    #[test]
    fn keep_alive_round_trip_uses_varint_packet() {
        let frame = encode_keep_alive_response(300);
        let body = read_frame(&mut Cursor::new(frame), 64).unwrap();
        let mut cursor = PacketCursor::new(&body);
        assert_eq!(cursor.read_varint().unwrap(), 0);
        assert_eq!(decode_keep_alive(&body[1..]).unwrap(), KeepAlive { id: 300 });
    }

    #[test]
    fn player_position_and_look_packet_encoding() {
        let update = PlayerMove { x: 12.5, y: 64.0, z: -3.25, yaw: 90.0, pitch: -15.0 };
        let frame = encode_player_position_look(update, true);
        let body = read_frame(&mut Cursor::new(frame), 64).unwrap();
        let mut cursor = PacketCursor::new(&body);
        assert_eq!(cursor.read_varint().unwrap(), 0x06);
        assert_eq!(cursor.read_f64().unwrap(), 12.5);
        assert_eq!(cursor.read_f64().unwrap(), 64.0);
        assert_eq!(cursor.read_f64().unwrap(), -3.25);
        assert_eq!(cursor.read_f32().unwrap(), 90.0);
        assert_eq!(cursor.read_f32().unwrap(), -15.0);
        assert_eq!(cursor.read_u8().unwrap(), 1);
        assert!(cursor.is_empty());
    }

    #[test]
    fn decodes_time_and_spawn_position() {
        let mut time = Vec::new();
        time.extend_from_slice(&123_i64.to_be_bytes());
        time.extend_from_slice(&(-6000_i64).to_be_bytes());
        assert_eq!(decode_time_update(&time).unwrap(), TimeUpdate { world_age: 123, time_of_day: -6000 });
        let mut spawn = Vec::new();
        spawn.extend_from_slice(&encode_position(-22, 70, 41).to_be_bytes());
        assert_eq!(decode_spawn_position(&spawn).unwrap(), BlockPos::new(-22, 70, 41));
    }

    #[test]
    fn packet_stream_reads_compressed_and_uncompressed_frames() {
        let frame = encode_compressed_packet(0x2A, &[7, 8, 9], 1).unwrap();
        let mut stream = PacketStream::new(Cursor::new(frame));
        stream.set_compression_threshold(Some(1));
        let packet = stream.read_packet().unwrap();
        let mut cursor = PacketCursor::new(&packet);
        assert_eq!(cursor.read_varint().unwrap(), 0x2A);
        assert_eq!(cursor.read_exact_bytes(3).unwrap(), [7, 8, 9]);
        let frame = encode_packet(0x2B, &[1, 2]);
        let mut stream = PacketStream::new(Cursor::new(frame));
        assert!(stream.compression_threshold().is_none());
        let packet = stream.read_packet().unwrap();
        let mut cursor = PacketCursor::new(&packet);
        assert_eq!(cursor.read_varint().unwrap(), 0x2B);
    }

    #[test]
    fn packet_stream_writes_with_negotiated_compression() {
        let mut stream = PacketStream::new(Cursor::new(Vec::new()));
        stream.set_compression_threshold(Some(1));
        stream.write_packet(0x31, &[b'X'; 64]).unwrap();
        let bytes = stream.into_inner().into_inner();
        let mut reader = PacketStream::new(Cursor::new(bytes));
        reader.set_compression_threshold(Some(1));
        let body = reader.read_packet().unwrap();
        let mut cursor = PacketCursor::new(&body);
        assert_eq!(cursor.read_varint().unwrap(), 0x31);
        assert_eq!(cursor.read_exact_bytes(64).unwrap(), vec![b'X'; 64]);
    }
    #[test]
    fn login_start_success_disconnect_and_compression_codecs() {
        let start = encode_login_start("RustPlayer").unwrap();
        let body = read_frame(&mut Cursor::new(start), 64).unwrap();
        let mut cursor = PacketCursor::new(&body);
        assert_eq!(cursor.read_varint().unwrap(), 0);
        assert_eq!(cursor.read_string(16).unwrap(), "RustPlayer");

        let mut success = Vec::new();
        write_string("12345678-1234-5678-1234-567812345678", &mut success);
        write_string("RustPlayer", &mut success);
        assert_eq!(decode_login_success(&success).unwrap(), LoginSuccess {
            uuid: "12345678-1234-5678-1234-567812345678".into(), username: "RustPlayer".into()
        });

        let mut disconnect = Vec::new();
        write_string(r#"{"text":"Disconnected"}"#, &mut disconnect);
        assert_eq!(decode_login_disconnect(&disconnect).unwrap().reason_json, r#"{"text":"Disconnected"}"#);

        let mut threshold = Vec::new();
        write_varint(256, &mut threshold);
        assert_eq!(decode_set_compression(&threshold).unwrap(), 256);
    }

    #[test]
    fn login_start_rejects_invalid_usernames() {
        assert!(encode_login_start("").is_err());
        assert!(encode_login_start("name that is too long").is_err());
        assert!(encode_login_start("Náme").is_err());
    }
    #[test]
    fn offline_login_completes_compressed_protocol_47_handshake() {
        use std::net::TcpListener;
        use std::thread;
        use std::time::Duration;

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let handshake = read_frame(&mut socket, 256).unwrap();
            let mut cursor = PacketCursor::new(&handshake);
            assert_eq!(cursor.read_varint().unwrap(), 0);
            assert_eq!(cursor.read_varint().unwrap(), 47);
            assert_eq!(cursor.read_string(255).unwrap(), "127.0.0.1");
            assert_eq!(cursor.read_u16().unwrap(), port);
            assert_eq!(cursor.read_varint().unwrap(), 2);
            let login = read_frame(&mut socket, 128).unwrap();
            let mut cursor = PacketCursor::new(&login);
            assert_eq!(cursor.read_varint().unwrap(), 0);
            assert_eq!(cursor.read_string(16).unwrap(), "RustPlayer");

            let mut threshold = Vec::new();
            write_varint(32, &mut threshold);
            socket.write_all(&encode_packet(0x03, &threshold)).unwrap();
            let mut server_stream = PacketStream::new(socket);
            server_stream.set_compression_threshold(Some(32));
            let mut success = Vec::new();
            write_string("12345678123456781234567812345678", &mut success);
            write_string("RustPlayer", &mut success);
            server_stream.write_packet(0x02, &success).unwrap();
        });

        let session = login_offline("127.0.0.1", port, "RustPlayer", Duration::from_secs(3)).unwrap();
        assert_eq!(session.identity.username, "RustPlayer");
        assert_eq!(session.identity.uuid, "12345678123456781234567812345678");
        assert_eq!(session.stream.compression_threshold(), Some(32));
        server.join().unwrap();
    }

    #[test]
    fn offline_login_refuses_online_mode_encryption_instead_of_bypassing_it() {
        use std::net::TcpListener;
        use std::thread;
        use std::time::Duration;

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let _handshake = read_frame(&mut socket, 256).unwrap();
            let _login = read_frame(&mut socket, 128).unwrap();
            socket.write_all(&encode_packet(0x01, &[0, 1, 2])).unwrap();
        });
        let result = login_offline("127.0.0.1", port, "RustPlayer", Duration::from_secs(3));
        assert!(matches!(result, Err(ProtocolError::OnlineModeRequired)));
        server.join().unwrap();
    }

    #[test]
    fn offline_login_returns_server_disconnect_reason() {
        use std::net::TcpListener;
        use std::thread;
        use std::time::Duration;

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let _handshake = read_frame(&mut socket, 256).unwrap();
            let _login = read_frame(&mut socket, 128).unwrap();
            let mut payload = Vec::new();
            write_string(r#"{"text":"Invalid username"}"#, &mut payload);
            socket.write_all(&encode_packet(0x00, &payload)).unwrap();
        });
        let result = login_offline("127.0.0.1", port, "RustPlayer", Duration::from_secs(3));
        assert!(matches!(result, Err(ProtocolError::LoginRejected(reason)) if reason.contains("Invalid username")));
        server.join().unwrap();
    }
    #[test]
    fn play_session_tracks_join_dimension_and_auto_answers_keep_alive() {
        use std::net::TcpListener;
        use std::thread;
        use std::time::Duration;

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let _handshake = read_frame(&mut socket, 256).unwrap();
            let _login = read_frame(&mut socket, 128).unwrap();
            let mut success = Vec::new();
            write_string("12345678123456781234567812345678", &mut success);
            write_string("RustPlayer", &mut success);
            socket.write_all(&encode_packet(0x02, &success)).unwrap();
            let mut join = Vec::new();
            join.extend_from_slice(&77_i32.to_be_bytes());
            join.extend_from_slice(&[1, 0, 2, 20]);
            write_string("default", &mut join);
            join.push(0);
            socket.write_all(&encode_packet(0x01, &join)).unwrap();
            let mut keep_alive = Vec::new();
            write_varint(12345, &mut keep_alive);
            socket.write_all(&encode_packet(0x00, &keep_alive)).unwrap();
            let reply = read_frame(&mut socket, 64).unwrap();
            let mut cursor = PacketCursor::new(&reply);
            assert_eq!(cursor.read_varint().unwrap(), 0x00);
            assert_eq!(cursor.read_varint().unwrap(), 12345);
        });

        let login = login_offline("127.0.0.1", port, "RustPlayer", Duration::from_secs(3)).unwrap();
        let mut play = login.into_play();
        assert!(matches!(play.read_event().unwrap(), PlayEvent::JoinGame(join) if join.entity_id == 77 && join.dimension == 0));
        assert_eq!(play.entity_id, 77);
        assert_eq!(play.dimension, Some(0));
        assert_eq!(play.read_event().unwrap(), PlayEvent::KeepAliveResponded(12345));
        server.join().unwrap();
    }

    #[test]
    fn play_session_requires_join_game_before_chunk_data() {
        use std::net::{TcpListener, TcpStream};
        use std::thread;
        use std::time::Duration;

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let packet = encode_packet(0x21, &make_chunk_payload(true, 1, true));
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.write_all(&packet).unwrap();
        });
        let socket = TcpStream::connect(("127.0.0.1", port)).unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut play = PlaySession {
            identity: LoginSuccess { uuid: "test".into(), username: "Player".into() },
            stream: PacketStream::new(socket),
            entity_id: 0,
            dimension: None,
            selected_hotbar_slot: 0,
            inventory_windows: std::collections::HashMap::new(),
            carried_item: None,
            open_windows: std::collections::HashMap::new(),
            window_properties: std::collections::HashMap::new(),
            transaction_results: std::collections::HashMap::new(),
        };
        assert!(matches!(play.read_event(), Err(ProtocolError::MissingJoinGame)));
        server.join().unwrap();
    }
    #[test]
    fn play_world_pump_applies_server_chunk_data() {
        use std::net::{TcpListener, TcpStream};
        use std::thread;
        use std::time::Duration;

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let join = {
            let mut payload = Vec::new();
            payload.extend_from_slice(&77_i32.to_be_bytes());
            payload.extend_from_slice(&[1, 0, 2, 20]);
            write_string("default", &mut payload);
            payload.push(0);
            encode_packet(0x01, &payload)
        };
        let chunk = encode_packet(0x21, &make_chunk_payload(true, 1 << 2, true));
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.write_all(&join).unwrap();
            socket.write_all(&chunk).unwrap();
        });
        let socket = TcpStream::connect(("127.0.0.1", port)).unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut play = PlaySession {
            identity: LoginSuccess { uuid: "test".into(), username: "Player".into() },
            stream: PacketStream::new(socket),
            entity_id: 0,
            dimension: None,
            selected_hotbar_slot: 0,
            inventory_windows: std::collections::HashMap::new(),
            carried_item: None,
            open_windows: std::collections::HashMap::new(),
            window_properties: std::collections::HashMap::new(),
            transaction_results: std::collections::HashMap::new(),
        };
        let mut world = World::new();
        assert!(matches!(play.read_world_event(&mut world).unwrap(), PlayEvent::JoinGame(_)));
        let event = play.read_world_event(&mut world).unwrap();
        assert!(matches!(event, PlayEvent::Chunk(_)));
        assert_eq!(world.chunk_count(), 1);
        assert_eq!(world.get_block(BlockPos::new(-45, 33, 114)), BlockState(0x1234));
        server.join().unwrap();
    }
    #[test]
    fn movement_digging_and_hotbar_encoders_match_protocol_47_layout() {
        let position = BlockPos::new(-12345, 64, 54321);
        let movement = PlayerMove { x: 12.5, y: 64.0, z: -3.25, yaw: 90.0, pitch: -15.0 };
        let full_frame = read_frame(&mut Cursor::new(encode_player_position_look(movement, true)), 64).unwrap();
        let mut full = PacketCursor::new(&full_frame);
        assert_eq!(full.read_varint().unwrap(), 0x06);
        assert_eq!(full.read_f64().unwrap(), 12.5);
        assert_eq!(full.read_f64().unwrap(), 64.0);
        assert_eq!(full.read_f64().unwrap(), -3.25);
        assert_eq!(full.read_f32().unwrap(), 90.0);
        assert_eq!(full.read_f32().unwrap(), -15.0);
        assert_eq!(full.read_u8().unwrap(), 1);

        let pos_frame = read_frame(&mut Cursor::new(encode_player_position(1.0, 2.0, 3.0, false)), 64).unwrap();
        let mut pos = PacketCursor::new(&pos_frame);
        assert_eq!(pos.read_varint().unwrap(), 0x04);
        assert_eq!(pos.read_f64().unwrap(), 1.0);
        assert_eq!(pos.read_f64().unwrap(), 2.0);
        assert_eq!(pos.read_f64().unwrap(), 3.0);
        assert_eq!(pos.read_u8().unwrap(), 0);

        let look_frame = read_frame(&mut Cursor::new(encode_player_look(45.0, 10.0, true)), 64).unwrap();
        let mut look = PacketCursor::new(&look_frame);
        assert_eq!(look.read_varint().unwrap(), 0x05);
        assert_eq!(look.read_f32().unwrap(), 45.0);
        assert_eq!(look.read_f32().unwrap(), 10.0);
        assert_eq!(look.read_u8().unwrap(), 1);

        let digging = encode_player_digging(DiggingStatus::Start, position, BlockFace::East);
        let body = read_frame(&mut Cursor::new(digging), 64).unwrap();
        let mut dig = PacketCursor::new(&body);
        assert_eq!(dig.read_varint().unwrap(), 0x07);
        assert_eq!(dig.read_varint().unwrap(), DiggingStatus::Start as i32);
        assert_eq!(decode_packed_position(dig.read_i64().unwrap() as u64), position);
        assert_eq!(dig.read_u8().unwrap(), BlockFace::East as u8);
        assert!(dig.is_empty());

        let slot = encode_held_item_change(8).unwrap();
        let body = read_frame(&mut Cursor::new(slot), 64).unwrap();
        let mut held = PacketCursor::new(&body);
        assert_eq!(held.read_varint().unwrap(), 0x09);
        assert_eq!(held.read_i16().unwrap(), 8);
        assert!(encode_held_item_change(9).is_err());
    }

    #[test]
    fn decodes_server_selected_hotbar_slot() {
        assert_eq!(decode_held_item_change(&[4]).unwrap(), 4);
        assert!(decode_held_item_change(&[9]).is_err());
        assert!(decode_held_item_change(&[]).is_err());
    }

    #[test]
    fn play_session_sends_movement_digging_and_held_slot_packets() {
        use std::net::{TcpListener, TcpStream};
        use std::thread;
        use std::time::Duration;

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let movement = read_frame(&mut socket, 128).unwrap();
            let mut movement = PacketCursor::new(&movement);
            assert_eq!(movement.read_varint().unwrap(), 0x06);
            assert_eq!(movement.read_f64().unwrap(), 1.0);
            assert_eq!(movement.read_f64().unwrap(), 65.0);
            assert_eq!(movement.read_f64().unwrap(), 2.0);
            assert_eq!(movement.read_f32().unwrap(), 30.0);
            assert_eq!(movement.read_f32().unwrap(), -5.0);
            assert_eq!(movement.read_u8().unwrap(), 1);

            let digging = read_frame(&mut socket, 128).unwrap();
            let mut digging = PacketCursor::new(&digging);
            assert_eq!(digging.read_varint().unwrap(), 0x07);
            assert_eq!(digging.read_varint().unwrap(), 0);
            assert_eq!(decode_packed_position(digging.read_i64().unwrap() as u64), BlockPos::new(4, 64, -2));
            assert_eq!(digging.read_u8().unwrap(), 1);

            let held = read_frame(&mut socket, 64).unwrap();
            let mut held = PacketCursor::new(&held);
            assert_eq!(held.read_varint().unwrap(), 0x09);
            assert_eq!(held.read_i16().unwrap(), 6);
        });

        let socket = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut play = PlaySession {
            identity: LoginSuccess { uuid: "test".into(), username: "Player".into() },
            stream: PacketStream::new(socket),
            entity_id: 1, dimension: Some(0), selected_hotbar_slot: 0,
            inventory_windows: std::collections::HashMap::new(), carried_item: None,
            open_windows: std::collections::HashMap::new(), window_properties: std::collections::HashMap::new(), transaction_results: std::collections::HashMap::new(),
        };
        play.send_player_movement(PlayerMove { x: 1.0, y: 65.0, z: 2.0, yaw: 30.0, pitch: -5.0 }, true).unwrap();
        play.send_digging(DiggingStatus::Start, BlockPos::new(4, 64, -2), BlockFace::Up).unwrap();
        play.send_held_item_change(6).unwrap();
        assert_eq!(play.selected_hotbar_slot, 6);
        server.join().unwrap();
    }
    fn put_network_item_stack(item_id: i16, count: i8, damage: i16, nbt: Option<&[u8]>, output: &mut Vec<u8>) {
        output.extend_from_slice(&item_id.to_be_bytes());
        if item_id >= 0 {
            output.push(count as u8);
            output.extend_from_slice(&damage.to_be_bytes());
            if let Some(nbt) = nbt { output.extend_from_slice(nbt); } else { output.push(0); }
        }
    }

    #[test]
    fn decodes_set_slot_with_empty_and_compound_nbt_items() {
        let mut payload = vec![0];
        payload.extend_from_slice(&5_i16.to_be_bytes());
        put_network_item_stack(-1, 0, 0, None, &mut payload);
        let empty = decode_set_slot(&payload).unwrap();
        assert_eq!(empty.window_id, 0);
        assert_eq!(empty.slot, 5);
        assert!(empty.item.is_none());

        let compound = [10, 0, 0, 0];
        let mut payload = vec![0];
        payload.extend_from_slice(&36_i16.to_be_bytes());
        put_network_item_stack(264, 1, 7, Some(&compound), &mut payload);
        let item = decode_set_slot(&payload).unwrap().item.unwrap();
        assert_eq!(item.item_id, 264);
        assert_eq!(item.count, 1);
        assert_eq!(item.damage, 7);
        assert_eq!(item.tag.unwrap().value, Tag::Compound(std::collections::BTreeMap::new()));
    }

    #[test]
    fn decodes_window_items_and_multiple_nbt_slots() {
        let compound = [10, 0, 0, 0];
        let mut payload = vec![0];
        payload.extend_from_slice(&3_i16.to_be_bytes());
        put_network_item_stack(-1, 0, 0, None, &mut payload);
        put_network_item_stack(1, 64, 0, None, &mut payload);
        put_network_item_stack(264, 1, 7, Some(&compound), &mut payload);
        let decoded = decode_window_items(&payload).unwrap();
        assert_eq!(decoded.window_id, 0);
        assert_eq!(decoded.items.len(), 3);
        assert!(decoded.items[0].is_none());
        assert_eq!(decoded.items[1].as_ref().unwrap().count, 64);
        assert!(decoded.items[2].as_ref().unwrap().tag.is_some());
    }

    #[test]
    fn rejects_malformed_item_nbt_and_window_lengths() {
        let mut malformed = vec![0];
        malformed.extend_from_slice(&1_i16.to_be_bytes());
        put_network_item_stack(1, 1, 0, Some(&[10, 0, 0]), &mut malformed);
        assert!(decode_set_slot(&malformed).is_err());

        let mut too_many = vec![0];
        too_many.extend_from_slice(&(-1_i16).to_be_bytes());
        assert!(matches!(decode_window_items(&too_many), Err(ProtocolError::InvalidWindowData(_))));
    }
    #[test]
    fn play_session_applies_server_inventory_updates() {
        use std::net::{TcpListener, TcpStream};
        use std::thread;
        use std::time::Duration;

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut window_items = vec![0];
            window_items.extend_from_slice(&3_i16.to_be_bytes());
            put_network_item_stack(-1, 0, 0, None, &mut window_items);
            put_network_item_stack(1, 64, 0, None, &mut window_items);
            put_network_item_stack(264, 1, 7, Some(&[10, 0, 0, 0]), &mut window_items);
            socket.write_all(&encode_packet(0x30, &window_items)).unwrap();

            let mut carried = vec![0xff];
            carried.extend_from_slice(&(-1_i16).to_be_bytes());
            put_network_item_stack(388, 4, 0, None, &mut carried);
            socket.write_all(&encode_packet(0x2F, &carried)).unwrap();

            let mut slot = vec![0];
            slot.extend_from_slice(&1_i16.to_be_bytes());
            put_network_item_stack(5, 12, 2, None, &mut slot);
            socket.write_all(&encode_packet(0x2F, &slot)).unwrap();
        });

        let socket = TcpStream::connect(("127.0.0.1", port)).unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut play = PlaySession {
            identity: LoginSuccess { uuid: "test".into(), username: "Player".into() },
            stream: PacketStream::new(socket),
            entity_id: 1,
            dimension: Some(0),
            selected_hotbar_slot: 0,
            inventory_windows: std::collections::HashMap::new(),
            carried_item: None,
            open_windows: std::collections::HashMap::new(),
            window_properties: std::collections::HashMap::new(),
            transaction_results: std::collections::HashMap::new(),
        };

        assert!(matches!(play.read_event().unwrap(), PlayEvent::WindowItems(_)));
        let inventory = play.inventory_windows.get(&0).unwrap();
        assert_eq!(inventory.len(), 3);
        assert!(inventory[0].is_none());
        assert_eq!(inventory[1].as_ref().unwrap().item_id, 1);
        assert_eq!(inventory[2].as_ref().unwrap().tag.as_ref().unwrap().value, Tag::Compound(std::collections::BTreeMap::new()));

        assert!(matches!(play.read_event().unwrap(), PlayEvent::SetSlot(_)));
        assert_eq!(play.carried_item.as_ref().unwrap().item_id, 388);
        assert_eq!(play.carried_item.as_ref().unwrap().count, 4);

        assert!(matches!(play.read_event().unwrap(), PlayEvent::SetSlot(_)));
        let inventory = play.inventory_windows.get(&0).unwrap();
        assert_eq!(inventory[1].as_ref().unwrap().item_id, 5);
        assert_eq!(inventory[1].as_ref().unwrap().count, 12);
        server.join().unwrap();
    }
    #[test]
    fn decodes_window_lifecycle_and_transaction_packets() {
        let mut open = vec![4];
        write_string("EntityHorse", &mut open);
        write_string(r#"{"text":"Horse"}"#, &mut open);
        open.push(17);
        open.extend_from_slice(&123_i32.to_be_bytes());
        let decoded = decode_open_window(&open).unwrap();
        assert_eq!(decoded.window_id, 4);
        assert_eq!(decoded.inventory_type, "EntityHorse");
        assert_eq!(decoded.title_json, r#"{"text":"Horse"}"#);
        assert_eq!(decoded.slot_count, 17);
        assert_eq!(decoded.horse_entity_id, Some(123));

        assert_eq!(decode_close_window(&[4]).unwrap(), CloseWindowData { window_id: 4 });

        let mut property = vec![4];
        property.extend_from_slice(&2_i16.to_be_bytes());
        property.extend_from_slice(&150_i16.to_be_bytes());
        assert_eq!(decode_window_property(&property).unwrap(), WindowPropertyData { window_id: 4, property: 2, value: 150 });

        let confirm = [4, 0x01, 0x23, 0];
        assert_eq!(decode_confirm_transaction(&confirm).unwrap(), ConfirmTransactionData { window_id: 4, action_number: 0x123, accepted: false });
        let reply = read_frame(&mut Cursor::new(encode_confirm_transaction_reply(4, 0x123, true)), 64).unwrap();
        let mut cursor = PacketCursor::new(&reply);
        assert_eq!(cursor.read_varint().unwrap(), 0x0F);
        assert_eq!(cursor.read_u8().unwrap(), 4);
        assert_eq!(cursor.read_i16().unwrap(), 0x123);
        assert_eq!(cursor.read_u8().unwrap(), 1);
    }

    #[test]
    fn play_session_tracks_windows_properties_closes_and_replies_to_rejected_transactions() {
        use std::net::{TcpListener, TcpStream};
        use std::thread;
        use std::time::Duration;

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut open = vec![2];
            write_string("minecraft:chest", &mut open);
            write_string(r#"{"text":"Test Chest"}"#, &mut open);
            open.push(27);
            socket.write_all(&encode_packet(0x2D, &open)).unwrap();

            let mut property = vec![2];
            property.extend_from_slice(&1_i16.to_be_bytes());
            property.extend_from_slice(&120_i16.to_be_bytes());
            socket.write_all(&encode_packet(0x31, &property)).unwrap();

            socket.write_all(&encode_packet(0x32, &[2, 0, 7, 0])).unwrap();
            let reply = read_frame(&mut socket, 64).unwrap();
            let mut cursor = PacketCursor::new(&reply);
            assert_eq!(cursor.read_varint().unwrap(), 0x0F);
            assert_eq!(cursor.read_u8().unwrap(), 2);
            assert_eq!(cursor.read_i16().unwrap(), 7);
            assert_eq!(cursor.read_u8().unwrap(), 1);

            socket.write_all(&encode_packet(0x2E, &[2])).unwrap();
        });
        let socket = TcpStream::connect(("127.0.0.1", port)).unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut play = PlaySession {
            identity: LoginSuccess { uuid: "test".into(), username: "Player".into() },
            stream: PacketStream::new(socket), entity_id: 1, dimension: Some(0),
            selected_hotbar_slot: 0,
            inventory_windows: std::collections::HashMap::new(), carried_item: None,
            open_windows: std::collections::HashMap::new(), window_properties: std::collections::HashMap::new(),
            transaction_results: std::collections::HashMap::new(),
        };
        assert!(matches!(play.read_event().unwrap(), PlayEvent::OpenWindow(_)));
        assert_eq!(play.open_windows.get(&2).unwrap().slot_count, 27);
        assert!(matches!(play.read_event().unwrap(), PlayEvent::WindowProperty(_)));
        assert_eq!(play.window_properties.get(&(2, 1)), Some(&120));
        assert!(matches!(play.read_event().unwrap(), PlayEvent::ConfirmTransaction(_)));
        assert_eq!(play.transaction_results.get(&(2, 7)), Some(&false));
        assert!(matches!(play.read_event().unwrap(), PlayEvent::CloseWindow(_)));
        assert!(!play.open_windows.contains_key(&2));
        assert!(!play.inventory_windows.contains_key(&2));
        server.join().unwrap();
    }
    #[test]
    fn encodes_protocol_47_block_placement_and_item_use() {
        let item = ItemStack { item_id: 35, count: 64, damage: 14, tag: None };
        let packet = encode_block_placement(BlockPos::new(-17, 64, 31), 1, Some(&item), [0.5, 1.0, 0.0]).unwrap();
        let body = read_frame(&mut Cursor::new(packet), 128).unwrap();
        let mut cursor = PacketCursor::new(&body);
        assert_eq!(cursor.read_varint().unwrap(), 0x08);
        assert_eq!(decode_packed_position(cursor.read_i64().unwrap() as u64), BlockPos::new(-17, 64, 31));
        assert_eq!(cursor.read_u8().unwrap(), 1);
        assert_eq!(cursor.read_i16().unwrap(), 35);
        assert_eq!(cursor.read_i8().unwrap(), 64);
        assert_eq!(cursor.read_i16().unwrap(), 14);
        assert_eq!(cursor.read_u8().unwrap(), 0); // Empty NBT marker (TAG_End).
        assert_eq!([cursor.read_u8().unwrap(), cursor.read_u8().unwrap(), cursor.read_u8().unwrap()], [8, 16, 0]);
        assert!(cursor.is_empty());
        let use_packet = encode_block_placement(BlockPos::new(-1, -1, -1), 255, None, [0.0; 3]).unwrap();
        let use_body = read_frame(&mut Cursor::new(use_packet), 64).unwrap();
        let mut use_cursor = PacketCursor::new(&use_body);
        assert_eq!(use_cursor.read_varint().unwrap(), 0x08);
        assert_eq!(decode_packed_position(use_cursor.read_i64().unwrap() as u64), BlockPos::new(-1, -1, -1));
        assert_eq!(use_cursor.read_u8().unwrap(), 255);
        assert_eq!(use_cursor.read_i16().unwrap(), -1);
        assert_eq!([use_cursor.read_u8().unwrap(), use_cursor.read_u8().unwrap(), use_cursor.read_u8().unwrap()], [0, 0, 0]);
    }

    #[test]
    fn item_stack_writer_round_trips_compound_nbt_and_checks_placement_arguments() {
        let mut contents = std::collections::BTreeMap::new();
        contents.insert("Name".into(), Tag::String("Test item".into()));
        let item = ItemStack { item_id: 276, count: 1, damage: 7, tag: Some(NamedTag { name: String::new(), value: Tag::Compound(contents) }) };
        let mut bytes = Vec::new();
        write_item_stack(Some(&item), &mut bytes).unwrap();
        let decoded = PacketCursor::new(&bytes).read_item_stack().unwrap().unwrap();
        assert_eq!(decoded, item);
        assert!(encode_block_placement(BlockPos::new(0, 64, 0), 1, None, [1.5, 0.0, 0.0]).is_err());
        assert!(encode_block_placement(BlockPos::new(0, 64, 0), 7, None, [0.0; 3]).is_err());
    }
    #[test]
    fn parses_host_ports() {
        assert_eq!(parse_host_port("play.example.net:25565").unwrap(), ("play.example.net".into(), 25565));
        assert_eq!(parse_host_port("[::1]:25566").unwrap(), ("::1".into(), 25566));
        assert!(parse_host_port("example.net").is_err());
    }
}
