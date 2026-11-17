//! Decoders for Minecraft Java 1.8.9 (protocol 47) Play-state packets.
//! Unknown packet payloads are preserved so later handlers can be added safely.
use crate::{decode_varint, CodecError};

#[derive(Debug, Clone, PartialEq)]
pub enum PlayPacket {
    JoinGame(JoinGame),
    ChatMessage { json: String, position: u8 },
    TimeUpdate { world_age: i64, time_of_day: i64 },
    SpawnPosition { x: i32, y: i32, z: i32 },
    UpdateHealth { health: f32, food: i32, food_saturation: f32 },
    PlayerPositionLook { x: f64, y: f64, z: f64, yaw: f32, pitch: f32, flags: u8 },
    ChunkData { x: i32, z: i32, ground_up: bool, primary_bitmap: u16, data: Vec<u8> },
    ChunkDataBulk { sky_light: bool, chunks: Vec<BulkChunkColumn> },
    BlockChange { x: i32, y: i32, z: i32, state: u16 },
    MultiBlockChange { chunk_x: i32, chunk_z: i32, changes: Vec<BlockChange> },
    KeepAlive { id: i32 },
    Unknown { id: i32, payload: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulkChunkColumn {
    pub x: i32,
    pub z: i32,
    pub primary_bitmap: u16,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockChange {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// Legacy global block state: (block ID << 4) | metadata.
    pub state: u16,
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
    fn bool(&mut self) -> Result<bool, String> {
        match self.u8()? { 0 => Ok(false), 1 => Ok(true), _ => Err("invalid boolean in play packet".into()) }
    }
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
        let len = self.varint()?;
        if len < 0 || len as usize > 32_767 { return Err("invalid play string length".into()); }
        let bytes = self.take(len as usize)?;
        String::from_utf8(bytes.to_vec()).map_err(|e| format!("invalid UTF-8 in play string: {e}"))
    }
    fn finish(self) -> Result<(), String> {
        if self.offset == self.bytes.len() { Ok(()) } else { Err("unexpected trailing bytes in play packet".into()) }
    }
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
            let x = sign_extend((packed >> 38) as i32, 26);
            let y = sign_extend(((packed >> 26) & 0xfff) as i32, 12);
            let z = sign_extend((packed & 0x03ff_ffff) as i32, 26);
            PlayPacket::SpawnPosition { x, y, z }
        }
        0x06 => PlayPacket::UpdateHealth { health: r.f32()?, food: r.i32()?, food_saturation: r.f32()? },
        0x08 => PlayPacket::PlayerPositionLook {
            x: r.f64()?, y: r.f64()?, z: r.f64()?, yaw: r.f32()?, pitch: r.f32()?, flags: r.u8()?,
        },
        0x26 => {
            let sky_light = r.bool()?;
            let count = r.varint()?;
            if !(0..=1024).contains(&count) {
                return Err("invalid map-chunk-bulk column count".into());
            }
            let mut metadata = Vec::with_capacity(count as usize);
            for _ in 0..count {
                let x = r.i32()?;
                let z = r.i32()?;
                let primary_bitmap = u16::from_be_bytes(r.take(2)?.try_into().unwrap());
                metadata.push((x, z, primary_bitmap));
            }

            let mut chunks = Vec::with_capacity(count as usize);
            for (x, z, primary_bitmap) in metadata {
                let sections = primary_bitmap.count_ones() as usize;
                let section_bytes = 8192_usize
                    .checked_add(2048)
                    .and_then(|size| size.checked_add(if sky_light { 2048 } else { 0 }))
                    .ok_or("bulk chunk section size overflow")?;
                let data_len = sections.checked_mul(section_bytes)
                    .and_then(|size| size.checked_add(256))
                    .ok_or("bulk chunk data size overflow")?;
                if data_len > 1_048_576 {
                    return Err("bulk chunk exceeds safety limit".into());
                }
                chunks.push(BulkChunkColumn {
                    x,
                    z,
                    primary_bitmap,
                    data: r.take(data_len)?.to_vec(),
                });
            }
            PlayPacket::ChunkDataBulk { sky_light, chunks }
        }
        0x22 => {
            let chunk_x = r.i32()?;
            let chunk_z = r.i32()?;
            let count = r.varint()?;
            if !(0..=65_536).contains(&count) {
                return Err("invalid multi-block-change record count".into());
            }
            let mut changes = Vec::with_capacity(count as usize);
            for _ in 0..count {
                let horizontal = r.u8()?;
                let y = i32::from(r.u8()?);
                let state = r.varint()?;
                if !(0..=u16::MAX as i32).contains(&state) {
                    return Err("invalid block state in multi-block change".into());
                }
                changes.push(BlockChange {
                    x: chunk_x * 16 + i32::from(horizontal >> 4),
                    y,
                    z: chunk_z * 16 + i32::from(horizontal & 0x0f),
                    state: state as u16,
                });
            }
            PlayPacket::MultiBlockChange { chunk_x, chunk_z, changes }
        }
        0x23 => {
            let packed = r.i64()?;
            let x = sign_extend((packed >> 38) as i32, 26);
            let y = sign_extend(((packed >> 26) & 0xfff) as i32, 12);
            let z = sign_extend((packed & 0x03ff_ffff) as i32, 26);
            let state = r.varint()?;
            if !(0..=u16::MAX as i32).contains(&state) {
                return Err("invalid block state in block-change packet".into());
            }
            PlayPacket::BlockChange { x, y, z, state: state as u16 }
        }
        0x21 => {
            let x = r.i32()?;
            let z = r.i32()?;
            let ground_up = r.bool()?;
            let primary_bitmap = u16::from_be_bytes(r.take(2)?.try_into().unwrap());
            let length = r.varint()?;
            if length < 0 || length as usize > 1_048_576 { return Err("invalid chunk data length".into()); }
            let data = r.take(length as usize)?.to_vec();
            PlayPacket::ChunkData { x, z, ground_up, primary_bitmap, data }
        }
        other => PlayPacket::Unknown { id: other, payload: r.bytes[r.offset..].to_vec() },
    };
    if matches!(packet, PlayPacket::Unknown { .. }) { return Ok(packet); }
    r.finish()?;
    Ok(packet)
}

