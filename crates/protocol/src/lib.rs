//! Minecraft Java 1.8.9 protocol primitives.
//!
//! This crate intentionally starts with small, independently tested codecs.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    Incomplete,
    VarIntTooLong,
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Incomplete => write!(f, "incomplete protocol value"),
            Self::VarIntTooLong => write!(f, "VarInt exceeds five bytes"),
        }
    }
}

impl std::error::Error for CodecError {}

/// Encodes a Minecraft protocol VarInt (signed values use two's-complement bits).
pub fn encode_varint(value: i32, out: &mut Vec<u8>) {
    let mut value = value as u32;
    loop {
        if value & !0x7f == 0 {
            out.push(value as u8);
            return;
        }
        out.push(((value & 0x7f) as u8) | 0x80);
        value >>= 7;
    }
}

/// Decodes a VarInt and returns `(value, bytes_consumed)`.
pub fn decode_varint(input: &[u8]) -> Result<(i32, usize), CodecError> {
    let mut result = 0_u32;
    for index in 0..5 {
        let byte = *input.get(index).ok_or(CodecError::Incomplete)?;
        result |= u32::from(byte & 0x7f) << (7 * index);
        if byte & 0x80 == 0 {
            return Ok((result as i32, index + 1));
        }
    }
    Err(CodecError::VarIntTooLong)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varint_round_trips_protocol_values() {
        for value in [0, 1, 127, 128, 255, 2_147_483_647, -1, -2_147_483_648] {
            let mut bytes = Vec::new();
            encode_varint(value, &mut bytes);
            assert_eq!(decode_varint(&bytes), Ok((value, bytes.len())));
        }
    }

    #[test]
    fn incomplete_varint_is_reported() {
        assert_eq!(decode_varint(&[0x80]), Err(CodecError::Incomplete));
    }

    #[test]
    fn oversized_varint_is_rejected() {
        assert_eq!(decode_varint(&[0x80; 5]), Err(CodecError::VarIntTooLong));
    }
}


pub mod play;
pub use play::{decode_play_packet, JoinGame, PlayPacket};
