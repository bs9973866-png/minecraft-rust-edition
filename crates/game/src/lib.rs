//! Platform-independent world model for the Minecraft 1.8.9 client.

/// Horizontal chunk coordinate pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkPos { pub x: i32, pub z: i32 }

impl ChunkPos {
    /// Returns the chunk containing a block coordinate, including negatives.
    pub const fn from_block(x: i32, z: i32) -> Self {
        Self { x: x.div_euclid(16), z: z.div_euclid(16) }
    }
}

/// Integer world-space block coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockPos { pub x: i32, pub y: i32, pub z: i32 }

impl BlockPos {
    pub const fn new(x: i32, y: i32, z: i32) -> Self { Self { x, y, z } }
    pub const fn offset(self, normal: [i32; 3]) -> Self {
        Self { x: self.x + normal[0], y: self.y + normal[1], z: self.z + normal[2] }
    }
}

/// Minimal block-state identifier used by the world model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BlockState(pub u16);

impl BlockState {
    /// Store a legacy 1.8 block ID and 4-bit metadata in one u16.
    pub const fn from_legacy(block_id: u16, metadata: u8) -> Self {
        Self((block_id << 4) | (metadata as u16 & 0x0f))
    }

    /// Return the legacy block ID. Values 1..=15 remain compatible with local demo IDs.
    pub const fn block_id(self) -> u16 {
        if self.0 <= 15 { self.0 } else { self.0 >> 4 }
    }

    pub const fn metadata(self) -> u8 {
        if self.0 <= 15 { 0 } else { (self.0 & 0x0f) as u8 }
    }
}

/// Result of tracing a ray into the voxel world.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RayHit {
    pub block: BlockPos,
    /// Face normal pointing from the hit block toward the previous empty cell.
    pub face: [i32; 3],
    pub distance_steps: u32,
}

/// Classic 1.8.9 chunk dimensions.
pub const CHUNK_WIDTH: usize = 16;
pub const CHUNK_HEIGHT: usize = 256;

/// One decoded 16×16×16 section from a protocol-47 chunk column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedChunkSection {
    pub section_y: u8,
    /// Block states in 1.8's YZX storage order: (y << 8) | (z << 4) | x.
    pub blocks: Vec<BlockState>,
}

/// Decoded protocol-47 chunk column. Light arrays are validated but not yet rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedChunkColumn {
    pub x: i32,
    pub z: i32,
    pub ground_up: bool,
    pub section_mask: u16,
    pub sections: Vec<DecodedChunkSection>,
    pub biomes: Option<Vec<u8>>,
}

/// Events delivered from the protocol worker to the render/game thread.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerEvent {
    ChunkColumn(DecodedChunkColumn),
    BlockChanges(Vec<(BlockPos, BlockState)>),
    Teleport { position: [f32; 3], yaw: f32, pitch: f32 },
}

/// A movement update sent from the render thread to the protocol worker.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerMovement {
    pub position: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
}

/// Server-side action requested by the game/render thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DigStatus {
    Start,
    Cancel,
    Finish,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerAction {
    DigBlock {
        position: BlockPos,
        /// Protocol-47 face: 0 bottom, 1 top, 2 north, 3 south, 4 west, 5 east.
        face: u8,
        status: DigStatus,
    },
}

