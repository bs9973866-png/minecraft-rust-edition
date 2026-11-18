//! Sparse world storage for the 16x256x16 chunk layout used by Minecraft 1.8.9.

use std::collections::HashMap;
use crate::entity::{EntityEvent, EntityState};
use crate::math::{Aabb, Vec3};

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

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct BlockState(pub u16);
impl BlockState {
    pub const AIR: Self = Self(0);
    pub const fn is_air(self) -> bool { self.0 == 0 }

    /// Legacy numeric block ID (metadata is stored in the low four bits).
    pub const fn block_id(self) -> u16 { self.0 >> 4 }

    /// True for the deliberately supported full-cube collision subset.
    ///
    /// Non-full blocks (slabs, stairs, fences, panes, doors, ladders, fluids,
    /// carpets, plants and similar shapes) require their own collision boxes.
    pub fn has_full_cube_collision(self) -> bool {
        matches!(self.block_id(),
            1 | 2 | 3 | 4 | 5 | 7 | 12 | 13 | 14 | 15 | 16 | 17 |
            19 | 20 | 21 | 22 | 23 | 24 | 25 | 35 | 41 | 42 | 43 |
            45 | 46 | 47 | 48 | 49 | 52 | 56 | 57 | 58 | 60 | 61 | 62 |
            73 | 74 | 79 | 80 | 82 | 84 | 86 | 87 | 89 | 91 | 95 |
            97 | 98 | 99 | 100 | 103 | 110 | 112 | 121 | 123 | 129 |
            133 | 138 | 152 | 153 | 155 | 159 | 162 | 165 | 166 |
            168 | 169 | 170 | 172 | 173 | 174 | 179 | 181
        )
    }

    /// Return this block's supported legacy collision box in world coordinates.
    /// Blocks with connected/rotated shapes are deliberately left for later shape providers.
    pub fn collision_box_at(self, position: BlockPos) -> Option<Aabb> {
        let x = f64::from(position.x);
        let y = f64::from(position.y);
        let z = f64::from(position.z);
        let metadata = (self.0 & 0x0f) as u8;
        let box_from_local = |min: [f64; 3], max: [f64; 3]| {
            Aabb::new(
                Vec3::new(x + min[0], y + min[1], z + min[2]),
                Vec3::new(x + max[0], y + max[1], z + max[2]),
            )
        };

        match self.block_id() {
            // Stone and wood single slabs: metadata bit 3 selects the upper half.
            44 | 126 => {
                let (min_y, max_y) = if metadata & 0x08 != 0 {
                    (0.5, 1.0)
                } else {
                    (0.0, 0.5)
                };
                Some(box_from_local([0.0, min_y, 0.0], [1.0, max_y, 1.0]))
            }
            // Snow layer: legacy metadata 0..=7 represents one through eight layers.
            78 => {
                let height = f64::from(metadata.min(7) + 1) / 8.0;
                Some(box_from_local([0.0, 0.0, 0.0], [1.0, height, 1.0]))
            }
            // Carpet.
            171 => Some(box_from_local([0.0, 0.0, 0.0], [1.0, 0.0625, 1.0])),
            // Soul sand's collision surface is one eighth below the block top.
            88 => Some(box_from_local([0.0, 0.0, 0.0], [1.0, 0.875, 1.0])),
            // Cactus has a one-sixteenth inset on every horizontal side.
            81 => Some(box_from_local([0.0625, 0.0, 0.0625], [0.9375, 1.0, 0.9375])),
            // Cake shrinks from one side as bites are removed.
            92 => {
                let min_x = (1.0 + 2.0 * f64::from(metadata.min(6))) / 16.0;
                Some(box_from_local([min_x, 0.0, 0.0625], [0.9375, 0.5, 0.9375]))
            }
            // Beds use the legacy half-block collision height.
            26 => Some(box_from_local([0.0, 0.0, 0.0], [1.0, 0.5625, 1.0])),
            // Full-cube collision subset (including farmland's full collision AABB in 1.8.9).
            _ if self.has_full_cube_collision() => {
                Some(box_from_local([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]))
            }
            _ => None,
        }
    }