fn sign_extend(value: i32, bits: u32) -> i32 {
    let shift = 32 - bits;
    (value << shift) >> shift
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode_varint;

    #[test]
    fn decodes_keep_alive_varint() {
        let mut p = vec![0x00];
        encode_varint(300, &mut p);
        assert_eq!(decode_play_packet(&p).unwrap(), PlayPacket::KeepAlive { id: 300 });
    }

    #[test]
    fn decodes_join_game_and_hardcore_flag() {
        let mut p = vec![0x01];
        p.extend_from_slice(&7_i32.to_be_bytes());
        p.push(0x0a);
        p.push((-1_i8) as u8);
        p.push(2);
        p.push(20);
        encode_varint(7, &mut p);
        p.extend_from_slice(b"default");
        p.push(1);
        assert_eq!(decode_play_packet(&p).unwrap(), PlayPacket::JoinGame(JoinGame {
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
        let packed = (((-12_i64) & 0x3ff_ffff) << 38) | ((70_i64 & 0xfff) << 26) | (31_i64 & 0x3ff_ffff);
        let mut p = vec![0x05];
        p.extend_from_slice(&packed.to_be_bytes());
        assert_eq!(decode_play_packet(&p).unwrap(), PlayPacket::SpawnPosition { x: -12, y: 70, z: 31 });
    }

    #[test]
    fn decodes_chunk_data_without_consuming_a_nonexistent_add_bitmap() {
        let mut p = vec![0x21];
        p.extend_from_slice(&(-2_i32).to_be_bytes());
        p.extend_from_slice(&5_i32.to_be_bytes());
        p.push(1);
        p.extend_from_slice(&3_u16.to_be_bytes());
        encode_varint(4, &mut p);
        p.extend_from_slice(&[9, 8, 7, 6]);
        assert_eq!(decode_play_packet(&p).unwrap(), PlayPacket::ChunkData {
            x: -2, z: 5, ground_up: true, primary_bitmap: 3, data: vec![9, 8, 7, 6],
        });
    }

    #[test]
    fn decodes_bulk_chunk_packet_with_shared_skylight_flag() {
        let mut p = vec![0x26, 1]; // sky light sent
        encode_varint(1, &mut p);
        p.extend_from_slice(&(-4_i32).to_be_bytes());
        p.extend_from_slice(&7_i32.to_be_bytes());
        p.extend_from_slice(&1_u16.to_be_bytes());
        p.extend_from_slice(&vec![0_u8; 8192 + 2048 + 2048 + 256]);
        assert_eq!(decode_play_packet(&p).unwrap(), PlayPacket::ChunkDataBulk {
            sky_light: true,
            chunks: vec![BulkChunkColumn {
                x: -4, z: 7, primary_bitmap: 1,
                data: vec![0; 8192 + 2048 + 2048 + 256],
            }],
        });
    }

    #[test]
    fn decodes_single_block_change_at_negative_coordinates() {
        let packed = (((-12_i64) & 0x3ff_ffff) << 38)
            | ((70_i64 & 0xfff) << 26)
            | ((-31_i64) & 0x3ff_ffff);
        let mut p = vec![0x23];
        p.extend_from_slice(&packed.to_be_bytes());
        encode_varint((35 << 4) | 14, &mut p);
        assert_eq!(decode_play_packet(&p).unwrap(), PlayPacket::BlockChange {
            x: -12, y: 70, z: -31, state: (35 << 4) | 14,
        });
    }

    #[test]
    fn decodes_multi_block_change_records() {
        let mut p = vec![0x22];
        p.extend_from_slice(&(-2_i32).to_be_bytes());
        p.extend_from_slice(&3_i32.to_be_bytes());
        encode_varint(1, &mut p);
        p.push(0xa5); // local x=10, z=5
        p.push(64);
        encode_varint((1 << 4) | 2, &mut p);
        assert_eq!(decode_play_packet(&p).unwrap(), PlayPacket::MultiBlockChange {
            chunk_x: -2,
            chunk_z: 3,
            changes: vec![BlockChange { x: -22, y: 64, z: 53, state: 18 }],
        });
    }

    #[test]
    fn rejects_truncated_known_packet() { assert!(decode_play_packet(&[0x06, 0, 0]).is_err()); }    #[test]
    fn rejects_truncated_known_packet() { assert!(decode_play_packet(&[0x06, 0, 0]).is_err()); }

    #[test]
    fn preserves_unknown_packet_payload() {
        assert_eq!(decode_play_packet(&[0x7f, 1, 2]).unwrap(),
            PlayPacket::Unknown { id: 0x7f, payload: vec![1, 2] });
    }
}
