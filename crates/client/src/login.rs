//! Offline-mode login handshake for Minecraft Java 1.8.9 (protocol 47).
//! Supports the Set Compression login packet; online-mode encryption is not implemented.
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use flate2::read::ZlibDecoder;
use protocol::{decode_varint, encode_varint};

const PROTOCOL_VERSION: i32 = 47;
const MAX_PACKET_BYTES: usize = 1_048_576;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginSuccess {
    pub uuid: String,
    pub username: String,
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

fn send_packet(stream: &mut TcpStream, payload: &[u8]) -> Result<(), String> {
    if payload.len() > MAX_PACKET_BYTES { return Err("outgoing packet exceeds safety limit".into()); }
    let mut framed = Vec::with_capacity(payload.len() + 5);
    encode_varint(payload.len() as i32, &mut framed);
    framed.extend_from_slice(payload);
    stream.write_all(&framed).map_err(|e| format!("sending login packet: {e}"))
}

fn read_frame(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut prefix = Vec::with_capacity(5);
    let length = loop {
        if prefix.len() == 5 { return Err("packet length VarInt is too long".into()); }
        let mut byte = [0_u8; 1];
        stream.read_exact(&mut byte).map_err(|e| format!("reading login packet length: {e}"))?;
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
    stream.read_exact(&mut frame).map_err(|e| format!("reading login packet: {e}"))?;
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
    if body.len() < 1 || body.len() >= MAX_PACKET_BYTES {
        return Err("invalid compressed packet body length".into());
    }
    if data_length as usize < threshold {
        return Err("server compressed a packet smaller than the compression threshold".into());
    }
    let mut decoder = ZlibDecoder::new(body);
    let mut decoded = Vec::with_capacity(data_length as usize);
    decoder.take((MAX_PACKET_BYTES + 1) as u64).read_to_end(&mut decoded)
        .map_err(|e| format!("decompressing server packet: {e}"))?;
    if decoded.len() != data_length as usize {
        return Err(format!("decompressed packet length mismatch: expected {data_length}, got {}", decoded.len()));
    }
    Ok(decoded)
}

/// Logs into an offline-mode server. Online-mode servers require authenticated encryption.
pub fn login_offline(host: &str, port: u16, username: &str) -> Result<LoginSuccess, String> {
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
    stream.set_read_timeout(Some(Duration::from_secs(8))).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(Duration::from_secs(8))).map_err(|e| e.to_string())?;

    let mut handshake = Vec::new();
    encode_varint(0, &mut handshake);
    encode_varint(PROTOCOL_VERSION, &mut handshake);
    write_string(host, &mut handshake)?;
    handshake.extend_from_slice(&port.to_be_bytes());
    encode_varint(2, &mut handshake);
    send_packet(&mut stream, &handshake)?;

    let mut start = Vec::new();
    encode_varint(0, &mut start);
    write_string(username, &mut start)?;
    send_packet(&mut stream, &start)?;

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
                return Ok(LoginSuccess { uuid, username: accepted_name });
            }
            0x00 => {
                let mut offset = id_len;
                let reason = read_string(&packet, &mut offset)
                    .unwrap_or_else(|_| "unknown reason".into());
                return Err(format!("server rejected login: {reason}"));
            }
            0x01 => return Err("server requires Microsoft/Mojang online-mode authentication; this prototype supports offline-mode servers only".into()),
            0x03 => {
                let (threshold, used) = decode_varint(packet.get(id_len..).ok_or("missing compression threshold")?)
                    .map_err(|e| e.to_string())?;
                if threshold < 0 || threshold as usize > MAX_PACKET_BYTES {
                    return Err(format!("server supplied invalid compression threshold: {threshold}"));
                }
                compression_threshold = Some(threshold as usize);
            }
            other => return Err(format!("unexpected login packet 0x{other:02x}; login is not complete")),
        }
    }
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
}
