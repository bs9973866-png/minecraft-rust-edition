//! Minecraft Java protocol primitives for protocol 47 (1.8.9).

use std::error::Error as StdError;
use std::fmt;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;

pub const PROTOCOL_VERSION: i32 = 47;
pub const DEFAULT_MAX_FRAME_SIZE: usize = 2 * 1024 * 1024;

#[derive(Debug)]
pub enum ProtocolError {
    Io(io::Error), Compression(String), UnexpectedEof, VarIntTooLong, InvalidLength(i32),
    FrameTooLarge { length: usize, maximum: usize }, InvalidUtf8,
    StringTooLong { length: usize, maximum: usize },
    UnexpectedPacket { expected: i32, actual: i32 }, InvalidAddress(String),
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
            Self::StringTooLong { length, maximum } => write!(f, "packet string is too long ({length} bytes; maximum {maximum})"),
            Self::UnexpectedPacket { expected, actual } => write!(f, "unexpected packet id {actual}; expected {expected}"),
            Self::InvalidAddress(address) => write!(f, "invalid server address: {address}"),
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
    #[test]
    fn parses_host_ports() {
        assert_eq!(parse_host_port("play.example.net:25565").unwrap(), ("play.example.net".into(), 25565));
        assert_eq!(parse_host_port("[::1]:25566").unwrap(), ("::1".into(), 25566));
        assert!(parse_host_port("example.net").is_err());
    }
}