    /// Block slipperiness used by ground movement; ordinary blocks use 0.6.
    pub fn slipperiness(self) -> f64 {
        match self.block_id() {
            79 | 174 => 0.98,
            165 => 0.8,
            _ => 0.6,
        }
    }
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
pub struct World {
    chunks: HashMap<ChunkPos, Chunk>,
    entities: HashMap<i32, EntityState>,
}
impl World {
    pub fn new() -> Self { Self::default() }
    pub fn chunk_count(&self) -> usize { self.chunks.len() }

    /// Return the coordinates of all currently loaded chunk columns.
    pub fn chunk_positions(&self) -> Vec<ChunkPos> {
        self.chunks.keys().copied().collect()
    }

    /// Clear all loaded chunks and tracked entities (for world/dimension changes).
    pub fn clear(&mut self) {
        self.chunks.clear();
        self.entities.clear();
    }
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

    /// Number of currently tracked server entities.
    pub fn entity_count(&self) -> usize { self.entities.len() }

    pub fn get_entity(&self, entity_id: i32) -> Option<&EntityState> { self.entities.get(&entity_id) }

    pub fn clear_entities(&mut self) { self.entities.clear(); }

    /// Advance one simulation/render tick for all tracked entity interpolations.
    pub fn tick_entity_interpolation(&mut self) {
        for entity in self.entities.values_mut() {
            entity.tick_interpolation();
        }
    }

    /// Apply one authoritative entity event from the protocol-47 server stream.
    pub fn apply_entity_event(&mut self, event: EntityEvent) {
        match event {
            EntityEvent::Spawn(entity) => { self.entities.insert(entity.entity_id, entity); }
            EntityEvent::RelativeMove { entity_id, delta, rotation, on_ground } => {
                if let Some(entity) = self.entities.get_mut(&entity_id) {
                    for axis in 0..3 { entity.target_position[axis] += delta[axis]; }
                    entity.position_interpolation_ticks = 3;
                    if let Some((yaw, pitch)) = rotation {
                        entity.target_yaw = yaw;
                        entity.target_pitch = pitch;
                        entity.rotation_interpolation_ticks = 3;
                    }
                    entity.on_ground = on_ground;
                }
            }
            EntityEvent::Teleport { entity_id, position, yaw, pitch, on_ground } => {
                if let Some(entity) = self.entities.get_mut(&entity_id) {
                    entity.position = position;
                    entity.target_position = position;
                    entity.position_interpolation_ticks = 0;
                    entity.yaw = yaw;
                    entity.pitch = pitch;
                    entity.target_yaw = yaw;
                    entity.target_pitch = pitch;
                    entity.rotation_interpolation_ticks = 0;
                    entity.on_ground = on_ground;
                }
            }
            EntityEvent::HeadLook { entity_id, yaw } => {
                if let Some(entity) = self.entities.get_mut(&entity_id) {
                    entity.target_head_yaw = yaw;
                    entity.head_interpolation_ticks = 3;
                }
            }
            EntityEvent::Velocity { entity_id, velocity } => {
                if let Some(entity) = self.entities.get_mut(&entity_id) { entity.velocity = velocity; }
            }
            EntityEvent::Equipment { entity_id, slot, item } => {
                if let Some(entity) = self.entities.get_mut(&entity_id) { entity.equipment.insert(slot, item); }
            }
            EntityEvent::Metadata { entity_id, entries } => {
                if let Some(entity) = self.entities.get_mut(&entity_id) {
                    entity.metadata.extend(entries);
                }
            }
            EntityEvent::Destroy(entity_ids) => {
                for entity_id in entity_ids { self.entities.remove(&entity_id); }
            }
        }
    }
    /// Apply many decoded columns in wire order.
    pub fn apply_chunk_batch<I>(&mut self, chunks: I) -> Result<(), WorldError>
    where
        I: IntoIterator<Item = ChunkColumnData>,
    {
        for chunk in chunks { self.apply_chunk_data(chunk)?; }
        Ok(())
    }

