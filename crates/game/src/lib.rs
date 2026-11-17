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
        let Some(chunk) = self.chunks.get_mut(&chunk_pos) else { return false; };
        if chunk.get(x, pos.y as usize, z).0 == 0 { return false; }
        let _ = chunk.set(x, pos.y as usize, z, BlockState(0));
        self.block_count -= 1;
        if chunk.is_empty() { self.chunks.remove(&chunk_pos); }
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
        for x in -4..=4 {
            for z in -4..=4 {
                let height = if (x * 3 + z * 5).rem_euclid(7) == 0 { 2 } else { 1 };
                for y in 0..height { world.set_solid(BlockPos::new(x, y, z)); }
            }
        }
        world
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
        assert_eq!(world.chunk_count(), 4);
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
        for y in 0..3 { for z in -1..=1 { world.set_solid(BlockPos::new(2, y, z)); } }
        let mut player = PlayerPhysics::new([1.0, 0.0, 0.0]);
        for _ in 0..60 { player.tick(&world, [1.0, 0.0], false, 1.0 / 60.0); }
        assert!(player.position()[0] <= 1.4001);
    }

    #[test]
    fn raycast_respects_max_distance_and_invalid_direction() {
        let mut world = World::new();
        world.set_solid(BlockPos::new(4, 0, 0));
        assert!(world.raycast([0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 2.0).is_none());
        assert!(world.raycast([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], 5.0).is_none());
    }
}
