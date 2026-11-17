//! Rendering abstraction.
//!
//! The GPU backend will be introduced after the client shell and protocol
//! foundations are validated. Keeping this crate separate allows profiling
//! and backend changes without coupling rendering to game state.

/// Rendering configuration shared by future GPU backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderConfig {
    pub vsync: bool,
    pub max_fps: u32,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self { vsync: false, max_fps: 0 }
    }
}
