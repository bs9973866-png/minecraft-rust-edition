//! Decoders for the serverbound-independent part of the protocol-47 Play state.
//! These types intentionally preserve chat JSON and raw chunk payloads for later layers.

use crate::{decode_varint, CodecError};

#[derive(Debug, Clone, PartialEq)]
pub enum PlayPacket {
    JoinGame(JoinGame),
    ChatMessage { json: String, position: u8 },
    TimeUpdate { world_age: i64, time_of_day: i64 },
    SpawnPosition { x: i32, y: i32, z: i32 },
    UpdateHealth { health: f32, food: i32, food_saturation: f32 },
    PlayerPositionLook {
        x: f64, y: f64, z: f64, yaw: f32, pitch: f32, flags: u8,
    },
    ChunkData { x: i32, z: i32, ground_up: bool, primary_bitmap: u16, add_bitmap: u16, data: Vec<u8> },
    KeepAlive { id: i32 },
    Unknown { id: i32, payload: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinGame {
    pub entity_id: i32,
    pub game_mode: u8,
    pub hardcore: bool,
    pub dimension: i8,
    pub difficulty: u8,
    pub max_players: u8,
    pub level_type: String,
    pub reduced_debug_info: bool,
}

struct Reader<'a> { bytes: &'a [u8], offset: usize }

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self { Self { bytes, offset: 0 } }
    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let end = self.offset.checked_add(count).ok_or("packet offset overflow")?;
        let value = self.bytes.get(self.offset..end).ok_or("truncated play packet")?;
        self.offset = end;
        Ok(value)
    }
    fn u8(&mut self) -> Result<u8, String> { Ok(self.take(1)?[0]) }
    fn i8(&mut self) -> Result<i8, String> { Ok(self.u8()? as i8) }
    fn bool(&mut self) -> Result<bool, String> { match self.u8()? { 0 => Ok(false), 1 => Ok(true), _ => Err("invalid boolean in play packet".into()) } }
    fn i32(&mut self) -> Result<i32, String> { Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap())) }
    fn varint(&mut self) -> Result<i32, String> {
        let (value, used) = decode_varint(self.bytes.get(self.offset..).ok_or("missing VarInt")?)
            .map_err(|e| e.to_string())?;
        self.offset += used;
        Ok(value)
    }
    fn i64(&mut self) -> Result<i64, String> { Ok(i64::from_be_bytes(self.take(8)?.try_into().unwrap())) }
    fn f32(&mut self) -> Result<f32, String> { Ok(f32::from_bits(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))) }
    fn f64(&mut self) -> Result<f64, String> { Ok(f64::from_bits(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))) }
    fn string(&mut self) -> Result<String, String> {
        let (len, used) = decode_varint(self.bytes.get(self.offset..).ok_or("missing string length")?)
            .map_err(|e| e.to_string())?;
        self.offset += used;
        if len < 0 || len as usize > 32_767 { return Err("invalid play string length".into()); }
        let bytes = self.take(len as usize)?;
        String::from_utf8(bytes.to_vec()).map_err(|e| format!("invalid UTF-8 in play string: {e}"))
    }
    fn remaining(&self) -> &'a [u8] { &self.bytes[self.offset..] }
    fn finish(self) -> Result<(), String> {
        if self.offset == self.bytes.len() { Ok(()) } else { Err("unexpected trailing bytes in play packet".into()) }
    }
}

fn decode_position(packed: i64) -> (i32, i32, i32) {
    let x = (packed >> 38) as i32;
    let y = ((packed >> 26) & 0xfff) as i32;
    let z = (packed << 26 >> 38) as i32;
    (sign_extend(x, 26), sign_extend(y, 12), sign_extend(z, 26))
}
fn sign_extend(value: i32, bits: u32) -> i32 {
    let shift = 32 - bits;
    (value << shift) >> shift
}

