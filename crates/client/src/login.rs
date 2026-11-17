//! Offline-mode login and live Play-state session for Minecraft Java 1.8.9 (protocol 47).
//! Online-mode encryption/authentication is deliberately not implemented.
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};
use game::{DigStatus, PlayerMovement, ServerAction};
use protocol::{decode_play_packet, decode_varint, encode_varint, PlayPacket};

const PROTOCOL_VERSION: i32 = 47;
const MAX_PACKET_BYTES: usize = 1_048_576;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginSuccess {
    pub uuid: String,
    pub username: String,
}

/// Thread-safe packet writer cloned from a connected Play session.
#[derive(Clone)]
pub struct PlayPacketSender {
    stream: Arc<Mutex<TcpStream>>,
    compression_threshold: Option<usize>,
}

fn encode_server_action(action: ServerAction) -> Result<Vec<u8>, String> {
    match action {
        ServerAction::DigBlock { position, face, status } => {
            if !(0..=255).contains(&position.y) {
                return Err("block Y coordinate is outside the 1.8.9 build height".into());
            }
            if face > 5 {
                return Err("invalid block face for protocol-47 digging packet".into());
            }
            let packed = ((i64::from(position.x) & 0x03ff_ffff) << 38)
                | ((i64::from(position.y) & 0x0fff) << 26)
                | (i64::from(position.z) & 0x03ff_ffff);
            let status = match status {
                DigStatus::Start => 0,
                DigStatus::Cancel => 1,
                DigStatus::Finish => 2,
            };
            let mut packet = vec![0x07, status];
            packet.extend_from_slice(&packed.to_be_bytes());
            packet.push(face);
            Ok(packet)
        }
        ServerAction::UseBlock { position, face, cursor } => {
            if !(0..=255).contains(&position.y) {
                return Err("block Y coordinate is outside the 1.8.9 build height".into());
            }
            if face > 5 {
                return Err("invalid block face for protocol-47 placement packet".into());
            }
            if cursor.iter().any(|value| *value > 15) {
                return Err("block hit cursor coordinates must be in 0..=15".into());
            }
            let packed = ((i64::from(position.x) & 0x03ff_ffff) << 38)
                | ((i64::from(position.y) & 0x0fff) << 26)
                | (i64::from(position.z) & 0x03ff_ffff);
            // C08 Player Block Placement. Item ID -1 means an empty hand.
            let mut packet = vec![0x08];
            packet.extend_from_slice(&packed.to_be_bytes());
            packet.push(face);
            packet.extend_from_slice(&(-1_i16).to_be_bytes());
            packet.extend_from_slice(&cursor);
            Ok(packet)
        }
    }
}

impl PlayPacketSender {
    fn send(&self, payload: &[u8]) -> Result<(), String> {
        let mut stream = self.stream.lock().map_err(|_| "packet writer lock poisoned")?;
        send_packet(&mut stream, payload, self.compression_threshold)
    }

    /// Send the protocol-47 serverbound Player Position And Look packet.
    pub fn send_player_movement(&self, movement: PlayerMovement) -> Result<(), String> {
        let mut packet = vec![0x06];
        for coordinate in movement.position {
            packet.extend_from_slice(&coordinate.to_be_bytes());
        }
        packet.extend_from_slice(&movement.yaw.to_be_bytes());
        packet.extend_from_slice(&movement.pitch.to_be_bytes());
        packet.push(u8::from(movement.on_ground));
        self.send(&packet)
    }

    /// Send a protocol-47 block-digging action.
    pub fn send_server_action(&self, action: ServerAction) -> Result<(), String> {
        let packet = encode_server_action(action)?;
        self.send(&packet)
    }

    /// Close the underlying TCP connection, including its read side.
    pub fn shutdown(&self) -> Result<(), String> {
        let stream = self.stream.lock().map_err(|_| "packet writer lock poisoned")?;
        stream.shutdown(Shutdown::Both).map_err(|e| format!("closing Minecraft connection: {e}"))
    }
}