/// Decode the raw S21PacketChunkData data array for protocol 47.
///
/// In 1.8 the data array stores every present section's block states first,
/// then all block-light arrays, then (when present) all sky-light arrays,
/// followed by 256 biome bytes for a ground-up chunk.
pub fn decode_chunk_column(
    x: i32,
    z: i32,
    ground_up: bool,
    section_mask: u16,
    data: &[u8],
    has_skylight: bool,
) -> Result<DecodedChunkColumn, String> {
    let section_ids: Vec<u8> = (0..16)
        .filter(|section| section_mask & (1_u16 << section) != 0)
        .map(|section| section as u8)
        .collect();
    let section_count = section_ids.len();
    let blocks_len = section_count.checked_mul(8192).ok_or("chunk block data size overflow")?;
    let light_len = section_count.checked_mul(2048).ok_or("chunk light data size overflow")?;
    let sky_len = if has_skylight { light_len } else { 0 };
    let biome_len = if ground_up { 256 } else { 0 };
    let expected_len = blocks_len.checked_add(light_len)
        .and_then(|v| v.checked_add(sky_len))
        .and_then(|v| v.checked_add(biome_len))
        .ok_or("chunk data size overflow")?;

    // A full-chunk packet with a zero mask is the 1.8 unload marker.
    if ground_up && section_mask == 0 && data.is_empty() {
        return Ok(DecodedChunkColumn {
            x, z, ground_up, section_mask, sections: Vec::new(), biomes: None,
        });
    }
    if data.len() != expected_len {
        return Err(format!(
            "invalid 1.8 chunk data length: expected {expected_len} bytes for {section_count} sections, got {}",
            data.len()
        ));
    }

    let mut sections = Vec::with_capacity(section_count);
    for (section_index, section_y) in section_ids.iter().copied().enumerate() {
        let start = section_index * 8192;
        let bytes = &data[start..start + 8192];
        let mut blocks = Vec::with_capacity(4096);
        for pair in bytes.chunks_exact(2) {
            let block_id = u16::from(pair[0] >> 4) | (u16::from(pair[1]) << 4);
            let metadata = pair[0] & 0x0f;
            blocks.push(if block_id == 0 {
                BlockState(0)
            } else {
                BlockState::from_legacy(block_id, metadata)
            });
        }
        sections.push(DecodedChunkSection { section_y, blocks });
    }

    let biome_start = blocks_len + light_len + sky_len;
    let biomes = if ground_up {
        Some(data[biome_start..biome_start + 256].to_vec())
    } else {
        None
    };

    Ok(DecodedChunkColumn { x, z, ground_up, section_mask, sections, biomes })
}

/// A sparse 16×256×16 chunk. Only non-air blocks occupy memory.
#[derive(Debug, Default, Clone)]
pub struct Chunk {
    blocks: std::collections::HashMap<usize, BlockState>,
}

impl Chunk {
    fn index(x: usize, y: usize, z: usize) -> usize {
        (y * CHUNK_WIDTH + z) * CHUNK_WIDTH + x
    }

    pub fn get(&self, x: usize, y: usize, z: usize) -> BlockState {
        if x >= CHUNK_WIDTH || z >= CHUNK_WIDTH || y >= CHUNK_HEIGHT {
            return BlockState(0);
        }
        self.blocks.get(&Self::index(x, y, z)).copied().unwrap_or_default()
    }

    pub fn set(&mut self, x: usize, y: usize, z: usize, state: BlockState) -> bool {
        if x >= CHUNK_WIDTH || z >= CHUNK_WIDTH || y >= CHUNK_HEIGHT {
            return false;
        }
        let index = Self::index(x, y, z);
        if state.0 == 0 { self.blocks.remove(&index); }
        else { self.blocks.insert(index, state); }
        true
    }

    pub fn non_air_count(&self) -> usize { self.blocks.len() }
    pub fn is_empty(&self) -> bool { self.blocks.is_empty() }
}

/// Sparse world backed by 16×256×16 chunks. Empty chunks are not retained.
#[derive(Debug, Default, Clone)]
pub struct World {
    chunks: std::collections::HashMap<ChunkPos, Chunk>,
    block_count: usize,
}

impl World {
    pub fn new() -> Self { Self::default() }

    fn chunk_and_local(pos: BlockPos) -> Option<(ChunkPos, usize, usize)> {
        if pos.y < 0 || pos.y >= CHUNK_HEIGHT as i32 { return None; }
        Some((
            ChunkPos::from_block(pos.x, pos.z),
            pos.x.rem_euclid(CHUNK_WIDTH as i32) as usize,
            pos.z.rem_euclid(CHUNK_WIDTH as i32) as usize,
        ))
    }

    /// Remove a chunk column and return the number of stored non-air blocks.
    pub fn unload_chunk(&mut self, pos: ChunkPos) -> usize {
        if let Some(chunk) = self.chunks.remove(&pos) {
            let removed = chunk.non_air_count();
            self.block_count = self.block_count.saturating_sub(removed);
            removed
        } else {
            0
        }
    }

