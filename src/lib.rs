//! Clean-room Rust implementation foundation for the Minecraft Java 1.8.9 protocol.
//!
//! This crate intentionally contains no Mojang game code or proprietary game assets.
//! It provides protocol framing, connection status probing, world storage, and math
//! primitives that can be extended into a complete client.

pub mod entity;
pub mod math;
pub mod mesh;
pub mod nbt;
pub mod physics;
pub mod protocol;
pub mod resource_pack;
pub mod simulation;
pub mod world;

pub const TARGET_VERSION: &str = "1.8.9";
pub const PROTOCOL_VERSION: i32 = 47;
