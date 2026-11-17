//! Platform-independent world model for the Minecraft 1.8.9 client.

use std::collections::HashSet;

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

/// Sparse block world. Empty positions are not stored.
#[derive(Debug, Default, Clone)]
pub struct World {
    blocks: HashSet<BlockPos>,
}

impl World {
    pub fn new() -> Self { Self::default() }
    pub fn contains(&self, pos: BlockPos) -> bool { self.blocks.contains(&pos) }
    pub fn set_solid(&mut self, pos: BlockPos) { self.blocks.insert(pos); }
    pub fn remove(&mut self, pos: BlockPos) -> bool { self.blocks.remove(&pos) }
    pub fn iter(&self) -> impl Iterator<Item = BlockPos> + '_ { self.blocks.iter().copied() }
    pub fn len(&self) -> usize { self.blocks.len() }
    pub fn is_empty(&self) -> bool { self.blocks.is_empty() }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_coordinates_handle_negative_blocks() {
        assert_eq!(ChunkPos::from_block(-1, -17), ChunkPos { x: -1, z: -2 });
        assert_eq!(ChunkPos::from_block(16, 31), ChunkPos { x: 1, z: 1 });
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
    fn raycast_respects_max_distance_and_invalid_direction() {
        let mut world = World::new();
        world.set_solid(BlockPos::new(4, 0, 0));
        assert!(world.raycast([0.5, 0.5, 0.5], [1.0, 0.0, 0.0], 2.0).is_none());
        assert!(world.raycast([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], 5.0).is_none());
    }
}