    /// Apply a full column or replace only the sections present in a partial update.
    pub fn apply_chunk_column(&mut self, column: &DecodedChunkColumn) {
        let chunk_pos = ChunkPos { x: column.x, z: column.z };
        if column.ground_up {
            self.unload_chunk(chunk_pos);
        } else {
            // Section updates replace the entire addressed section, including air blocks.
            for section in &column.sections {
                let base_y = i32::from(section.section_y) * 16;
                for local_y in 0..16 {
                    for local_z in 0..16 {
                        for local_x in 0..16 {
                            let pos = BlockPos::new(
                                column.x * 16 + local_x,
                                base_y + local_y,
                                column.z * 16 + local_z,
                            );
                            let _ = self.set_block(pos, BlockState(0));
                        }
                    }
                }
            }
        }

        for section in &column.sections {
            if section.blocks.len() != 4096 { continue; }
            let base_y = i32::from(section.section_y) * 16;
            for (index, state) in section.blocks.iter().copied().enumerate() {
                if state.0 == 0 { continue; }
                let local_x = (index & 0x0f) as i32;
                let local_z = ((index >> 4) & 0x0f) as i32;
                let local_y = ((index >> 8) & 0x0f) as i32;
                let pos = BlockPos::new(
                    column.x * 16 + local_x,
                    base_y + local_y,
                    column.z * 16 + local_z,
                );
                let _ = self.set_block(pos, state);
            }
        }
    }

    pub fn chunk(&self, pos: ChunkPos) -> Option<&Chunk> { self.chunks.get(&pos) }
    pub fn chunk_count(&self) -> usize { self.chunks.len() }
    pub fn chunk_positions(&self) -> impl Iterator<Item = ChunkPos> + '_ { self.chunks.keys().copied() }
    pub fn chunk_blocks(&self, pos: ChunkPos) -> Vec<BlockPos> {
        let Some(chunk) = self.chunks.get(&pos) else { return Vec::new(); };
        chunk.blocks.keys().map(|index| {
            let x = index % CHUNK_WIDTH;
            let z = (index / CHUNK_WIDTH) % CHUNK_WIDTH;
            let y = index / (CHUNK_WIDTH * CHUNK_WIDTH);
            BlockPos::new(
                pos.x * CHUNK_WIDTH as i32 + x as i32,
                y as i32,
                pos.z * CHUNK_WIDTH as i32 + z as i32,
            )
        }).collect()
    }
    pub fn contains(&self, pos: BlockPos) -> bool { self.block(pos).0 != 0 }

    pub fn block(&self, pos: BlockPos) -> BlockState {
        let Some((chunk_pos, x, z)) = Self::chunk_and_local(pos) else { return BlockState(0); };
        self.chunks.get(&chunk_pos)
            .map(|chunk| chunk.get(x, pos.y as usize, z))
            .unwrap_or_default()
    }

    pub fn set_block(&mut self, pos: BlockPos, state: BlockState) -> bool {
        let Some((chunk_pos, x, z)) = Self::chunk_and_local(pos) else { return false; };
        if state.0 == 0 { return self.remove(pos); }
        let chunk = self.chunks.entry(chunk_pos).or_default();
        let previous = chunk.get(x, pos.y as usize, z);
        let _ = chunk.set(x, pos.y as usize, z, state);
        if previous.0 == 0 { self.block_count += 1; }
        true
    }

    pub fn set_solid(&mut self, pos: BlockPos) { let _ = self.set_block(pos, BlockState(1)); }

    pub fn remove(&mut self, pos: BlockPos) -> bool {
        let Some((chunk_pos, x, z)) = Self::chunk_and_local(pos) else { return false; };
        let chunk_is_empty = {
            let Some(chunk) = self.chunks.get_mut(&chunk_pos) else { return false; };
            if chunk.get(x, pos.y as usize, z).0 == 0 { return false; }
            let _ = chunk.set(x, pos.y as usize, z, BlockState(0));
            chunk.is_empty()
        };
        self.block_count -= 1;
        if chunk_is_empty { self.chunks.remove(&chunk_pos); }
        true
    }

