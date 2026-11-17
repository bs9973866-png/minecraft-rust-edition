//! Sparse world storage for the 16x256x16 chunk layout used by Minecraft 1.8.9.

use std::collections::HashMap;
use crate::math::Vec3;

pub const WORLD_HEIGHT: i32 = 256;
pub const CHUNK_SIZE: i32 = 16;
const SECTION_COUNT: usize = 16;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct BlockPos { pub x: i32, pub y: i32, pub z: i32 }
impl BlockPos { pub const fn new(x: i32, y: i32, z: i32) -> Self { Self { x, y, z } } }

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct ChunkPos { pub x: i32, pub z: i32 }
impl ChunkPos {
    pub const fn new(x: i32, z: i32) -> Self { Self { x, z } }
    pub fn from_block(x: i32, z: i32) -> Self { Self::new(x.div_euclid(CHUNK_SIZE), z.div_euclid(CHUNK_SIZE)) }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BlockState(pub u16);
impl BlockState {
    pub const AIR: Self = Self(0);
    pub const fn is_air(self) -> bool { self.0 == 0 }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChunkSectionData {
    /// 4096 legacy block-state values ((block ID << 4) | metadata).
    pub blocks: Vec<u16>,
    /// Packed 4-bit block light values (2048 bytes).
    pub block_light: Vec<u8>,
    /// Packed 4-bit sky light values, absent in dimensions without a sky.
    pub sky_light: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChunkColumnData {
    pub position: ChunkPos,
    pub ground_up: bool,
    pub unload: bool,
    pub sections: [Option<ChunkSectionData>; 16],
    pub biomes: Option<[u8; 256]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorldError {
    InvalidBlockCount { section: usize, actual: usize },
    InvalidBlockLight { section: usize, actual: usize },
    InvalidSkyLight { section: usize, actual: usize },
}
impl std::fmt::Display for WorldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{self:?}") }
}
impl std::error::Error for WorldError {}

#[derive(Clone, Debug, Default)]
pub struct ChunkSection { blocks: HashMap<u16, BlockState>, block_light: Vec<u8>, sky_light: Option<Vec<u8>> }
impl ChunkSection {
    fn index(local_x: u8, local_y: u8, local_z: u8) -> u16 {
        (u16::from(local_y & 15) << 8) | (u16::from(local_z & 15) << 4) | u16::from(local_x & 15)
    }
    pub fn get(&self, local_x: u8, local_y: u8, local_z: u8) -> BlockState {
        self.blocks.get(&Self::index(local_x, local_y, local_z)).copied().unwrap_or(BlockState::AIR)
    }
    pub fn set(&mut self, local_x: u8, local_y: u8, local_z: u8, state: BlockState) {
        let index = Self::index(local_x, local_y, local_z);
        if state.is_air() { self.blocks.remove(&index); } else { self.blocks.insert(index, state); }
    }
    pub fn is_empty(&self) -> bool { self.blocks.is_empty() && self.block_light.iter().all(|v| *v == 0) && self.sky_light.as_ref().map(|light| light.iter().all(|v| *v == 0)).unwrap_or(true) }
    pub fn non_air_count(&self) -> usize { self.blocks.len() }
}

#[derive(Clone, Debug)]
pub struct Chunk {
    pub position: ChunkPos,
    sections: [Option<ChunkSection>; SECTION_COUNT],
    pub biomes: Option<[u8; 256]>,
}
impl Chunk {
    pub fn new(position: ChunkPos) -> Self {
        Self { position, sections: std::array::from_fn(|_| None), biomes: None }
    }
    pub fn get_block(&self, local_x: u8, y: i32, local_z: u8) -> BlockState {
        if !(0..WORLD_HEIGHT).contains(&y) { return BlockState::AIR; }
        self.sections[(y as usize) >> 4].as_ref()
            .map(|section| section.get(local_x, (y & 15) as u8, local_z))
            .unwrap_or(BlockState::AIR)
    }
    pub fn set_block(&mut self, local_x: u8, y: i32, local_z: u8, state: BlockState) {
        if !(0..WORLD_HEIGHT).contains(&y) { return; }
        let section_index = (y as usize) >> 4;
        if state.is_air() && self.sections[section_index].is_none() { return; }
        let section = self.sections[section_index].get_or_insert_with(ChunkSection::default);
        section.set(local_x, (y & 15) as u8, local_z, state);
        let section_is_empty = section.is_empty();
        if section_is_empty { self.sections[section_index] = None; }
    }
    pub fn non_air_count(&self) -> usize {
        self.sections.iter().flatten().map(ChunkSection::non_air_count).sum()
    }
    pub fn get_block_light(&self, local_x: u8, y: i32, local_z: u8) -> u8 {
        if !(0..WORLD_HEIGHT).contains(&y) { return 0; }
        self.sections[(y as usize) >> 4].as_ref().map(|s| nibble(&s.block_light, local_x, (y & 15) as u8, local_z)).unwrap_or(0)
    }
    pub fn get_sky_light(&self, local_x: u8, y: i32, local_z: u8) -> u8 {
        if !(0..WORLD_HEIGHT).contains(&y) { return 0; }
        self.sections[(y as usize) >> 4].as_ref().and_then(|s| s.sky_light.as_ref().map(|l| nibble(l, local_x, (y & 15) as u8, local_z))).unwrap_or(0)
    }
}

#[derive(Clone, Debug, Default)]
pub struct World { chunks: HashMap<ChunkPos, Chunk> }
impl World {
    pub fn new() -> Self { Self::default() }
    pub fn chunk_count(&self) -> usize { self.chunks.len() }
    pub fn get_chunk(&self, position: ChunkPos) -> Option<&Chunk> { self.chunks.get(&position) }
    pub fn get_block(&self, position: BlockPos) -> BlockState {
        if !(0..WORLD_HEIGHT).contains(&position.y) { return BlockState::AIR; }
        let chunk_pos = ChunkPos::from_block(position.x, position.z);
        let Some(chunk) = self.chunks.get(&chunk_pos) else { return BlockState::AIR; };
        chunk.get_block(position.x.rem_euclid(16) as u8, position.y, position.z.rem_euclid(16) as u8)
    }
    pub fn set_block(&mut self, position: BlockPos, state: BlockState) {
        if !(0..WORLD_HEIGHT).contains(&position.y) { return; }
        let chunk_pos = ChunkPos::from_block(position.x, position.z);
        if state.is_air() && !self.chunks.contains_key(&chunk_pos) { return; }
        let chunk = self.chunks.entry(chunk_pos).or_insert_with(|| Chunk::new(chunk_pos));
        chunk.set_block(position.x.rem_euclid(16) as u8, position.y, position.z.rem_euclid(16) as u8, state);
        let chunk_is_empty = chunk.non_air_count() == 0;
        if chunk_is_empty { self.chunks.remove(&chunk_pos); }
    }
    pub fn remove_chunk(&mut self, position: ChunkPos) -> Option<Chunk> { self.chunks.remove(&position) }
    /// Apply a decoded server column. Full updates replace the column; partial updates keep unmasked sections.
    pub fn apply_chunk_data(&mut self, data: ChunkColumnData) -> Result<(), WorldError> {
        if data.unload { self.chunks.remove(&data.position); return Ok(()); }
        for (index, section) in data.sections.iter().enumerate() {
            if let Some(section) = section {
                if section.blocks.len() != 4096 { return Err(WorldError::InvalidBlockCount { section: index, actual: section.blocks.len() }); }
                if section.block_light.len() != 2048 { return Err(WorldError::InvalidBlockLight { section: index, actual: section.block_light.len() }); }
                if let Some(light) = &section.sky_light {
                    if light.len() != 2048 { return Err(WorldError::InvalidSkyLight { section: index, actual: light.len() }); }
                }
            }
        }
        if data.ground_up { self.chunks.insert(data.position, Chunk::new(data.position)); }
        let chunk = self.chunks.entry(data.position).or_insert_with(|| Chunk::new(data.position));
        for (index, value) in data.sections.into_iter().enumerate() {
            let Some(value) = value else { continue; };
            let mut section = ChunkSection::default();
            for (block_index, state) in value.blocks.into_iter().enumerate() {
                if state != 0 { section.blocks.insert(block_index as u16, BlockState(state)); }
            }
            section.block_light = value.block_light;
            section.sky_light = value.sky_light;
            chunk.sections[index] = if section.is_empty() { None } else { Some(section) };
        }
        if let Some(biomes) = data.biomes { chunk.biomes = Some(biomes); }
        Ok(())
    }

    /// Traverse voxels with a 3D DDA ray and return the first non-air block.
    pub fn raycast(&self, origin: Vec3, direction: Vec3, max_distance: f64) -> Option<RayHit> {
        if !max_distance.is_finite() || max_distance < 0.0 { return None; }
        let direction = direction.normalized();
        if direction.length_squared() <= f64::EPSILON { return None; }
        let mut voxel = [origin.x.floor() as i32, origin.y.floor() as i32, origin.z.floor() as i32];
        let step = [sign(direction.x), sign(direction.y), sign(direction.z)];
        let t_delta = [delta(direction.x), delta(direction.y), delta(direction.z)];
        let mut t_max = [
            first_boundary(origin.x, voxel[0], direction.x, step[0]),
            first_boundary(origin.y, voxel[1], direction.y, step[1]),
            first_boundary(origin.z, voxel[2], direction.z, step[2]),
        ];
        let start = BlockPos::new(voxel[0], voxel[1], voxel[2]);
        let initial = self.get_block(start);
        if !initial.is_air() {
            return Some(RayHit { position: start, state: initial, distance: 0.0, face: None });
        }
        loop {
            let axis = if t_max[0] <= t_max[1] && t_max[0] <= t_max[2] { 0 }
                else if t_max[1] <= t_max[2] { 1 } else { 2 };
            let distance = t_max[axis];
            if !distance.is_finite() || distance > max_distance { return None; }
            voxel[axis] += step[axis];
            t_max[axis] += t_delta[axis];
            let position = BlockPos::new(voxel[0], voxel[1], voxel[2]);
            let state = self.get_block(position);
            if !state.is_air() {
                let mut face = [0_i8; 3];
                face[axis] = (-step[axis]) as i8;
                return Some(RayHit { position, state, distance, face: Some(face) });
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    pub position: BlockPos,
    pub state: BlockState,
    pub distance: f64,
    /// Hit face normal; components are in {-1, 0, 1}.
    pub face: Option<[i8; 3]>,
}
fn nibble(data: &[u8], local_x: u8, local_y: u8, local_z: u8) -> u8 {
    let index = (usize::from(local_y & 15) << 8) | (usize::from(local_z & 15) << 4) | usize::from(local_x & 15);
    let byte = data.get(index >> 1).copied().unwrap_or(0);
    if index & 1 == 0 { byte & 0x0f } else { byte >> 4 }
}

fn sign(value: f64) -> i32 { if value > 0.0 { 1 } else if value < 0.0 { -1 } else { 0 } }
fn delta(component: f64) -> f64 {
    if component.abs() <= f64::EPSILON { f64::INFINITY } else { (1.0 / component).abs() }
}
fn first_boundary(origin: f64, voxel: i32, direction: f64, step: i32) -> f64 {
    if step == 0 { return f64::INFINITY; }
    let boundary = if step > 0 { f64::from(voxel + 1) } else { f64::from(voxel) };
    (boundary - origin) / direction
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn applies_decoded_chunk_and_reads_packed_light_nibbles() {
        let mut blocks = vec![0_u16; 4096];
        blocks[(1 << 8) | (2 << 4) | 3] = 0x1234;
        let mut block_light = vec![0_u8; 2048];
        block_light[0] = 0xA5;
        let mut sky_light = vec![0_u8; 2048];
        sky_light[0] = 0x3C;
        let mut sections: [Option<ChunkSectionData>; 16] = std::array::from_fn(|_| None);
        sections[0] = Some(ChunkSectionData { blocks, block_light, sky_light: Some(sky_light) });
        let mut biomes = [0_u8; 256];
        biomes[0] = 4;
        let mut world = World::new();
        world.apply_chunk_data(ChunkColumnData { position: ChunkPos::new(-1, 2), ground_up: true, unload: false, sections, biomes: Some(biomes) }).unwrap();
        assert_eq!(world.get_block(BlockPos::new(-13, 1, 2)), BlockState(0x1234));
        let chunk = world.get_chunk(ChunkPos::new(-1, 2)).unwrap();
        assert_eq!(chunk.get_block_light(0, 0, 0), 5);
        assert_eq!(chunk.get_block_light(1, 0, 0), 10);
        assert_eq!(chunk.get_sky_light(0, 0, 0), 12);
        assert_eq!(chunk.get_sky_light(1, 0, 0), 3);
        assert_eq!(chunk.biomes.unwrap()[0], 4);
    }

    #[test]
    fn partial_chunk_update_preserves_unmasked_sections() {
        let mut world = World::new();
        world.set_block(BlockPos::new(0, 20, 0), BlockState(1));
        let mut sections: [Option<ChunkSectionData>; 16] = std::array::from_fn(|_| None);
        let mut blocks = vec![0_u16; 4096];
        blocks[0] = 2;
        sections[0] = Some(ChunkSectionData { blocks, block_light: vec![0; 2048], sky_light: None });
        world.apply_chunk_data(ChunkColumnData { position: ChunkPos::new(0, 0), ground_up: false, unload: false, sections, biomes: None }).unwrap();
        assert_eq!(world.get_block(BlockPos::new(0, 0, 0)), BlockState(2));
        assert_eq!(world.get_block(BlockPos::new(0, 20, 0)), BlockState(1));
    }
    #[test]
    fn unload_packet_removes_chunk_column() {
        let mut world = World::new();
        world.set_block(BlockPos::new(64, 32, -32), BlockState(7));
        let data = ChunkColumnData { position: ChunkPos::new(4, -2), ground_up: true, unload: true, sections: std::array::from_fn(|_| None), biomes: None };
        world.apply_chunk_data(data).unwrap();
        assert_eq!(world.chunk_count(), 0);
    }
    #[test]
    fn negative_coordinates_use_floor_chunk_math() {
        assert_eq!(ChunkPos::from_block(-1, -1), ChunkPos::new(-1, -1));
        assert_eq!(ChunkPos::from_block(-16, -16), ChunkPos::new(-1, -1));
        assert_eq!(ChunkPos::from_block(-17, -17), ChunkPos::new(-2, -2));
    }
    #[test]
    fn world_stores_and_reclaims_sparse_chunks() {
        let mut world = World::new();
        let position = BlockPos::new(-17, 64, 31);
        world.set_block(position, BlockState(1));
        assert_eq!(world.chunk_count(), 1);
        assert_eq!(world.get_block(position), BlockState(1));
        world.set_block(position, BlockState::AIR);
        assert_eq!(world.get_block(position), BlockState::AIR);
        assert_eq!(world.chunk_count(), 0);
    }
    #[test]
    fn out_of_height_blocks_are_air() {
        let mut world = World::new();
        world.set_block(BlockPos::new(0, 256, 0), BlockState(1));
        assert_eq!(world.get_block(BlockPos::new(0, 256, 0)), BlockState::AIR);
        assert_eq!(world.chunk_count(), 0);
    }
    #[test]
    fn dda_raycast_finds_first_solid_block_and_face() {
        let mut world = World::new();
        world.set_block(BlockPos::new(3, 64, 0), BlockState(5));
        let hit = world.raycast(Vec3::new(0.5, 64.5, 0.5), Vec3::new(1.0, 0.0, 0.0), 8.0).unwrap();
        assert_eq!(hit.position, BlockPos::new(3, 64, 0));
        assert_eq!(hit.state, BlockState(5));
        assert_eq!(hit.face, Some([-1, 0, 0]));
        assert!((hit.distance - 2.5).abs() < 1.0e-9);
    }
    #[test]
    fn dda_raycast_handles_negative_direction() {
        let mut world = World::new();
        world.set_block(BlockPos::new(-2, 10, 0), BlockState(2));
        let hit = world.raycast(Vec3::new(0.5, 10.5, 0.5), Vec3::new(-1.0, 0.0, 0.0), 8.0).unwrap();
        assert_eq!(hit.position, BlockPos::new(-2, 10, 0));
        assert_eq!(hit.face, Some([1, 0, 0]));
    }
}