    /// Apply one authoritative block update from the server.
    pub fn apply_block_change(&mut self, position: BlockPos, state: BlockState) {
        self.set_block(position, state);
    }

    /// Apply a batch of authoritative server block updates.
    pub fn apply_block_changes<I>(&mut self, updates: I)
    where
        I: IntoIterator<Item = (BlockPos, BlockState)>,
    {
        for (position, state) in updates { self.set_block(position, state); }
    }
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
    use crate::entity::{EntityKind, EntityMetadataValue};
    #[test]
    fn tracks_spawned_entities_and_applies_server_updates() {
        let mut world = World::new();
        let entity = EntityState {
            entity_id: 77,
            kind: EntityKind::Mob { mob_type: 54 },
            position: [1.0, 2.0, 3.0], target_position: [1.0, 2.0, 3.0],
            position_interpolation_ticks: 0,
            yaw: 350.0, pitch: 0.0, head_yaw: 0.0,
            target_yaw: 350.0, target_pitch: 0.0, target_head_yaw: 0.0,
            rotation_interpolation_ticks: 0, head_interpolation_ticks: 0,
            velocity: [0.0; 3], on_ground: false,
            equipment: std::collections::BTreeMap::new(),
            metadata: std::collections::BTreeMap::new(),
        };
        world.apply_entity_event(EntityEvent::Spawn(entity));
        assert_eq!(world.entity_count(), 1);
        assert_eq!(world.get_entity(77).unwrap().position, [1.0, 2.0, 3.0]);

        world.apply_entity_event(EntityEvent::RelativeMove {
            entity_id: 77, delta: [0.5, -1.0, 2.0], rotation: Some((90.0, 45.0)), on_ground: true
        });
        let entity = world.get_entity(77).unwrap();
        assert_eq!(entity.position, [1.0, 2.0, 3.0]);
        assert_eq!(entity.target_position, [1.5, 1.0, 5.0]);
        assert_eq!((entity.yaw, entity.target_yaw, entity.on_ground), (350.0, 90.0, true));
        world.tick_entity_interpolation();
        let current = world.get_entity(77).unwrap();
        assert!((current.position[0] - (1.0 + 0.5 / 3.0)).abs() < 1.0e-9);
        assert!((current.position[1] - (2.0 - 1.0 / 3.0)).abs() < 1.0e-9);
        assert!((current.position[2] - (3.0 + 2.0 / 3.0)).abs() < 1.0e-9);
        assert!((current.yaw - 383.33333).abs() < 0.001);
        world.tick_entity_interpolation();
        world.tick_entity_interpolation();
        assert_eq!(world.get_entity(77).unwrap().position, [1.5, 1.0, 5.0]);
        assert_eq!((world.get_entity(77).unwrap().yaw, world.get_entity(77).unwrap().pitch), (90.0, 45.0));

        world.apply_entity_event(EntityEvent::HeadLook { entity_id: 77, yaw: 180.0 });
        assert_eq!(world.get_entity(77).unwrap().head_yaw, 0.0);
        world.tick_entity_interpolation();
        world.tick_entity_interpolation();
        world.tick_entity_interpolation();
        assert!((world.get_entity(77).unwrap().head_yaw - 180.0).abs() < 0.001);

        world.apply_entity_event(EntityEvent::Velocity { entity_id: 77, velocity: [1.0, 2.0, -1.0] });
        assert_eq!(world.get_entity(77).unwrap().velocity, [1.0, 2.0, -1.0]);
        world.apply_entity_event(EntityEvent::Equipment { entity_id: 77, slot: 4, item: None });
        assert_eq!(world.get_entity(77).unwrap().equipment.get(&4), Some(&None));
        let mut metadata = std::collections::BTreeMap::new();
        metadata.insert(2, EntityMetadataValue::String("name tag".into()));
        world.apply_entity_event(EntityEvent::Metadata { entity_id: 77, entries: metadata });
        assert_eq!(world.get_entity(77).unwrap().metadata.get(&2), Some(&EntityMetadataValue::String("name tag".into())));

        world.apply_entity_event(EntityEvent::Teleport {
            entity_id: 77, position: [10.0, 20.0, 30.0], yaw: 270.0, pitch: 0.0, on_ground: false
        });
        assert_eq!(world.get_entity(77).unwrap().position, [10.0, 20.0, 30.0]);
        assert_eq!(world.get_entity(77).unwrap().target_position, [10.0, 20.0, 30.0]);
        assert_eq!(world.get_entity(77).unwrap().position_interpolation_ticks, 0);
        world.apply_entity_event(EntityEvent::Destroy(vec![77, 999]));
        assert_eq!(world.entity_count(), 0);
        assert!(world.get_entity(77).is_none());
    }
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
        world.apply_chunk_data(ChunkColumnData { position: ChunkPos::new(-1, 0), ground_up: true, unload: false, sections, biomes: Some(biomes) }).unwrap();
        assert_eq!(world.get_block(BlockPos::new(-13, 1, 2)), BlockState(0x1234));
        let chunk = world.get_chunk(ChunkPos::new(-1, 0)).unwrap();
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
    fn applies_server_block_change_batches() {
        let mut world = World::new();
        world.apply_block_change(BlockPos::new(-1, 64, 2), BlockState(5));
        world.apply_block_changes([
            (BlockPos::new(-1, 64, 2), BlockState::AIR),
            (BlockPos::new(3, 70, 4), BlockState(9)),
        ]);
        assert_eq!(world.get_block(BlockPos::new(-1, 64, 2)), BlockState::AIR);
        assert_eq!(world.get_block(BlockPos::new(3, 70, 4)), BlockState(9));
    }
    #[test]
    fn collision_shapes_cover_slabs_snow_carpet_soul_sand_and_cactus() {
        let position = BlockPos::new(-3, 10, 7);
        let lower_slab = BlockState((44 << 4) | 0).collision_box_at(position).unwrap();
        assert_eq!(lower_slab.min.y, 10.0);
        assert_eq!(lower_slab.max.y, 10.5);

        let upper_slab = BlockState((44 << 4) | 8).collision_box_at(position).unwrap();
        assert_eq!(upper_slab.min.y, 10.5);
        assert_eq!(upper_slab.max.y, 11.0);

        let first_snow_layer = BlockState(78 << 4).collision_box_at(position).unwrap();
        assert!((first_snow_layer.max.y - 10.125).abs() < 1.0e-9);
        let eighth_snow_layer = BlockState((78 << 4) | 7).collision_box_at(position).unwrap();
        assert_eq!(eighth_snow_layer.max.y, 11.0);

        let carpet = BlockState(171 << 4).collision_box_at(position).unwrap();
        assert!((carpet.max.y - 10.0625).abs() < 1.0e-9);
        let soul_sand = BlockState(88 << 4).collision_box_at(position).unwrap();
        assert_eq!(soul_sand.max.y, 10.875);

        let cactus = BlockState(81 << 4).collision_box_at(position).unwrap();
        assert_eq!(cactus.min.x, -2.9375);
        assert_eq!(cactus.max.x, -2.0625);
        assert_eq!(cactus.min.z, 7.0625);
        assert_eq!(cactus.max.z, 7.9375);

        let water = BlockState(9 << 4);
        assert!(water.collision_box_at(position).is_none());
    }

    #[test]
    fn collision_registry_distinguishes_full_blocks_and_partial_blocks() {
        assert!(BlockState(16).has_full_cube_collision());
        assert!(BlockState(60 << 4).has_full_cube_collision());
        assert!(!BlockState(44 << 4).has_full_cube_collision());
        assert!(!BlockState(78 << 4).has_full_cube_collision());
        assert_eq!(BlockState(60 << 4).collision_box_at(BlockPos::new(0, 0, 0)).unwrap().max.y, 1.0);
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