    pub fn iter(&self) -> impl Iterator<Item = BlockPos> + '_ {
        self.chunks.iter().flat_map(|(chunk_pos, chunk)| {
            chunk.blocks.iter().map(move |(index, _state)| {
                let x = index % CHUNK_WIDTH;
                let z = (index / CHUNK_WIDTH) % CHUNK_WIDTH;
                let y = index / (CHUNK_WIDTH * CHUNK_WIDTH);
                BlockPos::new(
                    chunk_pos.x * CHUNK_WIDTH as i32 + x as i32,
                    y as i32,
                    chunk_pos.z * CHUNK_WIDTH as i32 + z as i32,
                )
            })
        })
    }

    pub fn len(&self) -> usize { self.block_count }
    pub fn is_empty(&self) -> bool { self.block_count == 0 }

    /// Creates a deterministic 9×9 test island with a few raised blocks.
    pub fn demo() -> Self {
        let mut world = Self::new();
        for x in -4_i32..=4 {
            for z in -4_i32..=4 {
                let height = if (x * 3 + z * 5).rem_euclid(7) == 0 { 2 } else { 1 };
                for y in 0..height { world.set_solid(BlockPos::new(x, y, z)); }
            }
        }
        world
    }

    /// Returns the first air block above the generated terrain at world X/Z.
    pub fn terrain_surface_height(x: i32, z: i32, seed: u64) -> i32 {
        let broad = value_noise(x, z, 96, seed, 0xA0761D6478BD642F);
        let medium = value_noise(x, z, 32, seed, 0xE7037ED1A0B428DB);
        let detail = value_noise(x, z, 12, seed, 0x8EBC6AF09C88C6E3);
        (38.0 + broad * 13.0 + medium * 5.0 + detail * 2.0).round() as i32
    }

    /// Generates a square of deterministic terrain chunks around chunk (0, 0).
    /// Block IDs: 1 stone, 2 grass, 3 dirt, 17 log, 18 leaves.
    pub fn generate_region(seed: u64, chunk_radius: i32) -> Self {
        let mut world = Self::new();
        let radius = chunk_radius.max(0);
        for chunk_x in -radius..=radius {
            for chunk_z in -radius..=radius {
                world.generate_chunk_terrain(ChunkPos { x: chunk_x, z: chunk_z }, seed);
            }
        }
        for chunk_x in -radius..=radius {
            for chunk_z in -radius..=radius {
                world.generate_chunk_trees(ChunkPos { x: chunk_x, z: chunk_z }, seed);
            }
        }
        world
    }

    fn generate_chunk_terrain(&mut self, chunk_pos: ChunkPos, seed: u64) {
        let start_x = chunk_pos.x * CHUNK_WIDTH as i32;
        let start_z = chunk_pos.z * CHUNK_WIDTH as i32;
        for local_x in 0..CHUNK_WIDTH as i32 {
            for local_z in 0..CHUNK_WIDTH as i32 {
                let x = start_x + local_x;
                let z = start_z + local_z;
                let surface = Self::terrain_surface_height(x, z, seed).clamp(4, CHUNK_HEIGHT as i32 - 8);
                for y in 0..surface {
                    let state = if y == surface - 1 { 2 }
                        else if y >= surface - 4 { 3 }
                        else { 1 };
                    self.set_block(BlockPos::new(x, y, z), BlockState(state));
                }
            }
        }
    }

    fn generate_chunk_trees(&mut self, chunk_pos: ChunkPos, seed: u64) {
        let min_x = chunk_pos.x * CHUNK_WIDTH as i32;
        let min_z = chunk_pos.z * CHUNK_WIDTH as i32;
        let max_x = min_x + CHUNK_WIDTH as i32;
        let max_z = min_z + CHUNK_WIDTH as i32;
        // Inspect a two-block border so canopies remain continuous across chunk edges.
        for tree_x in (min_x - 2)..(max_x + 2) {
            for tree_z in (min_z - 2)..(max_z + 2) {
                if coordinate_hash(tree_x, tree_z, seed, 0x589965CC75374CC3) % 113 != 0 { continue; }
                let ground_y = Self::terrain_surface_height(tree_x, tree_z, seed);
                if self.block(BlockPos::new(tree_x, ground_y - 1, tree_z)).0 != 2 { continue; }
                let tree_height = 4 + (coordinate_hash(tree_x, tree_z, seed, 0x1D8E4E27C47D124F) % 3) as i32;
                for dy in 0..tree_height {
                    let y = ground_y + dy;
                    if tree_x >= min_x && tree_x < max_x && tree_z >= min_z && tree_z < max_z {
                        self.set_block(BlockPos::new(tree_x, y, tree_z), BlockState(17));
                    }
                }
                let canopy_y = ground_y + tree_height - 1;
                for dx in -2i32..=2 {
                    for dz in -2i32..=2 {
                        for dy in -1i32..=2 {
                            if dx.abs() + dz.abs() + (dy.max(0)) > 4 { continue; }
                            let x = tree_x + dx;
                            let z = tree_z + dz;
                            if x < min_x || x >= max_x || z < min_z || z >= max_z { continue; }
                            let y = canopy_y + dy;
                            if self.block(BlockPos::new(x, y, z)).0 == 0 {
                                self.set_block(BlockPos::new(x, y, z), BlockState(18));
                            }
                        }
                    }
                }
            }
        }
    }

    /// Voxel DDA raycast. Direction is normalized internally; max_distance is in blocks.
    pub fn raycast(&self, origin: [f32; 3], direction: [f32; 3], max_distance: f32) -> Option<RayHit> {
        if !max_distance.is_finite() || max_distance <= 0.0
            || origin.iter().any(|v| !v.is_finite())
            || direction.iter().any(|v| !v.is_finite()) { return None; }
        let length = (direction[0]*direction[0] + direction[1]*direction[1] + direction[2]*direction[2]).sqrt();
        if length <= f32::EPSILON { return None; }
        let dir = [direction[0]/length, direction[1]/length, direction[2]/length];
        let mut cell = [origin[0].floor() as i32, origin[1].floor() as i32, origin[2].floor() as i32];
        let step = [dir[0].signum() as i32, dir[1].signum() as i32, dir[2].signum() as i32];
        let mut t_delta = [f32::INFINITY; 3];
        let mut t_max = [f32::INFINITY; 3];
        for axis in 0..3 {
            if dir[axis] != 0.0 {
                t_delta[axis] = (1.0 / dir[axis]).abs();
                let boundary = if step[axis] > 0 { cell[axis] as f32 + 1.0 } else { cell[axis] as f32 };
                t_max[axis] = (boundary - origin[axis]) / dir[axis];
            }
        }
        let mut face = [0, 0, 0];
        let mut steps = 0;
        loop {
            let pos = BlockPos::new(cell[0], cell[1], cell[2]);
            if self.contains(pos) { return Some(RayHit { block: pos, face, distance_steps: steps }); }
            let axis = if t_max[0] <= t_max[1] && t_max[0] <= t_max[2] { 0 }
                else if t_max[1] <= t_max[2] { 1 } else { 2 };
            let distance = t_max[axis];
            if !distance.is_finite() || distance > max_distance { return None; }
            cell[axis] += step[axis];
            face = [0, 0, 0];
            face[axis] = -step[axis];
            t_max[axis] += t_delta[axis];
            steps += 1;
            if steps > 4096 { return None; }
        }
    }
}


