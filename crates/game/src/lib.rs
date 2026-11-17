//! Platform-independent game state for the 1.8.9 client.

/// Horizontal chunk coordinate pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

impl ChunkPos {
    /// Returns the chunk containing a block coordinate, including negatives.
    pub const fn from_block(x: i32, z: i32) -> Self {
        Self { x: x.div_euclid(16), z: z.div_euclid(16) }
    }
}

/// Minimal block-state identifier used by the world model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BlockState(pub u16);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_coordinates_handle_negative_blocks() {
        assert_eq!(ChunkPos::from_block(-1, -17), ChunkPos { x: -1, z: -2 });
        assert_eq!(ChunkPos::from_block(16, 31), ChunkPos { x: 1, z: 1 });
    }
}