/// Decode one already-decompressed protocol-47 Play packet payload (including packet ID).
pub fn decode_play_packet(payload: &[u8]) -> Result<PlayPacket, String> {
    let (id, id_len) = decode_varint(payload).map_err(|e: CodecError| e.to_string())?;
    let mut r = Reader::new(payload.get(id_len..).ok_or("missing play packet body")?);
    let packet = match id {
        0x00 => PlayPacket::KeepAlive { id: r.varint()? },
        0x01 => {
            let entity_id = r.i32()?;
            let game_mode_raw = r.u8()?;
            let dimension = r.i8()?;
            let difficulty = r.u8()?;
            let max_players = r.u8()?;
            let level_type = r.string()?;
            let reduced_debug_info = r.bool()?;
            PlayPacket::JoinGame(JoinGame {
                entity_id, game_mode: game_mode_raw & 0x07, hardcore: game_mode_raw & 0x08 != 0,
                dimension, difficulty, max_players, level_type, reduced_debug_info,
            })
        }
        0x02 => PlayPacket::ChatMessage { json: r.string()?, position: r.u8()? },
        0x03 => PlayPacket::TimeUpdate { world_age: r.i64()?, time_of_day: r.i64()? },
        0x05 => {
            let packed = r.i64()?;
            let (x, y, z) = decode_position(packed);
            PlayPacket::SpawnPosition { x, y, z }
        }
        0x06 => PlayPacket::UpdateHealth { health: r.f32()?, food: r.i32()?, food_saturation: r.f32()? },
        0x08 => PlayPacket::PlayerPositionLook {
            x: r.f64()?, y: r.f64()?, z: r.f64()?, yaw: r.f32()?, pitch: r.f32()?, flags: r.u8()?,
        },
        0x21 => {
            let x = r.i32()?;
            let z = r.i32()?;
            let ground_up = r.bool()?;
            let primary_bitmap = u16::from_be_bytes(r.take(2)?.try_into().unwrap());
            let add_bitmap = u16::from_be_bytes(r.take(2)?.try_into().unwrap());
            let (length, used) = decode_varint(r.remaining()).map_err(|e| e.to_string())?;
            if length < 0 || length as usize > 1_048_576 { return Err("invalid chunk data length".into()); }
            // Consume the VarInt prefix and retain the opaque chunk section bytes.
            let body_start = r.offset + used;
            let data = payload.get(id_len + body_start..id_len + body_start + length as usize)
                .ok_or("truncated chunk data payload")?.to_vec();
            r.offset = body_start + length as usize;
            PlayPacket::ChunkData { x, z, ground_up, primary_bitmap, add_bitmap, data }
        }
        other => PlayPacket::Unknown { id: other, payload: r.remaining().to_vec() },
    };
    if matches!(packet, PlayPacket::Unknown { .. }) { return Ok(packet); }
    r.finish()?;
    Ok(packet)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode_varint;

    #[test]
    fn decodes_keep_alive_varint() {\n        let mut p = vec![0x00];\n        encode_varint(300, &mut p);\n        assert_eq!(decode_play_packet(&p).unwrap(), PlayPacket::KeepAlive { id: 300 });\n    }\n\n    #[test]\n    fn decodes_join_game_and_hardcore_flag() {
        let mut p = vec![0x01];
        p.extend_from_slice(&7_i32.to_be_bytes());
        p.push(0x0a); // hardcore + creative
        p.extend_from_slice(&(-1_i8).to_be_bytes());
        p.extend_from_slice(&2_u8.to_be_bytes());
        p.push(20);
        encode_varint(6, &mut p);
        p.extend_from_slice(b"default");
        p.push(1);
        let decoded = decode_play_packet(&p).unwrap();
        assert_eq!(decoded, PlayPacket::JoinGame(JoinGame {
            entity_id: 7, game_mode: 2, hardcore: true, dimension: -1,
            difficulty: 2, max_players: 20, level_type: "default".into(),
            reduced_debug_info: true,
        }));
    }

    #[test]
    fn decodes_relative_player_position_fields() {
        let mut p = vec![0x08];
        p.extend_from_slice(&1.5_f64.to_be_bytes());
        p.extend_from_slice(&64.0_f64.to_be_bytes());
        p.extend_from_slice(&(-2.0_f64).to_be_bytes());
        p.extend_from_slice(&90.0_f32.to_be_bytes());
        p.extend_from_slice(&0.0_f32.to_be_bytes());
        p.push(0x01);
        assert_eq!(decode_play_packet(&p).unwrap(), PlayPacket::PlayerPositionLook {
            x: 1.5, y: 64.0, z: -2.0, yaw: 90.0, pitch: 0.0, flags: 1,
        });
    }

    #[test]
    fn decodes_negative_packed_spawn_position() {
        let pack = (((-12_i64) & 0x3ff_ffff) << 38) | ((70_i64 & 0xfff) << 26) | ((31_i64) & 0x3ff_ffff);
        let mut p = vec![0x05];
        p.extend_from_slice(&pack.to_be_bytes());
        assert_eq!(decode_play_packet(&p).unwrap(), PlayPacket::SpawnPosition { x: -12, y: 70, z: 31 });
    }

    #[test]
    fn rejects_truncated_known_packet() {
        assert!(decode_play_packet(&[0x06, 0, 0]).is_err());
    }

    #[test]
    fn preserves_unknown_packet_payload() {
        assert_eq!(decode_play_packet(&[0x7f, 1, 2]).unwrap(),
            PlayPacket::Unknown { id: 0x7f, payload: vec![1, 2] });
    }
}