/// A connected Minecraft Play-state connection. The TCP stream stays open after Login Success.
pub struct PlaySession {
    stream: TcpStream,
    writer: PlayPacketSender,
    pub player: LoginSuccess,
    compression_threshold: Option<usize>,
    position: [f64; 3],
    yaw: f32,
    pitch: f32,
}

fn write_string(value: &str, out: &mut Vec<u8>) -> Result<(), String> {
    if value.len() > 32_767 { return Err("protocol string is too long".into()); }
    encode_varint(value.len() as i32, out);
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

fn read_string(bytes: &[u8], offset: &mut usize) -> Result<String, String> {
    let (length, used) = decode_varint(bytes.get(*offset..).ok_or("missing string length")?)
        .map_err(|e| e.to_string())?;
    *offset += used;
    if length < 0 || length as usize > 32_767 { return Err("invalid protocol string length".into()); }
    let end = (*offset).checked_add(length as usize).ok_or("string length overflow")?;
    let value = bytes.get(*offset..end).ok_or("truncated protocol string")?;
    *offset = end;
    String::from_utf8(value.to_vec()).map_err(|e| format!("invalid UTF-8 string: {e}"))
}

fn send_packet(
    stream: &mut TcpStream,
    payload: &[u8],
    compression_threshold: Option<usize>,
) -> Result<(), String> {
    if payload.len() > MAX_PACKET_BYTES { return Err("outgoing packet exceeds safety limit".into()); }
    let mut body = Vec::with_capacity(payload.len() + 16);
    if let Some(threshold) = compression_threshold {
        if payload.len() >= threshold {
            encode_varint(payload.len() as i32, &mut body);
            let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(payload).map_err(|e| format!("compressing outgoing packet: {e}"))?;
            body.extend_from_slice(&encoder.finish().map_err(|e| format!("finishing packet compression: {e}"))?);
        } else {
            encode_varint(0, &mut body);
            body.extend_from_slice(payload);
        }
    } else {
        body.extend_from_slice(payload);
    }
    if body.len() > MAX_PACKET_BYTES { return Err("framed packet exceeds safety limit".into()); }
    let mut framed = Vec::with_capacity(body.len() + 5);
    encode_varint(body.len() as i32, &mut framed);
    framed.extend_from_slice(&body);
    stream.write_all(&framed).map_err(|e| format!("sending Minecraft packet: {e}"))
}

fn read_frame(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut prefix = Vec::with_capacity(5);
    let length = loop {
        if prefix.len() == 5 { return Err("packet length VarInt is too long".into()); }
        let mut byte = [0_u8; 1];
        stream.read_exact(&mut byte).map_err(|e| format!("reading Minecraft packet length: {e}"))?;
        prefix.push(byte[0]);
        match decode_varint(&prefix) {
            Ok((value, _)) => break value,
            Err(protocol::CodecError::Incomplete) => continue,
            Err(error) => return Err(error.to_string()),
        }
    };
    if length <= 0 || length as usize > MAX_PACKET_BYTES {
        return Err(format!("server returned invalid packet length: {length}"));
    }
    let mut frame = vec![0; length as usize];
    stream.read_exact(&mut frame).map_err(|e| format!("reading Minecraft packet: {e}"))?;
    Ok(frame)
}

fn decode_frame(frame: &[u8], compression_threshold: Option<usize>) -> Result<Vec<u8>, String> {
    if frame.len() > MAX_PACKET_BYTES { return Err("packet exceeds safety limit".into()); }
    let Some(threshold) = compression_threshold else { return Ok(frame.to_vec()); };
    let (data_length, prefix_len) = decode_varint(frame).map_err(|e| e.to_string())?;
    if data_length < 0 || data_length as usize > MAX_PACKET_BYTES {
        return Err("invalid compressed packet data length".into());
    }
    let body = frame.get(prefix_len..).ok_or("missing compressed packet body")?;
    if data_length == 0 {
        if body.len() >= threshold {
            return Err("server sent an uncompressed packet at or above the compression threshold".into());
        }
        return Ok(body.to_vec());
    }
    if body.is_empty() || body.len() >= MAX_PACKET_BYTES {
        return Err("invalid compressed packet body length".into());
    }
    if (data_length as usize) < threshold {
        return Err("server compressed a packet smaller than the compression threshold".into());
    }
    let decoder = ZlibDecoder::new(body);
    let mut decoded = Vec::with_capacity(data_length as usize);
    decoder.take((MAX_PACKET_BYTES + 1) as u64).read_to_end(&mut decoded)
        .map_err(|e| format!("decompressing server packet: {e}"))?;
    if decoded.len() != data_length as usize {
        return Err(format!("decompressed packet length mismatch: expected {data_length}, got {}", decoded.len()));
    }
    Ok(decoded)
}

impl PlaySession {
    /// Send the 1.8 client settings packet (locale, view distance, chat and skin flags).
    pub fn send_client_settings(&mut self) -> Result<(), String> {
        let mut packet = vec![0x15];
        write_string("en_US", &mut packet)?;
        packet.push(8); // view distance
        encode_varint(0, &mut packet); // enabled chat
        packet.push(1); // chat colours
        packet.push(0x7f); // all skin layers
        self.writer.send(&packet)
    }

    /// Obtain a cloneable packet writer for a dedicated movement thread.
    pub fn packet_sender(&self) -> PlayPacketSender {
        self.writer.clone()
    }

    /// Current authoritative position after the latest server correction.
    pub fn position(&self) -> [f64; 3] {
        self.position
    }

    pub fn rotation(&self) -> (f32, f32) {
        (self.yaw, self.pitch)
    }

    /// Read and decode the next serverbound-independent Play packet.
    pub fn read_play_packet(&mut self) -> Result<PlayPacket, String> {
        let frame = read_frame(&mut self.stream)?;
        let payload = decode_frame(&frame, self.compression_threshold)?;
        decode_play_packet(&payload)
    }

    /// Reply to protocol-required server packets and apply authoritative position corrections.
    pub fn respond_to_server_packet(&mut self, packet: &PlayPacket) -> Result<(), String> {
        match packet {
            PlayPacket::KeepAlive { id } => {
                let mut response = vec![0x00];
                encode_varint(*id, &mut response);
                self.writer.send(&response)?;
            }
            PlayPacket::PlayerPositionLook { x, y, z, yaw, pitch, flags } => {
                let next = [
                    if flags & 0x01 != 0 { self.position[0] + x } else { *x },
                    if flags & 0x02 != 0 { self.position[1] + y } else { *y },
                    if flags & 0x04 != 0 { self.position[2] + z } else { *z },
                ];
                let next_yaw = if flags & 0x08 != 0 { self.yaw + yaw } else { *yaw };
                let next_pitch = if flags & 0x10 != 0 { self.pitch + pitch } else { *pitch };

                let mut response = vec![0x06];
                for coordinate in next { response.extend_from_slice(&coordinate.to_be_bytes()); }
                response.extend_from_slice(&next_yaw.to_be_bytes());
                response.extend_from_slice(&next_pitch.to_be_bytes());
                response.push(1); // onGround
                send_packet(&mut self.stream, &response, self.compression_threshold)?;
                self.position = next;
                self.yaw = next_yaw;
                self.pitch = next_pitch;
            }
            _ => {}
        }
        Ok(())
    }
}

/// Complete an offline-mode login and return the still-open Play-state connection.
pub fn connect_offline(host: &str, port: u16, username: &str) -> Result<PlaySession, String> {
    if !(3..=16).contains(&username.len())
        || !username.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err("invalid offline nickname: use 3–16 Latin letters, digits, or underscores".into());
    }

    let address = (host, port).to_socket_addrs()
        .map_err(|e| format!("resolving {host}:{port}: {e}"))?
        .next().ok_or("hostname resolved to no addresses")?;
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(8))
        .map_err(|e| format!("connecting to {host}:{port}: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(120))).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(Duration::from_secs(15))).map_err(|e| e.to_string())?;
    stream.set_nodelay(true).map_err(|e| e.to_string())?;

    let mut handshake = Vec::new();
    encode_varint(0, &mut handshake);
    encode_varint(PROTOCOL_VERSION, &mut handshake);
    write_string(host, &mut handshake)?;
    handshake.extend_from_slice(&port.to_be_bytes());
    encode_varint(2, &mut handshake); // Login state
    send_packet(&mut stream, &handshake, None)?;

    let mut start = Vec::new();
    encode_varint(0, &mut start);
    write_string(username, &mut start)?;
    send_packet(&mut stream, &start, None)?;

    let mut compression_threshold = None;
    loop {
        let frame = read_frame(&mut stream)?;
        let packet = decode_frame(&frame, compression_threshold)?;
        let (packet_id, id_len) = decode_varint(&packet).map_err(|e| e.to_string())?;
        match packet_id {
            0x02 => {
                let mut offset = id_len;
                let uuid = read_string(&packet, &mut offset)?;
                let accepted_name = read_string(&packet, &mut offset)?;
                if offset != packet.len() { return Err("unexpected trailing bytes in Login Success".into()); }
                let writer_stream = stream.try_clone()
                    .map_err(|e| format!("cloning Minecraft packet writer: {e}"))?;
                let writer = PlayPacketSender {
                    stream: Arc::new(Mutex::new(writer_stream)),
                    compression_threshold,
                };
                return Ok(PlaySession {
                    stream,
                    writer,
                    player: LoginSuccess { uuid, username: accepted_name },
                    compression_threshold,
                    position: [0.0; 3],
                    yaw: 0.0,
                    pitch: 0.0,
                });
            }
            0x00 => {
                let mut offset = id_len;
                let reason = read_string(&packet, &mut offset)
                    .unwrap_or_else(|_| "unknown reason".into());
                return Err(format!("server rejected login: {reason}"));
            }
            0x01 => return Err("server requires Microsoft/Mojang online-mode authentication; this client currently supports offline-mode servers only".into()),
            0x03 => {
                let (threshold, used) = decode_varint(packet.get(id_len..).ok_or("missing compression threshold")?)
                    .map_err(|e| e.to_string())?;
                if threshold < 0 || threshold as usize > MAX_PACKET_BYTES {
                    return Err(format!("server supplied invalid compression threshold: {threshold}"));
                }
                if id_len + used != packet.len() { return Err("unexpected trailing bytes in Set Compression".into()); }
                compression_threshold = Some(threshold as usize);
            }
            other => return Err(format!("unexpected login packet 0x{other:02x}; login is not complete")),
        }
    }
}

/// Backwards-compatible login-only helper. Prefer connect_offline for a live session.
pub fn login_offline(host: &str, port: u16, username: &str) -> Result<LoginSuccess, String> {
    Ok(connect_offline(host, port, username)?.player)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::ZlibEncoder, Compression};

    #[test]
    fn reads_protocol_strings() {
        let mut bytes = Vec::new();
        write_string("RustPlayer", &mut bytes).unwrap();
        let mut offset = 0;
        assert_eq!(read_string(&bytes, &mut offset).unwrap(), "RustPlayer");
        assert_eq!(offset, bytes.len());
    }

    #[test]
    fn rejects_truncated_protocol_strings() {
        let mut offset = 0;
        assert!(read_string(&[5, b'a'], &mut offset).is_err());
    }

    #[test]
    fn accepts_uncompressed_packet_below_threshold() {
        let packet = [0, 42, 7];
        let mut frame = Vec::new();
        encode_varint(0, &mut frame);
        frame.extend_from_slice(&packet);
        assert_eq!(decode_frame(&frame, Some(64)).unwrap(), packet);
    }

    #[test]
    fn decompresses_packet_above_threshold() {
        let packet = vec![0x2a; 256];
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&packet).unwrap();
        let compressed = encoder.finish().unwrap();
        let mut frame = Vec::new();
        encode_varint(packet.len() as i32, &mut frame);
        frame.extend_from_slice(&compressed);
        assert_eq!(decode_frame(&frame, Some(32)).unwrap(), packet);
    }

    #[test]
    fn rejects_uncompressed_packet_above_threshold() {
        let packet = [0x2a; 80];
        let mut frame = Vec::new();
        encode_varint(0, &mut frame);
        frame.extend_from_slice(&packet);
        assert!(decode_frame(&frame, Some(32)).is_err());
    }

    #[test]
    fn encodes_protocol_47_block_digging_packet() {
        let packet = encode_server_action(ServerAction::DigBlock {
            position: game::BlockPos::new(-12, 70, -31),
            face: 4,
            status: DigStatus::Finish,
        }).unwrap();
        let packed = ((-12_i64 & 0x03ff_ffff) << 38)
            | ((70_i64 & 0x0fff) << 26)
            | (-31_i64 & 0x03ff_ffff);
        let mut expected = vec![0x07, 2];
        expected.extend_from_slice(&packed.to_be_bytes());
        expected.push(4);
        assert_eq!(packet, expected);
    }

    #[test]
    fn encodes_protocol_47_empty_hand_block_interaction() {
        let packet = encode_server_action(ServerAction::UseBlock {
            position: game::BlockPos::new(-12, 70, -31),
            face: 1,
            cursor: [8, 4, 15],
        }).unwrap();
        let packed = ((-12_i64 & 0x03ff_ffff) << 38)
            | ((70_i64 & 0x0fff) << 26)
            | (-31_i64 & 0x03ff_ffff);
        let mut expected = vec![0x08];
        expected.extend_from_slice(&packed.to_be_bytes());
        expected.push(1);
        expected.extend_from_slice(&(-1_i16).to_be_bytes());
        expected.extend_from_slice(&[8, 4, 15]);
        assert_eq!(packet, expected);
    }

    #[test]
    fn rejects_invalid_block_interaction_face_cursor_and_height() {
        let invalid = [
            ServerAction::UseBlock {
                position: game::BlockPos::new(0, 64, 0), face: 6, cursor: [8, 8, 8],
            },
            ServerAction::UseBlock {
                position: game::BlockPos::new(0, 256, 0), face: 1, cursor: [8, 8, 8],
            },
            ServerAction::UseBlock {
                position: game::BlockPos::new(0, 64, 0), face: 1, cursor: [8, 16, 8],
            },
        ];
        for action in invalid {
            assert!(encode_server_action(action).is_err());
        }
    }

    #[test]
    fn rejects_invalid_dig_face_and_height() {
        assert!(encode_server_action(ServerAction::DigBlock {
            position: game::BlockPos::new(0, 64, 0),
            face: 6,
            status: DigStatus::Start,
        }).is_err());
        assert!(encode_server_action(ServerAction::DigBlock {
            position: game::BlockPos::new(0, 256, 0),
            face: 1,
            status: DigStatus::Start,
        }).is_err());
    }

    #[test]
    fn encodes_compressed_outgoing_packet() {
        let packet = vec![0x42; 128];
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&packet).unwrap();
        let expected = encoder.finish().unwrap();
        let mut stream = Vec::new();
        let mut body = Vec::new();
        encode_varint(packet.len() as i32, &mut body);
        body.extend_from_slice(&expected);
        encode_varint(body.len() as i32, &mut stream);
        stream.extend_from_slice(&body);
        assert!(!stream.is_empty());
    }
}
