//! Minecraft 1.8.x server-list status ping (protocol state: Handshaking -> Status).
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use serde_json::Value;

use protocol::{decode_varint, encode_varint};

const PROTOCOL_VERSION_1_8_9: i32 = 47;
const MAX_PACKET_BYTES: usize = 1_048_576;

fn write_string(value: &str, out: &mut Vec<u8>) -> Result<(), String> {
    let bytes = value.as_bytes();
    if bytes.len() > 32_767 {
        return Err("protocol string is too long".into());
    }
    encode_varint(bytes.len() as i32, out);
    out.extend_from_slice(bytes);
    Ok(())
}

fn frame_packet(payload: &[u8]) -> Result<Vec<u8>, String> {
    if payload.len() > MAX_PACKET_BYTES {
        return Err("outgoing packet exceeds safety limit".into());
    }
    let mut framed = Vec::with_capacity(payload.len() + 5);
    encode_varint(payload.len() as i32, &mut framed);
    framed.extend_from_slice(payload);
    Ok(framed)
}

fn read_varint(stream: &mut TcpStream) -> Result<i32, String> {
    let mut bytes = Vec::with_capacity(5);
    for _ in 0..5 {
        let mut byte = [0_u8; 1];
        stream.read_exact(&mut byte).map_err(|e| format!("reading packet length: {e}"))?;
        bytes.push(byte[0]);
        match decode_varint(&bytes) {
            Ok((value, _)) => return Ok(value),
            Err(protocol::CodecError::Incomplete) => continue,
            Err(error) => return Err(error.to_string()),
        }
    }
    Err("packet length VarInt is too long".into())
}

/// Connects to a Minecraft Java 1.8.9-compatible server and requests its status JSON.
pub fn ping_server(host: &str, port: u16) -> Result<Value, String> {
    let mut addresses = (host, port).to_socket_addrs()
        .map_err(|e| format!("resolving {host}:{port}: {e}"))?;
    let address = addresses.next().ok_or_else(|| "hostname resolved to no addresses".to_string())?;

    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5))
        .map_err(|e| format!("connecting to {host}:{port}: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;
    stream.set_write_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;

    // Handshake packet: id=0, protocol=47, host, port, next state=Status(1).
    let mut handshake = Vec::new();
    encode_varint(0, &mut handshake);
    encode_varint(PROTOCOL_VERSION_1_8_9, &mut handshake);
    write_string(host, &mut handshake)?;
    handshake.extend_from_slice(&port.to_be_bytes());
    encode_varint(1, &mut handshake);
    stream.write_all(&frame_packet(&handshake)?).map_err(|e| format!("sending handshake: {e}"))?;

    // Status Request packet: packet id 0, empty body.
    stream.write_all(&frame_packet(&[0])?).map_err(|e| format!("sending status request: {e}"))?;

    let packet_len = read_varint(&mut stream)?;
    if packet_len <= 0 || packet_len as usize > MAX_PACKET_BYTES {
        return Err(format!("server returned invalid packet length: {packet_len}"));
    }
    let mut packet = vec![0_u8; packet_len as usize];
    stream.read_exact(&mut packet).map_err(|e| format!("reading status response: {e}"))?;

    let (packet_id, id_len) = decode_varint(&packet).map_err(|e| e.to_string())?;
    if packet_id != 0 {
        return Err(format!("expected status response packet 0, got {packet_id}"));
    }
    let (json_len, len_len) = decode_varint(&packet[id_len..]).map_err(|e| e.to_string())?;
    if json_len < 0 {
        return Err("server returned a negative JSON length".into());
    }
    let start = id_len + len_len;
    let end = start.checked_add(json_len as usize).ok_or("JSON length overflow")?;
    if end > packet.len() {
        return Err("status JSON is truncated".into());
    }
    serde_json::from_slice(&packet[start..end]).map_err(|e| format!("invalid status JSON: {e}"))
}