fn coordinate_hash(x: i32, z: i32, seed: u64, salt: u64) -> u64 {
    let mut value = seed ^ salt ^ (x as i64 as u64).wrapping_mul(0x9E3779B185EBCA87)
        ^ (z as i64 as u64).wrapping_mul(0xC2B2AE3D27D4EB4F);
    value ^= value >> 30;
    value = value.wrapping_mul(0xBF58476D1CE4E5B9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94D049BB133111EB);
    value ^ (value >> 31)
}

fn lattice_value(x: i32, z: i32, seed: u64, salt: u64) -> f32 {
    let value = coordinate_hash(x, z, seed, salt) >> 40;
    (value as f32 / 8_388_607.5) - 1.0
}

fn smooth_curve(value: f32) -> f32 { value * value * (3.0 - 2.0 * value) }

fn value_noise(x: i32, z: i32, scale: i32, seed: u64, salt: u64) -> f32 {
    let scale = scale.max(1);
    let cell_x = x.div_euclid(scale);
    let cell_z = z.div_euclid(scale);
    let local_x = x.rem_euclid(scale) as f32 / scale as f32;
    let local_z = z.rem_euclid(scale) as f32 / scale as f32;
    let tx = smooth_curve(local_x);
    let tz = smooth_curve(local_z);
    let a = lattice_value(cell_x, cell_z, seed, salt);
    let b = lattice_value(cell_x + 1, cell_z, seed, salt);
    let c = lattice_value(cell_x, cell_z + 1, seed, salt);
    let d = lattice_value(cell_x + 1, cell_z + 1, seed, salt);
    let top = a + (b - a) * tx;
    let bottom = c + (d - c) * tx;
    top + (bottom - top) * tz
}

/// Nine-slot quick-access bar. Slot values are block-state IDs; zero is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotbar {
    slots: [BlockState; 9],
    selected: usize,
}

impl Default for Hotbar {
    fn default() -> Self {
        Self { slots: [BlockState(1); 9], selected: 0 }
    }
}

impl Hotbar {
    pub const SLOT_COUNT: usize = 9;
    pub fn selected_index(&self) -> usize { self.selected }
    pub fn selected_block(&self) -> BlockState { self.slots[self.selected] }
    pub fn slot(&self, index: usize) -> Option<BlockState> { self.slots.get(index).copied() }
    pub fn select(&mut self, index: usize) -> bool {
        if index >= Self::SLOT_COUNT { return false; }
        self.selected = index;
        true
    }
    pub fn cycle(&mut self, delta: i32) {
        self.selected = (self.selected as i32 + delta).rem_euclid(Self::SLOT_COUNT as i32) as usize;
    }
    pub fn set_slot(&mut self, index: usize, block: BlockState) -> bool {
        if let Some(slot) = self.slots.get_mut(index) { *slot = block; true } else { false }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PlayerPhysics {
    position: [f32; 3],
    velocity: [f32; 3],
    on_ground: bool,
}

impl PlayerPhysics {
    pub const WIDTH: f32 = 0.6;
    pub const HEIGHT: f32 = 1.8;
    const HALF_WIDTH: f32 = Self::WIDTH * 0.5;

    pub fn new(feet_position: [f32; 3]) -> Self {
        Self { position: feet_position, velocity: [0.0; 3], on_ground: false }
    }
    pub fn position(&self) -> [f32; 3] { self.position }
    pub fn velocity(&self) -> [f32; 3] { self.velocity }
    pub fn on_ground(&self) -> bool { self.on_ground }
    pub fn teleport(&mut self, feet_position: [f32; 3]) {
        self.position = feet_position;
        self.velocity = [0.0; 3];
        self.on_ground = false;
    }

    fn collides(world: &World, p: [f32; 3]) -> bool {
        let min_x = (p[0] - Self::HALF_WIDTH).floor() as i32;
        let max_x = (p[0] + Self::HALF_WIDTH - 0.0001).floor() as i32;
        let min_y = p[1].floor() as i32;
        let max_y = (p[1] + Self::HEIGHT - 0.0001).floor() as i32;
        let min_z = (p[2] - Self::HALF_WIDTH).floor() as i32;
        let max_z = (p[2] + Self::HALF_WIDTH - 0.0001).floor() as i32;
        for x in min_x..=max_x {
            for y in min_y..=max_y {
                for z in min_z..=max_z {
                    if world.contains(BlockPos::new(x, y, z)) { return true; }
                }
            }
        }
        false
    }

    fn move_axis(&mut self, world: &World, axis: usize, amount: f32) {
        if amount == 0.0 { return; }
        let count = (amount.abs() / 0.2).ceil().max(1.0) as usize;
        let step = amount / count as f32;
        for _ in 0..count {
            let mut candidate = self.position;
            candidate[axis] += step;
            if Self::collides(world, candidate) {
                self.velocity[axis] = 0.0;
                if axis == 1 && step < 0.0 { self.on_ground = true; }
                break;
            }
            self.position = candidate;
        }
    }

    pub fn tick(&mut self, world: &World, horizontal: [f32; 2], jump_pressed: bool, dt: f32) {
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.05) } else { 0.0 };
        if dt == 0.0 { return; }
        let mut hx = horizontal[0];
        let mut hz = horizontal[1];
        let length = (hx * hx + hz * hz).sqrt();
        if length > 1.0 { hx /= length; hz /= length; }
        self.velocity[0] = hx * 5.0;
        self.velocity[2] = hz * 5.0;
        if jump_pressed && self.on_ground {
            self.velocity[1] = 8.0;
            self.on_ground = false;
        }
        self.velocity[1] = (self.velocity[1] - 24.0 * dt).max(-50.0);
        self.move_axis(world, 0, self.velocity[0] * dt);
        self.move_axis(world, 2, self.velocity[2] * dt);
        self.on_ground = false;
        self.move_axis(world, 1, self.velocity[1] * dt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_protocol_47_chunk_sections_and_legacy_metadata() {
        let mut data = vec![0_u8; 8192 + 2048 + 2048 + 256];
        // YZX index 0: stone (ID 1, metadata 0).
        data[0] = 0x10;
        // Index 1: wool (ID 35, metadata 14).
        data[2] = 0x3e;
        data[3] = 0x02;
        data[8192 + 2048 + 2048] = 4;

        let decoded = decode_chunk_column(2, -3, true, 1, &data, true).unwrap();
        assert_eq!(decoded.sections.len(), 1);
        assert_eq!(decoded.sections[0].section_y, 0);
        assert_eq!(decoded.sections[0].blocks.len(), 4096);
        assert_eq!(decoded.sections[0].blocks[0].block_id(), 1);
        assert_eq!(decoded.sections[0].blocks[1].block_id(), 35);
        assert_eq!(decoded.sections[0].blocks[1].metadata(), 14);
        assert_eq!(decoded.biomes.as_ref().unwrap()[0], 4);
    }

    #[test]
    fn decodes_extended_12_bit_legacy_block_ids() {
        // 0xABC5 = block ID 0xABC, metadata 5, serialized low byte first.
        let mut data = vec![0_u8; 8192 + 2048];
        data[0] = 0xC5;
        data[1] = 0xAB;
        let decoded = decode_chunk_column(0, 0, false, 1, &data, false).unwrap();
        let state = decoded.sections[0].blocks[0];
        assert_eq!(state.block_id(), 0xABC);
        assert_eq!(state.metadata(), 5);
    }

    #[test]
    fn decodes_nether_chunk_without_skylight() {
        let data = vec![0_u8; 8192 + 2048];
        let decoded = decode_chunk_column(0, 0, false, 1, &data, false).unwrap();
        assert_eq!(decoded.sections[0].blocks.len(), 4096);
        assert!(decoded.biomes.is_none());
    }

    #[test]
    fn rejects_chunk_data_with_wrong_section_length() {
        assert!(decode_chunk_column(0, 0, true, 1, &[0; 8192], true).is_err());
    }

    #[test]
    fn applies_full_and_partial_chunk_updates_to_world() {
        let mut full_data = vec![0_u8; 8192 + 2048 + 2048 + 256];
        full_data[0] = 0x10; // block ID 1 at local (0,0,0)
        let full = decode_chunk_column(1, 0, true, 1, &full_data, true).unwrap();
        let mut world = World::new();
        world.apply_chunk_column(&full);
        assert_eq!(world.block(BlockPos::new(16, 0, 0)).block_id(), 1);

        // A partial section update containing only air must remove that section's old blocks.
        let empty_section = decode_chunk_column(1, 0, false, 1, &vec![0; 8192 + 2048 + 2048], true).unwrap();
        world.apply_chunk_column(&empty_section);
        assert!(world.is_empty());
    }

    #[test]
    fn unloads_a_full_chunk_column() {
        let mut world = World::new();
        world.set_block(BlockPos::new(-1, 20, 3), BlockState(1));
        let unload = decode_chunk_column( -1, 0, true, 0, &[], true).unwrap();
        world.apply_chunk_column(&unload);
        assert!(world.is_empty());
    }

    #[test]
    fn hotbar_selects_and_cycles_with_wraparound() {
        let mut hotbar = Hotbar::default();
        assert_eq!(hotbar.selected_index(), 0);
        assert!(hotbar.select(8));
        assert_eq!(hotbar.selected_index(), 8);
        hotbar.cycle(1);
        assert_eq!(hotbar.selected_index(), 0);
        hotbar.cycle(-1);
        assert_eq!(hotbar.selected_index(), 8);
        assert!(!hotbar.select(9));
    }

    #[test]
    fn hotbar_stores_slot_block_states() {
        let mut hotbar = Hotbar::default();
        assert!(hotbar.set_slot(3, BlockState(5)));
        assert_eq!(hotbar.slot(3), Some(BlockState(5)));
        assert_eq!(hotbar.selected_block(), BlockState(1));
        assert!(!hotbar.set_slot(9, BlockState(2)));
    }

    #[test]
    fn terrain_generation_is_deterministic_and_has_layers() {
        let first = World::generate_region(12345, 0);
        let second = World::generate_region(12345, 0);
        assert_eq!(first.len(), second.len());
        for x in 0..16 {
            for z in 0..16 {
                let surface = World::terrain_surface_height(x, z, 12345);
                assert_eq!(first.block(BlockPos::new(x, surface - 1, z)), BlockState(2));
                assert_eq!(first.block(BlockPos::new(x, surface - 2, z)), BlockState(3));
                assert_eq!(first.block(BlockPos::new(x, surface - 5, z)), BlockState(1));
            }
        }
        assert_eq!(first.iter().collect::<std::collections::HashSet<_>>(),
                   second.iter().collect::<std::collections::HashSet<_>>());
    }

    #[test]
    fn generated_region_populates_expected_chunks_and_tree_blocks() {
        let world = World::generate_region(7, 1);
        assert!(world.chunk_count() >= 9);
        assert!(world.len() > 9 * 16 * 16 * 30);
        assert!(world.iter().any(|pos| world.block(pos).0 == 17));
        assert!(world.iter().any(|pos| world.block(pos).0 == 18));
    }

    #[test]
    fn terrain_height_is_continuous_across_chunk_edges() {
        let seed = 9876;
        let left = World::terrain_surface_height(15, 4, seed);
        let right = World::terrain_surface_height(16, 4, seed);
        assert!((left - right).abs() <= 3);
    }

    #[test]
    fn chunk_coordinates_handle_negative_blocks() {
        assert_eq!(ChunkPos::from_block(-1, -17), ChunkPos { x: -1, z: -2 });
        assert_eq!(ChunkPos::from_block(16, 31), ChunkPos { x: 1, z: 1 });
    }

    #[test]
    fn chunk_storage_handles_boundaries_and_negative_coordinates() {
        let mut world = World::new();
        let positions = [
            BlockPos::new(15, 0, 15),
            BlockPos::new(16, 255, 16),
            BlockPos::new(-1, 64, -1),
            BlockPos::new(-16, 128, -16),
        ];
        for pos in positions { assert!(world.set_block(pos, BlockState(7))); }
        for pos in positions { assert_eq!(world.block(pos), BlockState(7)); }
        assert_eq!(world.chunk_count(), 3);
        assert_eq!(world.len(), 4);
    }

    #[test]
    fn chunk_block_listing_contains_only_that_chunk() {
        let mut world = World::new();
        let a = BlockPos::new(0, 10, 0);
        let b = BlockPos::new(15, 20, 15);
        let c = BlockPos::new(16, 30, 0);
        world.set_solid(a);
        world.set_solid(b);
        world.set_solid(c);
        let mut blocks = world.chunk_blocks(ChunkPos { x: 0, z: 0 });
        blocks.sort_by_key(|p| (p.x, p.y, p.z));
        assert_eq!(blocks, vec![a, b]);
        assert_eq!(world.chunk_blocks(ChunkPos { x: 1, z: 0 }), vec![c]);
    }

    #[test]
    fn chunk_storage_rejects_out_of_height_blocks() {
        let mut world = World::new();
        assert!(!world.set_block(BlockPos::new(0, -1, 0), BlockState(1)));
        assert!(!world.set_block(BlockPos::new(0, 256, 0), BlockState(1)));
        assert_eq!(world.chunk_count(), 0);
        assert!(world.is_empty());
    }

    #[test]
    fn chunk_storage_updates_states_and_reclaims_empty_chunks() {
        let mut world = World::new();
        let pos = BlockPos::new(-17, 42, 32);
        assert!(world.set_block(pos, BlockState(2)));
        assert!(world.set_block(pos, BlockState(3)));
        assert_eq!(world.len(), 1);
        assert_eq!(world.block(pos), BlockState(3));
        assert!(world.remove(pos));
        assert!(world.is_empty());
        assert_eq!(world.chunk_count(), 0);
        assert!(!world.remove(pos));
    }

    #[test]
    fn world_adds_and_removes_blocks() {
        let mut world = World::new();
        let pos = BlockPos::new(1, 2, 3);
        world.set_solid(pos);
        assert!(world.contains(pos));
        assert!(world.remove(pos));
        assert!(!world.contains(pos));
        assert!(!world.remove(pos));
    }

    #[test]
    fn raycast_finds_block_and_hit_face() {
        let mut world = World::new();
        world.set_solid(BlockPos::new(2, 0, 0));
        let hit = world.raycast([0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 5.0).unwrap();
        assert_eq!(hit.block, BlockPos::new(2, 0, 0));
        assert_eq!(hit.face, [-1, 0, 0]);
    }

    #[test]
    fn player_physics_lands_and_stays_on_floor() {
        let world = World::demo();
        let mut player = PlayerPhysics::new([0.5, 5.0, 0.5]);
        for _ in 0..120 { player.tick(&world, [0.0, 0.0], false, 1.0 / 60.0); }
        assert!(player.on_ground());
        assert!((player.position()[1] - 2.0).abs() < 0.01);
    }

    #[test]
    fn player_physics_cannot_walk_through_a_wall() {
        let mut world = World::new();
        for x in -2..=5 { for z in -2..=2 { world.set_solid(BlockPos::new(x, 0, z)); } }
        for y in 1..=3 { for z in -1..=1 { world.set_solid(BlockPos::new(2, y, z)); } }
        let mut player = PlayerPhysics::new([1.0, 1.0, 0.0]);
        for _ in 0..60 { player.tick(&world, [1.0, 0.0], false, 1.0 / 60.0); }
        assert!(player.position()[0] <= 1.7001);
    }

    #[test]
    fn raycast_respects_max_distance_and_invalid_direction() {
        let mut world = World::new();
        world.set_solid(BlockPos::new(4, 0, 0));
        assert!(world.raycast([0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 2.0).is_none());
        assert!(world.raycast([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], 5.0).is_none());
    }
}
